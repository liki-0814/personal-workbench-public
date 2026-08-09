//! Provider-neutral model contracts.
//!
//! These compatibility exports define the new dependency surface while the
//! implementations are moved out of `llm::models` incrementally.

use serde::{Deserialize, Serialize};

/// Provider-neutral reasoning depth selected for an agent run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl ThinkingLevel {
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

impl From<bool> for ThinkingLevel {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::Medium
        } else {
            Self::Off
        }
    }
}

impl std::str::FromStr for ThinkingLevel {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            "ultra" => Ok(Self::Ultra),
            other => anyhow::bail!("unsupported thinking level: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffSemantics {
    #[default]
    Disabled,
    ProviderDefault,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingCapability {
    pub supported_levels: Vec<ThinkingLevel>,
    pub default_level: ThinkingLevel,
    pub off_semantics: OffSemantics,
    pub supports_visible_summary: bool,
}

pub use crate::ai::llm::models::{
    AiResponse, ChatMessage, FunctionCall, FunctionSchema, ImageAttachment, StopReason, TokenUsage,
    ToolCall, ToolSchema,
};
