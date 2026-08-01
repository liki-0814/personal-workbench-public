use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt};

use super::super::state::AppState;
use super::WebState;

#[derive(Clone)]
pub struct DataStore {
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl DataStore {
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&path)
            .with_context(|| format!("open frontend database {}", path.display()))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv_store (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT DEFAULT (datetime('now'))
            );",
        )?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path,
        })
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn all(&self) -> Result<HashMap<String, String>> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut statement = connection.prepare("SELECT key, value FROM kv_store")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_raw(&self, key: &str) -> Result<Option<String>> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        connection
            .query_row("SELECT value FROM kv_store WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn set_raw(&self, key: &str, value: &str) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        connection.execute(
            "INSERT INTO kv_store (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=datetime('now')",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        connection.execute("DELETE FROM kv_store WHERE key=?1", [key])?;
        Ok(())
    }

    pub fn batch(&self, entries: &[(String, String)]) -> Result<()> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let transaction = connection.transaction()?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO kv_store (key, value, updated_at)
                 VALUES (?1, ?2, datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=datetime('now')",
            )?;
            for (key, value) in entries {
                statement.execute(params![key, value])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn backup_to(&self, target: &std::path::Path) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        connection.backup(rusqlite::DatabaseName::Main, target, None)?;
        Ok(())
    }

    pub fn restore_from(&self, source: &std::path::Path) -> Result<()> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        connection.restore(
            rusqlite::DatabaseName::Main,
            source,
            None::<fn(rusqlite::backup::Progress)>,
        )?;
        Ok(())
    }
}

use rusqlite::OptionalExtension;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KvEvent {
    pub id: u64,
    pub keys: Vec<String>,
}

#[derive(Clone)]
pub struct KvEventJournal {
    inner: Arc<Mutex<KvEventJournalInner>>,
    sender: broadcast::Sender<KvEvent>,
    capacity: usize,
}

struct KvEventJournalInner {
    next_id: u64,
    history: VecDeque<KvEvent>,
}

impl KvEventJournal {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(16));
        Self {
            inner: Arc::new(Mutex::new(KvEventJournalInner {
                next_id: 1,
                history: VecDeque::new(),
            })),
            sender,
            capacity,
        }
    }

    pub fn publish(&self, keys: Vec<String>) -> KvEvent {
        let event = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let event = KvEvent {
                id: inner.next_id,
                keys,
            };
            inner.next_id += 1;
            inner.history.push_back(event.clone());
            while inner.history.len() > self.capacity {
                inner.history.pop_front();
            }
            event
        };
        let _ = self.sender.send(event.clone());
        event
    }

    fn after(&self, cursor: u64) -> Vec<KvEvent> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .history
            .iter()
            .filter(|event| event.id > cursor)
            .cloned()
            .collect()
    }

    fn subscribe(&self) -> broadcast::Receiver<KvEvent> {
        self.sender.subscribe()
    }
}

#[derive(Deserialize)]
struct PutValue {
    value: serde_json::Value,
}

#[derive(Deserialize)]
struct BatchRequest {
    entries: Vec<BatchEntry>,
}

#[derive(Deserialize)]
struct BatchEntry {
    key: String,
    value: serde_json::Value,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/data/events", get(events))
        .route("/api/data", get(get_all))
        .route("/api/data/batch", post(batch))
        .route(
            "/api/data/{key}",
            get(get_one).put(put_one).delete(delete_one),
        )
}

fn web(state: &AppState) -> Result<&Arc<WebState>, (StatusCode, Json<serde_json::Value>)> {
    state
        .web
        .as_ref()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Web service is disabled"))
}

async fn get_all(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let data = web(&state)?.data.all().map_err(internal)?;
    Ok(Json(serde_json::json!({ "success": true, "data": data })))
}

async fn get_one(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let web = web(&state)?;
    let value = match web.data.get_raw(&key).map_err(internal)? {
        Some(raw) => serde_json::from_str(&raw).map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Corrupt data: failed to parse JSON",
            )
        })?,
        None => serde_json::Value::Null,
    };
    let value = if key == "ai_providers" {
        super::config::mask_frontend_providers(value)
    } else {
        value
    };
    Ok(Json(serde_json::json!({ "success": true, "data": value })))
}

async fn put_one(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(request): Json<PutValue>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let web = web(&state)?;
    cleanup_removed_chat_session_images(web, &key, &request.value).await?;
    maybe_snapshot_destructive(web, &key, &request.value).map_err(internal)?;
    let value = persist_config_aware(web, &key, request.value)
        .await
        .map_err(internal)?;
    web.data
        .set_raw(&key, &value.to_string())
        .map_err(internal)?;
    web.events.publish(vec![key]);
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn delete_one(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let web = web(&state)?;
    if key == "chat_sessions" {
        cleanup_removed_chat_session_images(web, &key, &serde_json::Value::Array(Vec::new()))
            .await?;
    }
    web.data.delete(&key).map_err(internal)?;
    web.events.publish(vec![key]);
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn batch(
    State(state): State<AppState>,
    Json(request): Json<BatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let web = web(&state)?;
    let mut serialized = Vec::with_capacity(request.entries.len());
    let mut keys = Vec::with_capacity(request.entries.len());
    for entry in request.entries {
        cleanup_removed_chat_session_images(web, &entry.key, &entry.value).await?;
        maybe_snapshot_destructive(web, &entry.key, &entry.value).map_err(internal)?;
        let value = persist_config_aware(web, &entry.key, entry.value)
            .await
            .map_err(internal)?;
        keys.push(entry.key.clone());
        serialized.push((entry.key, value.to_string()));
    }
    web.data.batch(&serialized).map_err(internal)?;
    web.events.publish(keys);
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn cleanup_removed_chat_session_images(
    web: &WebState,
    key: &str,
    incoming: &serde_json::Value,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if key != "chat_sessions" {
        return Ok(());
    }
    let previous = web
        .data
        .get_raw(key)
        .map_err(internal)?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let next_ids = incoming
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|session| session.get("id").and_then(serde_json::Value::as_str))
        .collect::<std::collections::HashSet<_>>();
    let data_dir = web
        .data
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    for session_id in previous
        .iter()
        .filter_map(|session| session.get("id").and_then(serde_json::Value::as_str))
        .filter(|session_id| !next_ids.contains(session_id))
    {
        crate::image_generation::delete_session_artifacts(data_dir, session_id)
            .await
            .map_err(internal)?;
    }
    Ok(())
}

async fn persist_config_aware(
    web: &WebState,
    key: &str,
    value: serde_json::Value,
) -> Result<serde_json::Value> {
    let value = match key {
        "ai_providers" => web.config.write_frontend_providers(value),
        "app_config" => web.config.write_frontend_app_config(value),
        _ => Ok(value),
    }?;
    if key == "app_config" {
        crate::config::local_config::reload().await?;
    }
    Ok(value)
}

fn maybe_snapshot_destructive(
    web: &WebState,
    key: &str,
    incoming: &serde_json::Value,
) -> Result<()> {
    if key != "todos" {
        return Ok(());
    }
    let Some(previous) = web
        .data
        .get_raw(key)?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| value.as_array().cloned())
    else {
        return Ok(());
    };
    let Some(next) = incoming.as_array() else {
        return Ok(());
    };
    let dropped = previous.len().saturating_sub(next.len());
    if dropped < 3 || previous.is_empty() || dropped as f64 / (previous.len() as f64) < 0.3 {
        return Ok(());
    }
    let directory = web
        .data
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("backups");
    std::fs::create_dir_all(&directory)?;
    let filename = format!(
        "pre-write-todos-{}.db",
        chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%6fZ")
    );
    web.data.backup_to(&directory.join(filename))?;
    let mut backups = std::fs::read_dir(&directory)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("pre-write-todos-")
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect::<Vec<_>>();
    backups.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, path) in backups.into_iter().skip(5) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<serde_json::Value>)>
{
    let web = web(&state)?;
    let cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let history = web.events.after(cursor);
    let receiver = web.events.subscribe();
    let history_stream = tokio_stream::iter(history.into_iter().map(event_to_sse));
    let live_stream = tokio_stream::wrappers::BroadcastStream::new(receiver)
        .filter_map(|event| event.ok().map(event_to_sse));
    Ok(Sse::new(history_stream.chain(live_stream)).keep_alive(KeepAlive::default()))
}

fn event_to_sse(event: KvEvent) -> Result<Event, Infallible> {
    Ok(Event::default()
        .id(event.id.to_string())
        .json_data(serde_json::json!({ "keys": event.keys }))
        .expect("serialize KV event"))
}

fn internal(cause: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    error(StatusCode::INTERNAL_SERVER_ERROR, &cause.to_string())
}

fn error(status: StatusCode, message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "success": false, "error": message })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_is_atomic_and_compatible_with_existing_schema() {
        let directory = tempfile::tempdir().unwrap();
        let store = DataStore::open(directory.path().join("data.db")).unwrap();
        store
            .batch(&[
                ("one".into(), "[1]".into()),
                ("two".into(), "{\"x\":2}".into()),
            ])
            .unwrap();
        assert_eq!(store.get_raw("one").unwrap().as_deref(), Some("[1]"));
        assert_eq!(store.all().unwrap().len(), 2);
    }

    #[test]
    fn event_cursor_replays_only_newer_events() {
        let events = KvEventJournal::new(4);
        let first = events.publish(vec!["one".into()]);
        let second = events.publish(vec!["two".into()]);
        assert_eq!(events.after(first.id)[0].id, second.id);
    }
}
