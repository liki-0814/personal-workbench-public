pub use crate::ai::llm::models::StreamEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStepKind {
    ModelCall,
    Reasoning,
    AssistantSegment,
    ToolBatch,
    ToolCall,
    Permission,
    Decision,
    Recovery,
    BackgroundTask,
    CompletionCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStepStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Backgrounded,
    Discarded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantSegmentKind {
    Narration,
    Candidate,
    Final,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDisposition {
    Promoted,
    Discarded,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStopReason {
    Completed,
    ToolTerminated,
    Cancelled,
    MaxRounds,
    PolicyBlocked,
    Error { message: String },
}

/// Stable, transport-neutral event envelope used by timeline projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventRecord {
    pub sequence: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub run_id: String,
    pub turn_id: String,
    pub step_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        reason: AgentStopReason,
    },
    TurnStart {
        round: u32,
    },
    TurnEnd {
        round: u32,
    },
    StepStatus {
        kind: AgentStepKind,
        status: AgentStepStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    AssistantSegment {
        round: u32,
        kind: AssistantSegmentKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        disposition: Option<CandidateDisposition>,
    },
    ReasoningSummaryDelta {
        delta: String,
    },
    RuntimeUpdate {
        call_index: u32,
        thinking_level: crate::agent_core::contracts::ThinkingLevel,
    },
}
