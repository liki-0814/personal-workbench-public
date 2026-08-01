//! manage_jobs 工具：通过 Jobs REST API 管理定时任务（含 agent 类型）。

use std::sync::Arc;

use serde_json::json;

use crate::backend::BackendClient;
use crate::tools::registry::{ToolImpact, ToolRegistry};

const DESCRIPTION: &str = r#"管理定时调度任务（cron jobs）。支持两种类型：
- type="command"：执行 shell 命令
- type="agent"：调用 AI agent 执行 prompt（带完整工具能力：联网搜索、读写文件、code_agent 等）

actions：
- list：列出所有任务及状态
- propose_create：提议新任务；daemon 真实测试通过后才创建
- propose_update：提议修改；daemon 真实测试通过后才原子替换
- delete：删除任务
- run：手动立即执行
- logs：查看执行日志

agent 类型任务会在 cron 触发时自动调 pwcli agent API 执行 prompt，结果记录到日志。"#;

fn job_impact(args: &serde_json::Value) -> ToolImpact {
    match args.get("action").and_then(|value| value.as_str()) {
        Some("list" | "logs") => ToolImpact::Observe,
        Some("delete") => ToolImpact::IrreversibleMutation,
        Some("propose_create" | "propose_update" | "run") => ToolImpact::ExternalSideEffect,
        _ => ToolImpact::ExternalSideEffect,
    }
}

pub fn register(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    let b = Arc::clone(&backend);

    registry.register_with_impact_resolver(
        "manage_jobs",
        DESCRIPTION,
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "propose_create", "propose_update", "delete", "run", "logs"],
                    "description": "操作类型"
                },
                "name": {
                    "type": "string",
                    "description": "任务名称（create/update/delete/run/logs 必填，仅字母数字下划线连字符点号）"
                },
                "cron": {
                    "type": "string",
                    "description": "5段 cron 表达式，如 '0 9 * * *'（每天9点）。create 必填。"
                },
                "type": {
                    "type": "string",
                    "enum": ["command", "agent"],
                    "description": "任务类型。command=shell命令，agent=AI agent 执行 prompt。create 必填。"
                },
                "command": {
                    "type": "string",
                    "description": "shell 命令（type=command 时必填）"
                },
                "prompt": {
                    "type": "string",
                    "description": "AI agent 任务描述（type=agent 时必填）。写清楚要做什么、结果存哪里。"
                },
                "model": {
                    "type": "string",
                    "description": "模型名（type=agent 时可选），如 opus、sonnet"
                },
                "cwd": {
                    "type": "string",
                    "description": "工作目录（type=agent 时可选），agent 的文件操作相对此目录"
                },
                "enabled": {
                    "type": "string",
                    "description": "是否启用，'true' 或 'false'"
                },
                "on_missed": {
                    "type": "string",
                    "enum": ["run_once", "skip"],
                    "description": "错过执行时的策略。run_once=补执行一次，skip=跳过"
                },
                "group": {
                    "type": "string",
                    "description": "分组名，同组任务串行执行"
                },
                "test_command": {
                    "type": "string",
                    "description": "command 类型可选的无副作用测试命令；未提供时会真实执行 command 并要求副作用确认"
                }
            },
            "required": ["action"]
        }),
        ToolImpact::ExternalSideEffect,
        job_impact,
        Box::new(move |args: &serde_json::Value| {
            let backend = Arc::clone(&b);
            let args = args.clone();
            Box::pin(async move {
                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");

                match action {
                    "list" => {
                        let res = backend.jobs_request("GET", "", None).await?;
                        Ok(serde_json::to_string_pretty(&res)?)
                    }
                    "propose_create" | "propose_update" => {
                        if name.is_empty() {
                            anyhow::bail!("name is required for job proposal");
                        }
                        let body = json!({
                            "name": name,
                            "operation": if action == "propose_update" { "update" } else { "create" },
                            "cron": args.get("cron").and_then(|v| v.as_str()).unwrap_or(""),
                            "type": args.get("type").and_then(|v| v.as_str()),
                            "command": args.get("command").and_then(|v| v.as_str()),
                            "prompt": args.get("prompt").and_then(|v| v.as_str()),
                            "model": args.get("model").and_then(|v| v.as_str()),
                            "cwd": args.get("cwd").and_then(|v| v.as_str()),
                            "enabled": args.get("enabled").and_then(|v| v.as_str()),
                            "on_missed": args.get("on_missed").and_then(|v| v.as_str()),
                            "group": args.get("group").and_then(|v| v.as_str()),
                            "testCommand": args.get("test_command").and_then(|v| v.as_str()),
                            "approvedTest": true,
                        });
                        let res = backend.jobs_request("POST", "/proposals", Some(&body)).await?;
                        Ok(serde_json::to_string_pretty(&res)?)
                    }
                    "delete" => {
                        if name.is_empty() {
                            anyhow::bail!("name is required for delete");
                        }
                        let path = format!("/{}", name);
                        let res = backend.jobs_request("DELETE", &path, None).await?;
                        Ok(serde_json::to_string_pretty(&res)?)
                    }
                    "run" => {
                        if name.is_empty() {
                            anyhow::bail!("name is required for run");
                        }
                        let path = format!("/{}/run", name);
                        let res = backend.jobs_request("POST", &path, None).await?;
                        Ok(serde_json::to_string_pretty(&res)?)
                    }
                    "logs" => {
                        if name.is_empty() {
                            anyhow::bail!("name is required for logs");
                        }
                        let path = format!("/{}/logs", name);
                        let res = backend.jobs_request("GET", &path, None).await?;
                        Ok(serde_json::to_string_pretty(&res)?)
                    }
                    _ => anyhow::bail!("unknown action: {}", action),
                }
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_quiet_but_tests_require_attention() {
        assert_eq!(
            job_impact(&json!({ "action": "list" })),
            ToolImpact::Observe
        );
        assert_eq!(
            job_impact(&json!({ "action": "logs" })),
            ToolImpact::Observe
        );
        assert_eq!(
            job_impact(&json!({ "action": "propose_create" })),
            ToolImpact::ExternalSideEffect
        );
        assert_eq!(
            job_impact(&json!({ "action": "delete" })),
            ToolImpact::IrreversibleMutation
        );
    }
}
