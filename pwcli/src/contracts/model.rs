//! Provider-neutral model contracts.
//!
//! These compatibility exports define the new dependency surface while the
//! implementations are moved out of `llm::models` incrementally.

pub use crate::llm::models::{
    AiResponse, ChatMessage, FunctionCall, FunctionSchema, ImageAttachment, StopReason, TokenUsage,
    ToolCall, ToolSchema,
};
