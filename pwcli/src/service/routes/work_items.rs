use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::llm::ChatMessage;
use crate::service::session_manager::SessionManager;
use crate::session::QueuedInputDelivery;

use super::super::state::AppState;

static CAPTURE_LOCK: Mutex<()> = Mutex::new(());
static CAPTURE_SCHEMA_LOCK: Mutex<()> = Mutex::new(());
const ACTIVATION_CLAIM_TTL: chrono::Duration = chrono::Duration::minutes(10);

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutorPreference {
    Auto,
    Pwcli,
    Codex,
    Qoder,
    Kimi,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRequest {
    pub client_request_id: String,
    pub prompt: String,
    pub cwd: String,
    #[serde(default)]
    pub executor_preference: Option<ExecutorPreference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResponse {
    pub client_request_id: String,
    pub work_item_id: String,
    pub session_id: String,
    pub queued_input_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_task_id: Option<String>,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureOperation {
    client_request_id: String,
    prompt: String,
    cwd: String,
    executor_preference: Option<ExecutorPreference>,
    work_item_id: String,
    session_id: String,
    work_item: serde_json::Value,
    status: CaptureStatus,
    queued_input_id: Option<String>,
    runtime_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CaptureStatus {
    #[serde(alias = "prepared")]
    ActivationPending,
    ActivationClaimed,
    #[serde(alias = "complete")]
    Activated,
    Failed,
}

impl CaptureStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::ActivationPending => "activation_pending",
            Self::ActivationClaimed => "activation_claimed",
            Self::Activated => "activated",
            Self::Failed => "failed",
        }
    }
}

impl CaptureOperation {
    fn response(&self, created: bool) -> Result<CaptureResponse, CaptureError> {
        Ok(CaptureResponse {
            client_request_id: self.client_request_id.clone(),
            work_item_id: self.work_item_id.clone(),
            session_id: self.session_id.clone(),
            queued_input_id: self.queued_input_id.clone().ok_or_else(|| {
                CaptureError::Internal(anyhow::anyhow!(
                    "capture operation is missing its queued input"
                ))
            })?,
            runtime_task_id: self.runtime_task_id.clone(),
            created,
        })
    }
}

struct ClaimedCapture {
    operation: CaptureOperation,
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkbenchSnapshot {
    todos: Vec<serde_json::Value>,
    captures: Vec<CaptureOperation>,
    attention: Vec<crate::task::RuntimeAttention>,
}

#[derive(Debug)]
enum CaptureError {
    BadRequest(String),
    Conflict(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for CaptureError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<rusqlite::Error> for CaptureError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<serde_json::Error> for CaptureError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl CaptureError {
    fn into_http(self) -> (StatusCode, String) {
        match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::Internal(error) => {
                tracing::error!(%error, "work-item capture failed");
                (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            }
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/work-items/capture", post(capture))
        .route("/workbench/snapshot", get(snapshot))
}

async fn capture(
    State(state): State<AppState>,
    Json(request): Json<CaptureRequest>,
) -> Result<Json<CaptureResponse>, (StatusCode, String)> {
    let sandbox = crate::tools::fs_local::FsSandbox::from_config()
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let canonical = sandbox
        .resolve_existing_directory(&request.cwd)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let web = state
        .web
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Web service is disabled".to_string()))?;
    let response = capture_in_workspace(
        web.data.path(),
        &state.session_manager,
        &request,
        canonical.clone(),
    )
    .map_err(CaptureError::into_http)?;
    web.events.publish(vec!["todos".to_string()]);
    wake_capture_activation(state, response.client_request_id.clone(), false);
    Ok(Json(response))
}

pub(crate) fn spawn_capture_recovery(state: AppState) {
    let Some(web) = state.web.as_ref() else {
        return;
    };
    let journal = match CaptureJournal::open(web.data.path()) {
        Ok(journal) => journal,
        Err(error) => {
            tracing::warn!(error = %capture_error_message(&error), "failed to open capture journal for recovery");
            return;
        }
    };
    let ids = match journal.recovery_ids(chrono::Utc::now() - ACTIVATION_CLAIM_TTL) {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(error = %capture_error_message(&error), "failed to enumerate capture recovery work");
            return;
        }
    };
    for client_request_id in ids {
        wake_capture_activation(state.clone(), client_request_id, true);
    }
}

fn capture_error_message(error: &CaptureError) -> String {
    match error {
        CaptureError::BadRequest(message) | CaptureError::Conflict(message) => message.clone(),
        CaptureError::Internal(error) => error.to_string(),
    }
}

fn capture_error_to_anyhow(error: CaptureError) -> anyhow::Error {
    match error {
        CaptureError::BadRequest(message) | CaptureError::Conflict(message) => {
            anyhow::anyhow!(message)
        }
        CaptureError::Internal(error) => error,
    }
}

fn wake_capture_activation(
    state: AppState,
    client_request_id: String,
    recover_expired_claim: bool,
) {
    tokio::spawn(async move {
        if let Err(error) =
            activate_capture(&state, &client_request_id, recover_expired_claim).await
        {
            tracing::warn!(%client_request_id, %error, "captured work item activation failed");
        }
    });
}

async fn activate_capture(
    state: &AppState,
    client_request_id: &str,
    recover_expired_claim: bool,
) -> anyhow::Result<()> {
    let web = state.web.as_ref().context("Web service is disabled")?;
    let journal = CaptureJournal::open(web.data.path()).map_err(capture_error_to_anyhow)?;
    let Some(claimed) = journal
        .claim(client_request_id, recover_expired_claim)
        .map_err(capture_error_to_anyhow)?
    else {
        if let Some(operation) = journal
            .operation(client_request_id)
            .map_err(capture_error_to_anyhow)?
            .filter(|operation| operation.status == CaptureStatus::Activated)
        {
            cleanup_activated_queue(&state.session_manager, &operation);
        }
        return Ok(());
    };
    let mut operation = claimed.operation;
    let result = async {
        ensure_claimed_capture_resources(
            &state.session_manager,
            &journal,
            &claimed.token,
            &mut operation,
        )?;
        if let Some(task) = state
            .task_broker
            .task_for_work_item(&operation.work_item_id)?
        {
            return Ok(Some(task.id));
        }
        let session = state
            .session_manager
            .get(&operation.session_id)
            .context("capture session is missing")?;
        let executor = match operation
            .executor_preference
            .unwrap_or(ExecutorPreference::Auto)
        {
            ExecutorPreference::Auto => "auto",
            ExecutorPreference::Pwcli => "pwcli",
            ExecutorPreference::Codex => "codex",
            ExecutorPreference::Qoder => "qoder",
            ExecutorPreference::Kimi => "kimi",
        };
        let _ = super::chat(
            State(state.clone()),
            AxumPath(operation.session_id.clone()),
            Json(super::ChatRequest {
                messages: vec![ChatMessage {
                    role: "user".to_string(),
                    content: operation.prompt.clone(),
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    tool_calls: None,
                    tool_call_id: None,
                }],
                system_prompt: None,
                provider_override: None,
                thinking: false,
                cwd: Some(operation.cwd.clone()),
                session_name: Some(session.name),
                require_permission_approval: false,
                internal_callback: false,
                work_item_id: Some(operation.work_item_id.clone()),
                delegation_preference: Some(executor.to_string()),
            }),
        )
        .await
        .map_err(|status| anyhow::anyhow!("chat activation rejected with {status}"))?;
        state
            .task_broker
            .task_for_work_item(&operation.work_item_id)
            .map(|task| task.map(|task| task.id))
    }
    .await;

    match result {
        Ok(runtime_task_id) => {
            if journal
                .mark_activated(client_request_id, &claimed.token, runtime_task_id)
                .map_err(capture_error_to_anyhow)?
            {
                cleanup_activated_queue(&state.session_manager, &operation);
            }
            Ok(())
        }
        Err(error) => {
            journal
                .mark_failed(client_request_id, &claimed.token, &error.to_string())
                .map_err(capture_error_to_anyhow)?;
            Err(error)
        }
    }
}

fn ensure_claimed_capture_resources(
    session_manager: &SessionManager,
    journal: &CaptureJournal,
    claim_token: &str,
    operation: &mut CaptureOperation,
) -> anyhow::Result<()> {
    let canonical_cwd = PathBuf::from(&operation.cwd);
    session_manager.create_in_workspace_with_id(
        &operation.session_id,
        capture_session_name(&operation.prompt),
        canonical_cwd.clone(),
        canonical_cwd,
    )?;
    session_manager.append_work_item_binding(
        &operation.session_id,
        &operation.work_item_id,
        &operation.session_id,
    )?;
    let queued = session_manager.enqueue_input(
        &operation.session_id,
        format!("capture:{}", operation.client_request_id),
        QueuedInputDelivery::NextTurn,
        ChatMessage {
            role: "user".to_string(),
            content: operation.prompt.clone(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        },
        Vec::new(),
        Vec::new(),
    )?;
    if operation.queued_input_id.as_deref() != Some(queued.id.as_str()) {
        operation.queued_input_id = Some(queued.id);
        operation.updated_at = chrono::Utc::now().to_rfc3339();
        journal
            .update_claimed_operation(operation, claim_token)
            .map_err(capture_error_to_anyhow)?;
    }
    Ok(())
}

fn cleanup_activated_queue(session_manager: &SessionManager, operation: &CaptureOperation) {
    let Some(queued_input_id) = operation.queued_input_id.as_deref() else {
        return;
    };
    match session_manager.delete_queue_item(&operation.session_id, queued_input_id) {
        Ok(()) => {}
        Err(error) if error.to_string().contains("queue item not found") => {}
        Err(error) => tracing::warn!(
            session_id = %operation.session_id,
            queued_input_id,
            %error,
            "activated capture queue cleanup failed"
        ),
    }
}

async fn snapshot(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchSnapshot>, (StatusCode, String)> {
    let web = state
        .web
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Web service is disabled".to_string()))?;
    let journal = CaptureJournal::open(web.data.path()).map_err(CaptureError::into_http)?;
    let mut snapshot = journal.snapshot().map_err(CaptureError::into_http)?;
    snapshot.attention = state
        .task_broker
        .list_attention()
        .map_err(|error| CaptureError::Internal(error).into_http())?
        .into_iter()
        .filter(|item| item.status != "resolved")
        .collect();
    Ok(Json(snapshot))
}

fn capture_in_workspace(
    database_path: &FsPath,
    session_manager: &SessionManager,
    request: &CaptureRequest,
    canonical_cwd: PathBuf,
) -> Result<CaptureResponse, CaptureError> {
    let client_request_id = request.client_request_id.trim();
    let prompt = request.prompt.trim();
    if client_request_id.is_empty() {
        return Err(CaptureError::BadRequest(
            "clientRequestId must not be empty".to_string(),
        ));
    }
    if prompt.is_empty() {
        return Err(CaptureError::BadRequest(
            "prompt must not be empty".to_string(),
        ));
    }

    let _guard = CAPTURE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let journal = CaptureJournal::open(database_path)?;
    let canonical = canonical_cwd.to_string_lossy().into_owned();
    let (mut operation, created) = journal.prepare(request, prompt, &canonical)?;
    if matches!(
        operation.status,
        CaptureStatus::ActivationClaimed | CaptureStatus::Activated
    ) {
        return operation.response(false);
    }

    session_manager.create_in_workspace_with_id(
        &operation.session_id,
        capture_session_name(prompt),
        canonical_cwd,
        PathBuf::from(&request.cwd),
    )?;
    session_manager.append_work_item_binding(
        &operation.session_id,
        &operation.work_item_id,
        &operation.session_id,
    )?;
    let queued = session_manager.enqueue_input(
        &operation.session_id,
        format!("capture:{}", operation.client_request_id),
        QueuedInputDelivery::NextTurn,
        ChatMessage {
            role: "user".to_string(),
            content: operation.prompt.clone(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        },
        Vec::new(),
        Vec::new(),
    )?;
    operation.queued_input_id = Some(queued.id.clone());
    if operation.status != CaptureStatus::ActivationClaimed {
        operation.status = CaptureStatus::ActivationPending;
    }
    operation.updated_at = chrono::Utc::now().to_rfc3339();
    operation.work_item["linkedAgentSessionIds"] =
        serde_json::json!([operation.session_id.clone()]);
    operation.work_item["updatedAt"] = serde_json::json!(operation.updated_at);
    journal.finish(&operation)?;

    operation.response(created)
}

fn capture_session_name(prompt: &str) -> String {
    format!(
        "Capture: {}",
        capture_title(prompt).chars().take(60).collect::<String>()
    )
}

fn capture_title(prompt: &str) -> String {
    prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(prompt)
        .trim()
        .chars()
        .take(80)
        .collect()
}

struct CaptureJournal {
    connection: Mutex<Connection>,
}

impl CaptureJournal {
    fn open(database_path: &FsPath) -> Result<Self, CaptureError> {
        let _schema_guard = CAPTURE_SCHEMA_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let connection = Connection::open(database_path)
            .with_context(|| format!("open capture journal {}", database_path.display()))?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS work_item_capture_operations (
                client_request_id TEXT PRIMARY KEY,
                record TEXT NOT NULL,
                activation_state TEXT NOT NULL DEFAULT 'activation_pending',
                claim_token TEXT,
                claimed_at TEXT,
                last_error TEXT,
                updated_at TEXT NOT NULL
            );",
        )?;
        let had_activation_state = capture_column_exists(&connection, "activation_state")?;
        ensure_capture_column(
            &connection,
            "activation_state",
            "TEXT NOT NULL DEFAULT 'activation_pending'",
        )?;
        ensure_capture_column(&connection, "claim_token", "TEXT")?;
        ensure_capture_column(&connection, "claimed_at", "TEXT")?;
        ensure_capture_column(&connection, "last_error", "TEXT")?;
        if !had_activation_state {
            connection.execute(
                "UPDATE work_item_capture_operations
                 SET activation_state=CASE json_extract(record, '$.status')
                     WHEN 'complete' THEN 'activated'
                     WHEN 'activated' THEN 'activated'
                     WHEN 'activation_claimed' THEN 'activation_claimed'
                     WHEN 'failed' THEN 'failed'
                     ELSE 'activation_pending'
                 END",
                [],
            )?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn prepare(
        &self,
        request: &CaptureRequest,
        prompt: &str,
        canonical_cwd: &str,
    ) -> Result<(CaptureOperation, bool), CaptureError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let transaction = connection.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT record FROM work_item_capture_operations WHERE client_request_id=?1",
                [request.client_request_id.trim()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(raw) = existing {
            let operation: CaptureOperation = serde_json::from_str(&raw)?;
            if operation.prompt != prompt
                || operation.cwd != canonical_cwd
                || operation.executor_preference != request.executor_preference
            {
                return Err(CaptureError::Conflict(
                    "clientRequestId was already used for a different capture request".to_string(),
                ));
            }
            transaction.commit()?;
            return Ok((operation, false));
        }

        let now = chrono::Utc::now().to_rfc3339();
        let work_item_id = format!("todo_{}", uuid::Uuid::now_v7().simple());
        let session_id = format!("sess_capture_{}", uuid::Uuid::now_v7().simple());
        let title = capture_title(prompt);
        let work_item = serde_json::json!({
            "id": work_item_id,
            "title": title,
            "notes": prompt,
            "type": "longterm",
            "completed": false,
            "createdAt": now,
            "updatedAt": now,
            "order": next_todo_order(&transaction)?,
            "priority": "medium",
            "status": "todo",
            "progress": 0,
            "planningState": "inbox",
            "origin": "quick_capture",
            "linkedAgentSessionIds": [],
            "executionWorkspace": {
                "path": canonical_cwd,
                "boundAt": now,
            },
        });
        let operation = CaptureOperation {
            client_request_id: request.client_request_id.trim().to_string(),
            prompt: prompt.to_string(),
            cwd: canonical_cwd.to_string(),
            executor_preference: request.executor_preference,
            work_item_id,
            session_id,
            work_item: work_item.clone(),
            status: CaptureStatus::ActivationPending,
            queued_input_id: None,
            runtime_task_id: None,
            last_error: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let mut todos = read_todos(&transaction)?;
        todos.push(work_item);
        transaction.execute(
            "INSERT INTO kv_store (key, value, updated_at)
             VALUES ('todos', ?1, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=datetime('now')",
            [serde_json::to_string(&todos)?],
        )?;
        transaction.execute(
            "INSERT INTO work_item_capture_operations
             (client_request_id, record, activation_state, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                operation.client_request_id,
                serde_json::to_string(&operation)?,
                operation.status.as_str(),
                operation.updated_at,
            ],
        )?;
        transaction.commit()?;
        Ok((operation, true))
    }

    fn finish(&self, operation: &CaptureOperation) -> Result<(), CaptureError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let transaction = connection.transaction()?;
        let mut todos = read_todos(&transaction)?;
        if let Some(todo) = todos.iter_mut().find(|todo| {
            todo.get("id").and_then(serde_json::Value::as_str)
                == Some(operation.work_item_id.as_str())
        }) {
            todo["linkedAgentSessionIds"] = serde_json::json!([operation.session_id.clone()]);
            todo["updatedAt"] = serde_json::json!(operation.updated_at);
        }
        transaction.execute(
            "INSERT INTO kv_store (key, value, updated_at)
             VALUES ('todos', ?1, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=datetime('now')",
            [serde_json::to_string(&todos)?],
        )?;
        transaction.execute(
            "UPDATE work_item_capture_operations
             SET record=?2, activation_state=?3, claim_token=NULL, claimed_at=NULL,
                 last_error=?4, updated_at=?5
             WHERE client_request_id=?1",
            params![
                operation.client_request_id,
                serde_json::to_string(operation)?,
                operation.status.as_str(),
                operation.last_error,
                operation.updated_at,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn operation(&self, client_request_id: &str) -> Result<Option<CaptureOperation>, CaptureError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        connection
            .query_row(
                "SELECT record FROM work_item_capture_operations WHERE client_request_id=?1",
                [client_request_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|raw| serde_json::from_str(&raw).map_err(CaptureError::from))
            .transpose()
    }

    fn claim(
        &self,
        client_request_id: &str,
        recover_expired_claim: bool,
    ) -> Result<Option<ClaimedCapture>, CaptureError> {
        let _guard = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = transaction
            .query_row(
                "SELECT record, activation_state, claimed_at
                 FROM work_item_capture_operations WHERE client_request_id=?1",
                [client_request_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((raw, state, claimed_at)) = row else {
            transaction.commit()?;
            return Ok(None);
        };
        let stale_claim = recover_expired_claim
            && state == CaptureStatus::ActivationClaimed.as_str()
            && claimed_at
                .as_deref()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .is_none_or(|claimed_at| {
                    claimed_at.with_timezone(&chrono::Utc)
                        <= chrono::Utc::now() - ACTIVATION_CLAIM_TTL
                });
        if state != CaptureStatus::ActivationPending.as_str()
            && state != CaptureStatus::Failed.as_str()
            && !stale_claim
        {
            transaction.commit()?;
            return Ok(None);
        }

        let mut operation: CaptureOperation = serde_json::from_str(&raw)?;
        operation.status = CaptureStatus::ActivationClaimed;
        operation.last_error = None;
        operation.updated_at = chrono::Utc::now().to_rfc3339();
        let token = format!("capture_claim_{}", uuid::Uuid::now_v7().simple());
        let changed = transaction.execute(
            "UPDATE work_item_capture_operations
             SET record=?2, activation_state='activation_claimed', claim_token=?3,
                 claimed_at=?4, last_error=NULL, updated_at=?4
             WHERE client_request_id=?1 AND activation_state=?5",
            params![
                client_request_id,
                serde_json::to_string(&operation)?,
                token,
                operation.updated_at,
                state,
            ],
        )?;
        transaction.commit()?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(Some(ClaimedCapture { operation, token }))
    }

    fn mark_activated(
        &self,
        client_request_id: &str,
        claim_token: &str,
        runtime_task_id: Option<String>,
    ) -> Result<bool, CaptureError> {
        self.settle_claim(
            client_request_id,
            claim_token,
            CaptureStatus::Activated,
            runtime_task_id,
            None,
        )
    }

    fn update_claimed_operation(
        &self,
        operation: &CaptureOperation,
        claim_token: &str,
    ) -> Result<(), CaptureError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let changed = connection.execute(
            "UPDATE work_item_capture_operations SET record=?3, updated_at=?4
             WHERE client_request_id=?1 AND activation_state='activation_claimed'
               AND claim_token=?2",
            params![
                operation.client_request_id,
                claim_token,
                serde_json::to_string(operation)?,
                operation.updated_at,
            ],
        )?;
        if changed != 1 {
            return Err(CaptureError::Internal(anyhow::anyhow!(
                "capture activation claim was lost while provisioning"
            )));
        }
        Ok(())
    }

    fn mark_failed(
        &self,
        client_request_id: &str,
        claim_token: &str,
        error: &str,
    ) -> Result<bool, CaptureError> {
        self.settle_claim(
            client_request_id,
            claim_token,
            CaptureStatus::Failed,
            None,
            Some(error.to_string()),
        )
    }

    fn settle_claim(
        &self,
        client_request_id: &str,
        claim_token: &str,
        status: CaptureStatus,
        runtime_task_id: Option<String>,
        last_error: Option<String>,
    ) -> Result<bool, CaptureError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raw = transaction
            .query_row(
                "SELECT record FROM work_item_capture_operations
                 WHERE client_request_id=?1 AND activation_state='activation_claimed'
                   AND claim_token=?2",
                params![client_request_id, claim_token],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            transaction.commit()?;
            return Ok(false);
        };
        let mut operation: CaptureOperation = serde_json::from_str(&raw)?;
        operation.status = status;
        if runtime_task_id.is_some() {
            operation.runtime_task_id = runtime_task_id;
        }
        operation.last_error = last_error.clone();
        operation.updated_at = chrono::Utc::now().to_rfc3339();
        let changed = transaction.execute(
            "UPDATE work_item_capture_operations
             SET record=?3, activation_state=?4, claim_token=NULL, claimed_at=NULL,
                 last_error=?5, updated_at=?6
             WHERE client_request_id=?1 AND activation_state='activation_claimed'
               AND claim_token=?2",
            params![
                client_request_id,
                claim_token,
                serde_json::to_string(&operation)?,
                status.as_str(),
                last_error,
                operation.updated_at,
            ],
        )?;
        transaction.commit()?;
        Ok(changed == 1)
    }

    fn recovery_ids(
        &self,
        claimed_before: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<String>, CaptureError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut statement = connection.prepare(
            "SELECT client_request_id FROM work_item_capture_operations
             WHERE activation_state IN ('activation_pending', 'failed', 'activated')
                OR (activation_state='activation_claimed'
                    AND (claimed_at IS NULL OR claimed_at<=?1))
             ORDER BY updated_at",
        )?;
        let ids = statement
            .query_map([claimed_before.to_rfc3339()], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(CaptureError::from)?;
        Ok(ids)
    }

    fn snapshot(&self) -> Result<WorkbenchSnapshot, CaptureError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let todos = read_todos(&connection)?;
        let mut statement = connection
            .prepare("SELECT record FROM work_item_capture_operations ORDER BY updated_at DESC")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let captures = rows
            .into_iter()
            .map(|raw| serde_json::from_str(&raw))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkbenchSnapshot {
            todos,
            captures,
            attention: Vec::new(),
        })
    }
}

fn ensure_capture_column(
    connection: &Connection,
    column: &str,
    definition: &str,
) -> Result<(), CaptureError> {
    if !capture_column_exists(connection, column)? {
        connection.execute(
            &format!("ALTER TABLE work_item_capture_operations ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn capture_column_exists(connection: &Connection, column: &str) -> Result<bool, CaptureError> {
    let mut statement = connection.prepare("PRAGMA table_info(work_item_capture_operations)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|existing| existing == column))
}

fn read_todos(connection: &Connection) -> Result<Vec<serde_json::Value>, CaptureError> {
    let raw = connection
        .query_row("SELECT value FROM kv_store WHERE key='todos'", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    match raw {
        Some(raw) => serde_json::from_str(&raw).map_err(|error| {
            CaptureError::Internal(anyhow::Error::new(error).context("decode todos KV data"))
        }),
        None => Ok(Vec::new()),
    }
}

fn next_todo_order(connection: &Connection) -> Result<i64, CaptureError> {
    Ok(read_todos(connection)?
        .iter()
        .filter(|todo| todo.get("completed").and_then(serde_json::Value::as_bool) != Some(true))
        .filter_map(|todo| todo.get("order").and_then(serde_json::Value::as_i64))
        .max()
        .unwrap_or(-1)
        + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::web::DataStore;

    #[test]
    fn capture_is_idempotent_across_todo_session_and_queue() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("data.db");
        let data = DataStore::open(database_path.clone()).unwrap();
        let sessions = SessionManager::new_in(directory.path().join("sessions"));
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let request = CaptureRequest {
            client_request_id: "request-1".to_string(),
            prompt: "Ship durable capture".to_string(),
            cwd: workspace.to_string_lossy().into_owned(),
            executor_preference: Some(ExecutorPreference::Auto),
        };

        let first =
            capture_in_workspace(&database_path, &sessions, &request, workspace.clone()).unwrap();
        drop(sessions);
        let restarted = SessionManager::new_in(directory.path().join("sessions"));
        let second = capture_in_workspace(&database_path, &restarted, &request, workspace).unwrap();

        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.work_item_id, second.work_item_id);
        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.queued_input_id, second.queued_input_id);
        let todos: Vec<serde_json::Value> =
            serde_json::from_str(&data.get_raw("todos").unwrap().unwrap()).unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0]["planningState"], "inbox");
        assert_eq!(todos[0]["origin"], "quick_capture");
        assert_eq!(todos[0]["title"], "Ship durable capture");
        assert_eq!(todos[0]["notes"], "Ship durable capture");
        assert_eq!(todos[0]["linkedAgentSessionIds"][0], first.session_id);
        assert_eq!(restarted.runtime(&first.session_id).unwrap().queue.len(), 1);
        let operation = CaptureJournal::open(&database_path)
            .unwrap()
            .operation("request-1")
            .unwrap()
            .unwrap();
        assert_eq!(operation.status, CaptureStatus::ActivationPending);
    }

    #[test]
    fn capture_title_uses_first_non_empty_line_and_is_bounded() {
        let prompt = format!("\n\n{}\nfull notes", "x".repeat(100));
        assert_eq!(capture_title(&prompt), "x".repeat(80));
    }

    #[test]
    fn repeated_request_id_rejects_changed_payload() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("data.db");
        let _data = DataStore::open(database_path.clone()).unwrap();
        let sessions = SessionManager::new_in(directory.path().join("sessions"));
        let request = CaptureRequest {
            client_request_id: "request-1".to_string(),
            prompt: "first".to_string(),
            cwd: directory.path().to_string_lossy().into_owned(),
            executor_preference: None,
        };
        capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();
        let mut changed = request;
        changed.prompt = "different".to_string();
        let error = capture_in_workspace(
            &database_path,
            &sessions,
            &changed,
            directory.path().to_path_buf(),
        )
        .unwrap_err();
        assert!(matches!(error, CaptureError::Conflict(_)));
    }

    #[test]
    fn pending_capture_survives_restart_and_failed_claim_is_replayable() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("data.db");
        let _data = DataStore::open(database_path.clone()).unwrap();
        let sessions = SessionManager::new_in(directory.path().join("sessions"));
        let request = CaptureRequest {
            client_request_id: "request-crash".to_string(),
            prompt: "Recover me".to_string(),
            cwd: directory.path().to_string_lossy().into_owned(),
            executor_preference: None,
        };
        let response = capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();

        let restarted = CaptureJournal::open(&database_path).unwrap();
        assert_eq!(
            restarted
                .recovery_ids(chrono::Utc::now() - ACTIVATION_CLAIM_TTL)
                .unwrap(),
            vec!["request-crash".to_string()]
        );
        let first_claim = restarted.claim("request-crash", true).unwrap().unwrap();
        assert!(restarted.claim("request-crash", false).unwrap().is_none());
        assert!(restarted
            .mark_failed("request-crash", &first_claim.token, "provider unavailable")
            .unwrap());
        assert_eq!(
            sessions.runtime(&response.session_id).unwrap().queue.len(),
            1
        );

        let replay = capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();
        assert!(!replay.created);
        assert_eq!(replay.queued_input_id, response.queued_input_id);
        assert_eq!(
            sessions.runtime(&response.session_id).unwrap().queue.len(),
            1
        );
        assert!(restarted.claim("request-crash", false).unwrap().is_some());
    }

    #[test]
    fn claimed_capture_repairs_resources_after_prepare_crash() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("data.db");
        let _data = DataStore::open(database_path.clone()).unwrap();
        let sessions = SessionManager::new_in(directory.path().join("sessions"));
        let request = CaptureRequest {
            client_request_id: "request-prepare-crash".to_string(),
            prompt: "Repair resources".to_string(),
            cwd: directory.path().to_string_lossy().into_owned(),
            executor_preference: None,
        };
        let journal = CaptureJournal::open(&database_path).unwrap();
        let (prepared, created) = journal
            .prepare(&request, &request.prompt, &request.cwd)
            .unwrap();
        assert!(created);
        assert!(prepared.queued_input_id.is_none());
        assert!(sessions.get(&prepared.session_id).is_none());

        let mut claimed = journal
            .claim("request-prepare-crash", true)
            .unwrap()
            .unwrap();
        ensure_claimed_capture_resources(
            &sessions,
            &journal,
            &claimed.token,
            &mut claimed.operation,
        )
        .unwrap();

        assert!(sessions.get(&claimed.operation.session_id).is_some());
        assert_eq!(
            sessions
                .runtime(&claimed.operation.session_id)
                .unwrap()
                .queue
                .len(),
            1
        );
        assert!(journal
            .operation("request-prepare-crash")
            .unwrap()
            .unwrap()
            .queued_input_id
            .is_some());
    }

    #[test]
    fn activated_capture_replay_never_reenqueues_or_reclaims() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("data.db");
        let _data = DataStore::open(database_path.clone()).unwrap();
        let sessions = SessionManager::new_in(directory.path().join("sessions"));
        let request = CaptureRequest {
            client_request_id: "request-activated".to_string(),
            prompt: "Run once".to_string(),
            cwd: directory.path().to_string_lossy().into_owned(),
            executor_preference: Some(ExecutorPreference::Codex),
        };
        let first = capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();
        let journal = CaptureJournal::open(&database_path).unwrap();
        let claim = journal.claim("request-activated", false).unwrap().unwrap();
        let replay_while_claimed = capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();
        assert_eq!(replay_while_claimed.queued_input_id, first.queued_input_id);
        assert!(journal
            .mark_activated(
                "request-activated",
                &claim.token,
                Some("task-durable".to_string()),
            )
            .unwrap());
        assert_eq!(sessions.runtime(&first.session_id).unwrap().queue.len(), 1);
        assert!(journal
            .recovery_ids(chrono::Utc::now() - ACTIVATION_CLAIM_TTL)
            .unwrap()
            .contains(&"request-activated".to_string()));
        cleanup_activated_queue(&sessions, &claim.operation);
        assert!(sessions
            .runtime(&first.session_id)
            .unwrap()
            .queue
            .is_empty());

        let replay = capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();
        assert!(!replay.created);
        assert_eq!(replay.runtime_task_id.as_deref(), Some("task-durable"));
        assert!(sessions
            .runtime(&first.session_id)
            .unwrap()
            .queue
            .is_empty());
        assert!(journal.claim("request-activated", false).unwrap().is_none());
    }

    #[test]
    fn expired_claim_is_recovered_and_fences_the_old_owner() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("data.db");
        let _data = DataStore::open(database_path.clone()).unwrap();
        let sessions = SessionManager::new_in(directory.path().join("sessions"));
        let request = CaptureRequest {
            client_request_id: "request-expired".to_string(),
            prompt: "Recover stale owner".to_string(),
            cwd: directory.path().to_string_lossy().into_owned(),
            executor_preference: None,
        };
        capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();
        let journal = CaptureJournal::open(&database_path).unwrap();
        let old_claim = journal.claim("request-expired", false).unwrap().unwrap();
        {
            let connection = journal.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE work_item_capture_operations SET claimed_at=?2
                     WHERE client_request_id=?1",
                    params![
                        "request-expired",
                        (chrono::Utc::now() - ACTIVATION_CLAIM_TTL - chrono::Duration::seconds(1))
                            .to_rfc3339(),
                    ],
                )
                .unwrap();
        }

        let recovered = journal.claim("request-expired", true).unwrap().unwrap();
        assert_ne!(old_claim.token, recovered.token);
        assert!(!journal
            .mark_activated("request-expired", &old_claim.token, None)
            .unwrap());
        assert!(journal
            .mark_activated("request-expired", &recovered.token, None)
            .unwrap());
    }

    #[test]
    fn concurrent_claims_have_exactly_one_winner() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("data.db");
        let _data = DataStore::open(database_path.clone()).unwrap();
        let sessions = SessionManager::new_in(directory.path().join("sessions"));
        let request = CaptureRequest {
            client_request_id: "request-race".to_string(),
            prompt: "Only once".to_string(),
            cwd: directory.path().to_string_lossy().into_owned(),
            executor_preference: None,
        };
        capture_in_workspace(
            &database_path,
            &sessions,
            &request,
            directory.path().to_path_buf(),
        )
        .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let database_path = database_path.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let journal = CaptureJournal::open(&database_path).unwrap();
                barrier.wait();
                journal.claim("request-race", false).unwrap().is_some()
            }));
        }
        barrier.wait();
        let winners = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
    }
}
