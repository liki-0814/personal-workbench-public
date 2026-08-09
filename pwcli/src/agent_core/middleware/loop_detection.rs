use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;
use crate::ai::llm::{ChatMessage, ToolCall};

use super::types::{HookAction, ToolCallRequest, ToolResult};
use super::AgentMiddleware;

struct LoopDetectionState {
    hash_window: VecDeque<u64>,
    tool_freq: HashMap<String, u32>,
    successful_tool_freq: HashMap<String, u32>,
    pending_warning: Option<String>,
}

pub struct LoopDetectionMiddleware {
    warn_threshold: u32,
    hard_limit: u32,
    window_size: usize,
    tool_freq_warn: u32,
    tool_freq_hard: u32,
}

const MAX_DOCUMENT_EVIDENCE_SEARCHES: u32 = 12;
const WEB_ACCESS_TOOLS: &[&str] = &[
    "web_search_domains",
    "web_query",
    "web_read",
    "web_fetch_batch",
    "download_web_file",
];
const DOCUMENT_CONTRACT_TOOLS: &[&str] = &[
    "inspect_document",
    "inspect_document_evidence",
    "inspect_document_layout",
    "record_document_evidence",
    "render_document",
];

fn is_web_access_tool(name: &str) -> bool {
    WEB_ACCESS_TOOLS.contains(&name)
}

fn is_document_contract_tool(name: &str) -> bool {
    DOCUMENT_CONTRACT_TOOLS.contains(&name)
}

fn repetition_limits(tool_names: &[String], warn: u32, hard: u32) -> (u32, u32) {
    if tool_names
        .iter()
        .all(|name| is_document_contract_tool(name))
    {
        (warn.max(6), hard.max(8))
    } else {
        (warn, hard)
    }
}

fn suppress_exhausted_web_access_calls(state: &mut GraphState) -> Vec<String> {
    let exhausted = WEB_ACCESS_TOOLS
        .iter()
        .filter(|tool| state.tool_family_failure_count(tool) >= 2)
        .map(|tool| crate::agent_core::graph::state::tool_error_family(tool).to_string())
        .collect::<std::collections::HashSet<_>>();
    if exhausted.is_empty() {
        return Vec::new();
    }
    let before = state.pending_tool_calls.len();
    state.pending_tool_calls.retain(|call| {
        !is_web_access_tool(&call.function.name)
            || !exhausted.contains(crate::agent_core::graph::state::tool_error_family(
                &call.function.name,
            ))
    });
    if before == state.pending_tool_calls.len() {
        Vec::new()
    } else {
        let mut families = exhausted.into_iter().collect::<Vec<_>>();
        families.sort();
        families
    }
}

/// High-confidence detector for a provider getting stuck while still
/// streaming text. It intentionally requires four exact repeated blocks (or
/// six identical substantial lines) to avoid flagging normal long answers.
#[derive(Default)]
pub struct StreamDoomDetector {
    text: String,
    detected: bool,
}

impl StreamDoomDetector {
    pub fn observe(&mut self, delta: &str) -> Option<String> {
        if self.detected {
            return None;
        }
        self.text.push_str(delta);
        if self.text.chars().count() < 256 {
            return None;
        }
        let tail = self
            .text
            .chars()
            .rev()
            .take(4096)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        let chars = tail.chars().collect::<Vec<_>>();
        for block_len in [64usize, 128, 256] {
            if chars.len() < block_len * 4 {
                continue;
            }
            let suffix = &chars[chars.len() - block_len..];
            if (2..=4).all(|repeat| {
                let end = chars.len() - block_len * (repeat - 1);
                &chars[end - block_len..end] == suffix
            }) {
                self.detected = true;
                return Some(format!(
                    "stream repeated the same {block_len}-character block four times"
                ));
            }
        }
        let lines = tail
            .lines()
            .rev()
            .filter(|line| !line.trim().is_empty())
            .take(6)
            .collect::<Vec<_>>();
        if lines.len() == 6
            && lines[0].chars().count() >= 32
            && lines.iter().all(|line| *line == lines[0])
        {
            self.detected = true;
            return Some("stream repeated the same substantial line six times".into());
        }
        None
    }
}

impl LoopDetectionMiddleware {
    pub fn new(warn_threshold: u32, hard_limit: u32, window_size: usize) -> Self {
        Self::with_limits(warn_threshold, hard_limit, window_size, 30, 50)
    }

    pub fn with_limits(
        warn_threshold: u32,
        hard_limit: u32,
        window_size: usize,
        tool_freq_warn: u32,
        tool_freq_hard: u32,
    ) -> Self {
        Self {
            warn_threshold,
            hard_limit,
            window_size,
            tool_freq_warn,
            tool_freq_hard,
        }
    }
}

impl Default for LoopDetectionMiddleware {
    fn default() -> Self {
        Self::new(3, 5, 20)
    }
}

fn compute_stable_hash(tool_calls: &[ToolCall]) -> u64 {
    let mut entries: Vec<(String, u64)> = tool_calls
        .iter()
        .map(|tc| {
            let name = &tc.function.name;
            let args_hash = compute_args_hash(name, &tc.function.arguments);
            (name.clone(), args_hash)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut hasher = DefaultHasher::new();
    for (name, args_hash) in &entries {
        name.hash(&mut hasher);
        args_hash.hash(&mut hasher);
    }
    hasher.finish()
}

fn compute_args_hash(tool_name: &str, arguments_json: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    tool_name.hash(&mut hasher);

    match tool_name {
        "write" | "edit" | "str_replace" => {
            arguments_json.hash(&mut hasher);
        }
        "read" => {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json) {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    path.hash(&mut hasher);
                }
                if let Some(offset) = args.get("offset").and_then(|v| v.as_u64()) {
                    let bucket = (offset / 200) * 200;
                    bucket.hash(&mut hasher);
                }
            }
        }
        "read_artifact" => {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json) {
                if let Some(artifact_id) = args.get("artifact_id").and_then(|value| value.as_str())
                {
                    artifact_id.hash(&mut hasher);
                }
                if let Some(offset) = args.get("offset").and_then(|value| value.as_u64()) {
                    offset.hash(&mut hasher);
                }
            }
        }
        "search_file_content" => {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json) {
                for key in ["paths", "queries"] {
                    let mut values = args
                        .get(key)
                        .and_then(|value| value.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|value| value.as_str())
                        .map(|value| value.trim().to_lowercase())
                        .collect::<Vec<_>>();
                    values.sort();
                    key.hash(&mut hasher);
                    values.hash(&mut hasher);
                }
                for key in ["contextLines", "maxMatches"] {
                    if let Some(value) = args.get(key).and_then(|value| value.as_u64()) {
                        key.hash(&mut hasher);
                        value.hash(&mut hasher);
                    }
                }
            }
        }
        _ => {
            // Unknown and mutating tools must include the full payload. Hashing
            // only common locator fields made distinct document patches look
            // identical because patch_document has none of those fields. After
            // five legitimate revisions the loop detector silently discarded
            // the active tool call as an exact repetition.
            arguments_json.hash(&mut hasher);
        }
    }

    hasher.finish()
}

fn count_occurrences(window: &VecDeque<u64>, hash: u64) -> u32 {
    window.iter().filter(|&&h| h == hash).count() as u32
}

fn document_evidence_budget_guidance(tool_name: &str, count: u32) -> Option<&'static str> {
    match (tool_name, count) {
        ("search_file_content", 1) => Some(
            "[Document evidence guidance] One comprehensive local evidence search is complete. Read its per-query coverage counts. If every decision-critical query has coverage above zero, close the evidence phase and move to the editable document. Otherwise run narrow, materially different searches for each remaining decision-critical gap; do not repeat equivalent queries merely to save or spend tokens. Treat exact numeric facts visible in the search results or original user task as a closed allowlist: no other digit-bearing value may enter prose, charts, labels, footnotes, recommendations, scenario thresholds, policy cutoffs, or license notes. Before create_document, scan every visible number against that allowlist; replace unsupported thresholds with qualitative conditions. For each retained fact preserve subject, metric (sales/stock/share), unit, time, geography, scenario, denominator, and exclusions everywhere it appears, including summaries and titles. Mark reproducible calculations and recommendations explicitly as analysis rather than source facts.",
        ),
        ("bash", 1) => Some(
            "[Structured data guidance] One command has completed. If this task analyzes XLSX/CSV/Parquet or another structured file, do not continue with per-sheet or per-metric probes. Use the observed schema to make the next command batch-extract every decision-critical metric, calculation, and validation; locate columns by verified headers and guard missing/short rows instead of guessing numeric indices.",
        ),
        ("bash", 3) => Some(
            "[Structured data budget] Three successful commands have completed. Stop exploratory command fragmentation. Continue only if you can name one unresolved decision-critical metric, explain why existing output cannot answer it, and make one materially different command close that gap. Otherwise synthesize the available evidence and move to the editable deliverable.",
        ),
        _ => None,
    }
}

fn successful_tool_count(state: &LoopDetectionState, tool_name: &str) -> u32 {
    state
        .successful_tool_freq
        .get(tool_name)
        .copied()
        .unwrap_or(0)
}

fn record_successful_tool(
    state: &mut LoopDetectionState,
    tool_name: &str,
    is_error: bool,
) -> Option<String> {
    if is_error {
        return None;
    }
    let count = state
        .successful_tool_freq
        .entry(tool_name.to_string())
        .or_insert(0);
    *count += 1;
    document_evidence_budget_guidance(tool_name, *count).map(str::to_string)
}

#[async_trait]
impl AgentMiddleware for LoopDetectionMiddleware {
    fn name(&self) -> &str {
        "loop_detection"
    }

    async fn before_turn(
        &self,
        state: &mut GraphState,
        _ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        if state.extensions.get::<LoopDetectionState>().is_none() {
            state.extensions.insert(LoopDetectionState {
                hash_window: VecDeque::with_capacity(self.window_size),
                tool_freq: HashMap::new(),
                successful_tool_freq: HashMap::new(),
                pending_warning: None,
            });
        } else if let Some(ld) = state.extensions.get_mut::<LoopDetectionState>() {
            ld.pending_warning = None;
        }
        Ok(HookAction::Continue)
    }

    async fn before_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        let mut injected = false;
        if let Some(ld) = state.extensions.get_mut::<LoopDetectionState>() {
            if let Some(warning) = ld.pending_warning.take() {
                state.messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: warning,
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    tool_calls: None,
                    tool_call_id: None,
                });
                injected = true;
            }
        }
        if injected {
            ctx.record_intervention(
                self.name(),
                self.revision(),
                crate::agent_core::harness::MiddlewareHook::BeforeLlm,
                crate::agent_core::harness::InterventionKind::StateMutation,
                "loop_warning_injected",
                state.round_count,
                None,
                std::collections::BTreeMap::new(),
            )
            .await;
        }
        Ok(HookAction::Continue)
    }

    async fn after_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        if state.pending_tool_calls.is_empty() {
            return Ok(HookAction::Continue);
        }

        let suppressed_families = suppress_exhausted_web_access_calls(state);
        if !suppressed_families.is_empty() {
            state.recovery_reason = None;
            let ld = state
                .extensions
                .get_mut::<LoopDetectionState>()
                .expect("LoopDetectionState must be initialized in before_turn");
            ld.pending_warning = Some(format!(
                "[Web capability circuit open] 能力族 {} 已连续两次同因失败；本会话只停止该能力族，仍可使用其他独立联网能力（例如搜索失败后读取已知可信 URL）。不得用同族工具换名重试。",
                suppressed_families.join(", ")
            ));
            if state.pending_tool_calls.is_empty() {
                return Ok(HookAction::JumpToAgent {
                    reason_code: "exhausted_web_access_suppressed",
                    details: std::collections::BTreeMap::from([(
                        "family_count".to_string(),
                        serde_json::Value::from(suppressed_families.len() as u64),
                    )]),
                });
            }
            ctx.record_intervention(
                self.name(),
                self.revision(),
                crate::agent_core::harness::MiddlewareHook::AfterLlm,
                crate::agent_core::harness::InterventionKind::StateMutation,
                "exhausted_web_access_calls_suppressed",
                state.round_count,
                None,
                std::collections::BTreeMap::from([(
                    "family_count".to_string(),
                    serde_json::Value::from(suppressed_families.len() as u64),
                )]),
            )
            .await;
        }

        let hash = compute_stable_hash(&state.pending_tool_calls);

        let tool_names: Vec<String> = state
            .pending_tool_calls
            .iter()
            .map(|tc| tc.function.name.clone())
            .collect();

        let completed_searches = successful_tool_count(
            state
                .extensions
                .get::<LoopDetectionState>()
                .expect("LoopDetectionState must be initialized in before_turn"),
            "search_file_content",
        );
        if completed_searches >= MAX_DOCUMENT_EVIDENCE_SEARCHES
            && tool_names.iter().any(|name| name == "search_file_content")
        {
            let before = state.pending_tool_calls.len();
            state
                .pending_tool_calls
                .retain(|call| call.function.name != "search_file_content");
            state
                .extensions
                .get_mut::<LoopDetectionState>()
                .expect("LoopDetectionState must be initialized in before_turn")
                .pending_warning = Some(format!(
                "[Document evidence circuit] 已进行 {MAX_DOCUMENT_EVIDENCE_SEARCHES} 次成功的本地证据检索。停止重复检索并整理覆盖情况；已有证据足够时完成文档，核心问题仍缺证时明确报告缺口并缩窄结论，不得用占位内容伪装完成或编造。"
            ));
            if state.pending_tool_calls.is_empty() {
                return Ok(HookAction::JumpToAgent {
                    reason_code: "document_evidence_budget_exhausted",
                    details: std::collections::BTreeMap::from([(
                        "completed_searches".to_string(),
                        serde_json::Value::from(completed_searches),
                    )]),
                });
            }
            ctx.record_intervention(
                self.name(),
                self.revision(),
                crate::agent_core::harness::MiddlewareHook::AfterLlm,
                crate::agent_core::harness::InterventionKind::RequestMutation,
                "document_evidence_calls_suppressed",
                state.round_count,
                None,
                std::collections::BTreeMap::from([
                    (
                        "completed_searches".to_string(),
                        serde_json::Value::from(completed_searches),
                    ),
                    (
                        "suppressed_calls".to_string(),
                        serde_json::Value::from(
                            before.saturating_sub(state.pending_tool_calls.len()) as u64,
                        ),
                    ),
                ]),
            )
            .await;
        }

        let ld = state
            .extensions
            .get_mut::<LoopDetectionState>()
            .expect("LoopDetectionState must be initialized in before_turn");

        // Layer 1: hash-based pattern repetition
        if ld.hash_window.len() >= self.window_size {
            ld.hash_window.pop_front();
        }
        ld.hash_window.push_back(hash);

        let repetitions = count_occurrences(&ld.hash_window, hash);
        let (hash_warn_threshold, hash_hard_limit) =
            repetition_limits(&tool_names, self.warn_threshold, self.hard_limit);

        if repetitions >= hash_hard_limit {
            warn!(
                repetitions,
                hard_limit = hash_hard_limit,
                tools = ?tool_names,
                "loop detection hard stop: repeated tool call pattern"
            );
            state.pending_tool_calls.clear();
            return Ok(HookAction::ForceEnd {
                reason_code: "repeated_tool_pattern_hard_limit",
                details: std::collections::BTreeMap::from([(
                    "repetitions".to_string(),
                    serde_json::Value::from(repetitions),
                )]),
            });
        }

        if repetitions >= hash_warn_threshold && ld.pending_warning.is_none() {
            let warning = format!(
                "[Loop warning] The same tool call pattern has repeated {} times. \
                 Try a different approach or simplify the problem.",
                repetitions
            );
            ld.pending_warning = Some(warning.clone());
            state.recovery_reason = Some(warning);
        }

        // Layer 2: frequency-based single tool overuse
        for name in &tool_names {
            // These tools form the required create/patch -> evidence -> inspect ->
            // layout -> render closure. A valid revision cycle invokes them again,
            // so raw per-turn frequency is not evidence of a loop. Exact repeated
            // calls are still covered by the stable-hash detector above.
            if is_document_contract_tool(name) {
                continue;
            }
            let count = ld.tool_freq.entry(name.clone()).or_insert(0);
            *count += 1;

            if *count >= self.tool_freq_hard {
                warn!(
                    tool = %name,
                    count = *count,
                    limit = self.tool_freq_hard,
                    "loop detection hard stop: tool frequency exceeded"
                );
                state.pending_tool_calls.clear();
                return Ok(HookAction::ForceEnd {
                    reason_code: "tool_frequency_hard_limit",
                    details: std::collections::BTreeMap::from([
                        (
                            "tool_name".to_string(),
                            serde_json::Value::from(name.clone()),
                        ),
                        ("count".to_string(), serde_json::Value::from(*count)),
                    ]),
                });
            }

            if *count >= self.tool_freq_warn && ld.pending_warning.is_none() {
                let warning = format!(
                    "[Loop warning] Tool `{}` has been called {} times. \
                     Consider whether you are making progress.",
                    name, count
                );
                ld.pending_warning = Some(warning.clone());
                state.recovery_reason = Some(warning);
            }
        }

        Ok(HookAction::Continue)
    }

    async fn after_tool(
        &self,
        state: &mut GraphState,
        _ctx: &GraphContext<'_>,
        request: &ToolCallRequest,
        result: &mut ToolResult,
    ) -> Result<()> {
        let Some(ld) = state.extensions.get_mut::<LoopDetectionState>() else {
            return Ok(());
        };
        let guidance = record_successful_tool(ld, &request.name, result.is_error);
        if ld.pending_warning.is_none() {
            ld.pending_warning = guidance;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::FunctionCall;

    fn make_tool_call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{}", name),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
            thought_signature: None,
        }
    }

    #[test]
    fn stable_hash_same_calls_same_hash() {
        let calls = vec![make_tool_call("read", r#"{"path":"/a.txt","offset":0}"#)];
        let h1 = compute_stable_hash(&calls);
        let h2 = compute_stable_hash(&calls);
        assert_eq!(h1, h2);
    }

    #[test]
    fn document_evidence_search_limit_allows_targeted_gap_closure() {
        assert_eq!(MAX_DOCUMENT_EVIDENCE_SEARCHES, 12);
    }

    #[test]
    fn revision_closure_tools_are_exempt_from_frequency_limit() {
        for name in DOCUMENT_CONTRACT_TOOLS {
            assert!(is_document_contract_tool(name));
        }
        assert!(!is_document_contract_tool("patch_document"));
        assert!(!is_document_contract_tool("search_file_content"));
    }

    #[test]
    fn revision_closure_gets_room_for_multiple_qa_cycles() {
        assert_eq!(
            repetition_limits(&["render_document".to_string()], 3, 5),
            (6, 8)
        );
        assert_eq!(
            repetition_limits(&["patch_document".to_string()], 3, 5),
            (3, 5)
        );
    }

    #[test]
    fn exhausted_web_family_is_suppressed_without_blocking_other_tools() {
        let mut state = GraphState::new();
        state.record_tool_result("web_read", "Error: 启动 check-deps.mjs 失败", true);
        state.record_tool_result("web_fetch_batch", "Error: 启动 check-deps.mjs 失败", true);
        state.pending_tool_calls = vec![
            make_tool_call("web_read", r#"{"url":"https://example.com"}"#),
            make_tool_call("web_query", r#"{"query":"independent search"}"#),
            make_tool_call(
                "patch_document",
                r#"{"documentId":"doc-1","expectedRevision":1,"patches":[]}"#,
            ),
        ];

        assert_eq!(
            suppress_exhausted_web_access_calls(&mut state),
            vec!["web_read"]
        );
        assert_eq!(state.pending_tool_calls.len(), 2);
        assert_eq!(state.pending_tool_calls[0].function.name, "web_query");
        assert_eq!(state.pending_tool_calls[1].function.name, "patch_document");
    }

    #[test]
    fn exhausted_search_family_still_allows_known_url_reads() {
        let mut state = GraphState::new();
        state.record_tool_result("web_search_domains", "Error: AnySearch unavailable", true);
        state.record_tool_result("web_query", "Error: AnySearch unavailable", true);
        state.pending_tool_calls = vec![
            make_tool_call("web_query", r#"{"query":"retry"}"#),
            make_tool_call("web_read", r#"{"url":"https://example.com/source"}"#),
        ];

        assert_eq!(
            suppress_exhausted_web_access_calls(&mut state),
            vec!["web_search"]
        );
        assert_eq!(state.pending_tool_calls.len(), 1);
        assert_eq!(state.pending_tool_calls[0].function.name, "web_read");
    }

    #[test]
    fn stable_hash_different_calls_different_hash() {
        let c1 = vec![make_tool_call("read", r#"{"path":"/a.txt"}"#)];
        let c2 = vec![make_tool_call("read", r#"{"path":"/b.txt"}"#)];
        assert_ne!(compute_stable_hash(&c1), compute_stable_hash(&c2));
    }

    #[test]
    fn stable_hash_read_file_same_bucket() {
        let c1 = vec![make_tool_call("read", r#"{"path":"/a.txt","offset":10}"#)];
        let c2 = vec![make_tool_call("read", r#"{"path":"/a.txt","offset":150}"#)];
        assert_eq!(compute_stable_hash(&c1), compute_stable_hash(&c2));
    }

    #[test]
    fn stable_hash_read_file_different_bucket() {
        let c1 = vec![make_tool_call("read", r#"{"path":"/a.txt","offset":10}"#)];
        let c2 = vec![make_tool_call("read", r#"{"path":"/a.txt","offset":250}"#)];
        assert_ne!(compute_stable_hash(&c1), compute_stable_hash(&c2));
    }

    #[test]
    fn stable_hash_read_artifact_distinguishes_progressing_offsets() {
        let first = vec![make_tool_call(
            "read_artifact",
            r#"{"artifact_id":"artifact-1","offset":2000,"limit":10000}"#,
        )];
        let second = vec![make_tool_call(
            "read_artifact",
            r#"{"artifact_id":"artifact-1","offset":12000,"limit":10000}"#,
        )];
        assert_ne!(compute_stable_hash(&first), compute_stable_hash(&second));
    }

    #[test]
    fn stable_hash_read_artifact_detects_same_offset() {
        let first = vec![make_tool_call(
            "read_artifact",
            r#"{"artifact_id":"artifact-1","offset":2000,"limit":10000}"#,
        )];
        let repeated = vec![make_tool_call(
            "read_artifact",
            r#"{"artifact_id":"artifact-1","offset":2000,"limit":5000}"#,
        )];
        assert_eq!(compute_stable_hash(&first), compute_stable_hash(&repeated));
    }

    #[test]
    fn stable_hash_search_file_content_distinguishes_queries() {
        let first = vec![make_tool_call(
            "search_file_content",
            r#"{"paths":["/report.txt"],"queries":["2030 projection"]}"#,
        )];
        let second = vec![make_tool_call(
            "search_file_content",
            r#"{"paths":["/report.txt"],"queries":["battery risk"]}"#,
        )];
        assert_ne!(compute_stable_hash(&first), compute_stable_hash(&second));
    }

    #[test]
    fn stable_hash_search_file_content_normalizes_array_order() {
        let first = vec![make_tool_call(
            "search_file_content",
            r#"{"paths":["/a.txt","/b.txt"],"queries":["risk","growth"]}"#,
        )];
        let reordered = vec![make_tool_call(
            "search_file_content",
            r#"{"paths":["/b.txt","/a.txt"],"queries":["growth","risk"]}"#,
        )];
        assert_eq!(compute_stable_hash(&first), compute_stable_hash(&reordered));
    }

    #[test]
    fn stable_hash_write_file_full_args() {
        let c1 = vec![make_tool_call(
            "write",
            r#"{"path":"/a.txt","content":"hello"}"#,
        )];
        let c2 = vec![make_tool_call(
            "write",
            r#"{"path":"/a.txt","content":"world"}"#,
        )];
        assert_ne!(compute_stable_hash(&c1), compute_stable_hash(&c2));
    }

    #[test]
    fn stable_hash_document_patches_include_patch_payload() {
        let first = vec![make_tool_call(
            "patch_document",
            r#"{"documentId":"doc-1","expectedRevision":1,"patches":[{"op":"replaceText","matchText":"old","value":"first"}]}"#,
        )];
        let second = vec![make_tool_call(
            "patch_document",
            r#"{"documentId":"doc-1","expectedRevision":1,"patches":[{"op":"replaceText","matchText":"old","value":"second"}]}"#,
        )];
        assert_ne!(compute_stable_hash(&first), compute_stable_hash(&second));
    }

    #[test]
    fn stable_hash_identical_document_patch_still_matches() {
        let patch = vec![make_tool_call(
            "patch_document",
            r#"{"documentId":"doc-1","expectedRevision":1,"patches":[{"op":"replaceText","matchText":"old","value":"new"}]}"#,
        )];
        assert_eq!(compute_stable_hash(&patch), compute_stable_hash(&patch));
    }

    #[test]
    fn stable_hash_sorted_order() {
        let c1 = vec![
            make_tool_call("alpha", r#"{"path":"/a"}"#),
            make_tool_call("beta", r#"{"path":"/b"}"#),
        ];
        let c2 = vec![
            make_tool_call("beta", r#"{"path":"/b"}"#),
            make_tool_call("alpha", r#"{"path":"/a"}"#),
        ];
        assert_eq!(compute_stable_hash(&c1), compute_stable_hash(&c2));
    }

    #[test]
    fn count_occurrences_works() {
        let mut window = VecDeque::new();
        window.push_back(1);
        window.push_back(2);
        window.push_back(1);
        window.push_back(3);
        window.push_back(1);
        assert_eq!(count_occurrences(&window, 1), 3);
        assert_eq!(count_occurrences(&window, 2), 1);
        assert_eq!(count_occurrences(&window, 99), 0);
    }

    #[test]
    fn document_evidence_budget_guidance_fires_after_first_comprehensive_search() {
        assert!(document_evidence_budget_guidance("search_file_content", 1)
            .is_some_and(|message| message.contains("materially different searches")));
        assert!(document_evidence_budget_guidance("search_file_content", 1)
            .is_some_and(|message| message.contains("closed allowlist")));
        assert!(document_evidence_budget_guidance("search_file_content", 2).is_none());
        assert!(document_evidence_budget_guidance("search_file_content", 3).is_none());
    }

    #[test]
    fn structured_data_guidance_discourages_fragmented_commands() {
        assert!(document_evidence_budget_guidance("bash", 1)
            .is_some_and(|message| message.contains("batch-extract")));
        assert!(document_evidence_budget_guidance("bash", 3)
            .is_some_and(|message| message.contains("Stop exploratory command fragmentation")));
        assert!(document_evidence_budget_guidance("bash", 2).is_none());
        assert!(document_evidence_budget_guidance("bash", 4).is_none());
    }

    #[test]
    fn failed_searches_do_not_consume_success_budget() {
        let mut state = LoopDetectionState {
            hash_window: VecDeque::new(),
            tool_freq: HashMap::from([("search_file_content".to_string(), 2)]),
            successful_tool_freq: HashMap::new(),
            pending_warning: None,
        };
        assert!(record_successful_tool(&mut state, "search_file_content", true).is_none());
        assert!(record_successful_tool(&mut state, "search_file_content", true).is_none());
        assert_eq!(successful_tool_count(&state, "search_file_content"), 0);
        assert_eq!(state.tool_freq["search_file_content"], 2);
    }

    #[test]
    fn successful_searches_advance_budget_and_guidance() {
        let mut state = LoopDetectionState {
            hash_window: VecDeque::new(),
            tool_freq: HashMap::new(),
            successful_tool_freq: HashMap::new(),
            pending_warning: None,
        };
        let first = record_successful_tool(&mut state, "search_file_content", false);
        assert!(first.is_some_and(|message| message.contains("closed allowlist")));
        assert_eq!(successful_tool_count(&state, "search_file_content"), 1);
        for _ in 1..MAX_DOCUMENT_EVIDENCE_SEARCHES {
            assert!(record_successful_tool(&mut state, "search_file_content", false).is_none());
        }
        assert_eq!(
            successful_tool_count(&state, "search_file_content"),
            MAX_DOCUMENT_EVIDENCE_SEARCHES
        );
    }

    #[test]
    fn stream_detector_stops_exact_repetition() {
        let mut detector = StreamDoomDetector::default();
        let block = "0123456789abcdef".repeat(4);
        assert!(detector.observe(&block.repeat(4)).is_some());
    }

    #[test]
    fn stream_detector_allows_normal_long_output() {
        let mut detector = StreamDoomDetector::default();
        let normal = (0..100)
            .map(|index| format!("line {index}: distinct explanatory content"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(detector.observe(&normal).is_none());
    }
}
