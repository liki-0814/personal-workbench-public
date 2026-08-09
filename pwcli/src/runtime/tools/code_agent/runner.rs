//! code_agent 原生协议执行核心：统一 ACP、重试、决策上浮和沙箱校验。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::ai::llm::retry::{is_retriable_subprocess_failure, next_backoff};
use crate::runtime::tools::code_agent::acp_runner::{run_acp_turn, AcpTurnRequest};
use crate::runtime::tools::code_agent::backend::{AgentTransportKind, SubAgentBackend};
use crate::runtime::tools::code_agent::decision::parse_decision_marker;
use crate::runtime::tools::code_agent::prompt::build_subagent_addendum;

const DEFAULT_TIMEOUT_SECS: u64 = 600;
const MAX_TIMEOUT_SECS: u64 = 1800;
// The persistent Supervisor plan owns retries so daemon recovery and Attention can audit them.
// A nested retry loop here would multiply two plan attempts into four opaque CLI launches.
const MAX_ACP_ATTEMPTS: u32 = 1;
// Keep enough text for the configured 128K-token child output ceiling. The
// conversation store needs the real CLI answer; UI rendering can virtualize it.
const OUTPUT_TRUNCATE_CHARS: usize = 512_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeAgentArgs {
    pub task: String,
    pub cwd: String,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub resume_session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub spec: Option<String>,
    #[serde(default)]
    pub project_rules: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeAgentResult {
    pub status: String,
    pub session_id: String,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<CodeAgentUsage>,
    pub duration_ms: u64,
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CodeAgentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

pub type NativeSessionListener = Arc<dyn Fn(String) + Send + Sync>;

/// 单次原生协议调用结果。
#[derive(Debug, Default)]
struct ParsedRun {
    final_text: String,
    is_error: bool,
    cost_usd: Option<f64>,
    usage: Option<CodeAgentUsage>,
    native_session_id: Option<String>,
}

pub async fn execute_code_agent(
    args: CodeAgentArgs,
    backend: &dyn SubAgentBackend,
    sandbox_root: &Path,
) -> Result<CodeAgentResult> {
    execute_code_agent_with_listener(args, backend, sandbox_root, None).await
}

pub async fn execute_code_agent_with_listener(
    args: CodeAgentArgs,
    backend: &dyn SubAgentBackend,
    sandbox_root: &Path,
    native_session_listener: Option<NativeSessionListener>,
) -> Result<CodeAgentResult> {
    let cwd = sanitize_cwd(&args.cwd, sandbox_root)?;

    let mode = args.mode.as_deref().unwrap_or("edit").to_string();
    if !matches!(mode.as_str(), "edit" | "research") {
        return Err(anyhow!(
            "invalid mode: {} (expected 'research' or 'edit')",
            mode
        ));
    }

    // Apply defaults (per-call args win, then config.json).
    let cfg_code_agent = crate::runtime::settings::local_config::get()
        .tools
        .code_agent;
    let use_backend_defaults = same_backend(&cfg_code_agent.backend, backend.name());
    let cfg_effort = if !use_backend_defaults || cfg_code_agent.effort.trim().is_empty() {
        None
    } else {
        Some(cfg_code_agent.effort.clone())
    };
    let cfg_model = if !use_backend_defaults || cfg_code_agent.model.trim().is_empty() {
        None
    } else {
        Some(cfg_code_agent.model.clone())
    };
    let cfg_ctx: Option<u32> = if use_backend_defaults
        && cfg_code_agent.context_window > 0
        && cfg_code_agent.context_window <= u64::from(u32::MAX)
    {
        Some(cfg_code_agent.context_window as u32)
    } else {
        None
    };

    let requested_effort = args.effort.or(cfg_effort);
    if requested_effort
        .as_deref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 64)
    {
        return Err(anyhow!(
            "invalid effort: expected a non-empty value up to 64 bytes"
        ));
    }

    let model = args.model.or(cfg_model);
    let effort = supported_model_effort(
        &backend.list_models(),
        model.as_deref(),
        requested_effort.as_deref(),
    );
    if requested_effort.is_some() && effort.is_none() {
        crate::runtime::tools::progress::emit(
            "所选模型未声明该 reasoning effort；使用 CLI 的模型默认值",
        );
    }
    let context_window = args.context_window.or(cfg_ctx);
    let permission_mode = args.permission_mode;
    let timeout_secs = args
        .timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(10, MAX_TIMEOUT_SECS);

    let session_id = args
        .resume_session_id
        .clone()
        .unwrap_or_else(generate_uuid_v4);
    let is_resume = args.resume_session_id.is_some();

    let addendum = build_subagent_addendum(
        timeout_secs,
        args.spec.as_deref(),
        args.project_rules.as_deref(),
    );
    let started = Instant::now();
    let mut total_cost: f64 = 0.0;

    info!(
        backend = backend.name(),
        cwd = %cwd.display(),
        mode = mode,
        effort = ?effort,
        timeout_secs,
        session_id = %session_id,
        is_resume,
        "code_agent dispatched"
    );

    let total_deadline = started + Duration::from_secs(timeout_secs);

    let result = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        let mut attempt: u32 = 0;
        loop {
            let remaining_secs = total_deadline
                .checked_duration_since(Instant::now())
                .map(|d| d.as_secs().max(5))
                .unwrap_or(5);

            match run_once(
                backend,
                &args.task,
                &cwd,
                &session_id,
                is_resume,
                &mode,
                effort.as_deref(),
                &addendum,
                model.as_deref(),
                context_window,
                permission_mode.as_deref(),
                remaining_secs,
                native_session_listener.clone(),
            )
            .await
            {
                Ok(parsed) => {
                    if let Some(c) = parsed.cost_usd {
                        total_cost += c;
                    }
                    return Ok((parsed, attempt + 1));
                }
                Err(RunError::RetriableSubprocess { combined }) => {
                    if should_retry_acp(attempt + 1) {
                        if let Some(delay) = next_backoff(attempt) {
                            warn!(
                                attempt = attempt + 1,
                                max = MAX_ACP_ATTEMPTS,
                                delay_ms = delay.as_millis() as u64,
                                snippet = %first_chars(&combined, 200),
                                "code_agent: retriable subprocess error, retrying"
                            );
                            attempt += 1;
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                    return Err(RunError::RetryExhausted { combined });
                }
                Err(other) => return Err(other),
            }
        }
    })
    .await;

    let duration_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(Ok((parsed, attempts))) => {
            let returned_session_id = parsed
                .native_session_id
                .clone()
                .unwrap_or_else(|| session_id.clone());
            let usage = parsed.usage;
            let raw_text = parsed.final_text;
            if let Some(decision) = parse_decision_marker(&raw_text) {
                Ok(CodeAgentResult {
                    status: "decision_required".to_string(),
                    session_id: returned_session_id,
                    output: truncate_middle(&decision.stripped_output, OUTPUT_TRUNCATE_CHARS),
                    question: Some(decision.question),
                    options: Some(decision.options),
                    cost_usd: Some(total_cost),
                    usage,
                    duration_ms,
                    attempts,
                })
            } else {
                Ok(CodeAgentResult {
                    status: if parsed.is_error {
                        "error".to_string()
                    } else {
                        "ok".to_string()
                    },
                    session_id: returned_session_id,
                    output: truncate_middle(&raw_text, OUTPUT_TRUNCATE_CHARS),
                    question: None,
                    options: None,
                    cost_usd: Some(total_cost),
                    usage,
                    duration_ms,
                    attempts,
                })
            }
        }
        Ok(Err(RunError::RetryExhausted { combined })) => Ok(CodeAgentResult {
            status: "error".to_string(),
            session_id,
            output: format!(
                "{} 子 agent provider 调用失败（{} 次尝试）:\n{}",
                backend.name(),
                MAX_ACP_ATTEMPTS,
                truncate_middle(&combined, OUTPUT_TRUNCATE_CHARS - 80)
            ),
            question: None,
            options: None,
            cost_usd: Some(total_cost),
            usage: None,
            duration_ms,
            attempts: MAX_ACP_ATTEMPTS,
        }),
        Ok(Err(RunError::RetriableSubprocess { .. })) => {
            unreachable!("RetriableSubprocess should be handled inside retry loop")
        }
        Ok(Err(RunError::FatalSubprocess {
            combined,
            exit_code,
        })) => Ok(CodeAgentResult {
            status: "error".to_string(),
            session_id,
            output: format!(
                "{} 子进程 exit {}: {}",
                backend.name(),
                exit_code,
                truncate_middle(&combined, OUTPUT_TRUNCATE_CHARS - 80)
            ),
            question: None,
            options: None,
            cost_usd: Some(total_cost),
            usage: None,
            duration_ms,
            attempts: 1,
        }),
        Err(_) => Ok(CodeAgentResult {
            status: "timeout".to_string(),
            session_id: session_id.clone(),
            output: format!(
                "code_agent 在 {}s 内未完成（含重试）。子 agent 会话上下文已保留（session_id={}），\
                 无需从零重跑：用 resume_session_id={} 续聊接力即可从断点继续（可调大 timeout_secs），\
                 或拆小任务重新委托。",
                timeout_secs, session_id, session_id
            ),
            question: None,
            options: None,
            cost_usd: Some(total_cost),
            usage: None,
            duration_ms,
            attempts: 0,
        }),
    }
}

fn should_retry_acp(failed_attempts: u32) -> bool {
    failed_attempts < MAX_ACP_ATTEMPTS
}

fn same_backend(configured: &str, active: &str) -> bool {
    fn canonical(value: &str) -> String {
        match value.trim().to_ascii_lowercase().as_str() {
            "qodercli" | "qoder-cli" => "qoder".into(),
            "kimi-code" | "kimi_code" => "kimi".into(),
            "codex-acp" => "codex".into(),
            value => value.into(),
        }
    }
    canonical(configured) == canonical(active)
}

fn supported_model_effort(
    models: &[super::backend::ModelInfo],
    model: Option<&str>,
    requested: Option<&str>,
) -> Option<String> {
    let requested = requested?;
    let model = model?;
    let capabilities = models.iter().find(|candidate| candidate.id == model)?;
    capabilities
        .effort_options
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(requested))
        .cloned()
}

#[derive(Debug)]
enum RunError {
    RetriableSubprocess { combined: String },
    RetryExhausted { combined: String },
    FatalSubprocess { combined: String, exit_code: i32 },
}

/// Run a backend's optional refresh step before launching its ACP process.
/// Refresh failures fall back to the previously installed binary when present,
/// so an offline daemon keeps working with the last known adapter.
async fn refresh_backend(backend: &dyn SubAgentBackend) -> Result<(), RunError> {
    let Some((program, args)) = backend.prepare_command() else {
        return Ok(());
    };
    crate::runtime::tools::progress::emit(&format!(
        "正在刷新 {} ACP 适配器（npm install）…",
        backend.name()
    ));
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(&program)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await;
    let output = match output {
        Err(_) => return refresh_failure(backend, "timed out"),
        Ok(Err(error)) => return refresh_failure(backend, &format!("failed to start: {error}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if Path::new(backend.bin_path()).is_file() {
        warn!(backend = backend.name(), %stderr, "adapter refresh failed; falling back to installed binary");
        return Ok(());
    }
    let tail = stderr
        .chars()
        .rev()
        .take(400)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    Err(RunError::FatalSubprocess {
        combined: format!("{} adapter refresh failed: {tail}", backend.name()),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Map a refresh failure: reuse the previously installed binary when present,
/// otherwise surface a fatal error.
fn refresh_failure(backend: &dyn SubAgentBackend, reason: &str) -> Result<(), RunError> {
    if Path::new(backend.bin_path()).is_file() {
        warn!(
            backend = backend.name(),
            reason, "adapter refresh failed; falling back to installed binary"
        );
        return Ok(());
    }
    Err(RunError::FatalSubprocess {
        combined: format!("{} adapter refresh {reason}", backend.name()),
        exit_code: -1,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_once(
    backend: &dyn SubAgentBackend,
    task: &str,
    cwd: &Path,
    session_id: &str,
    is_resume: bool,
    mode: &str,
    effort: Option<&str>,
    system_addendum: &str,
    model: Option<&str>,
    context_window: Option<u32>,
    permission_mode: Option<&str>,
    remaining_secs: u64,
    native_session_listener: Option<NativeSessionListener>,
) -> Result<ParsedRun, RunError> {
    if backend.transport_kind() == AgentTransportKind::Acp {
        refresh_backend(backend).await?;
        let (program, program_args) =
            backend
                .acp_command()
                .ok_or_else(|| RunError::FatalSubprocess {
                    combined: format!("{} 声明 ACP transport 但未提供启动命令", backend.name()),
                    exit_code: -1,
                })?;
        let effective_task = if system_addendum.is_empty() {
            task.to_string()
        } else {
            format!("{system_addendum}\n\n---\n\n{task}")
        };
        let output = run_acp_turn(AcpTurnRequest {
            program: &program,
            program_args: &program_args,
            cwd,
            task: &effective_task,
            resume_session_id: is_resume.then_some(session_id),
            mode,
            permission_mode,
            model,
            effort,
            context_window,
            timeout: Duration::from_secs(remaining_secs),
            native_session_listener,
        })
        .await
        .map_err(|error| {
            let combined = format!("ACP transport failed: {error:#}");
            if is_retriable_subprocess_failure(&combined) {
                RunError::RetriableSubprocess { combined }
            } else {
                RunError::FatalSubprocess {
                    combined,
                    exit_code: -1,
                }
            }
        })?;
        return Ok(ParsedRun {
            final_text: output.output,
            native_session_id: Some(output.session_id),
            usage: output.usage,
            ..ParsedRun::default()
        });
    }

    Err(RunError::FatalSubprocess {
        combined: format!("{} 未声明 ACP transport", backend.name()),
        exit_code: -1,
    })
}

/// `cwd` 必须 canonicalize 后落在 sandbox_root 之下。
fn sanitize_cwd(cwd: &str, sandbox_root: &Path) -> Result<PathBuf> {
    let path = if cwd.starts_with('~') {
        let home = dirs::home_dir().context("home dir not found")?;
        home.join(cwd.trim_start_matches('~').trim_start_matches('/'))
    } else {
        PathBuf::from(cwd)
    };
    if !path.is_absolute() {
        return Err(anyhow!("cwd must be absolute: {}", cwd));
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("无法 canonicalize cwd: {}", cwd))?;
    let root_canonical = sandbox_root
        .canonicalize()
        .unwrap_or_else(|_| sandbox_root.to_path_buf());
    if !canonical.starts_with(&root_canonical) {
        return Err(anyhow!(
            "cwd '{}' 不在沙箱根 '{}' 之下",
            canonical.display(),
            root_canonical.display()
        ));
    }
    if !canonical.is_dir() {
        return Err(anyhow!("cwd 不是目录: {}", canonical.display()));
    }
    Ok(canonical)
}

pub(crate) fn truncate_middle(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head_n = max_chars / 2;
    let tail_n = max_chars - head_n;
    let chars: Vec<char> = s.chars().collect();
    let head: String = chars.iter().take(head_n).collect();
    let tail: String = chars
        .iter()
        .rev()
        .take(tail_n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(
        "{}\n\n... [{} chars truncated] ...\n\n{}",
        head,
        chars.len() - max_chars,
        tail
    )
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn generate_uuid_v4() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3],
        b[4], b[5],
        b[6], b[7],
        b[8], b[9],
        b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

#[cfg(test)]
mod runner_tests {
    use super::*;

    #[test]
    fn truncate_middle_keeps_head_and_tail() {
        let s = "a".repeat(100);
        let out = truncate_middle(&s, 20);
        assert!(out.contains("[80 chars truncated]"));
        assert!(out.starts_with("aaaa"));
    }

    #[test]
    fn truncate_middle_passthrough_short() {
        assert_eq!(truncate_middle("hello", 100), "hello");
    }

    #[test]
    fn uuid_v4_format() {
        let u = generate_uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn sanitize_cwd_rejects_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let res = sanitize_cwd(outside.path().to_str().unwrap(), tmp.path());
        assert!(res.is_err());
    }

    #[test]
    fn sanitize_cwd_accepts_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let res = sanitize_cwd(sub.to_str().unwrap(), tmp.path());
        assert!(res.is_ok());
    }

    #[test]
    fn sanitize_cwd_rejects_relative() {
        let tmp = tempfile::tempdir().unwrap();
        let res = sanitize_cwd("./relative", tmp.path());
        assert!(res.is_err());
    }

    #[test]
    fn configured_defaults_only_apply_to_the_matching_backend() {
        assert!(same_backend("qoder", "qoder"));
        assert!(same_backend("qodercli", "qoder"));
        assert!(same_backend("kimi-code", "kimi"));
        assert!(!same_backend("qoder", "kimi"));
        assert!(!same_backend("auto", "codex"));
    }

    #[test]
    fn effort_is_only_sent_when_the_selected_model_declares_it() {
        let model = super::super::backend::ModelInfo {
            id: "Qwen3.8-Max-Preview".into(),
            is_alias: false,
            context_window_options: vec![],
            default_context_window: 0,
            effort_options: vec![],
            default_effort: String::new(),
            display_name: "Qwen3.8-Max-Preview".into(),
        };
        assert_eq!(
            supported_model_effort(
                std::slice::from_ref(&model),
                Some("Qwen3.8-Max-Preview"),
                Some("high"),
            ),
            None
        );

        let supported = super::super::backend::ModelInfo {
            effort_options: vec!["low".into(), "high".into()],
            ..model
        };
        assert_eq!(
            supported_model_effort(&[supported], Some("Qwen3.8-Max-Preview"), Some("HIGH")),
            Some("high".into())
        );
    }

    #[test]
    fn supervisor_is_the_only_acp_retry_owner() {
        assert!(!should_retry_acp(1));
        assert!(!should_retry_acp(2));
        assert!(!should_retry_acp(3));
    }
}
