use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;
use crate::ai::llm::{ChatMessage, TokenUsage, ToolCall, ToolSchema};

/// Middleware hook 返回值，控制图执行流程。
#[derive(Debug)]
pub enum HookAction {
    Continue,
    ForceEnd {
        reason_code: &'static str,
        details: BTreeMap<String, Value>,
    },
    JumpToAgent {
        reason_code: &'static str,
        details: BTreeMap<String, Value>,
    },
    Abort {
        reason_code: &'static str,
        message: String,
        details: BTreeMap<String, Value>,
    },
}

/// LLM 调用请求（middleware 可修改）
pub struct ModelRequest {
    pub messages: Vec<ChatMessage>,
    pub system_prompt: String,
    pub tools: Option<Vec<ToolSchema>>,
    pub thinking: bool,
}

impl ModelRequest {
    pub fn from_state(state: &GraphState, ctx: &GraphContext<'_>) -> Self {
        Self {
            messages: state.messages.clone(),
            system_prompt: ctx.system_prompt.to_string(),
            tools: Some(ctx.tool_schemas.to_vec()),
            thinking: ctx.config.thinking_level.is_enabled(),
        }
    }
}

/// LLM 调用响应
pub struct ModelResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TokenUsage,
}

/// 工具调用请求
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// 工具执行结果
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub terminate: bool,
}

/// wrap_llm 的 inner handler 类型
pub type LlmHandler<'a> = Box<
    dyn FnOnce(
            &mut GraphState,
            &GraphContext<'_>,
            &mut ModelRequest,
        ) -> Pin<Box<dyn Future<Output = Result<ModelResponse>> + Send + 'a>>
        + Send
        + 'a,
>;

/// wrap_tool 的 inner handler 类型
pub type ToolHandler<'a> = Box<
    dyn FnOnce(
            &mut GraphState,
            &GraphContext<'_>,
            &ToolCallRequest,
        ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + 'a>>
        + Send
        + 'a,
>;
