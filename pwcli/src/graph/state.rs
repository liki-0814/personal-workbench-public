use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::agent_runner::TurnSummary;
use crate::fusion::PendingDecision;
use crate::llm::{ChatMessage, TokenUsage, ToolCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentCompletionStage {
    NeedsEvidenceRecord,
    NeedsEvidenceInspect,
    NeedsInspect,
    NeedsLayout,
    NeedsRender,
    Complete,
}

/// Runtime completion contracts are deliberately kept in the generic graph
/// state rather than in a document-specific agent loop. Domain tools advance
/// the contract, while the main Harness decides whether a natural stop is
/// actually safe.
#[derive(Debug, Default)]
pub struct ArtifactCompletionContracts {
    document_stage: Option<DocumentCompletionStage>,
    document_nudges: u8,
}

impl ArtifactCompletionContracts {
    const MAX_DOCUMENT_NUDGES: u8 = 2;

    fn record_tool_result(&mut self, tool_name: &str, content: &str, is_error: bool) {
        if is_error {
            return;
        }
        match tool_name {
            "create_document" | "patch_document" | "attach_document_asset" => {
                self.document_stage = Some(DocumentCompletionStage::NeedsInspect);
                self.document_nudges = 0;
            }
            "record_document_evidence" => {
                self.document_stage = Some(DocumentCompletionStage::NeedsEvidenceInspect);
                self.document_nudges = 0;
            }
            "inspect_document_evidence" => {
                let current = content.contains(r#""status": "current""#)
                    || content.contains(r#""status":"current""#);
                self.document_stage = Some(if current {
                    DocumentCompletionStage::NeedsInspect
                } else {
                    DocumentCompletionStage::NeedsEvidenceRecord
                });
                self.document_nudges = 0;
            }
            "inspect_document" if self.document_stage.is_some() => {
                self.document_stage = Some(DocumentCompletionStage::NeedsLayout);
            }
            "inspect_document_layout"
                if matches!(
                    self.document_stage,
                    Some(DocumentCompletionStage::NeedsLayout)
                ) =>
            {
                self.document_stage = Some(DocumentCompletionStage::NeedsRender);
            }
            "render_document"
                if matches!(
                    self.document_stage,
                    Some(DocumentCompletionStage::NeedsRender)
                ) =>
            {
                self.document_stage = Some(DocumentCompletionStage::Complete);
            }
            _ => {}
        }
    }

    pub fn take_document_nudge(&mut self) -> Option<String> {
        let stage = self.document_stage?;
        if stage == DocumentCompletionStage::Complete
            || self.document_nudges >= Self::MAX_DOCUMENT_NUDGES
        {
            return None;
        }
        self.document_nudges += 1;
        let remaining = match stage {
            DocumentCompletionStage::NeedsEvidenceRecord => {
                "`record_document_evidence` → `inspect_document_evidence` → \
                 `inspect_document` → `inspect_document_layout` → `render_document`"
            }
            DocumentCompletionStage::NeedsEvidenceInspect => {
                "`inspect_document_evidence` → `inspect_document` → \
                 `inspect_document_layout` → `render_document`"
            }
            DocumentCompletionStage::NeedsInspect => {
                "`inspect_document` → `inspect_document_layout` → `render_document`"
            }
            DocumentCompletionStage::NeedsLayout => "`inspect_document_layout` → `render_document`",
            DocumentCompletionStage::NeedsRender => "`render_document`",
            DocumentCompletionStage::Complete => return None,
        };
        Some(format!(
            "The document completion contract is still open for the current document review. \
             Do not finish or merely describe the remaining work. Continue with {remaining} for the \
             current revision. If a validation step fails, repair only the reported issue and restart \
             the contract from `inspect_document`."
        ))
    }

    pub fn is_open(&self) -> bool {
        self.document_stage
            .is_some_and(|stage| stage != DocumentCompletionStage::Complete)
    }
}

/// 图运行时的完整状态，流经每个节点和 middleware。
pub struct GraphState {
    pub messages: Vec<ChatMessage>,
    pub pending_tool_calls: Vec<ToolCall>,
    pub round_count: u32,
    pub token_usage: TokenUsage,
    pub last_content: String,
    pub response_truncated: bool,
    pub pending_decision: Option<PendingDecision>,
    pub reviewed_decisions: HashSet<String>,
    pub tool_call_count: u32,
    pub consecutive_error_count: u32,
    pub last_error_signature: Option<u64>,
    error_family_streaks: HashMap<String, (u64, u32)>,
    pub recovery_reason: Option<String>,
    pub agent_decision_request: Option<(String, String, Vec<String>)>,
    pub final_reviewed: bool,
    /// The first tool batch of each user turn must explicitly resolve whether
    /// clarification is required before research or execution begins.
    pub clarification_gate_resolved: bool,
    /// Set immediately after a tool batch and consumed by the next pre-LLM
    /// compaction check. This makes the post-batch preflight explicit.
    pub tool_batch_checkpoint: Option<usize>,
    pub stream_loop_recoveries: u32,
    pub artifact_completion: ArtifactCompletionContracts,
    pub extensions: Extensions,
}

impl GraphState {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            pending_tool_calls: Vec::new(),
            round_count: 0,
            token_usage: TokenUsage::default(),
            last_content: String::new(),
            response_truncated: false,
            pending_decision: None,
            reviewed_decisions: HashSet::new(),
            tool_call_count: 0,
            consecutive_error_count: 0,
            last_error_signature: None,
            error_family_streaks: HashMap::new(),
            recovery_reason: None,
            agent_decision_request: None,
            final_reviewed: false,
            clarification_gate_resolved: false,
            tool_batch_checkpoint: None,
            stream_loop_recoveries: 0,
            artifact_completion: ArtifactCompletionContracts::default(),
            extensions: Extensions::new(),
        }
    }

    pub fn from_messages(messages: Vec<ChatMessage>) -> Self {
        let clarification_gate_resolved = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .is_some_and(|message| message.content.contains("<!-- pwb-user-choice:"));
        Self {
            messages,
            pending_tool_calls: Vec::new(),
            round_count: 0,
            token_usage: TokenUsage::default(),
            last_content: String::new(),
            response_truncated: false,
            pending_decision: None,
            reviewed_decisions: HashSet::new(),
            tool_call_count: 0,
            consecutive_error_count: 0,
            last_error_signature: None,
            error_family_streaks: HashMap::new(),
            recovery_reason: None,
            agent_decision_request: None,
            final_reviewed: false,
            clarification_gate_resolved,
            tool_batch_checkpoint: None,
            stream_loop_recoveries: 0,
            artifact_completion: ArtifactCompletionContracts::default(),
            extensions: Extensions::new(),
        }
    }

    pub fn record_tool_result(&mut self, tool_name: &str, content: &str, is_error: bool) {
        self.tool_call_count = self.tool_call_count.saturating_add(1);
        self.artifact_completion
            .record_tool_result(tool_name, content, is_error);
        let error_family = tool_error_family(tool_name);
        let effective_error = is_error || tool_output_indicates_failure(error_family, content);
        if !effective_error {
            self.error_family_streaks.remove(error_family);
            if self.error_family_streaks.is_empty() {
                self.consecutive_error_count = 0;
                self.last_error_signature = None;
            }
            return;
        }
        let mut hasher = DefaultHasher::new();
        error_family.hash(&mut hasher);
        normalized_error_reason(error_family, content).hash(&mut hasher);
        let signature = hasher.finish();
        let streak = self
            .error_family_streaks
            .entry(error_family.to_string())
            .or_insert((signature, 0));
        if streak.0 == signature {
            streak.1 = streak.1.saturating_add(1);
        } else {
            *streak = (signature, 1);
        }
        self.last_error_signature = Some(signature);
        self.consecutive_error_count = streak.1;
        if self.consecutive_error_count >= 2 && !error_family.starts_with("web_") {
            self.recovery_reason = Some(format!(
                "Tool family `{error_family}` failed with the same underlying error {} consecutive times; do not retry through another equivalent tool",
                self.consecutive_error_count
            ));
        }
    }

    pub fn mark_tool_batch_complete(&mut self) {
        self.tool_batch_checkpoint = Some(self.messages.len());
    }

    pub fn take_tool_batch_checkpoint(&mut self) -> Option<usize> {
        self.tool_batch_checkpoint.take()
    }

    pub fn tool_family_failure_count(&self, tool_name: &str) -> u32 {
        self.error_family_streaks
            .get(tool_error_family(tool_name))
            .map(|(_, count)| *count)
            .unwrap_or(0)
    }
}

pub(crate) fn tool_error_family(tool_name: &str) -> &str {
    match tool_name {
        "web_query" | "web_search_domains" => "web_search",
        "web_read" | "web_fetch_batch" => "web_read",
        "download_web_file" => "web_download",
        _ => tool_name,
    }
}

fn normalized_error_reason(error_family: &str, content: &str) -> String {
    let lowercase = content.to_lowercase();
    if error_family.starts_with("web_") {
        if lowercase.contains("check-deps.mjs") {
            return "dependency_start_failed".into();
        }
        if lowercase.contains("429") || lowercase.contains("rate limit") {
            return "rate_limited".into();
        }
        if lowercase.contains("timed out") || lowercase.contains("timeout") {
            return "timeout".into();
        }
        if lowercase.contains("dns")
            || lowercase.contains("resolve host")
            || lowercase.contains("name resolution")
        {
            return "name_resolution".into();
        }
        if lowercase.contains("anysearch unavailable") {
            return "anysearch_unavailable".into();
        }
    }
    content.chars().take(400).collect()
}

fn tool_output_indicates_failure(error_family: &str, content: &str) -> bool {
    error_family.starts_with("web_")
        && (content.contains("启动 check-deps.mjs 失败")
            || content.contains("❌")
            || content.to_lowercase().contains("all web requests failed"))
}

impl Default for GraphState {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnSummary {
    pub fn from_state(state: &GraphState) -> Self {
        Self {
            role: "assistant".to_string(),
            content: state.last_content.clone(),
            tool_calls: None,
            duration_ms: 0,
            ttft_ms: None,
            token_usage: Some(state.token_usage),
        }
    }

    pub fn cancelled() -> Self {
        Self {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: None,
            duration_ms: 0,
            ttft_ms: None,
            token_usage: None,
        }
    }
}

/// Type-map: middleware 通过类型键存取私有状态。
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Extensions {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn insert<T: Send + Sync + 'static>(&mut self, val: T) {
        self.map.insert(TypeId::of::<T>(), Box::new(val));
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }
}

impl Default for Extensions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_insert_get() {
        let mut ext = Extensions::new();
        ext.insert(42u32);
        ext.insert("hello".to_string());

        assert_eq!(ext.get::<u32>(), Some(&42));
        assert_eq!(ext.get::<String>(), Some(&"hello".to_string()));
        assert_eq!(ext.get::<bool>(), None);
    }

    #[test]
    fn extensions_get_mut() {
        let mut ext = Extensions::new();
        ext.insert(10u32);
        if let Some(v) = ext.get_mut::<u32>() {
            *v = 20;
        }
        assert_eq!(ext.get::<u32>(), Some(&20));
    }

    #[test]
    fn graph_state_from_messages() {
        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: "hello".to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let state = GraphState::from_messages(msgs);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.round_count, 0);
        assert!(state.pending_tool_calls.is_empty());
        assert!(!state.clarification_gate_resolved);
    }

    #[test]
    fn user_choice_marker_resolves_clarification_gate_for_continuation() {
        let state = GraphState::from_messages(vec![ChatMessage {
            role: "user".to_string(),
            content: "选择公开数据\n<!-- pwb-user-choice:decision-1:public -->".to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }]);
        assert!(state.clarification_gate_resolved);
    }

    #[test]
    fn turn_summary_from_state() {
        let mut state = GraphState::new();
        state.last_content = "done".to_string();
        state.token_usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        let summary = TurnSummary::from_state(&state);
        assert_eq!(summary.content, "done");
        assert_eq!(summary.token_usage.unwrap().total_tokens, 150);
    }

    #[test]
    fn repeated_identical_tool_errors_schedule_recovery_review() {
        let mut state = GraphState::new();
        state.record_tool_result("read_file", "permission denied", true);
        assert!(state.recovery_reason.is_none());
        state.record_tool_result("read_file", "permission denied", true);
        assert!(state
            .recovery_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("2 consecutive times")));
    }

    #[test]
    fn equivalent_search_tool_failures_share_one_recovery_streak() {
        let mut state = GraphState::new();
        state.record_tool_result("web_search_domains", "Error: AnySearch unavailable", true);
        state.record_tool_result("memory_recall", "useful cached source", false);
        state.record_tool_result("web_query", "Error: AnySearch unavailable", true);
        assert_eq!(state.consecutive_error_count, 2);
        assert!(state.recovery_reason.is_none());
        assert_eq!(state.tool_family_failure_count("web_read"), 0);
    }

    #[test]
    fn web_tools_cannot_report_dependency_failure_as_success() {
        let mut state = GraphState::new();
        state.record_tool_result(
            "web_query",
            "## query one\n\n❌ AnySearch unavailable",
            false,
        );
        assert_eq!(state.consecutive_error_count, 1);
        state.record_tool_result("web_search_domains", "Error: AnySearch unavailable", true);
        assert_eq!(state.consecutive_error_count, 2);
        assert!(state.recovery_reason.is_none());
    }

    #[test]
    fn successful_tool_resets_error_streak() {
        let mut state = GraphState::new();
        state.record_tool_result("read_file", "permission denied", true);
        state.record_tool_result("read_file", "ok", false);
        state.record_tool_result("read_file", "permission denied", true);
        assert_eq!(state.consecutive_error_count, 1);
        assert!(state.recovery_reason.is_none());
    }

    #[test]
    fn tool_batch_checkpoint_is_consumed_once() {
        let mut state = GraphState::new();
        state.mark_tool_batch_complete();
        assert_eq!(state.take_tool_batch_checkpoint(), Some(0));
        assert_eq!(state.take_tool_batch_checkpoint(), None);
    }

    #[test]
    fn document_completion_contract_reopens_after_every_mutation() {
        let mut state = GraphState::new();
        state.record_tool_result("create_document", "created", false);
        assert!(state
            .artifact_completion
            .take_document_nudge()
            .unwrap()
            .contains("`inspect_document`"));

        state.record_tool_result("inspect_document", "revision 1", false);
        state.record_tool_result("inspect_document_layout", "pass", false);
        state.record_tool_result("render_document", "rendered", false);
        assert!(state.artifact_completion.take_document_nudge().is_none());

        state.record_tool_result("patch_document", "revision 2", false);
        assert!(state
            .artifact_completion
            .take_document_nudge()
            .unwrap()
            .contains("current revision"));
    }

    #[test]
    fn existing_document_evidence_review_must_reach_render() {
        let mut state = GraphState::new();
        state.record_tool_result(
            "inspect_document_evidence",
            r#"{"status":"missing"}"#,
            false,
        );
        assert!(state
            .artifact_completion
            .take_document_nudge()
            .unwrap()
            .contains("`record_document_evidence`"));

        state.record_tool_result("record_document_evidence", "revision=5", false);
        assert!(state
            .artifact_completion
            .take_document_nudge()
            .unwrap()
            .starts_with("The document completion contract"));
        state.record_tool_result(
            "inspect_document_evidence",
            r#"{"status":"current"}"#,
            false,
        );
        state.record_tool_result("inspect_document", "revision=5", false);
        state.record_tool_result("inspect_document_layout", "pass", false);
        state.record_tool_result("render_document", "rendered", false);
        assert!(state.artifact_completion.take_document_nudge().is_none());
    }

    #[test]
    fn failed_or_out_of_order_tools_do_not_close_document_contract() {
        let mut state = GraphState::new();
        state.record_tool_result("create_document", "created", false);
        state.record_tool_result("render_document", "rendered", false);
        state.record_tool_result("inspect_document", "failed", true);
        let instruction = state.artifact_completion.take_document_nudge().unwrap();
        assert!(instruction.contains("`inspect_document`"));
    }

    #[test]
    fn document_completion_contract_nudges_are_bounded() {
        let mut state = GraphState::new();
        state.record_tool_result("create_document", "created", false);
        assert!(state.artifact_completion.take_document_nudge().is_some());
        assert!(state.artifact_completion.take_document_nudge().is_some());
        assert!(state.artifact_completion.take_document_nudge().is_none());
    }
}
