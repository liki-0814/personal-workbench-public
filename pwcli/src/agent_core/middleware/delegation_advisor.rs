//! 委托顾问（软自动）。
//!
//! 主 agent 在一个 turn 内连续做大量低层探索（read/grep/ls/find/
//! search_file_content）而从未委托时，在下一次 LLM 调用前注入一条软提示，
//! 建议把仓库/模块级代码调研委托给 code_agent。只建议、不强制——
//! 决策权仍在模型（对齐 Pi 的"模型自决"哲学）。

use anyhow::Result;
use async_trait::async_trait;

use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;
use crate::ai::llm::ChatMessage;

use super::types::{HookAction, ToolCallRequest, ToolResult};
use super::AgentMiddleware;

/// 视为"低层探索"的工具名。
const EXPLORATION_TOOLS: &[&str] = &["ls", "read", "find", "grep", "search_file_content"];

/// 任一委托工具被调用即清零连击（主 agent 已在委托）。
const DELEGATION_TOOLS: &[&str] = &["code_agent", "dispatch_tasks"];

/// 图状态里的顾问计数器。
#[derive(Default)]
struct AdvisorState {
    streak: u32,
    advised: bool,
    pending_hint: Option<String>,
}

/// 共享计数逻辑：观察一次工具执行，返回应当注入的提示文案
///（达到阈值且尚未提示过）。委托工具清零，出错/非探索工具不计数。
fn observe_exploration(
    state: &mut AdvisorState,
    threshold: u32,
    tool_name: &str,
    is_error: bool,
) -> Option<String> {
    if DELEGATION_TOOLS.contains(&tool_name) {
        state.streak = 0;
        return None;
    }
    if is_error || !EXPLORATION_TOOLS.contains(&tool_name) {
        return None;
    }
    state.streak += 1;
    if state.streak >= threshold && !state.advised {
        state.advised = true;
        return Some(hint_text(state.streak));
    }
    None
}

/// 纯计数封装（与 GraphContext 解耦，便于单测）。
pub struct AdvisorCounter {
    threshold: u32,
    state: AdvisorState,
}

impl AdvisorCounter {
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold: threshold.max(6),
            state: AdvisorState::default(),
        }
    }

    pub fn observe(&mut self, tool_name: &str, is_error: bool) -> Option<String> {
        observe_exploration(&mut self.state, self.threshold, tool_name, is_error)
    }
}

fn hint_text(streak: u32) -> String {
    format!(
        "[委托建议] 你已连续做了 {} 次低层文件探索（read/grep/ls/find 等）。\
         如果目标是仓库/模块级的代码调研（跨文件理解、设计调研、找 bug），\
         建议一次性委托给 code_agent（task 里写清目标与必要上下文），通常比逐个读文件更高效；\
         若只需再看一两个文件就能得出结论，请继续当前方式。这只是建议，由你决定。",
        streak
    )
}

pub struct DelegationAdvisorMiddleware {
    threshold: u32,
}

impl DelegationAdvisorMiddleware {
    pub fn new(threshold: u32) -> Self {
        Self { threshold }
    }
}

impl Default for DelegationAdvisorMiddleware {
    fn default() -> Self {
        Self::new(10)
    }
}

#[async_trait]
impl AgentMiddleware for DelegationAdvisorMiddleware {
    fn name(&self) -> &str {
        "DelegationAdvisor"
    }

    async fn after_tool(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        request: &ToolCallRequest,
        result: &mut ToolResult,
    ) -> Result<()> {
        // 目录里没有 code_agent（未启用编码 CLI）时不建议委托。
        let code_agent_available = ctx
            .tool_schemas
            .iter()
            .any(|schema| schema.function.name == "code_agent");
        if !code_agent_available {
            return Ok(());
        }
        if state.extensions.get::<AdvisorState>().is_none() {
            state.extensions.insert(AdvisorState::default());
        }
        let Some(advisor) = state.extensions.get_mut::<AdvisorState>() else {
            return Ok(());
        };
        if let Some(hint) = observe_exploration(
            advisor,
            self.threshold.max(6),
            &request.name,
            result.is_error,
        ) {
            advisor.pending_hint = Some(hint);
        }
        Ok(())
    }

    async fn before_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        let Some(advisor) = state.extensions.get_mut::<AdvisorState>() else {
            return Ok(HookAction::Continue);
        };
        let Some(hint) = advisor.pending_hint.take() else {
            return Ok(HookAction::Continue);
        };
        state.messages.push(ChatMessage {
            role: "user".to_string(),
            content: hint,
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        });
        ctx.record_intervention(
            self.name(),
            self.revision(),
            crate::agent_core::harness::MiddlewareHook::BeforeLlm,
            crate::agent_core::harness::InterventionKind::StateMutation,
            "delegation_hint_injected",
            state.round_count,
            None,
            std::collections::BTreeMap::new(),
        )
        .await;
        Ok(HookAction::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_clamped_to_at_least_six() {
        let mut counter = AdvisorCounter::new(1);
        for _ in 0..5 {
            assert!(counter.observe("read", false).is_none());
        }
        assert!(counter.observe("read", false).is_some());
    }

    #[test]
    fn hints_once_after_consecutive_exploration() {
        let mut counter = AdvisorCounter::new(6);
        for _ in 0..5 {
            assert!(counter.observe("grep", false).is_none());
        }
        let hint = counter.observe("find", false);
        assert!(hint.is_some_and(|text| text.contains("[委托建议]")));
        // 已提示过，不再重复
        assert!(counter.observe("read", false).is_none());
    }

    #[test]
    fn delegation_resets_the_streak() {
        let mut counter = AdvisorCounter::new(6);
        for _ in 0..5 {
            counter.observe("read", false);
        }
        assert!(counter.observe("code_agent", false).is_none());
        // 重新计数，5 次探索不足以触发（阈值 6）
        for _ in 0..5 {
            assert!(counter.observe("ls", false).is_none());
        }
        assert!(counter.observe("ls", false).is_some());
    }

    #[test]
    fn errors_and_non_exploration_tools_do_not_count() {
        let mut counter = AdvisorCounter::new(6);
        for _ in 0..5 {
            counter.observe("read", false);
        }
        assert!(counter.observe("read", true).is_none());
        assert!(counter.observe("write", false).is_none());
        assert!(counter.observe("bash", false).is_none());
        // 仍停在 5 次，未触发
        assert!(counter.observe("ssh_execute", false).is_none());
    }
}
