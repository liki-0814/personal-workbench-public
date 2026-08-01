use crate::llm::ChatMessage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueuedInputDelivery {
    NextTurn,
    Guidance,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueuedInputSource {
    #[default]
    User,
    RuntimeCallback,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum QueuedInputPriority {
    #[default]
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueuedInputStatus {
    Queued,
    WaitingSafePoint,
    Paused,
    Claimed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedInput {
    pub id: String,
    pub client_message_id: String,
    pub sequence: u64,
    pub delivery: QueuedInputDelivery,
    pub status: QueuedInputStatus,
    #[serde(default)]
    pub source: QueuedInputSource,
    #[serde(default)]
    pub priority: QueuedInputPriority,
    pub message: ChatMessage,
    #[serde(default)]
    pub image_urls: Vec<String>,
    #[serde(default)]
    pub file_references: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by_turn_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRuntimeSnapshot {
    pub session_id: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    pub paused: bool,
    pub queue: Vec<QueuedInput>,
    pub last_event_sequence: u64,
}

impl SessionRuntimeSnapshot {
    pub fn idle(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            phase: "idle".to_string(),
            active_turn_id: None,
            paused: false,
            queue: Vec::new(),
            last_event_sequence: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_queued_input_defaults_to_user_normal_priority() {
        let item = serde_json::from_value::<QueuedInput>(serde_json::json!({
            "id": "queue-legacy",
            "clientMessageId": "client-legacy",
            "sequence": 1,
            "delivery": "next_turn",
            "status": "queued",
            "message": {
                "role": "user",
                "content": "legacy",
                "images": []
            },
            "createdAt": "2026-07-30T00:00:00Z"
        }))
        .unwrap();

        assert_eq!(item.source, QueuedInputSource::User);
        assert_eq!(item.priority, QueuedInputPriority::Normal);
    }
}
