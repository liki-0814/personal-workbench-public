//! `background_tasks` 工具：让模型主动查询/取消/重试后台任务。
//!
//! 超时自动晋升（或显式 `background=true`）产生的后台任务完成后，
//! 结果会通过 runtime callback 回到会话；但模型在 turn 内也可以随时
//! 用本工具掌握进度，不必盲等。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::runtime::background::BackgroundTaskManager;
use crate::runtime::tools::registry::{ToolExecutionMode, ToolImpact, ToolRegistry};

async fn ensure_task_owned_by_session(
    manager: &BackgroundTaskManager,
    task_id: &str,
    session_id: Option<&str>,
) -> anyhow::Result<()> {
    let session_id = session_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("后台任务控制需要有效的会话上下文"))?;
    let owner = manager
        .task_session_id(task_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("后台任务不存在: {}", task_id))?;
    if owner != session_id {
        anyhow::bail!("不能操作其他会话的后台任务: {}", task_id);
    }
    Ok(())
}

pub fn register(
    registry: &ToolRegistry,
    manager: Arc<BackgroundTaskManager>,
    tools: Arc<ToolRegistry>,
) {
    fn resolve_impact(args: &Value) -> ToolImpact {
        match args.get("action").and_then(Value::as_str).unwrap_or("list") {
            "cancel" => ToolImpact::Control,
            // list / retry（retry 仅支持幂等只读工具的重放）
            _ => ToolImpact::Observe,
        }
    }

    registry.register_contextual_structured_with_impact_resolver(
        "background_tasks",
        "管理本会话的后台任务（超时自动转后台或显式 background=true 产生的任务）。\
         action=list: 查看后台任务状态（默认仅当前会话，all=true 查看全部）；\
         action=cancel: 取消运行中的任务；\
         action=retry: 重试失败的、可重放的任务。\
         后台任务完成后会自动通知，无需轮询；仅在需要掌握进度或处理失败时使用。",
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "cancel", "retry"],
                    "description": "操作类型，默认 list"
                },
                "task_id": { "type": "string", "description": "后台任务 ID（形如 bg_xxx），cancel/retry 时必填" },
                "all": { "type": "boolean", "description": "list 时是否查看所有会话的后台任务（默认 false，仅当前会话）" }
            },
            "required": ["action"]
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::Observe,
        resolve_impact,
        Box::new(move |context, args| {
            let manager = Arc::clone(&manager);
            let tools = Arc::clone(&tools);
            let session_filter = context
                .session_id
                .as_ref()
                .map(|id| id.to_string())
                .filter(|id| !id.is_empty());
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("list")
                .to_string();
            let task_id = args
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
            Box::pin(async move {
                match action.as_str() {
                    "list" => {
                        let tasks = manager.list().await;
                        let scoped: Vec<_> = match (&session_filter, all) {
                            (Some(filter), false) => tasks
                                .into_iter()
                                .filter(|task| task.session_id == *filter)
                                .collect(),
                            _ => tasks,
                        };
                        if scoped.is_empty() {
                            return Ok(crate::runtime::tools::registry::ToolOutput::text(
                                "（当前没有后台任务）".to_string(),
                            ));
                        }
                        let lines: Vec<String> = scoped
                            .iter()
                            .map(|task| {
                                let status = match task.status {
                                    crate::runtime::background::TaskStatus::Running => "running",
                                    crate::runtime::background::TaskStatus::Completed => {
                                        match task.success {
                                            Some(true) => "completed",
                                            Some(false) => "failed",
                                            None => "completed",
                                        }
                                    }
                                    crate::runtime::background::TaskStatus::Failed => "failed",
                                    crate::runtime::background::TaskStatus::Cancelled => {
                                        "cancelled"
                                    }
                                };
                                let extra = task
                                    .summary
                                    .as_deref()
                                    .map(|summary| {
                                        let brief: String = summary.chars().take(200).collect();
                                        format!(" | {}", brief)
                                    })
                                    .unwrap_or_default();
                                let retryable =
                                    if task.replayable { " | 可重试(action=retry)" } else { "" };
                                format!(
                                    "- {} [{}] {}s {}{}{}",
                                    task.id,
                                    status,
                                    task.elapsed_secs,
                                    task.description,
                                    retryable,
                                    extra
                                )
                            })
                            .collect();
                        Ok(crate::runtime::tools::registry::ToolOutput::text(format!(
                            "后台任务 ({}):\n{}",
                            scoped.len(),
                            lines.join("\n")
                        )))
                    }
                    "cancel" => {
                        if task_id.is_empty() {
                            anyhow::bail!("cancel 需要 task_id");
                        }
                        ensure_task_owned_by_session(
                            manager.as_ref(),
                            &task_id,
                            session_filter.as_deref(),
                        )
                        .await?;
                        if manager.cancel(&task_id).await {
                            Ok(crate::runtime::tools::registry::ToolOutput::text(format!(
                                "✅ 后台任务 {} 已取消",
                                task_id
                            )))
                        } else {
                            Ok(crate::runtime::tools::registry::ToolOutput::text(format!(
                                "⚠ 任务 {} 不存在或不在运行中（可能已完成/已取消）",
                                task_id
                            )))
                        }
                    }
                    "retry" => {
                        if task_id.is_empty() {
                            anyhow::bail!("retry 需要 task_id");
                        }
                        ensure_task_owned_by_session(
                            manager.as_ref(),
                            &task_id,
                            session_filter.as_deref(),
                        )
                        .await?;
                        let new_id = manager.retry_tool(&task_id, tools).await?;
                        Ok(crate::runtime::tools::registry::ToolOutput::text(format!(
                            "✅ 已重新提交后台任务（新 id={}），完成后自动通知",
                            new_id
                        )))
                    }
                    other => anyhow::bail!("未知 action: {}", other),
                }
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tools::context::ToolExecutionContext;

    fn registry_with_tool() -> (Arc<ToolRegistry>, Arc<BackgroundTaskManager>) {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(
            "slow_read",
            "test tool",
            json!({"type": "object", "properties": {}}),
            Box::new(|_| Box::pin(async move { Ok("read-ok".to_string()) })),
        );
        let manager = Arc::new(BackgroundTaskManager::new(4));
        register(&registry, Arc::clone(&manager), Arc::clone(&registry));
        (registry, manager)
    }

    fn context(session: &str) -> ToolExecutionContext {
        ToolExecutionContext {
            session_id: Some(crate::agent_core::contracts::SessionId::new(session)),
            ..ToolExecutionContext::default()
        }
    }

    #[tokio::test]
    async fn list_is_scoped_to_current_session() {
        let (registry, manager) = registry_with_tool();
        manager
            .spawn_tool(
                "sess-a".into(),
                "slow_read".into(),
                "任务A".into(),
                json!({}),
                Arc::clone(&registry),
            )
            .await
            .unwrap();
        manager
            .spawn_tool(
                "sess-b".into(),
                "slow_read".into(),
                "任务B".into(),
                json!({}),
                Arc::clone(&registry),
            )
            .await
            .unwrap();

        let output = registry
            .execute_with_context_output(
                "background_tasks",
                &json!({"action": "list"}),
                &context("sess-a"),
            )
            .await
            .unwrap();
        assert!(output.content.contains("任务A"));
        assert!(!output.content.contains("任务B"));

        let all = registry
            .execute_with_context_output(
                "background_tasks",
                &json!({"action": "list", "all": true}),
                &context("sess-a"),
            )
            .await
            .unwrap();
        assert!(all.content.contains("任务A"));
        assert!(all.content.contains("任务B"));

        // 清理
        for task in manager.list().await {
            manager.cancel(&task.id).await;
        }
    }

    #[tokio::test]
    async fn cancel_running_task_via_tool() {
        let (registry, manager) = registry_with_tool();
        let id = manager
            .spawn(
                "sess-c".into(),
                "slow_read".into(),
                "慢任务".into(),
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    Ok("never".to_string())
                },
            )
            .await
            .unwrap();

        let output = registry
            .execute_with_context_output(
                "background_tasks",
                &json!({"action": "cancel", "task_id": id}),
                &context("sess-c"),
            )
            .await
            .unwrap();
        assert!(output.content.contains("已取消"));
    }

    #[tokio::test]
    async fn cannot_cancel_task_from_another_session() {
        let (registry, manager) = registry_with_tool();
        let id = manager
            .spawn(
                "sess-owner".into(),
                "slow_read".into(),
                "owner task".into(),
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    Ok("never".to_string())
                },
            )
            .await
            .unwrap();

        let result = registry
            .execute_with_context_output(
                "background_tasks",
                &json!({"action": "cancel", "task_id": id}),
                &context("sess-other"),
            )
            .await;
        assert!(result.is_err());
        assert!(manager.list().await.iter().any(|task| task.id == id));
        manager.cancel(&id).await;
    }

    #[tokio::test]
    async fn retry_failed_replayable_task() {
        let registry = Arc::new(ToolRegistry::new());
        // `read` 在幂等只读名单中，失败后可重放
        let flaky = Arc::new(std::sync::atomic::AtomicBool::new(true));
        registry.register(
            "read",
            "test tool",
            json!({"type": "object", "properties": {}}),
            Box::new(move |_| {
                let first = flaky.fetch_and(false, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    if first {
                        anyhow::bail!("boom");
                    }
                    Ok("recovered".to_string())
                })
            }),
        );
        let manager = Arc::new(BackgroundTaskManager::new(4));
        register(&registry, Arc::clone(&manager), Arc::clone(&registry));

        let id = manager
            .spawn_tool(
                "sess-d".into(),
                "read".into(),
                "失败任务".into(),
                json!({}),
                Arc::clone(&registry),
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let failed = manager
            .list()
            .await
            .into_iter()
            .find(|task| task.id == id)
            .expect("task recorded");
        assert_eq!(failed.success, Some(false));

        let cross_session = registry
            .execute_with_context_output(
                "background_tasks",
                &json!({"action": "retry", "task_id": id}),
                &context("sess-other"),
            )
            .await;
        assert!(cross_session.is_err());

        let output = registry
            .execute_with_context_output(
                "background_tasks",
                &json!({"action": "retry", "task_id": id}),
                &context("sess-d"),
            )
            .await
            .unwrap();
        assert!(output.content.contains("已重新提交"));

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let results = manager.list().await;
        assert!(results
            .iter()
            .any(|task| task.success == Some(true) && task.description == "失败任务"));

        // 非幂等工具失败后不可重放
        let non_replayable = manager
            .spawn("sess-d".into(), "write".into(), "写任务".into(), async {
                anyhow::bail!("denied")
            })
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let err = registry
            .execute_with_context_output(
                "background_tasks",
                &json!({"action": "retry", "task_id": non_replayable}),
                &context("sess-d"),
            )
            .await;
        assert!(err.is_err());
    }
}
