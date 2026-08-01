pub mod chain;
pub mod dangling_tool_call;
pub mod loop_detection;
pub mod memory_mw;
pub mod subagent_limit;
pub mod summarization;
pub mod tool_output_budget;
pub mod types;
pub mod xml_tool_call;

use anyhow::Result;
use async_trait::async_trait;

use crate::graph::state::GraphState;
use crate::graph::GraphContext;
use types::{
    HookAction, LlmHandler, ModelRequest, ModelResponse, ToolCallRequest, ToolHandler, ToolResult,
};

pub use chain::MiddlewareChain;

/// Agent middleware trait.
///
/// All methods have default no-op implementations — middleware only overrides
/// the hooks it needs.
///
/// Execution order:
///   before_turn / before_llm / wrap_llm / wrap_tool: registration order (forward)
///   after_llm / after_turn: reverse registration order
#[async_trait]
pub trait AgentMiddleware: Send + Sync {
    fn name(&self) -> &str;

    fn revision(&self) -> u32 {
        1
    }

    async fn before_turn(
        &self,
        _state: &mut GraphState,
        _ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        Ok(HookAction::Continue)
    }

    async fn after_turn(&self, _state: &mut GraphState, _ctx: &GraphContext<'_>) -> Result<()> {
        Ok(())
    }

    async fn before_llm(
        &self,
        _state: &mut GraphState,
        _ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        Ok(HookAction::Continue)
    }

    async fn after_llm(
        &self,
        _state: &mut GraphState,
        _ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        Ok(HookAction::Continue)
    }

    async fn wrap_llm<'s>(
        &'s self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        request: &mut ModelRequest,
        next: LlmHandler<'_>,
    ) -> Result<ModelResponse> {
        next(state, ctx, request).await
    }

    async fn wrap_tool<'s>(
        &'s self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
        request: &ToolCallRequest,
        next: ToolHandler<'_>,
    ) -> Result<ToolResult> {
        next(state, ctx, request).await
    }

    /// 每个工具执行完毕后调用。可修改 result（如截断/落盘大输出）。
    async fn after_tool(
        &self,
        _state: &mut GraphState,
        _ctx: &GraphContext<'_>,
        _request: &ToolCallRequest,
        _result: &mut ToolResult,
    ) -> Result<()> {
        Ok(())
    }
}
