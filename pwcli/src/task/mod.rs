use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::tools::code_agent::{
    execute_code_agent_with_listener, CodeAgentArgs, CodeAgentUsage, CodexBackend, KimiBackend,
    NativeSessionListener, QoderBackend, SubAgentBackend,
};

pub mod attention;
pub mod continuation;
pub mod routing;
pub mod worktree;

pub use attention::RuntimeAttention;

fn failure_from_status_and_error(
    status: &str,
    error: Option<&str>,
    attempt: u32,
) -> crate::reliability::FailureEnvelope {
    crate::reliability::classify_runtime_failure(
        status,
        error.unwrap_or_default(),
        attempt.saturating_sub(1),
        crate::reliability::MAX_AUTO_RETRIES,
    )
}

fn attention_kind_status(kind: &str) -> &str {
    match kind {
        "merge_required" => "merge_required",
        "paused_callback" | "waiting_user" | "decision_required" => "waiting_user",
        "task_failed" | "output_failed" | "background_failure" => "failed",
        "background_interrupted" => "recovery_required",
        other => other,
    }
}

fn enrich_attention_detail(kind: &str, title: &str, detail: Value) -> Value {
    let mut detail = if detail.is_object() {
        detail
    } else {
        serde_json::json!({})
    };
    if detail.get("failure").is_none() {
        let status = attention_kind_status(kind);
        let error = detail
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| detail.get("reason").and_then(|v| v.as_str()))
            .unwrap_or(title);
        let failure = failure_from_status_and_error(status, Some(error), 1);
        if let Ok(value) = serde_json::to_value(failure) {
            if let Some(object) = detail.as_object_mut() {
                object.insert("failure".into(), value);
            }
        }
    }
    detail
}

fn payload_with_failure(
    payload: &str,
    failure: Option<&crate::reliability::FailureEnvelope>,
) -> Result<String> {
    let mut value =
        serde_json::from_str::<Value>(payload).unwrap_or_else(|_| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    if let Some(object) = value.as_object_mut() {
        match failure {
            Some(failure) => {
                object.insert("failure".into(), serde_json::to_value(failure)?);
            }
            None => {
                object.remove("failure");
            }
        }
    }
    Ok(value.to_string())
}

fn create_attention_with_failure_tx(
    transaction: &rusqlite::Transaction<'_>,
    task_id: &str,
    kind: &str,
    dedupe_key: &str,
    title: &str,
    detail: Value,
    failure: Option<&crate::reliability::FailureEnvelope>,
) -> Result<Option<String>> {
    let mut detail = if detail.is_object() {
        detail
    } else {
        serde_json::json!({})
    };
    if let Some(failure) = failure {
        if let Some(object) = detail.as_object_mut() {
            object.insert("failure".into(), serde_json::to_value(failure)?);
        }
    }
    let detail = enrich_attention_detail(kind, title, detail);
    attention::create_attention_tx(transaction, task_id, kind, dedupe_key, title, detail)
}

fn derive_failure_from_row(
    status: &str,
    error: Option<&str>,
    attempt: u32,
    metadata: &Value,
) -> Option<crate::reliability::FailureEnvelope> {
    if let Some(failure) = metadata
        .get("failure")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
    {
        return Some(failure);
    }
    if !matches!(
        status,
        "failed" | "recovery_required" | "waiting_user" | "waiting_configuration" | "cancelled"
    ) && metadata.get("outputStatus").and_then(Value::as_str) != Some("failed")
    {
        return None;
    }
    let status_key = if metadata.get("outputStatus").and_then(Value::as_str) == Some("failed") {
        "output_failed"
    } else {
        status
    };
    Some(failure_from_status_and_error(
        status_key,
        error,
        attempt.max(1),
    ))
}

const MAX_BATCH_TASKS: usize = 16;
const MAX_DEPTH: u32 = 4;
const MAX_DESCENDANT_TASKS: i64 = 64;
const MAX_ACTIVE_WORKERS: i64 = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegatedRole {
    Researcher,
    Engineer,
    Reviewer,
    Analyst,
    Operator,
    #[default]
    General,
}

impl DelegatedRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Researcher => "researcher",
            Self::Engineer => "engineer",
            Self::Reviewer => "reviewer",
            Self::Analyst => "analyst",
            Self::Operator => "operator",
            Self::General => "general",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegatedAccess {
    #[default]
    ReadOnly,
    Mutating,
}

impl DelegatedAccess {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Mutating => "mutating",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegatedExecutor {
    #[default]
    Auto,
    Pwcli,
    Codex,
    Qoder,
    Kimi,
}

impl DelegatedExecutor {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pwcli => "pwcli",
            Self::Codex => "codex",
            Self::Qoder => "qoder",
            Self::Kimi => "kimi",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedModelSnapshot {
    #[serde(default)]
    pub provider_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub thinking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTaskSpec {
    pub objective: String,
    #[serde(default)]
    pub deliverable_title: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub role: DelegatedRole,
    #[serde(default)]
    pub access: DelegatedAccess,
    #[serde(default)]
    pub executor: DelegatedExecutor,
    /// Legacy compatibility fields. New callers use `role/access/executor`.
    #[serde(default = "default_task_kind")]
    pub kind: String,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub depth: u32,
    #[serde(default)]
    pub resume_session_id: Option<String>,
    #[serde(default)]
    pub resolved_executor_kind: Option<String>,
    #[serde(default)]
    pub resolved_executor_id: Option<String>,
    #[serde(default)]
    pub resolved_provider_id: Option<String>,
    #[serde(default)]
    pub resolved_model: Option<String>,
    #[serde(default)]
    pub resolved_effort: Option<String>,
    #[serde(default)]
    pub resolved_thinking: bool,
    #[serde(default)]
    pub resolved_permission_mode: Option<String>,
    #[serde(default)]
    pub routing_reason: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role_label: Option<String>,
    #[serde(default)]
    pub avatar_seed: Option<String>,
    #[serde(default)]
    pub worktree_lease: Option<worktree::WorktreeLease>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub continuation_refs: Vec<continuation::ContinuationArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_model_context: Option<DelegatedModelSnapshot>,
}

impl RuntimeTaskSpec {
    pub fn delegated(objective: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            objective: objective.into(),
            deliverable_title: None,
            cwd: cwd.into(),
            role: DelegatedRole::General,
            access: DelegatedAccess::ReadOnly,
            executor: DelegatedExecutor::Auto,
            kind: default_task_kind(),
            backend: None,
            mode: None,
            model: None,
            effort: None,
            permission_mode: None,
            parent_task_id: None,
            depth: 0,
            resume_session_id: None,
            resolved_executor_kind: None,
            resolved_executor_id: None,
            resolved_provider_id: None,
            resolved_model: None,
            resolved_effort: None,
            resolved_thinking: false,
            resolved_permission_mode: None,
            routing_reason: None,
            display_name: None,
            role_label: None,
            avatar_seed: None,
            worktree_lease: None,
            continuation_refs: Vec::new(),
            publisher_model_context: None,
        }
    }
}

fn default_task_kind() -> String {
    "child_cli".to_string()
}

fn normalize_legacy_spec(spec: &mut RuntimeTaskSpec) {
    if spec.mode.as_deref() == Some("edit") {
        spec.access = DelegatedAccess::Mutating;
    }
    if spec.role == DelegatedRole::General && spec.mode.as_deref() == Some("research") {
        spec.role = DelegatedRole::Researcher;
    }
    if spec.executor == DelegatedExecutor::Auto {
        spec.executor = match spec.backend.as_deref() {
            Some("codex") => DelegatedExecutor::Codex,
            Some("qoder") => DelegatedExecutor::Qoder,
            Some("kimi") => DelegatedExecutor::Kimi,
            _ if spec.kind == "child_agent" => DelegatedExecutor::Pwcli,
            _ => DelegatedExecutor::Auto,
        };
    }
    if spec.deliverable_title.is_none() {
        spec.deliverable_title = Some(spec.objective.chars().take(80).collect());
    }
}

fn assign_stable_identity(
    spec: &mut RuntimeTaskSpec,
    root_session_id: &str,
    lineage_root_id: &str,
    used_names: &[String],
) {
    let delegation = crate::config::local_config::get().tools.delegation;
    let configured = delegation.roles.get(spec.role.as_str());
    let label = configured
        .map(|role| role.label.clone())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| spec.role.as_str().to_string());
    let pool = configured
        .map(|role| role.names.clone())
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| {
            vec![
                "Jamie".to_string(),
                "Taylor".to_string(),
                "Morgan".to_string(),
            ]
        });
    let seed = sha256(format!(
        "{root_session_id}:{lineage_root_id}:{}",
        spec.role.as_str()
    ));
    let offset = u64::from_str_radix(&seed[..16], 16).unwrap_or_default() as usize;
    let name = (0..pool.len())
        .map(|index| pool[(offset + index) % pool.len()].clone())
        .find(|candidate| !used_names.iter().any(|used| used == candidate))
        .unwrap_or_else(|| pool[offset % pool.len()].clone());
    spec.display_name = Some(name);
    spec.role_label = Some(label);
    spec.avatar_seed = Some(seed[..12].to_string());
}

fn resolve_execution_spec(spec: &mut RuntimeTaskSpec) -> Option<String> {
    use routing::{ExecutorCapabilities, RoutingDecision, RoutingRequest};

    let config = crate::config::local_config::get().tools.delegation;
    let inherited_model = spec
        .publisher_model_context
        .as_ref()
        .map(|context| context.model.clone())
        .filter(|model| !model.trim().is_empty());
    let inherited_effort = spec
        .publisher_model_context
        .as_ref()
        .and_then(|context| context.effort.clone());
    let mut capabilities = vec![ExecutorCapabilities {
        executor_id: "pwcli".to_string(),
        installed: true,
        supported_roles: Vec::new(),
        models: Vec::new(),
        efforts: Vec::new(),
        permission_modes: Vec::new(),
        native_default_model: inherited_model.or_else(|| {
            crate::config::RuntimeConfig::load()
                .active_provider()
                .map(|provider| provider.model.clone())
        }),
        native_default_effort: inherited_effort.or_else(|| spec.effort.clone()),
        native_default_permission_mode: Some("default".to_string()),
    }];
    capabilities.extend(
        crate::tools::code_agent::list_available_backends(true)
            .into_iter()
            .map(|backend| {
                let efforts = backend
                    .models
                    .iter()
                    .flat_map(|model| model.effort_options.iter().cloned())
                    .collect::<Vec<_>>();
                ExecutorCapabilities {
                    executor_id: backend.name,
                    installed: backend.available,
                    supported_roles: Vec::new(),
                    models: backend.models.into_iter().map(|model| model.id).collect(),
                    efforts,
                    permission_modes: backend
                        .permission_modes
                        .into_iter()
                        .map(|mode| mode.id)
                        .collect(),
                    native_default_model: None,
                    native_default_effort: None,
                    native_default_permission_mode: Some("default".to_string()),
                }
            }),
    );
    let request = RoutingRequest {
        role: spec.role.as_str().to_string(),
        executor: Some(spec.executor.as_str().to_string()),
        model: spec.model.clone(),
        effort: spec.effort.clone(),
        permission_mode: spec.permission_mode.clone(),
    };
    match routing::resolve_routing(&config, &request, &capabilities) {
        RoutingDecision::Resolved(snapshot) => {
            let internal = snapshot.resolved_executor_kind == "pwcli";
            spec.resolved_executor_kind = Some(if internal {
                "internal_agent".to_string()
            } else {
                "acp_cli".to_string()
            });
            spec.resolved_executor_id = Some(if internal {
                "pwcli".to_string()
            } else {
                snapshot.resolved_executor_id.clone()
            });
            spec.resolved_model = snapshot.model.clone();
            spec.resolved_effort = snapshot.effort.clone();
            if internal {
                inherit_publisher_model_snapshot(spec);
            }
            spec.resolved_permission_mode = snapshot.permission_mode.clone();
            spec.routing_reason = Some(snapshot.routing_reason);
            spec.kind = "delegated".to_string();
            spec.backend = (!internal).then_some(snapshot.resolved_executor_id);
            spec.mode = Some(
                match spec.access {
                    DelegatedAccess::ReadOnly => "research",
                    DelegatedAccess::Mutating => "edit",
                }
                .to_string(),
            );
            None
        }
        RoutingDecision::WaitingConfiguration(waiting) => {
            spec.routing_reason = Some(waiting.reason.clone());
            Some(waiting.reason)
        }
    }
}

fn inherit_publisher_model_snapshot(spec: &mut RuntimeTaskSpec) {
    if spec.resolved_executor_kind.as_deref() != Some("internal_agent") {
        return;
    }
    let Some(context) = spec.publisher_model_context.as_ref() else {
        return;
    };
    spec.resolved_provider_id = context.provider_id.clone();
    spec.resolved_model = (!context.model.trim().is_empty()).then(|| context.model.clone());
    spec.resolved_effort = context.effort.clone();
    spec.resolved_thinking = context.thinking;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitTaskBatch {
    pub root_session_id: String,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub work_item_id: Option<String>,
    #[serde(default)]
    pub session_generation: Option<u64>,
    pub idempotency_key: String,
    #[serde(default = "default_join_mode")]
    pub join: String,
    pub tasks: Vec<RuntimeTaskSpec>,
}

fn default_join_mode() -> String {
    "all".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTaskRecord {
    pub id: String,
    pub batch_id: String,
    pub root_session_id: String,
    pub parent_task_id: Option<String>,
    pub kind: String,
    pub objective: String,
    pub deliverable_title: Option<String>,
    pub cwd: String,
    pub backend: Option<String>,
    pub model: Option<String>,
    pub role: String,
    pub access: String,
    pub executor_request: String,
    pub resolved_executor_kind: Option<String>,
    pub resolved_executor_id: Option<String>,
    pub resolved_provider_id: Option<String>,
    pub resolved_model: Option<String>,
    pub resolved_effort: Option<String>,
    pub resolved_permission_mode: Option<String>,
    pub routing_reason: Option<String>,
    pub display_name: Option<String>,
    pub role_label: Option<String>,
    pub avatar_seed: Option<String>,
    pub work_item_id: Option<String>,
    pub session_generation: Option<u64>,
    pub review_revision: u32,
    pub depth: u32,
    pub primary_document_id: Option<String>,
    pub output_status: String,
    pub review_status: String,
    pub status: String,
    pub delivery_status: String,
    pub attempt: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub result: Option<Value>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<crate::reliability::FailureEnvelope>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBatchAccepted {
    pub batch_id: String,
    pub task_ids: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub sequence: u64,
    pub event_id: String,
    pub task_id: String,
    pub attempt_id: Option<String>,
    pub kind: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteAttemptRequest {
    pub event_id: String,
    pub callback_token: String,
    pub lease_epoch: u64,
    pub outcome: String,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub native_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptEventRequest {
    pub event_id: String,
    pub callback_token: String,
    pub lease_epoch: u64,
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitChildTaskBatch {
    pub callback_token: String,
    pub lease_epoch: u64,
    pub idempotency_key: String,
    #[serde(default = "default_join_mode")]
    pub join: String,
    pub tasks: Vec<RuntimeTaskSpec>,
}

#[derive(Debug, Clone)]
pub struct WorkerDispatchContext {
    pub callback_base: String,
    pub attempt_id: String,
    pub callback_token: String,
    pub lease_epoch: u64,
}

pub type WorkerDispatchSignal = Arc<std::sync::Mutex<Option<TaskBatchAccepted>>>;

pub fn register_worker_dispatch_tool(
    registry: &crate::tools::registry::ToolRegistry,
    context: WorkerDispatchContext,
    signal: WorkerDispatchSignal,
) {
    use crate::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput};

    registry.register_structured_with_impact(
        "dispatch_tasks",
        "Publish durable child tasks, then end this worker turn immediately. The current worker exits; RuntimeTask outbox resumes this parent after every child Markdown output is ready. Never wait or poll.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "idempotencyKey": { "type": "string", "minLength": 1 },
                "join": { "type": "string", "enum": ["all", "each"], "default": "all" },
                "tasks": {
                    "type": "array", "minItems": 1, "maxItems": 16,
                    "items": {
                        "type": "object",
                        "properties": {
                            "objective": { "type": "string", "minLength": 1 },
                            "deliverableTitle": { "type": "string", "minLength": 1 },
                            "cwd": { "type": "string", "minLength": 1 },
                            "role": { "type": "string", "enum": ["researcher", "engineer", "reviewer", "analyst", "operator", "general"] },
                            "access": { "type": "string", "enum": ["read_only", "mutating"] },
                            "executor": { "type": "string", "enum": ["auto", "pwcli", "codex", "qoder", "kimi"] },
                            "model": { "type": "string" },
                            "effort": { "type": "string" },
                            "permissionMode": { "type": "string" }
                        },
                        "required": ["objective", "cwd"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["idempotencyKey", "tasks"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::Control,
        Box::new(move |arguments| {
            let context = context.clone();
            let signal = Arc::clone(&signal);
            let arguments = arguments.clone();
            Box::pin(async move {
                let request = SubmitChildTaskBatch {
                    callback_token: context.callback_token,
                    lease_epoch: context.lease_epoch,
                    idempotency_key: arguments
                        .get("idempotencyKey")
                        .and_then(Value::as_str)
                        .context("idempotencyKey is required")?
                        .to_string(),
                    join: arguments
                        .get("join")
                        .and_then(Value::as_str)
                        .unwrap_or("all")
                        .to_string(),
                    tasks: serde_json::from_value(
                        arguments.get("tasks").cloned().unwrap_or(Value::Null),
                    )?,
                };
                let response = reqwest::Client::new()
                    .post(format!(
                        "{}/internal/task-attempts/{}/tasks/batch",
                        context.callback_base, context.attempt_id
                    ))
                    .json(&request)
                    .send()
                    .await?;
                if !response.status().is_success() {
                    anyhow::bail!(
                        "child task publication failed with {}: {}",
                        response.status(),
                        response.text().await.unwrap_or_default()
                    );
                }
                let accepted = response.json::<TaskBatchAccepted>().await?;
                *signal
                    .lock()
                    .map_err(|_| anyhow::anyhow!("worker dispatch signal poisoned"))? =
                    Some(accepted.clone());
                Ok(ToolOutput {
                    content: format!(
                        "已发布 {} 个子任务（批次 {}）。当前 worker 现在退出，等待持久回调恢复。",
                        accepted.task_ids.len(), accepted.batch_id
                    ),
                    terminate: true,
                    details: Some(serde_json::json!({ "delegation": accepted })),
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
}

#[derive(Debug, Clone)]
pub struct CompletionProjection {
    pub task: RuntimeTaskRecord,
    pub batch_completed: bool,
    pub root_session_id: String,
    pub callback_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutboxRecord {
    pub id: String,
    pub source_event_id: String,
    pub destination: String,
    pub dedupe_key: String,
    pub payload: Value,
    pub attempt_count: u32,
    pub next_attempt_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskReviewAction {
    Confirm,
    RequestChanges,
    Reject,
    Apply,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskReviewRequest {
    pub client_action_id: String,
    pub review_revision: u32,
    pub action: TaskReviewAction,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub task_update: Option<Value>,
    #[serde(default)]
    pub memory_entries: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskReviewResponse {
    pub task_id: String,
    pub review_status: String,
    pub review_revision: u32,
    pub affected_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDecisionRequest {
    pub client_action_id: String,
    pub option: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDecisionResponse {
    pub task_id: String,
    pub status: String,
    pub selected_option: String,
}

fn should_project_review(review_status: &str, projection_payload: &Value) -> bool {
    review_status == "applied"
        && (!projection_payload["taskUpdate"].is_null()
            || projection_payload["memoryEntries"]
                .as_array()
                .is_some_and(|entries| !entries.is_empty()))
}

fn mark_cancelled_review(payload: &mut Value, access: DelegatedAccess) {
    if access == DelegatedAccess::Mutating {
        payload["reviewStatus"] = Value::String("rejected".to_string());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "adapter", rename_all = "snake_case")]
enum ResolvedExecution {
    PwcliAgent {
        objective: String,
        role: DelegatedRole,
        access: DelegatedAccess,
        provider_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        thinking: bool,
    },
    AcpCli {
        args: CodeAgentArgs,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerDescriptor {
    task_id: String,
    attempt_id: String,
    lease_epoch: u64,
    callback_token: String,
    callback_base: String,
    data_dir: PathBuf,
    cwd: String,
    deliverable_title: String,
    execution: ResolvedExecution,
    #[serde(default)]
    continuation_refs: Vec<continuation::ContinuationArtifactRef>,
    #[serde(default)]
    review_lease: Option<worktree::WorktreeLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerProcessFingerprint {
    version: u8,
    pid: u32,
    start_marker: String,
    executable_identity: String,
    command_sha256: String,
    descriptor_path: String,
    descriptor_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerSignalAudit {
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug)]
struct WorkerStartError {
    error: anyhow::Error,
    side_effects_possible: bool,
}

struct WorkerExecutionOutput {
    status: String,
    output: String,
    native_session_id: Option<String>,
    duration_ms: Option<u64>,
    token_usage: Option<Value>,
    child_batch: Option<TaskBatchAccepted>,
    question: Option<String>,
    options: Vec<String>,
}

type WorkerExecutionResult = Result<WorkerExecutionOutput>;

impl WorkerStartError {
    fn before_launch(error: impl Into<anyhow::Error>) -> Self {
        Self {
            error: error.into(),
            side_effects_possible: false,
        }
    }

    fn after_launch(error: impl Into<anyhow::Error>) -> Self {
        Self {
            error: error.into(),
            side_effects_possible: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDocumentOutcome {
    pub title: String,
    pub format: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationResult {
    pub label: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestedTaskUpdate {
    #[serde(default)]
    pub todo_id: Option<String>,
    #[serde(default)]
    pub progress: Option<u8>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub completed_subtask_ids: Vec<String>,
    #[serde(default)]
    pub mark_complete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionCandidate {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskExecutionOutcome {
    pub summary: String,
    pub document: TaskDocumentOutcome,
    pub changed_files: Vec<ChangedTextFile>,
    pub verification: Vec<VerificationResult>,
    #[serde(default)]
    pub suggested_task_update: Option<SuggestedTaskUpdate>,
    #[serde(default)]
    pub decision_candidates: Vec<DecisionCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_artifact: Option<worktree::ReviewArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskBroker {
    db_path: Arc<PathBuf>,
    data_dir: Arc<PathBuf>,
    callback_base: Arc<String>,
    workspace_root_override: Option<Arc<PathBuf>>,
    event_tx: tokio::sync::broadcast::Sender<TaskEvent>,
    outbox_notify: Arc<Notify>,
    lease_notify: Arc<Notify>,
    review_lock: Arc<std::sync::Mutex<()>>,
    launch_workers: bool,
}

impl TaskBroker {
    pub fn new(data_dir: &Path, callback_base: impl Into<String>) -> Result<Arc<Self>> {
        Self::new_internal(data_dir, callback_base, None)
    }

    #[cfg(test)]
    fn new_with_workspace_root(
        data_dir: &Path,
        callback_base: impl Into<String>,
        workspace_root: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::new_internal(data_dir, callback_base, Some(workspace_root))
    }

    fn new_internal(
        data_dir: &Path,
        callback_base: impl Into<String>,
        workspace_root_override: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        fs::create_dir_all(data_dir)?;
        let workspace_root_override = workspace_root_override
            .map(|root| root.canonicalize().map(Arc::new))
            .transpose()?;
        let (event_tx, _) = tokio::sync::broadcast::channel(1024);
        let broker = Arc::new(Self {
            db_path: Arc::new(data_dir.join("runtime-tasks.db")),
            data_dir: Arc::new(data_dir.to_path_buf()),
            callback_base: Arc::new(callback_base.into()),
            workspace_root_override,
            event_tx,
            outbox_notify: Arc::new(Notify::new()),
            lease_notify: Arc::new(Notify::new()),
            review_lock: Arc::new(std::sync::Mutex::new(())),
            launch_workers: !cfg!(test),
        });
        broker.initialize()?;
        broker.migrate_legacy_supervisor()?;
        broker.backfill_legacy_attention()?;
        broker.import_spool()?;
        broker.recover_orphaned_attempts()?;
        broker.recover_running_background_tools()?;
        broker.recover_review_apply_intents()?;
        Ok(broker)
    }

    fn current_workspace_root(&self) -> Result<PathBuf> {
        let configured = crate::config::local_config::get().tools.fs_base;
        self.workspace_root_for_setting(&configured)
    }

    fn workspace_root_for_setting(&self, configured: &str) -> Result<PathBuf> {
        match &self.workspace_root_override {
            Some(root) => Ok(root.as_ref().clone()),
            None => Ok(
                crate::tools::fs_local::FsSandbox::from_root_setting(configured)?
                    .root()
                    .to_path_buf(),
            ),
        }
    }

    fn connection(&self) -> Result<Connection> {
        let connection = Connection::open(self.db_path.as_ref())?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(connection)
    }

    fn initialize(&self) -> Result<()> {
        let connection = self.connection()?;
        // WAL is persistent for a database file. Switching it once at startup
        // avoids taking a schema lock on every short-lived broker connection.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS runtime_batches (
               id TEXT PRIMARY KEY,
               root_session_id TEXT NOT NULL,
               parent_task_id TEXT,
               work_item_id TEXT,
               session_generation INTEGER,
               join_mode TEXT NOT NULL,
               status TEXT NOT NULL,
               idempotency_key TEXT NOT NULL UNIQUE,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS runtime_tasks (
               id TEXT PRIMARY KEY,
               batch_id TEXT NOT NULL REFERENCES runtime_batches(id) ON DELETE CASCADE,
               root_session_id TEXT NOT NULL,
               parent_task_id TEXT,
               kind TEXT NOT NULL,
               objective TEXT NOT NULL,
               cwd TEXT NOT NULL,
               backend TEXT,
               model TEXT,
               payload_json TEXT NOT NULL,
               status TEXT NOT NULL,
               delivery_status TEXT NOT NULL,
               attempt INTEGER NOT NULL DEFAULT 0,
               result_json TEXT,
               continuation_ref TEXT,
               error TEXT,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS runtime_tasks_status_idx ON runtime_tasks(status, created_at);
             CREATE INDEX IF NOT EXISTS runtime_tasks_workspace_idx ON runtime_tasks(cwd, status);
             CREATE TABLE IF NOT EXISTS task_attempts (
               id TEXT PRIMARY KEY,
               task_id TEXT NOT NULL REFERENCES runtime_tasks(id) ON DELETE CASCADE,
               attempt_no INTEGER NOT NULL,
               lease_epoch INTEGER NOT NULL,
               token_hash TEXT NOT NULL,
               status TEXT NOT NULL,
               worker_pid INTEGER,
               worker_pgid INTEGER,
               process_fingerprint TEXT,
               lease_expires_at TEXT NOT NULL,
               native_session_id TEXT,
               result_hash TEXT,
               continuation_ref TEXT,
               error TEXT,
               started_at TEXT,
               finished_at TEXT
             );
             CREATE TABLE IF NOT EXISTS task_events (
               sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               event_id TEXT NOT NULL UNIQUE,
               task_id TEXT NOT NULL,
               attempt_id TEXT,
               kind TEXT NOT NULL,
               payload_json TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS task_outbox (
               id TEXT PRIMARY KEY,
               source_event_id TEXT NOT NULL,
               destination TEXT NOT NULL,
               dedupe_key TEXT NOT NULL UNIQUE,
               status TEXT NOT NULL,
               payload_json TEXT NOT NULL,
               attempt_count INTEGER NOT NULL DEFAULT 0,
               next_attempt_at TEXT,
               last_error TEXT,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS task_dependencies (
               task_id TEXT NOT NULL REFERENCES runtime_tasks(id) ON DELETE CASCADE,
               depends_on_task_id TEXT NOT NULL REFERENCES runtime_tasks(id) ON DELETE CASCADE,
               PRIMARY KEY(task_id, depends_on_task_id)
             );
             CREATE TABLE IF NOT EXISTS runtime_migrations (
               id TEXT PRIMARY KEY,
               completed_at TEXT NOT NULL,
               detail_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS task_review_actions (
               client_action_id TEXT PRIMARY KEY,
               task_id TEXT NOT NULL,
               action TEXT NOT NULL,
               response_json TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS task_review_apply_intents (
               client_action_id TEXT PRIMARY KEY,
               task_id TEXT NOT NULL,
               review_revision INTEGER NOT NULL,
               artifact_sha256 TEXT NOT NULL,
               state TEXT NOT NULL,
               request_json TEXT NOT NULL,
               affected_paths_json TEXT,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );",
        )?;
        // Indexes that depend on additive columns must be created only after old
        // installations have been upgraded. CREATE TABLE IF NOT EXISTS does not
        // add missing columns to an existing table.
        ensure_column(
            &connection,
            "task_outbox",
            "attempt_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(&connection, "task_outbox", "next_attempt_at", "TEXT")?;
        ensure_column(&connection, "task_outbox", "last_error", "TEXT")?;
        ensure_column(&connection, "runtime_batches", "work_item_id", "TEXT")?;
        ensure_column(&connection, "runtime_batches", "parent_task_id", "TEXT")?;
        ensure_column(&connection, "runtime_tasks", "continuation_ref", "TEXT")?;
        ensure_column(&connection, "runtime_tasks", "failure_json", "TEXT")?;
        ensure_column(&connection, "task_attempts", "continuation_ref", "TEXT")?;
        ensure_column(&connection, "task_attempts", "process_fingerprint", "TEXT")?;
        ensure_column(
            &connection,
            "runtime_batches",
            "session_generation",
            "INTEGER",
        )?;
        connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS runtime_batches_work_item_idx
               ON runtime_batches(work_item_id);",
        )?;
        attention::initialize(&connection)?;
        connection.execute(
            "UPDATE task_outbox SET status='pending' WHERE status='processing'",
            [],
        )?;
        Ok(())
    }

    /// Import the retired Supervisor task/child/dispatch journal once. RuntimeTask keeps
    /// the original identifiers so old chat projections and review links remain traceable.
    /// Active legacy work is fenced as `recovery_required`: it may already have produced
    /// side effects before the daemon stopped and must never be replayed automatically.
    fn migrate_legacy_supervisor(&self) -> Result<()> {
        const MIGRATION_ID: &str = "supervisor-child-dispatch-to-runtime-task-v1";
        let legacy_path = self.data_dir.join("supervisor.db");
        if !legacy_path.is_file() {
            return Ok(());
        }

        let mut connection = self.connection()?;
        if connection
            .query_row(
                "SELECT 1 FROM runtime_migrations WHERE id=?1",
                [MIGRATION_ID],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            archive_legacy_supervisor(&legacy_path);
            return Ok(());
        }
        connection.execute(
            "ATTACH DATABASE ?1 AS legacy_supervisor",
            [legacy_path.to_string_lossy().as_ref()],
        )?;
        let has_legacy_tables = connection.query_row(
            "SELECT COUNT(*)=4 FROM legacy_supervisor.sqlite_master
             WHERE type='table' AND name IN (
               'supervisor_tasks','supervisor_children','supervisor_dispatches','supervisor_events'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !has_legacy_tables {
            connection.execute("DETACH DATABASE legacy_supervisor", [])?;
            return Ok(());
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS legacy_supervisor.supervisor_agent_sessions (
               task_id TEXT NOT NULL,
               actor TEXT NOT NULL,
               session_id TEXT NOT NULL,
               PRIMARY KEY(task_id, actor)
             );",
        )?;

        for (table, column, definition) in [
            (
                "supervisor_tasks",
                "phase",
                "TEXT NOT NULL DEFAULT 'planning'",
            ),
            (
                "supervisor_tasks",
                "preferred_backend",
                "TEXT NOT NULL DEFAULT 'auto'",
            ),
            ("supervisor_tasks", "commander_session_id", "TEXT"),
            ("supervisor_tasks", "source_task_id", "TEXT"),
            ("supervisor_tasks", "execution_plan", "TEXT"),
            ("supervisor_tasks", "current_step_id", "TEXT"),
            ("supervisor_tasks", "run_started_at", "TEXT"),
            ("supervisor_tasks", "last_activity_at", "TEXT"),
            ("supervisor_tasks", "deadline_at", "TEXT"),
            (
                "supervisor_children",
                "implementation",
                "TEXT NOT NULL DEFAULT 'unknown'",
            ),
            ("supervisor_children", "native_session_id", "TEXT"),
            ("supervisor_children", "worktree_path", "TEXT"),
            ("supervisor_children", "worktree_root", "TEXT"),
            ("supervisor_children", "baseline_commit", "TEXT"),
            ("supervisor_children", "review_revision", "INTEGER"),
            (
                "supervisor_children",
                "depends_on",
                "TEXT NOT NULL DEFAULT '[]'",
            ),
            ("supervisor_children", "model", "TEXT"),
            ("supervisor_children", "effort", "TEXT"),
            ("supervisor_children", "context_window", "INTEGER"),
            ("supervisor_children", "permission_mode", "TEXT"),
            ("supervisor_children", "cwd", "TEXT"),
            (
                "supervisor_children",
                "context_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "supervisor_children",
                "context_window_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "supervisor_children",
                "resume_metadata",
                "TEXT NOT NULL DEFAULT '{}'",
            ),
            ("supervisor_dispatches", "parent_dispatch_id", "TEXT"),
            ("supervisor_dispatches", "resolves_dispatch_id", "TEXT"),
            ("supervisor_dispatches", "resolved_by_dispatch_id", "TEXT"),
            ("supervisor_dispatches", "resolution_json", "TEXT"),
            (
                "supervisor_dispatches",
                "version",
                "INTEGER NOT NULL DEFAULT 1",
            ),
            ("supervisor_dispatches", "response", "TEXT"),
            ("supervisor_dispatches", "error", "TEXT"),
            ("supervisor_events", "child_id", "TEXT"),
            ("supervisor_events", "dispatch_id", "TEXT"),
        ] {
            ensure_legacy_column(&connection, table, column, definition)?;
        }

        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "INSERT OR IGNORE INTO runtime_batches
               (id, root_session_id, join_mode, status, idempotency_key, created_at, updated_at)
             SELECT 'legacy_batch:' || task.id,
                    COALESCE(
                      NULLIF(task.commander_session_id, ''),
                      (SELECT session.session_id FROM legacy_supervisor.supervisor_agent_sessions session
                       WHERE session.task_id=task.id AND session.actor IN ('commander','pwcli') LIMIT 1),
                      'legacy:' || task.id
                    ),
                    'all',
                    CASE task.status
                      WHEN 'completed' THEN 'completed'
                      WHEN 'failed' THEN 'completed'
                      WHEN 'cancelled' THEN 'completed'
                      ELSE 'recovery_required'
                    END,
                    'legacy-supervisor:' || task.id,
                    task.created_at,
                    task.updated_at
             FROM legacy_supervisor.supervisor_tasks task;

             INSERT OR IGNORE INTO runtime_tasks
               (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd, backend,
                model, payload_json, status, delivery_status, attempt, result_json, error,
                created_at, updated_at)
             SELECT task.id,
                    'legacy_batch:' || task.id,
                    COALESCE(
                      NULLIF(task.commander_session_id, ''),
                      (SELECT session.session_id FROM legacy_supervisor.supervisor_agent_sessions session
                       WHERE session.task_id=task.id AND session.actor IN ('commander','pwcli') LIMIT 1),
                      'legacy:' || task.id
                    ),
                    NULL,
                    'supervisor_root',
                    task.objective,
                    task.project_dir,
                    NULLIF(task.preferred_backend, 'auto'),
                    NULL,
                    json_object(
                      'legacyType', 'supervisor_task',
                      'legacyId', task.id,
                      'title', substr(task.objective, 1, 80),
                      'phase', task.phase,
                      'sourceTaskId', task.source_task_id,
                      'executionPlan', CASE WHEN json_valid(task.execution_plan) THEN json(task.execution_plan) ELSE NULL END,
                      'currentStepId', task.current_step_id,
                      'runStartedAt', task.run_started_at,
                      'lastActivityAt', task.last_activity_at,
                      'deadlineAt', task.deadline_at
                    ),
                    CASE task.status
                      WHEN 'completed' THEN 'succeeded'
                      WHEN 'failed' THEN 'failed'
                      WHEN 'cancelled' THEN 'cancelled'
                      ELSE 'recovery_required'
                    END,
                    'legacy_imported',
                    0,
                    CASE WHEN task.result IS NULL THEN NULL ELSE json_object('summary', task.result) END,
                    CASE WHEN task.status='failed' THEN task.result ELSE NULL END,
                    task.created_at,
                    task.updated_at
             FROM legacy_supervisor.supervisor_tasks task;

             INSERT OR IGNORE INTO runtime_tasks
               (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd, backend,
                model, payload_json, status, delivery_status, attempt, result_json, error,
                created_at, updated_at)
             SELECT child.id,
                    'legacy_batch:' || child.task_id,
                    COALESCE(
                      NULLIF(task.commander_session_id, ''),
                      (SELECT session.session_id FROM legacy_supervisor.supervisor_agent_sessions session
                       WHERE session.task_id=task.id AND session.actor IN ('commander','pwcli') LIMIT 1),
                      'legacy:' || child.task_id
                    ),
                    child.task_id,
                    'supervisor_child',
                    child.label,
                    COALESCE(NULLIF(child.cwd, ''), NULLIF(child.worktree_path, ''), task.project_dir),
                    child.backend,
                    child.model,
                    json_object(
                      'legacyType', 'supervisor_child',
                      'legacyId', child.id,
                      'implementation', child.implementation,
                      'label', child.label,
                      'mode', child.mode,
                      'sessionId', child.session_id,
                      'nativeSessionId', child.native_session_id,
                      'worktreePath', child.worktree_path,
                      'worktreeRoot', child.worktree_root,
                      'baselineCommit', child.baseline_commit,
                      'reviewRevision', child.review_revision,
                      'dependsOn', CASE WHEN json_valid(child.depends_on) THEN json(child.depends_on) ELSE json('[]') END,
                      'effort', child.effort,
                      'contextWindow', child.context_window,
                      'permissionMode', child.permission_mode,
                      'contextTokens', child.context_tokens,
                      'contextWindowTokens', child.context_window_tokens,
                      'resumeMetadata', CASE WHEN json_valid(child.resume_metadata) THEN json(child.resume_metadata) ELSE json('{}') END
                    ),
                    CASE child.status
                      WHEN 'completed' THEN 'succeeded'
                      WHEN 'failed' THEN 'failed'
                      WHEN 'cancelled' THEN 'cancelled'
                      ELSE 'recovery_required'
                    END,
                    'legacy_imported',
                    0,
                    CASE WHEN child.summary IS NULL THEN NULL ELSE json_object('summary', child.summary) END,
                    CASE WHEN child.status='failed' THEN child.summary ELSE NULL END,
                    child.created_at,
                    child.updated_at
             FROM legacy_supervisor.supervisor_children child
             JOIN legacy_supervisor.supervisor_tasks task ON task.id=child.task_id;

             INSERT OR IGNORE INTO runtime_tasks
               (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd, backend,
                model, payload_json, status, delivery_status, attempt, result_json, error,
                created_at, updated_at)
             SELECT dispatch.id,
                    'legacy_batch:' || dispatch.task_id,
                    COALESCE(
                      NULLIF(task.commander_session_id, ''),
                      (SELECT session.session_id FROM legacy_supervisor.supervisor_agent_sessions session
                       WHERE session.task_id=task.id AND session.actor IN ('commander','pwcli') LIMIT 1),
                      'legacy:' || dispatch.task_id
                    ),
                    dispatch.child_id,
                    'supervisor_dispatch',
                    dispatch.content,
                    COALESCE(NULLIF(child.cwd, ''), NULLIF(child.worktree_path, ''), task.project_dir),
                    child.backend,
                    child.model,
                    json_object(
                      'legacyType', 'supervisor_dispatch',
                      'legacyId', dispatch.id,
                      'parentDispatchId', dispatch.parent_dispatch_id,
                      'resolvesDispatchId', dispatch.resolves_dispatch_id,
                      'resolvedByDispatchId', dispatch.resolved_by_dispatch_id,
                      'resolution', CASE WHEN json_valid(dispatch.resolution_json) THEN json(dispatch.resolution_json) ELSE NULL END,
                      'version', dispatch.version,
                      'summary', dispatch.summary
                    ),
                    CASE dispatch.status
                      WHEN 'completed' THEN 'succeeded'
                      WHEN 'resolved' THEN 'succeeded'
                      WHEN 'failed' THEN 'failed'
                      WHEN 'cancelled' THEN 'cancelled'
                      WHEN 'interrupted' THEN 'recovery_required'
                      ELSE 'recovery_required'
                    END,
                    'legacy_imported',
                    0,
                    CASE
                      WHEN dispatch.response IS NOT NULL THEN json_object('summary', dispatch.response)
                      WHEN dispatch.summary IS NOT NULL THEN json_object('summary', dispatch.summary)
                      ELSE NULL
                    END,
                    dispatch.error,
                    dispatch.created_at,
                    dispatch.updated_at
             FROM legacy_supervisor.supervisor_dispatches dispatch
             JOIN legacy_supervisor.supervisor_tasks task ON task.id=dispatch.task_id
             JOIN legacy_supervisor.supervisor_children child ON child.id=dispatch.child_id;

             INSERT OR IGNORE INTO task_dependencies (task_id, depends_on_task_id)
             SELECT child.id, dependency.value
             FROM legacy_supervisor.supervisor_children child,
                  json_each(CASE WHEN json_valid(child.depends_on) THEN child.depends_on ELSE '[]' END) dependency
             WHERE EXISTS (SELECT 1 FROM runtime_tasks parent WHERE parent.id=dependency.value);

             INSERT OR IGNORE INTO task_events
               (event_id, task_id, attempt_id, kind, payload_json, created_at)
             SELECT 'legacy-supervisor-event:' || event.sequence,
                    event.task_id,
                    NULL,
                    'legacy_' || event.kind,
                    json_object(
                      'actor', event.actor,
                      'childId', event.child_id,
                      'dispatchId', event.dispatch_id,
                      'message', event.message,
                      'detail', CASE WHEN json_valid(event.detail) THEN json(event.detail) ELSE json('{}') END,
                      'legacySequence', event.sequence
                    ),
                    event.created_at
             FROM legacy_supervisor.supervisor_events event
             WHERE EXISTS (SELECT 1 FROM runtime_tasks task WHERE task.id=event.task_id);

             INSERT OR IGNORE INTO task_events
               (event_id, task_id, attempt_id, kind, payload_json, created_at)
             SELECT 'legacy-dispatch:' || dispatch.id,
                    dispatch.id,
                    NULL,
                    'legacy_dispatch_migrated',
                    json_object(
                      'taskId', dispatch.task_id,
                      'childId', dispatch.child_id,
                      'status', dispatch.status,
                      'content', dispatch.content,
                      'response', dispatch.response,
                      'error', dispatch.error
                    ),
                    dispatch.updated_at
             FROM legacy_supervisor.supervisor_dispatches dispatch
             WHERE EXISTS (SELECT 1 FROM runtime_tasks task WHERE task.id=dispatch.id);

             UPDATE runtime_tasks
             SET payload_json=json_set(
                   payload_json,
                   '$.outputStatus', 'materializing',
                   '$.reviewStatus', 'not_required'
                 ),
                 delivery_status='not_ready'
             WHERE kind IN ('supervisor_root','supervisor_child')
               AND status='succeeded' AND result_json IS NOT NULL;

             INSERT OR IGNORE INTO task_events
               (event_id, task_id, attempt_id, kind, payload_json, created_at)
             SELECT 'legacy-output:' || id,
                    id,
                    'legacy-attempt:' || id,
                    'legacy_output_queued',
                    json_object('legacy', true),
                    updated_at
             FROM runtime_tasks
             WHERE kind IN ('supervisor_root','supervisor_child')
               AND status='succeeded' AND result_json IS NOT NULL;

             INSERT OR IGNORE INTO task_outbox
               (id, source_event_id, destination, dedupe_key, status, payload_json,
                created_at, updated_at)
             SELECT 'legacy-output-outbox:' || id,
                    'legacy-output:' || id,
                    'document',
                    'runtime-task-output:legacy:' || id,
                    'pending',
                    json_object(
                      'taskId', id,
                      'attemptId', 'legacy-attempt:' || id,
                      'batchId', batch_id,
                      'rootSessionId', root_session_id,
                      'executorId', COALESCE(backend, 'legacy'),
                      'agentName', COALESCE(json_extract(payload_json, '$.label'), '旧版委派'),
                      'title', COALESCE(json_extract(payload_json, '$.title'), objective),
                      'body', COALESCE(json_extract(result_json, '$.summary'), '旧委派任务已完成。')
                    ),
                    updated_at,
                    updated_at
             FROM runtime_tasks
             WHERE kind IN ('supervisor_root','supervisor_child')
               AND status='succeeded' AND result_json IS NOT NULL;"
        )?;
        let detail = serde_json::json!({
            "source": legacy_path,
            "policy": "active legacy executions fenced as recovery_required"
        });
        transaction.execute(
            "INSERT INTO runtime_migrations (id, completed_at, detail_json) VALUES (?1, ?2, ?3)",
            params![MIGRATION_ID, Utc::now().to_rfc3339(), detail.to_string()],
        )?;
        transaction.commit()?;
        connection.execute("DETACH DATABASE legacy_supervisor", [])?;
        archive_legacy_supervisor(&legacy_path);
        Ok(())
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TaskEvent> {
        self.event_tx.subscribe()
    }

    pub fn outbox_notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.outbox_notify)
    }

    pub fn lease_notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.lease_notify)
    }

    pub fn next_lease_expiry(&self) -> Result<Option<DateTime<Utc>>> {
        self.connection()?
            .query_row(
                "SELECT MIN(lease_expires_at) FROM task_attempts
                 WHERE status IN ('leased','starting','running')",
                [],
                |row| row.get::<_, Option<String>>(0),
            )?
            .map(parse_time)
            .transpose()
            .map_err(Into::into)
    }

    /// Atomically claim the next durable outbox item. A daemon restart resets an
    /// interrupted `processing` claim to `pending`, so delivery is at-least-once
    /// while destination-side dedupe keeps projections exactly-once.
    pub fn claim_outbox(&self) -> Result<Option<TaskOutboxRecord>> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let now = Utc::now().to_rfc3339();
        let row = transaction
            .query_row(
                "SELECT id, source_event_id, destination, dedupe_key, payload_json,
                        attempt_count, COALESCE(next_attempt_at, created_at)
                 FROM task_outbox
                 WHERE status='pending' AND COALESCE(next_attempt_at, created_at)<=?1
                 ORDER BY created_at LIMIT 1",
                [&now],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u32>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, source_event_id, destination, dedupe_key, payload, attempt_count, next)) =
            row
        else {
            return Ok(None);
        };
        let claimed = transaction.execute(
            "UPDATE task_outbox SET status='processing', updated_at=?2
             WHERE id=?1 AND status='pending'",
            params![id, now],
        )?;
        if claimed == 0 {
            return Ok(None);
        }
        transaction.commit()?;
        Ok(Some(TaskOutboxRecord {
            id,
            source_event_id,
            destination,
            dedupe_key,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            attempt_count,
            next_attempt_at: parse_time(next)?,
        }))
    }

    pub fn complete_outbox(&self, id: &str) -> Result<()> {
        self.connection()?.execute(
            "UPDATE task_outbox SET status='delivered', last_error=NULL, updated_at=?2
             WHERE id=?1 AND status='processing'",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn fail_outbox(&self, id: &str, error: &str) -> Result<()> {
        self.connection()?.execute(
            "UPDATE task_outbox SET status='failed', last_error=?2, updated_at=?3
             WHERE id=?1 AND status IN ('processing','pending')",
            params![id, error, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn retry_outbox(&self, id: &str, error: &str) -> Result<()> {
        let connection = self.connection()?;
        let attempts: u32 = connection.query_row(
            "SELECT attempt_count + 1 FROM task_outbox WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        let backoff_seconds = 2_i64.pow(attempts.min(8)).min(300);
        connection.execute(
            "UPDATE task_outbox SET status='pending', attempt_count=?2, last_error=?3,
                    next_attempt_at=?4, updated_at=?5 WHERE id=?1",
            params![
                id,
                attempts,
                error,
                (Utc::now() + chrono::Duration::seconds(backoff_seconds)).to_rfc3339(),
                Utc::now().to_rfc3339()
            ],
        )?;
        self.outbox_notify.notify_one();
        Ok(())
    }

    /// Permanently fail a document projection and its outbox row in one
    /// transaction. The terminal task remains readable, but is explicitly
    /// surfaced as durable user attention instead of being retried forever.
    pub fn fail_document_outbox(
        &self,
        outbox_id: &str,
        task_id: &str,
        attempt_id: &str,
        error: &str,
    ) -> Result<()> {
        let mut connection = self.connection()?;
        let raw_payload: String = connection
            .query_row(
                "SELECT payload_json FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| row.get(0),
            )
            .context("document outbox references a missing RuntimeTask")?;
        let mut payload = serde_json::from_str::<Value>(&raw_payload)?;
        if let Value::Object(object) = &mut payload {
            object.insert("outputStatus".into(), Value::String("failed".into()));
        }
        let failure = failure_from_status_and_error("output_failed", Some(error), 1);
        if let Value::Object(object) = &mut payload {
            object.insert("failure".into(), serde_json::to_value(&failure)?);
        }
        let now = Utc::now().to_rfc3339();
        let event_id = format!("{attempt_id}:document-failed");
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET payload_json=?2, delivery_status='waiting_user',
                    error=?3, updated_at=?4 WHERE id=?1",
            params![task_id, payload.to_string(), error, now],
        )?;
        transaction.execute(
            "UPDATE runtime_batches SET status='output_failed', updated_at=?2
             WHERE id=(SELECT batch_id FROM runtime_tasks WHERE id=?1)",
            params![task_id, now],
        )?;
        transaction.execute(
            "UPDATE task_outbox SET status='failed', last_error=?2, updated_at=?3
             WHERE id=?1 AND status IN ('processing','pending')",
            params![outbox_id, error, now],
        )?;
        insert_event_tx(
            &transaction,
            &event_id,
            task_id,
            Some(attempt_id),
            "task_output_failed",
            serde_json::json!({
                "error": error,
                "persistentAttention": true,
                "failure": failure,
            }),
        )?;
        create_attention_with_failure_tx(
            &transaction,
            task_id,
            "output_failed",
            &format!("runtime-task:{task_id}:attempt:{attempt_id}:output-failed"),
            "协作者产出整理失败",
            serde_json::json!({ "attemptId": attempt_id, "error": error }),
            Some(&failure),
        )?;
        transaction.commit()?;
        self.emit_persisted_event(&event_id)
    }

    pub fn next_outbox_due_at(&self) -> Result<Option<DateTime<Utc>>> {
        self.connection()?
            .query_row(
                "SELECT MIN(COALESCE(next_attempt_at, created_at)) FROM task_outbox WHERE status='pending'",
                [],
                |row| row.get::<_, Option<String>>(0),
            )?
            .map(parse_time)
            .transpose()
            .map_err(Into::into)
    }

    pub fn mark_document_ready(
        &self,
        task_id: &str,
        attempt_id: &str,
        document_id: &str,
    ) -> Result<bool> {
        let mut connection = self.connection()?;
        let (
            batch_id,
            root_session_id,
            payload,
            session_generation,
            work_item_id,
            parent_task_id,
            join_mode,
        ): (
            String,
            String,
            String,
            Option<u64>,
            Option<String>,
            Option<String>,
            String,
        ) = connection
            .query_row(
                "SELECT task.batch_id, task.root_session_id, task.payload_json,
                        batch.session_generation, batch.work_item_id, batch.parent_task_id,
                        batch.join_mode
                 FROM runtime_tasks task
                 JOIN runtime_batches batch ON batch.id=task.batch_id
                 WHERE task.id=?1",
                [task_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .context("runtime task not found while materializing document")?;
        let mut payload = serde_json::from_str::<Value>(&payload)?;
        if let Value::Object(object) = &mut payload {
            object.insert(
                "primaryDocumentId".to_string(),
                Value::String(document_id.to_string()),
            );
            object.insert(
                "outputStatus".to_string(),
                Value::String("ready".to_string()),
            );
        }
        let now = Utc::now().to_rfc3339();
        let event_id = format!("{attempt_id}:document-ready");
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET payload_json=?2, delivery_status='review_ready', updated_at=?3
             WHERE id=?1",
            params![task_id, payload.to_string(), now],
        )?;
        insert_event_tx(
            &transaction,
            &event_id,
            task_id,
            Some(attempt_id),
            "task_output_ready",
            serde_json::json!({ "documentId": document_id }),
        )?;
        attention::create_attention_tx(
            &transaction,
            task_id,
            "document_ready",
            &format!("runtime-task-output:{attempt_id}:ready"),
            "协作者产出待处理",
            serde_json::json!({
                "documentId": document_id,
                "attemptId": attempt_id,
                "reviewRevision": payload
                    .get("reviewRevision")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            }),
        )?;
        let remaining: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM runtime_tasks
             WHERE batch_id=?1 AND (
               status NOT IN ('succeeded','failed','cancelled') OR
               COALESCE(json_extract(payload_json, '$.outputStatus'), 'pending')!='ready'
             )",
            [&batch_id],
            |row| row.get(0),
        )?;
        let batch_completed = remaining == 0;
        if batch_completed {
            transaction.execute(
                "UPDATE runtime_batches SET status='completed', updated_at=?2 WHERE id=?1",
                params![batch_id, now],
            )?;
        }
        if join_mode == "each" || batch_completed {
            let (summary, task_ids, dedupe_key) = if join_mode == "each" {
                (
                    task_summary(&transaction, task_id)?,
                    vec![task_id.to_string()],
                    format!("runtime-batch:{batch_id}:task:{task_id}:ready"),
                )
            } else {
                (
                    batch_summary(&transaction, &batch_id)?,
                    task_ids_for_batch(&transaction, &batch_id)?,
                    format!("runtime-batch:{batch_id}:ready"),
                )
            };
            let destination = parent_task_id
                .as_ref()
                .map(|parent| format!("parent_task:{parent}"))
                .unwrap_or_else(|| root_session_id.clone());
            transaction.execute(
                "INSERT OR IGNORE INTO task_outbox
                 (id, source_event_id, destination, dedupe_key, status, payload_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?6)",
                params![
                    format!("outbox_{}", uuid::Uuid::now_v7().simple()),
                    event_id,
                    destination,
                    dedupe_key,
                    serde_json::json!({
                        "batchId": batch_id,
                        "taskIds": task_ids,
                        "summary": summary,
                        "sessionGeneration": session_generation,
                        "workItemId": work_item_id,
                        "parentTaskId": parent_task_id,
                        "joinMode": join_mode,
                    }).to_string(),
                    now,
                ],
            )?;
        }
        transaction.commit()?;
        self.emit_persisted_event(&event_id)?;
        if join_mode == "each" || batch_completed {
            self.outbox_notify.notify_one();
        }
        Ok(batch_completed)
    }

    pub fn resume_parent_from_children(
        &self,
        parent_task_id: &str,
        callback: &Value,
    ) -> Result<()> {
        let batch_id = callback
            .get("batchId")
            .and_then(Value::as_str)
            .context("child batch callback is missing batchId")?;
        let callback_key = callback
            .get("taskIds")
            .and_then(Value::as_array)
            .and_then(|ids| ids.first())
            .and_then(Value::as_str)
            .unwrap_or(batch_id);
        let event_id = format!("{parent_task_id}:children-ready:{batch_id}:{callback_key}");
        let mut connection = self.connection()?;
        if connection
            .query_row(
                "SELECT 1 FROM task_events WHERE event_id=?1",
                [&event_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Ok(());
        }
        let (raw_payload, parent_batch_id, status, prior_result): (
            String,
            String,
            String,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT payload_json, batch_id, status, result_json
                 FROM runtime_tasks WHERE id=?1",
                [parent_task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .context("parent RuntimeTask not found")?;
        if is_terminal(&status) {
            return Ok(());
        }
        let continuation_ref = materialize_child_continuation(
            self.data_dir.as_ref(),
            &connection,
            parent_task_id,
            batch_id,
            callback,
        )?;
        let summary = callback
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("子任务批次已完成");
        let mut event_payload = callback.clone();
        if let Value::Object(object) = &mut event_payload {
            object.insert(
                "continuationRef".to_string(),
                serde_json::to_value(&continuation_ref)?,
            );
        }
        if status != "waiting_children" && status != "queued" {
            let mut spec = serde_json::from_str::<RuntimeTaskSpec>(&raw_payload)?;
            merge_continuation_ref(&mut spec.continuation_refs, continuation_ref.clone());
            let mut payload = serde_json::from_str::<Value>(&serialize_spec_preserving_runtime(
                &spec,
                &raw_payload,
            )?)?;
            let pending = payload
                .as_object_mut()
                .context("parent RuntimeTask payload must be an object")?
                .entry("pendingContinuationRefs")
                .or_insert_with(|| Value::Array(Vec::new()));
            let pending = pending
                .as_array_mut()
                .context("pendingContinuationRefs must be an array")?;
            if !pending.iter().any(|item| {
                item.get("reference")
                    .and_then(|value| value.get("artifactId"))
                    .and_then(Value::as_str)
                    == Some(continuation_ref.artifact_id.as_str())
            }) {
                pending.push(serde_json::json!({
                    "reference": continuation_ref,
                    "summary": summary,
                }));
            }
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE runtime_tasks SET payload_json=?2, continuation_ref=?3, updated_at=?4
                 WHERE id=?1 AND status NOT IN ('succeeded','failed','cancelled')",
                params![
                    parent_task_id,
                    payload.to_string(),
                    serde_json::to_string(&spec.continuation_refs)?,
                    Utc::now().to_rfc3339()
                ],
            )?;
            insert_event_tx(
                &transaction,
                &event_id,
                parent_task_id,
                None,
                "task_child_result_buffered",
                event_payload,
            )?;
            transaction.commit()?;
            self.emit_persisted_event(&event_id)?;
            return Ok(());
        }
        let mut spec = serde_json::from_str::<RuntimeTaskSpec>(&raw_payload)?;
        merge_continuation_ref(&mut spec.continuation_refs, continuation_ref.clone());
        if let Some(parent_summary) = prior_result
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|result| {
                result
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
        {
            spec.objective.push_str(&format!(
                "\n\n## 父任务上一轮进展\n{}",
                parent_summary.chars().take(1_000).collect::<String>()
            ));
        }
        if !spec.objective.contains(&continuation_ref.artifact_id) {
            spec.objective.push_str(&continuation_objective_marker(
                batch_id,
                summary,
                &continuation_ref,
            ));
        }
        let mut payload = serde_json::from_str::<Value>(&serialize_spec_preserving_runtime(
            &spec,
            &raw_payload,
        )?)?;
        if let Value::Object(object) = &mut payload {
            object.insert("outputStatus".into(), Value::String("pending".into()));
            object.insert(
                "reviewStatus".into(),
                Value::String(
                    if spec.access == DelegatedAccess::Mutating {
                        "pending"
                    } else {
                        "not_required"
                    }
                    .into(),
                ),
            );
        }
        let now = Utc::now().to_rfc3339();
        let transaction = connection.transaction()?;
        if status == "waiting_children" {
            transaction.execute(
                "UPDATE runtime_tasks SET status='queued', payload_json=?2, objective=?3,
                        continuation_ref=?4, result_json=NULL, error=NULL, updated_at=?5
                 WHERE id=?1 AND status='waiting_children'",
                params![
                    parent_task_id,
                    payload.to_string(),
                    spec.objective,
                    serde_json::to_string(&spec.continuation_refs)?,
                    now
                ],
            )?;
        } else {
            transaction.execute(
                "UPDATE runtime_tasks SET payload_json=?2, objective=?3, continuation_ref=?4,
                        updated_at=?5
                 WHERE id=?1 AND status='queued'",
                params![
                    parent_task_id,
                    payload.to_string(),
                    spec.objective,
                    serde_json::to_string(&spec.continuation_refs)?,
                    now
                ],
            )?;
        }
        transaction.execute(
            "UPDATE runtime_batches SET status='running', updated_at=?2 WHERE id=?1",
            params![parent_batch_id, now],
        )?;
        insert_event_tx(
            &transaction,
            &event_id,
            parent_task_id,
            None,
            "task_children_ready",
            event_payload,
        )?;
        transaction.commit()?;
        self.emit_persisted_event(&event_id)?;
        if self.launch_workers {
            self.try_start(parent_task_id)?;
        }
        Ok(())
    }

    pub fn callback_base(&self) -> &str {
        self.callback_base.as_str()
    }

    pub fn data_dir(&self) -> &Path {
        self.data_dir.as_ref()
    }

    pub(crate) fn import_callback_spool(&self) -> Result<()> {
        self.import_spool()
    }

    pub fn events_after(&self, after: u64) -> Result<Vec<TaskEvent>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT sequence, event_id, task_id, attempt_id, kind, payload_json, created_at
             FROM task_events WHERE sequence>?1 ORDER BY sequence",
        )?;
        let events = statement
            .query_map([after], row_to_event)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into);
        events
    }

    pub fn list(&self) -> Result<Vec<RuntimeTaskRecord>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, batch_id, root_session_id, parent_task_id, kind, objective, cwd,
                    backend, model, status, delivery_status, attempt, created_at, updated_at,
                    result_json, error, payload_json
             FROM runtime_tasks ORDER BY created_at DESC",
        )?;
        let tasks = statement
            .query_map([], row_to_task)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into);
        tasks
    }

    pub fn list_attention(&self) -> Result<Vec<RuntimeAttention>> {
        attention::list(&self.connection()?)
    }

    /// One-time compatibility projection for tasks that already needed user
    /// action before durable attention was introduced. Any historical
    /// attention row, including a resolved one, fences the task from being
    /// reopened on later daemon starts.
    fn backfill_legacy_attention(&self) -> Result<usize> {
        let existing = self
            .list_attention()?
            .into_iter()
            .map(|item| item.task_id)
            .collect::<std::collections::HashSet<_>>();
        let mut inserted = 0;
        for task in self.list()? {
            if existing.contains(&task.id) {
                continue;
            }
            let kind = if task.output_status == "failed" {
                Some("output_failed")
            } else if task.review_status == "merge_required" {
                Some("merge_required")
            } else if task.status == "waiting_user" {
                Some("waiting_user")
            } else if task.status == "waiting_configuration" {
                Some("waiting_configuration")
            } else if matches!(task.status.as_str(), "failed" | "recovery_required") {
                Some("task_failed")
            } else if task.review_status == "pending"
                || (task.access == "read_only"
                    && task.status == "succeeded"
                    && task.output_status == "ready"
                    && !matches!(task.review_status.as_str(), "applied" | "rejected"))
            {
                Some("document_ready")
            } else {
                None
            };
            let Some(kind) = kind else { continue };
            let title = task
                .deliverable_title
                .as_deref()
                .unwrap_or(task.objective.as_str());
            if self
                .create_attention(
                    &task.id,
                    kind,
                    &format!(
                        "legacy-attention:{}:{}:{}",
                        task.id, task.review_revision, kind
                    ),
                    title,
                    serde_json::json!({
                        "backfilled": true,
                        "reviewRevision": task.review_revision,
                    }),
                )?
                .is_some()
            {
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    pub fn attention(&self, id: &str) -> Result<Option<RuntimeAttention>> {
        attention::get(&self.connection()?, id)
    }

    /// Project an ordinary background tool into a lightweight RuntimeTask so
    /// failures can reuse durable Attention without launching workers.
    pub fn project_background_tool(
        &self,
        session_id: &str,
        background_task_id: &str,
        tool_name: &str,
        description: &str,
        arguments: Option<Value>,
        cwd: &str,
    ) -> Result<RuntimeTaskRecord> {
        let replayable = crate::reliability::is_idempotent_read_tool(tool_name);
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let now = Utc::now().to_rfc3339();
        let batch_id = format!("batch_bg_{}", uuid::Uuid::now_v7().simple());
        let task_id = format!("task_bg_{}", uuid::Uuid::now_v7().simple());
        let objective = if description.trim().is_empty() {
            format!("后台工具 {tool_name}")
        } else {
            description.to_string()
        };
        let mut payload = serde_json::json!({
            "mode": "background_tool",
            "toolName": tool_name,
            "backgroundTaskId": background_task_id,
            "replayable": replayable,
            "outputStatus": "pending",
            "reviewStatus": "not_required",
            "deliverableTitle": objective.chars().take(80).collect::<String>(),
            "objective": objective,
            "cwd": cwd,
            "role": "operator",
            "access": "read_only",
            "executor": "pwcli",
            "kind": "background_tool",
        });
        if replayable {
            if let Some(arguments) = arguments {
                if let Some(object) = payload.as_object_mut() {
                    object.insert("toolArguments".into(), arguments);
                }
            }
        }
        transaction.execute(
            "INSERT INTO runtime_batches
             (id, root_session_id, parent_task_id, work_item_id, session_generation, join_mode,
              status, idempotency_key, created_at, updated_at)
             VALUES (?1, ?2, NULL, NULL, NULL, 'all', 'running', ?3, ?4, ?4)",
            params![
                batch_id,
                session_id,
                format!("background-tool:{background_task_id}"),
                now
            ],
        )?;
        transaction.execute(
            "INSERT INTO runtime_tasks
             (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd, backend, model,
              payload_json, status, delivery_status, attempt, error, created_at, updated_at)
             VALUES (?1, ?2, ?3, NULL, 'background_tool', ?4, ?5, NULL, NULL, ?6, 'running',
                     'not_ready', 1, NULL, ?7, ?7)",
            params![
                task_id,
                batch_id,
                session_id,
                objective,
                cwd,
                payload.to_string(),
                now
            ],
        )?;
        insert_event_tx(
            &transaction,
            &format!("{task_id}:background-started"),
            &task_id,
            None,
            "task_started",
            serde_json::json!({
                "backgroundTaskId": background_task_id,
                "toolName": tool_name,
                "replayable": replayable,
            }),
        )?;
        transaction.commit()?;
        self.emit_persisted_event(&format!("{task_id}:background-started"))?;
        self.task(&task_id)?
            .context("background RuntimeTask disappeared")
    }

    pub fn complete_background_tool(
        &self,
        task_id: &str,
        success: bool,
        summary: &str,
    ) -> Result<RuntimeTaskRecord> {
        let mut connection = self.connection()?;
        let (raw_payload, current_status, attempt): (String, String, u32) = connection
            .query_row(
                "SELECT payload_json, status, attempt FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("background RuntimeTask not found")?;
        if matches!(
            current_status.as_str(),
            "succeeded" | "failed" | "cancelled"
        ) {
            return self
                .task(task_id)?
                .context("background RuntimeTask not found");
        }
        let mut payload =
            serde_json::from_str::<Value>(&raw_payload).unwrap_or(serde_json::json!({}));
        let now = Utc::now().to_rfc3339();
        let transaction = connection.transaction()?;
        if success {
            if let Value::Object(object) = &mut payload {
                object.insert("outputStatus".into(), Value::String("ready".into()));
                object.remove("failure");
            }
            let result = serde_json::json!({
                "summary": summary,
                "document": {
                    "title": payload.get("deliverableTitle").and_then(Value::as_str).unwrap_or("后台工具结果"),
                    "format": "markdown",
                    "body": summary,
                }
            });
            transaction.execute(
                "UPDATE runtime_tasks SET status='succeeded', delivery_status='delivered',
                        result_json=?2, error=NULL, payload_json=?3, updated_at=?4 WHERE id=?1",
                params![task_id, result.to_string(), payload.to_string(), now],
            )?;
            transaction.execute(
                "UPDATE runtime_batches SET status='completed', updated_at=?2
                 WHERE id=(SELECT batch_id FROM runtime_tasks WHERE id=?1)",
                params![task_id, now],
            )?;
            insert_event_tx(
                &transaction,
                &format!("{task_id}:background-succeeded"),
                task_id,
                None,
                "task_completed",
                serde_json::json!({ "summary": summary }),
            )?;
            attention::resolve_task_tx(
                &transaction,
                task_id,
                Some(&[
                    "background_failure",
                    "background_interrupted",
                    "task_failed",
                ]),
            )?;
            transaction.commit()?;
            self.emit_persisted_event(&format!("{task_id}:background-succeeded"))?;
        } else {
            let tool_name = payload
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("background_tool")
                .to_string();
            let mut envelope = crate::reliability::classify_tool_failure(
                &tool_name,
                summary,
                crate::reliability::FailureSource::BackgroundTool,
                attempt.saturating_sub(1),
                crate::reliability::MAX_AUTO_RETRIES,
            );
            let replayable = payload
                .get("replayable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !replayable {
                envelope.actions.retain(|action| {
                    !matches!(
                        action.kind,
                        crate::reliability::RecoveryActionKind::RetryTask
                    )
                });
                if envelope.actions.is_empty() {
                    envelope.actions.push(crate::reliability::RecoveryAction {
                        kind: crate::reliability::RecoveryActionKind::OpenSession,
                        label: "打开原会话".into(),
                        recommended: Some(true),
                    });
                }
            }
            if let Value::Object(object) = &mut payload {
                object.insert("outputStatus".into(), Value::String("failed".into()));
                object.insert("failure".into(), serde_json::to_value(&envelope)?);
            }
            transaction.execute(
                "UPDATE runtime_tasks SET status='failed', delivery_status='waiting_user',
                        result_json=NULL, error=?2, payload_json=?3, updated_at=?4 WHERE id=?1",
                params![task_id, summary, payload.to_string(), now],
            )?;
            transaction.execute(
                "UPDATE runtime_batches SET status='completed', updated_at=?2
                 WHERE id=(SELECT batch_id FROM runtime_tasks WHERE id=?1)",
                params![task_id, now],
            )?;
            insert_event_tx(
                &transaction,
                &format!("{task_id}:background-failed"),
                task_id,
                None,
                "task_failed",
                serde_json::json!({ "error": summary, "failure": envelope }),
            )?;
            create_attention_with_failure_tx(
                &transaction,
                task_id,
                "background_failure",
                &format!("runtime-task:{task_id}:background-failed:{attempt}"),
                "后台工具执行失败",
                serde_json::json!({ "error": summary, "toolName": tool_name }),
                Some(&envelope),
            )?;
            transaction.commit()?;
            self.emit_persisted_event(&format!("{task_id}:background-failed"))?;
        }
        self.task(task_id)?
            .context("background RuntimeTask not found")
    }

    pub fn interrupt_background_tool(
        &self,
        task_id: &str,
        reason: &str,
    ) -> Result<RuntimeTaskRecord> {
        let mut connection = self.connection()?;
        let (raw_payload, current_status, attempt): (String, String, u32) = connection
            .query_row(
                "SELECT payload_json, status, attempt FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("background RuntimeTask not found")?;
        if matches!(
            current_status.as_str(),
            "succeeded" | "failed" | "cancelled" | "recovery_required"
        ) {
            return self
                .task(task_id)?
                .context("background RuntimeTask not found");
        }
        let mut payload =
            serde_json::from_str::<Value>(&raw_payload).unwrap_or(serde_json::json!({}));
        let envelope =
            failure_from_status_and_error("recovery_required", Some(reason), attempt.max(1));
        if let Value::Object(object) = &mut payload {
            object.insert("failure".into(), serde_json::to_value(&envelope)?);
        }
        let now = Utc::now().to_rfc3339();
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET status='recovery_required', delivery_status='waiting_user',
                    error=?2, payload_json=?3, updated_at=?4 WHERE id=?1",
            params![task_id, reason, payload.to_string(), now],
        )?;
        insert_event_tx(
            &transaction,
            &format!("{task_id}:background-interrupted"),
            task_id,
            None,
            "task_recovery_required",
            serde_json::json!({ "reason": reason, "failure": envelope }),
        )?;
        create_attention_with_failure_tx(
            &transaction,
            task_id,
            "background_interrupted",
            &format!("runtime-task:{task_id}:background-interrupted"),
            "后台工具需要恢复",
            serde_json::json!({ "error": reason }),
            Some(&envelope),
        )?;
        transaction.commit()?;
        self.emit_persisted_event(&format!("{task_id}:background-interrupted"))?;
        self.task(task_id)?
            .context("background RuntimeTask not found")
    }

    pub fn cancel_background_tool(&self, task_id: &str, reason: &str) -> Result<RuntimeTaskRecord> {
        let mut connection = self.connection()?;
        let (raw_payload, current_status): (String, String) = connection
            .query_row(
                "SELECT payload_json, status FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("background RuntimeTask not found")?;
        if matches!(
            current_status.as_str(),
            "succeeded" | "failed" | "cancelled"
        ) {
            return self
                .task(task_id)?
                .context("background RuntimeTask not found");
        }
        let mut payload =
            serde_json::from_str::<Value>(&raw_payload).unwrap_or(serde_json::json!({}));
        if let Value::Object(object) = &mut payload {
            object.remove("failure");
            object.insert("outputStatus".into(), Value::String("failed".into()));
        }
        let now = Utc::now().to_rfc3339();
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET status='cancelled', delivery_status='delivered',
                    error=?2, payload_json=?3, updated_at=?4 WHERE id=?1",
            params![task_id, reason, payload.to_string(), now],
        )?;
        insert_event_tx(
            &transaction,
            &format!("{task_id}:background-cancelled"),
            task_id,
            None,
            "task_cancelled",
            serde_json::json!({ "reason": reason }),
        )?;
        attention::resolve_task_tx(
            &transaction,
            task_id,
            Some(&[
                "background_failure",
                "background_interrupted",
                "task_failed",
            ]),
        )?;
        transaction.commit()?;
        self.emit_persisted_event(&format!("{task_id}:background-cancelled"))?;
        self.task(task_id)?
            .context("background RuntimeTask not found")
    }

    pub fn recover_running_background_tools(&self) -> Result<usize> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id FROM runtime_tasks
             WHERE kind='background_tool' AND status IN ('running','queued','leased','starting')",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let mut recovered = 0usize;
        for task_id in ids {
            self.interrupt_background_tool(
                &task_id,
                "daemon restarted before background tool completion was recorded",
            )?;
            recovered += 1;
        }
        Ok(recovered)
    }

    pub fn create_attention(
        &self,
        task_id: &str,
        kind: &str,
        dedupe_key: &str,
        title: &str,
        detail: Value,
    ) -> Result<Option<RuntimeAttention>> {
        let mut connection = self.connection()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let detail = enrich_attention_detail(kind, title, detail);
        let inserted =
            attention::create_attention_tx(&transaction, task_id, kind, dedupe_key, title, detail)?;
        let event_id = inserted.as_ref().map(|id| format!("{id}:created"));
        if let (Some(id), Some(event_id)) = (inserted.as_deref(), event_id.as_deref()) {
            insert_event_tx(
                &transaction,
                event_id,
                task_id,
                None,
                "attention_created",
                serde_json::json!({ "attentionId": id, "kind": kind }),
            )?;
        }
        transaction.commit()?;
        if let Some(event_id) = event_id {
            self.emit_persisted_event(&event_id)?;
            return self.attention(inserted.as_deref().unwrap_or_default());
        }
        Ok(None)
    }

    pub fn update_attention_status(
        &self,
        id: &str,
        status: &str,
        expected_revision: Option<u32>,
    ) -> Result<RuntimeAttention> {
        let mut connection = self.connection()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = attention::update_status_tx(&transaction, id, status, expected_revision)?;
        let event_id = changed
            .as_ref()
            .map(|(_, revision)| format!("{id}:status:{revision}"));
        if let (Some((task_id, revision)), Some(event_id)) = (changed.as_ref(), event_id.as_deref())
        {
            insert_event_tx(
                &transaction,
                event_id,
                task_id,
                None,
                "attention_updated",
                serde_json::json!({
                    "attentionId": id,
                    "status": status,
                    "revision": revision,
                }),
            )?;
        }
        transaction.commit()?;
        if let Some(event_id) = event_id {
            self.emit_persisted_event(&event_id)?;
        }
        self.attention(id)?.context("attention not found")
    }

    /// Remove an item from the Workbench inbox without deleting its task or
    /// artifacts. The resolved row is intentionally retained as a tombstone:
    /// callback replay and legacy backfill must not recreate dismissed work.
    pub fn dismiss_attention(&self, id: &str) -> Result<RuntimeAttention> {
        self.update_attention_status(id, "resolved", None)
    }

    pub fn resolve_attention_for_task(
        &self,
        task_id: &str,
        kinds: Option<&[&str]>,
    ) -> Result<usize> {
        let mut connection = self.connection()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = attention::resolve_task_tx(&transaction, task_id, kinds)?;
        let event_id = (changed > 0).then(|| {
            format!(
                "{task_id}:attention-resolved:{}",
                uuid::Uuid::now_v7().simple()
            )
        });
        if let Some(event_id) = event_id.as_deref() {
            insert_event_tx(
                &transaction,
                event_id,
                task_id,
                None,
                "attention_updated",
                serde_json::json!({ "status": "resolved", "count": changed }),
            )?;
        }
        transaction.commit()?;
        if let Some(event_id) = event_id {
            self.emit_persisted_event(&event_id)?;
        }
        Ok(changed)
    }

    pub fn active_count(&self) -> Result<usize> {
        let count = self.connection()?.query_row(
            "SELECT COUNT(*) FROM runtime_tasks
             WHERE status IN ('queued','leased','starting','running','waiting_permission')",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count.max(0) as usize)
    }

    pub fn task(&self, id: &str) -> Result<Option<RuntimeTaskRecord>> {
        self.connection()?
            .query_row(
                "SELECT id, batch_id, root_session_id, parent_task_id, kind, objective, cwd,
                        backend, model, status, delivery_status, attempt, created_at, updated_at,
                        result_json, error, payload_json FROM runtime_tasks WHERE id=?1",
                [id],
                row_to_task,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn task_for_work_item(&self, work_item_id: &str) -> Result<Option<RuntimeTaskRecord>> {
        self.connection()?
            .query_row(
                "SELECT task.id, task.batch_id, task.root_session_id, task.parent_task_id,
                        task.kind, task.objective, task.cwd, task.backend, task.model, task.status,
                        task.delivery_status, task.attempt, task.created_at, task.updated_at,
                        task.result_json, task.error, task.payload_json
                 FROM runtime_tasks task
                 JOIN runtime_batches batch ON batch.id=task.batch_id
                 WHERE batch.work_item_id=?1
                 ORDER BY task.created_at DESC, task.id DESC LIMIT 1",
                [work_item_id],
                row_to_task,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn submit_batch_in_workspace(
        &self,
        request: SubmitTaskBatch,
        bound_workspace: &Path,
    ) -> Result<TaskBatchAccepted> {
        self.submit_batch_in_workspace_with_model(request, bound_workspace, None)
    }

    pub fn submit_batch_in_workspace_with_model(
        &self,
        request: SubmitTaskBatch,
        bound_workspace: &Path,
        publisher_model_context: Option<DelegatedModelSnapshot>,
    ) -> Result<TaskBatchAccepted> {
        let workspace_root = self.current_workspace_root()?;
        let bound_workspace = bound_workspace.canonicalize().with_context(|| {
            format!(
                "bound session workspace is not accessible: {}",
                bound_workspace.display()
            )
        })?;
        if !bound_workspace.is_dir() || !bound_workspace.starts_with(&workspace_root) {
            anyhow::bail!("bound session workspace is outside tools.fsBase");
        }
        for task in &request.tasks {
            let task_workspace = PathBuf::from(&task.cwd)
                .canonicalize()
                .with_context(|| format!("task workspace is not accessible: {}", task.cwd))?;
            if !task_workspace.is_dir() || !task_workspace.starts_with(&bound_workspace) {
                anyhow::bail!("task workspace must stay inside the root session workspace");
            }
        }
        self.submit_root_batch(request, publisher_model_context)
    }

    #[cfg(test)]
    fn submit_batch(&self, request: SubmitTaskBatch) -> Result<TaskBatchAccepted> {
        self.submit_root_batch(request, None)
    }

    fn submit_root_batch(
        &self,
        request: SubmitTaskBatch,
        publisher_model_context: Option<DelegatedModelSnapshot>,
    ) -> Result<TaskBatchAccepted> {
        let mut request = request;
        // External callers may choose what to execute, but lineage and resolved
        // execution snapshots are daemon-owned and cannot be forged to bypass
        // recursion/budget limits or misrepresent the selected executor.
        for task in &mut request.tasks {
            task.parent_task_id = None;
            task.depth = 0;
            task.resume_session_id = None;
            task.resolved_executor_kind = None;
            task.resolved_executor_id = None;
            task.resolved_provider_id = None;
            task.resolved_model = None;
            task.resolved_effort = None;
            task.resolved_thinking = false;
            task.resolved_permission_mode = None;
            task.routing_reason = None;
            task.display_name = None;
            task.role_label = None;
            task.avatar_seed = None;
            task.worktree_lease = None;
            task.continuation_refs.clear();
            task.publisher_model_context = publisher_model_context.clone();
        }
        request.parent_task_id = None;
        self.submit_batch_internal(request)
    }

    fn submit_batch_internal(&self, request: SubmitTaskBatch) -> Result<TaskBatchAccepted> {
        if request.root_session_id.trim().is_empty() || request.idempotency_key.trim().is_empty() {
            anyhow::bail!("rootSessionId and idempotencyKey are required");
        }
        if request.tasks.is_empty() || request.tasks.len() > MAX_BATCH_TASKS {
            anyhow::bail!("a batch must contain 1..={MAX_BATCH_TASKS} tasks");
        }
        if request.join != "all" && request.join != "each" {
            anyhow::bail!("join must be all or each");
        }
        let workspace_root = self.current_workspace_root()?;
        let mut normalized = Vec::with_capacity(request.tasks.len());
        for mut task in request.tasks {
            if task.objective.trim().is_empty() || task.depth > MAX_DEPTH {
                anyhow::bail!("task objective is empty or delegation depth exceeds {MAX_DEPTH}");
            }
            normalize_legacy_spec(&mut task);
            let cwd = PathBuf::from(&task.cwd).canonicalize()?;
            if !cwd.is_dir() || !cwd.starts_with(&workspace_root) {
                anyhow::bail!("task workspace is outside tools.fsBase");
            }
            task.cwd = cwd.to_string_lossy().into_owned();
            normalized.push(task);
        }

        let mut connection = self.connection()?;
        if let Some((existing, existing_root, existing_parent, existing_join)) = connection
            .query_row(
                "SELECT id, root_session_id, parent_task_id, join_mode
                 FROM runtime_batches WHERE idempotency_key=?1",
                [&request.idempotency_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
        {
            if existing_root != request.root_session_id
                || existing_parent != request.parent_task_id
                || existing_join != request.join
            {
                anyhow::bail!("idempotencyKey was already used for a different task batch");
            }
            let ids = task_ids_for_batch(&connection, &existing)?;
            return Ok(TaskBatchAccepted {
                batch_id: existing,
                task_ids: ids,
                status: "accepted".into(),
            });
        }
        let transaction = connection.transaction()?;
        let now = Utc::now().to_rfc3339();
        let batch_id = format!("batch_{}", uuid::Uuid::now_v7().simple());
        transaction.execute(
            "INSERT INTO runtime_batches
             (id, root_session_id, parent_task_id, work_item_id, session_generation, join_mode,
              status, idempotency_key, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, ?8)",
            params![
                batch_id,
                request.root_session_id,
                request.parent_task_id,
                request.work_item_id,
                request.session_generation,
                request.join,
                request.idempotency_key,
                now
            ],
        )?;
        let mut task_ids = Vec::with_capacity(normalized.len());
        let mut used_names = Vec::<String>::new();
        for mut spec in normalized {
            let task_id = format!("task_{}", uuid::Uuid::now_v7().simple());
            assign_stable_identity(&mut spec, &request.root_session_id, &task_id, &used_names);
            if let Some(name) = spec.display_name.clone() {
                used_names.push(name);
            }
            let waiting_reason = resolve_execution_spec(&mut spec);
            let status = if waiting_reason.is_some() {
                "waiting_configuration"
            } else {
                "queued"
            };
            let mut payload = serde_json::to_value(&spec)?;
            if let Value::Object(object) = &mut payload {
                object.insert("outputStatus".into(), Value::String("pending".into()));
                object.insert(
                    "reviewStatus".into(),
                    Value::String(
                        if spec.access == DelegatedAccess::Mutating {
                            "pending"
                        } else {
                            "not_required"
                        }
                        .into(),
                    ),
                );
                if let Some(work_item_id) = request.work_item_id.as_ref() {
                    object.insert("workItemId".into(), Value::String(work_item_id.clone()));
                }
                if let Some(generation) = request.session_generation {
                    object.insert("sessionGeneration".into(), Value::Number(generation.into()));
                }
            }
            transaction.execute(
                "INSERT INTO runtime_tasks
                 (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd, backend, model,
                  payload_json, status, delivery_status, attempt, error, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'not_ready', 0, ?12, ?13, ?13)",
                params![
                    task_id,
                    batch_id,
                    request.root_session_id,
                    spec.parent_task_id,
                    spec.kind,
                    spec.objective,
                    spec.cwd,
                    spec.backend,
                    spec.resolved_model,
                    payload.to_string(),
                    status,
                    waiting_reason,
                    now,
                ],
            )?;
            insert_event_tx(
                &transaction,
                &format!("{task_id}:published"),
                &task_id,
                None,
                "task_published",
                serde_json::json!({
                    "batchId": batch_id,
                    "status": status,
                    "executor": spec.resolved_executor_id,
                    "routingReason": spec.routing_reason,
                }),
            )?;
            task_ids.push(task_id);
        }
        transaction.commit()?;
        for task_id in &task_ids {
            self.emit_persisted_event(&format!("{task_id}:published"))?;
        }
        if self.launch_workers {
            for task_id in &task_ids {
                if let Err(error) = self.try_start(task_id) {
                    tracing::warn!(%error, %task_id, "failed to start newly published RuntimeTask");
                }
            }
        }
        Ok(TaskBatchAccepted {
            batch_id,
            task_ids,
            status: "accepted".into(),
        })
    }

    fn try_start(&self, task_id: &str) -> Result<bool> {
        let mut connection = self.connection()?;
        let task = connection
            .query_row(
                "SELECT payload_json, cwd, status FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .context("runtime task not found")?;
        if task.2 != "queued" {
            return Ok(false);
        }
        if let Ok(payload) = serde_json::from_str::<Value>(&task.0) {
            if payload.get("mode").and_then(Value::as_str) == Some("background_tool")
                || payload.get("kind").and_then(Value::as_str) == Some("background_tool")
            {
                return Ok(false);
            }
        }
        let mut spec: RuntimeTaskSpec = serde_json::from_str(&task.0)?;
        let active: i64 = connection.query_row(
            "SELECT COUNT(*) FROM runtime_tasks WHERE status IN ('leased','starting','running','waiting_permission')",
            [],
            |row| row.get(0),
        )?;
        if active >= MAX_ACTIVE_WORKERS {
            return Ok(false);
        }
        if spec.mode.as_deref().unwrap_or("edit") == "edit" {
            let writer: i64 = connection.query_row(
                "SELECT COUNT(*) FROM runtime_tasks
                 WHERE cwd=?1 AND id<>?2 AND status IN ('leased','starting','running','waiting_permission')
                   AND json_extract(payload_json, '$.mode') IS NOT 'research'",
                params![task.1, task_id],
                |row| row.get(0),
            )?;
            if writer > 0 {
                return Ok(false);
            }
        }

        let review_lease = if spec.access == DelegatedAccess::Mutating {
            if let Some(existing) = spec.worktree_lease.clone() {
                Some(existing)
            } else {
                match worktree::prepare_mutating_task(
                    self.data_dir.as_ref(),
                    task_id,
                    Path::new(&spec.cwd),
                ) {
                    Ok(lease) => {
                        spec.worktree_lease = Some(lease.clone());
                        let payload = serialize_spec_preserving_runtime(&spec, &task.0)?;
                        connection.execute(
                            "UPDATE runtime_tasks SET payload_json=?2, updated_at=?3 WHERE id=?1",
                            params![task_id, payload, Utc::now().to_rfc3339()],
                        )?;
                        Some(lease)
                    }
                    Err(error) => {
                        let message =
                            format!("mutating delegation requires Git isolation: {error}");
                        connection.execute(
                            "UPDATE runtime_tasks SET status='waiting_configuration', error=?2, updated_at=?3
                             WHERE id=?1 AND status='queued'",
                            params![task_id, message, Utc::now().to_rfc3339()],
                        )?;
                        let event_id = format!("{task_id}:waiting-configuration");
                        connection.execute(
                            "INSERT OR IGNORE INTO task_events
                             (event_id, task_id, kind, payload_json, created_at)
                             VALUES (?1, ?2, 'task_waiting_configuration', ?3, ?4)",
                            params![
                                event_id,
                                task_id,
                                serde_json::json!({ "reason": message }).to_string(),
                                Utc::now().to_rfc3339()
                            ],
                        )?;
                        self.emit_persisted_event(&event_id)?;
                        return Ok(false);
                    }
                }
            }
        } else {
            None
        };

        let attempt_no: u32 = connection.query_row(
            "SELECT attempt + 1 FROM runtime_tasks WHERE id=?1",
            [task_id],
            |row| row.get(0),
        )?;
        let lease_epoch = u64::from(attempt_no);
        let attempt_id = format!("attempt_{}", uuid::Uuid::now_v7().simple());
        let callback_token = random_token();
        let token_hash = sha256(&callback_token);
        let continuation_ref_json = serde_json::to_string(&spec.continuation_refs)?;
        let now = Utc::now();
        let transaction = connection.transaction()?;
        let claimed = transaction.execute(
            "UPDATE runtime_tasks SET status='leased', attempt=?2, continuation_ref=?3,
                    updated_at=?4 WHERE id=?1 AND status='queued'",
            params![task_id, attempt_no, continuation_ref_json, now.to_rfc3339()],
        )?;
        if claimed == 0 {
            return Ok(false);
        }
        transaction.execute(
            "INSERT INTO task_attempts
             (id, task_id, attempt_no, lease_epoch, token_hash, status, lease_expires_at,
              continuation_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, 'leased', ?6, ?7)",
            params![
                attempt_id,
                task_id,
                attempt_no,
                lease_epoch,
                token_hash,
                (now + chrono::Duration::minutes(5)).to_rfc3339(),
                continuation_ref_json
            ],
        )?;
        insert_event_tx(
            &transaction,
            &format!("{attempt_id}:leased"),
            task_id,
            Some(&attempt_id),
            "task_leased",
            serde_json::json!({ "attempt": attempt_no, "leaseEpoch": lease_epoch }),
        )?;
        transaction.commit()?;
        self.lease_notify.notify_one();
        self.emit_persisted_event(&format!("{attempt_id}:leased"))?;

        let cwd = review_lease
            .as_ref()
            .map(|lease| lease.worker_cwd.to_string_lossy().into_owned())
            .unwrap_or_else(|| spec.cwd.clone());
        let deliverable_title = spec
            .deliverable_title
            .clone()
            .unwrap_or_else(|| spec.objective.chars().take(80).collect());
        let continuation_refs = spec.continuation_refs.clone();
        let execution = if spec.resolved_executor_kind.as_deref() == Some("internal_agent") {
            ResolvedExecution::PwcliAgent {
                objective: spec.objective,
                role: spec.role,
                access: spec.access,
                provider_id: spec.resolved_provider_id,
                model: spec.resolved_model,
                effort: spec.resolved_effort,
                thinking: spec.resolved_thinking,
            }
        } else {
            ResolvedExecution::AcpCli {
                args: CodeAgentArgs {
                    task: spec.objective,
                    cwd: cwd.clone(),
                    backend: spec.resolved_executor_id.or(spec.backend),
                    mode: spec.mode,
                    effort: spec.resolved_effort.or(spec.effort),
                    timeout_secs: Some(1800),
                    resume_session_id: spec.resume_session_id,
                    model: spec.resolved_model.or(spec.model),
                    context_window: None,
                    permission_mode: spec.resolved_permission_mode.or(spec.permission_mode),
                    spec: None,
                    project_rules: None,
                },
            }
        };
        let descriptor = WorkerDescriptor {
            task_id: task_id.to_string(),
            attempt_id,
            lease_epoch,
            callback_token,
            callback_base: self.callback_base.as_ref().clone(),
            data_dir: self.data_dir.as_ref().clone(),
            cwd,
            deliverable_title,
            execution,
            continuation_refs,
            review_lease,
        };
        match self.spawn_worker(&descriptor) {
            Ok(()) => Ok(true),
            Err(failure) => {
                self.record_worker_start_failure(&descriptor, &failure)?;
                Ok(false)
            }
        }
    }

    fn spawn_worker(
        &self,
        descriptor: &WorkerDescriptor,
    ) -> std::result::Result<(), WorkerStartError> {
        let descriptor_dir = self.data_dir.join("task-descriptors");
        let log_dir = self.data_dir.join("task-logs");
        fs::create_dir_all(&descriptor_dir).map_err(WorkerStartError::before_launch)?;
        fs::create_dir_all(&log_dir).map_err(WorkerStartError::before_launch)?;
        let descriptor_path = descriptor_dir.join(format!("{}.json", descriptor.attempt_id));
        write_private_json(&descriptor_path, descriptor)
            .map_err(WorkerStartError::before_launch)?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("{}.log", descriptor.task_id)))
            .map_err(WorkerStartError::before_launch)?;
        let stderr = log.try_clone().map_err(WorkerStartError::before_launch)?;
        let executable = std::env::current_exe().map_err(WorkerStartError::before_launch)?;
        let descriptor_sha256 =
            sha256(serde_json::to_vec(descriptor).map_err(WorkerStartError::before_launch)?);
        let mut command = Command::new(&executable);
        command
            .arg("task-worker")
            .arg("--descriptor")
            .arg(&descriptor_path)
            .current_dir(&descriptor.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .context("spawn independent task worker")
            .map_err(WorkerStartError::before_launch)?;
        let pid = child.id();
        let process_fingerprint = match capture_process_fingerprint(
            pid,
            &executable,
            &descriptor_path,
            descriptor_sha256,
        ) {
            Ok(fingerprint) => match serde_json::to_string(&fingerprint) {
                Ok(serialized) => serialized,
                Err(error) => {
                    let _ = child.kill();
                    return Err(WorkerStartError::after_launch(error));
                }
            },
            Err(error) => {
                let _ = child.kill();
                return Err(WorkerStartError::after_launch(error));
            }
        };
        let now = Utc::now().to_rfc3339();
        let persist_started = (|| -> Result<()> {
            let mut connection = self.connection()?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE task_attempts SET status='running', worker_pid=?2, worker_pgid=?2,
                        process_fingerprint=?3, started_at=?4, lease_expires_at=?5 WHERE id=?1",
                params![
                    descriptor.attempt_id,
                    pid,
                    process_fingerprint,
                    now,
                    (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()
                ],
            )?;
            transaction.execute(
                "UPDATE runtime_tasks SET status='running', updated_at=?2 WHERE id=?1",
                params![descriptor.task_id, now],
            )?;
            insert_event_tx(
                &transaction,
                &format!("{}:started", descriptor.attempt_id),
                &descriptor.task_id,
                Some(&descriptor.attempt_id),
                "task_started",
                serde_json::json!({}),
            )?;
            transaction.commit()?;
            Ok(())
        })();
        if let Err(error) = persist_started {
            let _ = child.kill();
            return Err(WorkerStartError::after_launch(error));
        }
        if let Err(error) = self.emit_persisted_event(&format!("{}:started", descriptor.attempt_id))
        {
            tracing::warn!(%error, attempt_id = %descriptor.attempt_id, "failed to broadcast persisted task_started event");
        }
        self.lease_notify.notify_one();
        Ok(())
    }

    fn record_worker_start_failure(
        &self,
        descriptor: &WorkerDescriptor,
        failure: &WorkerStartError,
    ) -> Result<()> {
        let (task_status, attempt_status, event_kind, event_suffix) =
            if failure.side_effects_possible {
                (
                    "recovery_required",
                    "orphaned",
                    "task_recovery_required",
                    "start-recovery-required",
                )
            } else {
                (
                    "waiting_configuration",
                    "start_failed",
                    "task_start_failed",
                    "start-failed",
                )
            };
        let message = failure.error.to_string();
        let now = Utc::now().to_rfc3339();
        let event_id = format!("{}:{event_suffix}", descriptor.attempt_id);
        let mut connection = self.connection()?;
        let raw_payload: String = connection.query_row(
            "SELECT payload_json FROM runtime_tasks WHERE id=?1",
            [&descriptor.task_id],
            |row| row.get(0),
        )?;
        let attempt: u32 = connection
            .query_row(
                "SELECT attempt FROM runtime_tasks WHERE id=?1",
                [&descriptor.task_id],
                |row| row.get(0),
            )
            .unwrap_or(1);
        let envelope = failure_from_status_and_error(task_status, Some(&message), attempt.max(1));
        let payload = payload_with_failure(&raw_payload, Some(&envelope))?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE task_attempts SET status=?2, error=?3, finished_at=?4 WHERE id=?1",
            params![descriptor.attempt_id, attempt_status, &message, &now],
        )?;
        transaction.execute(
            "UPDATE runtime_tasks SET status=?2, error=?3, payload_json=?4, updated_at=?5 WHERE id=?1",
            params![descriptor.task_id, task_status, &message, payload, &now],
        )?;
        insert_event_tx(
            &transaction,
            &event_id,
            &descriptor.task_id,
            Some(&descriptor.attempt_id),
            event_kind,
            serde_json::json!({
                "reason": message,
                "sideEffectsPossible": failure.side_effects_possible,
                "failure": envelope,
            }),
        )?;
        create_attention_with_failure_tx(
            &transaction,
            &descriptor.task_id,
            "task_failed",
            &format!(
                "runtime-task:{}:attempt:{}:{event_suffix}",
                descriptor.task_id, descriptor.attempt_id
            ),
            if failure.side_effects_possible {
                "协作者需要恢复"
            } else {
                "协作者启动失败"
            },
            serde_json::json!({
                "attemptId": descriptor.attempt_id,
                "error": message,
                "recoveryRequired": failure.side_effects_possible,
            }),
            Some(&envelope),
        )?;
        transaction.commit()?;
        let _ = fs::remove_file(
            self.data_dir
                .join("task-descriptors")
                .join(format!("{}.json", descriptor.attempt_id)),
        );
        self.emit_persisted_event(&event_id)
    }

    pub fn record_attempt_event(
        &self,
        attempt_id: &str,
        request: AttemptEventRequest,
    ) -> Result<()> {
        self.verify_attempt(attempt_id, &request.callback_token, request.lease_epoch)?;
        if request.event_id.trim().is_empty() || request.event_id.len() > 512 {
            anyhow::bail!("attempt eventId is invalid");
        }
        if request.kind != "native_session_started" {
            anyhow::bail!("unsupported RuntimeTask attempt event kind");
        }
        let native_session_id = request
            .payload
            .get("nativeSessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 4_096)
            .context("native_session_started requires nativeSessionId")?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let task_id = transaction.query_row(
            "SELECT task_id FROM task_attempts WHERE id=?1",
            [attempt_id],
            |row| row.get::<_, String>(0),
        )?;
        let event_payload = serde_json::json!({ "nativeSessionId": native_session_id });
        if let Some((existing_task_id, existing_attempt_id, existing_kind, existing_payload)) =
            transaction
                .query_row(
                    "SELECT task_id, attempt_id, kind, payload_json
                     FROM task_events WHERE event_id=?1",
                    [&request.event_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?
        {
            let same_event = existing_task_id == task_id
                && existing_attempt_id.as_deref() == Some(attempt_id)
                && existing_kind == "task_native_session_started"
                && serde_json::from_str::<Value>(&existing_payload)
                    .ok()
                    .as_ref()
                    == Some(&event_payload);
            if !same_event {
                anyhow::bail!("attempt eventId was already used for a different event");
            }
            transaction.commit()?;
            return Ok(());
        }
        transaction.execute(
            "UPDATE task_attempts SET native_session_id=?2 WHERE id=?1",
            params![attempt_id, native_session_id],
        )?;
        insert_event_tx(
            &transaction,
            &request.event_id,
            &task_id,
            Some(attempt_id),
            "task_native_session_started",
            event_payload,
        )?;
        transaction.commit()?;
        self.emit_persisted_event(&request.event_id)
    }

    pub fn heartbeat(&self, attempt_id: &str, token: &str, lease_epoch: u64) -> Result<()> {
        self.verify_attempt(attempt_id, token, lease_epoch)?;
        let changed = self.connection()?.execute(
            "UPDATE task_attempts SET lease_expires_at=?2 WHERE id=?1 AND status='running'",
            params![
                attempt_id,
                (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()
            ],
        )?;
        if changed == 0 {
            anyhow::bail!("task attempt is not running");
        }
        self.lease_notify.notify_one();
        Ok(())
    }

    pub fn callback_capability_valid(
        &self,
        attempt_id: &str,
        token: &str,
        lease_epoch: u64,
    ) -> Result<bool> {
        let expected = self
            .connection()?
            .query_row(
                "SELECT token_hash, lease_epoch FROM task_attempts WHERE id=?1",
                [attempt_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .optional()?;
        Ok(expected
            .is_some_and(|expected| expected.0 == sha256(token) && expected.1 == lease_epoch))
    }

    pub fn record_stale_callback(&self, attempt_id: &str, event_id: &str) -> Result<()> {
        let connection = self.connection()?;
        let task_id = connection
            .query_row(
                "SELECT task_id FROM task_attempts WHERE id=?1",
                [attempt_id],
                |row| row.get::<_, String>(0),
            )
            .context("task attempt not found")?;
        let audit_event_id = format!("{event_id}:stale-discarded");
        connection.execute(
            "INSERT OR IGNORE INTO task_events
             (event_id, task_id, attempt_id, kind, payload_json, created_at)
             VALUES (?1, ?2, ?3, 'stale_callback_discarded', '{}', ?4)",
            params![audit_event_id, task_id, attempt_id, Utc::now().to_rfc3339()],
        )?;
        self.emit_persisted_event(&audit_event_id)
    }

    pub fn submit_child_batch(
        &self,
        attempt_id: &str,
        request: SubmitChildTaskBatch,
    ) -> Result<TaskBatchAccepted> {
        self.verify_attempt(attempt_id, &request.callback_token, request.lease_epoch)?;
        let connection = self.connection()?;
        let (
            parent_task_id,
            root_session_id,
            parent_payload,
            work_item_id,
            session_generation,
            parent_status,
            attempt_status,
        ): (
            String,
            String,
            String,
            Option<String>,
            Option<u64>,
            String,
            String,
        ) = connection
            .query_row(
                "SELECT task.id, task.root_session_id, task.payload_json,
                        batch.work_item_id, batch.session_generation, task.status, attempt.status
                 FROM task_attempts attempt
                 JOIN runtime_tasks task ON task.id=attempt.task_id
                 JOIN runtime_batches batch ON batch.id=task.batch_id
                 WHERE attempt.id=?1 AND task.status IN ('running','waiting_children')",
                [attempt_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .context("parent RuntimeTask is not dispatchable")?;
        let parent: RuntimeTaskSpec = serde_json::from_str(&parent_payload)?;
        if request.idempotency_key.trim().is_empty() {
            anyhow::bail!("idempotencyKey is required");
        }
        if request.join != "all" && request.join != "each" {
            anyhow::bail!("join must be all or each");
        }
        if request.tasks.is_empty() || request.tasks.len() > MAX_BATCH_TASKS {
            anyhow::bail!("a batch must contain 1..={MAX_BATCH_TASKS} tasks");
        }
        if parent.depth >= MAX_DEPTH {
            anyhow::bail!("delegation depth exceeds {MAX_DEPTH}");
        }
        let descendant_count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM runtime_tasks
             WHERE root_session_id=?1 AND parent_task_id IS NOT NULL",
            [&root_session_id],
            |row| row.get(0),
        )?;
        if descendant_count + request.tasks.len() as i64 > MAX_DESCENDANT_TASKS {
            anyhow::bail!(
                "delegated task total exceeds {MAX_DESCENDANT_TASKS} for this root session"
            );
        }
        let parent_cwd = PathBuf::from(&parent.cwd).canonicalize()?;
        let inherited_model_context = Some(DelegatedModelSnapshot {
            provider_id: parent.resolved_provider_id.clone(),
            model: parent.resolved_model.clone().unwrap_or_default(),
            effort: parent.resolved_effort.clone(),
            thinking: parent.resolved_thinking,
        });
        let mut tasks = request.tasks;
        for task in &mut tasks {
            if task.objective.trim().is_empty() {
                anyhow::bail!("child task objective is required");
            }
            // A child cannot choose a new filesystem authority. Its workspace
            // is derived from the authenticated parent attempt by the daemon.
            task.cwd = parent_cwd.to_string_lossy().into_owned();
            task.parent_task_id = Some(parent_task_id.clone());
            task.depth = parent.depth.saturating_add(1);
            task.resume_session_id = None;
            task.resolved_executor_kind = None;
            task.resolved_executor_id = None;
            task.resolved_provider_id = None;
            task.resolved_model = None;
            task.resolved_effort = None;
            task.resolved_thinking = false;
            task.resolved_permission_mode = None;
            task.routing_reason = None;
            task.display_name = None;
            task.role_label = None;
            task.avatar_seed = None;
            task.worktree_lease = None;
            task.continuation_refs.clear();
            task.publisher_model_context = inherited_model_context.clone();
        }
        let now = Utc::now().to_rfc3339();
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET status='waiting_children', updated_at=?2
             WHERE id=?1 AND status IN ('running','waiting_children')",
            params![parent_task_id, now],
        )?;
        transaction.execute(
            "UPDATE task_attempts SET status='waiting_children'
             WHERE id=?1 AND status IN ('running','waiting_children')",
            [attempt_id],
        )?;
        transaction.commit()?;
        drop(connection);
        let submitted = self.submit_batch_internal(SubmitTaskBatch {
            root_session_id,
            parent_task_id: Some(parent_task_id.clone()),
            work_item_id,
            session_generation,
            idempotency_key: request.idempotency_key,
            join: request.join,
            tasks,
        });
        if submitted.is_err() {
            let mut connection = self.connection()?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE runtime_tasks SET status=?2, updated_at=?3
                 WHERE id=?1 AND status='waiting_children'",
                params![parent_task_id, parent_status, Utc::now().to_rfc3339()],
            )?;
            transaction.execute(
                "UPDATE task_attempts SET status=?2
                 WHERE id=?1 AND status='waiting_children'",
                params![attempt_id, attempt_status],
            )?;
            transaction.commit()?;
        }
        submitted
    }

    pub fn complete(
        &self,
        attempt_id: &str,
        request: CompleteAttemptRequest,
    ) -> Result<CompletionProjection> {
        self.verify_attempt(attempt_id, &request.callback_token, request.lease_epoch)?;
        let mut connection = self.connection()?;
        let identity = connection.query_row(
            "SELECT a.task_id, t.batch_id, t.root_session_id, t.status, t.payload_json
             FROM task_attempts a JOIN runtime_tasks t ON t.id=a.task_id WHERE a.id=?1",
            [attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        if is_terminal(&identity.3) {
            let task = self
                .task(&identity.0)?
                .context("completed task disappeared")?;
            return Ok(CompletionProjection {
                task,
                batch_completed: false,
                root_session_id: identity.2,
                callback_text: None,
            });
        }
        let status = if request.outcome == "succeeded" {
            "succeeded"
        } else {
            "failed"
        };
        let now = Utc::now().to_rfc3339();
        let result_json = request
            .result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        if request.outcome == "waiting_children" {
            let now = Utc::now().to_rfc3339();
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE task_attempts SET status='waiting_children', result_hash=?2,
                        native_session_id=?3, finished_at=?4 WHERE id=?1",
                params![
                    attempt_id,
                    result_json.as_deref().map(sha256),
                    request.native_session_id,
                    now
                ],
            )?;
            transaction.execute(
                "UPDATE runtime_tasks SET status='waiting_children', result_json=?2,
                        delivery_status='not_ready', updated_at=?3 WHERE id=?1",
                params![identity.0, result_json, now],
            )?;
            insert_event_tx(
                &transaction,
                &request.event_id,
                &identity.0,
                Some(attempt_id),
                "task_waiting_children",
                request.result.clone().unwrap_or(Value::Null),
            )?;
            transaction.commit()?;
            self.emit_persisted_event(&request.event_id)?;
            let task = self
                .task(&identity.0)?
                .context("waiting parent task disappeared")?;
            return Ok(CompletionProjection {
                task,
                batch_completed: false,
                root_session_id: identity.2,
                callback_text: None,
            });
        }
        if request.outcome == "waiting_user" {
            let result = request
                .result
                .as_ref()
                .context("waiting_user callback requires a decision payload")?;
            let question = result
                .get("question")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .context("waiting_user callback requires a question")?;
            let options = result
                .get("options")
                .and_then(Value::as_array)
                .context("waiting_user callback requires options")?;
            if options.is_empty()
                || options.iter().any(|option| {
                    option
                        .as_str()
                        .map(str::trim)
                        .is_none_or(|value| value.is_empty())
                })
            {
                anyhow::bail!("waiting_user callback requires non-empty string options");
            }
            let native_session_id = request
                .native_session_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .context("waiting_user callback requires nativeSessionId")?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE task_attempts SET status='waiting_user', result_hash=?2,
                        native_session_id=?3, error=NULL, finished_at=?4 WHERE id=?1",
                params![
                    attempt_id,
                    result_json.as_deref().map(sha256),
                    native_session_id,
                    now
                ],
            )?;
            transaction.execute(
                "UPDATE runtime_tasks SET status='waiting_user', result_json=?2,
                        delivery_status='waiting_user', error=NULL, updated_at=?3 WHERE id=?1",
                params![identity.0, result_json, now],
            )?;
            insert_event_tx(
                &transaction,
                &request.event_id,
                &identity.0,
                Some(attempt_id),
                "task_waiting_user",
                serde_json::json!({
                    "question": question,
                    "options": options,
                    "nativeSessionId": native_session_id,
                }),
            )?;
            let envelope = failure_from_status_and_error("waiting_user", Some(&question), 1);
            create_attention_with_failure_tx(
                &transaction,
                &identity.0,
                "decision_required",
                &format!("runtime-task:{}:decision:{attempt_id}", identity.0),
                "协作者需要你的决定",
                serde_json::json!({
                    "attemptId": attempt_id,
                    "question": question,
                    "options": options,
                }),
                Some(&envelope),
            )?;
            transaction.commit()?;
            self.emit_persisted_event(&request.event_id)?;
            let task = self
                .task(&identity.0)?
                .context("waiting decision task disappeared")?;
            return Ok(CompletionProjection {
                task,
                batch_completed: false,
                root_session_id: identity.2,
                callback_text: None,
            });
        }
        let stored_payload = serde_json::from_str::<Value>(&identity.4)?;
        let mut pending_continuations = stored_payload
            .get("pendingContinuationRefs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for legacy_callback in stored_payload
            .get("pendingChildResults")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let child_batch = legacy_callback
                .get("batchId")
                .and_then(Value::as_str)
                .context("buffered child callback is missing batchId")?;
            let reference = materialize_child_continuation(
                self.data_dir.as_ref(),
                &connection,
                &identity.0,
                child_batch,
                legacy_callback,
            )?;
            pending_continuations.push(serde_json::json!({
                "reference": reference,
                "summary": legacy_callback.get("summary").cloned().unwrap_or(Value::Null),
            }));
        }
        if !pending_continuations.is_empty() {
            let mut spec = serde_json::from_str::<RuntimeTaskSpec>(&identity.4)?;
            if request.native_session_id.is_some() {
                spec.resume_session_id = request.native_session_id.clone();
            }
            if let Some(parent_summary) = request
                .result
                .as_ref()
                .and_then(|result| result.get("summary"))
                .and_then(Value::as_str)
            {
                spec.objective.push_str(&format!(
                    "\n\n## 父任务上一轮进展\n{}",
                    parent_summary.chars().take(1_000).collect::<String>()
                ));
            }
            for pending in &pending_continuations {
                let reference = serde_json::from_value::<continuation::ContinuationArtifactRef>(
                    pending
                        .get("reference")
                        .cloned()
                        .context("pending continuation is missing reference")?,
                )?;
                merge_continuation_ref(&mut spec.continuation_refs, reference.clone());
                if !spec.objective.contains(&reference.artifact_id) {
                    spec.objective.push_str(&continuation_objective_marker(
                        &reference.child_batch_id,
                        pending
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or("子任务已完成"),
                        &reference,
                    ));
                }
            }
            let mut payload = serde_json::from_str::<Value>(&serialize_spec_preserving_runtime(
                &spec,
                &identity.4,
            )?)?;
            if let Value::Object(object) = &mut payload {
                object.remove("pendingContinuationRefs");
                object.remove("pendingChildResults");
                object.insert("outputStatus".into(), Value::String("pending".into()));
                object.insert(
                    "reviewStatus".into(),
                    Value::String(
                        if spec.access == DelegatedAccess::Mutating {
                            "pending"
                        } else {
                            "not_required"
                        }
                        .into(),
                    ),
                );
            }
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE task_attempts SET status=?2, native_session_id=?3, result_hash=?4,
                        error=?5, finished_at=?6 WHERE id=?1",
                params![
                    attempt_id,
                    status,
                    request.native_session_id,
                    result_json.as_deref().map(sha256),
                    request.error,
                    now,
                ],
            )?;
            transaction.execute(
                "UPDATE runtime_tasks SET status='queued', delivery_status='not_ready',
                        payload_json=?2, objective=?3, continuation_ref=?4,
                        result_json=NULL, error=NULL, updated_at=?5
                 WHERE id=?1",
                params![
                    identity.0,
                    payload.to_string(),
                    spec.objective,
                    serde_json::to_string(&spec.continuation_refs)?,
                    now
                ],
            )?;
            transaction.execute(
                "UPDATE runtime_batches SET status='running', updated_at=?2 WHERE id=?1",
                params![identity.1, now],
            )?;
            insert_event_tx(
                &transaction,
                &request.event_id,
                &identity.0,
                Some(attempt_id),
                "task_child_results_queued",
                serde_json::json!({ "continuationRefs": pending_continuations }),
            )?;
            transaction.commit()?;
            self.emit_persisted_event(&request.event_id)?;
            if self.launch_workers {
                self.try_start(&identity.0)?;
            }
            let task = self
                .task(&identity.0)?
                .context("continued parent task disappeared")?;
            return Ok(CompletionProjection {
                task,
                batch_completed: false,
                root_session_id: identity.2,
                callback_text: None,
            });
        }
        if status == "succeeded"
            && (has_incomplete_child_batches(&connection, &identity.0)?
                || has_unconsumed_child_results(&connection, &identity.0)?)
        {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE task_attempts SET status='succeeded', native_session_id=?2,
                        result_hash=?3, error=NULL, finished_at=?4 WHERE id=?1",
                params![
                    attempt_id,
                    request.native_session_id,
                    result_json.as_deref().map(sha256),
                    now
                ],
            )?;
            transaction.execute(
                "UPDATE runtime_tasks SET status='waiting_children', delivery_status='not_ready',
                        result_json=?2, error=NULL, updated_at=?3 WHERE id=?1",
                params![identity.0, result_json, now],
            )?;
            insert_event_tx(
                &transaction,
                &request.event_id,
                &identity.0,
                Some(attempt_id),
                "task_waiting_children",
                serde_json::json!({
                    "reason": "join_each_children_pending",
                    "result": request.result,
                }),
            )?;
            transaction.commit()?;
            self.emit_persisted_event(&request.event_id)?;
            let task = self
                .task(&identity.0)?
                .context("waiting parent task disappeared")?;
            return Ok(CompletionProjection {
                task,
                batch_completed: false,
                root_session_id: identity.2,
                callback_text: None,
            });
        }
        let spec = serde_json::from_str::<RuntimeTaskSpec>(&identity.4)?;
        let result_value = request.result.clone().unwrap_or(Value::Null);
        let document = result_value.get("document");
        let document_title = document
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str)
            .or(spec.deliverable_title.as_deref())
            .unwrap_or("委派任务产物")
            .to_string();
        let document_body = document
            .and_then(|value| value.get("body"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                request.error.clone().unwrap_or_else(|| {
                    "任务已结束，但执行器没有返回可读正文。请查看执行详情。".to_string()
                })
            });
        let mut task_payload = serde_json::from_str::<Value>(&identity.4)?;
        if let Value::Object(object) = &mut task_payload {
            object.insert(
                "outputStatus".to_string(),
                Value::String("materializing".to_string()),
            );
        }
        let failure = if status == "failed" {
            let attempt: u32 = connection
                .query_row(
                    "SELECT attempt FROM runtime_tasks WHERE id=?1",
                    [&identity.0],
                    |row| row.get(0),
                )
                .unwrap_or(1);
            Some(failure_from_status_and_error(
                "failed",
                request.error.as_deref(),
                attempt.max(1),
            ))
        } else {
            None
        };
        if let Some(failure) = failure.as_ref() {
            if let Value::Object(object) = &mut task_payload {
                object.insert("failure".into(), serde_json::to_value(failure)?);
            }
        }
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE task_attempts SET status=?2, native_session_id=?3, result_hash=?4, error=?5, finished_at=?6
             WHERE id=?1",
            params![attempt_id, status, request.native_session_id, result_json.as_deref().map(sha256), request.error, now],
        )?;
        transaction.execute(
            "UPDATE runtime_tasks SET status=?2, delivery_status='not_ready', result_json=?3,
                    error=?4, payload_json=?5, updated_at=?6 WHERE id=?1",
            params![
                identity.0,
                status,
                result_json,
                request.error,
                task_payload.to_string(),
                now
            ],
        )?;
        insert_event_tx(
            &transaction,
            &request.event_id,
            &identity.0,
            Some(attempt_id),
            if status == "succeeded" {
                "task_completed"
            } else {
                "task_failed"
            },
            serde_json::json!({
                "outcome": request.outcome,
                "result": request.result,
                "error": request.error,
                "failure": failure,
            }),
        )?;
        if status == "failed" {
            create_attention_with_failure_tx(
                &transaction,
                &identity.0,
                "task_failed",
                &format!("runtime-task:{}:attempt:{attempt_id}:failed", identity.0),
                "协作者执行失败",
                serde_json::json!({
                    "attemptId": attempt_id,
                    "error": request.error,
                }),
                failure.as_ref(),
            )?;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO task_outbox
             (id, source_event_id, destination, dedupe_key, status, payload_json, created_at, updated_at)
             VALUES (?1, ?2, 'document', ?3, 'pending', ?4, ?5, ?5)",
            params![
                format!("outbox_{}", uuid::Uuid::now_v7().simple()),
                request.event_id,
                format!("runtime-task-output:{attempt_id}"),
                serde_json::json!({
                    "taskId": identity.0,
                    "attemptId": attempt_id,
                    "batchId": identity.1,
                    "rootSessionId": identity.2,
                    "executorId": spec.resolved_executor_id.as_deref().unwrap_or("legacy"),
                    "agentName": spec.display_name.as_deref().unwrap_or("旧版委派"),
                    "title": document_title,
                    "body": document_body,
                }).to_string(),
                now,
            ],
        )?;
        let remaining: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM runtime_tasks WHERE batch_id=?1 AND status NOT IN ('succeeded','failed','cancelled')",
            [&identity.1],
            |row| row.get(0),
        )?;
        if remaining == 0 {
            transaction.execute(
                "UPDATE runtime_batches SET status='materializing', updated_at=?2 WHERE id=?1",
                params![identity.1, now],
            )?;
        }
        transaction.commit()?;
        self.outbox_notify.notify_one();
        self.emit_persisted_event(&request.event_id)?;
        self.start_ready_tasks()?;
        let task = self
            .task(&identity.0)?
            .context("completed task disappeared")?;
        Ok(CompletionProjection {
            task,
            batch_completed: false,
            root_session_id: identity.2,
            callback_text: None,
        })
    }

    pub fn cancel(&self, task_id: &str) -> Result<RuntimeTaskRecord> {
        let children = {
            let connection = self.connection()?;
            let mut statement = connection.prepare(
                "SELECT id FROM runtime_tasks
                 WHERE parent_task_id=?1 AND status NOT IN ('succeeded','failed','cancelled')",
            )?;
            let rows = statement
                .query_map([task_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for child_id in children {
            self.cancel(&child_id)?;
        }
        let mut connection = self.connection()?;
        let worker_process = connection
            .query_row(
                "SELECT worker_pid, worker_pgid, process_fingerprint FROM task_attempts
                 WHERE task_id=?1 AND status IN ('starting','running')
                 ORDER BY attempt_no DESC LIMIT 1",
                [task_id],
                |row| {
                    Ok((
                        row.get::<_, Option<u32>>(0)?,
                        row.get::<_, Option<u32>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let (batch_id, root_session_id, raw_payload): (String, String, String) = connection
            .query_row(
                "SELECT batch_id, root_session_id, payload_json FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("runtime task not found")?;
        let worker_signal = worker_process.map_or_else(
            || signal_worker_process(None, None, None),
            |(pid, pgid, fingerprint)| signal_worker_process(pid, pgid, fingerprint.as_deref()),
        );
        if worker_signal.outcome == "skipped" {
            tracing::warn!(
                %task_id,
                reason = worker_signal.reason.as_deref().unwrap_or("unknown"),
                "skipped RuntimeTask worker signal because process identity was not verified"
            );
        }
        let mut spec = serde_json::from_str::<RuntimeTaskSpec>(&raw_payload)?;
        if let Some(lease) = spec.worktree_lease.take() {
            if let Err(error) = worktree::archive_rejected(&lease) {
                tracing::warn!(%error, %task_id, "failed to archive cancelled task worktree");
            }
        }
        let mut payload = serde_json::from_str::<Value>(&serialize_spec_preserving_runtime(
            &spec,
            &raw_payload,
        )?)?;
        if let Value::Object(object) = &mut payload {
            object.insert(
                "outputStatus".to_string(),
                Value::String("materializing".to_string()),
            );
        }
        mark_cancelled_review(&mut payload, spec.access);
        let now = Utc::now().to_rfc3339();
        let event_id = format!("{task_id}:cancelled");
        let attempt_id = connection
            .query_row(
                "SELECT id FROM task_attempts WHERE task_id=?1 ORDER BY attempt_no DESC LIMIT 1",
                [task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| format!("cancel_{task_id}"));
        let cancelled_body = format!(
            "# 已取消\n\n任务在完成前被用户取消。\n\n## 原目标\n\n{}",
            spec.objective
        );
        let cancelled_outcome = TaskExecutionOutcome {
            summary: "任务已取消".to_string(),
            document: TaskDocumentOutcome {
                title: format!(
                    "{}（已取消）",
                    spec.deliverable_title.as_deref().unwrap_or("委派任务")
                ),
                format: "markdown".to_string(),
                body: cancelled_body.clone(),
            },
            changed_files: Vec::new(),
            verification: Vec::new(),
            suggested_task_update: None,
            decision_candidates: Vec::new(),
            duration_ms: None,
            token_usage: None,
            review_artifact: None,
            native_session_id: None,
        };
        let cancelled_result = serde_json::to_string(&cancelled_outcome)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET status='cancelled', delivery_status='not_ready',
                    payload_json=?2, result_json=?3, updated_at=?4
             WHERE id=?1 AND status NOT IN ('succeeded','failed','cancelled')",
            params![task_id, payload.to_string(), cancelled_result, now],
        )?;
        transaction.execute(
            "UPDATE task_attempts SET status='cancelled', finished_at=?2
             WHERE task_id=?1 AND status IN ('leased','starting','running')",
            params![task_id, now],
        )?;
        insert_event_tx(
            &transaction,
            &event_id,
            task_id,
            Some(&attempt_id),
            "task_cancelled",
            serde_json::json!({ "workerSignal": worker_signal }),
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO task_outbox
             (id, source_event_id, destination, dedupe_key, status, payload_json, created_at, updated_at)
             VALUES (?1, ?2, 'document', ?3, 'pending', ?4, ?5, ?5)",
            params![
                format!("outbox_{}", uuid::Uuid::now_v7().simple()),
                event_id,
                format!("runtime-task-output:{attempt_id}"),
                serde_json::json!({
                    "taskId": task_id,
                    "attemptId": attempt_id,
                    "batchId": batch_id,
                    "rootSessionId": root_session_id,
                    "executorId": spec.resolved_executor_id.as_deref().unwrap_or("legacy"),
                    "agentName": spec.display_name.as_deref().unwrap_or("旧版委派"),
                    "title": cancelled_outcome.document.title,
                    "body": cancelled_body,
                }).to_string(),
                now,
            ],
        )?;
        transaction.commit()?;
        self.outbox_notify.notify_one();
        self.emit_persisted_event(&event_id)?;
        self.task(task_id)?.context("runtime task not found")
    }

    pub fn cancel_for_session(&self, session_id: &str) -> Result<usize> {
        let task_ids = self
            .list()?
            .into_iter()
            .filter(|task| task.root_session_id == session_id && !is_terminal(&task.status))
            .map(|task| task.id)
            .collect::<Vec<_>>();
        for task_id in &task_ids {
            let _ = self.cancel(task_id);
        }
        Ok(task_ids.len())
    }

    pub fn retry(&self, task_id: &str) -> Result<RuntimeTaskRecord> {
        self.retry_with_executor(task_id, None)
    }

    pub fn retry_with_executor(
        &self,
        task_id: &str,
        executor_override: Option<DelegatedExecutor>,
    ) -> Result<RuntimeTaskRecord> {
        let connection = self.connection()?;
        let (raw_payload, current_status): (String, String) = connection
            .query_row(
                "SELECT payload_json, status FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("runtime task not found")?;
        if !matches!(
            current_status.as_str(),
            "failed" | "cancelled" | "recovery_required" | "waiting_configuration"
        ) {
            anyhow::bail!("only failed or cancelled tasks can be retried");
        }
        if executor_override == Some(DelegatedExecutor::Auto) {
            anyhow::bail!("executor override must explicitly name an executor");
        }
        let mut payload = if current_status == "waiting_configuration"
            || executor_override.is_some()
        {
            let mut spec = serde_json::from_str::<RuntimeTaskSpec>(&raw_payload)?;
            if let Some(executor) = executor_override {
                spec.executor = executor;
            }
            spec.resolved_executor_kind = None;
            spec.resolved_executor_id = None;
            spec.resolved_provider_id = None;
            spec.resolved_model = None;
            spec.resolved_effort = None;
            spec.resolved_thinking = false;
            spec.resolved_permission_mode = None;
            let waiting = resolve_execution_spec(&mut spec);
            if let Some(reason) = waiting {
                connection.execute(
                    "UPDATE runtime_tasks SET status='waiting_configuration', error=?2,
                            payload_json=?3, updated_at=?4 WHERE id=?1",
                    params![
                        task_id,
                        reason,
                        serialize_spec_preserving_runtime(&spec, &raw_payload)?,
                        Utc::now().to_rfc3339()
                    ],
                )?;
                return self.task(task_id)?.context("runtime task not found");
            }
            serde_json::from_str::<Value>(&serialize_spec_preserving_runtime(&spec, &raw_payload)?)?
        } else {
            serde_json::from_str::<Value>(&raw_payload)?
        };
        if let Ok(mut spec) = serde_json::from_value::<RuntimeTaskSpec>(payload.clone()) {
            if let Some(lease) = spec.worktree_lease.take() {
                if let Err(error) = worktree::archive_rejected(&lease) {
                    tracing::warn!(%error, %task_id, "failed to archive stale retry worktree");
                }
                payload = serde_json::from_str(&serialize_spec_preserving_runtime(
                    &spec,
                    &payload.to_string(),
                )?)?;
            }
        }
        if let Value::Object(object) = &mut payload {
            object.insert("outputStatus".into(), Value::String("pending".into()));
            let mutating = object
                .get("access")
                .and_then(Value::as_str)
                .is_some_and(|access| access == "mutating");
            object.insert(
                "reviewStatus".into(),
                Value::String(if mutating { "pending" } else { "not_required" }.into()),
            );
            object.remove("failure");
        }
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE runtime_tasks SET status='queued', delivery_status='not_ready', result_json=NULL,
                    error=NULL, payload_json=?2, updated_at=?3 WHERE id=?1
                    AND status IN ('failed','cancelled','recovery_required','waiting_configuration')",
            params![task_id, payload.to_string(), Utc::now().to_rfc3339()],
        )?;
        if changed == 0 {
            anyhow::bail!("only failed or cancelled tasks can be retried");
        }
        attention::resolve_task_tx(
            &transaction,
            task_id,
            Some(&[
                "task_failed",
                "output_failed",
                "background_failure",
                "background_interrupted",
                "decision_required",
                "merge_required",
            ]),
        )?;
        transaction.commit()?;
        // Background tool projections never use worker leases. Safe replay is
        // handled by the BackgroundTaskManager after this state transition.
        if payload.get("mode").and_then(Value::as_str) == Some("background_tool")
            || payload.get("kind").and_then(Value::as_str) == Some("background_tool")
        {
            // Background projections are advanced by BackgroundTaskManager.
            // Mark running now so the inbox reflects an active retry.
            let connection = self.connection()?;
            connection.execute(
                "UPDATE runtime_tasks SET status='running', delivery_status='not_ready',
                        attempt=attempt+1, updated_at=?2 WHERE id=?1",
                params![task_id, Utc::now().to_rfc3339()],
            )?;
            return self.task(task_id)?.context("runtime task not found");
        }
        if self.launch_workers {
            self.try_start(task_id)?;
        }
        self.task(task_id)?.context("runtime task not found")
    }

    pub fn resolve_decision(
        &self,
        task_id: &str,
        request: TaskDecisionRequest,
    ) -> Result<TaskDecisionResponse> {
        if request.client_action_id.trim().is_empty() {
            anyhow::bail!("clientActionId is required");
        }
        if request.option.trim().is_empty() {
            anyhow::bail!("option is required");
        }
        if let Some((action, response)) = self
            .connection()?
            .query_row(
                "SELECT action, response_json FROM task_review_actions WHERE client_action_id=?1",
                [&request.client_action_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if action != "decision" {
                anyhow::bail!("clientActionId was already used for another action");
            }
            return serde_json::from_str(&response).map_err(Into::into);
        }
        let _guard = self
            .review_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("task decision lock poisoned"))?;
        let mut connection = self.connection()?;
        let (raw_payload, raw_result, status): (String, Option<String>, String) = connection
            .query_row(
                "SELECT payload_json, result_json, status FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("runtime task not found")?;
        if status != "waiting_user" {
            anyhow::bail!("task is not waiting for a decision");
        }
        let result = raw_result
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()?
            .context("waiting task has no decision payload")?;
        let question = result
            .get("question")
            .and_then(Value::as_str)
            .context("waiting task has no decision question")?;
        let options = result
            .get("options")
            .and_then(Value::as_array)
            .context("waiting task has no decision options")?;
        let selected = request.option.trim();
        if !options
            .iter()
            .filter_map(Value::as_str)
            .any(|option| option == selected)
        {
            anyhow::bail!("selected option is not available for this decision");
        }
        let native_session_id = connection
            .query_row(
                "SELECT native_session_id FROM task_attempts
                 WHERE task_id=?1 AND native_session_id IS NOT NULL
                 ORDER BY attempt_no DESC LIMIT 1",
                [task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .context("waiting decision has no resumable native session")?;
        let mut spec = serde_json::from_str::<RuntimeTaskSpec>(&raw_payload)?;
        if spec.resolved_executor_kind.as_deref() != Some("acp_cli") {
            anyhow::bail!("only ACP CLI tasks can resume a native decision");
        }
        spec.resume_session_id = Some(native_session_id);
        spec.objective.push_str(&format!(
            "\n\n## 用户对执行器问题的决定\n问题：{}\n选择：{}",
            question.trim(),
            selected
        ));
        if let Some(note) = request
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
        {
            spec.objective.push_str(&format!("\n补充说明：{note}"));
        }
        let mut payload = serde_json::from_str::<Value>(&serialize_spec_preserving_runtime(
            &spec,
            &raw_payload,
        )?)?;
        if let Value::Object(object) = &mut payload {
            object.insert("outputStatus".into(), Value::String("pending".into()));
            object.insert(
                "reviewStatus".into(),
                Value::String(
                    if spec.access == DelegatedAccess::Mutating {
                        "pending"
                    } else {
                        "not_required"
                    }
                    .into(),
                ),
            );
        }
        let response = TaskDecisionResponse {
            task_id: task_id.to_string(),
            status: "queued".to_string(),
            selected_option: selected.to_string(),
        };
        let now = Utc::now().to_rfc3339();
        let event_id = format!("{task_id}:decision:{}", request.client_action_id.trim());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET status='queued', delivery_status='not_ready',
                    objective=?2, payload_json=?3, result_json=NULL, error=NULL, updated_at=?4
             WHERE id=?1 AND status='waiting_user'",
            params![task_id, spec.objective, payload.to_string(), now],
        )?;
        transaction.execute(
            "UPDATE runtime_batches SET status='running', updated_at=?2
             WHERE id=(SELECT batch_id FROM runtime_tasks WHERE id=?1)",
            params![task_id, now],
        )?;
        transaction.execute(
            "INSERT INTO task_review_actions
             (client_action_id, task_id, action, response_json, created_at)
             VALUES (?1, ?2, 'decision', ?3, ?4)",
            params![
                request.client_action_id,
                task_id,
                serde_json::to_string(&response)?,
                now,
            ],
        )?;
        insert_event_tx(
            &transaction,
            &event_id,
            task_id,
            None,
            "task_decision_resolved",
            serde_json::to_value(&response)?,
        )?;
        attention::resolve_task_tx(&transaction, task_id, Some(&["decision_required"]))?;
        transaction.commit()?;
        drop(_guard);
        self.emit_persisted_event(&event_id)?;
        if self.launch_workers {
            self.try_start(task_id)?;
        }
        Ok(response)
    }

    pub fn review(&self, task_id: &str, request: TaskReviewRequest) -> Result<TaskReviewResponse> {
        if request.client_action_id.trim().is_empty() {
            anyhow::bail!("clientActionId is required");
        }
        if let Some(existing) = self
            .connection()?
            .query_row(
                "SELECT response_json FROM task_review_actions WHERE client_action_id=?1",
                [&request.client_action_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return serde_json::from_str(&existing).map_err(Into::into);
        }
        let _guard = self
            .review_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("task review lock poisoned"))?;
        let connection = self.connection()?;
        let (raw_payload, raw_result, status): (String, Option<String>, String) = connection
            .query_row(
                "SELECT payload_json, result_json, status FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("runtime task not found")?;
        if !is_terminal(&status) {
            anyhow::bail!("task must be terminal before review");
        }
        let mut payload = serde_json::from_str::<Value>(&raw_payload)?;
        let current_revision = payload
            .get("reviewRevision")
            .and_then(Value::as_u64)
            .unwrap_or_default() as u32;
        if request.review_revision != current_revision {
            anyhow::bail!(
                "review revision changed (expected {current_revision}, got {})",
                request.review_revision
            );
        }
        let mut spec = serde_json::from_value::<RuntimeTaskSpec>(payload.clone())?;
        if matches!(request.action, TaskReviewAction::Confirm) {
            if spec.access != DelegatedAccess::ReadOnly {
                anyhow::bail!("mutating tasks must be applied or rejected through Git review");
            }
            if payload.get("outputStatus").and_then(Value::as_str) != Some("ready") {
                anyhow::bail!("read-only task output must be ready before confirmation");
            }
        } else if spec.access != DelegatedAccess::Mutating {
            anyhow::bail!("read-only tasks only support confirm review actions");
        }
        let lease = spec.worktree_lease.clone();
        let artifact = raw_result
            .as_deref()
            .and_then(|raw| serde_json::from_str::<TaskExecutionOutcome>(raw).ok())
            .and_then(|outcome| outcome.review_artifact);

        let apply_intent = if matches!(request.action, TaskReviewAction::Apply) {
            let artifact = artifact
                .as_ref()
                .context("task has no materialized Git review patch")?;
            let existing = connection
                .query_row(
                    "SELECT state, affected_paths_json, task_id, review_revision, artifact_sha256
                     FROM task_review_apply_intents WHERE client_action_id=?1",
                    [&request.client_action_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, u32>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            if let Some(intent) = existing {
                if intent.2 != task_id
                    || intent.3 != current_revision
                    || intent.4 != artifact.patch_sha256
                {
                    anyhow::bail!("Git apply intent does not match this task revision");
                }
                Some((intent.0, intent.1))
            } else {
                let now = Utc::now().to_rfc3339();
                connection.execute(
                    "INSERT INTO task_review_apply_intents
                     (client_action_id, task_id, review_revision, artifact_sha256, state,
                      request_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, ?6, ?6)",
                    params![
                        request.client_action_id,
                        task_id,
                        current_revision,
                        artifact.patch_sha256,
                        serde_json::to_string(&request)?,
                        now,
                    ],
                )?;
                Some(("prepared".to_string(), None))
            }
        } else {
            None
        };

        let next_revision = current_revision.saturating_add(1);
        let mut affected_paths = Vec::new();
        let mut reason = None;
        let mut next_result = raw_result.clone();
        let review_status = match request.action {
            TaskReviewAction::Confirm => "applied",
            TaskReviewAction::Apply => {
                let lease = lease
                    .as_ref()
                    .context("task has no isolated Git worktree")?;
                let artifact = artifact
                    .clone()
                    .context("task has no materialized Git review patch")?;
                let intent = apply_intent.context("Git apply intent was not persisted")?;
                let outcome = if intent.0 == "applied" {
                    worktree::ApplyOutcome::Applied {
                        affected_paths: intent
                            .1
                            .as_deref()
                            .map(serde_json::from_str)
                            .transpose()?
                            .unwrap_or_else(|| artifact.affected_paths.clone()),
                    }
                } else {
                    worktree::verify_and_apply(lease, &artifact)?
                };
                match outcome {
                    worktree::ApplyOutcome::Applied {
                        affected_paths: paths,
                    } => {
                        connection.execute(
                            "UPDATE task_review_apply_intents
                             SET state='applied', affected_paths_json=?2, updated_at=?3
                             WHERE client_action_id=?1",
                            params![
                                request.client_action_id,
                                serde_json::to_string(&paths)?,
                                Utc::now().to_rfc3339(),
                            ],
                        )?;
                        affected_paths = paths
                            .into_iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect();
                        "applied"
                    }
                    worktree::ApplyOutcome::MergeRequired { reason: conflict } => {
                        reason = Some(conflict);
                        "merge_required"
                    }
                }
            }
            TaskReviewAction::Reject => {
                lease
                    .as_ref()
                    .context("task has no isolated Git worktree")?;
                "rejected"
            }
            TaskReviewAction::RequestChanges => {
                lease
                    .as_ref()
                    .context("task has no isolated Git worktree")?;
                let note = request
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|note| !note.is_empty())
                    .context("request_changes requires a note")?;
                spec.objective = format!(
                    "{}\n\n## 用户审阅要求（revision {}）\n{}",
                    spec.objective, next_revision, note
                );
                spec.resume_session_id = connection
                    .query_row(
                        "SELECT native_session_id FROM task_attempts
                         WHERE task_id=?1 AND native_session_id IS NOT NULL
                         ORDER BY attempt_no DESC LIMIT 1",
                        [task_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                payload = serde_json::to_value(&spec)?;
                "changes_requested"
            }
        };
        if matches!(
            request.action,
            TaskReviewAction::Apply | TaskReviewAction::Reject
        ) {
            let lease = lease
                .as_ref()
                .context("task has no isolated Git worktree")?;
            let archived = worktree::archive_rejected(lease)?;
            let archived_lease = worktree::archived_review_lease(lease, &archived);
            spec.worktree_lease = None;
            payload = serde_json::from_str(&serialize_spec_preserving_runtime(
                &spec,
                &payload.to_string(),
            )?)?;
            if let Value::Object(object) = &mut payload {
                object.insert(
                    "archivedReviewLease".to_string(),
                    serde_json::to_value(&archived_lease)?,
                );
            }
            if let (Some(raw), Some(artifact)) = (next_result.as_deref(), artifact.as_ref()) {
                let mut outcome = serde_json::from_str::<TaskExecutionOutcome>(raw)?;
                outcome.review_artifact = Some(worktree::archived_review_artifact(
                    lease, &archived, artifact,
                ));
                next_result = Some(serde_json::to_string(&outcome)?);
            }
        }
        if let Value::Object(object) = &mut payload {
            object.insert(
                "reviewStatus".to_string(),
                Value::String(review_status.to_string()),
            );
            object.insert(
                "reviewRevision".to_string(),
                Value::Number(next_revision.into()),
            );
            if matches!(request.action, TaskReviewAction::RequestChanges) {
                object.insert(
                    "outputStatus".to_string(),
                    Value::String("pending".to_string()),
                );
            }
        }
        let response = TaskReviewResponse {
            task_id: task_id.to_string(),
            review_status: review_status.to_string(),
            review_revision: next_revision,
            affected_paths,
            reason,
        };
        let client_action_id = request.client_action_id.clone();
        let projection_payload = serde_json::json!({
            "clientActionId": client_action_id,
            "taskId": task_id,
            "taskUpdate": request.task_update,
            "memoryEntries": request.memory_entries,
        });
        let needs_projection = should_project_review(review_status, &projection_payload);
        let review_event_id = format!("{task_id}:review:{client_action_id}");
        let transaction = connection.unchecked_transaction()?;
        if matches!(request.action, TaskReviewAction::RequestChanges) {
            transaction.execute(
                "UPDATE runtime_tasks SET status='queued', delivery_status='not_ready',
                        payload_json=?2, result_json=NULL, error=NULL, updated_at=?3 WHERE id=?1",
                params![task_id, payload.to_string(), Utc::now().to_rfc3339()],
            )?;
            transaction.execute(
                "UPDATE runtime_batches SET status='running', updated_at=?2
                 WHERE id=(SELECT batch_id FROM runtime_tasks WHERE id=?1)",
                params![task_id, Utc::now().to_rfc3339()],
            )?;
        } else {
            transaction.execute(
                "UPDATE runtime_tasks SET payload_json=?2, result_json=?3, updated_at=?4 WHERE id=?1",
                params![
                    task_id,
                    payload.to_string(),
                    next_result,
                    Utc::now().to_rfc3339()
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO task_review_actions
             (client_action_id, task_id, action, response_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                request.client_action_id,
                task_id,
                match request.action {
                    TaskReviewAction::Confirm => "confirm",
                    TaskReviewAction::RequestChanges => "request_changes",
                    TaskReviewAction::Reject => "reject",
                    TaskReviewAction::Apply => "apply",
                },
                serde_json::to_string(&response)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        if matches!(request.action, TaskReviewAction::Apply) {
            transaction.execute(
                "DELETE FROM task_review_apply_intents WHERE client_action_id=?1",
                [&request.client_action_id],
            )?;
        }
        insert_event_tx(
            &transaction,
            &review_event_id,
            task_id,
            None,
            "task_review_updated",
            serde_json::to_value(&response)?,
        )?;
        if review_status == "merge_required" {
            let envelope =
                failure_from_status_and_error("merge_required", response.reason.as_deref(), 1);
            create_attention_with_failure_tx(
                &transaction,
                task_id,
                "merge_required",
                &format!("runtime-task:{task_id}:merge-required:{next_revision}"),
                "改动需要手动合并",
                serde_json::json!({
                    "reviewRevision": next_revision,
                    "reason": response.reason,
                }),
                Some(&envelope),
            )?;
        } else {
            attention::resolve_task_tx(&transaction, task_id, None)?;
        }
        if needs_projection {
            transaction.execute(
                "INSERT OR IGNORE INTO task_outbox
                 (id, source_event_id, destination, dedupe_key, status, payload_json,
                  created_at, updated_at)
                 VALUES (?1, ?2, 'review_projection', ?3, 'pending', ?4, ?5, ?5)",
                params![
                    format!("outbox_{}", uuid::Uuid::now_v7().simple()),
                    review_event_id,
                    format!("review-projection:{client_action_id}"),
                    projection_payload.to_string(),
                    Utc::now().to_rfc3339(),
                ],
            )?;
        }
        transaction.commit()?;
        drop(_guard);
        self.emit_persisted_event(&review_event_id)?;
        if needs_projection {
            self.outbox_notify.notify_one();
        }
        if matches!(request.action, TaskReviewAction::RequestChanges) {
            self.try_start(task_id)?;
        }
        Ok(response)
    }

    pub fn follow_up(&self, task_id: &str, objective: String) -> Result<TaskBatchAccepted> {
        if objective.trim().is_empty() {
            anyhow::bail!("follow-up objective is required");
        }
        let mut connection = self.connection()?;
        let (batch_id, payload, status): (String, String, String) = connection
            .query_row(
                "SELECT batch_id, payload_json, status FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("runtime task not found")?;
        if !is_terminal(&status) {
            anyhow::bail!("follow-up requires a terminal task");
        }
        let mut spec: RuntimeTaskSpec = serde_json::from_str(&payload)?;
        if let Some(lease) = spec.worktree_lease.take() {
            if let Err(error) = worktree::archive_rejected(&lease) {
                tracing::warn!(%error, %task_id, "failed to archive follow-up baseline worktree");
            }
        }
        spec.objective = format!("{}\n\n## 追加要求\n{}", spec.objective, objective.trim());
        spec.resume_session_id = connection
            .query_row(
                "SELECT native_session_id FROM task_attempts
                 WHERE task_id=?1 AND native_session_id IS NOT NULL
                 ORDER BY attempt_no DESC LIMIT 1",
                [task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let mut next_payload = serde_json::to_value(&spec)?;
        let revision = serde_json::from_str::<Value>(&payload)?
            .get("reviewRevision")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            .saturating_add(1);
        if let Value::Object(object) = &mut next_payload {
            object.insert("outputStatus".into(), Value::String("pending".into()));
            object.insert(
                "reviewStatus".into(),
                Value::String(
                    if spec.access == DelegatedAccess::Mutating {
                        "pending"
                    } else {
                        "not_required"
                    }
                    .into(),
                ),
            );
            object.insert("reviewRevision".into(), Value::Number(revision.into()));
        }
        let event_id = format!("{task_id}:follow-up:{revision}");
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runtime_tasks SET status='queued', delivery_status='not_ready', payload_json=?2,
                    result_json=NULL, error=NULL, updated_at=?3 WHERE id=?1",
            params![task_id, next_payload.to_string(), Utc::now().to_rfc3339()],
        )?;
        transaction.execute(
            "UPDATE runtime_batches SET status='running', updated_at=?2 WHERE id=?1",
            params![batch_id, Utc::now().to_rfc3339()],
        )?;
        insert_event_tx(
            &transaction,
            &event_id,
            task_id,
            None,
            "task_follow_up_queued",
            serde_json::json!({ "objective": objective }),
        )?;
        transaction.commit()?;
        self.emit_persisted_event(&event_id)?;
        self.try_start(task_id)?;
        Ok(TaskBatchAccepted {
            batch_id,
            task_ids: vec![task_id.to_string()],
            status: "accepted".to_string(),
        })
    }

    pub fn mark_delivery(&self, task_id: &str, status: &str) -> Result<()> {
        self.connection()?.execute(
            "UPDATE runtime_tasks SET delivery_status=?2, updated_at=?3 WHERE id=?1",
            params![task_id, status, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    fn start_ready_tasks(&self) -> Result<()> {
        let ids = {
            let connection = self.connection()?;
            let mut statement = connection.prepare(
                "SELECT id FROM runtime_tasks WHERE status='queued' ORDER BY created_at LIMIT 32",
            )?;
            let task_ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            task_ids
        };
        for id in ids {
            let _ = self.try_start(&id);
        }
        Ok(())
    }

    fn verify_attempt(&self, attempt_id: &str, token: &str, lease_epoch: u64) -> Result<()> {
        let expected = self.connection()?.query_row(
            "SELECT attempt.token_hash, attempt.lease_epoch, attempt.status,
                    attempt.attempt_no, task.attempt
             FROM task_attempts attempt
             JOIN runtime_tasks task ON task.id=attempt.task_id
             WHERE attempt.id=?1",
            [attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                ))
            },
        )?;
        if expected.0 != sha256(token) || expected.1 != lease_epoch || expected.3 != expected.4 {
            anyhow::bail!("stale or unauthorized task attempt callback");
        }
        Ok(())
    }

    fn emit_persisted_event(&self, event_id: &str) -> Result<()> {
        if let Some(event) = self
            .connection()?
            .query_row(
                "SELECT sequence, event_id, task_id, attempt_id, kind, payload_json, created_at
             FROM task_events WHERE event_id=?1",
                [event_id],
                row_to_event,
            )
            .optional()?
        {
            let _ = self.event_tx.send(event);
        }
        Ok(())
    }

    fn import_spool(&self) -> Result<()> {
        let spool = self.data_dir.join("task-callback-spool");
        let Ok(entries) = fs::read_dir(&spool) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            if let Ok(envelope) = serde_json::from_slice::<SpoolEnvelope>(&bytes) {
                let capability_valid = self.callback_capability_valid(
                    &envelope.attempt_id,
                    &envelope.request.callback_token,
                    envelope.request.lease_epoch,
                )?;
                let event_id = envelope.request.event_id.clone();
                let completed = self
                    .complete(&envelope.attempt_id, envelope.request)
                    .is_ok();
                if completed || capability_valid {
                    if !completed {
                        let _ = self.record_stale_callback(&envelope.attempt_id, &event_id);
                    }
                    let _ = fs::remove_file(path);
                }
                continue;
            }
            let Ok(envelope) = serde_json::from_slice::<AttemptEventSpoolEnvelope>(&bytes) else {
                continue;
            };
            let capability_valid = self.callback_capability_valid(
                &envelope.attempt_id,
                &envelope.request.callback_token,
                envelope.request.lease_epoch,
            )?;
            let event_id = envelope.request.event_id.clone();
            let recorded = self
                .record_attempt_event(&envelope.attempt_id, envelope.request)
                .is_ok();
            if recorded || capability_valid {
                if !recorded {
                    let _ = self.record_stale_callback(&envelope.attempt_id, &event_id);
                }
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }

    fn recover_orphaned_attempts(&self) -> Result<()> {
        let mut connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT a.id, a.task_id, a.status, a.worker_pid, a.process_fingerprint
             FROM task_attempts a
             JOIN runtime_tasks t ON t.id=a.task_id
             WHERE a.status IN ('leased','starting','running')
               AND t.status IN ('leased','starting','running')",
        )?;
        let attempts = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<u32>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let mut recovered_event_ids = Vec::new();
        for (attempt_id, task_id, attempt_status, pid, fingerprint) in attempts {
            let (alive, identity_reason) = match (pid, fingerprint.as_deref()) {
                (Some(pid), Some(fingerprint)) => {
                    match verify_process_fingerprint(pid, fingerprint) {
                        Ok(true) => (true, "verified".to_string()),
                        Ok(false) => (false, "process_fingerprint_mismatch".to_string()),
                        Err(error) => (false, format!("process_fingerprint_unverifiable: {error}")),
                    }
                }
                (Some(_), None) => (false, "missing_process_fingerprint".to_string()),
                (None, _) => (false, "worker_pid_missing".to_string()),
            };
            if alive {
                continue;
            }
            let before_launch = attempt_status == "leased";
            let (next_attempt_status, next_task_status, event_kind, message) = if before_launch {
                (
                    "start_failed",
                    "waiting_configuration",
                    "task_start_failed",
                    "daemon restarted before worker launch completed",
                )
            } else {
                (
                    "orphaned",
                    "recovery_required",
                    "task_recovery_required",
                    "worker disappeared; side effects may be incomplete",
                )
            };
            let now = Utc::now().to_rfc3339();
            let event_id = format!("{attempt_id}:startup-recovery");
            let raw_payload: String = connection.query_row(
                "SELECT payload_json FROM runtime_tasks WHERE id=?1",
                [&task_id],
                |row| row.get(0),
            )?;
            let attempt: u32 = connection
                .query_row(
                    "SELECT attempt FROM runtime_tasks WHERE id=?1",
                    [&task_id],
                    |row| row.get(0),
                )
                .unwrap_or(1);
            let envelope =
                failure_from_status_and_error(next_task_status, Some(message), attempt.max(1));
            let payload = payload_with_failure(&raw_payload, Some(&envelope))?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE task_attempts SET status=?2, error=?3, finished_at=?4 WHERE id=?1",
                params![attempt_id, next_attempt_status, message, now],
            )?;
            transaction.execute(
                "UPDATE runtime_tasks SET status=?2, error=?3, payload_json=?4, updated_at=?5 WHERE id=?1",
                params![task_id, next_task_status, message, payload, now],
            )?;
            insert_event_tx(
                &transaction,
                &event_id,
                &task_id,
                Some(&attempt_id),
                event_kind,
                serde_json::json!({
                    "reason": message,
                    "recoveredAttemptStatus": attempt_status,
                    "processIdentity": identity_reason,
                    "failure": envelope,
                }),
            )?;
            create_attention_with_failure_tx(
                &transaction,
                &task_id,
                if next_task_status == "recovery_required" {
                    "background_interrupted"
                } else {
                    "task_failed"
                },
                &format!("runtime-task:{task_id}:attempt:{attempt_id}:startup-recovery"),
                if next_task_status == "recovery_required" {
                    "协作者需要恢复"
                } else {
                    "协作者启动失败"
                },
                serde_json::json!({
                    "attemptId": attempt_id,
                    "error": message,
                    "recoveryRequired": next_task_status == "recovery_required",
                }),
                Some(&envelope),
            )?;
            transaction.commit()?;
            recovered_event_ids.push(event_id);
        }
        for event_id in recovered_event_ids {
            self.emit_persisted_event(&event_id)?;
        }
        Ok(())
    }

    pub fn recover_expired_attempts(&self) -> Result<usize> {
        let mut connection = self.connection()?;
        let now = Utc::now();
        let mut statement = connection.prepare(
            "SELECT a.id, a.task_id, a.status, a.worker_pid, a.worker_pgid,
                    a.process_fingerprint FROM task_attempts a
             JOIN runtime_tasks t ON t.id=a.task_id
             WHERE a.status IN ('leased','starting','running')
               AND t.status IN ('leased','starting','running')
               AND a.lease_expires_at<=?1",
        )?;
        let attempts = statement
            .query_map([now.to_rfc3339()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<u32>>(3)?,
                    row.get::<_, Option<u32>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let mut event_ids = Vec::new();
        for (attempt_id, task_id, attempt_status, pid, pgid, fingerprint) in &attempts {
            let worker_signal = signal_worker_process(*pid, *pgid, fingerprint.as_deref());
            if worker_signal.outcome == "skipped" {
                tracing::warn!(
                    %task_id,
                    reason = worker_signal.reason.as_deref().unwrap_or("unknown"),
                    "skipped expired RuntimeTask worker signal because process identity was not verified"
                );
            }
            let before_launch = attempt_status == "leased";
            let (attempt_next, task_next, event_kind, message) = if before_launch {
                (
                    "start_failed",
                    "waiting_configuration",
                    "task_start_failed",
                    "worker lease expired before launch",
                )
            } else {
                (
                    "orphaned",
                    "recovery_required",
                    "task_recovery_required",
                    "worker heartbeat lease expired; side effects may be incomplete",
                )
            };
            let event_id = format!("{attempt_id}:lease-expired");
            let timestamp = Utc::now().to_rfc3339();
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE task_attempts SET status=?2, error=?3, finished_at=?4
                 WHERE id=?1 AND status IN ('leased','starting','running') AND lease_expires_at<=?5",
                params![attempt_id, attempt_next, message, timestamp, now.to_rfc3339()],
            )?;
            if changed == 0 {
                transaction.rollback()?;
                continue;
            }
            let raw_payload: String = transaction.query_row(
                "SELECT payload_json FROM runtime_tasks WHERE id=?1",
                [task_id],
                |row| row.get(0),
            )?;
            let attempt: u32 = transaction
                .query_row(
                    "SELECT attempt FROM runtime_tasks WHERE id=?1",
                    [task_id],
                    |row| row.get(0),
                )
                .unwrap_or(1);
            let envelope = failure_from_status_and_error(task_next, Some(message), attempt.max(1));
            let payload = payload_with_failure(&raw_payload, Some(&envelope))?;
            transaction.execute(
                "UPDATE runtime_tasks SET status=?2, error=?3, payload_json=?4, updated_at=?5
                 WHERE id=?1 AND status IN ('leased','starting','running')",
                params![task_id, task_next, message, payload, timestamp],
            )?;
            insert_event_tx(
                &transaction,
                &event_id,
                task_id,
                Some(attempt_id),
                event_kind,
                serde_json::json!({
                    "reason": message,
                    "leaseExpired": true,
                    "workerSignal": worker_signal,
                    "failure": envelope,
                }),
            )?;
            create_attention_with_failure_tx(
                &transaction,
                task_id,
                if task_next == "recovery_required" {
                    "background_interrupted"
                } else {
                    "task_failed"
                },
                &format!("runtime-task:{task_id}:attempt:{attempt_id}:lease-expired"),
                if task_next == "recovery_required" {
                    "协作者需要恢复"
                } else {
                    "协作者启动失败"
                },
                serde_json::json!({
                    "attemptId": attempt_id,
                    "error": message,
                    "recoveryRequired": task_next == "recovery_required",
                }),
                Some(&envelope),
            )?;
            transaction.commit()?;
            event_ids.push(event_id);
        }
        for event_id in &event_ids {
            self.emit_persisted_event(event_id)?;
        }
        self.start_ready_tasks()?;
        Ok(event_ids.len())
    }

    fn recover_review_apply_intents(&self) -> Result<()> {
        let intents = {
            let connection = self.connection()?;
            let mut statement = connection.prepare(
                "SELECT task_id, request_json FROM task_review_apply_intents
                 WHERE state IN ('prepared','applied') ORDER BY created_at",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (task_id, raw_request) in intents {
            let request = serde_json::from_str::<TaskReviewRequest>(&raw_request)?;
            if let Err(error) = self.review(&task_id, request) {
                tracing::warn!(%error, %task_id, "failed to recover Git apply intent");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpoolEnvelope {
    attempt_id: String,
    request: CompleteAttemptRequest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttemptEventSpoolEnvelope {
    attempt_id: String,
    request: AttemptEventRequest,
}

pub async fn run_worker(descriptor_path: PathBuf) -> Result<()> {
    let bytes = fs::read(&descriptor_path)
        .with_context(|| format!("read task descriptor {}", descriptor_path.display()))?;
    let descriptor: WorkerDescriptor = serde_json::from_slice(&bytes)?;
    let _ = fs::remove_file(&descriptor_path);
    let before = snapshot_text_workspace(Path::new(&descriptor.cwd));
    let heartbeat_descriptor = descriptor.clone();
    let heartbeat = tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let response = client
                .post(format!(
                    "{}/internal/task-attempts/{}/heartbeat",
                    heartbeat_descriptor.callback_base, heartbeat_descriptor.attempt_id
                ))
                .json(&serde_json::json!({
                    "callbackToken": heartbeat_descriptor.callback_token,
                    "leaseEpoch": heartbeat_descriptor.lease_epoch,
                }))
                .send()
                .await;
            if response.is_err() {
                continue;
            }
        }
    });
    let execution = descriptor.execution.clone();
    let dispatch_context = WorkerDispatchContext {
        callback_base: descriptor.callback_base.clone(),
        attempt_id: descriptor.attempt_id.clone(),
        callback_token: descriptor.callback_token.clone(),
        lease_epoch: descriptor.lease_epoch,
    };
    let result: WorkerExecutionResult = async {
        let continuation_context =
            load_worker_continuations(&descriptor.data_dir, &descriptor.continuation_refs)?;
        // Both adapters run in an unattended RuntimeTask process and must use
        // the same fail-closed filesystem boundary. A writable policy is only
        // possible when the daemon supplied a validated Git worktree lease.
        crate::tools::fs_local::install_worker_execution_policy(
            Path::new(&descriptor.cwd),
            &descriptor.data_dir,
            descriptor.review_lease.is_some(),
        )?;
        match execution {
            ResolvedExecution::AcpCli { args } => {
                let mut args = args;
                if !continuation_context.is_empty() {
                    args.task.push_str(&continuation_context);
                }
                args.task.push_str(task_outcome_contract());
                let backend = backend_for(args.backend.as_deref())?;
                let listener = runtime_task_native_session_listener(&descriptor);
                let result = execute_code_agent_with_listener(
                    args,
                    backend.as_ref(),
                    Path::new(&descriptor.cwd),
                    Some(listener),
                )
                .await?;
                Ok(WorkerExecutionOutput {
                    status: result.status,
                    output: result.output,
                    native_session_id: Some(result.session_id),
                    duration_ms: Some(result.duration_ms),
                    token_usage: result.usage.map(task_token_usage),
                    child_batch: None,
                    question: result.question,
                    options: result.options.unwrap_or_default(),
                })
            }
            ResolvedExecution::PwcliAgent {
                objective,
                role,
                access,
                provider_id,
                model,
                effort,
                thinking,
            } => {
                let mut prompt = internal_agent_prompt(role, access, &objective);
                prompt.push_str(&continuation_context);
                prompt.push_str(task_outcome_contract());
                let started = std::time::Instant::now();
                let run = crate::app::run_delegated_worker(
                    prompt,
                    access == DelegatedAccess::Mutating,
                    provider_id,
                    model,
                    effort,
                    thinking,
                    Some(dispatch_context),
                )
                .await?;
                Ok(WorkerExecutionOutput {
                    status: "ok".to_string(),
                    output: run.output,
                    native_session_id: None,
                    duration_ms: Some(started.elapsed().as_millis() as u64),
                    token_usage: None,
                    child_batch: run.child_batch,
                    question: None,
                    options: Vec::new(),
                })
            }
        }
    }
    .await;
    heartbeat.abort();
    let request = match result {
        Ok(output) => {
            if let Some(child_batch) = output.child_batch {
                CompleteAttemptRequest {
                    event_id: format!("{}:waiting-children", descriptor.attempt_id),
                    callback_token: descriptor.callback_token.clone(),
                    lease_epoch: descriptor.lease_epoch,
                    outcome: "waiting_children".to_string(),
                    result: Some(serde_json::json!({
                        "summary": output.output,
                        "continuation": {
                            "kind": "fresh_worker_resume",
                            "childBatch": child_batch,
                        }
                    })),
                    error: None,
                    native_session_id: output.native_session_id,
                }
            } else if output.status == "decision_required" {
                CompleteAttemptRequest {
                    event_id: format!("{}:decision-required", descriptor.attempt_id),
                    callback_token: descriptor.callback_token.clone(),
                    lease_epoch: descriptor.lease_epoch,
                    outcome: "waiting_user".to_string(),
                    result: Some(serde_json::json!({
                        "summary": output.output,
                        "question": output.question.unwrap_or_else(|| "请选择下一步".to_string()),
                        "options": output.options,
                        "nativeSessionId": output.native_session_id,
                    })),
                    error: None,
                    native_session_id: output.native_session_id,
                }
            } else {
                let succeeded = output.status == "ok";
                let preview_files = changed_text_files(
                    &before,
                    &snapshot_text_workspace(Path::new(&descriptor.cwd)),
                );
                let (review_artifact, review_error) = match descriptor
                    .review_lease
                    .as_ref()
                    .map(|lease| worktree::finalize_attempt(lease, &descriptor.attempt_id))
                    .transpose()
                {
                    Ok(artifact) => (artifact, None),
                    Err(error) => (
                        None,
                        Some(format!("failed to materialize Git review: {error}")),
                    ),
                };
                let changed_files = complete_review_file_list(
                    preview_files,
                    descriptor.review_lease.as_ref(),
                    review_artifact.as_ref(),
                );
                let final_succeeded = succeeded && review_error.is_none();
                let normalized = if final_succeeded {
                    normalize_task_execution_output(&output.output)
                } else {
                    NormalizedTaskOutput {
                        summary: output.output.trim().chars().take(2_000).collect(),
                        document_body: review_error.as_ref().map_or_else(
                            || output.output.clone(),
                            |error| format!("{}\n\n## Git 审阅产物失败\n\n{error}", output.output),
                        ),
                        suggested_task_update: None,
                        decision_candidates: Vec::new(),
                    }
                };
                let outcome = TaskExecutionOutcome {
                    summary: normalized.summary,
                    document: TaskDocumentOutcome {
                        title: descriptor.deliverable_title.clone(),
                        format: "markdown".to_string(),
                        body: normalized.document_body,
                    },
                    changed_files,
                    verification: review_error
                        .as_ref()
                        .map(|error| {
                            vec![VerificationResult {
                                label: "git review materialization".to_string(),
                                status: "failed".to_string(),
                                detail: Some(error.clone()),
                            }]
                        })
                        .unwrap_or_default(),
                    suggested_task_update: normalized.suggested_task_update,
                    decision_candidates: normalized.decision_candidates,
                    duration_ms: output.duration_ms,
                    token_usage: output.token_usage,
                    review_artifact,
                    native_session_id: output.native_session_id.clone(),
                };
                CompleteAttemptRequest {
                    event_id: format!("{}:complete", descriptor.attempt_id),
                    callback_token: descriptor.callback_token.clone(),
                    lease_epoch: descriptor.lease_epoch,
                    outcome: if final_succeeded {
                        "succeeded"
                    } else {
                        "failed"
                    }
                    .to_string(),
                    error: if !succeeded {
                        Some(output.output.clone())
                    } else {
                        review_error
                    },
                    native_session_id: output.native_session_id,
                    result: Some(serde_json::to_value(outcome)?),
                }
            }
        }
        Err(error) => {
            let message = error.to_string();
            let preview_files = changed_text_files(
                &before,
                &snapshot_text_workspace(Path::new(&descriptor.cwd)),
            );
            let review_artifact = descriptor
                .review_lease
                .as_ref()
                .map(|lease| worktree::finalize_attempt(lease, &descriptor.attempt_id))
                .transpose()
                .ok()
                .flatten();
            let outcome = TaskExecutionOutcome {
                summary: message.clone(),
                document: TaskDocumentOutcome {
                    title: format!("{}（执行诊断）", descriptor.deliverable_title),
                    format: "markdown".to_string(),
                    body: format!(
                        "# 执行未完成\n\n任务 worker 在产出完整结果前失败。\n\n## 诊断\n\n```text\n{}\n```",
                        message
                    ),
                },
                changed_files: complete_review_file_list(
                    preview_files,
                    descriptor.review_lease.as_ref(),
                    review_artifact.as_ref(),
                ),
                verification: vec![VerificationResult {
                    label: "worker execution".to_string(),
                    status: "failed".to_string(),
                    detail: Some(message.clone()),
                }],
                suggested_task_update: None,
                decision_candidates: Vec::new(),
                duration_ms: None,
                token_usage: None,
                review_artifact,
                native_session_id: None,
            };
            CompleteAttemptRequest {
                event_id: format!("{}:complete", descriptor.attempt_id),
                callback_token: descriptor.callback_token.clone(),
                lease_epoch: descriptor.lease_epoch,
                outcome: "failed".to_string(),
                result: Some(serde_json::to_value(outcome)?),
                error: Some(message),
                native_session_id: None,
            }
        }
    };
    if post_completion(&descriptor, &request).await.is_err() {
        write_spool_envelope(
            &descriptor.data_dir,
            &SpoolEnvelope {
                attempt_id: descriptor.attempt_id,
                request,
            },
        )?;
        notify_spool_listener(&descriptor.data_dir).await;
    }
    Ok(())
}

fn write_spool_envelope(data_dir: &Path, envelope: &SpoolEnvelope) -> Result<()> {
    let spool = data_dir.join("task-callback-spool");
    fs::create_dir_all(&spool)?;
    let final_path = spool.join(format!("{}.json", envelope.attempt_id));
    let temporary_path = spool.join(format!(
        ".{}.{}.tmp",
        envelope.attempt_id,
        uuid::Uuid::now_v7().simple()
    ));
    write_private_json(&temporary_path, envelope)?;
    if let Err(error) = fs::rename(&temporary_path, &final_path) {
        let _ = fs::remove_file(&temporary_path);
        if !final_path.is_file() {
            return Err(error.into());
        }
    }
    Ok(())
}

fn write_attempt_event_spool(
    data_dir: &Path,
    envelope: &AttemptEventSpoolEnvelope,
) -> Result<PathBuf> {
    let spool = data_dir.join("task-callback-spool");
    fs::create_dir_all(&spool)?;
    let suffix = &sha256(&envelope.request.event_id)[..16];
    let final_path = spool.join(format!("{}.event-{suffix}.json", envelope.attempt_id));
    let temporary_path = spool.join(format!(
        ".{}.event-{}.tmp",
        envelope.attempt_id,
        uuid::Uuid::now_v7().simple()
    ));
    write_private_json(&temporary_path, envelope)?;
    if let Err(error) = fs::rename(&temporary_path, &final_path) {
        let _ = fs::remove_file(&temporary_path);
        if !final_path.is_file() {
            return Err(error.into());
        }
    }
    Ok(final_path)
}

fn runtime_task_native_session_listener(descriptor: &WorkerDescriptor) -> NativeSessionListener {
    let descriptor = descriptor.clone();
    Arc::new(move |native_session_id| {
        let request = AttemptEventRequest {
            event_id: format!(
                "{}:native-session:{}",
                descriptor.attempt_id,
                &sha256(&native_session_id)[..16]
            ),
            callback_token: descriptor.callback_token.clone(),
            lease_epoch: descriptor.lease_epoch,
            kind: "native_session_started".to_string(),
            payload: serde_json::json!({ "nativeSessionId": native_session_id }),
        };
        let envelope = AttemptEventSpoolEnvelope {
            attempt_id: descriptor.attempt_id.clone(),
            request: request.clone(),
        };
        let Ok(spool_path) = write_attempt_event_spool(&descriptor.data_dir, &envelope) else {
            return;
        };
        let delivery_descriptor = descriptor.clone();
        let delivery_data_dir = descriptor.data_dir.clone();
        let delivered = std::thread::Builder::new()
            .name("pwcli-attempt-event".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let result = runtime.block_on(post_attempt_event(&delivery_descriptor, &request));
                if result.is_err() {
                    runtime.block_on(notify_spool_listener(&delivery_data_dir));
                }
                result
            })
            .and_then(|thread| {
                thread
                    .join()
                    .map_err(|_| std::io::Error::other("attempt event callback thread panicked"))
            })
            .is_ok_and(|result| result.is_ok());
        if delivered {
            let _ = fs::remove_file(spool_path);
        }
    })
}

async fn notify_spool_listener(data_dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(socket) = tokio::net::UnixDatagram::unbound() {
            let _ = socket
                .send_to(
                    b"callback-ready",
                    data_dir.join("task-callback-spool.notify.sock"),
                )
                .await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = data_dir;
    }
}

fn task_token_usage(usage: CodeAgentUsage) -> Value {
    serde_json::json!({
        "inputTokens": usage.input_tokens,
        "outputTokens": usage.output_tokens,
        "cacheTokens": usage.cache_tokens,
        "totalTokens": usage.input_tokens.saturating_add(usage.output_tokens),
    })
}

const TASK_OUTCOME_ENVELOPE_START: &str = "<!-- PWCLI_TASK_OUTCOME_V1";
const TASK_OUTCOME_ENVELOPE_END: &str = "-->";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskOutcomeEnvelope {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    suggested_task_update: Option<SuggestedTaskUpdate>,
    #[serde(default)]
    decision_candidates: Vec<DecisionCandidate>,
}

struct NormalizedTaskOutput {
    summary: String,
    document_body: String,
    suggested_task_update: Option<SuggestedTaskUpdate>,
    decision_candidates: Vec<DecisionCandidate>,
}

fn task_outcome_contract() -> &'static str {
    r#"

## Workbench 结构化产出协议

最终回复必须先输出一个 Markdown 注释元数据块，紧接完整 Markdown 文档：

<!-- PWCLI_TASK_OUTCOME_V1
{"summary":"不超过 200 字的结果摘要","suggestedTaskUpdate":null,"decisionCandidates":[]}
-->

`suggestedTaskUpdate` 可包含 `todoId`、0-100 的 `progress`、`status`、`completedSubtaskIds`、`markComplete`；没有建议时必须为 null。`decisionCandidates` 每项可包含 `id`、`title`、`summary`，且必须包含非空 `content`；没有长期决策候选时必须为空数组。注释块之后只输出可直接阅读的 Markdown 正文，不要使用代码围栏包裹元数据块。
"#
}

fn normalize_task_execution_output(output: &str) -> NormalizedTaskOutput {
    let fallback = || NormalizedTaskOutput {
        summary: output.trim().chars().take(2_000).collect(),
        document_body: output.trim().to_string(),
        suggested_task_update: None,
        decision_candidates: Vec::new(),
    };
    let Some(start) = output.find(TASK_OUTCOME_ENVELOPE_START) else {
        return fallback();
    };
    let metadata_start = start + TASK_OUTCOME_ENVELOPE_START.len();
    let Some(relative_end) = output[metadata_start..].find(TASK_OUTCOME_ENVELOPE_END) else {
        return fallback();
    };
    let end = metadata_start + relative_end;
    let Ok(envelope) =
        serde_json::from_str::<TaskOutcomeEnvelope>(output[metadata_start..end].trim())
    else {
        return fallback();
    };
    if envelope
        .suggested_task_update
        .as_ref()
        .and_then(|update| update.progress)
        .is_some_and(|progress| progress > 100)
        || envelope
            .decision_candidates
            .iter()
            .any(|candidate| candidate.content.trim().is_empty())
    {
        return fallback();
    }
    let before = output[..start].trim();
    let after = output[end + TASK_OUTCOME_ENVELOPE_END.len()..].trim();
    let document_body = [before, after]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let summary = envelope
        .summary
        .map(|value| value.trim().chars().take(2_000).collect::<String>())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| document_body.chars().take(2_000).collect());
    NormalizedTaskOutput {
        summary,
        document_body,
        suggested_task_update: envelope.suggested_task_update,
        decision_candidates: envelope.decision_candidates,
    }
}

fn internal_agent_prompt(role: DelegatedRole, access: DelegatedAccess, objective: &str) -> String {
    let role_instruction = match role {
        DelegatedRole::Researcher => "以只读调研员身份收集证据并形成可核验结论。",
        DelegatedRole::Engineer => "以工程师身份完成实现并验证关键行为。",
        DelegatedRole::Reviewer => "以审阅员身份识别缺陷、风险和可执行改进。",
        DelegatedRole::Analyst => "以分析师身份给出结构化分析、证据和建议。",
        DelegatedRole::Operator => "以执行员身份可靠完成明确操作并记录结果。",
        DelegatedRole::General => "以通用协作者身份完整处理目标。",
    };
    let access_instruction = match access {
        DelegatedAccess::ReadOnly => "本任务为只读：不得修改、创建或删除项目文件。",
        DelegatedAccess::Mutating => {
            "本任务允许修改当前隔离工作目录中的文件；不要提交 Git commit。"
        }
    };
    format!(
        "{role_instruction}\n{access_instruction}\n\n任务目标：\n{objective}\n\n请在最终回复中给出完整 Markdown 产物，包括结论、验证、风险和后续建议。"
    )
}

fn load_worker_continuations(
    data_dir: &Path,
    references: &[continuation::ContinuationArtifactRef],
) -> Result<String> {
    if references.is_empty() {
        return Ok(String::new());
    }
    let mut context = String::from(
        "\n\n<private_runtime_continuations>\n以下内容从 daemon 持久化的 continuation artifact 读取，是恢复父任务的完整结构化事实源。不要把标签本身展示给用户；如需按段复核，可使用 read_artifact 和对应 artifact_id。\n",
    );
    for reference in references {
        let artifact = continuation::load(data_dir, reference)?;
        context.push_str(&format!(
            "\n<continuation artifact_id=\"{}\" sha256=\"{}\">\n{}\n</continuation>\n",
            reference.artifact_id,
            reference.sha256,
            serde_json::to_string(&artifact)?
        ));
    }
    context.push_str("</private_runtime_continuations>");
    Ok(context)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedTextFile {
    pub path: String,
    pub patch: String,
}

fn snapshot_text_workspace(root: &Path) -> BTreeMap<String, String> {
    const MAX_FILE_BYTES: u64 = 512 * 1024;
    const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
    let mut snapshot = BTreeMap::new();
    let mut total = 0_usize;
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some(".git" | "node_modules" | "target" | "dist" | ".next")
            )
        })
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES || total >= MAX_TOTAL_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        total = total.saturating_add(content.len());
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        snapshot.insert(relative.to_string_lossy().replace('\\', "/"), content);
    }
    snapshot
}

fn changed_text_files(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<ChangedTextFile> {
    let mut paths = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let old = before.get(&path).map(String::as_str).unwrap_or("");
            let new = after.get(&path).map(String::as_str).unwrap_or("");
            if old == new {
                return None;
            }
            let patch = similar::TextDiff::from_lines(old, new)
                .unified_diff()
                .header(&format!("a/{path}"), &format!("b/{path}"))
                .to_string();
            Some(ChangedTextFile { path, patch })
        })
        .take(100)
        .collect()
}

fn complete_review_file_list(
    previews: Vec<ChangedTextFile>,
    lease: Option<&worktree::WorktreeLease>,
    artifact: Option<&worktree::ReviewArtifact>,
) -> Vec<ChangedTextFile> {
    let (Some(lease), Some(artifact)) = (lease, artifact) else {
        return previews;
    };
    let mut previews = previews
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    artifact
        .affected_paths
        .iter()
        .map(|path| {
            let path = path
                .strip_prefix(&lease.bound_relative)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            previews.remove(&path).unwrap_or(ChangedTextFile {
                path,
                patch: String::new(),
            })
        })
        .collect()
}

async fn post_completion(
    descriptor: &WorkerDescriptor,
    request: &CompleteAttemptRequest,
) -> Result<()> {
    let client = reqwest::Client::new();
    let delays = [0, 250, 1_000, 2_000, 5_000];
    let mut last_error = None;
    for delay in delays {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        match client
            .post(format!(
                "{}/internal/task-attempts/{}/complete",
                descriptor.callback_base, descriptor.attempt_id
            ))
            .json(request)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => {
                last_error = Some(anyhow::anyhow!("callback returned {}", response.status()))
            }
            Err(error) => last_error = Some(error.into()),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("task callback failed")))
}

async fn post_attempt_event(
    descriptor: &WorkerDescriptor,
    request: &AttemptEventRequest,
) -> Result<()> {
    let client = reqwest::Client::new();
    let delays = [0, 250, 1_000, 2_000];
    let mut last_error = None;
    for delay in delays {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        match client
            .post(format!(
                "{}/internal/task-attempts/{}/events",
                descriptor.callback_base, descriptor.attempt_id
            ))
            .json(request)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => {
                last_error = Some(anyhow::anyhow!(
                    "attempt event callback returned {}",
                    response.status()
                ))
            }
            Err(error) => last_error = Some(error.into()),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("attempt event callback failed")))
}

fn backend_for(name: Option<&str>) -> Result<Arc<dyn SubAgentBackend>> {
    let name = name
        .map(str::to_string)
        .or_else(|| {
            crate::tools::code_agent::enabled_backend_names()
                .into_iter()
                .next()
        })
        .context("no code agent backend is enabled")?;
    match name.as_str() {
        "codex" => crate::tools::code_agent::detect_codex_acp_binary()
            .map(|(binary, _)| Arc::new(CodexBackend::new(binary)) as Arc<dyn SubAgentBackend>)
            .context("Codex ACP adapter is unavailable"),
        "qoder" => crate::tools::code_agent::detect_qoder_binary()
            .map(|(binary, _)| Arc::new(QoderBackend::new(binary)) as Arc<dyn SubAgentBackend>)
            .context("QoderCLI is unavailable"),
        "kimi" => crate::tools::code_agent::detect_kimi_binary()
            .map(|(binary, _)| Arc::new(KimiBackend::new(binary)) as Arc<dyn SubAgentBackend>)
            .context("Kimi CLI is unavailable"),
        _ => anyhow::bail!("unsupported task worker backend: {name}"),
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeTaskRecord> {
    let result_json: Option<String> = row.get(14)?;
    let metadata = row
        .get::<_, String>(16)
        .ok()
        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .unwrap_or(Value::Null);
    let spec = serde_json::from_value::<RuntimeTaskSpec>(metadata.clone()).ok();
    let legacy_mode = spec.as_ref().and_then(|spec| spec.mode.as_deref());
    let role = spec
        .as_ref()
        .map(|spec| spec.role.as_str())
        .unwrap_or("general")
        .to_string();
    let access = spec
        .as_ref()
        .map(|spec| spec.access.as_str())
        .unwrap_or_else(|| {
            if legacy_mode == Some("edit") {
                "mutating"
            } else {
                "read_only"
            }
        })
        .to_string();
    let executor_request = spec
        .as_ref()
        .map(|spec| spec.executor.as_str().to_string())
        .or_else(|| {
            metadata
                .get("backend")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "auto".to_string());
    let review_status = metadata
        .get("reviewStatus")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if access == "mutating" {
                "pending"
            } else {
                "not_required"
            }
        })
        .to_string();
    Ok(RuntimeTaskRecord {
        id: row.get(0)?,
        batch_id: row.get(1)?,
        root_session_id: row.get(2)?,
        parent_task_id: row.get(3)?,
        kind: row.get(4)?,
        objective: row.get(5)?,
        deliverable_title: spec
            .as_ref()
            .and_then(|spec| spec.deliverable_title.clone()),
        cwd: row.get(6)?,
        backend: row.get(7)?,
        model: row.get(8)?,
        role,
        access,
        executor_request,
        resolved_executor_kind: spec
            .as_ref()
            .and_then(|spec| spec.resolved_executor_kind.clone()),
        resolved_executor_id: spec
            .as_ref()
            .and_then(|spec| spec.resolved_executor_id.clone()),
        resolved_provider_id: spec
            .as_ref()
            .and_then(|spec| spec.resolved_provider_id.clone()),
        resolved_model: spec.as_ref().and_then(|spec| spec.resolved_model.clone()),
        resolved_effort: spec.as_ref().and_then(|spec| spec.resolved_effort.clone()),
        resolved_permission_mode: spec
            .as_ref()
            .and_then(|spec| spec.resolved_permission_mode.clone()),
        routing_reason: spec.as_ref().and_then(|spec| spec.routing_reason.clone()),
        display_name: spec.as_ref().and_then(|spec| spec.display_name.clone()),
        role_label: spec.as_ref().and_then(|spec| spec.role_label.clone()),
        avatar_seed: spec.as_ref().and_then(|spec| spec.avatar_seed.clone()),
        work_item_id: metadata
            .get("workItemId")
            .and_then(Value::as_str)
            .map(str::to_string),
        session_generation: metadata.get("sessionGeneration").and_then(Value::as_u64),
        review_revision: metadata
            .get("reviewRevision")
            .and_then(Value::as_u64)
            .unwrap_or_default() as u32,
        depth: spec.as_ref().map(|spec| spec.depth).unwrap_or_default(),
        primary_document_id: metadata
            .get("primaryDocumentId")
            .and_then(Value::as_str)
            .map(str::to_string),
        output_status: metadata
            .get("outputStatus")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string(),
        review_status,
        status: row.get(9)?,
        delivery_status: row.get(10)?,
        attempt: row.get(11)?,
        created_at: parse_time(row.get::<_, String>(12)?)?,
        updated_at: parse_time(row.get::<_, String>(13)?)?,
        result: result_json.and_then(|value| serde_json::from_str(&value).ok()),
        error: row.get(15)?,
        failure: {
            let status: String = row.get(9)?;
            let attempt: u32 = row.get(11)?;
            let error: Option<String> = row.get(15)?;
            derive_failure_from_row(&status, error.as_deref(), attempt, &metadata)
        },
        metadata,
    })
}

fn serialize_spec_preserving_runtime(spec: &RuntimeTaskSpec, previous: &str) -> Result<String> {
    let mut value = serde_json::to_value(spec)?;
    let previous = serde_json::from_str::<Value>(previous).unwrap_or(Value::Null);
    if let (Value::Object(current), Value::Object(previous)) = (&mut value, previous) {
        for key in [
            "outputStatus",
            "reviewStatus",
            "primaryDocumentId",
            "reviewRevision",
            "workItemId",
            "sessionGeneration",
            "failure",
            "mode",
            "toolName",
            "toolArguments",
            "replayable",
            "backgroundTaskId",
        ] {
            if let Some(existing) = previous.get(key) {
                current.insert(key.to_string(), existing.clone());
            }
        }
    }
    Ok(value.to_string())
}

fn ensure_legacy_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement =
        connection.prepare(&format!("PRAGMA legacy_supervisor.table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|candidate| candidate == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE legacy_supervisor.{table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|candidate| candidate == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

fn archive_legacy_supervisor(source: &Path) {
    if !source.is_file() {
        return;
    }
    let archived = source.with_file_name("supervisor.db.migrated-v1");
    if archived.exists() {
        tracing::warn!(
            source = %source.display(),
            archived = %archived.display(),
            "legacy Supervisor database was migrated but could not be archived because the target exists"
        );
        return;
    }
    if let Err(error) = fs::rename(source, &archived) {
        tracing::warn!(
            %error,
            source = %source.display(),
            archived = %archived.display(),
            "legacy Supervisor database was migrated but could not be archived"
        );
    }
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskEvent> {
    let payload: String = row.get(5)?;
    Ok(TaskEvent {
        sequence: row.get(0)?,
        event_id: row.get(1)?,
        task_id: row.get(2)?,
        attempt_id: row.get(3)?,
        kind: row.get(4)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        created_at: parse_time(row.get::<_, String>(6)?)?,
    })
}

fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                value.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn insert_event_tx(
    transaction: &rusqlite::Transaction<'_>,
    event_id: &str,
    task_id: &str,
    attempt_id: Option<&str>,
    kind: &str,
    payload: Value,
) -> Result<()> {
    transaction.execute(
        "INSERT OR IGNORE INTO task_events
         (event_id, task_id, attempt_id, kind, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event_id,
            task_id,
            attempt_id,
            kind,
            payload.to_string(),
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

fn task_ids_for_batch(connection: &Connection, batch_id: &str) -> Result<Vec<String>> {
    let mut statement =
        connection.prepare("SELECT id FROM runtime_tasks WHERE batch_id=?1 ORDER BY created_at")?;
    let task_ids = statement
        .query_map([batch_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(task_ids)
}

fn batch_summary(transaction: &rusqlite::Transaction<'_>, batch_id: &str) -> Result<String> {
    let mut statement = transaction.prepare(
        "SELECT objective, status, result_json, error FROM runtime_tasks WHERE batch_id=?1 ORDER BY created_at",
    )?;
    let rows = statement
        .query_map([batch_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut summary = String::from("异步委派批次已完成：\n");
    for (index, (objective, status, result, error)) in rows.into_iter().enumerate() {
        let detail = error.or(result).unwrap_or_default();
        let bounded = detail.chars().take(4_000).collect::<String>();
        summary.push_str(&format!(
            "\n{}. [{}] {}\n{}\n",
            index + 1,
            status,
            objective,
            bounded
        ));
    }
    Ok(summary)
}

fn task_summary(transaction: &rusqlite::Transaction<'_>, task_id: &str) -> Result<String> {
    let (objective, status, result, error): (String, String, Option<String>, Option<String>) =
        transaction.query_row(
            "SELECT objective, status, result_json, error FROM runtime_tasks WHERE id=?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    let detail = error.or(result).unwrap_or_default();
    Ok(format!(
        "异步委派任务已完成：\n\n[{}] {}\n{}\n",
        status,
        objective,
        detail.chars().take(4_000).collect::<String>()
    ))
}

fn materialize_child_continuation(
    data_dir: &Path,
    connection: &Connection,
    parent_task_id: &str,
    batch_id: &str,
    callback: &Value,
) -> Result<continuation::ContinuationArtifactRef> {
    let (batch_parent, join_mode): (Option<String>, String) = connection
        .query_row(
            "SELECT parent_task_id, join_mode FROM runtime_batches WHERE id=?1",
            [batch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("child RuntimeTask batch not found")?;
    if batch_parent.as_deref() != Some(parent_task_id) {
        anyhow::bail!("child RuntimeTask batch does not belong to parent");
    }
    let task_ids = callback
        .get("taskIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|ids| !ids.is_empty())
        .unwrap_or(task_ids_for_batch(connection, batch_id)?);
    let mut outcomes = Vec::with_capacity(task_ids.len());
    for task_id in &task_ids {
        let (objective, status, raw_payload, raw_result, error): (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT objective, status, payload_json, result_json, error
                 FROM runtime_tasks WHERE id=?1 AND batch_id=?2 AND parent_task_id=?3",
                params![task_id, batch_id, parent_task_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .with_context(|| format!("child RuntimeTask {task_id} not found"))?;
        if !is_terminal(&status) {
            anyhow::bail!("child RuntimeTask {task_id} is not terminal");
        }
        let payload = serde_json::from_str::<Value>(&raw_payload)?;
        let result = raw_result
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()?;
        let document = result
            .as_ref()
            .and_then(|value| value.get("document"))
            .map(|document| continuation::ContinuationDocumentRef {
                document_id: payload
                    .get("primaryDocumentId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                title: document
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                sha256: document.get("body").and_then(Value::as_str).map(sha256),
            });
        outcomes.push(continuation::ChildTaskOutcome {
            task_id: task_id.clone(),
            objective,
            status,
            output_status: payload
                .get("outputStatus")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
            review_status: payload
                .get("reviewStatus")
                .and_then(Value::as_str)
                .unwrap_or("not_required")
                .to_string(),
            executor_id: payload
                .get("resolvedExecutorId")
                .and_then(Value::as_str)
                .map(str::to_string),
            document,
            result_hash: raw_result.as_deref().map(sha256),
            result,
            error,
        });
    }
    continuation::store(
        data_dir,
        &continuation::ContinuationArtifact {
            schema_version: 1,
            parent_task_id: parent_task_id.to_string(),
            child_batch_id: batch_id.to_string(),
            join_mode,
            task_ids,
            outcomes,
        },
    )
}

fn merge_continuation_ref(
    refs: &mut Vec<continuation::ContinuationArtifactRef>,
    reference: continuation::ContinuationArtifactRef,
) {
    if !refs
        .iter()
        .any(|existing| existing.artifact_id == reference.artifact_id)
    {
        refs.push(reference);
    }
}

fn continuation_objective_marker(
    batch_id: &str,
    summary: &str,
    reference: &continuation::ContinuationArtifactRef,
) -> String {
    let bounded_summary = summary.chars().take(1_000).collect::<String>();
    format!(
        "\n\n## 子任务批次 {batch_id} 已返回\n{bounded_summary}\n\n完整结构化结果：artifact_id={}（sha256={}）。继续父任务时以该 artifact 为事实源；需要更多上下文时使用 read_artifact 分块读取。",
        reference.artifact_id, reference.sha256
    )
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "cancelled")
}

fn has_incomplete_child_batches(connection: &Connection, parent_task_id: &str) -> Result<bool> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM runtime_batches
         WHERE parent_task_id=?1 AND status NOT IN ('completed','cancelled')",
        [parent_task_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn has_unconsumed_child_results(connection: &Connection, parent_task_id: &str) -> Result<bool> {
    let raw_payload: String = connection.query_row(
        "SELECT payload_json FROM runtime_tasks WHERE id=?1",
        [parent_task_id],
        |row| row.get(0),
    )?;
    let payload = serde_json::from_str::<Value>(&raw_payload)?;
    if ["pendingContinuationRefs", "pendingChildResults"]
        .into_iter()
        .any(|key| {
            payload
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        })
    {
        return Ok(true);
    }
    let consumed = serde_json::from_value::<RuntimeTaskSpec>(payload)?
        .continuation_refs
        .into_iter()
        .flat_map(|reference| reference.task_ids)
        .collect::<Vec<_>>();
    let mut statement = connection.prepare(
        "SELECT id FROM runtime_tasks
         WHERE parent_task_id=?1
           AND status IN ('succeeded','failed','cancelled')
           AND json_extract(payload_json, '$.outputStatus')='ready'",
    )?;
    let ready = statement
        .query_map([parent_task_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ready.iter().any(|task_id| !consumed.contains(task_id)))
}

fn capture_process_fingerprint(
    pid: u32,
    executable: &Path,
    descriptor_path: &Path,
    descriptor_sha256: String,
) -> Result<WorkerProcessFingerprint> {
    let (start_marker, executable_identity, command) = observe_process_identity(pid)?;
    let descriptor_path = descriptor_path.to_string_lossy().into_owned();
    let executable_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .context("task worker executable has no file name")?;
    if !command.contains("task-worker")
        || !command.contains("--descriptor")
        || !command.contains(&descriptor_path)
        || !command.contains(executable_name)
    {
        anyhow::bail!("spawned process command does not identify the task worker descriptor");
    }
    Ok(WorkerProcessFingerprint {
        version: 1,
        pid,
        start_marker,
        executable_identity,
        command_sha256: sha256(command),
        descriptor_path,
        descriptor_sha256,
    })
}

fn verify_process_fingerprint(pid: u32, serialized: &str) -> Result<bool> {
    let fingerprint: WorkerProcessFingerprint = serde_json::from_str(serialized)?;
    let (start_marker, executable_identity, command) = observe_process_identity(pid)?;
    Ok(process_fingerprint_matches(
        &fingerprint,
        pid,
        &start_marker,
        &executable_identity,
        &command,
    ))
}

fn process_fingerprint_matches(
    fingerprint: &WorkerProcessFingerprint,
    pid: u32,
    start_marker: &str,
    executable_identity: &str,
    command: &str,
) -> bool {
    fingerprint.version == 1
        && fingerprint.pid == pid
        && start_marker == fingerprint.start_marker
        && executable_identity == fingerprint.executable_identity
        && sha256(command) == fingerprint.command_sha256
        && command.contains("task-worker")
        && command.contains("--descriptor")
        && command.contains(&fingerprint.descriptor_path)
}

fn observe_process_identity(pid: u32) -> Result<(String, String, String)> {
    #[cfg(unix)]
    {
        fn field(pid: u32, name: &str) -> Result<String> {
            let output = Command::new("ps")
                .args(["-ww", "-p", &pid.to_string(), "-o", &format!("{name}=")])
                .output()
                .with_context(|| format!("inspect process {pid} {name}"))?;
            if !output.status.success() {
                anyhow::bail!("process {pid} is not available");
            }
            let value = String::from_utf8(output.stdout)?.trim().to_string();
            if value.is_empty() {
                anyhow::bail!("process {pid} returned an empty {name}");
            }
            Ok(value)
        }
        Ok((
            field(pid, "lstart")?,
            field(pid, "comm")?,
            field(pid, "args")?,
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        anyhow::bail!("process fingerprinting is unavailable on this platform")
    }
}

fn signal_worker_process(
    pid: Option<u32>,
    pgid: Option<u32>,
    serialized_fingerprint: Option<&str>,
) -> WorkerSignalAudit {
    let (Some(pid), Some(pgid)) = (pid, pgid) else {
        return WorkerSignalAudit {
            outcome: "not_running".to_string(),
            reason: None,
        };
    };
    let Some(serialized_fingerprint) = serialized_fingerprint else {
        return WorkerSignalAudit {
            outcome: "skipped".to_string(),
            reason: Some("missing_process_fingerprint".to_string()),
        };
    };
    match verify_process_fingerprint(pid, serialized_fingerprint) {
        Ok(true) => {}
        Ok(false) => {
            return WorkerSignalAudit {
                outcome: "skipped".to_string(),
                reason: Some("process_fingerprint_mismatch".to_string()),
            };
        }
        Err(error) => {
            return WorkerSignalAudit {
                outcome: "skipped".to_string(),
                reason: Some(format!("process_fingerprint_unverifiable: {error}")),
            };
        }
    }
    #[cfg(unix)]
    {
        match Command::new("kill")
            .args(["-TERM", &format!("-{pgid}")])
            .status()
        {
            Ok(status) if status.success() => WorkerSignalAudit {
                outcome: "sent".to_string(),
                reason: None,
            },
            Ok(status) => WorkerSignalAudit {
                outcome: "failed".to_string(),
                reason: Some(format!("kill exited with {status}")),
            },
            Err(error) => WorkerSignalAudit {
                outcome: "failed".to_string(),
                reason: Some(error.to_string()),
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        WorkerSignalAudit {
            outcome: "skipped".to_string(),
            reason: Some("process_group_signals_are_unavailable".to_string()),
        }
    }
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn sha256(value: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(value.as_ref()))
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_upgrades_legacy_runtime_batches_before_creating_indexes() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("runtime-tasks.db");
        let connection = Connection::open(&database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_batches (
                   id TEXT PRIMARY KEY,
                   root_session_id TEXT NOT NULL,
                   join_mode TEXT NOT NULL,
                   status TEXT NOT NULL,
                   idempotency_key TEXT NOT NULL UNIQUE,
                   created_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL
                 );
                 INSERT INTO runtime_batches
                   (id, root_session_id, join_mode, status, idempotency_key, created_at, updated_at)
                 VALUES
                   ('legacy-batch', 'legacy-session', 'all', 'completed', 'legacy-key',
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        drop(connection);

        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let connection = broker.connection().unwrap();
        let columns = connection
            .prepare("PRAGMA table_info(runtime_batches)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "work_item_id"));
        assert!(columns.iter().any(|column| column == "parent_task_id"));
        assert!(columns.iter().any(|column| column == "session_generation"));
        assert_eq!(
            connection
                .query_row(
                    "SELECT root_session_id FROM runtime_batches WHERE id='legacy-batch'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "legacy-session"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='index' AND name='runtime_batches_work_item_idx'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .unwrap(),
            1
        );
        drop(connection);
        drop(broker);
        TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
    }

    fn insert_active_attempt(
        broker: &TaskBroker,
        task_id: &str,
        attempt_id: &str,
        status: &str,
        cwd: &Path,
    ) {
        let now = Utc::now().to_rfc3339();
        let batch_id = format!("batch-{task_id}");
        let spec = RuntimeTaskSpec::delegated("test worker", cwd.to_string_lossy());
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "INSERT INTO runtime_batches
                 (id, root_session_id, join_mode, status, idempotency_key, created_at, updated_at)
                 VALUES (?1, 'session-test', 'all', 'running', ?2, ?3, ?3)",
                params![batch_id, format!("key-{task_id}"), now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_tasks
                 (id, batch_id, root_session_id, kind, objective, cwd, payload_json, status,
                  delivery_status, attempt, created_at, updated_at)
                 VALUES (?1, ?2, 'session-test', 'delegated', 'test worker', ?3, ?4, ?5,
                         'not_ready', 1, ?6, ?6)",
                params![
                    task_id,
                    batch_id,
                    cwd.to_string_lossy(),
                    serde_json::to_string(&spec).unwrap(),
                    status,
                    now,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_attempts
                 (id, task_id, attempt_no, lease_epoch, token_hash, status, lease_expires_at)
                 VALUES (?1, ?2, 1, 1, 'hash', ?3, ?4)",
                params![
                    attempt_id,
                    task_id,
                    status,
                    (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(),
                ],
            )
            .unwrap();
    }

    #[test]
    fn runtime_task_can_be_resolved_by_work_item() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let accepted = broker
            .submit_batch(SubmitTaskBatch {
                root_session_id: "session-capture".to_string(),
                parent_task_id: None,
                work_item_id: Some("todo-capture".to_string()),
                session_generation: Some(1),
                idempotency_key: "capture-runtime-task".to_string(),
                join: "all".to_string(),
                tasks: vec![RuntimeTaskSpec::delegated(
                    "durable capture",
                    workspace.path().to_string_lossy(),
                )],
            })
            .unwrap();

        assert_eq!(
            broker
                .task_for_work_item("todo-capture")
                .unwrap()
                .unwrap()
                .id,
            accepted.task_ids[0]
        );
        assert!(broker.task_for_work_item("todo-missing").unwrap().is_none());
    }

    #[test]
    fn prelaunch_failure_leaves_retryable_terminal_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let missing_cwd = workspace.path().join("missing");
        insert_active_attempt(
            &broker,
            "task-start-failure",
            "attempt-start-failure",
            "leased",
            &missing_cwd,
        );
        let descriptor = WorkerDescriptor {
            task_id: "task-start-failure".to_string(),
            attempt_id: "attempt-start-failure".to_string(),
            lease_epoch: 1,
            callback_token: "token".to_string(),
            callback_base: "http://127.0.0.1:9".to_string(),
            data_dir: directory.path().to_path_buf(),
            cwd: missing_cwd.to_string_lossy().into_owned(),
            deliverable_title: "test".to_string(),
            execution: ResolvedExecution::PwcliAgent {
                objective: "test".to_string(),
                role: DelegatedRole::General,
                access: DelegatedAccess::ReadOnly,
                provider_id: None,
                model: None,
                effort: None,
                thinking: false,
            },
            continuation_refs: Vec::new(),
            review_lease: None,
        };

        let failure = broker.spawn_worker(&descriptor).unwrap_err();
        assert!(!failure.side_effects_possible);
        broker
            .record_worker_start_failure(&descriptor, &failure)
            .unwrap();

        let connection = broker.connection().unwrap();
        let task_status: String = connection
            .query_row(
                "SELECT status FROM runtime_tasks WHERE id='task-start-failure'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let attempt_status: String = connection
            .query_row(
                "SELECT status FROM task_attempts WHERE id='attempt-start-failure'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(task_status, "waiting_configuration");
        assert_eq!(attempt_status, "start_failed");
        assert!(broker
            .events_after(0)
            .unwrap()
            .iter()
            .any(|event| event.kind == "task_start_failed"));
    }

    #[test]
    fn startup_recovery_handles_leased_and_starting_attempts() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "task-leased",
            "attempt-leased",
            "leased",
            workspace.path(),
        );
        insert_active_attempt(
            &broker,
            "task-starting",
            "attempt-starting",
            "starting",
            workspace.path(),
        );

        broker.recover_orphaned_attempts().unwrap();

        let connection = broker.connection().unwrap();
        let status = |table: &str, id: &str| -> String {
            connection
                .query_row(
                    &format!("SELECT status FROM {table} WHERE id=?1"),
                    [id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(
            status("runtime_tasks", "task-leased"),
            "waiting_configuration"
        );
        assert_eq!(status("task_attempts", "attempt-leased"), "start_failed");
        assert_eq!(
            status("runtime_tasks", "task-starting"),
            "recovery_required"
        );
        assert_eq!(status("task_attempts", "attempt-starting"), "orphaned");
    }

    #[test]
    fn startup_recovery_does_not_trust_legacy_live_pid_without_fingerprint() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "legacy-live-pid-task",
            "legacy-live-pid-attempt",
            "running",
            workspace.path(),
        );
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE task_attempts SET worker_pid=?2, worker_pgid=?2 WHERE id=?1",
                params!["legacy-live-pid-attempt", std::process::id()],
            )
            .unwrap();

        broker.recover_orphaned_attempts().unwrap();
        assert_eq!(
            broker.task("legacy-live-pid-task").unwrap().unwrap().status,
            "recovery_required"
        );
        let event = broker
            .events_after(0)
            .unwrap()
            .into_iter()
            .find(|event| event.kind == "task_recovery_required")
            .unwrap();
        assert_eq!(
            event.payload["processIdentity"],
            "missing_process_fingerprint"
        );
    }

    #[test]
    fn task_execution_outcome_uses_frontend_token_usage_fields() {
        let outcome = TaskExecutionOutcome {
            summary: "done".to_string(),
            document: TaskDocumentOutcome {
                title: "Result".to_string(),
                format: "markdown".to_string(),
                body: "done".to_string(),
            },
            changed_files: Vec::new(),
            verification: Vec::new(),
            suggested_task_update: None,
            decision_candidates: Vec::new(),
            duration_ms: Some(5),
            token_usage: Some(task_token_usage(CodeAgentUsage {
                input_tokens: 10,
                output_tokens: 4,
                cache_tokens: 3,
            })),
            review_artifact: None,
            native_session_id: None,
        };

        let value = serde_json::to_value(outcome).unwrap();
        assert!(value.get("token_usage").is_none());
        assert_eq!(value["tokenUsage"]["inputTokens"], 10);
        assert_eq!(value["tokenUsage"]["outputTokens"], 4);
        assert_eq!(value["tokenUsage"]["cacheTokens"], 3);
        assert_eq!(value["tokenUsage"]["totalTokens"], 14);
    }

    #[test]
    fn internal_executor_inherits_publisher_model_snapshot_but_cli_does_not() {
        let snapshot = DelegatedModelSnapshot {
            provider_id: Some("root-provider".to_string()),
            model: "root-model".to_string(),
            effort: Some("xhigh".to_string()),
            thinking: true,
        };
        let mut internal = RuntimeTaskSpec::delegated("internal", ".");
        internal.resolved_executor_kind = Some("internal_agent".to_string());
        internal.resolved_model = Some("task-model".to_string());
        internal.resolved_effort = Some("low".to_string());
        internal.publisher_model_context = Some(snapshot.clone());
        inherit_publisher_model_snapshot(&mut internal);
        assert_eq!(
            internal.resolved_provider_id.as_deref(),
            Some("root-provider")
        );
        assert_eq!(internal.resolved_model.as_deref(), Some("root-model"));
        assert_eq!(internal.resolved_effort.as_deref(), Some("xhigh"));
        assert!(internal.resolved_thinking);

        let mut cli = RuntimeTaskSpec::delegated("cli", ".");
        cli.resolved_executor_kind = Some("acp_cli".to_string());
        cli.resolved_model = Some("cli-model".to_string());
        cli.resolved_effort = Some("medium".to_string());
        cli.publisher_model_context = Some(snapshot);
        inherit_publisher_model_snapshot(&mut cli);
        assert_eq!(cli.resolved_provider_id, None);
        assert_eq!(cli.resolved_model.as_deref(), Some("cli-model"));
        assert_eq!(cli.resolved_effort.as_deref(), Some("medium"));
        assert!(!cli.resolved_thinking);
    }

    #[test]
    fn structured_task_output_extracts_suggestions_and_keeps_markdown_body() {
        let normalized = normalize_task_execution_output(
            r#"<!-- PWCLI_TASK_OUTCOME_V1
{"summary":"完成核心改造","suggestedTaskUpdate":{"progress":80,"completedSubtaskIds":["sub-1"]},"decisionCandidates":[{"id":"decision-1","title":"统一协议","content":"后续统一使用 RuntimeTask"}]}
-->
# 交付报告

验证通过。"#,
        );
        assert_eq!(normalized.summary, "完成核心改造");
        assert_eq!(normalized.document_body, "# 交付报告\n\n验证通过。");
        let update = normalized.suggested_task_update.unwrap();
        assert_eq!(update.progress, Some(80));
        assert_eq!(update.completed_subtask_ids, ["sub-1"]);
        assert_eq!(normalized.decision_candidates.len(), 1);
        assert_eq!(
            normalized.decision_candidates[0].content,
            "后续统一使用 RuntimeTask"
        );
    }

    #[test]
    fn legacy_task_outcome_defaults_suggestions_to_none_and_empty() {
        let outcome: TaskExecutionOutcome = serde_json::from_value(serde_json::json!({
            "summary": "legacy",
            "document": { "title": "Legacy", "format": "markdown", "body": "body" },
            "changedFiles": [],
            "verification": []
        }))
        .unwrap();
        assert_eq!(outcome.suggested_task_update, None);
        assert!(outcome.decision_candidates.is_empty());
        let value = serde_json::to_value(outcome).unwrap();
        assert!(value["suggestedTaskUpdate"].is_null());
        assert_eq!(value["decisionCandidates"], serde_json::json!([]));
    }

    #[test]
    fn review_projection_requires_an_applied_patch() {
        let projection = serde_json::json!({
            "taskUpdate": { "todoId": "todo-1", "markComplete": true },
            "memoryEntries": [{ "content": "remember" }],
        });

        assert!(should_project_review("applied", &projection));
        assert!(!should_project_review("merge_required", &projection));
        assert!(!should_project_review("rejected", &projection));
    }

    #[test]
    fn read_only_confirmation_is_idempotent_and_projects_updates() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let now = Utc::now().to_rfc3339();
        let spec = RuntimeTaskSpec::delegated("research", workspace.path().to_string_lossy());
        let mut payload = serde_json::to_value(spec).unwrap();
        payload["outputStatus"] = Value::String("ready".to_string());
        payload["reviewStatus"] = Value::String("not_required".to_string());
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "INSERT INTO runtime_batches
                 (id, root_session_id, join_mode, status, idempotency_key, created_at, updated_at)
                 VALUES ('confirm-batch', 'confirm-session', 'all', 'completed', 'confirm-key', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_tasks
                 (id, batch_id, root_session_id, kind, objective, cwd, payload_json, status,
                  delivery_status, attempt, created_at, updated_at)
                 VALUES ('confirm-task', 'confirm-batch', 'confirm-session', 'delegated',
                         'research', ?1, ?2, 'succeeded', 'review_ready', 1, ?3, ?3)",
                params![workspace.path().to_string_lossy(), payload.to_string(), now],
            )
            .unwrap();
        drop(connection);
        let request = TaskReviewRequest {
            client_action_id: "confirm-action".to_string(),
            review_revision: 0,
            action: TaskReviewAction::Confirm,
            note: None,
            task_update: Some(serde_json::json!({ "todoId": "todo-1" })),
            memory_entries: Vec::new(),
        };
        let first = broker.review("confirm-task", request.clone()).unwrap();
        let replay = broker.review("confirm-task", request).unwrap();
        assert_eq!(first.review_status, "applied");
        assert_eq!(first.review_revision, replay.review_revision);
        let projection_count: i64 = broker
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM task_outbox WHERE destination='review_projection'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projection_count, 1);
    }

    #[test]
    fn prepared_apply_intent_replays_after_patch_side_effect() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let data = root.path().join("data");
        fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        fs::write(repo.join("a.txt"), "base\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-qm", "initial"]);

        let broker = TaskBroker::new_with_workspace_root(
            &data,
            "http://127.0.0.1:9",
            repo.canonicalize().unwrap(),
        )
        .unwrap();
        let lease = worktree::prepare_mutating_task(&data, "apply-task", &repo).unwrap();
        fs::write(lease.worker_cwd.join("a.txt"), "worker\n").unwrap();
        let artifact = worktree::finalize_attempt(&lease, "apply-attempt").unwrap();
        let mut spec = RuntimeTaskSpec::delegated("edit", repo.to_string_lossy());
        spec.access = DelegatedAccess::Mutating;
        spec.worktree_lease = Some(lease.clone());
        let mut payload = serde_json::to_value(&spec).unwrap();
        payload["outputStatus"] = Value::String("ready".to_string());
        payload["reviewStatus"] = Value::String("pending".to_string());
        let outcome = TaskExecutionOutcome {
            summary: "done".to_string(),
            document: TaskDocumentOutcome {
                title: "edit".to_string(),
                format: "markdown".to_string(),
                body: "done".to_string(),
            },
            changed_files: Vec::new(),
            verification: Vec::new(),
            suggested_task_update: None,
            decision_candidates: Vec::new(),
            duration_ms: None,
            token_usage: None,
            review_artifact: Some(artifact.clone()),
            native_session_id: None,
        };
        let now = Utc::now().to_rfc3339();
        let request = TaskReviewRequest {
            client_action_id: "apply-action".to_string(),
            review_revision: 0,
            action: TaskReviewAction::Apply,
            note: None,
            task_update: None,
            memory_entries: Vec::new(),
        };
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "INSERT INTO runtime_batches
                 (id, root_session_id, join_mode, status, idempotency_key, created_at, updated_at)
                 VALUES ('apply-batch', 'apply-session', 'all', 'completed', 'apply-key', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_tasks
                 (id, batch_id, root_session_id, kind, objective, cwd, payload_json, status,
                  delivery_status, attempt, result_json, created_at, updated_at)
                 VALUES ('apply-task', 'apply-batch', 'apply-session', 'delegated', 'edit', ?1,
                         ?2, 'succeeded', 'review_ready', 1, ?3, ?4, ?4)",
                params![
                    repo.to_string_lossy(),
                    payload.to_string(),
                    serde_json::to_string(&outcome).unwrap(),
                    now,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_review_apply_intents
                 (client_action_id, task_id, review_revision, artifact_sha256, state,
                  request_json, created_at, updated_at)
                 VALUES ('apply-action', 'apply-task', 0, ?1, 'prepared', ?2, ?3, ?3)",
                params![
                    artifact.patch_sha256,
                    serde_json::to_string(&request).unwrap(),
                    now,
                ],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            worktree::verify_and_apply(&lease, &artifact).unwrap(),
            worktree::ApplyOutcome::Applied { .. }
        ));
        let response = broker.review("apply-task", request.clone()).unwrap();
        let replay = broker.review("apply-task", request).unwrap();
        assert_eq!(response.review_status, "applied");
        assert_eq!(response.review_revision, replay.review_revision);
        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), "worker\n");
        let task = broker.task("apply-task").unwrap().unwrap();
        let spec = serde_json::from_value::<RuntimeTaskSpec>(task.metadata.clone()).unwrap();
        assert!(spec.worktree_lease.is_none());
        assert!(task.metadata.get("archivedReviewLease").is_some());
        let intent_count: i64 = broker
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM task_review_apply_intents",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(intent_count, 0);
    }

    #[test]
    fn cancelling_mutating_task_clears_pending_review() {
        let mut payload = serde_json::json!({ "reviewStatus": "pending" });
        mark_cancelled_review(&mut payload, DelegatedAccess::Mutating);
        assert_eq!(payload["reviewStatus"], "rejected");
    }

    #[test]
    fn batch_submit_is_idempotent_and_persistent() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let request = SubmitTaskBatch {
            root_session_id: "session-1".into(),
            parent_task_id: None,
            work_item_id: None,
            session_generation: Some(1),
            idempotency_key: "same-request".into(),
            join: "all".into(),
            tasks: vec![RuntimeTaskSpec {
                objective: "inspect".into(),
                deliverable_title: None,
                cwd: workspace.path().to_string_lossy().into_owned(),
                role: DelegatedRole::Researcher,
                access: DelegatedAccess::ReadOnly,
                executor: DelegatedExecutor::Auto,
                kind: "child_cli".into(),
                backend: Some("missing".into()),
                mode: Some("research".into()),
                model: None,
                effort: None,
                permission_mode: None,
                parent_task_id: None,
                depth: 0,
                resume_session_id: None,
                resolved_executor_kind: None,
                resolved_executor_id: None,
                resolved_provider_id: None,
                resolved_model: None,
                resolved_effort: None,
                resolved_thinking: false,
                resolved_permission_mode: None,
                routing_reason: None,
                display_name: None,
                role_label: None,
                avatar_seed: None,
                worktree_lease: None,
                continuation_refs: Vec::new(),
                publisher_model_context: None,
            }],
        };
        let first = broker.submit_batch(request.clone()).unwrap();
        let second = broker.submit_batch(request).unwrap();
        assert_eq!(first.batch_id, second.batch_id);
        assert_eq!(first.task_ids, second.task_ids);
        assert_eq!(broker.list().unwrap().len(), 1);
    }

    #[test]
    fn completion_materializes_document_before_batch_callback() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let mut spec = RuntimeTaskSpec::delegated(
            "write a durable report",
            workspace.path().to_string_lossy(),
        );
        spec.role = DelegatedRole::Researcher;
        spec.executor = DelegatedExecutor::Pwcli;
        spec.deliverable_title = Some("Durable report".to_string());
        let accepted = broker
            .submit_batch(SubmitTaskBatch {
                root_session_id: "session-doc".to_string(),
                parent_task_id: None,
                work_item_id: Some("todo-doc".to_string()),
                session_generation: Some(7),
                idempotency_key: "document-first".to_string(),
                join: "all".to_string(),
                tasks: vec![spec],
            })
            .unwrap();
        let task_id = &accepted.task_ids[0];
        let attempt_id = "attempt-document";
        let token = "private-callback-token";
        let now = Utc::now().to_rfc3339();
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "UPDATE runtime_tasks SET status='running', attempt=1 WHERE id=?1",
                [task_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_attempts
                 (id, task_id, attempt_no, lease_epoch, token_hash, status, lease_expires_at)
                 VALUES (?1, ?2, 1, 1, ?3, 'running', ?4)",
                params![attempt_id, task_id, sha256(token), now],
            )
            .unwrap();
        drop(connection);

        let outcome = TaskExecutionOutcome {
            summary: "done".to_string(),
            document: TaskDocumentOutcome {
                title: "Durable report".to_string(),
                format: "markdown".to_string(),
                body: "# Result\n\nDurable content.".to_string(),
            },
            changed_files: Vec::new(),
            verification: Vec::new(),
            suggested_task_update: None,
            decision_candidates: Vec::new(),
            duration_ms: Some(20),
            token_usage: None,
            review_artifact: None,
            native_session_id: None,
        };
        broker
            .complete(
                attempt_id,
                CompleteAttemptRequest {
                    event_id: "attempt-document:complete".to_string(),
                    callback_token: token.to_string(),
                    lease_epoch: 1,
                    outcome: "succeeded".to_string(),
                    result: Some(serde_json::to_value(outcome).unwrap()),
                    error: None,
                    native_session_id: None,
                },
            )
            .unwrap();
        let task = broker.task(task_id).unwrap().unwrap();
        assert_eq!(task.status, "succeeded");
        assert_eq!(task.output_status, "materializing");

        let document_outbox = broker.claim_outbox().unwrap().unwrap();
        assert_eq!(document_outbox.destination, "document");
        let origin = crate::documents::DocumentOrigin {
            origin_type: crate::documents::DocumentOriginType::RuntimeTask,
            task_id: task_id.clone(),
            attempt_id: attempt_id.to_string(),
            batch_id: accepted.batch_id.clone(),
            executor_id: "pwcli".to_string(),
            agent_name: task.display_name.clone().unwrap(),
        };
        let document = crate::documents::create_delegated_markdown(
            directory.path(),
            "Durable report",
            "# Result\n\nDurable content.",
            origin,
        )
        .unwrap();
        assert!(broker
            .mark_document_ready(task_id, attempt_id, &document.manifest.id)
            .unwrap());
        broker.complete_outbox(&document_outbox.id).unwrap();

        let callback = broker.claim_outbox().unwrap().unwrap();
        assert_eq!(callback.destination, "session-doc");
        assert_eq!(callback.payload["sessionGeneration"].as_u64(), Some(7));
        assert_eq!(callback.payload["workItemId"], "todo-doc");
        let task = broker.task(task_id).unwrap().unwrap();
        assert_eq!(task.output_status, "ready");
        assert_eq!(task.primary_document_id, Some(document.manifest.id));
    }

    #[test]
    fn child_batch_is_derived_from_authenticated_parent_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let mut parent = RuntimeTaskSpec::delegated("parent", workspace.path().to_string_lossy());
        parent.executor = DelegatedExecutor::Pwcli;
        let accepted = broker
            .submit_batch(SubmitTaskBatch {
                root_session_id: "session-parent".to_string(),
                parent_task_id: None,
                work_item_id: None,
                session_generation: Some(1),
                idempotency_key: "parent".to_string(),
                join: "all".to_string(),
                tasks: vec![parent],
            })
            .unwrap();
        let parent_id = &accepted.task_ids[0];
        let token = "parent-token";
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "UPDATE runtime_tasks SET status='running', attempt=1 WHERE id=?1",
                [parent_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_attempts
                 (id, task_id, attempt_no, lease_epoch, token_hash, status, lease_expires_at)
                 VALUES ('parent-attempt', ?1, 1, 1, ?2, 'running', ?3)",
                params![parent_id, sha256(token), Utc::now().to_rfc3339()],
            )
            .unwrap();
        drop(connection);
        let requested_child_cwd = directory.path().join("untrusted-child-cwd");
        let mut child = RuntimeTaskSpec::delegated("child", requested_child_cwd.to_string_lossy());
        child.executor = DelegatedExecutor::Pwcli;
        let children = broker
            .submit_child_batch(
                "parent-attempt",
                SubmitChildTaskBatch {
                    callback_token: token.to_string(),
                    lease_epoch: 1,
                    idempotency_key: "child-batch".to_string(),
                    join: "all".to_string(),
                    tasks: vec![child],
                },
            )
            .unwrap();
        let parent = broker.task(parent_id).unwrap().unwrap();
        assert_eq!(parent.status, "waiting_children");
        let child = broker.task(&children.task_ids[0]).unwrap().unwrap();
        assert_eq!(child.parent_task_id.as_deref(), Some(parent_id.as_str()));
        assert_eq!(child.metadata["depth"].as_u64(), Some(1));
        assert_eq!(
            PathBuf::from(child.cwd).canonicalize().unwrap(),
            workspace.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn child_batch_failure_restores_parent_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let mut parent = RuntimeTaskSpec::delegated("parent", workspace.path().to_string_lossy());
        parent.executor = DelegatedExecutor::Pwcli;
        let accepted = broker
            .submit_batch(SubmitTaskBatch {
                root_session_id: "session-parent".to_string(),
                parent_task_id: None,
                work_item_id: None,
                session_generation: None,
                idempotency_key: "parent-key".to_string(),
                join: "all".to_string(),
                tasks: vec![parent],
            })
            .unwrap();
        let parent_id = &accepted.task_ids[0];
        let token = "parent-token";
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "UPDATE runtime_tasks SET status='running', attempt=1 WHERE id=?1",
                [parent_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_attempts
                 (id, task_id, attempt_no, lease_epoch, token_hash, status, lease_expires_at)
                 VALUES ('parent-attempt-failure', ?1, 1, 1, ?2, 'running', ?3)",
                params![
                    parent_id,
                    sha256(token),
                    (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()
                ],
            )
            .unwrap();
        drop(connection);

        let child = RuntimeTaskSpec::delegated("child", workspace.path().to_string_lossy());
        let error = broker
            .submit_child_batch(
                "parent-attempt-failure",
                SubmitChildTaskBatch {
                    callback_token: token.to_string(),
                    lease_epoch: 1,
                    idempotency_key: "parent-key".to_string(),
                    join: "all".to_string(),
                    tasks: vec![child],
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("different task batch"));
        assert_eq!(broker.task(parent_id).unwrap().unwrap().status, "running");
        let attempt_status: String = broker
            .connection()
            .unwrap()
            .query_row(
                "SELECT status FROM task_attempts WHERE id='parent-attempt-failure'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempt_status, "running");
    }

    #[test]
    fn each_join_enqueues_each_ready_task_once() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let accepted = broker
            .submit_batch(SubmitTaskBatch {
                root_session_id: "session-each".to_string(),
                parent_task_id: None,
                work_item_id: None,
                session_generation: None,
                idempotency_key: "each-key".to_string(),
                join: "each".to_string(),
                tasks: vec![
                    RuntimeTaskSpec::delegated("one", workspace.path().to_string_lossy()),
                    RuntimeTaskSpec::delegated("two", workspace.path().to_string_lossy()),
                ],
            })
            .unwrap();
        let connection = broker.connection().unwrap();
        for task_id in &accepted.task_ids {
            connection
                .execute(
                    "UPDATE runtime_tasks SET status='succeeded' WHERE id=?1",
                    [task_id],
                )
                .unwrap();
        }
        drop(connection);

        assert!(!broker
            .mark_document_ready(&accepted.task_ids[0], "attempt-each-1", "document-1")
            .unwrap());
        assert!(!broker
            .mark_document_ready(&accepted.task_ids[0], "attempt-each-1", "document-1")
            .unwrap());
        assert!(broker
            .mark_document_ready(&accepted.task_ids[1], "attempt-each-2", "document-2")
            .unwrap());
        let callbacks: i64 = broker
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM task_outbox WHERE destination='session-each'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(callbacks, 2);
    }

    #[test]
    fn nested_each_buffers_results_arriving_while_parent_runs() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let token = "nested-parent-token";
        let mut spec = RuntimeTaskSpec::delegated("parent", workspace.path().to_string_lossy());
        spec.executor = DelegatedExecutor::Pwcli;
        let full_document = format!("complete-child-document:{}", "z".repeat(6_000));
        let mut child_spec =
            RuntimeTaskSpec::delegated("child research", workspace.path().to_string_lossy());
        child_spec.parent_task_id = Some("nested-parent".to_string());
        child_spec.depth = 1;
        child_spec.resolved_executor_id = Some("pwcli".to_string());
        let mut child_payload = serde_json::to_value(&child_spec).unwrap();
        child_payload["outputStatus"] = Value::String("ready".to_string());
        child_payload["reviewStatus"] = Value::String("not_required".to_string());
        child_payload["primaryDocumentId"] = Value::String("child-document".to_string());
        let child_result = serde_json::json!({
            "summary": "child one result",
            "document": {
                "title": "Complete child result",
                "format": "markdown",
                "body": full_document,
            },
            "changedFiles": [{ "path": "src/lib.rs", "patch": "full patch" }],
            "verification": [{ "label": "tests", "status": "passed" }],
        });
        let now = Utc::now().to_rfc3339();
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "INSERT INTO runtime_batches
                 (id, root_session_id, join_mode, status, idempotency_key, created_at, updated_at)
                 VALUES ('nested-batch', 'nested-session', 'all', 'running', 'nested-key', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_tasks
                 (id, batch_id, root_session_id, kind, objective, cwd, payload_json, status,
                  delivery_status, attempt, created_at, updated_at)
                 VALUES ('nested-parent', 'nested-batch', 'nested-session', 'delegated', 'parent',
                         ?1, ?2, 'running', 'not_ready', 1, ?3, ?3)",
                params![
                    workspace.path().to_string_lossy(),
                    serde_json::to_string(&spec).unwrap(),
                    now,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_attempts
                 (id, task_id, attempt_no, lease_epoch, token_hash, status, lease_expires_at)
                 VALUES ('nested-attempt', 'nested-parent', 1, 1, ?1, 'running', ?2)",
                params![
                    sha256(token),
                    (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_batches
                 (id, root_session_id, parent_task_id, join_mode, status, idempotency_key,
                  created_at, updated_at)
                 VALUES ('child-batch', 'nested-session', 'nested-parent', 'each', 'running',
                         'child-key', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_tasks
                 (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd,
                  payload_json, status, delivery_status, attempt, result_json, created_at,
                  updated_at)
                 VALUES ('child-one', 'child-batch', 'nested-session', 'nested-parent',
                         'delegated', 'child research', ?1, ?2, 'succeeded', 'not_ready', 1,
                         ?3, ?4, ?4)",
                params![
                    workspace.path().to_string_lossy(),
                    child_payload.to_string(),
                    child_result.to_string(),
                    now,
                ],
            )
            .unwrap();
        drop(connection);
        let callback = serde_json::json!({
            "batchId": "child-batch",
            "taskIds": ["child-one"],
            "summary": "child one result",
            "joinMode": "each",
        });
        broker
            .resume_parent_from_children("nested-parent", &callback)
            .unwrap();
        broker
            .resume_parent_from_children("nested-parent", &callback)
            .unwrap();
        let buffered = broker.task("nested-parent").unwrap().unwrap();
        assert_eq!(
            buffered.metadata["pendingContinuationRefs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        broker
            .complete(
                "nested-attempt",
                CompleteAttemptRequest {
                    event_id: "nested-attempt:complete".to_string(),
                    callback_token: token.to_string(),
                    lease_epoch: 1,
                    outcome: "succeeded".to_string(),
                    result: Some(serde_json::json!({ "summary": "parent progress" })),
                    error: None,
                    native_session_id: None,
                },
            )
            .unwrap();
        let continued = broker.task("nested-parent").unwrap().unwrap();
        assert_eq!(continued.status, "queued");
        assert!(continued.objective.contains("child one result"));
        assert!(continued.objective.contains("artifact_id="));
        assert!(continued.metadata.get("pendingChildResults").is_none());
        assert!(continued.metadata.get("pendingContinuationRefs").is_none());
        let continued_spec =
            serde_json::from_value::<RuntimeTaskSpec>(continued.metadata.clone()).unwrap();
        assert_eq!(continued_spec.continuation_refs.len(), 1);
        let persisted =
            continuation::load(directory.path(), &continued_spec.continuation_refs[0]).unwrap();
        assert_eq!(persisted.outcomes[0].result, Some(child_result.clone()));
        assert!(
            load_worker_continuations(directory.path(), &continued_spec.continuation_refs)
                .unwrap()
                .contains(&"z".repeat(6_000))
        );

        let second_token = "nested-parent-token-2";
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "UPDATE runtime_tasks SET status='running', attempt=2 WHERE id='nested-parent'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_attempts
                 (id, task_id, attempt_no, lease_epoch, token_hash, status, lease_expires_at,
                  continuation_ref)
                 VALUES ('nested-attempt-2', 'nested-parent', 2, 2, ?1, 'running', ?2, ?3)",
                params![
                    sha256(second_token),
                    (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
                    serde_json::to_string(&continued_spec.continuation_refs).unwrap(),
                ],
            )
            .unwrap();
        drop(connection);
        broker
            .complete(
                "nested-attempt-2",
                CompleteAttemptRequest {
                    event_id: "nested-attempt-2:complete".to_string(),
                    callback_token: second_token.to_string(),
                    lease_epoch: 2,
                    outcome: "succeeded".to_string(),
                    result: Some(serde_json::json!({ "summary": "processed child one" })),
                    error: None,
                    native_session_id: None,
                },
            )
            .unwrap();
        assert_eq!(
            broker.task("nested-parent").unwrap().unwrap().status,
            "waiting_children"
        );

        let mut second_payload = child_payload;
        second_payload["primaryDocumentId"] = Value::String("child-document-2".to_string());
        let second_result = serde_json::json!({
            "summary": "child two result",
            "document": {
                "title": "Second child result",
                "format": "markdown",
                "body": "second complete document",
            },
            "changedFiles": [],
            "verification": [{ "label": "tests", "status": "passed" }],
        });
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "INSERT INTO runtime_tasks
                 (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd,
                  payload_json, status, delivery_status, attempt, result_json, created_at,
                  updated_at)
                 VALUES ('child-two', 'child-batch', 'nested-session', 'nested-parent',
                         'delegated', 'second child', ?1, ?2, 'succeeded', 'not_ready', 1,
                         ?3, ?4, ?4)",
                params![
                    workspace.path().to_string_lossy(),
                    second_payload.to_string(),
                    second_result.to_string(),
                    Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE runtime_batches SET status='completed' WHERE id='child-batch'",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(
            has_unconsumed_child_results(&broker.connection().unwrap(), "nested-parent").unwrap()
        );
        broker
            .resume_parent_from_children(
                "nested-parent",
                &serde_json::json!({
                    "batchId": "child-batch",
                    "taskIds": ["child-two"],
                    "summary": "child two result",
                    "joinMode": "each",
                }),
            )
            .unwrap();
        let after_second = broker.task("nested-parent").unwrap().unwrap();
        assert_eq!(after_second.status, "queued");
        let after_second_spec =
            serde_json::from_value::<RuntimeTaskSpec>(after_second.metadata).unwrap();
        assert_eq!(after_second_spec.continuation_refs.len(), 2);
        assert!(
            !has_unconsumed_child_results(&broker.connection().unwrap(), "nested-parent").unwrap()
        );

        drop(broker);
        let reopened = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let restored = reopened.task("nested-parent").unwrap().unwrap();
        let restored_spec = serde_json::from_value::<RuntimeTaskSpec>(restored.metadata).unwrap();
        assert_eq!(restored_spec.continuation_refs.len(), 2);
        assert_eq!(
            continuation::load(directory.path(), &restored_spec.continuation_refs[0])
                .unwrap()
                .outcomes[0]
                .result,
            Some(child_result)
        );
        assert_eq!(
            continuation::load(directory.path(), &restored_spec.continuation_refs[1])
                .unwrap()
                .outcomes[0]
                .result,
            Some(second_result)
        );
    }

    #[test]
    fn cancelling_parent_cascades_to_descendants() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let now = Utc::now().to_rfc3339();
        let parent = RuntimeTaskSpec::delegated("parent", workspace.path().to_string_lossy());
        let child = RuntimeTaskSpec::delegated("child", workspace.path().to_string_lossy());
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "INSERT INTO runtime_batches
                 (id, root_session_id, join_mode, status, idempotency_key, created_at, updated_at)
                 VALUES ('cancel-batch', 'cancel-session', 'all', 'running', 'cancel-key', ?1, ?1)",
                [&now],
            )
            .unwrap();
        for (id, parent_id, spec) in [
            ("cancel-parent", None, parent),
            ("cancel-child", Some("cancel-parent"), child),
        ] {
            connection
                .execute(
                    "INSERT INTO runtime_tasks
                     (id, batch_id, root_session_id, parent_task_id, kind, objective, cwd,
                      payload_json, status, delivery_status, attempt, created_at, updated_at)
                     VALUES (?1, 'cancel-batch', 'cancel-session', ?2, 'delegated', 'task', ?3,
                             ?4, 'queued', 'not_ready', 0, ?5, ?5)",
                    params![
                        id,
                        parent_id,
                        workspace.path().to_string_lossy(),
                        serde_json::to_string(&spec).unwrap(),
                        now,
                    ],
                )
                .unwrap();
        }
        drop(connection);
        broker.cancel("cancel-parent").unwrap();
        assert_eq!(
            broker.task("cancel-parent").unwrap().unwrap().status,
            "cancelled"
        );
        assert_eq!(
            broker.task("cancel-child").unwrap().unwrap().status,
            "cancelled"
        );
    }

    #[test]
    fn expired_running_lease_is_recovered_without_restart() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "expired-task",
            "expired-attempt",
            "running",
            workspace.path(),
        );
        assert_eq!(broker.recover_expired_attempts().unwrap(), 1);
        assert_eq!(
            broker.task("expired-task").unwrap().unwrap().status,
            "recovery_required"
        );
    }

    #[test]
    fn process_fingerprint_detects_identity_changes() {
        let command = "/bin/pwcli task-worker --descriptor /tmp/attempt.json";
        let fingerprint = WorkerProcessFingerprint {
            version: 1,
            pid: 42,
            start_marker: "Thu Jul 30 03:00:00 2026".to_string(),
            executable_identity: "/bin/pwcli".to_string(),
            command_sha256: sha256(command),
            descriptor_path: "/tmp/attempt.json".to_string(),
            descriptor_sha256: sha256("descriptor"),
        };
        assert!(process_fingerprint_matches(
            &fingerprint,
            42,
            "Thu Jul 30 03:00:00 2026",
            "/bin/pwcli",
            command,
        ));
        assert!(!process_fingerprint_matches(
            &fingerprint,
            42,
            "Thu Jul 30 04:00:00 2026",
            "/bin/pwcli",
            command,
        ));
    }

    #[cfg(unix)]
    #[test]
    fn legacy_attempt_without_fingerprint_is_never_signalled() {
        use std::os::unix::process::CommandExt;

        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let mut child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        insert_active_attempt(
            &broker,
            "legacy-running-task",
            "legacy-running-attempt",
            "running",
            workspace.path(),
        );
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE task_attempts SET worker_pid=?2, worker_pgid=?2 WHERE id=?1",
                params!["legacy-running-attempt", child.id()],
            )
            .unwrap();

        broker.cancel("legacy-running-task").unwrap();
        assert!(child.try_wait().unwrap().is_none());
        let event = broker
            .events_after(0)
            .unwrap()
            .into_iter()
            .find(|event| event.kind == "task_cancelled")
            .unwrap();
        assert_eq!(event.payload["workerSignal"]["outcome"], "skipped");
        assert_eq!(
            event.payload["workerSignal"]["reason"],
            "missing_process_fingerprint"
        );
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn callback_spool_is_atomically_materialized() {
        let directory = tempfile::tempdir().unwrap();
        let envelope = SpoolEnvelope {
            attempt_id: "spooled-attempt".to_string(),
            request: CompleteAttemptRequest {
                event_id: "spooled-attempt:complete".to_string(),
                callback_token: "secret".to_string(),
                lease_epoch: 1,
                outcome: "failed".to_string(),
                result: None,
                error: Some("offline".to_string()),
                native_session_id: None,
            },
        };
        write_spool_envelope(directory.path(), &envelope).unwrap();
        let spool = directory.path().join("task-callback-spool");
        let entries = fs::read_dir(&spool)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].file_name().and_then(|name| name.to_str()),
            Some("spooled-attempt.json")
        );
        let restored =
            serde_json::from_slice::<SpoolEnvelope>(&fs::read(&entries[0]).unwrap()).unwrap();
        assert_eq!(restored.attempt_id, envelope.attempt_id);
    }

    #[test]
    fn review_file_list_includes_paths_without_text_previews() {
        let lease = worktree::WorktreeLease {
            task_id: "task-files".to_string(),
            lease_key: "lease".to_string(),
            repo_root: PathBuf::from("/repo"),
            bound_relative: PathBuf::from("bound"),
            original_head: "head".to_string(),
            baseline_commit: "base".to_string(),
            baseline_ref: "ref".to_string(),
            lease_dir: PathBuf::from("/data/lease"),
            worktree_dir: PathBuf::from("/data/lease/worktree"),
            worker_cwd: PathBuf::from("/data/lease/worktree/bound"),
            manifest: BTreeMap::new(),
            created_at: Utc::now(),
        };
        let affected_paths = (0..=101)
            .map(|index| PathBuf::from(format!("bound/file-{index}.bin")))
            .collect::<Vec<_>>();
        let artifact = worktree::ReviewArtifact {
            task_id: lease.task_id.clone(),
            lease_key: lease.lease_key.clone(),
            attempt_id: "attempt-files".to_string(),
            baseline_commit: "base".to_string(),
            result_commit: "result".to_string(),
            patch_path: PathBuf::from("/data/lease/attempts/a/changes.patch"),
            patch_bytes: 0,
            patch_sha256: "hash".to_string(),
            affected_paths,
            created_at: Utc::now(),
        };
        let files = complete_review_file_list(Vec::new(), Some(&lease), Some(&artifact));
        assert_eq!(files.len(), 102);
        assert_eq!(files[101].path, "file-101.bin");
    }

    #[test]
    fn root_batch_cannot_escape_its_bound_session_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let workspace_root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let bound = workspace_root.path().join("bound");
        let sibling = workspace_root.path().join("sibling");
        let nested = bound.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace_root.path().to_path_buf(),
        )
        .unwrap();
        let request = |cwd: &Path, key: &str| SubmitTaskBatch {
            root_session_id: "bound-session".to_string(),
            parent_task_id: None,
            work_item_id: None,
            session_generation: Some(1),
            idempotency_key: key.to_string(),
            join: "all".to_string(),
            tasks: vec![RuntimeTaskSpec::delegated(
                "bounded task",
                cwd.to_string_lossy(),
            )],
        };

        let error = broker
            .submit_batch_in_workspace(request(&sibling, "escape"), &bound)
            .unwrap_err();
        assert!(error.to_string().contains("root session workspace"));
        assert!(broker
            .submit_batch_in_workspace(request(&nested, "nested"), &bound)
            .is_ok());
    }

    #[test]
    fn dynamic_broker_resolves_the_latest_workspace_root_setting() {
        let directory = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let broker =
            TaskBroker::new_internal(directory.path(), "http://127.0.0.1:9", None).unwrap();

        assert_eq!(
            broker
                .workspace_root_for_setting(&first.path().to_string_lossy())
                .unwrap(),
            first.path().canonicalize().unwrap()
        );
        assert_eq!(
            broker
                .workspace_root_for_setting(&second.path().to_string_lossy())
                .unwrap(),
            second.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn native_session_event_spool_is_durable_idempotent_and_fenced() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "native-task",
            "native-attempt",
            "running",
            workspace.path(),
        );
        let token = "native-token";
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE task_attempts SET token_hash=?2 WHERE id=?1",
                params!["native-attempt", sha256(token)],
            )
            .unwrap();
        let request = AttemptEventRequest {
            event_id: "native-attempt:session".to_string(),
            callback_token: token.to_string(),
            lease_epoch: 1,
            kind: "native_session_started".to_string(),
            payload: serde_json::json!({ "nativeSessionId": "codex-session-1" }),
        };
        let path = write_attempt_event_spool(
            directory.path(),
            &AttemptEventSpoolEnvelope {
                attempt_id: "native-attempt".to_string(),
                request: request.clone(),
            },
        )
        .unwrap();

        broker.import_spool().unwrap();
        assert!(!path.exists());
        broker
            .record_attempt_event("native-attempt", request)
            .unwrap();
        let connection = broker.connection().unwrap();
        let native_session_id: String = connection
            .query_row(
                "SELECT native_session_id FROM task_attempts WHERE id='native-attempt'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM task_events WHERE event_id='native-attempt:session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(native_session_id, "codex-session-1");
        assert_eq!(event_count, 1);
        drop(connection);
        let conflicting = AttemptEventRequest {
            event_id: "native-attempt:session".to_string(),
            callback_token: token.to_string(),
            lease_epoch: 1,
            kind: "native_session_started".to_string(),
            payload: serde_json::json!({ "nativeSessionId": "different-session" }),
        };
        assert!(broker
            .record_attempt_event("native-attempt", conflicting)
            .is_err());
        let stale = AttemptEventRequest {
            event_id: "native-attempt:stale".to_string(),
            callback_token: token.to_string(),
            lease_epoch: 2,
            kind: "native_session_started".to_string(),
            payload: serde_json::json!({ "nativeSessionId": "stale" }),
        };
        assert!(broker
            .record_attempt_event("native-attempt", stale)
            .is_err());
    }

    #[test]
    fn acp_decision_is_persisted_and_resumes_the_same_native_session() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "decision-task",
            "decision-attempt",
            "running",
            workspace.path(),
        );
        let mut spec =
            RuntimeTaskSpec::delegated("choose safely", workspace.path().to_string_lossy());
        spec.executor = DelegatedExecutor::Codex;
        spec.resolved_executor_kind = Some("acp_cli".to_string());
        spec.resolved_executor_id = Some("codex".to_string());
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE runtime_tasks SET payload_json=?2 WHERE id=?1",
                params!["decision-task", serde_json::to_string(&spec).unwrap()],
            )
            .unwrap();
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE task_attempts SET token_hash=?2 WHERE id=?1",
                params!["decision-attempt", sha256("decision-token")],
            )
            .unwrap();

        broker
            .complete(
                "decision-attempt",
                CompleteAttemptRequest {
                    event_id: "decision-attempt:required".to_string(),
                    callback_token: "decision-token".to_string(),
                    lease_epoch: 1,
                    outcome: "waiting_user".to_string(),
                    result: Some(serde_json::json!({
                        "summary": "需要用户判断",
                        "question": "保留兼容接口吗？",
                        "options": ["保留一版", "立即删除"],
                        "nativeSessionId": "codex-native-1",
                    })),
                    error: None,
                    native_session_id: Some("codex-native-1".to_string()),
                },
            )
            .unwrap();
        let waiting = broker.task("decision-task").unwrap().unwrap();
        assert_eq!(waiting.status, "waiting_user");
        assert_eq!(waiting.delivery_status, "waiting_user");
        assert_eq!(waiting.result.unwrap()["question"], "保留兼容接口吗？");

        let request = TaskDecisionRequest {
            client_action_id: "decision-action-1".to_string(),
            option: "保留一版".to_string(),
            note: None,
        };
        let response = broker
            .resolve_decision("decision-task", request.clone())
            .unwrap();
        assert_eq!(response.status, "queued");
        assert_eq!(
            broker
                .resolve_decision("decision-task", request)
                .unwrap()
                .selected_option,
            "保留一版"
        );
        let resumed = broker.task("decision-task").unwrap().unwrap();
        assert_eq!(resumed.status, "queued");
        let resumed_spec = serde_json::from_value::<RuntimeTaskSpec>(resumed.metadata).unwrap();
        assert_eq!(
            resumed_spec.resume_session_id.as_deref(),
            Some("codex-native-1")
        );
        assert_eq!(resumed_spec.resolved_executor_id.as_deref(), Some("codex"));
        assert!(resumed_spec.objective.contains("保留一版"));
    }

    #[test]
    fn permanent_document_failure_stops_outbox_and_requires_attention() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "output-task",
            "output-attempt",
            "succeeded",
            workspace.path(),
        );
        let connection = broker.connection().unwrap();
        connection
            .execute(
                "UPDATE runtime_tasks
                 SET payload_json=json_set(payload_json, '$.outputStatus', 'materializing')
                 WHERE id='output-task'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_outbox
                 (id, source_event_id, destination, dedupe_key, status, payload_json,
                  created_at, updated_at)
                 VALUES ('output-outbox', 'output-source', 'document', 'output-dedupe',
                         'processing', '{}', ?1, ?1)",
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
        drop(connection);

        broker
            .fail_document_outbox(
                "output-outbox",
                "output-task",
                "output-attempt",
                "document storage is not writable",
            )
            .unwrap();
        let task = broker.task("output-task").unwrap().unwrap();
        assert_eq!(task.output_status, "failed");
        assert_eq!(task.delivery_status, "waiting_user");
        let connection = broker.connection().unwrap();
        let outbox_status: String = connection
            .query_row(
                "SELECT status FROM task_outbox WHERE id='output-outbox'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outbox_status, "failed");
        let event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM task_events
                 WHERE event_id='output-attempt:document-failed'
                   AND kind='task_output_failed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 1);
        assert_eq!(broker.list_attention().unwrap()[0].kind, "output_failed");
    }

    #[test]
    fn background_tool_projection_persists_failure_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let task = broker
            .project_background_tool(
                "session-bg",
                "bg_1",
                "web_query",
                "query docs",
                Some(serde_json::json!({"q":"hello"})),
                workspace.path().to_string_lossy().as_ref(),
            )
            .unwrap();
        assert_eq!(task.kind, "background_tool");
        assert_eq!(task.status, "running");
        let failed = broker
            .complete_background_tool(&task.id, false, "HTTP 503 service unavailable")
            .unwrap();
        assert_eq!(failed.status, "failed");
        let failure = failed.failure.expect("failure envelope");
        assert_eq!(failure.class, crate::reliability::FailureClass::Transient);
        assert_eq!(
            failure.disposition,
            crate::reliability::FailureDisposition::UserActionRequired
        );
        let attention = broker.list_attention().unwrap();
        assert_eq!(attention.len(), 1);
        assert_eq!(attention[0].kind, "background_failure");
        assert!(attention[0].detail.get("failure").is_some());
    }

    #[test]
    fn legacy_attention_backfill_is_idempotent_and_never_reopens_resolved_work() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "legacy-attention-task",
            "legacy-attention-attempt",
            "succeeded",
            workspace.path(),
        );
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE runtime_tasks
                 SET status='succeeded',
                     payload_json=json_set(payload_json,
                       '$.outputStatus', 'ready', '$.reviewStatus', 'pending')
                 WHERE id='legacy-attention-task'",
                [],
            )
            .unwrap();

        assert_eq!(broker.backfill_legacy_attention().unwrap(), 1);
        assert_eq!(broker.backfill_legacy_attention().unwrap(), 0);
        let attention = broker.list_attention().unwrap().remove(0);
        assert_eq!(attention.kind, "document_ready");
        broker
            .update_attention_status(&attention.id, "resolved", Some(attention.revision))
            .unwrap();
        assert_eq!(broker.backfill_legacy_attention().unwrap(), 0);
    }

    #[test]
    fn dismissed_attention_is_an_idempotent_persistent_tombstone() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "dismissed-attention-task",
            "dismissed-attention-attempt",
            "failed",
            workspace.path(),
        );
        let attention = broker
            .create_attention(
                "dismissed-attention-task",
                "task_failed",
                "dismissed-attention-task:failed:1",
                "执行失败",
                Value::Null,
            )
            .unwrap()
            .unwrap();

        let dismissed = broker.dismiss_attention(&attention.id).unwrap();
        assert_eq!(dismissed.status, "resolved");
        assert_eq!(dismissed.revision, attention.revision + 1);
        let repeated = broker.dismiss_attention(&attention.id).unwrap();
        assert_eq!(repeated.status, "resolved");
        assert_eq!(repeated.revision, dismissed.revision);

        drop(broker);
        let reopened = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        assert_eq!(reopened.backfill_legacy_attention().unwrap(), 0);
        let persisted = reopened.attention(&attention.id).unwrap().unwrap();
        assert_eq!(persisted.status, "resolved");
        assert_eq!(reopened.list_attention().unwrap().len(), 1);
    }

    #[test]
    fn attention_mutation_waits_for_short_database_write_contention() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "contended-attention-task",
            "contended-attention-attempt",
            "failed",
            workspace.path(),
        );
        let attention = broker
            .create_attention(
                "contended-attention-task",
                "task_failed",
                "contended-attention-task:failed:1",
                "执行失败",
                Value::Null,
            )
            .unwrap()
            .unwrap();

        let mut blocker = Connection::open(broker.db_path.as_ref()).unwrap();
        let blocker_transaction = blocker
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        blocker_transaction
            .execute(
                "UPDATE runtime_attention SET title=title WHERE id=?1",
                [&attention.id],
            )
            .unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = Arc::clone(&broker);
        let attention_id = attention.id.clone();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            result_tx
                .send(worker.dismiss_attention(&attention_id))
                .unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(result_rx
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err());

        blocker_transaction.commit().unwrap();
        let dismissed = result_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap();
        handle.join().unwrap();
        assert_eq!(dismissed.status, "resolved");
    }

    #[test]
    fn retry_preserves_executor_unless_user_explicitly_overrides_it() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        insert_active_attempt(
            &broker,
            "retry-original",
            "retry-original-attempt",
            "failed",
            workspace.path(),
        );
        let mut original = RuntimeTaskSpec::delegated("retry", workspace.path().to_string_lossy());
        original.executor = DelegatedExecutor::Codex;
        original.resolved_executor_kind = Some("acp_cli".to_string());
        original.resolved_executor_id = Some("codex".to_string());
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE runtime_tasks SET payload_json=?2 WHERE id=?1",
                params!["retry-original", serde_json::to_string(&original).unwrap()],
            )
            .unwrap();
        let unchanged = broker.retry("retry-original").unwrap();
        assert_eq!(unchanged.resolved_executor_id.as_deref(), Some("codex"));

        insert_active_attempt(
            &broker,
            "retry-switched",
            "retry-switched-attempt",
            "failed",
            workspace.path(),
        );
        broker
            .connection()
            .unwrap()
            .execute(
                "UPDATE runtime_tasks SET payload_json=?2 WHERE id=?1",
                params!["retry-switched", serde_json::to_string(&original).unwrap()],
            )
            .unwrap();
        let switched = broker
            .retry_with_executor("retry-switched", Some(DelegatedExecutor::Pwcli))
            .unwrap();
        assert_eq!(switched.executor_request, "pwcli");
        assert_eq!(switched.resolved_executor_id.as_deref(), Some("pwcli"));
        assert_eq!(
            switched.resolved_executor_kind.as_deref(),
            Some("internal_agent")
        );
    }

    #[test]
    fn legacy_supervisor_graph_is_imported_once_and_active_work_is_fenced() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let task_id = "task-legacy";
        let now = Utc::now().to_rfc3339();
        let connection = Connection::open(directory.path().join("supervisor.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE supervisor_tasks (
                   id TEXT PRIMARY KEY, objective TEXT NOT NULL, project_dir TEXT NOT NULL,
                   status TEXT NOT NULL, result TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                 );
                 CREATE TABLE supervisor_children (
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, backend TEXT NOT NULL,
                   label TEXT NOT NULL, mode TEXT NOT NULL, status TEXT NOT NULL,
                   session_id TEXT, summary TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                 );
                 CREATE TABLE supervisor_dispatches (
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, child_id TEXT NOT NULL,
                   content TEXT NOT NULL, status TEXT NOT NULL, summary TEXT,
                   created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                 );
                 CREATE TABLE supervisor_events (
                   sequence INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL,
                   kind TEXT NOT NULL, actor TEXT NOT NULL, message TEXT NOT NULL,
                   detail TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO supervisor_tasks
                 (id, objective, project_dir, status, created_at, updated_at)
                 VALUES (?1, 'legacy objective', ?2, 'running', ?3, ?3)",
                params![task_id, workspace.path().to_string_lossy(), now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO supervisor_children
                 (id, task_id, backend, label, mode, status, created_at, updated_at)
                 VALUES ('child-legacy', ?1, 'codex', 'Codex', 'edit', 'running', ?2, ?2)",
                params![task_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO supervisor_children
                 (id, task_id, backend, label, mode, status, summary, created_at, updated_at)
                 VALUES ('child-complete', ?1, 'codex', 'Researcher', 'research',
                         'completed', 'legacy durable result', ?2, ?2)",
                params![task_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO supervisor_dispatches
                 (id, task_id, child_id, content, status, summary, created_at, updated_at)
                 VALUES ('dispatch-legacy', ?1, 'child-legacy', 'finish it', 'completed', 'done', ?2, ?2)",
                params![task_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO supervisor_events
                 (task_id, kind, actor, message, detail, created_at)
                 VALUES (?1, 'child_summary', 'codex', 'done', '{}', ?2)",
                params![task_id, now],
            )
            .unwrap();
        drop(connection);
        let broker = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        let records = broker.list().unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(
            records
                .iter()
                .find(|record| record.id == "child-legacy")
                .unwrap()
                .status,
            "recovery_required"
        );
        assert_eq!(
            records
                .iter()
                .find(|record| record.id == "dispatch-legacy")
                .unwrap()
                .status,
            "succeeded"
        );
        let completed = records
            .iter()
            .find(|record| record.id == "child-complete")
            .unwrap();
        assert_eq!(completed.status, "succeeded");
        assert_eq!(completed.output_status, "materializing");
        let legacy_document = broker.claim_outbox().unwrap().unwrap();
        assert_eq!(legacy_document.destination, "document");
        assert_eq!(legacy_document.payload["taskId"], "child-complete");
        assert!(broker.events_after(0).unwrap().iter().any(|event| {
            event.task_id == "dispatch-legacy" && event.kind == "legacy_dispatch_migrated"
        }));
        assert!(!directory.path().join("supervisor.db").exists());
        assert!(directory.path().join("supervisor.db.migrated-v1").exists());
        drop(broker);

        let reopened = TaskBroker::new_with_workspace_root(
            directory.path(),
            "http://127.0.0.1:9",
            workspace.path().to_path_buf(),
        )
        .unwrap();
        assert_eq!(reopened.list().unwrap().len(), 4);
    }
}
