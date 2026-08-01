//! Non-interactive harness execution and shared session compaction.

use std::sync::Arc;

use anyhow::Result;

use crate::commands::{append_response_language_policy, append_web_search_note};
use crate::config::RuntimeConfig;
use crate::llm::{ChatMessage, LlmClient};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};
use crate::usage::UsageTracker;

/// Supervised execution with a structured progress callback. The callback is
/// intentionally transport-agnostic so the durable supervisor can persist the
/// same events it broadcasts to web clients.
pub async fn run_supervised_with_progress(
    prompt: String,
    progress: crate::tools::progress::ProgressEmitter,
) -> Result<String> {
    run_oneshot_inner(prompt, true, Some(progress), None, None, None).await
}

pub async fn run_supervised_with_progress_and_audit(
    prompt: String,
    progress: crate::tools::progress::ProgressEmitter,
    audit_sink: Arc<dyn crate::harness::HarnessAuditSink>,
) -> Result<String> {
    run_oneshot_inner(prompt, true, Some(progress), Some(audit_sink), None, None).await
}

#[derive(Debug)]
pub struct DelegatedWorkerRun {
    pub output: String,
    pub child_batch: Option<crate::task::TaskBatchAccepted>,
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
    dispatch: Option<crate::task::WorkerDispatchContext>,
) -> Result<DelegatedWorkerRun> {
    let signal: crate::task::WorkerDispatchSignal = Arc::new(std::sync::Mutex::new(None));
    let worker_dispatch = dispatch.map(|context| (context, Arc::clone(&signal)));
    let output = run_oneshot_inner(
        prompt,
        allow_mutation,
        None,
        None,
        Some(crate::task::DelegatedModelSnapshot {
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
    progress: Option<crate::tools::progress::ProgressEmitter>,
    audit_sink: Option<Arc<dyn crate::harness::HarnessAuditSink>>,
    model_snapshot: Option<crate::task::DelegatedModelSnapshot>,
    worker_dispatch: Option<(
        crate::task::WorkerDispatchContext,
        crate::task::WorkerDispatchSignal,
    )>,
) -> Result<String> {
    use crate::agent_runner::{AgentRunner, PermissionDecision, ToolEventSink};
    use crate::backend::BackendClient;
    use crate::commands::{
        get_system_prompt_with_catalog, handle_command, memory_injection_opts_from_features,
    };
    use crate::config::RuntimeConfig;
    use crate::hooks::HookRunner;
    use crate::llm::{ChatMessage, LlmClient};
    use crate::permissions::PermissionEngine;
    use crate::session::{ConversationMessage, Session};
    use crate::tools::register::{register_all_tools, register_memory_tools};
    use crate::tools::registry::ToolRegistry;
    use crate::usage::UsageTracker;
    use std::io::Write;

    // 1) config.json + backend cache overlay
    let mut config = RuntimeConfig::load();
    let backend = std::sync::Arc::new(BackendClient::new(&config.backend_url));
    let _ = config.override_providers_from_backend(&backend).await;
    if let Some(snapshot) = model_snapshot.as_ref() {
        apply_delegated_model_snapshot(&mut config, snapshot)?;
    }

    // 2) 内置 slash command 短路，不进入 LLM。
    let cmd_result = handle_command(&prompt, &backend, &config).await;
    if !cmd_result.output.is_empty() {
        return Ok(cmd_result.output);
    }

    // 3) LLM oneshot
    let llm = Arc::new(if model_snapshot.is_some() {
        let provider = config
            .active_provider()
            .ok_or_else(|| anyhow::anyhow!("委派任务的 provider 快照已不可用"))?
            .clone();
        LlmClient::with_provider(provider, config.backend_url.clone())
    } else {
        LlmClient::from_config(&config)
            .map_err(|e| anyhow::anyhow!("AI Provider 未配置（{}）。运行 `pwcli config`。", e))?
    });

    let mut tool_registry = ToolRegistry::new();
    register_all_tools(&mut tool_registry, std::sync::Arc::clone(&backend));
    let user_slug = config
        .user
        .as_ref()
        .and_then(|u| u.slug.clone())
        .unwrap_or_else(|| "local".to_string());
    register_memory_tools(&mut tool_registry, user_slug.clone());
    let tool_registry = Arc::new(tool_registry);
    crate::tools::register::register_runtime_tools(Arc::clone(&tool_registry), Arc::clone(&llm))
        .await;
    if let Some((context, signal)) = worker_dispatch {
        crate::task::register_worker_dispatch_tool(&tool_registry, context, signal);
    }
    let tool_schemas = tool_registry.to_schemas();
    let permission_engine = PermissionEngine::default_engine();
    let hook_runner = HookRunner::new();
    let mut system_prompt = get_system_prompt_with_catalog(
        &backend,
        &user_slug,
        &memory_injection_opts_from_features(&config.features, Some(prompt.clone())),
    )
    .await;
    append_web_search_note(&mut system_prompt);
    append_response_language_policy(&mut system_prompt);

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
        progress: Option<crate::tools::progress::ProgressEmitter>,
    }
    impl ToolEventSink for OneshotSink {
        fn on_tool_call(&self, _id: &str, name: &str, args: &str) {
            let _ = writeln!(std::io::stderr(), "🔧 {}({})", name, truncate(args, 80));
            if let Some(progress) = &self.progress {
                progress(&format!("🔧 {} · {}", name, truncate(args, 120)));
            }
        }
        fn on_tool_result(&self, _id: &str, _name: &str, result: &str, is_err: bool) {
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
        ) -> Option<crate::tools::progress::ProgressEmitter> {
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
    let harness = crate::harness::HarnessControl::default();
    let decision_reviewer =
        crate::fusion::moa::active_runtime(&backend, config.backend_url.clone()).await?;
    let runner = AgentRunner {
        llm: &llm,
        tool_registry: &tool_registry,
        permission_engine: &permission_engine,
        hook_runner: &hook_runner,
        tool_schemas: &tool_schemas,
        system_prompt: &system_prompt,
        config: &config,
        run_options: crate::agent_runner::HarnessRunOptions::oneshot_with_thinking(
            allow_mutation,
            model_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.thinking),
        ),
        sink: Some(&sink),
        cancel_token: None,
        background_tasks: None,
        session_id: None,
        tool_registry_arc: Some(Arc::clone(&tool_registry)),
        web_cache: None,
        harness: Some(&harness),
        decision_reviewer: decision_reviewer
            .as_ref()
            .map(|reviewer| reviewer as &dyn crate::fusion::DecisionReviewer),
        audit_sink: audit_sink.as_deref(),
    };

    let summary = {
        let active_model_ctx = build_active_model_context_local(
            &config,
            model_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.thinking),
        );
        crate::llm::model_context::with_active_model(active_model_ctx, async {
            runner
                .run_turn(&mut messages, &mut session, &mut usage)
                .await
        })
        .await?
    };

    Ok(summary.map(|value| value.content).unwrap_or_default())
}

fn apply_delegated_model_snapshot(
    config: &mut RuntimeConfig,
    snapshot: &crate::task::DelegatedModelSnapshot,
) -> Result<()> {
    let provider_id = snapshot
        .provider_id
        .as_deref()
        .or(config.active_provider.as_deref())
        .ok_or_else(|| anyhow::anyhow!("委派任务没有可恢复的 provider 快照"))?
        .to_string();
    config.active_provider = Some(provider_id.clone());
    let providers = config
        .providers
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("委派任务的 provider `{provider_id}` 已不可用"))?;
    let provider = providers
        .iter_mut()
        .find(|provider| provider.name == provider_id)
        .ok_or_else(|| anyhow::anyhow!("委派任务的 provider `{provider_id}` 已不可用"))?;
    if !snapshot.model.trim().is_empty() {
        provider.model = snapshot.model.clone();
    }
    if let Some(effort) = snapshot
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
    {
        let model_id = provider.model.clone();
        if !provider.models.iter().any(|model| model.id == model_id) {
            provider.models.push(crate::config::provider::ModelEntry {
                id: model_id.clone(),
                name: model_id.clone(),
                enabled: None,
                max_output: None,
                context_window: None,
                capabilities: None,
                request_params: None,
                thinking_params: None,
                deferred_tools_mode: None,
            });
        }
        let model = provider
            .models
            .iter_mut()
            .find(|model| model.id == model_id)
            .expect("the selected model entry was inserted above");
        model
            .request_params
            .get_or_insert_with(serde_json::Map::new)
            .insert(
                "reasoning_effort".to_string(),
                serde_json::Value::String(effort.to_string()),
            );
        if snapshot.thinking {
            model
                .thinking_params
                .get_or_insert_with(serde_json::Map::new)
                .insert(
                    "reasoning_effort".to_string(),
                    serde_json::Value::String(effort.to_string()),
                );
        }
    }
    Ok(())
}

/// REPL/TUI/oneshot 用：从 RuntimeConfig 的 active provider
/// 推导 ActiveModelContext。每轮调用一次，让 /provider /model 切换立即反映。
///
/// 与 service 模式 `routes.rs::build_active_model_context` 行为对齐：
/// - 未配置 active provider → model_id 空、supports_vision=false（安全默认）
/// - 普通 model → 在本机 providers 列表查 capabilities.vision
fn build_active_model_context_local(
    config: &crate::config::RuntimeConfig,
    thinking: bool,
) -> crate::llm::model_context::ActiveModelContext {
    let Some(active) = config.active_provider() else {
        return crate::llm::model_context::ActiveModelContext {
            provider_id: None,
            model_id: String::new(),
            effort: None,
            thinking,
            supports_vision: false,
        };
    };
    let provider_id = Some(active.name.clone());
    let model_id = active.model.clone();
    let effort = crate::llm::model_context::effort_for_provider(active, thinking);
    let providers = config.providers.clone().unwrap_or_default();
    let supports_vision = if model_id.is_empty() {
        false
    } else {
        crate::llm::model_context::compute_vision_support_for(&providers, &model_id)
    };
    crate::llm::model_context::ActiveModelContext {
        provider_id,
        model_id,
        effort,
        thinking,
        supports_vision,
    }
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

    let to_summarize_count = crate::middleware::summarization::token_compaction_split(
        messages,
        crate::middleware::summarization::KEEP_RECENT_TOKENS,
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
    let summary = match crate::llm::summarize_via_llm(
        &user_content,
        summary_prompt,
        llm,
        usage_tracker,
    )
    .await
    {
        Ok(s) => s,
        Err(crate::llm::SummarizeError::EmptyResponse) => {
            return CompactOutcome::Failed("⚠️  AI 返回空摘要，已取消压缩".to_string())
        }
        Err(crate::llm::SummarizeError::LlmError(e)) => {
            return CompactOutcome::Failed(format!("❌ 摘要失败: {}", e))
        }
    };

    let now = chrono::Utc::now();
    let traces =
        crate::middleware::summarization::collect_file_traces(&messages[..to_summarize_count]);
    let summary_content = crate::middleware::summarization::encode_compaction_summary(
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

    fn provider(name: &str, model: &str) -> crate::config::ProviderConfig {
        crate::config::ProviderConfig {
            name: name.to_string(),
            base_url: "http://127.0.0.1:9".to_string(),
            api_key: "test-key".to_string(),
            protocol: "openai".to_string(),
            model: model.to_string(),
            models: Vec::new(),
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
            &crate::task::DelegatedModelSnapshot {
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
