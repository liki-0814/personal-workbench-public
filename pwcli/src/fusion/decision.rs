use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::llm::{ChatMessage, TokenUsage, ToolCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionTrigger {
    PreAction,
    Recovery,
    FinalReview,
    AgentRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionRisk {
    Low,
    Elevated,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Proceed,
    Revise,
    GatherEvidence,
    Escalate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionResume {
    Tool,
    Agent,
    End,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRequest {
    pub id: String,
    pub trigger: DecisionTrigger,
    pub risk: DecisionRisk,
    pub question: String,
    pub proposal: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(skip)]
    pub tool_calls: Vec<ToolCall>,
}

impl DecisionRequest {
    pub fn new(
        trigger: DecisionTrigger,
        risk: DecisionRisk,
        question: impl Into<String>,
        proposal: impl Into<String>,
        evidence: Vec<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        let question = question.into();
        let proposal = proposal.into();
        let id = decision_id(trigger, risk, &question, &proposal, &tool_calls);
        Self {
            id,
            trigger,
            risk,
            question,
            proposal,
            evidence,
            tool_calls,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvisorResult {
    pub model: String,
    pub succeeded: bool,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionVerdict {
    pub outcome: DecisionOutcome,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub consensus: f32,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub instruction: String,
    #[serde(default)]
    pub options: Vec<DecisionOption>,
    #[serde(default = "default_review_rounds")]
    pub rounds: u8,
    #[serde(default)]
    pub advisors: Vec<AdvisorResult>,
    #[serde(skip)]
    pub usage: TokenUsage,
}

fn default_review_rounds() -> u8 {
    1
}

impl DecisionVerdict {
    pub fn normalized(mut self) -> Self {
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.consensus = self.consensus.clamp(0.0, 1.0);
        self
    }
}

#[derive(Debug, Clone)]
pub struct PendingDecision {
    pub request: DecisionRequest,
    pub resume: DecisionResume,
}

#[async_trait]
pub trait DecisionReviewer: Send + Sync {
    async fn review(
        &self,
        request: &DecisionRequest,
        messages: &[ChatMessage],
        cancel: &CancellationToken,
    ) -> anyhow::Result<DecisionVerdict>;
}

fn decision_id(
    trigger: DecisionTrigger,
    risk: DecisionRisk,
    question: &str,
    proposal: &str,
    tool_calls: &[ToolCall],
) -> String {
    let mut hasher = DefaultHasher::new();
    trigger.hash(&mut hasher);
    risk.hash(&mut hasher);
    question.hash(&mut hasher);
    proposal.hash(&mut hasher);
    for tool_call in tool_calls {
        tool_call.function.name.hash(&mut hasher);
        tool_call.function.arguments.hash(&mut hasher);
    }
    format!("decision_{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::FunctionCall;

    fn tool_call(arguments: &str) -> ToolCall {
        ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "remove_file".into(),
                arguments: arguments.into(),
            },
            thought_signature: None,
        }
    }

    #[test]
    fn decision_ids_are_stable_and_sensitive_to_the_proposal() {
        let one = DecisionRequest::new(
            DecisionTrigger::PreAction,
            DecisionRisk::High,
            "safe?",
            "remove a",
            vec![],
            vec![tool_call(r#"{"path":"a"}"#)],
        );
        let same = DecisionRequest::new(
            DecisionTrigger::PreAction,
            DecisionRisk::High,
            "safe?",
            "remove a",
            vec![],
            vec![tool_call(r#"{"path":"a"}"#)],
        );
        let other = DecisionRequest::new(
            DecisionTrigger::PreAction,
            DecisionRisk::High,
            "safe?",
            "remove b",
            vec![],
            vec![tool_call(r#"{"path":"b"}"#)],
        );
        assert_eq!(one.id, same.id);
        assert_ne!(one.id, other.id);
    }

    #[test]
    fn verdict_scores_are_clamped() {
        let verdict = DecisionVerdict {
            outcome: DecisionOutcome::Proceed,
            confidence: 3.0,
            consensus: -1.0,
            rationale: String::new(),
            instruction: String::new(),
            options: Vec::new(),
            rounds: 1,
            advisors: Vec::new(),
            usage: TokenUsage::default(),
        }
        .normalized();
        assert_eq!(verdict.confidence, 1.0);
        assert_eq!(verdict.consensus, 0.0);
    }
}
