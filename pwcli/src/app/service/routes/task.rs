use std::convert::Infallible;
use std::path::Path as FilePath;

use anyhow::Context;
use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;

use crate::ai::llm::ChatMessage;
use crate::runtime::session::SessionState;
use crate::runtime::task::{
    AttemptEventRequest, CompleteAttemptRequest, DelegatedExecutor, RuntimeTaskSpec,
    SubmitChildTaskBatch, SubmitTaskBatch, TaskDecisionRequest, TaskExecutionOutcome,
    TaskReviewRequest,
};

use super::super::state::AppState;
use super::super::state::RuntimeInputActivation;

fn user_message(content: String) -> ChatMessage {
    ChatMessage {
        role: "user".into(),
        content,
        images: Vec::new(),
        generated_images: Vec::new(),
        tool_calls: None,
        tool_call_id: None,
    }
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/runtime/tasks/batch", post(submit_batch))
        .route("/runtime/tasks/{id}/cancel", post(cancel_task))
        .route("/runtime/tasks/{id}/retry", post(retry_task))
        .route("/runtime/tasks/{id}/decision", post(resolve_decision))
        .route("/runtime/tasks/{id}/follow-up", post(follow_up_task))
        .route("/runtime/tasks/{id}/review", post(review_task))
        .route("/runtime/tasks/{id}/review-file", get(review_file))
        .route("/runtime/tasks/{id}/review-diff", get(review_diff))
        .route("/runtime/tasks/{id}/documents", get(task_documents))
        .route("/runtime/snapshot", get(snapshot))
        .route("/runtime/events", get(events))
        .route("/internal/task-attempts/{id}/heartbeat", post(heartbeat))
        .route("/internal/task-attempts/{id}/events", post(attempt_event))
        .route(
            "/internal/task-attempts/{id}/tasks/batch",
            post(submit_child_batch),
        )
        .route("/internal/task-attempts/{id}/complete", post(complete))
}

type ApiError = (axum::http::StatusCode, String);

fn bad_request(error: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::BAD_REQUEST, error.to_string())
}

async fn submit_batch(
    State(state): State<AppState>,
    Json(mut request): Json<SubmitTaskBatch>,
) -> Result<
    (
        axum::http::StatusCode,
        Json<crate::runtime::task::TaskBatchAccepted>,
    ),
    ApiError,
> {
    let session = state
        .session_manager
        .get(&request.root_session_id)
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                "root session not found".into(),
            )
        })?;
    let bound_workspace = session
        .workspace
        .as_ref()
        .map(|workspace| workspace.canonical_path.clone())
        .ok_or_else(|| {
            (
                axum::http::StatusCode::CONFLICT,
                "root session has no bound workspace".into(),
            )
        })?;
    request.session_generation = Some(session.generation);
    state
        .task_broker
        .submit_batch_in_workspace(request, &bound_workspace)
        .map(|accepted| (axum::http::StatusCode::ACCEPTED, Json(accepted)))
        .map_err(bad_request)
}

async fn snapshot(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::runtime::task::RuntimeTaskRecord>>, ApiError> {
    state.task_broker.list().map(Json).map_err(bad_request)
}

async fn cancel_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::runtime::task::RuntimeTaskRecord>, ApiError> {
    state.task_broker.cancel(&id).map(Json).map_err(bad_request)
}

async fn retry_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<crate::runtime::task::RuntimeTaskRecord>, ApiError> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct RetryRequest {
        executor: Option<DelegatedExecutor>,
    }

    let request = if body.iter().all(u8::is_ascii_whitespace) {
        RetryRequest::default()
    } else {
        serde_json::from_slice::<RetryRequest>(&body).map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("invalid retry request: {error}"),
            )
        })?
    };
    let record = state
        .task_broker
        .retry_with_executor(&id, request.executor)
        .map_err(bad_request)?;
    if record.kind == "background_tool" {
        let tool_name = record
            .metadata
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let description = record.objective.clone();
        let arguments = record.metadata.get("toolArguments").cloned();
        let replayable = record
            .metadata
            .get("replayable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !replayable {
            return Err((
                axum::http::StatusCode::CONFLICT,
                "background tool is not replayable".into(),
            ));
        }
        let Some(arguments) = arguments else {
            return Err((
                axum::http::StatusCode::CONFLICT,
                "background tool is not replayable".into(),
            ));
        };
        match state
            .background_tasks
            .spawn_tool_with_runtime(
                record.root_session_id.clone(),
                tool_name,
                description,
                arguments,
                std::sync::Arc::clone(&state.tool_registry),
                record.id.clone(),
            )
            .await
        {
            Ok(new_bg_id) => {
                tracing::info!(
                    runtime_task_id = %record.id,
                    background_task_id = %new_bg_id,
                    "replayed durable background tool after RuntimeTask retry"
                );
            }
            Err(error) => {
                let _ = state.task_broker.complete_background_tool(
                    &record.id,
                    false,
                    &format!("后台重试启动失败: {error}"),
                );
                return Err(bad_request(error));
            }
        }
    }
    // Reload because background spawn may have advanced the durable task.
    state
        .task_broker
        .task(&id)
        .map_err(bad_request)?
        .map(Json)
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                "runtime task not found".into(),
            )
        })
}

async fn resolve_decision(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<TaskDecisionRequest>,
) -> Result<Json<crate::runtime::task::TaskDecisionResponse>, ApiError> {
    state
        .task_broker
        .resolve_decision(&id, request)
        .map(Json)
        .map_err(bad_request)
}

#[derive(Debug, Deserialize)]
struct FollowUpRequest {
    objective: String,
}

async fn follow_up_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<FollowUpRequest>,
) -> Result<
    (
        axum::http::StatusCode,
        Json<crate::runtime::task::TaskBatchAccepted>,
    ),
    ApiError,
> {
    state
        .task_broker
        .follow_up(&id, request.objective)
        .map(|accepted| (axum::http::StatusCode::ACCEPTED, Json(accepted)))
        .map_err(bad_request)
}

async fn review_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<TaskReviewRequest>,
) -> Result<Json<crate::runtime::task::TaskReviewResponse>, ApiError> {
    state
        .task_broker
        .review(&id, request)
        .map(Json)
        .map_err(bad_request)
}

#[derive(Debug, Deserialize)]
struct ReviewFileQuery {
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewFileResponse {
    task_id: String,
    review_revision: u32,
    path: String,
    content: String,
    size: u64,
    is_binary: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewDiffQuery {
    path: String,
    review_revision: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewDiffResponse {
    task_id: String,
    review_revision: u32,
    patch_sha256: String,
    baseline_commit: String,
    history_anomaly: bool,
    status: String,
    files: Vec<crate::runtime::task::worktree::ReviewDiffFile>,
    preview_truncated: bool,
}

fn review_material(
    state: &AppState,
    id: &str,
) -> Result<
    (
        crate::runtime::task::RuntimeTaskRecord,
        crate::runtime::task::worktree::WorktreeLease,
        crate::runtime::task::worktree::ReviewArtifact,
    ),
    ApiError,
> {
    let task = state
        .task_broker
        .task(id)
        .map_err(bad_request)?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                "runtime task not found".into(),
            )
        })?;
    if task.access != "mutating" {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "read-only runtime tasks do not have Git review artifacts".into(),
        ));
    }
    let spec = serde_json::from_value::<RuntimeTaskSpec>(task.metadata.clone())
        .context("runtime task has invalid worktree metadata")
        .map_err(bad_request)?;
    let archived_lease = task
        .metadata
        .get("archivedReviewLease")
        .cloned()
        .map(serde_json::from_value::<crate::runtime::task::worktree::WorktreeLease>)
        .transpose()
        .context("runtime task has invalid archived review metadata")
        .map_err(bad_request)?;
    let lease = spec
        .worktree_lease
        .or(archived_lease)
        .context("runtime task has no isolated or archived Git review")
        .map_err(bad_request)?;
    if lease.task_id != task.id {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "worktree lease does not belong to this runtime task".into(),
        ));
    }
    let artifact = task
        .result
        .clone()
        .and_then(|result| serde_json::from_value::<TaskExecutionOutcome>(result).ok())
        .and_then(|outcome| outcome.review_artifact)
        .context("runtime task has no current review artifact")
        .map_err(bad_request)?;
    Ok((task, lease, artifact))
}

async fn review_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ReviewFileQuery>,
) -> Result<Json<ReviewFileResponse>, ApiError> {
    let (task, lease, artifact) = review_material(&state, &id)?;
    let file = crate::runtime::task::worktree::read_review_text_file(
        &lease,
        &artifact,
        FilePath::new(&query.path),
    )
    .map_err(bad_request)?;
    Ok(Json(ReviewFileResponse {
        task_id: task.id,
        review_revision: task.review_revision,
        path: file.path,
        content: file.content,
        size: file.size,
        is_binary: file.is_binary,
    }))
}

async fn review_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ReviewDiffQuery>,
) -> Result<Json<ReviewDiffResponse>, ApiError> {
    let (task, lease, artifact) = review_material(&state, &id)?;
    if query
        .review_revision
        .is_some_and(|revision| revision != task.review_revision)
    {
        return Err((
            axum::http::StatusCode::CONFLICT,
            "review revision changed; reload the latest review".into(),
        ));
    }
    let file = crate::runtime::task::worktree::read_review_diff_file(
        &lease,
        &artifact,
        FilePath::new(&query.path),
    )
    .map_err(bad_request)?;
    let preview_truncated = file.truncated;
    Ok(Json(ReviewDiffResponse {
        task_id: task.id,
        review_revision: task.review_revision,
        patch_sha256: artifact.patch_sha256,
        baseline_commit: artifact.baseline_commit,
        history_anomaly: false,
        status: task.review_status,
        files: vec![file],
        preview_truncated,
    }))
}

async fn task_documents(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<crate::runtime::documents::DocumentSummary>>, ApiError> {
    let documents = crate::runtime::documents::list(state.task_broker.data_dir())
        .map_err(bad_request)?
        .into_iter()
        .filter(|document| {
            document
                .origin
                .as_ref()
                .is_some_and(|origin| origin.task_id == id)
        })
        .collect();
    Ok(Json(documents))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: u64,
}

async fn events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let replay = state
        .task_broker
        .events_after(query.after)
        .map_err(bad_request)?;
    let mut receiver = state.task_broker.subscribe();
    let stream = async_stream::stream! {
        for event in replay {
            yield Ok(Event::default().id(event.sequence.to_string()).event(&event.kind).json_data(event).unwrap_or_default());
        }
        loop {
            match receiver.recv().await {
                Ok(event) => yield Ok(Event::default().id(event.sequence.to_string()).event(&event.kind).json_data(event).unwrap_or_default()),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatRequest {
    callback_token: String,
    lease_epoch: u64,
}

async fn heartbeat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<HeartbeatRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .task_broker
        .heartbeat(&id, &request.callback_token, request.lease_epoch)
        .map_err(bad_request)?;
    Ok(Json(serde_json::json!({ "accepted": true })))
}

async fn attempt_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AttemptEventRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .task_broker
        .record_attempt_event(&id, request)
        .map_err(bad_request)?;
    Ok(Json(serde_json::json!({ "accepted": true })))
}

async fn submit_child_batch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SubmitChildTaskBatch>,
) -> Result<
    (
        axum::http::StatusCode,
        Json<crate::runtime::task::TaskBatchAccepted>,
    ),
    ApiError,
> {
    state
        .task_broker
        .submit_child_batch(&id, request)
        .map(|accepted| (axum::http::StatusCode::ACCEPTED, Json(accepted)))
        .map_err(bad_request)
}

async fn complete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CompleteAttemptRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let capability_valid = state
        .task_broker
        .callback_capability_valid(&id, &request.callback_token, request.lease_epoch)
        .map_err(bad_request)?;
    let event_id = request.event_id.clone();
    let projection = match state.task_broker.complete(&id, request) {
        Ok(projection) => projection,
        Err(error) if capability_valid => {
            tracing::info!(%id, %error, "discarded fenced RuntimeTask callback");
            let _ = state.task_broker.record_stale_callback(&id, &event_id);
            return Ok(Json(serde_json::json!({
                "accepted": false,
                "stale": true,
            })));
        }
        Err(error) => return Err(bad_request(error)),
    };
    Ok(Json(serde_json::json!({
        "accepted": true,
        "batchCompleted": projection.batch_completed,
    })))
}

/// Start the durable outbox dispatcher. It drains crash-recovered rows before
/// waiting on `Notify`; retries use a single scheduled wake-up instead of
/// polling the database or retaining a parent Agent process.
pub(crate) fn spawn_outbox_dispatcher(state: AppState) {
    spawn_lease_expiry_recovery(state.clone());
    spawn_callback_spool_listener(state.clone());
    let notify = state.task_broker.outbox_notifier();
    tokio::spawn(async move {
        // A crash or an older queue race can leave already-persisted inputs in
        // an idle session. Re-wake the per-session actors once at startup; a
        // paused session intentionally remains user-controlled.
        for (session_id, _) in state.session_manager.list() {
            let Some(runtime) = state.session_manager.runtime(&session_id) else {
                continue;
            };
            if runtime.paused || runtime.phase != "idle" {
                continue;
            }
            for queued in recoverable_idle_inputs(&runtime) {
                let task_ids = if queued.source
                    == crate::runtime::session::QueuedInputSource::RuntimeCallback
                {
                    state
                        .session_manager
                        .runtime_callback_task_ids(&session_id, &queued.client_message_id)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                if let Err(error) = schedule_runtime_input(
                    state.clone(),
                    session_id.clone(),
                    RuntimeInputActivation {
                        queued,
                        task_ids,
                        work_item_id: None,
                    },
                )
                .await
                {
                    tracing::warn!(%error, %session_id, "failed to recover queued runtime input");
                }
            }
        }
        loop {
            loop {
                let item = match state.task_broker.claim_outbox() {
                    Ok(Some(item)) => item,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "failed to claim RuntimeTask outbox item");
                        break;
                    }
                };
                match deliver_outbox(&state, &item).await {
                    Ok(()) => {
                        if let Err(error) = state.task_broker.complete_outbox(&item.id) {
                            tracing::warn!(%error, outbox_id = %item.id, "failed to acknowledge RuntimeTask outbox item");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, outbox_id = %item.id, "RuntimeTask outbox delivery failed");
                        if item.destination == "document" {
                            if let Err(failure) =
                                handle_document_outbox_failure(&state, &item, &error)
                            {
                                tracing::error!(%failure, outbox_id = %item.id, "failed to persist terminal document outbox failure");
                            }
                        } else {
                            let _ = state.task_broker.retry_outbox(&item.id, &error.to_string());
                        }
                    }
                }
            }

            let sleep_until_due =
                state
                    .task_broker
                    .next_outbox_due_at()
                    .ok()
                    .flatten()
                    .map(|due| {
                        let delay = (due - chrono::Utc::now())
                            .to_std()
                            .unwrap_or_else(|_| std::time::Duration::from_millis(10));
                        tokio::time::sleep(delay)
                    });
            if let Some(sleep) = sleep_until_due {
                tokio::select! {
                    _ = notify.notified() => {},
                    _ = sleep => {},
                }
            } else {
                notify.notified().await;
            }
        }
    });
}

fn recoverable_idle_inputs(
    runtime: &crate::runtime::session::SessionRuntimeSnapshot,
) -> Vec<crate::runtime::session::QueuedInput> {
    if runtime.paused || runtime.phase != "idle" || runtime.active_turn_id.is_some() {
        return Vec::new();
    }
    runtime
        .queue
        .iter()
        .filter(|queued| {
            queued.delivery == crate::runtime::session::QueuedInputDelivery::NextTurn
                && queued.status == crate::runtime::session::QueuedInputStatus::Queued
        })
        .cloned()
        .collect()
}

const MAX_DOCUMENT_OUTBOX_RETRIES: u32 = 3;

fn is_transient_document_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
            )
        }) || cause
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|sqlite| {
                matches!(
                    sqlite,
                    rusqlite::Error::SqliteFailure(details, _)
                        if matches!(
                            details.code,
                            rusqlite::ErrorCode::DatabaseBusy
                                | rusqlite::ErrorCode::DatabaseLocked
                        )
                )
            })
    })
}

fn should_retry_document_outbox(attempt_count: u32, error: &anyhow::Error) -> bool {
    attempt_count < MAX_DOCUMENT_OUTBOX_RETRIES && is_transient_document_error(error)
}

fn handle_document_outbox_failure(
    state: &AppState,
    item: &crate::runtime::task::TaskOutboxRecord,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    if should_retry_document_outbox(item.attempt_count, error) {
        return state.task_broker.retry_outbox(&item.id, &error.to_string());
    }
    let task_id = item
        .payload
        .get("taskId")
        .and_then(serde_json::Value::as_str);
    let attempt_id = item
        .payload
        .get("attemptId")
        .and_then(serde_json::Value::as_str);
    match (task_id, attempt_id) {
        (Some(task_id), Some(attempt_id)) => state.task_broker.fail_document_outbox(
            &item.id,
            task_id,
            attempt_id,
            &error.to_string(),
        ),
        _ => state.task_broker.fail_outbox(&item.id, &error.to_string()),
    }
}

fn spawn_callback_spool_listener(state: AppState) {
    #[cfg(unix)]
    tokio::spawn(async move {
        use std::os::unix::fs::PermissionsExt;

        let socket_path = state
            .task_broker
            .data_dir()
            .join("task-callback-spool.notify.sock");
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }
        let socket = match tokio::net::UnixDatagram::bind(&socket_path) {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(%error, path = %socket_path.display(), "failed to bind RuntimeTask spool event listener");
                return;
            }
        };
        if let Err(error) =
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        {
            tracing::warn!(%error, path = %socket_path.display(), "failed to restrict RuntimeTask spool listener permissions");
        }
        let mut buffer = [0_u8; 64];
        loop {
            match socket.recv(&mut buffer).await {
                Ok(_) => {
                    let broker = std::sync::Arc::clone(&state.task_broker);
                    match tokio::task::spawn_blocking(move || broker.import_callback_spool()).await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "failed to import event-triggered RuntimeTask callback spool");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "RuntimeTask callback spool importer stopped unexpectedly");
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "RuntimeTask callback spool listener stopped");
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(socket_path);
    });

    #[cfg(not(unix))]
    let _ = state;
}

fn spawn_lease_expiry_recovery(state: AppState) {
    let notify = state.task_broker.lease_notifier();
    tokio::spawn(async move {
        loop {
            if let Err(error) = state.task_broker.recover_expired_attempts() {
                tracing::warn!(%error, "failed to recover expired RuntimeTask leases");
            }
            let next_expiry = state.task_broker.next_lease_expiry().ok().flatten();
            if let Some(expiry) = next_expiry {
                let delay = (expiry - chrono::Utc::now())
                    .to_std()
                    .unwrap_or_else(|_| std::time::Duration::from_millis(10));
                tokio::select! {
                    _ = notify.notified() => {},
                    _ = tokio::time::sleep(delay) => {},
                }
            } else {
                notify.notified().await;
            }
        }
    });
}

async fn deliver_outbox(
    state: &AppState,
    item: &crate::runtime::task::TaskOutboxRecord,
) -> anyhow::Result<()> {
    if item.destination == "document" {
        let string = |key: &str| {
            item.payload
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("document outbox is missing {key}"))
        };
        let task_id = string("taskId")?;
        let attempt_id = string("attemptId")?;
        let payload = crate::runtime::documents::create_delegated_markdown_or_diagnostic(
            state.task_broker.data_dir(),
            &string("title")?,
            &string("body")?,
            crate::runtime::documents::DocumentOrigin {
                origin_type: crate::runtime::documents::DocumentOriginType::RuntimeTask,
                task_id: task_id.clone(),
                attempt_id: attempt_id.clone(),
                batch_id: string("batchId")?,
                executor_id: string("executorId")?,
                agent_name: string("agentName")?,
            },
        )?;
        state
            .task_broker
            .mark_document_ready(&task_id, &attempt_id, &payload.manifest.id)?;
        return Ok(());
    }
    if item.destination == "review_projection" {
        apply_review_projection(state, &item.payload)?;
        return Ok(());
    }
    if let Some(parent_task_id) = item.destination.strip_prefix("parent_task:") {
        state
            .task_broker
            .resume_parent_from_children(parent_task_id, &item.payload)?;
        return Ok(());
    }
    let callback = item
        .payload
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("runtime callback outbox is missing summary"))?;
    let task_ids = item
        .payload
        .get("taskIds")
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut callback_payload = item.payload.clone();
    if let serde_json::Value::Object(object) = &mut callback_payload {
        object.insert(
            "dedupeKey".to_string(),
            serde_json::Value::String(item.dedupe_key.clone()),
        );
    }
    project_callback(
        state.clone(),
        item.destination.clone(),
        callback.to_string(),
        item.dedupe_key.clone(),
        task_ids,
        callback_payload,
    )
    .await
}

fn apply_review_projection(state: &AppState, payload: &serde_json::Value) -> anyhow::Result<()> {
    if let Some(update) = payload.get("taskUpdate").filter(|update| !update.is_null()) {
        let web = state
            .web
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Web data store is unavailable"))?;
        let mut todos = web
            .data
            .get_raw("todos")?
            .map(|raw| serde_json::from_str::<Vec<serde_json::Value>>(&raw))
            .transpose()?
            .unwrap_or_default();
        let todo_id = update
            .get("todoId")
            .or_else(|| update.get("id"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("taskUpdate.todoId is required"))?;
        let todo = todos
            .iter_mut()
            .find(|todo| todo.get("id").and_then(serde_json::Value::as_str) == Some(todo_id))
            .ok_or_else(|| anyhow::anyhow!("Todo {todo_id} no longer exists"))?;
        for key in ["status", "progress", "planningState"] {
            if let Some(value) = update.get(key) {
                todo[key] = value.clone();
            }
        }
        if update
            .get("markComplete")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            todo["completed"] = serde_json::Value::Bool(true);
            todo["status"] = serde_json::Value::String("done".to_string());
            todo["progress"] = serde_json::json!(100);
        }
        if let Some(completed_ids) = update
            .get("completedSubtaskIds")
            .and_then(serde_json::Value::as_array)
        {
            let completed = completed_ids
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<std::collections::HashSet<_>>();
            if let Some(subtasks) = todo
                .get_mut("subtasks")
                .and_then(serde_json::Value::as_array_mut)
            {
                for subtask in subtasks {
                    if subtask
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| completed.contains(id))
                    {
                        subtask["completed"] = serde_json::Value::Bool(true);
                        subtask["status"] = serde_json::Value::String("done".to_string());
                    }
                }
            }
        }
        todo["updatedAt"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
        web.data.set_raw("todos", &serde_json::to_string(&todos)?)?;
        web.events.publish(vec!["todos".to_string()]);
    }

    let entries = payload
        .get("memoryEntries")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if entries.is_empty() {
        return Ok(());
    }
    let user_slug = state
        .config
        .user
        .as_ref()
        .and_then(|user| user.slug.clone())
        .unwrap_or_else(|| "local".to_string());
    let store = crate::runtime::memory::MemoryStore::new(&user_slug)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let action_id = payload
        .get("clientActionId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("review");
    let task_id = payload
        .get("taskId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let slug_prefix = action_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(40)
        .collect::<String>();
    for (index, candidate) in entries.into_iter().enumerate() {
        let content = candidate
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|content| !content.is_empty())
            .ok_or_else(|| anyhow::anyhow!("memoryEntries[{index}].content is required"))?;
        let summary = candidate
            .get("summary")
            .or_else(|| candidate.get("title"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(content)
            .chars()
            .take(crate::runtime::memory::MAX_SUMMARY_CHARS)
            .collect::<String>();
        let slug = format!("decision-{slug_prefix}-{index}");
        if store.read_entry(&slug).is_ok() {
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        let mut entry = crate::runtime::memory::MemoryEntry::new_active(
            slug.clone(),
            summary.clone(),
            content.to_string(),
            now,
            None,
        );
        entry.kind = "decision".to_string();
        entry.tags = vec!["runtime-task".to_string(), task_id.to_string()];
        entry.sources.push(crate::runtime::memory::MemorySource {
            kind: "runtime_task_review".to_string(),
            uri: format!("runtime-task:{task_id}"),
            title: candidate
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            content_hash: None,
            captured_at: Some(now),
        });
        store
            .write_entry_atomic(&entry)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        store
            .upsert_index_line(&crate::runtime::memory::MemoryIndexLine {
                slug,
                summary,
                updated_at: now,
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        crate::runtime::memory::sync_index::best_effort_upsert(&store, &entry);
    }
    Ok(())
}

async fn project_callback(
    state: AppState,
    session_id: String,
    callback: String,
    dedupe_key: String,
    task_ids: Vec<String>,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    let Some(session) = state.session_manager.get(&session_id) else {
        for task_id in &task_ids {
            let _ = state
                .task_broker
                .mark_delivery(task_id, "discarded_session_deleted");
        }
        return Ok(());
    };
    if payload
        .get("sessionGeneration")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|expected| expected != session.generation)
    {
        for task_id in &task_ids {
            let _ = state
                .task_broker
                .mark_delivery(task_id, "discarded_generation_changed");
        }
        return Ok(());
    }
    state
        .session_manager
        .append_runtime_callback(&session_id, payload.clone())?;
    let runtime = state.session_manager.runtime(&session_id);
    let paused = runtime.as_ref().is_some_and(|runtime| runtime.paused)
        || session.state == SessionState::Paused;
    let Some(queued) = state.session_manager.enqueue_runtime_callback(
        &session_id,
        dedupe_key,
        user_message(callback.clone()),
    )?
    else {
        for task_id in &task_ids {
            let _ = state.task_broker.mark_delivery(task_id, "delivered");
            let _ = state
                .task_broker
                .resolve_attention_for_task(task_id, Some(&["paused_callback"]));
        }
        return Ok(());
    };
    for task_id in &task_ids {
        let _ = state.task_broker.mark_delivery(
            task_id,
            if paused {
                "waiting_user"
            } else {
                "callback_queued"
            },
        );
    }
    if paused {
        for task_id in &task_ids {
            let _ = state.task_broker.create_attention(
                task_id,
                "paused_callback",
                &format!(
                    "runtime-callback:{}:{}:paused",
                    queued.client_message_id, task_id
                ),
                "协作者结果等待处理",
                serde_json::json!({
                    "sessionId": session_id,
                    "queuedInputId": queued.id,
                    "callbackDedupeKey": queued.client_message_id,
                }),
            );
        }
        return Ok(());
    }
    schedule_runtime_input(
        state,
        session_id,
        RuntimeInputActivation {
            queued,
            task_ids,
            work_item_id: payload
                .get("workItemId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        },
    )
    .await
}

pub(crate) async fn schedule_runtime_input(
    state: AppState,
    session_id: String,
    activation: RuntimeInputActivation,
) -> anyhow::Result<()> {
    let mut actors = state.runtime_input_actors.lock().await;
    if let Some(sender) = actors.get(&session_id) {
        if sender.send(activation.clone()).is_ok() {
            return Ok(());
        }
        actors.remove(&session_id);
    }

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender
        .send(activation)
        .map_err(|_| anyhow::anyhow!("runtime input actor closed before activation"))?;
    actors.insert(session_id.clone(), sender);
    drop(actors);

    tokio::spawn(async move {
        let actor_state = state.clone();
        let actor_session_id = session_id.clone();
        run_serial_actor(receiver, move |activation| {
            let state = actor_state.clone();
            let session_id = actor_session_id.clone();
            async move {
                let task_ids = activation.task_ids.clone();
                if let Err(error) = activate_runtime_input(&state, &session_id, activation).await {
                    tracing::warn!(%error, %session_id, "runtime input actor failed to activate queued input");
                    let _ = state.session_manager.set_queue_paused(&session_id, true);
                    for task_id in task_ids {
                        let _ = state.task_broker.mark_delivery(&task_id, "waiting_user");
                        let _ = state.task_broker.create_attention(
                            &task_id,
                            "paused_callback",
                            &format!("runtime-callback:{session_id}:{task_id}:activation-paused"),
                            "协作者结果等待处理",
                            serde_json::json!({ "sessionId": session_id, "error": error.to_string() }),
                        );
                    }
                }
            }
        })
        .await;
    });
    Ok(())
}

async fn run_serial_actor<T, F, Fut>(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<T>,
    mut handler: F,
) where
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    while let Some(item) = receiver.recv().await {
        handler(item).await;
    }
}

async fn activate_runtime_input(
    state: &AppState,
    session_id: &str,
    activation: RuntimeInputActivation,
) -> anyhow::Result<()> {
    let Some(runtime) = state.session_manager.runtime(session_id) else {
        for task_id in &activation.task_ids {
            let _ = state
                .task_broker
                .mark_delivery(task_id, "discarded_session_deleted");
        }
        return Ok(());
    };
    if !runtime
        .queue
        .iter()
        .any(|queued| queued.id == activation.queued.id)
    {
        for task_id in &activation.task_ids {
            let _ = state.task_broker.mark_delivery(task_id, "delivered");
            let _ = state
                .task_broker
                .resolve_attention_for_task(task_id, Some(&["paused_callback"]));
        }
        return Ok(());
    }
    if runtime.paused {
        return Ok(());
    }
    if runtime.phase == "running" {
        // The durable session queue is the execution source. The active runner
        // will take this input at its next atomic turn boundary.
        return Ok(());
    }

    if !state
        .session_manager
        .claim_queued_input_for_activation(session_id, &activation.queued.id)?
    {
        // The phase may have changed after the snapshot above, or another
        // serialized activation may already own the claim. In both cases the
        // durable callback remains owned by exactly one execution path.
        return Ok(());
    }

    let Some(session) = state.session_manager.get(session_id) else {
        state.session_manager.restore_claimed_input_for_activation(
            session_id,
            &activation.queued.id,
            true,
        )?;
        anyhow::bail!("session not found");
    };
    let mut messages = session.to_chat_messages();
    messages.push(activation.queued.message.clone());
    let result = super::chat(
        State(state.clone()),
        Path(session_id.to_string()),
        Json(super::ChatRequest {
            messages,
            system_prompt: None,
            provider_override: None,
            thinking: false,
            cwd: session
                .workspace
                .map(|workspace| workspace.canonical_path.to_string_lossy().into_owned()),
            session_name: Some(session.name),
            require_permission_approval: false,
            internal_callback: activation.queued.source
                == crate::runtime::session::QueuedInputSource::RuntimeCallback,
            work_item_id: activation.work_item_id,
            delegation_preference: None,
        }),
    )
    .await;
    if let Err(status) = result {
        // A user turn may win after the actor claim but before chat starts.
        // Restore queued status so that runner can take it once.
        let user_turn_won = status == axum::http::StatusCode::CONFLICT
            && state
                .session_manager
                .runtime(session_id)
                .is_some_and(|runtime| runtime.phase == "running" && !runtime.paused);
        state.session_manager.restore_claimed_input_for_activation(
            session_id,
            &activation.queued.id,
            !user_turn_won,
        )?;
        if user_turn_won {
            return Ok(());
        }
        anyhow::bail!("runtime input activation rejected with {status}");
    }
    state
        .session_manager
        .consume_queued_input_by_id(session_id, &activation.queued.id)?;
    for task_id in &activation.task_ids {
        state.task_broker.mark_delivery(task_id, "delivered")?;
        state
            .task_broker
            .resolve_attention_for_task(task_id, Some(&["paused_callback"]))?;
    }
    Ok(())
}

#[cfg(test)]
mod callback_actor_tests {
    use super::{recoverable_idle_inputs, run_serial_actor, should_retry_document_outbox};
    use std::sync::{Arc, Mutex};

    fn queued_input(id: &str, source: &str) -> crate::runtime::session::QueuedInput {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "clientMessageId": format!("client-{id}"),
            "sequence": 1,
            "delivery": "next_turn",
            "status": "queued",
            "source": source,
            "message": {
                "role": "user",
                "content": id,
                "images": []
            },
            "createdAt": "2026-07-30T00:00:00Z"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn callback_actor_preserves_order_without_blocking_publishers() {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let order = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let task = {
            let order = Arc::clone(&order);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            tokio::spawn(run_serial_actor(receiver, move |value| {
                let order = Arc::clone(&order);
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                async move {
                    order.lock().unwrap().push(value);
                    if value == 1 {
                        started.notify_one();
                        release.notified().await;
                    }
                }
            }))
        };

        sender.send(1).unwrap();
        started.notified().await;
        sender.send(2).unwrap();
        assert_eq!(*order.lock().unwrap(), vec![1]);
        release.notify_one();
        drop(sender);
        task.await.unwrap();
        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
    }

    #[test]
    fn daemon_startup_recovers_idle_user_and_callback_inputs() {
        let mut runtime = crate::runtime::session::SessionRuntimeSnapshot::idle("session-recovery");
        runtime.queue = vec![
            queued_input("user", "user"),
            queued_input("callback", "runtime_callback"),
        ];

        let recovered = recoverable_idle_inputs(&runtime);
        assert_eq!(
            recovered
                .iter()
                .map(|queued| queued.id.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "callback"]
        );

        runtime.paused = true;
        assert!(recoverable_idle_inputs(&runtime).is_empty());
    }

    #[test]
    fn document_outbox_only_retries_transient_io_with_a_fixed_limit() {
        let transient = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::WouldBlock));
        assert!(should_retry_document_outbox(0, &transient));
        assert!(!should_retry_document_outbox(3, &transient));
        assert!(!should_retry_document_outbox(
            0,
            &anyhow::anyhow!("Markdown document is empty"),
        ));
    }
}
