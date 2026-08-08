use anyhow::Context;
use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::Stream;
use tracing::{error, info, warn};

use tokio::sync::broadcast;

use crate::ai::llm::{ChatMessage, ProviderConfig, StreamEvent};
use crate::runtime::session::ConversationMessage;

use super::state::AppState;

mod attention;
mod background;
mod chat;
mod system;
pub(crate) mod task;
pub(crate) mod work_items;

fn append_latest_user_message(
    session: &mut crate::runtime::session::Session,
    messages: &[ChatMessage],
) {
    if let Some(message) = messages
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
    {
        session.lock_workspace();
        session.add_message(ConversationMessage::new_user(message.content.clone()));
    }
}

/// 前端透传的 provider 覆盖：用于 per-request 模型 / provider 切换
#[derive(Debug, Deserialize)]
pub struct ProviderOverride {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub provider_index: Option<usize>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    pub model: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub request_params: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub thinking_params: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub deferred_tools_mode: Option<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
}

impl ProviderOverride {
    fn resolve_provider(&self, state: &AppState) -> anyhow::Result<ProviderConfig> {
        if let Some(id) = self
            .provider_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            let provider = state
                .web
                .as_ref()
                .context("Web service is disabled")?
                .config
                .provider_by_id(id)?;
            return provider_config_for_model(provider, &self.model);
        }
        if let Some(index) = self.provider_index {
            let provider = state
                .web
                .as_ref()
                .context("Web service is disabled")?
                .config
                .provider(index)?;
            return provider_config_for_model(provider, &self.model);
        }

        let model = self.model.clone();
        let model_entry = crate::ai::config::ModelEntry {
            id: model.clone(),
            name: model.clone(),
            enabled: None,
            max_output: None,
            context_window: self.context_window,
            capabilities: None,
            request_params: self.request_params.clone(),
            thinking_params: self.thinking_params.clone(),
            deferred_tools_mode: self.deferred_tools_mode.clone(),
            ..Default::default()
        };
        Ok(ProviderConfig {
            name: self.name.clone().unwrap_or_else(|| "override".to_string()),
            base_url: self.base_url.clone().context("base_url is required")?,
            api_key: self.api_key.clone().context("api_key is required")?,
            protocol: self.protocol.clone().context("protocol is required")?,
            model,
            models: vec![model_entry],
            use_proxy: None,
            compat_profile: None,
        })
    }
}

fn provider_config_for_model(
    provider: super::web::ProviderEndpoint,
    model: &str,
) -> anyhow::Result<ProviderConfig> {
    let model = provider
        .models
        .iter()
        .find(|entry| {
            entry.get("id").and_then(serde_json::Value::as_str) == Some(model)
                || entry.get("name").and_then(serde_json::Value::as_str) == Some(model)
        })
        .cloned()
        .context("Model is not configured for the selected provider")?;
    let model_entry: crate::ai::config::ModelEntry =
        serde_json::from_value(model).context("Invalid model configuration")?;
    Ok(ProviderConfig {
        name: provider.name,
        base_url: provider.base_url,
        api_key: provider.api_key,
        protocol: provider.protocol,
        model: model_entry.id.clone(),
        models: vec![model_entry],
        use_proxy: provider.use_proxy,
        compat_profile: provider.compat_profile,
    })
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub name: Option<String>,
    pub cwd: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub state: crate::runtime::session::SessionState,
    pub generation: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub message_count: usize,
    pub created_at: String,
    pub cwd: Option<String>,
    pub state: crate::runtime::session::SessionState,
    pub generation: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub id: String,
    pub name: String,
    pub messages: Vec<ChatMessage>,
    pub estimated_tokens: u32,
    pub created_at: String,
    pub updated_at: String,
    pub cwd: Option<String>,
    pub state: crate::runtime::session::SessionState,
    pub generation: u64,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub system_prompt: Option<String>,
    /// 可选：前端指定本次请求使用的 provider/model，覆盖 pwcli 全局 config
    #[serde(default)]
    pub provider_override: Option<ProviderOverride>,
    /// 启用扩展思考 / extended thinking。
    /// - Anthropic: 顶层 `thinking: { type:"enabled", budget_tokens:1024 }`
    /// - OpenAI-compatible providers: 顶层 `enable_thinking: true`
    ///
    /// 不支持的模型协议会忽略此字段。
    #[serde(default)]
    pub thinking: bool,
    /// 会话级工作目录（绝对路径）。非空时注入到 system prompt，
    /// 告知 AI 用户当前关注的目录，code_agent 默认以此为 cwd。
    #[serde(default)]
    pub cwd: Option<String>,
    /// 前端 ChatSession.id，用于后台任务回调匹配。
    /// 每次 stream 请求时传入，后端用来更新 session.name。
    #[serde(default)]
    pub session_name: Option<String>,
    /// Collaboration mode may suspend mutating tools until the attention item is resolved.
    #[serde(default)]
    pub require_permission_approval: bool,
    /// Durable daemon callbacks are model inputs, not user-authored messages.
    /// They are journaled separately and therefore must not be projected as a
    /// normal user bubble by `append_latest_user_message`.
    #[serde(default)]
    pub(crate) internal_callback: bool,
    /// Optional Todo binding propagated into RuntimeTask batches published by
    /// this turn. It is daemon-owned for capture/resume flows.
    #[serde(default)]
    pub(crate) work_item_id: Option<String>,
    /// Capture-only routing preference injected into the system prompt without
    /// altering the user-authored message shown in the timeline.
    #[serde(default)]
    pub(crate) delegation_preference: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessAction {
    Steer,
    FollowUp,
    NextTurn,
    Abort,
    Resume,
    SendNext,
    ClearQueue,
}

#[derive(Debug, Deserialize)]
pub struct HarnessControlRequest {
    pub action: HarnessAction,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HarnessControlResponse {
    pub phase: crate::agent_core::harness::HarnessPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released_message: Option<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released_input: Option<crate::runtime::session::QueuedInput>,
    pub runtime: Option<crate::runtime::session::SessionRuntimeSnapshot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelocateWorkspaceRequest {
    path: String,
    confirm: bool,
    #[serde(default)]
    expected_generation: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InputDeliveryRequest {
    Auto,
    NextTurn,
    Guidance,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitInputRequest {
    client_message_id: String,
    delivery: InputDeliveryRequest,
    message: ChatMessage,
    #[serde(default)]
    image_urls: Vec<String>,
    #[serde(default)]
    file_references: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitInputResponse {
    item: crate::runtime::session::QueuedInput,
    runtime: crate::runtime::session::SessionRuntimeSnapshot,
    effective_delivery: crate::runtime::session::QueuedInputDelivery,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PatchQueueItemRequest {
    content: Option<String>,
    position: Option<usize>,
    delivery: Option<crate::runtime::session::QueuedInputDelivery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompactSessionResponse {
    summarized: usize,
    kept: usize,
    message_count: usize,
    tokens_before: u32,
    tokens_after: u32,
}

#[derive(Debug, Deserialize, Default)]
struct CompactSessionRequest {
    #[serde(default)]
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchBranchRequest {
    target_entry_id: uuid::Uuid,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(attention::routes())
        .merge(chat::routes())
        .merge(background::routes())
        .merge(task::routes())
        .merge(work_items::routes())
        .merge(system::routes())
        .merge(super::memory_routes::routes())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonShutdownRequest {
    instance_id: String,
}

async fn daemon_status(
    State(state): State<AppState>,
) -> Result<Json<crate::app::platform::daemon::DaemonStatus>, axum::http::StatusCode> {
    let runtime = state
        .daemon_runtime
        .as_ref()
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    let active_tasks = state.background_tasks.running_count().await
        + state.task_broker.active_count().unwrap_or_default();
    let active_acp_sessions = crate::runtime::tools::code_agent::acp_runner::active_session_count();
    Ok(Json(crate::app::platform::daemon::DaemonStatus {
        running: true,
        pid: Some(runtime.metadata.pid),
        instance_id: Some(runtime.metadata.instance_id.clone()),
        version: Some(runtime.metadata.version.clone()),
        started_at: Some(runtime.metadata.started_at),
        http_address: runtime.metadata.http_address.clone(),
        data_dir: runtime.metadata.data_dir.clone(),
        config_file: runtime.metadata.config_file.clone(),
        active_tasks,
        active_acp_sessions,
        healthy: true,
    }))
}

async fn daemon_shutdown(
    State(state): State<AppState>,
    Json(request): Json<DaemonShutdownRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let runtime = state
        .daemon_runtime
        .as_ref()
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    if request.instance_id != runtime.metadata.instance_id {
        return Err(axum::http::StatusCode::CONFLICT);
    }
    runtime.shutdown.cancel();
    Ok(Json(serde_json::json!({ "accepted": true })))
}

#[derive(Debug, Deserialize)]
struct ReplayEventsQuery {
    #[serde(default)]
    after: u64,
}

async fn replay_harness_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ReplayEventsQuery>,
) -> Json<Vec<crate::agent_core::harness::HarnessEventRecord>> {
    let harness = session_harness(&state, &id).await;
    Json(harness.events().replay_after(query.after))
}

async fn session_harness(
    state: &AppState,
    session_id: &str,
) -> std::sync::Arc<crate::agent_core::harness::HarnessControl> {
    let mut harnesses = state.harnesses.lock().await;
    std::sync::Arc::clone(harnesses.entry(session_id.to_string()).or_insert_with(|| {
        std::sync::Arc::new(crate::agent_core::harness::HarnessControl::default())
    }))
}

async fn control_harness(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<HarnessControlRequest>,
) -> Result<Json<HarnessControlResponse>, (axum::http::StatusCode, String)> {
    let harness = session_harness(&state, &id).await;
    let message = || {
        let content = request
            .content
            .as_deref()
            .map(str::trim)
            .filter(|content| !content.is_empty())
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    "content is required for this action".to_string(),
                )
            })?;
        Ok(ChatMessage {
            role: "user".to_string(),
            content: content.to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        })
    };

    let mut released_message = None;
    let mut released_input = None;
    let result = match request.action {
        HarnessAction::Steer => harness.steer(message()?).await,
        HarnessAction::FollowUp => harness.follow_up(message()?).await,
        HarnessAction::NextTurn => harness.next_turn(message()?).await,
        HarnessAction::Abort => {
            state
                .permission_broker
                .deny_session(&id, "任务已取消，权限请求已拒绝");
            state
                .session_manager
                .set_queue_paused(&id, true)
                .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
            harness.abort().await
        }
        HarnessAction::Resume => {
            let released = state
                .session_manager
                .release_next_queued(&id, true)
                .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
            if let Some(item) = released {
                harness
                    .remove_next_turn_by_id(&item.id)
                    .await
                    .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
                if item.source == crate::runtime::session::QueuedInputSource::RuntimeCallback {
                    activate_released_runtime_callback(&state, &id, &item)
                        .await
                        .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
                }
                released_message = released_message_for_client(&item);
                released_input = Some(item);
            }
            Ok(())
        }
        HarnessAction::SendNext => {
            let released = state
                .session_manager
                .release_next_queued(&id, false)
                .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
            if let Some(item) = released {
                harness
                    .remove_next_turn_by_id(&item.id)
                    .await
                    .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
                if item.source == crate::runtime::session::QueuedInputSource::RuntimeCallback {
                    activate_released_runtime_callback(&state, &id, &item)
                        .await
                        .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
                }
                released_message = released_message_for_client(&item);
                released_input = Some(item);
            }
            Ok(())
        }
        HarnessAction::ClearQueue => {
            state
                .session_manager
                .clear_queue(&id)
                .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
            harness.clear_next_turn().await
        }
    };
    result.map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;

    Ok(Json(HarnessControlResponse {
        phase: harness.phase(),
        released_message,
        released_input,
        runtime: state.session_manager.runtime(&id),
    }))
}

fn released_message_for_client(item: &crate::runtime::session::QueuedInput) -> Option<ChatMessage> {
    (item.source == crate::runtime::session::QueuedInputSource::User).then(|| item.message.clone())
}

async fn activate_released_runtime_callback(
    state: &AppState,
    session_id: &str,
    item: &crate::runtime::session::QueuedInput,
) -> anyhow::Result<()> {
    let session = state
        .session_manager
        .get(session_id)
        .ok_or_else(|| anyhow::anyhow!("session not found"))?;
    let mut messages = session.to_chat_messages();
    messages.push(item.message.clone());
    let result = chat(
        State(state.clone()),
        Path(session_id.to_string()),
        Json(ChatRequest {
            messages,
            system_prompt: None,
            provider_override: None,
            thinking: false,
            cwd: session
                .workspace
                .map(|workspace| workspace.canonical_path.to_string_lossy().into_owned()),
            session_name: Some(session.name),
            require_permission_approval: false,
            internal_callback: true,
            work_item_id: None,
            delegation_preference: None,
        }),
    )
    .await;
    if let Err(status) = result {
        state
            .session_manager
            .requeue_claimed_input(session_id, &item.id)?;
        anyhow::bail!("callback activation rejected with {status}");
    }
    state
        .session_manager
        .consume_queued_input_by_id(session_id, &item.id)?;
    for task_id in state
        .session_manager
        .runtime_callback_task_ids(session_id, &item.client_message_id)?
    {
        state.task_broker.mark_delivery(&task_id, "delivered")?;
        state
            .task_broker
            .resolve_attention_for_task(&task_id, Some(&["paused_callback"]))?;
    }
    Ok(())
}

async fn submit_session_input(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SubmitInputRequest>,
) -> Result<Json<SubmitInputResponse>, (axum::http::StatusCode, String)> {
    if request.message.role != "user"
        || (request.message.content.trim().is_empty()
            && request.image_urls.is_empty()
            && request.file_references.is_empty())
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "a non-empty user message is required".to_string(),
        ));
    }
    let runtime = state.session_manager.runtime(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "session runtime not found".to_string(),
        )
    })?;
    let requested_delivery = match request.delivery {
        InputDeliveryRequest::Guidance => crate::runtime::session::QueuedInputDelivery::Guidance,
        InputDeliveryRequest::Auto if runtime.active_turn_id.is_some() => {
            crate::runtime::session::QueuedInputDelivery::NextTurn
        }
        InputDeliveryRequest::Auto | InputDeliveryRequest::NextTurn => {
            crate::runtime::session::QueuedInputDelivery::NextTurn
        }
    };
    let item = state
        .session_manager
        .enqueue_input(
            &id,
            request.client_message_id,
            requested_delivery,
            request.message.clone(),
            request.image_urls,
            request.file_references,
        )
        .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
    if runtime.active_turn_id.is_some() && !runtime.paused {
        let harness = session_harness(&state, &id).await;
        let queue = state
            .session_manager
            .runtime(&id)
            .map(|runtime| runtime.queue)
            .unwrap_or_default();
        harness
            .mutate_durable_queue(None, || Ok(((), queue)))
            .await
            .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
    }
    let runtime = state.session_manager.runtime(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "session runtime not found".to_string(),
        )
    })?;
    if runtime.phase == "idle"
        && !runtime.paused
        && runtime.queue.iter().any(|queued| {
            queued.id == item.id
                && queued.status == crate::runtime::session::QueuedInputStatus::Queued
                && queued.delivery == crate::runtime::session::QueuedInputDelivery::NextTurn
        })
    {
        task::schedule_runtime_input(
            state.clone(),
            id.clone(),
            super::state::RuntimeInputActivation {
                queued: item.clone(),
                task_ids: Vec::new(),
                work_item_id: None,
            },
        )
        .await
        .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))?;
    }
    Ok(Json(SubmitInputResponse {
        effective_delivery: item.delivery,
        item,
        runtime,
    }))
}

async fn get_session_runtime(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::runtime::session::SessionRuntimeSnapshot>, axum::http::StatusCode> {
    state
        .session_manager
        .runtime(&id)
        .map(Json)
        .ok_or(axum::http::StatusCode::NOT_FOUND)
}

async fn session_runtime_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ReplayEventsQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial = state.session_manager.runtime(&id);
    let mut receiver = state.session_manager.subscribe_runtime();
    let stream = async_stream::stream! {
        let mut cursor = query.after;
        if let Some(snapshot) = initial {
            if snapshot.last_event_sequence > cursor {
                cursor = snapshot.last_event_sequence;
                yield Ok(Event::default()
                    .id(cursor.to_string())
                    .event("session_runtime")
                    .data(serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string())));
            }
        }
        loop {
            match receiver.recv().await {
                Ok(snapshot) if snapshot.session_id == id && snapshot.last_event_sequence > cursor => {
                    cursor = snapshot.last_event_sequence;
                    yield Ok(Event::default()
                        .id(cursor.to_string())
                        .event("session_runtime")
                        .data(serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string())));
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if let Some(snapshot) = state.session_manager.runtime(&id) {
                        if snapshot.last_event_sequence > cursor {
                            cursor = snapshot.last_event_sequence;
                            yield Ok(Event::default()
                                .id(cursor.to_string())
                                .event("session_runtime")
                                .data(serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string())));
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default().interval(std::time::Duration::from_secs(15)))
}

async fn patch_queue_item(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(String, String)>,
    Json(request): Json<PatchQueueItemRequest>,
) -> Result<Json<crate::runtime::session::QueuedInput>, (axum::http::StatusCode, String)> {
    let harness = session_harness(&state, &id).await;
    harness
        .mutate_durable_queue(Some(&item_id), || {
            let updated = state.session_manager.replace_queue_item(
                &id,
                &item_id,
                request.content,
                request.position,
                request.delivery,
            )?;
            let queue = state
                .session_manager
                .runtime(&id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?
                .queue;
            Ok((updated, queue))
        })
        .await
        .map(Json)
        .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))
}

async fn delete_queue_item(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(String, String)>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let harness = session_harness(&state, &id).await;
    harness
        .mutate_durable_queue(Some(&item_id), || {
            state.session_manager.delete_queue_item(&id, &item_id)?;
            let queue = state
                .session_manager
                .runtime(&id)
                .ok_or_else(|| anyhow::anyhow!("session runtime not found"))?
                .queue;
            Ok(((), queue))
        })
        .await
        .map(|()| axum::http::StatusCode::NO_CONTENT)
        .map_err(|error| (axum::http::StatusCode::CONFLICT, error.to_string()))
}

#[derive(Debug, Deserialize)]
struct ListBackendsQuery {
    #[serde(default = "default_include_models")]
    include_models: bool,
}

fn default_include_models() -> bool {
    true
}

/// List available code_agent backends, optionally skipping expensive model discovery.
async fn list_backends(Query(query): Query<ListBackendsQuery>) -> Json<Vec<serde_json::Value>> {
    let backends = crate::runtime::tools::code_agent::list_available_backends(query.include_models);
    let out: Vec<_> = backends
        .into_iter()
        .map(|b| {
            serde_json::json!({
                "name": b.name,
                "available": b.available,
                "transport": b.transport,
                "models": b.models,
                "permissionModes": b.permission_modes,
                "install_hint": b.install_hint,
            })
        })
        .collect();
    Json(out)
}

/// List all loaded skills (from ~/.agents/skills/) for frontend slash-command picker.
async fn list_skills() -> Json<Vec<serde_json::Value>> {
    let skills = crate::runtime::skills::load_skills();
    let out: Vec<_> = skills
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "has_command": s.command.is_some(),
            })
        })
        .collect();
    Json(out)
}

async fn health() -> &'static str {
    "ok"
}

fn resolve_web_provider_selection(
    state: &AppState,
    provider_override: Option<&ProviderOverride>,
) -> anyhow::Result<Option<crate::app::composition::ProviderSelection>> {
    Ok(provider_override
        .map(|value| value.resolve_provider(state))
        .transpose()?
        .map(crate::app::composition::ProviderSelection::resolved))
}

#[allow(clippy::too_many_arguments)]
async fn create_web_runtime(
    state: &AppState,
    session_id: &str,
    chat_session_id: Option<String>,
    work_item_id: Option<String>,
    workspace: std::path::PathBuf,
    thinking: bool,
    system_prompt: String,
    harness: Arc<crate::agent_core::harness::HarnessControl>,
    provider: Option<crate::app::composition::ProviderSelection>,
    image_references: Arc<crate::runtime::visual_generation::ImageReferenceRegistry>,
) -> anyhow::Result<crate::app::composition::AgentRuntime> {
    let factory = crate::app::composition::RuntimeFactory::from_shared(
        Arc::clone(&state.config),
        Arc::clone(&state.backend),
        Arc::clone(&state.tool_registry),
        Arc::clone(&state.permission_engine),
        Arc::clone(&state.web_cache),
        Arc::clone(&state.background_tasks),
        Arc::clone(&state.auth_manager),
    );
    factory
        .create(crate::app::composition::RuntimeRequest {
            profile: crate::app::composition::RuntimeProfile::WebMain,
            provider_override: provider,
            workspace: workspace.clone(),
            permission_mode: crate::runtime::permissions::current_agent_permission_mode(),
            thinking,
            session_id: Some(session_id.to_string().into()),
            worker_dispatch: None,
            system_prompt,
            harness: Some(harness),
            tool_context: crate::runtime::tools::context::ToolExecutionContext {
                chat_session_id: chat_session_id.map(Into::into),
                work_item_id: work_item_id.map(Into::into),
                cwd: workspace,
                image_references: Some(image_references),
                web_cache: Some(Arc::clone(&state.web_cache)),
                ..crate::runtime::tools::context::ToolExecutionContext::default()
            },
        })
        .await
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, (axum::http::StatusCode, String)> {
    let name = req.name.unwrap_or_else(|| "unnamed".to_string());
    let sandbox = crate::runtime::tools::fs_local::FsSandbox::from_config()
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?;
    let canonical = sandbox
        .resolve_existing_directory(&req.cwd)
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?;
    let id = state
        .session_manager
        .create_in_workspace(&name, canonical.clone(), std::path::PathBuf::from(&req.cwd))
        .map_err(|error| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            )
        })?;
    Ok(Json(CreateSessionResponse {
        id,
        name,
        cwd: canonical.to_string_lossy().into_owned(),
        state: crate::runtime::session::SessionState::Ready,
        generation: 1,
    }))
}

async fn list_sessions(State(state): State<AppState>) -> Json<Vec<(String, String)>> {
    Json(state.session_manager.list())
}

async fn reload_resources(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    crate::runtime::skills::init();
    let skills = crate::runtime::skills::load_skills().len();
    let extensions = crate::runtime::tools::extensions::register_trusted(std::sync::Arc::clone(
        &state.tool_registry,
    ))
    .await
    .map_err(|error| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
        )
    })?;
    Ok(Json(
        serde_json::json!({ "skills": skills, "extensions": extensions }),
    ))
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionInfo>, axum::http::StatusCode> {
    let session = state
        .session_manager
        .get(&id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    Ok(Json(SessionInfo {
        id: session.id.clone(),
        name: session.name.clone(),
        message_count: session.messages.len(),
        created_at: session.created_at.to_rfc3339(),
        cwd: session
            .workspace
            .as_ref()
            .map(|workspace| workspace.canonical_path.to_string_lossy().into_owned()),
        state: session.state,
        generation: session.generation,
    }))
}

async fn get_session_snapshot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionSnapshot>, axum::http::StatusCode> {
    let session = state
        .session_manager
        .get(&id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    Ok(Json(SessionSnapshot {
        id: session.id.clone(),
        name: session.name.clone(),
        messages: session.to_public_chat_messages(),
        estimated_tokens: session.estimate_tokens(),
        created_at: session.created_at.to_rfc3339(),
        updated_at: session.updated_at.to_rfc3339(),
        cwd: session
            .workspace
            .as_ref()
            .map(|workspace| workspace.canonical_path.to_string_lossy().into_owned()),
        state: session.state,
        generation: session.generation,
    }))
}

async fn list_session_branches(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<
    Json<Vec<crate::runtime::session::manager::BranchEntryInfo>>,
    (axum::http::StatusCode, String),
> {
    state
        .session_manager
        .branches(&id)
        .map(Json)
        .map_err(|error| (axum::http::StatusCode::NOT_FOUND, error.to_string()))
}

async fn switch_session_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SwitchBranchRequest>,
) -> Result<Json<SessionInfo>, (axum::http::StatusCode, String)> {
    let divergent = state
        .session_manager
        .divergent_branch_text(&id, request.target_entry_id)
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?;
    let summary = if divergent.trim().is_empty() {
        None
    } else {
        let mut usage = crate::ai::usage::UsageTracker::new();
        match crate::ai::llm::summarize_via_llm(
            &divergent,
            "总结即将离开的会话分支，保留目标、关键判断、已完成动作和未决事项。输出简洁中文摘要。",
            &state.llm_client,
            &mut usage,
        )
        .await
        {
            Ok(summary) => Some(summary),
            Err(error) => {
                warn!(session_id = %id, error = %error, "branch summary failed; switching without summary");
                None
            }
        }
    };
    let session = state
        .session_manager
        .switch_branch(&id, request.target_entry_id, summary)
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(SessionInfo {
        id: session.id,
        name: session.name,
        message_count: session.messages.len(),
        created_at: session.created_at.to_rfc3339(),
        cwd: session
            .workspace
            .as_ref()
            .map(|workspace| workspace.canonical_path.to_string_lossy().into_owned()),
        state: session.state,
        generation: session.generation,
    }))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    state.task_broker.cancel_for_session(&id).map_err(|error| {
        warn!(%error, session_id = %id, "failed to cancel delegated tasks for deleted session");
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "无法取消会话关联的委派任务，会话未删除".to_string(),
        )
    })?;
    state
        .permission_broker
        .deny_session(&id, "会话已删除，权限请求已拒绝");
    let harness = state.harnesses.lock().await.remove(&id);
    if let Some(harness) = harness {
        if let Err(error) = harness.abort().await {
            warn!(%error, session_id = %id, "failed to abort harness for deleted session");
        }
    }
    state.runtime_input_actors.lock().await.remove(&id);
    state.web_cache.clear_session(&id).await;
    crate::runtime::bash::shell_state::clear(&id);
    let cancelled_background_tasks = state.background_tasks.cancel_by_session(&id).await;
    let deleted = state.session_manager.delete(&id);
    Ok(Json(serde_json::json!({
        "deleted": deleted,
        "sessionId": id,
        "cancelledBackgroundTasks": cancelled_background_tasks,
    })))
}

async fn relocate_session_workspace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RelocateWorkspaceRequest>,
) -> Result<Json<SessionInfo>, (axum::http::StatusCode, String)> {
    if !request.confirm {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "explicit confirmation is required".into(),
        ));
    }
    let mut session = state.session_manager.get(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "session not found".into(),
        )
    })?;
    if request
        .expected_generation
        .is_some_and(|generation| generation != session.generation)
    {
        return Err((
            axum::http::StatusCode::CONFLICT,
            "session generation changed".into(),
        ));
    }
    let sandbox = crate::runtime::tools::fs_local::FsSandbox::from_config()
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?;
    let canonical = sandbox
        .resolve_existing_directory(&request.path)
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?;
    session.workspace = Some(crate::runtime::session::WorkspaceBinding {
        canonical_path: canonical.clone(),
        display_path: std::path::PathBuf::from(&request.path),
        locked_at: Some(chrono::Utc::now()),
    });
    session.state = crate::runtime::session::SessionState::Paused;
    session.generation = session.generation.saturating_add(1);
    state.session_manager.update(session.clone());
    state
        .session_manager
        .set_queue_paused(&id, true)
        .map_err(|error| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            )
        })?;
    Ok(Json(SessionInfo {
        id: session.id,
        name: session.name,
        message_count: session.messages.len(),
        created_at: session.created_at.to_rfc3339(),
        cwd: Some(canonical.to_string_lossy().into_owned()),
        state: session.state,
        generation: session.generation,
    }))
}

async fn compact_session_context(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CompactSessionRequest>,
) -> Result<Json<CompactSessionResponse>, (axum::http::StatusCode, String)> {
    let mut session = state.session_manager.get(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "agent session not found".to_string(),
        )
    })?;
    if !request.messages.is_empty() {
        session.messages = request
            .messages
            .into_iter()
            .filter_map(|message| match message.role.as_str() {
                "user" if !message.content.trim().is_empty() => {
                    Some(ConversationMessage::new_user(message.content))
                }
                "assistant" if !message.content.trim().is_empty() => {
                    Some(ConversationMessage::new_assistant(message.content))
                }
                _ => None,
            })
            .collect();
    }
    let tokens_before = session.estimate_tokens();
    let user_slug = state
        .config
        .user
        .as_ref()
        .and_then(|user| user.slug.clone())
        .unwrap_or_else(|| "local".into());
    crate::runtime::memory::schedule_before_compaction(
        user_slug,
        session.messages.clone(),
        crate::app::config::local_config::get()
            .features
            .auto_memory_extract,
    );
    let mut messages = session.to_chat_messages();
    let mut usage = crate::ai::usage::UsageTracker::new();
    match crate::app::cli::application::compact_session(
        &mut session,
        &mut messages,
        &mut usage,
        &state.config,
        Some(state.llm_client.as_ref()),
    )
    .await
    {
        crate::app::cli::application::CompactOutcome::Done {
            summarized,
            kept,
            total,
        } => {
            let tokens_after = session.estimate_tokens();
            state.session_manager.update(session);
            Ok(Json(CompactSessionResponse {
                summarized,
                kept,
                message_count: total,
                tokens_before,
                tokens_after,
            }))
        }
        crate::app::cli::application::CompactOutcome::Skipped(reason) => {
            Err((axum::http::StatusCode::UNPROCESSABLE_ENTITY, reason))
        }
        crate::app::cli::application::CompactOutcome::Failed(reason) => {
            Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, reason))
        }
    }
}

async fn chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, axum::http::StatusCode> {
    let chat_session_id = req.session_name.clone();
    let mut session = state
        .session_manager
        .get(&id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    let bound_cwd = session
        .workspace
        .as_ref()
        .map(|workspace| workspace.canonical_path.clone())
        .ok_or(axum::http::StatusCode::CONFLICT)?;
    if let Some(request_cwd) = req.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
        let sandbox = crate::runtime::tools::fs_local::FsSandbox::from_config()
            .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;
        let request_cwd = sandbox
            .resolve_existing_directory(request_cwd)
            .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;
        if request_cwd != bound_cwd {
            return Err(axum::http::StatusCode::CONFLICT);
        }
    }
    if let Some(ref sn) = req.session_name {
        if session.name != *sn {
            session.name = sn.clone();
            state.session_manager.update(session.clone());
        }
    }

    let user_slug = state
        .config
        .user
        .as_ref()
        .and_then(|u| u.slug.clone())
        .unwrap_or_else(|| "local".to_string());
    let mem_opts = crate::app::cli::commands::memory_injection_opts_from_features(
        &state.config.features,
        crate::app::cli::commands::last_user_query_from_chat_messages(&req.messages),
    );
    let mut system_prompt = match req.system_prompt {
        Some(sp) => {
            crate::app::cli::commands::enhance_system_prompt(&sp, &user_slug, &mem_opts).await
        }
        None => {
            crate::app::cli::commands::get_system_prompt_with_catalog(
                &state.backend,
                &user_slug,
                &mem_opts,
            )
            .await
        }
    };
    system_prompt.push_str(&format!(
        "\n\n## 当前工作目录\n\n用户已设置会话工作目录为 `{}`。\n\
                - 使用 code_agent 时默认 cwd 传此路径\n\
                - 使用 ls / read / write 时优先使用相对于此目录的路径\n\
                - 用户提到的相对路径默认相对此目录",
        bound_cwd.display()
    ));
    crate::app::cli::commands::append_web_search_note(&mut system_prompt);
    crate::app::cli::commands::append_response_language_policy(&mut system_prompt);
    crate::app::cli::commands::append_final_answer_contract(&mut system_prompt);
    if let Some(executor) = req
        .delegation_preference
        .as_deref()
        .filter(|executor| !executor.trim().is_empty())
    {
        system_prompt.push_str(&format!(
            "\n\n## 捕获事项执行约束\n\n这是一个异步捕获事项。请先判断合适 role/access，随后调用 dispatch_tasks；executor 必须设为 `{executor}`。发布成功后立即结束本回合，不要在主进程内自行等待。"
        ));
    }
    let mut messages = req.messages;
    if !req.internal_callback {
        append_latest_user_message(&mut session, &messages);
    }
    if session
        .messages
        .first()
        .is_some_and(|message| message.id.starts_with("msg_compact_"))
    {
        messages = session.to_chat_messages();
    }
    let image_references =
        crate::runtime::visual_generation::build_image_reference_registry(&mut messages);
    let mut usage_tracker = crate::ai::usage::UsageTracker::new();

    let provider = resolve_web_provider_selection(&state, req.provider_override.as_ref()).map_err(
        |error| {
            warn!(error = %error, "invalid provider override");
            axum::http::StatusCode::BAD_REQUEST
        },
    )?;
    let harness = session_harness(&state, &id).await;
    let audit_sink = state.session_manager.audit_sink(id.clone());
    let runtime = create_web_runtime(
        &state,
        &id,
        chat_session_id,
        req.work_item_id.clone(),
        bound_cwd,
        req.thinking,
        system_prompt,
        Arc::clone(&harness),
        provider,
        image_references,
    )
    .await
    .map_err(|error| {
        warn!(error = %error, "failed to create Web AgentRuntime");
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;

    state
        .session_manager
        .mark_turn_started(&id)
        .map_err(|_| axum::http::StatusCode::CONFLICT)?;
    let noop_sink = crate::agent_core::runner::NoopSink;
    let turn_result = execute_turn(
        &runtime,
        &noop_sink,
        Some(&audit_sink),
        &state.session_manager,
        &id,
        &mut messages,
        &mut session,
        &mut usage_tracker,
    )
    .await;
    if turn_result.is_err() {
        let _ = state.session_manager.mark_turn_settled(&id, true);
    }
    let summary = turn_result.map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // 将修改后的 session 存回管理器
    finalize_turn(&state, user_slug, &session);

    let content = summary
        .as_ref()
        .map(|s| s.content.clone())
        .unwrap_or_default();
    let tool_calls = summary.and_then(|s| {
        s.tool_calls.map(|tcs| {
            tcs.into_iter()
                .map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments
                        }
                    })
                })
                .collect()
        })
    });

    Ok(Json(ChatResponse {
        content,
        tool_calls,
    }))
}

async fn execute_turn(
    runtime: &crate::app::composition::AgentRuntime,
    sink: &dyn crate::agent_core::runner::ToolEventSink,
    audit_sink: Option<&dyn crate::agent_core::harness::HarnessAuditSink>,
    session_manager: &crate::runtime::session::manager::SessionManager,
    session_id: &str,
    messages: &mut Vec<ChatMessage>,
    session: &mut crate::runtime::session::Session,
    usage: &mut crate::ai::usage::UsageTracker,
) -> anyhow::Result<Option<crate::agent_core::runner::TurnSummary>> {
    let harness = runtime.harness();
    let mut result = runtime
        .run_turn(messages, session, usage, sink, audit_sink)
        .await;
    while result.is_ok() {
        let legacy_guidance = harness
            .drain_pending_guidance(|remaining_queue_item_ids| {
                session_manager.reconcile_guidance(session_id, remaining_queue_item_ids)
            })
            .await?;
        for message in legacy_guidance {
            harness.next_turn(message).await?;
        }
        if session_manager
            .runtime(session_id)
            .is_some_and(|runtime| runtime.paused)
        {
            session_manager.mark_turn_settled(session_id, false)?;
            break;
        }
        if let Some(item) = session_manager.take_next_queued_input(session_id)? {
            let conversation_message = match item.source {
                crate::runtime::session::QueuedInputSource::User => {
                    crate::runtime::session::ConversationMessage::new_user(&item.message.content)
                }
                crate::runtime::session::QueuedInputSource::RuntimeCallback => {
                    crate::runtime::session::ConversationMessage::new_system(&item.message.content)
                }
            };
            session.add_message(conversation_message);
            messages.push(queued_input_execution_message(&item));
        } else if harness
            .consume_next_turn(messages, session)
            .await?
            .is_none()
        {
            if let Some(item) = session_manager.take_next_queued_input_or_settle(session_id)? {
                let conversation_message = match item.source {
                    crate::runtime::session::QueuedInputSource::User => {
                        crate::runtime::session::ConversationMessage::new_user(
                            &item.message.content,
                        )
                    }
                    crate::runtime::session::QueuedInputSource::RuntimeCallback => {
                        crate::runtime::session::ConversationMessage::new_system(
                            &item.message.content,
                        )
                    }
                };
                session.add_message(conversation_message);
                messages.push(queued_input_execution_message(&item));
            } else {
                break;
            }
        }
        result = runtime
            .run_turn(messages, session, usage, sink, audit_sink)
            .await;
    }
    result
}

fn queued_input_execution_message(item: &crate::runtime::session::QueuedInput) -> ChatMessage {
    let mut message = item.message.clone();
    message.images = item
        .image_urls
        .iter()
        .filter_map(|url| crate::ai::llm::ImageAttachment::from_url(url))
        .collect();
    for reference in &item.file_references {
        let title = reference
            .get("path")
            .or_else(|| reference.get("title"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("附件");
        let content = reference
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        message
            .content
            .push_str(&format!("\n\n[引用文件: {title}]\n{content}"));
    }
    message
}

fn finalize_turn(state: &AppState, user_slug: String, session: &crate::runtime::session::Session) {
    state.session_manager.update(session.clone());
    crate::runtime::memory::schedule_after_turn(
        user_slug,
        state.llm_client.as_ref().clone(),
        session.messages.clone(),
        crate::app::config::local_config::get()
            .features
            .auto_memory_extract,
    );
}

/// Sink that forwards every event into an unbounded mpsc channel.
/// The SSE stream task drains the channel concurrently with the runner.
struct ChannelSink {
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    permission_broker: std::sync::Arc<crate::runtime::permissions::PermissionBroker>,
    session_id: String,
    cwd: String,
    require_permission_approval: bool,
}

impl crate::agent_core::runner::ToolEventSink for ChannelSink {
    // 流式期间已经通过 on_tool_call_streaming + on_tool_call_args_delta 发了
    // ToolCallStart/Delta；此回调（run_turn 即将执行 tool 时调用）不重发，避免
    // 前端 trace chip 出现两次。
    fn on_tool_call(&self, _id: &str, _name: &str, _args: &str) {}
    fn on_assistant_segment_start(&self, round: u32) {
        let _ = self.tx.send(StreamEvent::AssistantSegmentStart { round });
    }
    fn on_assistant_segment_end(&self, round: u32, has_tool_calls: bool) {
        let _ = self.tx.send(StreamEvent::AssistantSegmentEnd {
            round,
            has_tool_calls,
        });
    }
    fn on_tool_call_streaming(&self, id: &str, name: &str) {
        let _ = self.tx.send(StreamEvent::ToolCallStart {
            id: id.to_string(),
            name: name.to_string(),
            thought_signature: None,
        });
    }
    fn on_tool_call_args_delta(&self, id: &str, delta: &str) {
        let _ = self.tx.send(StreamEvent::ToolCallDelta {
            id: id.to_string(),
            arguments_delta: delta.to_string(),
        });
    }
    fn on_tool_result(
        &self,
        id: &str,
        name: &str,
        result: &str,
        is_error: bool,
        failure: Option<&crate::agent_core::reliability::FailureEnvelope>,
    ) {
        let _ = self.tx.send(StreamEvent::ToolResult {
            id: id.to_string(),
            name: name.to_string(),
            result: result.to_string(),
            is_error,
            failure: failure.and_then(|value| serde_json::to_value(value).ok()),
        });
    }
    fn on_tool_recovery(
        &self,
        id: &str,
        name: &str,
        failure: &crate::agent_core::reliability::FailureEnvelope,
        phase: &str,
    ) {
        let _ = self.tx.send(StreamEvent::ToolRecovery {
            id: id.to_string(),
            name: name.to_string(),
            failure: serde_json::to_value(failure).unwrap_or(serde_json::Value::Null),
            phase: phase.to_string(),
        });
    }
    fn on_tool_details(&self, id: &str, _name: &str, details: &serde_json::Value) {
        if let Some(document) = details.get("documentRef") {
            let _ = self.tx.send(StreamEvent::ToolDocument {
                id: id.to_string(),
                document: document.clone(),
            });
        }
        if let Some(decision) = details.get("decisionPrompt") {
            let _ = self.tx.send(StreamEvent::ToolDecision {
                id: id.to_string(),
                decision: decision.clone(),
            });
        }
    }
    fn on_permission_prompt(&self, _name: &str, _args: &str) {}
    fn on_permission_denied(&self, _name: &str) {}
    fn on_decision_started(
        &self,
        id: &str,
        trigger: crate::agent_core::decision::DecisionTrigger,
        risk: crate::agent_core::decision::DecisionRisk,
    ) {
        let _ = self.tx.send(StreamEvent::DecisionStarted {
            id: id.to_string(),
            trigger: format!("{trigger:?}").to_ascii_lowercase(),
            risk: format!("{risk:?}").to_ascii_lowercase(),
        });
    }
    fn on_decision_advisor(&self, id: &str, model: &str, succeeded: bool) {
        let _ = self.tx.send(StreamEvent::DecisionAdvisor {
            id: id.to_string(),
            model: model.to_string(),
            status: if succeeded { "done" } else { "error" }.into(),
        });
    }
    fn on_decision_resolved(
        &self,
        id: &str,
        verdict: &crate::agent_core::decision::DecisionVerdict,
    ) {
        let _ = self.tx.send(StreamEvent::DecisionResolved {
            id: id.to_string(),
            outcome: format!("{:?}", verdict.outcome).to_ascii_lowercase(),
            confidence: verdict.confidence,
            consensus: verdict.consensus,
            rationale: verdict.rationale.clone(),
        });
    }
    fn on_decision_escalated(
        &self,
        id: &str,
        verdict: &crate::agent_core::decision::DecisionVerdict,
    ) {
        let _ = self.tx.send(StreamEvent::DecisionEscalated {
            id: id.to_string(),
            rationale: verdict.rationale.clone(),
            options: verdict
                .options
                .iter()
                .filter_map(|option| serde_json::to_value(option).ok())
                .collect(),
        });
    }
    fn on_stream_retry(&self, reason: &str) {
        let _ = self.tx.send(StreamEvent::StreamReset {
            reason: reason.to_string(),
        });
    }
    fn ask_permission<'a>(
        &'a self,
        name: &'a str,
        args: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = crate::agent_core::runner::PermissionDecision>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if name == "bash" {
                let parsed: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
                let cmd = parsed.get("command").and_then(|v| v.as_str()).unwrap_or("");
                match crate::runtime::permissions::bash_safety::evaluate(cmd) {
                    crate::runtime::permissions::bash_safety::Verdict::Deny(reason) => {
                        return crate::agent_core::runner::PermissionDecision::DenyWithReason(
                            reason,
                        );
                    }
                    crate::runtime::permissions::bash_safety::Verdict::Prompt(_) => {}
                    crate::runtime::permissions::bash_safety::Verdict::Allow => {
                        if crate::runtime::permissions::allow_rule_matches(name, args, &self.cwd) {
                            return crate::agent_core::runner::PermissionDecision::Allow;
                        }
                    }
                }
            } else if crate::runtime::permissions::allow_rule_matches(name, args, &self.cwd) {
                return crate::agent_core::runner::PermissionDecision::Allow;
            }
            let agent_mode = crate::runtime::permissions::current_agent_permission_mode();
            if self.require_permission_approval
                || agent_mode != crate::runtime::permissions::AgentPermissionMode::Full
            {
                return self
                    .permission_broker
                    .request(
                        &self.session_id,
                        name,
                        args,
                        &self.cwd,
                        std::time::Duration::from_secs(5 * 60),
                    )
                    .await;
            }
            if name == "write" || name == "edit" || name == "remove_file" {
                return crate::agent_core::runner::PermissionDecision::Allow;
            }
            if name != "bash" {
                return crate::agent_core::runner::PermissionDecision::Deny;
            }
            let parsed: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
            let cmd = parsed.get("command").and_then(|v| v.as_str()).unwrap_or("");
            match crate::runtime::permissions::bash_safety::evaluate(cmd) {
                crate::runtime::permissions::bash_safety::Verdict::Allow => {
                    crate::agent_core::runner::PermissionDecision::Allow
                }
                crate::runtime::permissions::bash_safety::Verdict::Deny(reason) => {
                    crate::agent_core::runner::PermissionDecision::DenyWithReason(reason)
                }
                crate::runtime::permissions::bash_safety::Verdict::Prompt(reason) => {
                    crate::agent_core::runner::PermissionDecision::DenyWithReason(reason)
                }
            }
        })
    }
    fn on_text_delta(&self, delta: &str) {
        let _ = self.tx.send(StreamEvent::TextDelta(delta.to_string()));
    }
    fn on_thinking_delta(&self, delta: &str) {
        let _ = self.tx.send(StreamEvent::ThinkingDelta(delta.to_string()));
    }
    fn on_context_usage(&self, usage: crate::ai::llm::TokenUsage, call_index: u32) {
        let _ = self
            .tx
            .send(StreamEvent::ContextUsage { usage, call_index });
    }
    fn progress_emitter_for(
        &self,
        id: &str,
    ) -> Option<crate::runtime::tools::progress::ProgressEmitter> {
        let tx = self.tx.clone();
        let id = id.to_string();
        Some(std::sync::Arc::new(move |line: &str| {
            let _ = tx.send(StreamEvent::ToolProgress {
                id: id.clone(),
                line: line.to_string(),
            });
        }))
    }
    fn image_emitter_for(&self, id: &str) -> Option<crate::runtime::tools::progress::ImageEmitter> {
        let tx = self.tx.clone();
        let id = id.to_string();
        Some(std::sync::Arc::new(move |url: &str, alt: &str, record| {
            let _ = tx.send(StreamEvent::ToolImage {
                id: id.clone(),
                url: url.to_string(),
                alt: alt.to_string(),
                record: record
                    .and_then(|value| serde_json::to_value(value).ok())
                    .map(Box::new),
            });
        }))
    }
}

async fn stream_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let model_info = req
        .provider_override
        .as_ref()
        .map(|po| {
            format!(
                "{}({})",
                po.model,
                po.protocol.as_deref().unwrap_or("configured")
            )
        })
        .unwrap_or_else(|| "default".to_string());
    let msg_count = req.messages.len();
    let last_user_msg = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.chars().take(80).collect::<String>())
        .unwrap_or_default();
    info!(
        session_id = %id,
        model = %model_info,
        msg_count = msg_count,
        thinking = req.thinking,
        last_user_msg = %last_user_msg,
        "stream_chat request"
    );

    let session_name_for_stream = req.session_name.clone();
    let state = state.clone();
    let stream = async_stream::stream! {
        let Some(mut session) = state.session_manager.get(&id) else {
            yield Ok(super::sse::stream_event_to_sse(StreamEvent::FirstToken));
            yield Ok(super::sse::stream_event_to_sse(StreamEvent::Error("会话不存在或已删除".to_string())));
            return;
        };
        let Some(bound_workspace) = session.workspace.as_ref() else {
            yield Ok(super::sse::stream_event_to_sse(StreamEvent::FirstToken));
            yield Ok(super::sse::stream_event_to_sse(StreamEvent::Error("请先为历史会话绑定工作文件夹".to_string())));
            return;
        };
        let bound_cwd = bound_workspace.canonical_path.clone();
        if let Some(request_cwd) = req.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
            let request_matches = crate::runtime::tools::fs_local::FsSandbox::from_config()
                .and_then(|sandbox| sandbox.resolve_existing_directory(request_cwd))
                .is_ok_and(|path| path == bound_cwd);
            if !request_matches {
                yield Ok(super::sse::stream_event_to_sse(StreamEvent::FirstToken));
                yield Ok(super::sse::stream_event_to_sse(StreamEvent::Error("请求工作目录与会话绑定目录不一致".to_string())));
                return;
            }
        }
        if let Some(ref sn) = session_name_for_stream {
            if session.name != *sn {
                session.name = sn.clone();
                state.session_manager.update(session.clone());
            }
        }
        let user_slug = state
            .config
            .user
            .as_ref()
            .and_then(|u| u.slug.clone())
            .unwrap_or_else(|| "local".to_string());
        let mem_opts = crate::app::cli::commands::memory_injection_opts_from_features(
            &state.config.features,
            crate::app::cli::commands::last_user_query_from_chat_messages(&req.messages),
        );
        let mut system_prompt = match req.system_prompt {
            Some(sp) => crate::app::cli::commands::enhance_system_prompt(&sp, &user_slug, &mem_opts).await,
            None => {
                crate::app::cli::commands::get_system_prompt_with_catalog(
                    &state.backend,
                    &user_slug,
                    &mem_opts,
                )
                .await
            }
        };
        system_prompt.push_str(&format!(
                    "\n\n## 当前工作目录\n\n用户已设置会话工作目录为 `{}`。\n\
                    - 使用 code_agent 时默认 cwd 传此路径\n\
                    - 使用 ls / read / write 时优先使用相对于此目录的路径\n\
                    - 用户提到的相对路径默认相对此目录",
                    bound_cwd.display()
                ));
        crate::app::cli::commands::append_web_search_note(&mut system_prompt);
        crate::app::cli::commands::append_response_language_policy(&mut system_prompt);
        crate::app::cli::commands::append_final_answer_contract(&mut system_prompt);
        if let Some(executor) = req
            .delegation_preference
            .as_deref()
            .filter(|executor| !executor.trim().is_empty())
        {
            system_prompt.push_str(&format!(
                "\n\n## 捕获事项执行约束\n\n这是一个异步捕获事项。请先判断合适 role/access，随后调用 dispatch_tasks；executor 必须设为 `{executor}`。发布成功后立即结束本回合，不要在主进程内自行等待。"
            ));
        }
        let mut messages_for_task = req.messages;
        if !req.internal_callback {
            append_latest_user_message(&mut session, &messages_for_task);
        }
        // Persist the accepted user turn before starting provider/tool work so a
        // daemon crash cannot erase the request that was already acknowledged.
        state.session_manager.update(session.clone());
        if session
            .messages
            .first()
            .is_some_and(|message| message.id.starts_with("msg_compact_"))
        {
            messages_for_task = session.to_chat_messages();
        }
        let image_references_for_task =
            crate::runtime::visual_generation::build_image_reference_registry(&mut messages_for_task);
        let provider = match resolve_web_provider_selection(&state, req.provider_override.as_ref()) {
            Ok(resolved) => resolved,
            Err(error) => {
                warn!(error = %error, "invalid provider override");
                yield Ok(super::sse::stream_event_to_sse(StreamEvent::FirstToken));
                yield Ok(super::sse::stream_event_to_sse(StreamEvent::Error(error.to_string())));
                return;
            }
        };

        let harness_for_task = session_harness(&state, &id).await;
        let runtime = match create_web_runtime(
            &state,
            &id,
            session_name_for_stream.clone(),
            req.work_item_id.clone(),
            bound_cwd.clone(),
            req.thinking,
            system_prompt,
            Arc::clone(&harness_for_task),
            provider,
            image_references_for_task,
        )
        .await
        {
            Ok(runtime) => runtime,
            Err(error) => {
                warn!(error = %error, "failed to create Web AgentRuntime");
                yield Ok(super::sse::stream_event_to_sse(StreamEvent::FirstToken));
                yield Ok(super::sse::stream_event_to_sse(StreamEvent::Error(error.to_string())));
                return;
            }
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();

        // The daemon owns the agent run. The SSE response is only a subscriber:
        // disconnecting a Web/TUI client must not cancel the run or skip the
        // durable session update performed below.
        let session_for_task = session.clone();
        let mut usage_for_task = crate::ai::usage::UsageTracker::new();
        let state_for_task = state.clone();
        let require_permission_approval = req.require_permission_approval;
        let request_cwd = bound_cwd.to_string_lossy().into_owned();

        if let Err(error) = state.session_manager.mark_turn_started(&id) {
            yield Ok(super::sse::stream_event_to_sse(StreamEvent::FirstToken));
            yield Ok(super::sse::stream_event_to_sse(StreamEvent::Error(error.to_string())));
            return;
        }

        let session_id_owned = id.clone();
        tokio::spawn(async move {
            let sink = ChannelSink {
                tx,
                permission_broker: Arc::clone(&state_for_task.permission_broker),
                session_id: session_id_owned.clone(),
                cwd: request_cwd,
                require_permission_approval,
            };
            let audit_sink = state_for_task
                .session_manager
                .audit_sink(session_id_owned.clone());
            let mut session_local = session_for_task;
            let result = execute_turn(
                &runtime,
                &sink,
                Some(&audit_sink),
                &state_for_task.session_manager,
                &session_id_owned,
                &mut messages_for_task,
                &mut session_local,
                &mut usage_for_task,
            )
            .await;
            finalize_turn(&state_for_task, user_slug, &session_local);
            if result.is_err() {
                let _ = state_for_task
                    .session_manager
                    .mark_turn_settled(&session_id_owned, true);
            }

            match result {
                Ok(Some(ref summary)) => {
                    info!(
                        session_id = %session_id_owned,
                        content_len = summary.content.len(),
                        has_tool_calls = summary.tool_calls.is_some(),
                        token_usage = ?summary.token_usage,
                        "stream_chat completed"
                    );
                    let _ = sink.tx.send(StreamEvent::Done(summary.token_usage));
                }
                Ok(None) => {
                    info!(session_id = %session_id_owned, "stream_chat completed (no summary)");
                    let _ = sink.tx.send(StreamEvent::Done(None));
                }
                Err(ref error) => {
                    error!(session_id = %session_id_owned, error = %error, "stream_chat failed");
                    let _ = sink.tx.send(StreamEvent::Error(error.to_string()));
                }
            }
            // `sink` drops here. If a client is still attached, rx observes EOF
            // after the terminal event; otherwise the durable work is already done.
        });

        // Do not acknowledge the stream until the daemon-owned task has been
        // detached. Receiving this event guarantees that client disconnect is safe.
        yield Ok(super::sse::stream_event_to_sse(StreamEvent::FirstToken));

        // Drain channel until task closes the sender.
        while let Some(ev) = rx.recv().await {
            yield Ok(super::sse::stream_event_to_sse(ev));
        }

    };

    Sse::new(stream)
}

/// SSE 端点：推送指定 session 的后台任务事件
async fn background_events(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.background_tasks.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let (event_type, data) = match &event {
                        crate::runtime::background::TaskEvent::Started { session_id: sid, .. } => {
                            if sid != &session_id { continue; }
                            ("background_task_started", serde_json::to_string(&event).unwrap_or_default())
                        }
                        crate::runtime::background::TaskEvent::Completed(r) => {
                            if r.session_id != session_id { continue; }
                            ("background_task_completed", serde_json::to_string(r).unwrap_or_default())
                        }
                        crate::runtime::background::TaskEvent::Cancelled(r) => {
                            if r.session_id != session_id { continue; }
                            ("background_task_cancelled", serde_json::to_string(r).unwrap_or_default())
                        }
                    };
                    yield Ok(Event::default().event(event_type).data(data));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream)
}

/// SSE 端点：推送所有 session 的后台任务事件（全局）
async fn background_events_global(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.background_tasks.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let (event_type, data) = match &event {
                        crate::runtime::background::TaskEvent::Started { .. } => {
                            ("background_task_started", serde_json::to_string(&event).unwrap_or_default())
                        }
                        crate::runtime::background::TaskEvent::Completed(r) => {
                            ("background_task_completed", serde_json::to_string(r).unwrap_or_default())
                        }
                        crate::runtime::background::TaskEvent::Cancelled(r) => {
                            ("background_task_cancelled", serde_json::to_string(r).unwrap_or_default())
                        }
                    };
                    yield Ok(Event::default().event(event_type).data(data));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream)
}

/// 列出所有后台任务
async fn list_background_tasks(
    State(state): State<AppState>,
) -> Json<Vec<crate::runtime::background::TaskInfo>> {
    let mut tasks = state.background_tasks.list().await;
    for task in &mut tasks {
        if let Some(session) = state.session_manager.get(&task.session_id) {
            task.session_name = Some(session.name.clone());
        }
    }
    Json(tasks)
}

/// 取消 + 移除后台任务
async fn cancel_background_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Json<serde_json::Value> {
    state.background_tasks.cancel(&task_id).await;
    let status = if state.background_tasks.remove(&task_id).await {
        "removed"
    } else {
        "cancelled_or_not_found"
    };
    Json(serde_json::json!({ "status": status, "taskId": task_id }))
}

async fn retry_background_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    state
        .background_tasks
        .retry_tool(&task_id, Arc::clone(&state.tool_registry))
        .await
        .map(|new_task_id| Json(serde_json::json!({ "taskId": new_task_id })))
        .map_err(|error| {
            let message = error.to_string();
            let status = if message.contains("not replayable") {
                axum::http::StatusCode::CONFLICT
            } else if message.contains("not found") {
                axum::http::StatusCode::NOT_FOUND
            } else {
                axum::http::StatusCode::BAD_REQUEST
            };
            (status, message)
        })
}

/// 关闭 session 时取消该 session 的所有后台任务
async fn cancel_session_background_tasks(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    let count = state.background_tasks.cancel_by_session(&session_id).await;
    Json(serde_json::json!({ "cancelled": count }))
}

async fn list_tools(State(state): State<AppState>) -> Json<Vec<serde_json::Value>> {
    let tools: Vec<_> = state
        .tool_registry
        .list_definitions()
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name,
                "description": d.description,
                "parameters": d.parameters
            })
        })
        .collect();
    Json(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat_message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn append_latest_user_message_only_appends_current_user_turn() {
        let mut session = crate::runtime::session::Session::new("test");
        let messages = vec![
            chat_message("user", "earlier question"),
            chat_message("assistant", "earlier answer"),
            chat_message("user", "current question"),
        ];

        append_latest_user_message(&mut session, &messages);

        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].text_content(), "current question");
    }

    #[test]
    fn append_latest_user_message_ignores_empty_user_content() {
        let mut session = crate::runtime::session::Session::new("test");
        let messages = vec![chat_message("user", "   ")];

        append_latest_user_message(&mut session, &messages);

        assert!(session.messages.is_empty());
    }

    #[test]
    fn runtime_callback_release_is_not_forwarded_as_a_user_message() {
        let callback =
            serde_json::from_value::<crate::runtime::session::QueuedInput>(serde_json::json!({
                "id": "queue-callback",
                "clientMessageId": "runtime-batch:1:ready",
                "sequence": 1,
                "delivery": "next_turn",
                "status": "claimed",
                "source": "runtime_callback",
                "priority": "high",
                "message": {
                    "role": "user",
                    "content": "internal callback",
                    "images": []
                },
                "createdAt": "2026-07-30T00:00:00Z"
            }))
            .unwrap();
        let user =
            serde_json::from_value::<crate::runtime::session::QueuedInput>(serde_json::json!({
                "id": "queue-user",
                "clientMessageId": "client-user",
                "sequence": 2,
                "delivery": "next_turn",
                "status": "queued",
                "message": {
                    "role": "user",
                    "content": "user follow-on",
                    "images": []
                },
                "createdAt": "2026-07-30T00:00:01Z"
            }))
            .unwrap();

        assert!(released_message_for_client(&callback).is_none());
        assert_eq!(
            released_message_for_client(&user).unwrap().content,
            "user follow-on"
        );
    }

    #[test]
    fn persisted_turn_is_eligible_for_memory_extraction() {
        let mut session = crate::runtime::session::Session::new("test");
        let messages = vec![chat_message("user", "remember this preference")];

        append_latest_user_message(&mut session, &messages);
        session.add_message(ConversationMessage::new_assistant("understood"));

        let turn = crate::runtime::memory::turn_messages_slice(&session.messages);
        assert!(crate::runtime::memory::format_turn_messages(turn).is_some());
    }

    #[test]
    fn provider_index_resolution_uses_server_secret_and_model_metadata() {
        let provider = super::super::web::ProviderEndpoint {
            id: "provider-example".into(),
            name: "Example".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key: "server-secret".into(),
            protocol: "openai".into(),
            use_proxy: None,
            compat_profile: Some("credential:provider-example".into()),
            models: vec![serde_json::json!({
                "id": "Qwen3.7-Max-DogFooding",
                "name": "Qwen 3.7 Max DogFooding",
                "maxOutput": 65536,
                "contextWindow": 990998
            })],
        };

        let resolved = provider_config_for_model(provider, "Qwen3.7-Max-DogFooding").unwrap();

        assert_eq!(resolved.api_key, "server-secret");
        assert_eq!(resolved.model, "Qwen3.7-Max-DogFooding");
        assert_eq!(resolved.current_model_max_output(), Some(65536));
        assert_eq!(resolved.current_model_context_window(), Some(990998));
    }

    #[test]
    fn provider_index_resolution_rejects_unknown_model() {
        let provider = super::super::web::ProviderEndpoint {
            id: "provider-example".into(),
            name: "Example".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key: "server-secret".into(),
            protocol: "openai".into(),
            use_proxy: None,
            compat_profile: Some("credential:provider-example".into()),
            models: vec![],
        };

        let error = provider_config_for_model(provider, "missing").unwrap_err();

        assert!(error.to_string().contains("not configured"));
    }
}
