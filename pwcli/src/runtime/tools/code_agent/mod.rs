//! code_agent 工具：通过统一 ACP transport 调用原生 CLI 子 agent。
//!
//! 设计文档见 CLAUDE.md「pwcli Runtime」章节的「### code_agent 子 agent」。
//!
//! 当前支持实际暴露 ACP server 的 CLI；不为缺少 ACP 的 CLI 伪造能力。

pub mod acp_runner;
pub mod backend;
mod codex_backend;
mod decision;
mod kimi_backend;
mod prompt;
mod qoder_backend;
mod runner;

use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use tracing::info;

use crate::runtime::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

pub use backend::{AgentTransportKind, ModelInfo, PermissionModeInfo, SubAgentBackend};
pub use codex_backend::CodexBackend;
pub use decision::{parse_decision_marker, DECISION_END, DECISION_START};
pub use kimi_backend::KimiBackend;
pub use qoder_backend::QoderBackend;
pub use runner::{
    execute_code_agent, execute_code_agent_with_listener, CodeAgentArgs, CodeAgentResult,
    CodeAgentUsage, NativeSessionListener,
};

static NEXT_BACKEND: AtomicUsize = AtomicUsize::new(0);
pub const SESSION_STARTED_PROGRESS_PREFIX: &str = "CODE_AGENT_SESSION_STARTED ";

/// Detect one enabled backend for capability registration.
pub fn detect_backend() -> Option<Box<dyn SubAgentBackend>> {
    for backend in enabled_backend_names() {
        let detected = detect_named_backend(&backend);
        if detected.is_some() {
            return detected;
        }
    }
    None
}

fn detect_backend_for_call() -> Option<Box<dyn SubAgentBackend>> {
    let available = enabled_backend_names()
        .into_iter()
        .filter_map(|name| detect_named_backend(&name))
        .collect::<Vec<_>>();
    if available.is_empty() {
        return None;
    }
    let index = NEXT_BACKEND.fetch_add(1, Ordering::Relaxed) % available.len();
    available.into_iter().nth(index)
}

fn detect_named_backend(name: &str) -> Option<Box<dyn SubAgentBackend>> {
    match name {
        "qoder" => detect_qoder_binary()
            .map(|(bin, _)| Box::new(QoderBackend::new(bin)) as Box<dyn SubAgentBackend>),
        "codex" => detect_codex_backend()
            .map(|b| Box::new(b) as Box<dyn SubAgentBackend>),
        "kimi" => detect_kimi_binary()
            .map(|(bin, _)| Box::new(KimiBackend::new(bin)) as Box<dyn SubAgentBackend>),
        _ => None,
    }
}

pub fn enabled_backend_names() -> Vec<String> {
    let delegation = crate::runtime::settings::local_config::get()
        .tools
        .delegation;
    ordered_enabled_backends(&delegation.enabled_executors, &delegation.cli_priority)
}

fn normalize_enabled_backends(configured: &[String]) -> Vec<String> {
    let mut enabled = Vec::new();
    for value in configured {
        let canonical = match value.trim().to_ascii_lowercase().as_str() {
            "codex" => "codex",
            "qoder" | "qodercli" => "qoder",
            "kimi" | "kimi-code" => "kimi",
            _ => continue,
        };
        if !enabled.iter().any(|entry| entry == canonical) {
            enabled.push(canonical.to_string());
        }
    }
    enabled
}

fn ordered_enabled_backends(enabled_executors: &[String], cli_priority: &[String]) -> Vec<String> {
    let enabled = normalize_enabled_backends(enabled_executors);
    let priority = normalize_enabled_backends(cli_priority);
    let mut ordered = priority
        .into_iter()
        .filter(|candidate| enabled.contains(candidate))
        .collect::<Vec<_>>();
    for candidate in enabled {
        if !ordered.contains(&candidate) {
            ordered.push(candidate);
        }
    }
    ordered
}

pub fn backend_is_enabled(name: &str) -> bool {
    enabled_backend_names()
        .iter()
        .any(|enabled| enabled == name)
}

#[cfg(test)]
mod backend_candidate_tests {
    use super::{normalize_enabled_backends, ordered_enabled_backends};

    #[test]
    fn configured_backends_keep_user_order() {
        assert_eq!(
            normalize_enabled_backends(&["qoder".into(), "codex".into()]),
            ["qoder", "codex"]
        );
        assert_eq!(
            normalize_enabled_backends(&["codex".into(), "missing".into()]),
            ["codex"]
        );
    }

    #[test]
    fn explicit_empty_enabled_list_disables_all_cli_backends() {
        assert!(normalize_enabled_backends(&[]).is_empty());
    }

    #[test]
    fn delegation_priority_orders_only_enabled_backends() {
        assert_eq!(
            ordered_enabled_backends(
                &["pwcli".into(), "qoder".into(), "codex".into()],
                &["codex".into(), "kimi".into(), "qoder".into()],
            ),
            ["codex", "qoder"]
        );
    }
}

/// Detect qodercli binary; returns (path, version) or None.
pub fn detect_qoder_binary() -> Option<(String, String)> {
    detect_binary_version("qodercli", &["qodercli"])
}

/// Detect codex binary; returns (path, version) or None.
pub fn detect_codex_binary() -> Option<(String, String)> {
    detect_binary_version("codex", &["codex"])
}

pub fn detect_codex_acp_binary() -> Option<(String, String)> {
    // ACP adapters are stdio servers; some versions do not implement `--version` and would
    // wait for protocol input forever. Executability is the capability check here.
    which_binary("codex-acp", &["codex-acp"]).map(|bin| (bin, String::new()))
}

/// Detect a usable Codex ACP launcher. Prefers an installed `codex-acp` adapter;
/// falls back to the auto-refresh launcher (re-installs the newest adapter via
/// npm before every launch) when the Codex CLI itself is present.
fn detect_codex_backend() -> Option<CodexBackend> {
    if let Some((bin, _)) = detect_codex_acp_binary() {
        return Some(CodexBackend::new(bin));
    }
    if detect_codex_binary().is_some() && which_binary("npm", &["npm"]).is_some() {
        return Some(CodexBackend::auto());
    }
    None
}

pub fn detect_kimi_binary() -> Option<(String, String)> {
    detect_binary_version("kimi", &["kimi"])
}

fn detect_binary_version(name: &str, candidates: &[&str]) -> Option<(String, String)> {
    let bin = which_binary(name, candidates)?;
    let output = StdCommand::new(&bin)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some((bin, version))
}

/// Scan PATH + common fallback locations for a binary by name.
fn which_binary(name: &str, extra_names: &[&str]) -> Option<String> {
    // Scan PATH
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for n in std::iter::once(name).chain(extra_names.iter().copied()) {
                let p = dir.join(n);
                if p.is_file() {
                    return Some(p.to_string_lossy().to_string());
                }
            }
        }
    }

    // Fallback absolute paths (DMG / launchd environments with minimal PATH)
    let home = dirs::home_dir()?;
    let fallbacks = [
        home.join(format!(".npm-global/bin/{}", name)),
        home.join(format!(".volta/bin/{}", name)),
        home.join(format!(".bun/bin/{}", name)),
        home.join(format!(".local/bin/{}", name)),
        home.join(format!(".kimi-code/bin/{}", name)),
        PathBuf::from(format!("/opt/homebrew/bin/{}", name)),
        PathBuf::from(format!("/usr/local/bin/{}", name)),
    ];
    fallbacks
        .iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
}

/// Discover all available backends with their model lists.
/// Used by the service layer to expose `GET /api/agent/backends`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendInfo {
    pub name: String,
    pub available: bool,
    pub transport: AgentTransportKind,
    pub models: Vec<ModelInfo>,
    pub permission_modes: Vec<PermissionModeInfo>,
    pub install_hint: String,
}

pub fn list_available_backends(include_models: bool) -> Vec<BackendInfo> {
    let mut backends = Vec::new();

    let qoder_avail = if include_models {
        detect_qoder_binary()
    } else {
        which_binary("qodercli", &["qodercli"]).map(|bin| (bin, String::new()))
    };
    if let Some((bin, _)) = &qoder_avail {
        let b = QoderBackend::new(bin.clone());
        backends.push(BackendInfo {
            name: "qoder".to_string(),
            available: true,
            transport: b.transport_kind(),
            models: if include_models {
                b.list_models()
            } else {
                Vec::new()
            },
            permission_modes: b.permission_modes(),
            install_hint: b.install_hint().to_string(),
        });
    } else {
        backends.push(BackendInfo {
            name: "qoder".to_string(),
            available: false,
            transport: AgentTransportKind::Acp,
            models: Vec::new(),
            permission_modes: Vec::new(),
            install_hint: "npm install -g @anthropic-ai/qodercli".to_string(),
        });
    }

    let codex_avail = detect_codex_backend();
    if let Some(b) = &codex_avail {
        backends.push(BackendInfo {
            name: "codex".to_string(),
            available: true,
            transport: b.transport_kind(),
            models: if include_models {
                b.list_models()
            } else {
                Vec::new()
            },
            permission_modes: b.permission_modes(),
            install_hint: b.install_hint().to_string(),
        });
    } else {
        backends.push(BackendInfo {
            name: "codex".to_string(),
            available: false,
            transport: AgentTransportKind::Acp,
            models: Vec::new(),
            permission_modes: Vec::new(),
            install_hint: if detect_codex_binary().is_some() {
                "Codex is installed but neither codex-acp nor npm is available; run npm install -g @agentclientprotocol/codex-acp".to_string()
            } else {
                "Install Codex CLI and run npm install -g @agentclientprotocol/codex-acp".to_string()
            },
        });
    }

    let kimi_avail = if include_models {
        detect_kimi_binary()
    } else {
        which_binary("kimi", &["kimi"]).map(|bin| (bin, String::new()))
    };
    if let Some((bin, _)) = &kimi_avail {
        let backend = KimiBackend::new(bin.clone());
        backends.push(BackendInfo {
            name: "kimi".to_string(),
            available: true,
            transport: backend.transport_kind(),
            models: if include_models {
                backend.list_models()
            } else {
                Vec::new()
            },
            permission_modes: backend.permission_modes(),
            install_hint: backend.install_hint().to_string(),
        });
    } else {
        backends.push(BackendInfo {
            name: "kimi".to_string(),
            available: false,
            transport: AgentTransportKind::Acp,
            models: Vec::new(),
            permission_modes: Vec::new(),
            install_hint: "Install Kimi Code from https://moonshotai.github.io/kimi-code/"
                .to_string(),
        });
    }

    backends
}

/// 推断 sandbox 根目录：`tools.fsBase` (config.json) > `$HOME`.
pub fn resolve_sandbox_root() -> PathBuf {
    let from_cfg = crate::runtime::settings::local_config::get().tools.fs_base;
    if !from_cfg.is_empty() {
        return expand_tilde(&from_cfg);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_tilde(s: &str) -> PathBuf {
    let s = s.trim();
    if s == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// 注册 `code_agent` 工具。接受一个 backend 实例。
pub fn register(
    registry: &mut ToolRegistry,
    _backend: Arc<dyn SubAgentBackend>,
    sandbox_root: Arc<PathBuf>,
) {
    let root = Arc::clone(&sandbox_root);

    registry.register_structured_with_impact(
        "code_agent",
        DESCRIPTION,
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "用自然语言描述要做的事；包含必要上下文（关注哪些文件、目标是什么）。子 agent 看不到 pwcli 当前对话，所以要写完整。"
                },
                "cwd": {
                    "type": "string",
                    "description": format!(
                        "项目路径（必须在沙箱根目录 {} 下）。支持绝对路径和 ~/...。用户给出 ~/... 时保持原样，不要猜测或将 ~ 展开成 /root。子 agent 在此目录下启动并读取项目级规则。",
                        root.display()
                    )
                },
                "backend": {
                    "type": "string",
                    "enum": ["codex", "qoder", "kimi"],
                    "description": "指定本次使用的编码 CLI。用户明确点名或 @codex/@qoder/@kimi 时必须传对应值；省略时才从设置中已启用且本机可用的 CLI 动态选择。"
                },
                "mode": {
                    "type": "string",
                    "enum": ["research", "edit"],
                    "description": "edit（默认）：子 agent 可读可写；research：只读不写盘，仅当用户明确说『只调研不要改文件』时才传。多数任务（写代码、产出文档/论文、生成报告）都应是 edit。"
                },
                "effort": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "xhigh", "max"],
                    "description": "推理深度。简单调研用 low，常规任务 medium（默认），复杂重构/调试用 high/xhigh/max。仅 Ultimate/Performance 支持 xhigh。"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "总超时秒数（含重试）。默认 600，硬上限 1800。"
                },
                "resume_session_id": {
                    "type": "string",
                    "description": "上次返回 status='decision_required' 时给的 session_id。续聊时填，task 字段写明你的决定。"
                },
                "model": {
                    "type": "string",
                    "description": "子 agent 使用的模型。具体值来自 /api/agent/backends；新会话省略时 code_agent 会先暂停并让用户选择，选择后必须用该 model 重新调用。续聊无需重复传。"
                },
                "context_window": {
                    "type": "integer",
                    "description": "上下文窗口大小（token 数，正整数）。仅 qoder 后端支持（--context-window）。如 128000、200000。省略时用后端默认。"
                },
                "permission_mode": {
                    "type": "string",
                    "description": "CLI 原生权限模式（可选）。可用值由 /api/agent/backends 的 permissionModes 返回；不同后端的值不可混用。"
                },
                "spec": {
                    "type": "string",
                    "description": "任务规格说明（可选）。包含：目标、约束条件、验收标准。当主 agent 拆解复杂任务时，将结构化规格传给子 agent，比裸 task 文本更精确。"
                },
                "project_rules": {
                    "type": "string",
                    "description": "项目约束规则（可选）。从 CLAUDE.md 或用户指示中提取的关键约束（如『不用 Context API』『所有 state 走 core/storage』），让子 agent 遵守项目规范。"
                }
            },
            "required": ["task", "cwd"]
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::ReversibleMutation,
        Box::new(move |args: &serde_json::Value| {
            let root = Arc::clone(&root);
            let args = args.clone();
            Box::pin(async move {
                let mut parsed: CodeAgentArgs = serde_json::from_value(args)
                    .map_err(|e| anyhow::anyhow!("code_agent 参数错误: {}", e))?;
                // An explicit per-call backend must never silently fall back to
                // the configured CLI: that could send a task to the wrong provider.
                let backend: Arc<dyn SubAgentBackend> = match parsed.backend.as_deref() {
                    Some("codex") if backend_is_enabled("codex") => detect_codex_backend()
                        .map(|b| Arc::new(b) as Arc<dyn SubAgentBackend>)
                        .ok_or_else(|| anyhow::anyhow!("Codex CLI 不可用（需要 codex + npx，或已安装 codex-acp）"))?,
                    Some("qoder") if backend_is_enabled("qoder") => detect_qoder_binary()
                        .map(|(bin, _)| Arc::new(QoderBackend::new(bin)) as Arc<dyn SubAgentBackend>)
                        .ok_or_else(|| anyhow::anyhow!("QoderCLI 未安装或不可用"))?,
                    Some("kimi") if backend_is_enabled("kimi") => detect_kimi_binary()
                        .map(|(bin, _)| Arc::new(KimiBackend::new(bin)) as Arc<dyn SubAgentBackend>)
                        .ok_or_else(|| anyhow::anyhow!("Kimi Code 未安装或不可用"))?,
                    Some("codex" | "qoder" | "kimi") => {
                        return Err(anyhow::anyhow!(
                            "请求的 code_agent backend 已在设置中停用"
                        ));
                    }
                    Some(value) => {
                        return Err(anyhow::anyhow!("不支持的 code_agent backend: {value}"));
                    }
                    None => detect_backend_for_call()
                        .map(Arc::from)
                        .ok_or_else(|| {
                            anyhow::anyhow!("设置中启用的编码 CLI 均未安装或不可用")
                        })?,
                };
                if parsed.resume_session_id.is_none() && parsed.model.is_none() {
                    let models = backend.list_models();
                    if !models.is_empty() {
                        let options = models
                            .iter()
                            .map(|model| {
                                let label = if model.display_name.is_empty() {
                                    model.id.clone()
                                } else {
                                    model.display_name.clone()
                                };
                                json!({
                                    "id": model.id,
                                    "label": label,
                                    "description": format!(
                                        "{}{}",
                                        model.id,
                                        if model.default_effort.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" · 默认推理强度 {}", model.default_effort)
                                        }
                                    )
                                })
                            })
                            .collect::<Vec<_>>();
                        return Ok(ToolOutput {
                            content: format!(
                                "等待用户选择 {} 模型。收到选择后，用 model=所选 option id 重新调用 code_agent；不要自行替用户选择。",
                                backend.name()
                            ),
                            terminate: true,
                            details: Some(json!({
                                "decisionPrompt": {
                                    "id": uuid::Uuid::new_v4().simple().to_string(),
                                    "title": format!("选择 {} 模型", backend.name()),
                                    "rationale": "该选择仅用于这次新建的 CLI 协作会话。",
                                    "options": options,
                                    "step": 1,
                                    "total": 1,
                                    "allowCustom": false,
                                    "allowSkip": false
                                }
                            })),
                            added_tool_names: Vec::new(),
                        });
                    }
                }
                if parsed.permission_mode.is_none() {
                    parsed.permission_mode = default_permission_mode(backend.as_ref());
                }
                let objective = parsed.task.clone();
                let cwd = parsed.cwd.clone();
                let collaboration_cwd = std::fs::canonicalize(expand_tilde(&cwd))
                    .unwrap_or_else(|_| expand_tilde(&cwd));
                let mode = parsed.mode.clone().unwrap_or_else(|| "edit".into());
                let model = parsed.model.clone();
                let effort = parsed.effort.clone();
                let context_window = parsed.context_window;
                let permission_mode = parsed.permission_mode.clone();
                let backend_name = backend.name().to_string();
                let listener_objective = objective.clone();
                let listener_cwd = collaboration_cwd.clone();
                let listener_backend = backend_name.clone();
                let listener_mode = mode.clone();
                let listener_model = model.clone();
                let listener_effort = effort.clone();
                let listener_permission_mode = permission_mode.clone();
                let listener: NativeSessionListener = Arc::new(move |native_session_id| {
                    let metadata = json!({
                        "nativeSessionId": native_session_id,
                        "backend": listener_backend,
                        "objective": listener_objective,
                        "cwd": listener_cwd,
                        "mode": listener_mode,
                        "status": "running",
                        "output": "",
                        "model": listener_model,
                        "effort": listener_effort,
                        "contextWindow": context_window,
                        "permissionMode": listener_permission_mode
                    });
                    crate::runtime::tools::progress::emit(&format!(
                        "{SESSION_STARTED_PROGRESS_PREFIX}{metadata}"
                    ));
                });
                let result = execute_code_agent_with_listener(
                    parsed,
                    backend.as_ref(),
                    &root,
                    Some(listener),
                )
                .await?;
                info!(
                    status = %result.status,
                    session_id = %result.session_id,
                    attempts = result.attempts,
                    duration_ms = result.duration_ms,
                    cost_usd = ?result.cost_usd,
                    "code_agent finished"
                );
                let content = serde_json::to_string_pretty(&result)?;
                Ok(ToolOutput {
                    content,
                    terminate: code_agent_result_terminates(&result.status),
                    details: Some(json!({
                        "codeAgentSession": {
                            "nativeSessionId": result.session_id,
                            "backend": backend_name,
                            "objective": objective,
                            "cwd": collaboration_cwd,
                            "mode": mode,
                            "status": result.status,
                            "output": result.output,
                            "model": model,
                            "effort": effort,
                            "contextWindow": context_window,
                            "permissionMode": permission_mode
                        }
                    })),
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
}

fn code_agent_result_terminates(status: &str) -> bool {
    matches!(status, "error" | "timeout")
}

fn default_permission_mode(backend: &dyn SubAgentBackend) -> Option<String> {
    let preferred = match backend.name() {
        "qoder" => "bypass_permissions",
        "codex" => "bypass",
        "kimi" => "yolo",
        _ => return None,
    };
    backend
        .permission_modes()
        .iter()
        .any(|mode| mode.id == preferred)
        .then(|| preferred.to_string())
}

const DESCRIPTION: &str = r##"将代码相关任务委托给本机的编码 CLI 子 agent。

用途：
- 调研某个仓库 / 模块的设计、调用关系、关键抽象
- 跨文件理解、找 bug、写代码、做重构
- 复杂时子 agent 会自动用它内置的 skills

不要用于：
- 读单个 .md 看看（用 read）
- 列一个目录（用 ls）
- 工作台数据 CRUD（用 data_crud）

返回 JSON，字段：
- status: 'ok' | 'decision_required' | 'timeout' | 'error'
- output: 子 agent 最终输出（markdown）
- session_id: 始终返回，给续聊用
- question / options: 仅 status='decision_required' 时
- cost_usd, duration_ms, attempts

mode 默认 'edit'：子 agent 直接读写文件；如果用户明说"只调研别改"再传 'research'。

新建会话必须由用户选择该 CLI 的模型。首次调用省略 model，code_agent 会返回选择卡片并暂停；
用户选择后，从 `pwb-user-choice` 标记读取 option id，作为 model 原样重新调用。续聊使用
resume_session_id，不再次询问模型。

决策上浮处理：
- status='decision_required' 时：先看你能否凭已有信息（用户原始诉求 + 当前对话）合理决定。
  能决定 → 用同一 session_id 再调，task 写明决定。
  不能决定 → 转告用户，让他选。不要每次都甩给用户。
- status='timeout': 部分输出在 output 里。可调大 timeout_secs 重试或拆任务。
- status='error': 见 output 字段。多半是子 agent 调用失败，告知用户。

实现后验证：
复杂编码任务（跨文件重构、新 feature）完成后，建议用同一 cwd 再调一次 code_agent（mode='research'），
task 写"审查上述变更是否满足原始要求，检查遗漏和安全隐患"。简单任务不必验证。
"##;

#[cfg(test)]
mod tests {
    use super::{
        code_agent_result_terminates, default_permission_mode, CodexBackend, KimiBackend,
        QoderBackend,
    };

    #[test]
    fn failed_or_timed_out_code_agent_ends_the_parent_turn() {
        assert!(code_agent_result_terminates("error"));
        assert!(code_agent_result_terminates("timeout"));
        assert!(!code_agent_result_terminates("ok"));
        assert!(!code_agent_result_terminates("decision_required"));
    }

    #[test]
    fn new_native_sessions_use_the_backend_bypass_mode() {
        assert_eq!(
            default_permission_mode(&QoderBackend::new("qoder".into())).as_deref(),
            Some("bypass_permissions")
        );
        assert_eq!(
            default_permission_mode(&CodexBackend::new("codex".into())).as_deref(),
            Some("bypass")
        );
        assert_eq!(
            default_permission_mode(&KimiBackend::new("kimi".into())).as_deref(),
            Some("yolo")
        );
    }
}
