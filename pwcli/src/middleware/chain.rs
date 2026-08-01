use anyhow::Result;

use crate::graph::state::GraphState;
use crate::graph::GraphContext;
use crate::harness::{InterventionKind, MiddlewareHook};

use super::types::{HookAction, ModelRequest, ModelResponse, ToolCallRequest, ToolResult}; // ToolHandler/LlmHandler not needed yet (stubs)
use super::AgentMiddleware;

/// Middleware 调度器：持有有序的 middleware 列表，负责按正序/逆序 dispatch。
pub struct MiddlewareChain {
    middlewares: Vec<Box<dyn AgentMiddleware>>,
}

impl MiddlewareChain {
    pub fn new(middlewares: Vec<Box<dyn AgentMiddleware>>) -> Self {
        Self { middlewares }
    }

    /// before_turn: 正序执行，遇到 non-Continue 立即返回
    pub async fn dispatch_before_turn(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        for m in &self.middlewares {
            let action = m.before_turn(state, ctx).await?;
            if !matches!(action, HookAction::Continue) {
                record_control_action(m.as_ref(), &action, state, ctx, MiddlewareHook::BeforeTurn)
                    .await;
                return Ok(action);
            }
        }
        Ok(HookAction::Continue)
    }

    /// after_turn: 逆序执行
    pub async fn dispatch_after_turn(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<()> {
        for m in self.middlewares.iter().rev() {
            m.after_turn(state, ctx).await?;
        }
        Ok(())
    }

    /// before_llm: 正序执行，遇到 non-Continue 立即返回
    pub async fn dispatch_before_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        for m in &self.middlewares {
            let action = m.before_llm(state, ctx).await?;
            if !matches!(action, HookAction::Continue) {
                record_control_action(m.as_ref(), &action, state, ctx, MiddlewareHook::BeforeLlm)
                    .await;
                return Ok(action);
            }
        }
        Ok(HookAction::Continue)
    }

    /// after_llm: 逆序执行，遇到 non-Continue 立即返回
    pub async fn dispatch_after_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        for m in self.middlewares.iter().rev() {
            let action = m.after_llm(state, ctx).await?;
            if !matches!(action, HookAction::Continue) {
                record_control_action(m.as_ref(), &action, state, ctx, MiddlewareHook::AfterLlm)
                    .await;
                return Ok(action);
            }
        }
        Ok(HookAction::Continue)
    }

    /// wrap_llm: 简化版——按正序依次调用 wrap_llm，暂不构建完整洋葱链。
    /// 完整的洋葱链实现需要解决 &self middleware 引用的生命周期问题，
    /// 留到 executor 阶段实现。
    pub async fn dispatch_wrap_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        request: &mut ModelRequest,
    ) -> Result<ModelResponse> {
        // Placeholder: the actual LLM call will be wired in by the executor.
        // For now this just returns a default response since we don't have the
        // actual call handler here — the executor will bypass this and call
        // the individual wrap_llm hooks directly.
        let _ = (state, ctx, request);
        Ok(ModelResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            usage: crate::llm::TokenUsage::default(),
        })
    }

    /// wrap_tool: 简化版——按正序依次调用 wrap_tool。
    /// 同上，完整洋葱链留到 executor 阶段。
    pub async fn dispatch_wrap_tool(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        request: &ToolCallRequest,
    ) -> Result<ToolResult> {
        let _ = (state, ctx, request);
        Ok(ToolResult {
            content: String::new(),
            is_error: false,
            terminate: false,
        })
    }

    /// after_tool: 正序执行，每个 middleware 可修改工具执行结果
    pub async fn dispatch_after_tool(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        request: &ToolCallRequest,
        result: &mut ToolResult,
    ) -> Result<()> {
        for m in &self.middlewares {
            m.after_tool(state, ctx, request, result).await?;
        }
        Ok(())
    }

    pub fn middlewares(&self) -> &[Box<dyn AgentMiddleware>] {
        &self.middlewares
    }
}

async fn record_control_action(
    middleware: &dyn AgentMiddleware,
    action: &HookAction,
    state: &GraphState,
    ctx: &GraphContext<'_>,
    hook: MiddlewareHook,
) {
    let (reason_code, details) = match action {
        HookAction::Continue => return,
        HookAction::ForceEnd {
            reason_code,
            details,
        }
        | HookAction::JumpToAgent {
            reason_code,
            details,
        }
        | HookAction::Abort {
            reason_code,
            details,
            ..
        } => (*reason_code, details.clone()),
    };
    ctx.record_intervention(
        middleware.name(),
        middleware.revision(),
        hook,
        InterventionKind::ControlFlow,
        reason_code,
        state.round_count,
        None,
        details,
    )
    .await;
}
