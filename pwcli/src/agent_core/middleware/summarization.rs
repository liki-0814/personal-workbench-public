use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::agent_core::contracts::{ContentBlock, ConversationMessage, MessageRole};
use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;
use crate::ai::llm::ChatMessage;
use crate::ai::usage::UsageTracker;

use super::types::HookAction;
use super::AgentMiddleware;

pub const KEEP_RECENT_TOKENS: u32 = 20_000;
const DEFAULT_TOKEN_THRESHOLD: u32 = 100_000;
const COMPACTION_PREFIX: &str = "\u{001e}pwcli-compaction:";
const COMPACTION_SUFFIX: char = '\u{001e}';

const COMPACT_SYSTEM_PROMPT: &str = "你是会话摘要专家。把以下历史对话压缩为一段中文摘要：\
保留用户目标、关键决策、重要数据点、已完成动作以及读写过的文件；省略寒暄、长输出、重复细节。\
输出 200-400 字。直接给摘要，不要前缀。";

/// Pre-LLM middleware: auto-compact conversation when context grows too large.
///
/// Mirrors the 70%-context auto-compact logic in `app.rs`.
/// Checks `state.messages` token estimate before each LLM call;
/// if it exceeds the threshold, summarizes older messages and replaces them
/// with a single system summary + the last KEEP_RECENT messages.
pub struct SummarizationMiddleware {
    token_threshold: u32,
}

impl SummarizationMiddleware {
    pub fn new(token_threshold: u32) -> Self {
        Self { token_threshold }
    }

    pub fn with_model_context(model: &str) -> Self {
        let window = crate::ai::usage::context_window_for_model(model);
        Self::with_context_window(window)
    }

    pub fn with_context_window(window: u32) -> Self {
        let threshold = (window as f64 * 0.7) as u32;
        Self {
            token_threshold: threshold,
        }
    }
}

impl Default for SummarizationMiddleware {
    fn default() -> Self {
        Self {
            token_threshold: DEFAULT_TOKEN_THRESHOLD,
        }
    }
}

pub fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
    messages
        .iter()
        .map(|message| {
            crate::agent_core::contracts::session::estimate_text_tokens(&message.content)
                + message
                    .tool_calls
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .map(|call| {
                        crate::agent_core::contracts::session::estimate_text_tokens(
                            &call.function.arguments,
                        )
                    })
                    .sum::<u32>()
        })
        .sum()
}

fn format_messages_for_summary(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let role_label = match m.role.as_str() {
                "user" => "[用户]",
                "assistant" => "[助手]",
                "tool" => "[工具结果]",
                "system" => "[系统]",
                _ => "[其他]",
            };
            let content = if m.role == "tool" {
                let preview: String = m.content.chars().take(120).collect();
                if m.content.chars().count() > 120 {
                    format!("{}…", preview)
                } else {
                    preview
                }
            } else if m.content.chars().count() > 2000 {
                let truncated: String = m.content.chars().take(2000).collect();
                format!("{}…", truncated)
            } else {
                m.content.clone()
            };
            format!("{}. {} {}", i + 1, role_label, content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait]
impl AgentMiddleware for SummarizationMiddleware {
    fn name(&self) -> &str {
        "summarization"
    }

    async fn before_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        let post_tool_batch = state.take_tool_batch_checkpoint().is_some();
        let estimated = estimate_messages_tokens(&state.messages);
        if estimated <= self.token_threshold {
            return Ok(HookAction::Continue);
        }

        let total = state.messages.len();
        if total <= 3 {
            debug!(total, threshold = 4, "too few messages to compact");
            return Ok(HookAction::Continue);
        }

        info!(
            estimated_tokens = estimated,
            threshold = self.token_threshold,
            total_messages = total,
            post_tool_batch,
            "auto-compacting conversation"
        );

        let to_summarize_count = token_compaction_split(&state.messages, KEEP_RECENT_TOKENS);
        if to_summarize_count == 0 {
            return Ok(HookAction::Continue);
        }
        let old_messages = &state.messages[..to_summarize_count];
        let convo_text = format_messages_for_summary(old_messages);
        let user_content = format!(
            "以下是 {} 条历史消息：\n\n{}",
            to_summarize_count, convo_text
        );

        let mut tracker = UsageTracker::new();
        let summary = match crate::ai::llm::summarize_via_llm(
            &user_content,
            COMPACT_SYSTEM_PROMPT,
            ctx.llm,
            &mut tracker,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "summarization failed, continuing without compact");
                return Ok(HookAction::Continue);
            }
        };

        // Replace state.messages: [summary_system_msg] + last KEEP_RECENT
        let traces = collect_file_traces(old_messages);
        let kept: Vec<ChatMessage> = state.messages.split_off(to_summarize_count);
        let summary_content =
            encode_compaction_summary(&format!("【历史摘要】{}", summary), &traces);
        let summary_msg = ChatMessage {
            role: "system".to_string(),
            content: summary_content.clone(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        };
        state.messages = vec![summary_msg];
        state.messages.extend(kept);

        // Sync to Session (persistent layer)
        let now = chrono::Utc::now();
        let summary_conv = ConversationMessage {
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

        let mut session = ctx.session.lock().await;
        let sess_total = session.messages.len();
        let kept_count = state.messages.len().saturating_sub(1);
        if sess_total > kept_count {
            let sess_keep_count = kept_count.min(sess_total);
            let sess_kept = session.messages.split_off(sess_total - sess_keep_count);
            session.messages = vec![summary_conv];
            session.messages.extend(sess_kept);
        }
        drop(session);

        info!(
            summarized = to_summarize_count,
            kept = kept_count,
            remaining = state.messages.len(),
            "conversation compacted"
        );
        ctx.record_intervention(
            self.name(),
            self.revision(),
            crate::agent_core::harness::MiddlewareHook::BeforeLlm,
            crate::agent_core::harness::InterventionKind::StateMutation,
            "automatic_compaction",
            state.round_count,
            None,
            std::collections::BTreeMap::from([
                (
                    "summarized_messages".to_string(),
                    serde_json::Value::from(to_summarize_count as u64),
                ),
                (
                    "remaining_messages".to_string(),
                    serde_json::Value::from(state.messages.len() as u64),
                ),
                (
                    "estimated_tokens_before".to_string(),
                    serde_json::Value::from(estimated),
                ),
            ]),
        )
        .await;

        Ok(HookAction::Continue)
    }
}

pub fn token_compaction_split(messages: &[ChatMessage], keep_tokens: u32) -> usize {
    let mut retained: u32 = 0;
    let mut desired = 0;
    for (index, message) in messages.iter().enumerate().rev() {
        let tokens = estimate_messages_tokens(std::slice::from_ref(message));
        if retained.saturating_add(tokens) > keep_tokens {
            desired = index + 1;
            break;
        }
        retained = retained.saturating_add(tokens);
    }
    if desired == 0 {
        return 0;
    }
    if let Some(user_offset) = messages[desired..]
        .iter()
        .position(|message| message.role == "user")
    {
        return desired + user_offset;
    }
    while desired > 0 && desired < messages.len() && messages[desired].role == "tool" {
        desired -= 1;
    }
    desired
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileTraces {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

pub fn collect_file_traces(messages: &[ChatMessage]) -> FileTraces {
    let mut traces = messages
        .iter()
        .find_map(|message| decode_compaction_summary(&message.content).1)
        .unwrap_or_default();
    for call in messages
        .iter()
        .filter_map(|message| message.tool_calls.as_ref())
        .flatten()
    {
        let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.function.arguments)
        else {
            continue;
        };
        let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let target = match call.function.name.as_str() {
            "read" | "ls" | "find" | "grep" | "get_file_info" => &mut traces.read_files,
            "write" | "edit" | "remove_file" => &mut traces.modified_files,
            _ => continue,
        };
        if !target.iter().any(|existing| existing == path) {
            target.push(path.to_string());
        }
    }
    traces.read_files.sort();
    traces.modified_files.sort();
    traces
}

pub fn encode_compaction_summary(summary: &str, traces: &FileTraces) -> String {
    let metadata = serde_json::to_string(traces).unwrap_or_else(|_| "{}".to_string());
    format!("{COMPACTION_PREFIX}{metadata}{COMPACTION_SUFFIX}{summary}")
}

pub fn decode_compaction_summary(content: &str) -> (&str, Option<FileTraces>) {
    let Some(rest) = content.strip_prefix(COMPACTION_PREFIX) else {
        return (content, None);
    };
    let Some((metadata, summary)) = rest.split_once(COMPACTION_SUFFIX) else {
        return (content, None);
    };
    (summary, serde_json::from_str(metadata).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_threshold() {
        let mw = SummarizationMiddleware::default();
        assert_eq!(mw.token_threshold, DEFAULT_TOKEN_THRESHOLD);
    }

    #[test]
    fn estimate_tokens_basic() {
        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: "a".repeat(400),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let est = estimate_messages_tokens(&msgs);
        assert_eq!(est, 100);
    }

    #[test]
    fn format_truncates_tool_output() {
        let msgs = vec![ChatMessage {
            role: "tool".to_string(),
            content: "x".repeat(200),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let formatted = format_messages_for_summary(&msgs);
        assert!(formatted.contains("[工具结果]"));
        assert!(formatted.contains('…'));
    }

    #[test]
    fn with_model_context_sets_70_pct() {
        let mw = SummarizationMiddleware::with_model_context("claude-sonnet-4-6");
        let window = crate::ai::usage::context_window_for_model("claude-sonnet-4-6");
        let expected = (window as f64 * 0.7) as u32;
        assert_eq!(mw.token_threshold, expected);
    }

    #[test]
    fn compaction_never_splits_assistant_tool_result_pair() {
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "a".into(),
                images: vec![],
                generated_images: vec![],
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                images: vec![],
                generated_images: vec![],
                tool_calls: Some(vec![]),
                tool_call_id: None,
            },
            ChatMessage {
                role: "tool".into(),
                content: "result".into(),
                images: vec![],
                generated_images: vec![],
                tool_calls: None,
                tool_call_id: Some("t".into()),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "done".into(),
                images: vec![],
                generated_images: vec![],
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        assert_eq!(token_compaction_split(&messages, 2), 1);
    }

    #[test]
    fn file_traces_accumulate_across_compactions() {
        let previous = FileTraces {
            read_files: vec!["old.md".into()],
            modified_files: vec![],
        };
        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: encode_compaction_summary("summary", &previous),
                images: vec![],
                generated_images: vec![],
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                images: vec![],
                generated_images: vec![],
                tool_calls: Some(vec![
                    crate::ai::llm::ToolCall {
                        id: "call".into(),
                        kind: "function".into(),
                        function: crate::ai::llm::FunctionCall {
                            name: "edit".into(),
                            arguments: r#"{"path":"new.rs"}"#.into(),
                        },
                        thought_signature: None,
                    },
                    crate::ai::llm::ToolCall {
                        id: "call2".into(),
                        kind: "function".into(),
                        function: crate::ai::llm::FunctionCall {
                            name: "grep".into(),
                            arguments: r#"{"path":"src"}"#.into(),
                        },
                        thought_signature: None,
                    },
                ]),
                tool_call_id: None,
            },
        ];
        let traces = collect_file_traces(&messages);
        assert_eq!(traces.read_files, vec!["old.md", "src"]);
        assert_eq!(traces.modified_files, vec!["new.rs"]);
    }
}
