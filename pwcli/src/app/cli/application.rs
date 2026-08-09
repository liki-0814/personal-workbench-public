//! Non-interactive harness execution and shared session compaction.

use std::sync::Arc;

use anyhow::Result;

use crate::ai::llm::{ChatMessage, LlmClient};
use crate::ai::usage::UsageTracker;
use crate::app::cli::commands::{append_response_language_policy, append_web_search_note};
use crate::app::config::RuntimeConfig;
use crate::runtime::session::{ContentBlock, ConversationMessage, MessageRole, Session};

/// Supervised execution with a structured progress callback. The callback is
/// intentionally transport-agnostic so the durable supervisor can persist the
/// same events it broadcasts to web clients.
pub async fn run_supervised_with_progress(
    prompt: String,
    progress: crate::runtime::tools::progress::ProgressEmitter,
) -> Result<String> {
    run_oneshot_inner(prompt, true, Some(progress), None, None, None).await
}

pub async fn run_supervised_with_progress_and_audit(
    prompt: String,
    progress: crate::runtime::tools::progress::ProgressEmitter,
    audit_sink: Arc<dyn crate::agent_core::harness::HarnessAuditSink>,
) -> Result<String> {
    run_oneshot_inner(prompt, true, Some(progress), Some(audit_sink), None, None).await
}

#[derive(Debug)]
pub struct DelegatedWorkerRun {
    pub output: String,
    pub child_batch: Option<crate::runtime::task::TaskBatchAccepted>,
}

/// Execute one durable internal-agent attempt inside its independent worker
/// process. The model is the daemon-resolved snapshot captured at publish time;
/// read-only roles cannot approve mutating tools.
pub async fn run_delegated_worker(
    prompt: String,
    allow_mutation: bool,
    provider_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    thinking: bool,
    dispatch: Option<crate::runtime::task::WorkerDispatchContext>,
) -> Result<DelegatedWorkerRun> {
    let signal: crate::runtime::task::WorkerDispatchSignal = Arc::new(std::sync::Mutex::new(None));
    let worker_dispatch = dispatch.map(|context| (context, Arc::clone(&signal)));
    let output = run_oneshot_inner(
        prompt,
        allow_mutation,
        None,
        None,
        Some(crate::runtime::task::DelegatedModelSnapshot {
            provider_id,
            model: model.unwrap_or_default(),
            effort,
            thinking,
        }),
        worker_dispatch,
    )
    .await?;
    let child_batch = signal
        .lock()
        .map_err(|_| anyhow::anyhow!("worker dispatch signal poisoned"))?
        .clone();
    Ok(DelegatedWorkerRun {
        output,
        child_batch,
    })
}

async fn run_oneshot_inner(
    prompt: String,
    allow_mutation: bool,
    progress: Option<crate::runtime::tools::progress::ProgressEmitter>,
    audit_sink: Option<Arc<dyn crate::agent_core::harness::HarnessAuditSink>>,
    model_snapshot: Option<crate::runtime::task::DelegatedModelSnapshot>,
    worker_dispatch: Option<(
        crate::runtime::task::WorkerDispatchContext,
        crate::runtime::task::WorkerDispatchSignal,
    )>,
) -> Result<String> {
    use crate::agent_core::runner::{PermissionDecision, ToolEventSink};
    use crate::ai::llm::ChatMessage;
    use crate::ai::usage::UsageTracker;
    use crate::app::cli::commands::{
        get_system_prompt_with_catalog, handle_command, memory_injection_opts_from_features,
    };
    use crate::runtime::session::{ConversationMessage, Session};
    use std::io::Write;

    // 1) config.json + backend cache overlay
    let factory = crate::app::composition::RuntimeFactory::load_local().await?;

    // 2) 内置 slash command 短路，不进入 LLM。
    let cmd_result = handle_command(&prompt, factory.backend(), factory.config()).await;
    if !cmd_result.output.is_empty() {
        return Ok(cmd_result.output);
    }

    // 3) RuntimeFactory composition for CLI/internal worker.
    let thinking = model_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.thinking);
    let user_slug = factory
        .config()
        .user
        .as_ref()
        .and_then(|user| user.slug.clone())
        .unwrap_or_else(|| "local".to_string());
    let mut system_prompt = get_system_prompt_with_catalog(
        factory.backend(),
        &user_slug,
        &memory_injection_opts_from_features(&factory.config().features, Some(prompt.clone())),
    )
    .await;
    append_web_search_note(&mut system_prompt);
    append_response_language_policy(&mut system_prompt);
    crate::app::cli::commands::append_final_answer_contract(&mut system_prompt);
    let runtime = factory
        .create(crate::app::composition::RuntimeRequest {
            profile: if model_snapshot.is_some() {
                crate::app::composition::RuntimeProfile::InternalWorker
            } else {
                crate::app::composition::RuntimeProfile::CliOneshot
            },
            provider_override: model_snapshot
                .as_ref()
                .map(crate::app::composition::ProviderSelection::from),
            workspace: std::env::current_dir()?,
            permission_mode: if allow_mutation {
                crate::runtime::permissions::AgentPermissionMode::Full
            } else {
                crate::runtime::permissions::AgentPermissionMode::Risk
            },
            thinking,
            thinking_level: thinking.into(),
            session_id: None,
            worker_dispatch,
            system_prompt,
            harness: None,
            tool_context: crate::runtime::tools::context::ToolExecutionContext::default(),
        })
        .await?;

    let mut messages: Vec<ChatMessage> = vec![ChatMessage {
        images: Vec::new(),
        generated_images: Vec::new(),
        role: "user".to_string(),
        content: prompt.clone(),
        tool_calls: None,
        tool_call_id: None,
    }];
    let mut session = Session::new_timestamped();
    session.add_message(ConversationMessage::new_user(&prompt));
    let mut usage = UsageTracker::new();

    // Sink：工具调用打 stderr（让 stdout 干净，方便 pipe），文本 delta 也打 stderr。
    // 最终 content 走 TurnSummary 一次性打 stdout。
    struct OneshotSink {
        allow_mutation: bool,
        progress: Option<crate::runtime::tools::progress::ProgressEmitter>,
    }
    impl ToolEventSink for OneshotSink {
        fn on_tool_call(&self, _id: &str, name: &str, args: &str) {
            let _ = writeln!(std::io::stderr(), "🔧 {}({})", name, truncate(args, 80));
            if let Some(progress) = &self.progress {
                progress(&format!("🔧 {} · {}", name, truncate(args, 120)));
            }
        }
        fn on_tool_result(
            &self,
            _id: &str,
            _name: &str,
            result: &str,
            is_err: bool,
            _failure: Option<&crate::agent_core::reliability::FailureEnvelope>,
        ) {
            let prefix = if is_err { "✗" } else { "↳" };
            let _ = writeln!(std::io::stderr(), "  {} {}", prefix, truncate(result, 200));
            if let Some(progress) = &self.progress {
                progress(&format!("{} {}", prefix, truncate(result, 240)));
            }
        }
        fn on_permission_prompt(&self, _: &str, _: &str) {}
        fn on_permission_denied(&self, name: &str) {
            let _ = writeln!(std::io::stderr(), "🚫 工具被拒：{}", name);
        }
        fn on_thinking_start(&self) {}
        fn on_text_delta(&self, _: &str) {}
        fn on_assistant_end(&self) {}
        // 非交互模式没法 prompt 用户 → 一律拒绝（保险）。如需放行用 --yolo（待加）。
        fn ask_permission<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PermissionDecision> + Send + 'a>>
        {
            let decision = if self.allow_mutation {
                PermissionDecision::Allow
            } else {
                PermissionDecision::Deny
            };
            Box::pin(async move { decision })
        }
        fn progress_emitter_for(
            &self,
            _id: &str,
        ) -> Option<crate::runtime::tools::progress::ProgressEmitter> {
            self.progress.clone()
        }
    }
    fn truncate(s: &str, n: usize) -> String {
        if s.chars().count() <= n {
            s.to_string()
        } else {
            let head: String = s.chars().take(n).collect();
            format!("{}…", head)
        }
    }

    let sink = OneshotSink {
        allow_mutation,
        progress,
    };
    let summary = runtime
        .run_turn(
            &mut messages,
            &mut session,
            &mut usage,
            &sink,
            audit_sink.as_deref(),
        )
        .await?;

    Ok(summary.map(|value| value.content).unwrap_or_default())
}

#[cfg(test)]
fn apply_delegated_model_snapshot(
    config: &mut RuntimeConfig,
    snapshot: &crate::runtime::task::DelegatedModelSnapshot,
) -> Result<()> {
    crate::app::composition::apply_provider_selection(
        config,
        Some(&crate::app::composition::ProviderSelection::from(snapshot)),
    )
}

/// `/compact` 与自动压缩共用的结果类型
pub enum CompactOutcome {
    Done {
        summarized: usize,
        kept: usize,
        total: usize,
    },
    Skipped(String),
    Failed(String),
}

/// 压缩会话历史：摘要旧 turn，保留最近 20K token 的完整 turn。
/// 同步更新 messages（LLM 协议层）和 session.messages（持久化层），
/// 摘要 token 用量记入 usage_tracker。
pub async fn compact_session(
    session: &mut Session,
    messages: &mut Vec<ChatMessage>,
    usage_tracker: &mut UsageTracker,
    config: &RuntimeConfig,
    llm_client: Option<&LlmClient>,
) -> CompactOutcome {
    let total = session.messages.len();
    if total <= 3 {
        return CompactOutcome::Skipped(format!("ℹ️  消息数 ({}) 太少，无需压缩", total));
    }
    let llm = match llm_client {
        Some(l) => l,
        None => {
            return CompactOutcome::Skipped(
                "⚠️  AI Provider 未配置，无法智能压缩；可用 /new 新建会话".to_string(),
            );
        }
    };

    let to_summarize_count = crate::agent_core::middleware::summarization::token_compaction_split(
        messages,
        crate::agent_core::middleware::summarization::KEEP_RECENT_TOKENS,
    );
    if to_summarize_count == 0 {
        return CompactOutcome::Skipped("ℹ️  历史未超过 20K token 保留窗口，无需压缩".to_string());
    }
    let convo_text: String = session.messages[..to_summarize_count]
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let body = match m.role {
                MessageRole::User => format!("[用户] {}", m.text_content()),
                MessageRole::Assistant => {
                    let text = m.text_content();
                    let tool_uses: Vec<String> = m
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse { name, .. } => Some(format!("调用 {}", name)),
                            _ => None,
                        })
                        .collect();
                    if tool_uses.is_empty() {
                        format!("[助手] {}", text)
                    } else {
                        format!("[助手] {} ({})", text, tool_uses.join(", "))
                    }
                }
                MessageRole::Tool => {
                    let preview: String = m
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolResult { content, .. } => {
                                Some(content.chars().take(120).collect::<String>())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!("[工具结果] {}", preview)
                }
                MessageRole::System => format!("[系统] {}", m.text_content()),
            };
            format!("{}. {}", i + 1, body)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let summary_prompt = "你是会话摘要专家。把以下历史对话压缩为一段中文摘要：保留用户目标、关键决策、重要数据点、已完成动作和文件操作；省略寒暄、长输出、重复细节。输出 200-400 字。直接给摘要，不要前缀。";
    let user_content = format!(
        "以下是 {} 条历史消息：\n\n{}",
        to_summarize_count, convo_text
    );

    let _ = config; // active_provider 信息已通过 llm.provider() 取
    let summary =
        match crate::ai::llm::summarize_via_llm(&user_content, summary_prompt, llm, usage_tracker)
            .await
        {
            Ok(s) => s,
            Err(crate::ai::llm::SummarizeError::EmptyResponse) => {
                return CompactOutcome::Failed("⚠️  AI 返回空摘要，已取消压缩".to_string())
            }
            Err(crate::ai::llm::SummarizeError::LlmError(e)) => {
                return CompactOutcome::Failed(format!("❌ 摘要失败: {}", e))
            }
        };

    let now = chrono::Utc::now();
    let traces = crate::agent_core::middleware::summarization::collect_file_traces(
        &messages[..to_summarize_count],
    );
    let summary_content = crate::agent_core::middleware::summarization::encode_compaction_summary(
        &format!("【历史摘要】{}", summary),
        &traces,
    );
    let summary_msg = ConversationMessage {
        id: format!("msg_compact_{}", now.timestamp_nanos_opt().unwrap_or(0)),
        role: MessageRole::System,
        content: vec![ContentBlock::Text {
            text: summary_content,
        }],
        created_at: now,
        parent_id: None,
        model: None,
        token_usage: None,
    };

    let kept_msgs: Vec<ConversationMessage> = session.messages.split_off(to_summarize_count);
    session.messages = vec![summary_msg];
    session.messages.extend(kept_msgs);
    *messages = session.to_chat_messages();

    CompactOutcome::Done {
        summarized: to_summarize_count,
        kept: total.saturating_sub(to_summarize_count),
        total: session.messages.len(),
    }
}

/// Returned to main.rs to keep `Result<()>` flow.
pub type AgentTaskResult = Result<()>;

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str, model: &str) -> crate::ai::config::ProviderConfig {
        crate::ai::config::ProviderConfig {
            name: name.to_string(),
            base_url: "http://127.0.0.1:9".to_string(),
            api_key: "test-key".to_string(),
            protocol: "openai".to_string(),
            model: model.to_string(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        }
    }

    #[test]
    fn delegated_model_snapshot_restores_provider_model_and_wire_effort() {
        let mut config = RuntimeConfig {
            providers: Some(vec![
                provider("global", "global-model"),
                provider("root-turn", "stale-model"),
            ]),
            active_provider: Some("global".to_string()),
            ..RuntimeConfig::default()
        };

        apply_delegated_model_snapshot(
            &mut config,
            &crate::runtime::task::DelegatedModelSnapshot {
                provider_id: Some("root-turn".to_string()),
                model: "root-model".to_string(),
                effort: Some("high".to_string()),
                thinking: true,
            },
        )
        .unwrap();

        assert_eq!(config.active_provider.as_deref(), Some("root-turn"));
        let providers = config.providers.as_ref().unwrap();
        assert_eq!(providers[0].model, "global-model");
        let selected = &providers[1];
        assert_eq!(selected.model, "root-model");
        let model = selected.current_model_entry().unwrap();
        assert_eq!(
            model.request_params.as_ref().unwrap()["reasoning_effort"],
            "high"
        );
        assert_eq!(
            model.thinking_params.as_ref().unwrap()["reasoning_effort"],
            "high"
        );
    }
}
