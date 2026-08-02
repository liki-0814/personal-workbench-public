pub mod journal;
pub mod message;
pub mod runtime;

pub use journal::*;
pub use message::*;
pub use runtime::*;

use crate::llm::{ChatMessage, FunctionCall, ToolCall};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Ready,
    Paused,
    WorkspaceMissing,
    AwaitingBinding,
    Deleting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceBinding {
    pub canonical_path: PathBuf,
    pub display_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 会话句柄：管理单个对话的完整生命周期
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub messages: Vec<ConversationMessage>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    // 多用户隔离：bootstrap 解析后的 user_slug；P2 序列化前补 serde 派生
    pub owner: Option<String>,
    pub workspace: Option<WorkspaceBinding>,
    pub state: SessionState,
    pub generation: u64,
}

impl Session {
    pub fn new(name: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: format!("sess_{}", now.timestamp_nanos_opt().unwrap_or(0)),
            name: name.into(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            owner: None,
            workspace: None,
            state: SessionState::AwaitingBinding,
            generation: 1,
        }
    }

    pub fn new_in_workspace(
        name: impl Into<String>,
        canonical_path: PathBuf,
        display_path: PathBuf,
    ) -> Self {
        let mut session = Self::new(name);
        session.workspace = Some(WorkspaceBinding {
            canonical_path,
            display_path,
            locked_at: None,
        });
        session.state = SessionState::Ready;
        session
    }

    pub fn lock_workspace(&mut self) {
        if let Some(workspace) = &mut self.workspace {
            workspace.locked_at.get_or_insert_with(chrono::Utc::now);
        }
    }

    /// 生成时间戳化的会话（避免每次 /new 覆盖同一份 default.json）
    /// 格式: session-YYYYMMDD-HHMMSS
    pub fn new_timestamped() -> Self {
        let local = chrono::Local::now();
        Self::new(local.format("session-%Y%m%d-%H%M%S").to_string())
    }

    pub fn add_message(&mut self, msg: ConversationMessage) {
        self.messages.push(msg);
        self.updated_at = chrono::Utc::now();
    }

    pub fn last_message(&self) -> Option<&ConversationMessage> {
        self.messages.last()
    }

    pub fn estimate_tokens(&self) -> u32 {
        self.messages.iter().map(estimate_message_tokens).sum()
    }

    /// 把会话消息还原成 LLM 协议层的 ChatMessage 序列
    /// 用途：/resume 时把磁盘会话注水回 messages，让 LLM 看到完整上下文
    pub fn to_chat_messages(&self) -> Vec<ChatMessage> {
        let mut out = Vec::new();
        for msg in &self.messages {
            match msg.role {
                MessageRole::System | MessageRole::User => {
                    out.push(ChatMessage {
                        role: msg.role.to_string(),
                        content: msg.text_content(),
                        images: Vec::new(),
                        generated_images: Vec::new(),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                MessageRole::Assistant => {
                    let text = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let tool_calls: Vec<ToolCall> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                                id: id.clone(),
                                kind: "function".to_string(),
                                function: FunctionCall {
                                    name: name.clone(),
                                    arguments: input.to_string(),
                                },
                                thought_signature: None,
                            }),
                            _ => None,
                        })
                        .collect();
                    out.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: text,
                        images: Vec::new(),
                        generated_images: Vec::new(),
                        tool_calls: if tool_calls.is_empty() {
                            None
                        } else {
                            Some(tool_calls)
                        },
                        tool_call_id: None,
                    });
                }
                MessageRole::Tool => {
                    // 每个 ToolResult 块还原成独立的 tool 消息（与 LLM API 习惯一致）
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = block
                        {
                            out.push(ChatMessage {
                                role: "tool".to_string(),
                                content: content.clone(),
                                images: Vec::new(),
                                generated_images: Vec::new(),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// Render the conversation as a self-contained Markdown document.
    /// Used by the `/export` slash command and intended for sharing /
    /// archiving completed AI sessions.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", self.name));
        out.push_str(&format!(
            "_Exported {} · {} messages · ~{} tokens_\n\n---\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            self.messages.len(),
            self.estimate_tokens()
        ));

        for msg in &self.messages {
            match msg.role {
                MessageRole::System => {
                    out.push_str("## System\n\n> ");
                    out.push_str(&msg.text_content().replace('\n', "\n> "));
                    out.push_str("\n\n");
                }
                MessageRole::User => {
                    out.push_str("## 🧑 User\n\n");
                    out.push_str(&msg.text_content());
                    out.push_str("\n\n");
                }
                MessageRole::Assistant => {
                    out.push_str("## 🤖 Assistant\n\n");
                    let text = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.is_empty() {
                        out.push_str(&text);
                        out.push_str("\n\n");
                    }
                    for block in &msg.content {
                        if let ContentBlock::ToolUse { name, input, .. } = block {
                            out.push_str(&format!(
                                "<details><summary>🔧 <code>{}</code></summary>\n\n```json\n{}\n```\n\n</details>\n\n",
                                name,
                                serde_json::to_string_pretty(input).unwrap_or_default()
                            ));
                        }
                    }
                }
                MessageRole::Tool => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = block
                        {
                            out.push_str(&format!(
                                "<details><summary>↳ tool result <code>{}</code></summary>\n\n```\n{}\n```\n\n</details>\n\n",
                                tool_use_id,
                                content
                            ));
                        }
                    }
                }
            }
        }
        out
    }
}

/// 估算单条消息的 token 数（简单启发式）
pub fn estimate_message_tokens(msg: &ConversationMessage) -> u32 {
    estimate_text_tokens(&msg.text_content())
}

/// 估算一段尚未写入 Session 的文本，用于 TUI 输入与流式响应的实时上下文提示。
pub fn estimate_text_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    // CJK 字符 ≈ 1 token，其他 ≈ 0.25 token
    let count: f64 = text
        .chars()
        .map(|c| {
            if ('\u{4e00}'..='\u{9fff}').contains(&c)
                || ('\u{3000}'..='\u{303f}').contains(&c)
                || ('\u{ff00}'..='\u{ffef}').contains(&c)
            {
                1.0
            } else {
                0.25
            }
        })
        .sum();
    count.max(1.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_add_message() {
        let mut session = Session::new("test");
        assert_eq!(session.messages.len(), 0);

        session.add_message(ConversationMessage::new_user("hello"));
        assert_eq!(session.messages.len(), 1);

        let last = session.last_message().unwrap();
        assert_eq!(last.role, MessageRole::User);
        assert_eq!(last.text_content(), "hello");
    }

    #[test]
    fn test_estimate_tokens_ascii() {
        // "hello world" = 11 chars * 0.25 = 2.75 -> ceil to 3 (but we use max(1, sum) as u32)
        let msg = ConversationMessage::new_user("hello world");
        let tokens = estimate_message_tokens(&msg);
        assert!(
            (2..=4).contains(&tokens),
            "Expected ~3 tokens, got {}",
            tokens
        );
    }

    #[test]
    fn test_estimate_tokens_cjk() {
        // "你好世界" = 4 chars * 1 = 4
        let msg = ConversationMessage::new_user("你好世界");
        let tokens = estimate_message_tokens(&msg);
        assert_eq!(tokens, 4);
    }

    #[test]
    fn test_estimate_tokens_mixed() {
        // "hello world 你好世界"
        // "hello world " = 12 chars * 0.25 = 3
        // "你好世界" = 4 chars * 1 = 4
        // total = 7
        let msg = ConversationMessage::new_user("hello world 你好世界");
        let tokens = estimate_message_tokens(&msg);
        assert!(
            (6..=8).contains(&tokens),
            "Expected ~7 tokens, got {}",
            tokens
        );
    }

    #[test]
    fn test_session_estimate_tokens_empty() {
        let session = Session::new("empty");
        assert_eq!(session.estimate_tokens(), 0);
    }

    #[test]
    fn test_session_estimate_tokens_multiple() {
        let mut session = Session::new("multi");
        session.add_message(ConversationMessage::new_user("hello"));
        session.add_message(ConversationMessage::new_assistant("world"));
        let tokens = session.estimate_tokens();
        // "hello" = 5 * 0.25 = 1.25 -> 1
        // "world" = 5 * 0.25 = 1.25 -> 1
        // total = 2
        assert!(
            (2..=3).contains(&tokens),
            "Expected ~2 tokens, got {}",
            tokens
        );
    }

    #[test]
    fn test_session_id_format() {
        let session = Session::new("test");
        assert!(session.id.starts_with("sess_"));
    }

    #[test]
    fn test_to_chat_messages_roundtrip() {
        // user → assistant(text + tool_use) → tool_result → assistant(text) 的完整一轮
        let mut session = Session::new("rt");
        session.add_message(ConversationMessage::new_user("列出待办"));
        session.add_message(ConversationMessage {
            id: "m2".into(),
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "好的".into(),
                },
                ContentBlock::ToolUse {
                    id: "tc_1".into(),
                    name: "data_crud".into(),
                    input: serde_json::json!({"domain":"todo","action":"query"}),
                },
            ],
            created_at: chrono::Utc::now(),
            parent_id: None,
            model: None,
            token_usage: None,
        });
        session.add_message(ConversationMessage {
            id: "m3".into(),
            role: MessageRole::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tc_1".into(),
                content: "找到 1 篇".into(),
                is_error: false,
                failure: None,
            }],
            created_at: chrono::Utc::now(),
            parent_id: None,
            model: None,
            token_usage: None,
        });
        session.add_message(ConversationMessage::new_assistant("当前共 1 篇 wiki"));

        let chat = session.to_chat_messages();
        assert_eq!(chat.len(), 4);

        assert_eq!(chat[0].role, "user");
        assert_eq!(chat[0].content, "列出待办");

        assert_eq!(chat[1].role, "assistant");
        assert_eq!(chat[1].content, "好的");
        let tcs = chat[1].tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "tc_1");
        assert_eq!(tcs[0].function.name, "data_crud");

        assert_eq!(chat[2].role, "tool");
        assert_eq!(chat[2].tool_call_id.as_deref(), Some("tc_1"));
        assert_eq!(chat[2].content, "找到 1 篇");

        assert_eq!(chat[3].role, "assistant");
        assert_eq!(chat[3].content, "当前共 1 篇 wiki");
        assert!(chat[3].tool_calls.is_none());
    }
}
