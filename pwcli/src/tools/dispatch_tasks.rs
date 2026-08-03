use std::sync::Arc;

use serde_json::Value;

use crate::contracts::ports::{
    PublisherModelContext, SessionContextPort, TaskPublishRequest, TaskPublisherPort,
};
use crate::contracts::SessionId;
use crate::task::{DelegatedAccess, DelegatedExecutor, DelegatedRole, RuntimeTaskSpec};
use crate::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

pub fn register(
    registry: &ToolRegistry,
    sessions: Arc<dyn SessionContextPort>,
    publisher: Arc<dyn TaskPublisherPort>,
) {
    let dispatch_sessions = Arc::clone(&sessions);
    let dispatch_publisher = Arc::clone(&publisher);
    registry.register_contextual_structured_with_impact(
        "dispatch_tasks",
        "Atomically publish one or more durable delegated tasks to Workbench Agent, Codex, Qoder or Kimi. The current turn ends immediately after acceptance; the daemon resumes this session through its durable outbox after all required Markdown outputs are ready. Never wait or poll for these tasks.",
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
                            "role": { "type": "string", "enum": ["researcher", "engineer", "reviewer", "analyst", "operator", "general"], "default": "general" },
                            "access": { "type": "string", "enum": ["read_only", "mutating"], "default": "read_only" },
                            "executor": { "type": "string", "enum": ["auto", "pwcli", "codex", "qoder", "kimi"], "default": "auto" },
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
        Box::new(move |context, args: &Value| {
            let sessions = Arc::clone(&dispatch_sessions);
            let publisher = Arc::clone(&dispatch_publisher);
            let session_id = context.session_id.clone();
            let work_item_id = context.work_item_id.clone();
            let publisher_model = context.active_model.as_ref().map(|model| PublisherModelContext {
                provider_id: model.provider_id.clone(),
                model: model.model_id.clone(),
                effort: model.effort.clone(),
                thinking: model.thinking,
            });
            let args = args.clone();
            Box::pin(async move {
                let root_session_id = session_id
                    .ok_or_else(|| anyhow::anyhow!("dispatch_tasks requires an active daemon session"))?;
                let session = sessions.resolve(&root_session_id).await?;
                let idempotency_key = args
                    .get("idempotencyKey")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("idempotencyKey is required"))?
                    .to_string();
                let accepted = publisher
                    .publish(
                        &session,
                        TaskPublishRequest {
                            idempotency_key,
                            join: args
                                .get("join")
                                .and_then(Value::as_str)
                                .unwrap_or("all")
                                .to_string(),
                            work_item_id,
                            tasks: args.get("tasks").cloned().unwrap_or(Value::Null),
                            publisher_model,
                        },
                    )
                    .await?;
                let task_count = accepted
                    .get("taskIds")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default();
                let batch_id = accepted
                    .get("batchId")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Ok(ToolOutput {
                    content: format!(
                        "已委派 {} 个任务（批次 {}）。当前回合现在结束；daemon 将在完成后恢复会话。",
                        task_count, batch_id
                    ),
                    terminate: true,
                    details: Some(serde_json::json!({
                        "delegation": accepted,
                    })),
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
    register_legacy_task(registry, sessions, publisher);
}

fn register_legacy_task(
    registry: &ToolRegistry,
    sessions: Arc<dyn SessionContextPort>,
    publisher: Arc<dyn TaskPublisherPort>,
) {
    registry.register_contextual_structured_with_impact(
        "task",
        "Compatibility wrapper for one durable Workbench Agent delegation. It publishes a RuntimeTask and ends the current turn immediately; it never runs or waits inside the daemon process.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "minLength": 1 },
                "agent_type": { "type": "string", "enum": ["general-purpose", "researcher", "bash"] },
                "access": { "type": "string", "enum": ["read_only", "mutating"] },
                "deliverableTitle": { "type": "string" }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::Control,
        Box::new(move |context, args: &Value| {
            let sessions = Arc::clone(&sessions);
            let publisher = Arc::clone(&publisher);
            let session_id = context.session_id.clone();
            let work_item_id = context.work_item_id.clone();
            let publisher_model = context.active_model.as_ref().map(|model| PublisherModelContext {
                provider_id: model.provider_id.clone(),
                model: model.model_id.clone(),
                effort: model.effort.clone(),
                thinking: model.thinking,
            });
            let args = args.clone();
            Box::pin(async move {
                let root_session_id: SessionId = session_id
                    .ok_or_else(|| anyhow::anyhow!("task requires an active daemon session"))?;
                let session = sessions.resolve(&root_session_id).await?;
                let cwd = session.workspace.clone();
                let prompt = args
                    .get("prompt")
                    .and_then(Value::as_str)
                    .filter(|prompt| !prompt.trim().is_empty())
                    .ok_or_else(|| anyhow::anyhow!("prompt is required"))?;
                let agent_type = args
                    .get("agent_type")
                    .and_then(Value::as_str)
                    .unwrap_or("general-purpose");
                let mut spec = RuntimeTaskSpec::delegated(prompt, cwd);
                spec.executor = DelegatedExecutor::Pwcli;
                spec.role = match agent_type {
                    "researcher" => DelegatedRole::Researcher,
                    "bash" => DelegatedRole::Operator,
                    _ => DelegatedRole::General,
                };
                spec.access = match args.get("access").and_then(Value::as_str) {
                    Some("mutating") => DelegatedAccess::Mutating,
                    Some("read_only") => DelegatedAccess::ReadOnly,
                    _ if agent_type == "bash" => DelegatedAccess::Mutating,
                    _ => DelegatedAccess::ReadOnly,
                };
                spec.deliverable_title = args
                    .get("deliverableTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let accepted = publisher
                    .publish(
                        &session,
                        TaskPublishRequest {
                            idempotency_key: format!("legacy-task:{}", uuid::Uuid::now_v7()),
                            join: "all".to_string(),
                            work_item_id,
                            tasks: serde_json::to_value(vec![spec])?,
                            publisher_model,
                        },
                    )
                    .await?;
                let task_id = accepted
                    .get("taskIds")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Ok(ToolOutput {
                    content: format!(
                        "已委派 Workbench Agent（任务 {}）。当前回合结束；结果文档就绪后 daemon 会恢复会话。",
                        task_id
                    ),
                    terminate: true,
                    details: Some(serde_json::json!({ "delegation": accepted })),
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
}
