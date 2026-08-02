use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 消息角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageRole::System => write!(f, "system"),
            MessageRole::User => write!(f, "user"),
            MessageRole::Assistant => write!(f, "assistant"),
            MessageRole::Tool => write!(f, "tool"),
        }
    }
}

/// 内容块（支持多模态扩展）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<crate::reliability::FailureEnvelope>,
    },
}

/// Token 用量（从 llm::models 复用）
pub use crate::llm::TokenUsage;

/// 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
}

impl ConversationMessage {
    pub fn new_user(text: impl Into<String>) -> Self {
        Self {
            id: generate_id(),
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            created_at: Utc::now(),
            parent_id: None,
            model: None,
            token_usage: None,
        }
    }

    pub fn new_assistant(text: impl Into<String>) -> Self {
        Self {
            id: generate_id(),
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
            created_at: Utc::now(),
            parent_id: None,
            model: None,
            token_usage: None,
        }
    }

    pub fn new_system(text: impl Into<String>) -> Self {
        Self {
            id: generate_id(),
            role: MessageRole::System,
            content: vec![ContentBlock::Text { text: text.into() }],
            created_at: Utc::now(),
            parent_id: None,
            model: None,
            token_usage: None,
        }
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// 生成简单唯一 ID（时间戳 + 计数器）
fn generate_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", ts, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_role_display() {
        assert_eq!(format!("{}", MessageRole::User), "user");
        assert_eq!(format!("{}", MessageRole::Assistant), "assistant");
        assert_eq!(format!("{}", MessageRole::Tool), "tool");
        assert_eq!(format!("{}", MessageRole::System), "system");
    }

    #[test]
    fn test_user_message() {
        let msg = ConversationMessage::new_user("hello");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.text_content(), "hello");
        assert!(!msg.id.is_empty());
    }

    #[test]
    fn test_assistant_message() {
        let msg = ConversationMessage::new_assistant("world");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.text_content(), "world");
    }

    #[test]
    fn test_text_content_filters_non_text() {
        let msg = ConversationMessage {
            id: "test".to_string(),
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Hello ".to_string(),
                },
                ContentBlock::Thinking {
                    thinking: "thinking...".to_string(),
                },
                ContentBlock::Text {
                    text: "world".to_string(),
                },
            ],
            created_at: Utc::now(),
            parent_id: None,
            model: None,
            token_usage: None,
        };
        assert_eq!(msg.text_content(), "Hello world");
    }

    #[test]
    fn test_content_block_tool_use() {
        let block = ContentBlock::ToolUse {
            id: "tu_1".to_string(),
            name: "list_files".to_string(),
            input: serde_json::json!({"path": "/tmp"}),
        };
        match block {
            ContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "tu_1");
                assert_eq!(name, "list_files");
            }
            _ => panic!("Expected ToolUse"),
        }
    }

    #[test]
    fn test_token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
    }
}
