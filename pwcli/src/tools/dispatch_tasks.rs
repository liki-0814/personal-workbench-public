use std::sync::Arc;

use serde_json::Value;

use crate::service::session_manager::SessionManager;
use crate::task::{
    DelegatedAccess, DelegatedExecutor, DelegatedRole, RuntimeTaskSpec, SubmitTaskBatch, TaskBroker,
};
use crate::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

pub fn register(registry: &ToolRegistry, broker: Arc<TaskBroker>, sessions: SessionManager) {
    let dispatch_broker = Arc::clone(&broker);
    let dispatch_sessions = sessions.clone();
    registry.register_structured_with_impact(
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
        Box::new(move |args: &Value| {
            let broker = Arc::clone(&dispatch_broker);
            let sessions = dispatch_sessions.clone();
            let args = args.clone();
            Box::pin(async move {
                let root_session_id = crate::tools::web_context::session_id()
                    .ok_or_else(|| anyhow::anyhow!("dispatch_tasks requires an active daemon session"))?;
                let session = sessions
                    .get(&root_session_id)
                    .ok_or_else(|| anyhow::anyhow!("active daemon session disappeared"))?;
                let bound_workspace = session
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.canonical_path.clone())
                    .ok_or_else(|| anyhow::anyhow!("session has no bound workspace"))?;
                let generation = session.generation;
                let work_item_id = crate::tools::web_context::work_item_id();
                let idempotency_key = args
                    .get("idempotencyKey")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("idempotencyKey is required"))?
                    .to_string();
                let tasks = serde_json::from_value::<Vec<RuntimeTaskSpec>>(
                    args.get("tasks").cloned().unwrap_or(Value::Null),
                )?;
                let accepted = broker.submit_batch_in_workspace(
                    SubmitTaskBatch {
                        root_session_id,
                        parent_task_id: None,
                        work_item_id,
                        session_generation: Some(generation),
                        idempotency_key,
                        join: args
                            .get("join")
                            .and_then(Value::as_str)
                            .unwrap_or("all")
                            .to_string(),
                        tasks,
                    },
                    &bound_workspace,
                )?;
                Ok(ToolOutput {
                    content: format!(
                        "已委派 {} 个任务（批次 {}）。当前回合现在结束；daemon 将在完成后恢复会话。",
                        accepted.task_ids.len(), accepted.batch_id
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
    register_legacy_task(registry, broker, sessions);
}

fn register_legacy_task(
    registry: &ToolRegistry,
    broker: Arc<TaskBroker>,
    sessions: SessionManager,
) {
    registry.register_structured_with_impact(
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
        Box::new(move |args: &Value| {
            let broker = Arc::clone(&broker);
            let sessions = sessions.clone();
            let args = args.clone();
            Box::pin(async move {
                let root_session_id = crate::tools::web_context::session_id()
                    .ok_or_else(|| anyhow::anyhow!("task requires an active daemon session"))?;
                let session = sessions
                    .get(&root_session_id)
                    .ok_or_else(|| anyhow::anyhow!("active daemon session disappeared"))?;
                let bound_workspace = session
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.canonical_path.clone())
                    .ok_or_else(|| anyhow::anyhow!("session has no bound workspace"))?;
                let cwd = bound_workspace.to_string_lossy().into_owned();
                let generation = session.generation;
                let work_item_id = crate::tools::web_context::work_item_id();
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
                let accepted = broker.submit_batch_in_workspace(
                    SubmitTaskBatch {
                        root_session_id,
                        parent_task_id: None,
                        work_item_id,
                        session_generation: Some(generation),
                        idempotency_key: format!("legacy-task:{}", uuid::Uuid::now_v7()),
                        join: "all".to_string(),
                        tasks: vec![spec],
                    },
                    &bound_workspace,
                )?;
                Ok(ToolOutput {
                    content: format!(
                        "已委派 Workbench Agent（任务 {}）。当前回合结束；结果文档就绪后 daemon 会恢复会话。",
                        accepted.task_ids[0]
                    ),
                    terminate: true,
                    details: Some(serde_json::json!({ "delegation": accepted })),
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
}
