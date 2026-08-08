use std::collections::HashMap;

use anyhow::Result;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::agent_core::contracts::ports::{
    BackgroundRunOutcome, BackgroundTaskPort, OpaqueToolContext, PermissionOutcome, PermissionPort,
    ToolExecutorPort, ToolInvocationContext,
};
use crate::agent_core::contracts::tool::{ToolExecutionMode, ToolImpact};
use crate::agent_core::contracts::{ContentBlock, ConversationMessage, MessageRole, Session};
use crate::agent_core::decision::{
    DecisionOutcome, DecisionRequest, DecisionResume, DecisionReviewer, DecisionRisk,
    DecisionTrigger, DecisionVerdict, PendingDecision,
};
use crate::agent_core::graph::node::{NodeId, Transition};
use crate::agent_core::graph::state::GraphState;
use crate::agent_core::harness::{
    HarnessAuditSink, HarnessControl, HarnessFingerprint, InterventionKind, MiddlewareHook,
    MiddlewareIntervention,
};
use crate::agent_core::hooks::{HookEvent, HookRunner};
use crate::agent_core::middleware::chain::MiddlewareChain;
use crate::agent_core::middleware::types::{HookAction, ToolCallRequest, ToolResult};
use crate::agent_core::runner::{PermissionDecision, ToolEventSink, TurnSummary};
use crate::ai::llm::{
    ChatMessage, FunctionCall, LlmClient, LlmStreamOptions, StreamEvent, TokenUsage, ToolCall,
    ToolSchema,
};

#[cfg(test)]
const DECISION_REVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(420);
// Keep an open document review moving through observable checkpoints. This
// caps one planning turn, not the task: the completion contract schedules the
// next turn until evidence, inspection, layout and render are complete.
const DOCUMENT_ACTION_TURN_MAX_TOKENS: u32 = 16_384;

fn has_required_decision_consensus(verdict: &DecisionVerdict, basis_points: u16) -> bool {
    verdict.consensus > f32::from(basis_points) / 10_000.0
}

fn clarification_gate_tool_index(tool_calls: &[ToolCall]) -> Option<usize> {
    tool_calls.iter().position(|tool_call| {
        matches!(
            tool_call.function.name.as_str(),
            "request_user_choice" | "proceed_without_clarification"
        )
    })
}

fn normalized_tool_arguments(arguments: String) -> String {
    if arguments.trim().is_empty() {
        "{}".to_string()
    } else {
        arguments
    }
}

fn resolves_clarification_gate(tool_name: &str, is_error: bool) -> bool {
    !is_error
        && matches!(
            tool_name,
            "request_user_choice" | "proceed_without_clarification"
        )
}

fn has_safe_conservative_outcome(verdict: &DecisionVerdict) -> bool {
    matches!(
        verdict.outcome,
        DecisionOutcome::Revise | DecisionOutcome::GatherEvidence
    )
}

fn review_failure_verdict(trigger: DecisionTrigger, error: &anyhow::Error) -> DecisionVerdict {
    let recovery = trigger == DecisionTrigger::Recovery;
    DecisionVerdict {
        outcome: if recovery {
            DecisionOutcome::Revise
        } else {
            DecisionOutcome::Escalate
        },
        confidence: 0.0,
        consensus: 0.0,
        rationale: format!("MoA 两轮共识评审失败：{error}"),
        instruction: if recovery {
            "The recovery reviewer is unavailable; this does not imply that the prior tool failed. Reassess the remaining evidence gap from actual tool results. Avoid an exact duplicate call, but use a different query or tool when it adds necessary evidence; otherwise continue the deliverable with explicit limitations."
                .into()
        } else {
            "Pause and ask the user how to proceed.".into()
        },
        options: Vec::new(),
        rounds: 2,
        advisors: Vec::new(),
        usage: TokenUsage::default(),
    }
}

fn should_review_final(
    state: &GraphState,
    natural: &Transition,
    round_threshold: u32,
    tool_threshold: u32,
) -> bool {
    matches!(natural, Transition::End | Transition::Goto(NodeId::End))
        && !state.final_reviewed
        && (state.round_count >= round_threshold || state.tool_call_count >= tool_threshold)
}

/// 图配置
pub struct GraphConfig {
    pub max_rounds: u32,
    pub thinking: bool,
    pub yolo_mode: bool,
    pub decision_review_round_threshold: u32,
    pub decision_review_tool_threshold: u32,
    pub decision_timeout_seconds: u64,
    pub required_consensus_basis_points: u16,
    pub background_promotion_timeout_seconds: u64,
    pub background_eligible_tools: Vec<String>,
    pub unified_recovery: bool,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            max_rounds: 100,
            thinking: false,
            yolo_mode: false,
            decision_review_round_threshold: 6,
            decision_review_tool_threshold: 4,
            decision_timeout_seconds: 420,
            required_consensus_basis_points: 6_600,
            background_promotion_timeout_seconds: 60,
            background_eligible_tools: BG_ELIGIBLE_TOOLS
                .iter()
                .map(|tool| (*tool).into())
                .collect(),
            unified_recovery: true,
        }
    }
}

impl Clone for GraphConfig {
    fn clone(&self) -> Self {
        Self {
            max_rounds: self.max_rounds,
            thinking: self.thinking,
            yolo_mode: self.yolo_mode,
            decision_review_round_threshold: self.decision_review_round_threshold,
            decision_review_tool_threshold: self.decision_review_tool_threshold,
            decision_timeout_seconds: self.decision_timeout_seconds,
            required_consensus_basis_points: self.required_consensus_basis_points,
            background_promotion_timeout_seconds: self.background_promotion_timeout_seconds,
            background_eligible_tools: self.background_eligible_tools.clone(),
            unified_recovery: self.unified_recovery,
        }
    }
}

/// 图执行期间的不可变上下文。Middleware 和节点通过此访问外部资源。
pub struct GraphContext<'a> {
    pub llm: &'a LlmClient,
    pub tool_registry: &'a dyn ToolExecutorPort,
    pub permission_engine: &'a dyn PermissionPort,
    pub hook_runner: &'a HookRunner,
    pub tool_schemas: &'a [ToolSchema],
    pub system_prompt: &'a str,
    pub config: &'a GraphConfig,
    pub sink: &'a dyn ToolEventSink,
    pub cancel_token: &'a CancellationToken,
    pub session: &'a Mutex<Session>,
    pub background_tasks: Option<&'a dyn BackgroundTaskPort>,
    pub session_id: Option<&'a str>,
    pub harness: Option<&'a HarnessControl>,
    pub decision_reviewer: Option<&'a dyn DecisionReviewer>,
    pub harness_fingerprint: &'a HarnessFingerprint,
    pub audit_sink: Option<&'a dyn HarnessAuditSink>,
    pub tool_context: Option<OpaqueToolContext>,
    pub artifact_store: Option<&'a dyn crate::agent_core::contracts::ports::ArtifactStorePort>,
}

impl GraphContext<'_> {
    #[allow(clippy::too_many_arguments)]
    pub async fn record_intervention(
        &self,
        middleware: &str,
        middleware_revision: u32,
        hook: MiddlewareHook,
        kind: InterventionKind,
        reason_code: &str,
        round: u32,
        tool_call_id: Option<String>,
        details: std::collections::BTreeMap<String, Value>,
    ) {
        let intervention = MiddlewareIntervention::new(
            self.harness_fingerprint.clone(),
            middleware,
            middleware_revision,
            hook,
            kind,
            reason_code,
            round,
            tool_call_id,
            details,
        );
        info!(
            target: "pwcli::agent_core::harness::audit",
            intervention = %serde_json::to_string(&intervention).unwrap_or_default(),
            "middleware intervention"
        );
        if let Some(sink) = self.audit_sink {
            if let Err(error) = sink.record_intervention(&intervention).await {
                warn!(error = %error, "failed to persist middleware intervention");
            }
        }
    }
}

/// 图实例——持有 middleware chain 和配置。
/// 执行方法由后续 agent 实现。
pub struct AgentGraph {
    pub(crate) middlewares: MiddlewareChain,
    pub(crate) config: GraphConfig,
}

/// Truncate a string for display purposes (args preview, background task descriptions).
fn truncate_for_display(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect::<String>() + "…"
}

fn tool_invocation_context(
    ctx: &GraphContext<'_>,
    progress: Option<crate::agent_core::contracts::ports::ProgressEmitter>,
    image: Option<crate::agent_core::contracts::ports::ImageEmitter>,
) -> ToolInvocationContext {
    ToolInvocationContext {
        session_id: ctx.session_id.map(str::to_string),
        cancellation: ctx.cancel_token.clone(),
        progress,
        image,
        opaque: ctx.tool_context.clone(),
    }
}

/// Tools eligible for auto-promotion to background after 60s timeout.
const BG_ELIGIBLE_TOOLS: &[&str] = &["ssh_execute", "ssh_upload", "ssh_download"];

#[cfg(test)]
#[cfg(test)]
fn is_bg_eligible_tool(tool_name: &str) -> bool {
    BG_ELIGIBLE_TOOLS.contains(&tool_name)
}

fn is_configured_background_tool(config: &GraphConfig, tool_name: &str) -> bool {
    tool_name != "code_agent"
        && config
            .background_eligible_tools
            .iter()
            .any(|tool| tool == tool_name)
}

fn is_tool_allowed(tool_schemas: &[ToolSchema], tool_name: &str) -> bool {
    tool_schemas
        .iter()
        .any(|schema| schema.function.name == tool_name)
}

fn is_runtime_tool_allowed(ctx: &GraphContext<'_>, tool_name: &str) -> bool {
    is_tool_allowed(ctx.tool_schemas, tool_name)
        && ctx
            .harness
            .map(|harness| harness.is_tool_active(tool_name))
            .unwrap_or(true)
}

fn requests_background(arguments: &str) -> bool {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| value.get("background").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn resume_transition(resume: &DecisionResume) -> Transition {
    match resume {
        DecisionResume::Tool => Transition::Goto(NodeId::Tool),
        DecisionResume::Agent => Transition::Goto(NodeId::Agent),
        DecisionResume::End => Transition::End,
    }
}

fn private_decision_message(id: &str, verdict: &DecisionVerdict) -> ChatMessage {
    ChatMessage {
        role: "user".into(),
        content: format!(
            "<private_decision_review id=\"{}\" outcome=\"{:?}\">\nRationale: {}\nInstruction: {}\n</private_decision_review>",
            id, verdict.outcome, verdict.rationale, verdict.instruction
        ),
        images: Vec::new(),
        generated_images: Vec::new(),
        tool_calls: None,
        tool_call_id: None,
    }
}

fn continue_incomplete_artifact_contract(
    state: &mut GraphState,
    natural: &Transition,
) -> Option<Transition> {
    if !matches!(natural, Transition::End | Transition::Goto(NodeId::End)) {
        return None;
    }
    let instruction = state.artifact_completion.take_document_nudge()?;
    state.messages.push(ChatMessage {
        role: "user".into(),
        content: format!(
            "<private_artifact_completion_contract>\n{instruction}\n</private_artifact_completion_contract>"
        ),
        images: Vec::new(),
        generated_images: Vec::new(),
        tool_calls: None,
        tool_call_id: None,
    });
    Some(Transition::Goto(NodeId::Agent))
}

impl AgentGraph {
    pub fn builder() -> GraphBuilder {
        GraphBuilder::new()
    }

    // ─── Public entry point ───

    pub async fn run_turn(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<TurnSummary> {
        let action = self.middlewares.dispatch_before_turn(state, ctx).await?;
        if let HookAction::Abort { message, .. } = action {
            state.last_content = message;
            return Ok(TurnSummary::from_state(state));
        }

        let summary = self.execute_loop(state, ctx).await?;

        self.middlewares.dispatch_after_turn(state, ctx).await?;

        Ok(summary)
    }

    // ─── Core loop ───

    async fn execute_loop(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<TurnSummary> {
        loop {
            if ctx.cancel_token.is_cancelled() {
                return Ok(TurnSummary::cancelled());
            }

            if state.round_count >= self.config.max_rounds {
                return Ok(TurnSummary::from_state(state));
            }

            // ═══ Agent Node ═══
            let transition = self.execute_agent_node(state, ctx).await?;
            match transition {
                Transition::Goto(NodeId::Decision) => {
                    let transition = self.execute_decision_node(state, ctx).await?;
                    match transition {
                        Transition::Goto(NodeId::Tool) => {}
                        Transition::Goto(NodeId::Agent) => continue,
                        Transition::Goto(NodeId::End) | Transition::End => {
                            return Ok(TurnSummary::from_state(state));
                        }
                        Transition::Goto(NodeId::Decision) => continue,
                    }
                }
                Transition::End => {
                    if self.append_follow_up_messages(state, ctx).await? {
                        continue;
                    }
                    return Ok(TurnSummary::from_state(state));
                }
                Transition::Goto(NodeId::Tool) => {}
                Transition::Goto(NodeId::Agent) => continue,
                Transition::Goto(NodeId::End) => return Ok(TurnSummary::from_state(state)),
            }

            // ═══ Tool Node ═══
            let transition = self.execute_tool_node(state, ctx).await?;
            state.mark_tool_batch_complete();
            match transition {
                Transition::Goto(NodeId::Decision) => {
                    let transition = self.execute_decision_node(state, ctx).await?;
                    match transition {
                        Transition::Goto(NodeId::Agent) => continue,
                        Transition::Goto(NodeId::Tool) => continue,
                        Transition::Goto(NodeId::End) | Transition::End => {
                            return Ok(TurnSummary::from_state(state));
                        }
                        Transition::Goto(NodeId::Decision) => continue,
                    }
                }
                Transition::End => return Ok(TurnSummary::from_state(state)),
                Transition::Goto(NodeId::Agent) => {
                    self.append_steering_messages(state, ctx).await?;
                    continue;
                }
                Transition::Goto(NodeId::Tool) => continue,
                Transition::Goto(NodeId::End) => return Ok(TurnSummary::from_state(state)),
            }
        }
    }

    async fn append_steering_messages(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<bool> {
        let Some(harness) = ctx.harness else {
            return Ok(false);
        };
        let messages = harness.drain_steer().await?;
        self.append_queued_user_messages(state, ctx, messages).await
    }

    async fn append_follow_up_messages(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<bool> {
        let Some(harness) = ctx.harness else {
            return Ok(false);
        };
        let messages = harness.drain_follow_up().await?;
        self.append_queued_user_messages(state, ctx, messages).await
    }

    async fn append_queued_user_messages(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        messages: Vec<ChatMessage>,
    ) -> Result<bool> {
        if messages.is_empty() {
            return Ok(false);
        }
        let mut session = ctx.session.lock().await;
        for message in messages {
            session.add_message(ConversationMessage::new_user(message.content.clone()));
            state.messages.push(message);
        }
        Ok(true)
    }

    // ─── Agent node: LLM call + routing ───

    async fn execute_agent_node(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<Transition> {
        state.round_count += 1;

        // 1. before_llm
        let action = self.middlewares.dispatch_before_llm(state, ctx).await?;
        match action {
            HookAction::Continue => {}
            HookAction::Abort { message, .. } => {
                state.last_content = message;
                return Ok(Transition::End);
            }
            HookAction::ForceEnd { .. } => return Ok(Transition::End),
            HookAction::JumpToAgent { .. } => return Ok(Transition::Goto(NodeId::Agent)),
        }

        // 2. LLM streaming call
        let (content, tool_calls, usage, response_truncated) =
            self.stream_llm_call(state, ctx).await?;

        ctx.sink.on_context_usage(usage, state.round_count);

        // 3. Update state
        state.last_content = content.clone();
        state.pending_tool_calls = tool_calls.clone();
        state.response_truncated = response_truncated;
        state.token_usage = TokenUsage {
            prompt_tokens: state.token_usage.prompt_tokens + usage.prompt_tokens,
            completion_tokens: state.token_usage.completion_tokens + usage.completion_tokens,
            total_tokens: state.token_usage.total_tokens + usage.total_tokens,
        };

        // 4. Push assistant ChatMessage to LLM protocol messages
        let assistant_msg = ChatMessage {
            images: Vec::new(),
            generated_images: Vec::new(),
            role: "assistant".to_string(),
            content,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
        };
        state.messages.push(assistant_msg);

        // 5. Persist assistant message to session
        self.persist_assistant_message(state, ctx).await;

        // 6. after_llm
        let action = self.middlewares.dispatch_after_llm(state, ctx).await?;

        match action {
            HookAction::Continue => {
                let natural = if state.pending_tool_calls.is_empty() {
                    Transition::End
                } else {
                    Transition::Goto(NodeId::Tool)
                };
                Ok(self.maybe_schedule_decision(state, ctx, natural))
            }
            HookAction::ForceEnd { .. } => Ok(Transition::End),
            HookAction::JumpToAgent { .. } => Ok(Transition::Goto(NodeId::Agent)),
            HookAction::Abort { message, .. } => {
                state.last_content = message;
                Ok(Transition::End)
            }
        }
    }

    fn maybe_schedule_decision(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        natural: Transition,
    ) -> Transition {
        if let Some(next) = continue_incomplete_artifact_contract(state, &natural) {
            return next;
        }
        if ctx.decision_reviewer.is_none() || state.pending_decision.is_some() {
            return natural;
        }
        // A choice made in the consensus dialog is an explicit user ruling for
        // this turn. Do not send the selected plan back through the same MoA
        // gate; normal tool permission checks still apply.
        if has_moa_user_decision(&state.messages) {
            return natural;
        }

        if let Some((question, proposal, evidence)) = state.agent_decision_request.take() {
            let request = DecisionRequest::new(
                DecisionTrigger::AgentRequest,
                DecisionRisk::Low,
                question,
                proposal,
                evidence,
                Vec::new(),
            );
            if !state.reviewed_decisions.contains(&request.id) {
                state.pending_decision = Some(PendingDecision {
                    request,
                    resume: DecisionResume::Agent,
                });
                return Transition::Goto(NodeId::Decision);
            }
        }

        if let Some(reason) = state.recovery_reason.take() {
            let request = DecisionRequest::new(
                DecisionTrigger::Recovery,
                DecisionRisk::Elevated,
                "How should the agent recover without repeating the same failure?",
                state.last_content.clone(),
                vec![reason],
                state.pending_tool_calls.clone(),
            );
            if !state.reviewed_decisions.contains(&request.id) {
                state.pending_decision = Some(PendingDecision {
                    request,
                    resume: match natural {
                        Transition::Goto(NodeId::Tool) => DecisionResume::Tool,
                        Transition::Goto(NodeId::Agent) => DecisionResume::Agent,
                        _ => DecisionResume::End,
                    },
                });
                return Transition::Goto(NodeId::Decision);
            }
        }

        if !state.pending_tool_calls.is_empty() {
            let high_impact = state.pending_tool_calls.iter().any(|call| {
                let arguments = serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
                ctx.tool_registry
                    .impact_for_call(&call.function.name, &arguments)
                    .is_some_and(|impact| impact.requires_decision())
            });
            if high_impact {
                let proposal = state
                    .pending_tool_calls
                    .iter()
                    .map(|call| format!("{}({})", call.function.name, call.function.arguments))
                    .collect::<Vec<_>>()
                    .join("\n");
                let request = DecisionRequest::new(
                    DecisionTrigger::PreAction,
                    DecisionRisk::High,
                    "Should the proposed high-impact tool calls execute now?",
                    proposal,
                    Vec::new(),
                    state.pending_tool_calls.clone(),
                );
                if !state.reviewed_decisions.contains(&request.id) {
                    state.pending_decision = Some(PendingDecision {
                        request,
                        resume: DecisionResume::Tool,
                    });
                    return Transition::Goto(NodeId::Decision);
                }
            }
        }

        let complex_final = state.pending_tool_calls.is_empty()
            && should_review_final(
                state,
                &natural,
                ctx.config.decision_review_round_threshold,
                ctx.config.decision_review_tool_threshold,
            );
        if complex_final {
            state.final_reviewed = true;
            let request = DecisionRequest::new(
                DecisionTrigger::FinalReview,
                DecisionRisk::Elevated,
                "Is the proposed final response adequately supported and complete?",
                state.last_content.clone(),
                Vec::new(),
                Vec::new(),
            );
            if !state.reviewed_decisions.contains(&request.id) {
                state.pending_decision = Some(PendingDecision {
                    request,
                    resume: DecisionResume::End,
                });
                return Transition::Goto(NodeId::Decision);
            }
        }

        natural
    }

    async fn execute_decision_node(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<Transition> {
        let Some(pending) = state.pending_decision.take() else {
            return Ok(Transition::Goto(NodeId::Agent));
        };
        let Some(reviewer) = ctx.decision_reviewer else {
            return Ok(resume_transition(&pending.resume));
        };

        state.reviewed_decisions.insert(pending.request.id.clone());
        ctx.sink.on_decision_started(
            &pending.request.id,
            pending.request.trigger,
            pending.request.risk,
        );
        if let Some(harness) = ctx.harness {
            harness
                .events()
                .emit(crate::agent_core::harness::HarnessEvent::DecisionStarted {
                    id: pending.request.id.clone(),
                    trigger: pending.request.trigger,
                    risk: pending.request.risk,
                })
                .await?;
        }

        let decision_timeout = std::time::Duration::from_secs(ctx.config.decision_timeout_seconds);
        let reviewed = tokio::time::timeout(
            decision_timeout,
            reviewer.review(&pending.request, &state.messages, ctx.cancel_token),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "decision review timed out after {}s",
                decision_timeout.as_secs()
            )
        })
        .and_then(|result| result);
        let verdict = match reviewed {
            Ok(verdict)
                if has_required_decision_consensus(
                    &verdict,
                    ctx.config.required_consensus_basis_points,
                ) || has_safe_conservative_outcome(&verdict) =>
            {
                verdict
            }
            Ok(mut verdict) => {
                verdict.outcome = DecisionOutcome::Escalate;
                verdict.rationale = format!(
                    "两轮评审后共识度为 {:.0}%，未超过 66%。{}{}",
                    verdict.consensus * 100.0,
                    if verdict.rationale.is_empty() {
                        ""
                    } else {
                        " "
                    },
                    verdict.rationale
                );
                verdict
            }
            Err(error) => review_failure_verdict(pending.request.trigger, &error),
        };
        state.token_usage.prompt_tokens += verdict.usage.prompt_tokens;
        state.token_usage.completion_tokens += verdict.usage.completion_tokens;
        state.token_usage.total_tokens += verdict.usage.total_tokens;
        for advisor in &verdict.advisors {
            ctx.sink
                .on_decision_advisor(&pending.request.id, &advisor.model, advisor.succeeded);
        }
        ctx.sink.on_decision_resolved(&pending.request.id, &verdict);
        if let Some(harness) = ctx.harness {
            harness
                .events()
                .emit(crate::agent_core::harness::HarnessEvent::DecisionResolved {
                    id: pending.request.id.clone(),
                    outcome: verdict.outcome,
                    confidence: verdict.confidence,
                    consensus: verdict.consensus,
                    rationale: verdict.rationale.clone(),
                })
                .await?;
        }

        match verdict.outcome {
            DecisionOutcome::Proceed => Ok(resume_transition(&pending.resume)),
            DecisionOutcome::Revise | DecisionOutcome::GatherEvidence => {
                if pending.request.trigger == DecisionTrigger::FinalReview {
                    state.final_reviewed = false;
                }
                state.reviewed_decisions.remove(&pending.request.id);
                state.pending_tool_calls.clear();
                state
                    .messages
                    .push(private_decision_message(&pending.request.id, &verdict));
                Ok(Transition::Goto(NodeId::Agent))
            }
            DecisionOutcome::Escalate => {
                state.pending_tool_calls.clear();
                let message = format!(
                    "MoA 未形成必要共识，已暂停执行。{}",
                    if verdict.rationale.is_empty() {
                        String::new()
                    } else {
                        format!("\n\n{}", verdict.rationale)
                    }
                );
                ctx.sink
                    .on_decision_escalated(&pending.request.id, &verdict);
                ctx.sink.on_assistant_start();
                ctx.sink.on_text_delta(&message);
                ctx.sink.on_assistant_end();
                state.last_content = message.clone();
                state.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: message.clone(),
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    tool_calls: None,
                    tool_call_id: None,
                });
                ctx.session
                    .lock()
                    .await
                    .add_message(ConversationMessage::new_assistant(message));
                if let Some(harness) = ctx.harness {
                    harness
                        .events()
                        .emit(
                            crate::agent_core::harness::HarnessEvent::DecisionEscalated {
                                id: pending.request.id,
                                rationale: verdict.rationale,
                                options: verdict.options,
                            },
                        )
                        .await?;
                }
                Ok(Transition::End)
            }
        }
    }

    // ─── Tool node: execute pending tool calls ───

    async fn execute_tool_node(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<Transition> {
        let mut tool_calls = std::mem::take(&mut state.pending_tool_calls);
        let mut has_background_task = false;
        let mut all_terminate = !tool_calls.is_empty();

        if state.response_truncated {
            state.response_truncated = false;
            for tc in &tool_calls {
                let message = format!(
                    "Tool call '{}' was not executed because the model response hit its output token limit and the arguments may be incomplete. Re-issue the tool call with complete arguments.",
                    tc.function.name
                );
                ctx.sink
                    .on_tool_result(&tc.id, &tc.function.name, &message, true, None);
                self.push_tool_result(state, ctx, &tc.id, &message, true, None)
                    .await;
            }
            return Ok(Transition::Goto(NodeId::Agent));
        }

        let clarification_gate_available = ctx.tool_registry.has_tool("request_user_choice")
            && ctx.tool_registry.has_tool("proceed_without_clarification");
        if clarification_gate_available
            && !state.clarification_gate_resolved
            && !tool_calls.is_empty()
        {
            let Some(gate_index) = clarification_gate_tool_index(&tool_calls) else {
                let message = "Clarification gate unresolved. Before any other tool, call exactly one of `request_user_choice` when a material choice is missing, or `proceed_without_clarification` when the request is already safe and complete enough to execute.";
                for tool_call in &tool_calls {
                    ctx.sink.on_tool_call(
                        &tool_call.id,
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                    );
                    ctx.sink.on_tool_result(
                        &tool_call.id,
                        &tool_call.function.name,
                        message,
                        true,
                        None,
                    );
                    self.push_tool_result(state, ctx, &tool_call.id, message, true, None)
                        .await;
                }
                return Ok(Transition::Goto(NodeId::Agent));
            };
            let gate_call = tool_calls.remove(gate_index);
            let message = "Tool was not executed because the clarification gate must be the only call in its first batch.";
            for tool_call in &tool_calls {
                ctx.sink.on_tool_call(
                    &tool_call.id,
                    &tool_call.function.name,
                    &tool_call.function.arguments,
                );
                ctx.sink.on_tool_result(
                    &tool_call.id,
                    &tool_call.function.name,
                    message,
                    true,
                    None,
                );
                self.push_tool_result(state, ctx, &tool_call.id, message, true, None)
                    .await;
            }
            tool_calls = vec![gate_call];
            all_terminate = true;
        }

        let requires_sequential = tool_calls.iter().any(|tool_call| {
            is_configured_background_tool(ctx.config, &tool_call.function.name)
                || requests_background(&tool_call.function.arguments)
                || ctx.tool_registry.execution_mode(&tool_call.function.name)
                    == Some(ToolExecutionMode::Sequential)
        });
        if !requires_sequential {
            return self
                .execute_tool_batch_parallel(state, ctx, tool_calls)
                .await;
        }

        for tc in &tool_calls {
            let args_str = tc.function.arguments.clone();
            let args_value: Value =
                serde_json::from_str(&args_str).unwrap_or(Value::Object(Default::default()));

            ctx.sink.on_tool_call(&tc.id, &tc.function.name, &args_str);

            // The schemas sent to the model are the effective capability
            // allowlist for this turn. Enforce that same boundary before any
            // background or permission branch so a forged / stale tool call
            // cannot reach a handler that was intentionally hidden.
            if !is_runtime_tool_allowed(ctx, &tc.function.name)
                || !ctx.tool_registry.has_tool(&tc.function.name)
            {
                let deny_msg = "工具不在当前会话的允许列表中".to_string();
                ctx.sink.on_permission_denied(&tc.function.name);
                ctx.sink
                    .on_tool_result(&tc.id, &tc.function.name, &deny_msg, true, None);
                self.push_tool_result(state, ctx, &tc.id, &deny_msg, true, None)
                    .await;
                all_terminate = false;
                continue;
            }

            // === Permission gate ===
            if !ctx.config.yolo_mode {
                match ctx.permission_engine.check_call(
                    &tc.function.name,
                    &args_str,
                    ctx.tool_registry
                        .impact_for_call(&tc.function.name, &args_value)
                        .unwrap_or(ToolImpact::ExternalSideEffect),
                ) {
                    PermissionOutcome::Allow => {}
                    PermissionOutcome::Deny => {
                        let deny_msg = "工具被策略拒绝执行".to_string();
                        ctx.sink.on_permission_denied(&tc.function.name);
                        ctx.sink
                            .on_tool_result(&tc.id, &tc.function.name, &deny_msg, true, None);
                        self.push_tool_result(state, ctx, &tc.id, &deny_msg, true, None)
                            .await;
                        all_terminate = false;
                        continue;
                    }
                    PermissionOutcome::Prompt => {
                        ctx.sink.on_permission_prompt(&tc.function.name, &args_str);
                        match ctx.sink.ask_permission(&tc.function.name, &args_str).await {
                            PermissionDecision::Allow => {}
                            PermissionDecision::Deny => {
                                let deny_msg = "用户拒绝了工具执行".to_string();
                                ctx.sink.on_permission_denied(&tc.function.name);
                                ctx.sink.on_tool_result(
                                    &tc.id,
                                    &tc.function.name,
                                    &deny_msg,
                                    true,
                                    None,
                                );
                                self.push_tool_result(state, ctx, &tc.id, &deny_msg, true, None)
                                    .await;
                                all_terminate = false;
                                continue;
                            }
                            PermissionDecision::DenyWithReason(reason) => {
                                ctx.sink.on_permission_denied(&tc.function.name);
                                ctx.sink.on_tool_result(
                                    &tc.id,
                                    &tc.function.name,
                                    &reason,
                                    true,
                                    None,
                                );
                                self.push_tool_result(state, ctx, &tc.id, &reason, true, None)
                                    .await;
                                all_terminate = false;
                                continue;
                            }
                        }
                    }
                }
            }

            // === Background task: model args contain "background": true ===
            if let (Some(bg_mgr), Some(sid)) = (ctx.background_tasks, ctx.session_id) {
                if tc.function.name != "code_agent"
                    && args_value
                        .get("background")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                {
                    let tool_name = tc.function.name.clone();
                    let desc = format!("{}({})", tool_name, truncate_for_display(&args_str, 80));
                    let args_clone = args_value.clone();
                    match bg_mgr
                        .spawn_tool(sid.to_string(), tool_name.clone(), desc, args_clone)
                        .await
                    {
                        Ok(task_id) => {
                            let placeholder = format!(
                                "⏳ 已安排后台任务 #{} — 工具 {} 将在后台执行，完成后自动通知。",
                                task_id, tool_name
                            );
                            self.push_tool_result(state, ctx, &tc.id, &placeholder, false, None)
                                .await;
                            ctx.sink
                                .on_tool_result(&tc.id, &tool_name, &placeholder, false, None);
                            has_background_task = true;
                            all_terminate = false;
                        }
                        Err(e) => {
                            let err_msg = format!("后台任务创建失败: {}，将同步执行", e);
                            warn!(tool = %tool_name, error = %e, "后台任务创建失败");
                            self.push_tool_result(state, ctx, &tc.id, &err_msg, true, None)
                                .await;
                            ctx.sink
                                .on_tool_result(&tc.id, &tool_name, &err_msg, true, None);
                            all_terminate = false;
                        }
                    }
                    continue;
                }
            }

            // === Hook: pre-execute ===
            ctx.hook_runner.run(&HookEvent::ToolPreExecute {
                tool_name: tc.function.name.clone(),
                arguments: args_str.clone(),
            });
            if let Some(harness) = ctx.harness {
                harness
                    .events()
                    .emit(crate::agent_core::harness::HarnessEvent::BeforeTool {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        arguments: args_value.clone(),
                    })
                    .await?;
            }

            // === Set up progress/image emitters ===
            let progress_em = ctx.sink.progress_emitter_for(&tc.id);
            let image_em = ctx.sink.image_emitter_for(&tc.id);

            // === Execute tool (with auto-background for eligible tools) ===
            let bg_eligible = is_configured_background_tool(ctx.config, &tc.function.name);

            let retry_policy = tool_retry_policy(&tc.function.name);
            let unified_recovery = ctx.config.unified_recovery;
            let mut attempt = 0u32;
            let (result_str, is_err, terminate, failure) = loop {
                let (result_str, is_err, terminate) = if bg_eligible {
                    if let Some(bg_mgr) = ctx.background_tasks {
                        let bg_timeout = std::time::Duration::from_secs(
                            ctx.config.background_promotion_timeout_seconds,
                        );
                        let desc = format!(
                            "{}({})",
                            tc.function.name,
                            truncate_for_display(&args_str, 80)
                        );
                        match bg_mgr
                            .execute_with_promotion(
                                tc.function.name.clone(),
                                desc,
                                args_value.clone(),
                                tool_invocation_context(ctx, progress_em.clone(), image_em.clone()),
                                bg_timeout,
                            )
                            .await
                        {
                            Ok(BackgroundRunOutcome::Completed(result)) => (result, false, false),
                            Ok(BackgroundRunOutcome::Promoted { task_id }) => {
                                let msg = format!(
                                    "⚡ 已自动转为后台任务 #{}（运行超过 60s），完成后自动通知。",
                                    task_id
                                );
                                info!(tool = %tc.function.name, task_id = %task_id, "前台超时，自动转后台");
                                has_background_task = true;
                                (msg, false, false)
                            }
                            Err(e) => {
                                warn!(tool = %tc.function.name, error = %e, "tool execution failed");
                                (format!("Error: {}", e), true, false)
                            }
                        }
                    } else {
                        let result = ctx
                            .tool_registry
                            .execute(
                                &tc.function.name,
                                &args_value,
                                tool_invocation_context(ctx, progress_em.clone(), image_em.clone()),
                            )
                            .await;
                        match result {
                            Ok(output) => (output.content, false, false),
                            Err(e) => {
                                warn!(tool = %tc.function.name, error = %e, "tool execution failed");
                                (format!("Error: {}", e), true, false)
                            }
                        }
                    }
                } else {
                    let result = ctx
                        .tool_registry
                        .execute(
                            &tc.function.name,
                            &args_value,
                            tool_invocation_context(ctx, progress_em.clone(), image_em.clone()),
                        )
                        .await;
                    match result {
                        Ok(output) => {
                            if let Some(details) = output.details.as_ref() {
                                ctx.sink.on_tool_details(&tc.id, &tc.function.name, details);
                            }
                            (
                                crate::ai::llm::deferred_tools::attach(
                                    output.content,
                                    &output.added_tool_names,
                                ),
                                false,
                                output.terminate,
                            )
                        }
                        Err(e) => {
                            warn!(tool = %tc.function.name, error = %e, "tool execution failed");
                            (format!("Error: {}", e), true, false)
                        }
                    }
                };

                if !is_err {
                    break (result_str, false, terminate, None);
                }

                let failure = crate::agent_core::reliability::classify_tool_failure(
                    &tc.function.name,
                    &result_str,
                    crate::agent_core::reliability::FailureSource::ForegroundTool,
                    attempt,
                    retry_policy.max_attempts,
                )
                .with_correlation_id(tc.id.clone());
                if unified_recovery && failure.can_auto_retry(retry_policy) {
                    let retry_after =
                        crate::agent_core::reliability::parse_retry_after_secs(&result_str);
                    let delay = crate::agent_core::reliability::retry_delay(attempt, retry_after);
                    let next_retry_at = (chrono::Utc::now()
                        + chrono::Duration::from_std(delay).unwrap_or_default())
                    .to_rfc3339();
                    let recovering = failure.clone().mark_auto_retrying(Some(next_retry_at));
                    ctx.sink.on_tool_recovery(
                        &tc.id,
                        &tc.function.name,
                        &recovering,
                        "auto_retrying",
                    );
                    tokio::time::sleep(delay).await;
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                break (result_str, true, terminate, Some(failure));
            };
            debug!(tool = %tc.function.name, is_err = is_err, result_len = result_str.len(),
                "tool executed");

            ctx.sink.on_tool_result(
                &tc.id,
                &tc.function.name,
                &result_str,
                is_err,
                failure.as_ref(),
            );

            // === Hook: post-execute ===
            ctx.hook_runner.run(&HookEvent::ToolPostExecute {
                tool_name: tc.function.name.clone(),
                result: result_str.clone(),
                duration_ms: 0,
            });

            // === Middleware after_tool (e.g. ToolOutputBudgetMiddleware: externalize large results) ===
            let tc_request = ToolCallRequest {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                arguments: args_value.clone(),
            };
            let mut tool_result = ToolResult {
                content: result_str,
                is_error: is_err,
                terminate,
            };
            self.middlewares
                .dispatch_after_tool(state, ctx, &tc_request, &mut tool_result)
                .await?;
            if let Some(harness) = ctx.harness {
                harness
                    .events()
                    .emit(crate::agent_core::harness::HarnessEvent::AfterTool {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        content: tool_result.content.clone(),
                        is_error: tool_result.is_error,
                    })
                    .await?;
            }

            state.record_tool_result(
                &tc.function.name,
                &tool_result.content,
                tool_result.is_error,
            );
            if resolves_clarification_gate(&tc.function.name, tool_result.is_error) {
                state.clarification_gate_resolved = true;
            }
            if tc.function.name == "request_decision_review" {
                state.agent_decision_request = Some((
                    args_value
                        .get("question")
                        .and_then(Value::as_str)
                        .unwrap_or("Review the acting agent's decision")
                        .to_string(),
                    args_value
                        .get("proposal")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    args_value
                        .get("evidence")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                ));
            }

            self.push_tool_result(
                state,
                ctx,
                &tc.id,
                &tool_result.content,
                tool_result.is_error,
                failure,
            )
            .await;
            all_terminate &= tool_result.terminate;
        }

        if has_background_task {
            info!("后台任务已安排，结束当前 turn");
            return Ok(Transition::End);
        }

        if all_terminate {
            // A successful terminating tool already produced the final artifact.
            // Re-entering decision review can ask the model to "gather evidence"
            // and repeat the same side effect (for example, create a second
            // Archify document).
            return Ok(Transition::End);
        }

        Ok(self.maybe_schedule_decision(state, ctx, Transition::Goto(NodeId::Agent)))
    }

    async fn execute_tool_batch_parallel(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        tool_calls: Vec<ToolCall>,
    ) -> Result<Transition> {
        let batch_size = tool_calls.len();
        let mut outcomes: Vec<
            Option<(
                ToolCall,
                String,
                bool,
                bool,
                Option<crate::agent_core::reliability::FailureEnvelope>,
            )>,
        > = vec![None; batch_size];
        let mut prepared = Vec::new();

        for (index, tool_call) in tool_calls.into_iter().enumerate() {
            let args = match serde_json::from_str::<Value>(&tool_call.function.arguments) {
                Ok(args) => args,
                Err(error) => {
                    let message = format!("Invalid JSON arguments: {}", error);
                    let failure = Some(crate::agent_core::reliability::classify_tool_failure(
                        &tool_call.function.name,
                        &message,
                        crate::agent_core::reliability::FailureSource::ForegroundTool,
                        0,
                        crate::agent_core::reliability::MAX_AUTO_RETRIES,
                    ));
                    ctx.sink.on_tool_result(
                        &tool_call.id,
                        &tool_call.function.name,
                        &message,
                        true,
                        failure.as_ref(),
                    );
                    outcomes[index] = Some((tool_call, message, true, false, failure));
                    continue;
                }
            };

            ctx.sink.on_tool_call(
                &tool_call.id,
                &tool_call.function.name,
                &tool_call.function.arguments,
            );

            if !is_runtime_tool_allowed(ctx, &tool_call.function.name)
                || !ctx.tool_registry.has_tool(&tool_call.function.name)
            {
                let message = "工具不在当前会话的允许列表中".to_string();
                let failure = Some(crate::agent_core::reliability::classify_tool_failure(
                    &tool_call.function.name,
                    &message,
                    crate::agent_core::reliability::FailureSource::ForegroundTool,
                    0,
                    crate::agent_core::reliability::MAX_AUTO_RETRIES,
                ));
                ctx.sink.on_permission_denied(&tool_call.function.name);
                ctx.sink.on_tool_result(
                    &tool_call.id,
                    &tool_call.function.name,
                    &message,
                    true,
                    failure.as_ref(),
                );
                outcomes[index] = Some((tool_call, message, true, false, failure));
                continue;
            }

            if let Err(error) = ctx
                .tool_registry
                .validate_arguments(&tool_call.function.name, &args)
            {
                let message = error.to_string();
                let failure = Some(crate::agent_core::reliability::classify_tool_failure(
                    &tool_call.function.name,
                    &message,
                    crate::agent_core::reliability::FailureSource::ForegroundTool,
                    0,
                    crate::agent_core::reliability::MAX_AUTO_RETRIES,
                ));
                ctx.sink.on_tool_result(
                    &tool_call.id,
                    &tool_call.function.name,
                    &message,
                    true,
                    failure.as_ref(),
                );
                outcomes[index] = Some((tool_call, message, true, false, failure));
                continue;
            }

            if !ctx.config.yolo_mode {
                let decision = match ctx.permission_engine.check_call(
                    &tool_call.function.name,
                    &tool_call.function.arguments,
                    ctx.tool_registry
                        .impact_for_call(&tool_call.function.name, &args)
                        .unwrap_or(ToolImpact::ExternalSideEffect),
                ) {
                    PermissionOutcome::Allow => None,
                    PermissionOutcome::Deny => Some("工具被策略拒绝执行".to_string()),
                    PermissionOutcome::Prompt => {
                        ctx.sink.on_permission_prompt(
                            &tool_call.function.name,
                            &tool_call.function.arguments,
                        );
                        match ctx
                            .sink
                            .ask_permission(&tool_call.function.name, &tool_call.function.arguments)
                            .await
                        {
                            PermissionDecision::Allow => None,
                            PermissionDecision::Deny => Some("用户拒绝了工具执行".to_string()),
                            PermissionDecision::DenyWithReason(reason) => Some(reason),
                        }
                    }
                };
                if let Some(message) = decision {
                    let failure = Some(crate::agent_core::reliability::classify_tool_failure(
                        &tool_call.function.name,
                        &message,
                        crate::agent_core::reliability::FailureSource::ForegroundTool,
                        0,
                        crate::agent_core::reliability::MAX_AUTO_RETRIES,
                    ));
                    ctx.sink.on_permission_denied(&tool_call.function.name);
                    ctx.sink.on_tool_result(
                        &tool_call.id,
                        &tool_call.function.name,
                        &message,
                        true,
                        failure.as_ref(),
                    );
                    outcomes[index] = Some((tool_call, message, true, false, failure));
                    continue;
                }
            }

            ctx.hook_runner.run(&HookEvent::ToolPreExecute {
                tool_name: tool_call.function.name.clone(),
                arguments: tool_call.function.arguments.clone(),
            });
            if let Some(harness) = ctx.harness {
                harness
                    .events()
                    .emit(crate::agent_core::harness::HarnessEvent::BeforeTool {
                        id: tool_call.id.clone(),
                        name: tool_call.function.name.clone(),
                        arguments: args.clone(),
                    })
                    .await?;
            }
            prepared.push((index, tool_call, args));
        }

        let mut running = FuturesUnordered::new();
        for (index, tool_call, args) in prepared {
            let progress = ctx.sink.progress_emitter_for(&tool_call.id);
            let image = ctx.sink.image_emitter_for(&tool_call.id);
            running.push(async move {
                let started = std::time::Instant::now();
                let result = ctx
                    .tool_registry
                    .execute(
                        &tool_call.function.name,
                        &args,
                        tool_invocation_context(ctx, progress, image),
                    )
                    .await;
                (index, tool_call, args, result, started.elapsed())
            });
        }

        while let Some((index, tool_call, args, execution, duration)) = running.next().await {
            let (content, is_error, terminate) = match execution {
                Ok(output) => {
                    if let Some(details) = output.details.as_ref() {
                        ctx.sink
                            .on_tool_details(&tool_call.id, &tool_call.function.name, details);
                    }
                    (
                        crate::ai::llm::deferred_tools::attach(
                            output.content,
                            &output.added_tool_names,
                        ),
                        false,
                        output.terminate,
                    )
                }
                Err(error) => (format!("Error: {}", error), true, false),
            };
            ctx.hook_runner.run(&HookEvent::ToolPostExecute {
                tool_name: tool_call.function.name.clone(),
                result: content.clone(),
                duration_ms: duration.as_millis().try_into().unwrap_or(u64::MAX),
            });
            let request = ToolCallRequest {
                id: tool_call.id.clone(),
                name: tool_call.function.name.clone(),
                arguments: args,
            };
            let mut result = ToolResult {
                content,
                is_error,
                terminate,
            };
            self.middlewares
                .dispatch_after_tool(state, ctx, &request, &mut result)
                .await?;
            if let Some(harness) = ctx.harness {
                harness
                    .events()
                    .emit(crate::agent_core::harness::HarnessEvent::AfterTool {
                        id: tool_call.id.clone(),
                        name: tool_call.function.name.clone(),
                        content: result.content.clone(),
                        is_error: result.is_error,
                    })
                    .await?;
            }
            let failure = if result.is_error {
                Some(crate::agent_core::reliability::classify_tool_failure(
                    &tool_call.function.name,
                    &result.content,
                    crate::agent_core::reliability::FailureSource::ForegroundTool,
                    0,
                    crate::agent_core::reliability::MAX_AUTO_RETRIES,
                ))
            } else {
                None
            };
            ctx.sink.on_tool_result(
                &tool_call.id,
                &tool_call.function.name,
                &result.content,
                result.is_error,
                failure.as_ref(),
            );
            outcomes[index] = Some((
                tool_call,
                result.content,
                result.is_error,
                result.terminate,
                failure,
            ));
        }

        let outcome_flags = outcomes
            .iter()
            .filter_map(|outcome| {
                outcome
                    .as_ref()
                    .map(|(_, _, is_error, terminate, _)| (*is_error, *terminate))
            })
            .collect::<Vec<_>>();
        let should_terminate = successful_batch_should_terminate(&outcome_flags, batch_size);
        for outcome in outcomes.into_iter().flatten() {
            let (tool_call, content, is_error, _, failure) = outcome;
            state.record_tool_result(&tool_call.function.name, &content, is_error);
            self.push_tool_result(state, ctx, &tool_call.id, &content, is_error, failure)
                .await;
        }

        if should_terminate {
            // Preserve the terminal contract for parallel tool batches too.
            Ok(Transition::End)
        } else {
            Ok(self.maybe_schedule_decision(state, ctx, Transition::Goto(NodeId::Agent)))
        }
    }

    // ─── LLM streaming call ───

    async fn stream_llm_call(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<(String, Vec<ToolCall>, TokenUsage, bool)> {
        let runtime_tools: Vec<ToolSchema> = ctx
            .tool_schemas
            .iter()
            .filter(|schema| {
                ctx.harness
                    .map(|harness| harness.is_tool_active(&schema.function.name))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        let tools = if runtime_tools.is_empty() {
            None
        } else {
            Some(runtime_tools.as_slice())
        };

        let mut retry_attempt: u32 = 0;
        let mut doom_retry_attempt: u32 = 0;
        let mut request_messages = state.messages.clone();
        'retry: loop {
            ctx.sink.on_thinking_start();
            let mut stream = ctx.llm.chat_stream_with_options(
                &request_messages,
                Some(ctx.system_prompt),
                tools,
                None,
                LlmStreamOptions {
                    thinking: ctx.config.thinking,
                    max_tokens: state
                        .artifact_completion
                        .is_open()
                        .then_some(DOCUMENT_ACTION_TURN_MAX_TOKENS),
                    temperature: None,
                },
            );

            let mut content = String::new();
            let mut tool_call_order: Vec<String> = Vec::new();
            let mut tool_call_state: HashMap<String, (String, String, Option<String>)> =
                HashMap::new();
            let mut usage: Option<TokenUsage> = None;
            let mut error_msg: Option<String> = None;
            let mut emitted_assistant_start = false;
            let mut cancelled = false;
            let mut response_truncated = false;
            let mut doom_reason = None;
            let mut doom_detector =
                crate::agent_core::middleware::loop_detection::StreamDoomDetector::default();

            loop {
                let next_or_cancel = async {
                    tokio::select! {
                        biased;
                        _ = ctx.cancel_token.cancelled() => None,
                        item = stream.next() => Some(item),
                    }
                };
                let Some(maybe_ev) = next_or_cancel.await else {
                    cancelled = true;
                    break;
                };
                let Some(ev) = maybe_ev else { break };
                match ev {
                    StreamEvent::FirstToken => {}
                    StreamEvent::ThinkingDelta(t) => {
                        ctx.sink.on_thinking_delta(&t);
                    }
                    StreamEvent::TextDelta(t) => {
                        if !emitted_assistant_start {
                            emitted_assistant_start = true;
                            ctx.sink.on_assistant_segment_start(state.round_count);
                            ctx.sink.on_assistant_start();
                        }
                        content.push_str(&t);
                        ctx.sink.on_text_delta(&t);
                        if let Some(reason) = doom_detector.observe(&t) {
                            doom_reason = Some(reason);
                            break;
                        }
                    }
                    StreamEvent::ToolCallStart {
                        id,
                        name,
                        thought_signature,
                    } => {
                        if !id.is_empty() {
                            if !tool_call_state.contains_key(&id) {
                                tool_call_order.push(id.clone());
                            }
                            tool_call_state
                                .entry(id.clone())
                                .and_modify(|(n, _, sig)| {
                                    if n.is_empty() {
                                        *n = name.clone();
                                    }
                                    if sig.is_none() && thought_signature.is_some() {
                                        *sig = thought_signature.clone();
                                    }
                                })
                                .or_insert((name.clone(), String::new(), thought_signature));
                            ctx.sink.on_tool_call_streaming(&id, &name);
                        }
                    }
                    StreamEvent::ToolCallDelta {
                        id,
                        arguments_delta,
                    } => {
                        if let Some((_, args, _)) = tool_call_state.get_mut(&id) {
                            args.push_str(&arguments_delta);
                        }
                        ctx.sink.on_tool_call_args_delta(&id, &arguments_delta);
                    }
                    StreamEvent::ToolCallEnd { .. } => {}
                    StreamEvent::ToolResult { .. } => {}
                    StreamEvent::ToolRecovery { .. } => {}
                    StreamEvent::ToolProgress { .. } => {}
                    StreamEvent::ToolImage { .. }
                    | StreamEvent::ToolDocument { .. }
                    | StreamEvent::ToolDecision { .. } => {}
                    StreamEvent::ResponseStop(reason) => {
                        response_truncated = reason.is_length();
                    }
                    StreamEvent::ContextUsage { .. } => {}
                    StreamEvent::Done(u) => {
                        usage = u;
                        break;
                    }
                    StreamEvent::Error(msg) => {
                        error_msg = Some(msg);
                        break;
                    }
                    StreamEvent::DecisionStarted { .. }
                    | StreamEvent::DecisionAdvisor { .. }
                    | StreamEvent::DecisionResolved { .. }
                    | StreamEvent::DecisionEscalated { .. }
                    | StreamEvent::AssistantSegmentStart { .. }
                    | StreamEvent::AssistantSegmentEnd { .. }
                    | StreamEvent::StreamReset { .. } => {}
                }
            }

            if emitted_assistant_start {
                ctx.sink.on_assistant_end();
                ctx.sink
                    .on_assistant_segment_end(state.round_count, !tool_call_state.is_empty());
            }

            if cancelled {
                info!("stream_llm_call cancelled by user");
                anyhow::bail!("已取消");
            }
            if let Some(reason) = doom_reason {
                state.stream_loop_recoveries = state.stream_loop_recoveries.saturating_add(1);
                state.recovery_reason = Some(reason.clone());
                if doom_retry_attempt == 0 {
                    doom_retry_attempt += 1;
                    ctx.sink.on_stream_retry(&reason);
                    request_messages = state.messages.clone();
                    request_messages.push(ChatMessage {
                        role: "user".into(),
                        content: format!(
                            "<private_stream_recovery>The previous response was aborted because {reason}. Start over once, avoid repeating text, and use a materially different concise formulation.</private_stream_recovery>"
                        ),
                        images: Vec::new(),
                        generated_images: Vec::new(),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                    continue 'retry;
                }
                anyhow::bail!("模型流式输出连续两次陷入高置信重复循环：{reason}");
            }
            if let Some(ref msg) = error_msg {
                let had_output = !content.is_empty() || !tool_call_state.is_empty();
                if !had_output && crate::ai::llm::retry::is_retriable_stream_error(msg) {
                    if let Some(delay) = crate::ai::llm::retry::next_backoff(retry_attempt) {
                        warn!(
                            attempt = retry_attempt + 1,
                            model = %ctx.llm.provider().model,
                            delay_ms = delay.as_millis() as u64,
                            error = %msg,
                            "retriable stream error, retrying"
                        );
                        tokio::time::sleep(delay).await;
                        retry_attempt += 1;
                        continue 'retry;
                    }
                }
                anyhow::bail!("{}", msg);
            }

            let tool_calls: Vec<ToolCall> = tool_call_order
                .into_iter()
                .filter_map(|id| {
                    tool_call_state
                        .remove(&id)
                        .map(|(name, args, thought_sig)| ToolCall {
                            id,
                            kind: "function".to_string(),
                            function: FunctionCall {
                                name,
                                // Empty wire arguments are a valid empty JSON
                                // object for optional-argument tools. Required
                                // properties are still enforced by ToolRegistry.
                                arguments: normalized_tool_arguments(args),
                            },
                            thought_signature: thought_sig,
                        })
                })
                .collect();

            return Ok((
                content,
                tool_calls,
                usage.unwrap_or_default(),
                response_truncated,
            ));
        } // 'retry loop end
    }

    // ─── Session persistence helpers ───

    async fn persist_assistant_message(&self, state: &GraphState, ctx: &GraphContext<'_>) {
        let now = chrono::Utc::now();
        let mut session_blocks: Vec<ContentBlock> = Vec::new();
        if !state.last_content.is_empty() {
            session_blocks.push(ContentBlock::Text {
                text: state.last_content.clone(),
            });
        }
        for tc in &state.pending_tool_calls {
            let input = serde_json::from_str::<Value>(&tc.function.arguments)
                .unwrap_or(Value::Object(Default::default()));
            session_blocks.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                input,
            });
        }

        let msg = ConversationMessage {
            id: format!("msg_{}", now.timestamp_nanos_opt().unwrap_or(0)),
            role: MessageRole::Assistant,
            content: session_blocks,
            created_at: now,
            parent_id: None,
            model: Some(ctx.llm.provider().model.clone()),
            token_usage: Some(state.token_usage),
        };

        let mut session = ctx.session.lock().await;
        session.add_message(msg);
    }

    async fn push_tool_result(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        tool_call_id: &str,
        result: &str,
        is_error: bool,
        failure: Option<crate::agent_core::reliability::FailureEnvelope>,
    ) {
        state.messages.push(ChatMessage {
            role: "tool".to_string(),
            content: result.to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
        });

        let now = chrono::Utc::now();
        let msg = ConversationMessage {
            id: format!("msg_{}", now.timestamp_nanos_opt().unwrap_or(0)),
            role: MessageRole::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_call_id.to_string(),
                content: result.to_string(),
                is_error,
                failure,
            }],
            created_at: now,
            parent_id: None,
            model: None,
            token_usage: None,
        };

        let mut session = ctx.session.lock().await;
        session.add_message(msg);
    }
}

fn tool_retry_policy(tool_name: &str) -> crate::agent_core::reliability::ToolRetryPolicy {
    crate::agent_core::reliability::ToolRetryPolicy {
        allow_auto_retry: crate::agent_core::reliability::is_idempotent_read_tool(tool_name),
        max_attempts: crate::agent_core::reliability::MAX_AUTO_RETRIES,
    }
}

fn successful_batch_should_terminate(outcomes: &[(bool, bool)], expected_count: usize) -> bool {
    expected_count > 0
        && outcomes.len() == expected_count
        && outcomes.iter().all(|(is_error, _)| !is_error)
        && outcomes.iter().any(|(_, terminate)| *terminate)
}

const MOA_USER_DECISION_MARKER: &str = "<!-- pwb-moa-user-decision:";

fn has_moa_user_decision(messages: &[ChatMessage]) -> bool {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .is_some_and(|message| message.content.contains(MOA_USER_DECISION_MARKER))
}

/// 图构建器
pub struct GraphBuilder {
    middlewares: Vec<Box<dyn crate::agent_core::middleware::AgentMiddleware>>,
    config: GraphConfig,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
            config: GraphConfig::default(),
        }
    }

    pub fn config(mut self, config: GraphConfig) -> Self {
        self.config = config;
        self
    }

    pub fn middleware(
        mut self,
        m: impl crate::agent_core::middleware::AgentMiddleware + 'static,
    ) -> Self {
        self.middlewares.push(Box::new(m));
        self
    }

    pub fn middlewares(
        mut self,
        ms: Vec<Box<dyn crate::agent_core::middleware::AgentMiddleware>>,
    ) -> Self {
        self.middlewares = ms;
        self
    }

    pub fn build(self) -> AgentGraph {
        AgentGraph {
            middlewares: MiddlewareChain::new(self.middlewares),
            config: self.config,
        }
    }
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        continue_incomplete_artifact_contract, has_moa_user_decision,
        has_required_decision_consensus, has_safe_conservative_outcome, is_bg_eligible_tool,
        is_configured_background_tool, is_tool_allowed, normalized_tool_arguments,
        resolves_clarification_gate, review_failure_verdict, should_review_final,
        successful_batch_should_terminate, GraphConfig, DECISION_REVIEW_TIMEOUT,
        DOCUMENT_ACTION_TURN_MAX_TOKENS,
    };
    use crate::agent_core::decision::{DecisionOutcome, DecisionTrigger, DecisionVerdict};
    use crate::ai::llm::{ChatMessage, FunctionSchema, TokenUsage, ToolSchema};

    #[test]
    fn complete_decision_review_is_time_bounded() {
        assert_eq!(DECISION_REVIEW_TIMEOUT, std::time::Duration::from_secs(420));
    }

    #[test]
    fn document_action_turn_has_a_checkpoint_budget() {
        assert_eq!(DOCUMENT_ACTION_TURN_MAX_TOKENS, 16_384);
    }

    fn schema(name: &str) -> ToolSchema {
        ToolSchema {
            kind: "function".to_string(),
            function: FunctionSchema {
                name: name.to_string(),
                description: String::new(),
                parameters: serde_json::json!({ "type": "object" }),
            },
        }
    }

    #[test]
    fn advertised_tool_is_allowed() {
        assert!(is_tool_allowed(&[schema("data_query")], "data_query"));
    }

    #[test]
    fn hidden_or_unknown_tool_is_denied() {
        let schemas = [schema("data_query")];
        assert!(!is_tool_allowed(&schemas, "run_command"));
        assert!(!is_tool_allowed(&[], "data_query"));
    }

    #[test]
    fn empty_stream_arguments_become_an_empty_json_object() {
        assert_eq!(normalized_tool_arguments(String::new()), "{}");
        assert_eq!(normalized_tool_arguments("  \n".into()), "{}");
        assert_eq!(
            normalized_tool_arguments(r#"{"pattern":"*.rs"}"#.into()),
            r#"{"pattern":"*.rs"}"#
        );
    }

    #[test]
    fn clarification_gate_resolves_only_after_success() {
        assert!(resolves_clarification_gate(
            "proceed_without_clarification",
            false
        ));
        assert!(resolves_clarification_gate("request_user_choice", false));
        assert!(!resolves_clarification_gate(
            "proceed_without_clarification",
            true
        ));
        assert!(!resolves_clarification_gate("find", false));
    }

    #[test]
    fn durable_tools_stay_foreground_so_their_results_can_be_projected() {
        assert!(!is_bg_eligible_tool("read_pdf"));
        assert!(!is_bg_eligible_tool("code_agent"));
        let mut config = GraphConfig::default();
        config.background_eligible_tools.push("code_agent".into());
        assert!(!is_configured_background_tool(&config, "code_agent"));
    }

    #[test]
    fn moa_requires_more_than_sixty_six_percent_consensus() {
        let verdict = |consensus| DecisionVerdict {
            outcome: DecisionOutcome::Proceed,
            confidence: 1.0,
            consensus,
            rationale: String::new(),
            instruction: String::new(),
            options: Vec::new(),
            rounds: 1,
            advisors: Vec::new(),
            usage: TokenUsage::default(),
        };
        assert!(!has_required_decision_consensus(&verdict(0.66), 6_600));
        assert!(has_required_decision_consensus(&verdict(0.6601), 6_600));
    }

    #[test]
    fn low_consensus_conservative_verdict_can_return_to_agent() {
        let verdict = DecisionVerdict {
            outcome: DecisionOutcome::GatherEvidence,
            confidence: 0.8,
            consensus: 0.6,
            rationale: String::new(),
            instruction: String::new(),
            options: Vec::new(),
            rounds: 2,
            advisors: Vec::new(),
            usage: TokenUsage::default(),
        };
        assert!(!has_required_decision_consensus(&verdict, 6_600));
        assert!(has_safe_conservative_outcome(&verdict));
    }

    #[test]
    fn unavailable_recovery_review_revises_instead_of_blocking() {
        let verdict = review_failure_verdict(
            DecisionTrigger::Recovery,
            &anyhow::anyhow!("invalid judge JSON"),
        );
        assert_eq!(verdict.outcome, DecisionOutcome::Revise);
        assert!(verdict
            .instruction
            .contains("does not imply that the prior tool failed"));
    }

    #[test]
    fn unavailable_high_impact_review_still_escalates() {
        let verdict = review_failure_verdict(
            DecisionTrigger::PreAction,
            &anyhow::anyhow!("review unavailable"),
        );
        assert_eq!(verdict.outcome, DecisionOutcome::Escalate);
    }

    #[test]
    fn tool_results_return_to_agent_without_premature_final_review() {
        let mut state = crate::agent_core::graph::state::GraphState::new();
        state.tool_call_count = 4;
        state.last_content = "draft planning text".into();
        assert!(!should_review_final(
            &state,
            &crate::agent_core::graph::node::Transition::Goto(
                crate::agent_core::graph::node::NodeId::Agent
            ),
            6,
            4,
        ));
    }

    #[test]
    fn completed_complex_answer_gets_final_review() {
        let mut state = crate::agent_core::graph::state::GraphState::new();
        state.tool_call_count = 4;
        state.last_content = "supported final answer".into();
        assert!(should_review_final(
            &state,
            &crate::agent_core::graph::node::Transition::End,
            6,
            4,
        ));
    }

    #[test]
    fn completed_tool_only_deliverable_gets_final_review() {
        let mut state = crate::agent_core::graph::state::GraphState::new();
        state.tool_call_count = 4;
        state.last_content.clear();
        assert!(should_review_final(
            &state,
            &crate::agent_core::graph::node::Transition::End,
            6,
            4,
        ));
    }

    #[test]
    fn incomplete_document_contract_returns_main_harness_to_agent() {
        let mut state = crate::agent_core::graph::state::GraphState::new();
        state.record_tool_result("create_document", "created", false);
        let next = continue_incomplete_artifact_contract(
            &mut state,
            &crate::agent_core::graph::node::Transition::End,
        );
        assert!(matches!(
            next,
            Some(crate::agent_core::graph::node::Transition::Goto(
                crate::agent_core::graph::node::NodeId::Agent
            ))
        ));
        assert!(state.messages.last().is_some_and(|message| {
            message
                .content
                .contains("private_artifact_completion_contract")
                && message.content.contains("`inspect_document`")
        }));
    }

    #[test]
    fn completed_document_contract_does_not_change_natural_stop() {
        let mut state = crate::agent_core::graph::state::GraphState::new();
        for tool in [
            "create_document",
            "inspect_document",
            "inspect_document_layout",
            "render_document",
        ] {
            state.record_tool_result(tool, "ok", false);
        }
        assert!(continue_incomplete_artifact_contract(
            &mut state,
            &crate::agent_core::graph::node::Transition::End
        )
        .is_none());
    }

    #[test]
    fn latest_user_consensus_choice_bypasses_repeated_review() {
        let message = |role: &str, content: &str| ChatMessage {
            role: role.into(),
            content: content.into(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        };
        assert!(has_moa_user_decision(&[message(
            "user",
            "执行方案 A\n<!-- pwb-moa-user-decision:d1:o1 -->"
        )]));
        assert!(!has_moa_user_decision(&[
            message("user", "<!-- pwb-moa-user-decision:d1:o1 -->"),
            message("assistant", "ok"),
            message("user", "新的普通请求"),
        ]));
    }

    #[test]
    fn successful_parallel_batch_terminates_when_one_tool_is_terminal() {
        assert!(successful_batch_should_terminate(
            &[(false, false), (false, false), (false, true)],
            3
        ));
        assert!(!successful_batch_should_terminate(
            &[(false, false), (true, false), (false, true)],
            3
        ));
        assert!(!successful_batch_should_terminate(&[(false, true)], 2));
        assert!(!successful_batch_should_terminate(&[], 0));
    }
}
