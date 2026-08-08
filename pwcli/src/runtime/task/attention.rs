use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAttention {
    pub id: String,
    pub task_id: String,
    pub session_id: String,
    pub work_item_id: Option<String>,
    pub kind: String,
    pub status: String,
    pub revision: u32,
    /// Compatibility alias for the retired Supervisor/TUI attention protocol.
    pub version: u32,
    pub dedupe_key: String,
    pub title: String,
    pub detail: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS runtime_attention (
           id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL REFERENCES runtime_tasks(id) ON DELETE CASCADE,
           session_id TEXT NOT NULL,
           work_item_id TEXT,
           kind TEXT NOT NULL,
           status TEXT NOT NULL CHECK(status IN ('unread','viewed','resolved')),
           revision INTEGER NOT NULL DEFAULT 1,
           dedupe_key TEXT NOT NULL UNIQUE,
           title TEXT NOT NULL,
           detail_json TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS runtime_attention_status_idx
           ON runtime_attention(status, updated_at DESC);
         CREATE INDEX IF NOT EXISTS runtime_attention_session_idx
           ON runtime_attention(session_id, status);
         CREATE INDEX IF NOT EXISTS runtime_attention_work_item_idx
           ON runtime_attention(work_item_id, status);",
    )?;
    Ok(())
}

/// Insert one attention row in the caller's state transition transaction.
/// The stable dedupe key makes callback and outbox replay harmless.
pub(crate) fn create_attention_tx(
    transaction: &Transaction<'_>,
    task_id: &str,
    kind: &str,
    dedupe_key: &str,
    title: &str,
    detail: Value,
) -> Result<Option<String>> {
    let (session_id, work_item_id): (String, Option<String>) = transaction
        .query_row(
            "SELECT task.root_session_id, batch.work_item_id
             FROM runtime_tasks task
             JOIN runtime_batches batch ON batch.id=task.batch_id
             WHERE task.id=?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("runtime task not found while creating attention")?;
    let id = format!("attn_{}", uuid::Uuid::now_v7().simple());
    let now = Utc::now().to_rfc3339();
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO runtime_attention
         (id, task_id, session_id, work_item_id, kind, status, revision,
          dedupe_key, title, detail_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'unread', 1, ?6, ?7, ?8, ?9, ?9)",
        params![
            id,
            task_id,
            session_id,
            work_item_id,
            kind,
            dedupe_key,
            title,
            detail.to_string(),
            now,
        ],
    )?;
    Ok((inserted == 1).then_some(id))
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<RuntimeAttention>> {
    let mut statement = connection.prepare(
        "SELECT id, task_id, session_id, work_item_id, kind, status, revision,
                dedupe_key, title, detail_json, created_at, updated_at
         FROM runtime_attention
         ORDER BY updated_at DESC, id DESC",
    )?;
    let items = statement
        .query_map([], row_to_attention)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(anyhow::Error::from)?;
    Ok(items)
}

pub(crate) fn get(connection: &Connection, id: &str) -> Result<Option<RuntimeAttention>> {
    connection
        .query_row(
            "SELECT id, task_id, session_id, work_item_id, kind, status, revision,
                    dedupe_key, title, detail_json, created_at, updated_at
             FROM runtime_attention WHERE id=?1",
            [id],
            row_to_attention,
        )
        .optional()
        .map_err(Into::into)
}

pub(crate) fn update_status_tx(
    transaction: &Transaction<'_>,
    id: &str,
    status: &str,
    expected_revision: Option<u32>,
) -> Result<Option<(String, u32)>> {
    if !matches!(status, "unread" | "viewed" | "resolved") {
        anyhow::bail!("attention status must be unread, viewed, or resolved");
    }
    let existing = transaction
        .query_row(
            "SELECT task_id, status, revision FROM runtime_attention WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        )
        .optional()?
        .context("attention not found")?;
    if expected_revision.is_some_and(|expected| expected != existing.2) {
        anyhow::bail!(
            "attention revision changed (expected {}, got {})",
            existing.2,
            expected_revision.unwrap_or_default()
        );
    }
    if existing.1 == status || (existing.1 == "resolved" && status != "resolved") {
        return Ok(None);
    }
    let next_revision = existing.2.saturating_add(1);
    transaction.execute(
        "UPDATE runtime_attention
         SET status=?2, revision=?3, updated_at=?4 WHERE id=?1",
        params![id, status, next_revision, Utc::now().to_rfc3339()],
    )?;
    Ok(Some((existing.0, next_revision)))
}

pub(crate) fn resolve_task_tx(
    transaction: &Transaction<'_>,
    task_id: &str,
    kinds: Option<&[&str]>,
) -> Result<usize> {
    let now = Utc::now().to_rfc3339();
    let changed = match kinds {
        None => transaction.execute(
            "UPDATE runtime_attention
             SET status='resolved', revision=revision+1, updated_at=?2
             WHERE task_id=?1 AND status!='resolved'",
            params![task_id, now],
        )?,
        Some([]) => 0,
        Some(kinds) => {
            let placeholders = std::iter::repeat_n("?", kinds.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "UPDATE runtime_attention
                 SET status='resolved', revision=revision+1, updated_at=?2
                 WHERE task_id=?1 AND status!='resolved' AND kind IN ({placeholders})"
            );
            let mut values = vec![
                rusqlite::types::Value::Text(task_id.to_string()),
                rusqlite::types::Value::Text(now),
            ];
            values.extend(
                kinds
                    .iter()
                    .map(|kind| rusqlite::types::Value::Text((*kind).to_string())),
            );
            transaction.execute(&sql, rusqlite::params_from_iter(values))?
        }
    };
    Ok(changed)
}

fn row_to_attention(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeAttention> {
    let revision = row.get::<_, u32>(6)?;
    let detail = row
        .get::<_, String>(9)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null);
    Ok(RuntimeAttention {
        id: row.get(0)?,
        task_id: row.get(1)?,
        session_id: row.get(2)?,
        work_item_id: row.get(3)?,
        kind: row.get(4)?,
        status: row.get(5)?,
        revision,
        version: revision,
        dedupe_key: row.get(7)?,
        title: row.get(8)?,
        detail,
        created_at: super::parse_time(row.get::<_, String>(10)?)?,
        updated_at: super::parse_time(row.get::<_, String>(11)?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_batches (
                   id TEXT PRIMARY KEY,
                   work_item_id TEXT
                 );
                 CREATE TABLE runtime_tasks (
                   id TEXT PRIMARY KEY,
                   batch_id TEXT NOT NULL REFERENCES runtime_batches(id) ON DELETE CASCADE,
                   root_session_id TEXT NOT NULL
                 );
                 INSERT INTO runtime_batches(id, work_item_id) VALUES ('batch-1', 'todo-1');
                 INSERT INTO runtime_tasks(id, batch_id, root_session_id)
                   VALUES ('task-1', 'batch-1', 'session-1');",
            )
            .unwrap();
        initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn attention_is_deduped_and_persists_status_revisions() {
        let mut connection = connection();
        let first_id = {
            let transaction = connection.transaction().unwrap();
            let id = create_attention_tx(
                &transaction,
                "task-1",
                "document_ready",
                "task-1:document:1",
                "产出待处理",
                serde_json::json!({ "documentId": "doc-1" }),
            )
            .unwrap()
            .unwrap();
            transaction.commit().unwrap();
            id
        };
        {
            let transaction = connection.transaction().unwrap();
            assert!(create_attention_tx(
                &transaction,
                "task-1",
                "document_ready",
                "task-1:document:1",
                "重复回放",
                Value::Null,
            )
            .unwrap()
            .is_none());
            transaction.commit().unwrap();
        }
        {
            let transaction = connection.transaction().unwrap();
            assert!(update_status_tx(&transaction, &first_id, "viewed", Some(1))
                .unwrap()
                .is_some());
            transaction.commit().unwrap();
        }

        let items = list(&connection).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].session_id, "session-1");
        assert_eq!(items[0].work_item_id.as_deref(), Some("todo-1"));
        assert_eq!(items[0].status, "viewed");
        assert_eq!(items[0].revision, 2);
        assert_eq!(items[0].version, 2);
    }

    #[test]
    fn resolved_attention_cannot_be_reopened() {
        let mut connection = connection();
        let id = {
            let transaction = connection.transaction().unwrap();
            let id = create_attention_tx(
                &transaction,
                "task-1",
                "task_failed",
                "task-1:failed:1",
                "执行失败",
                Value::Null,
            )
            .unwrap()
            .unwrap();
            transaction.commit().unwrap();
            id
        };
        {
            let transaction = connection.transaction().unwrap();
            assert_eq!(resolve_task_tx(&transaction, "task-1", None).unwrap(), 1);
            transaction.commit().unwrap();
        }
        {
            let transaction = connection.transaction().unwrap();
            assert!(update_status_tx(&transaction, &id, "unread", Some(2))
                .unwrap()
                .is_none());
            transaction.commit().unwrap();
        }
        assert_eq!(get(&connection, &id).unwrap().unwrap().status, "resolved");
    }
}
