use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    pub terminate: bool,
    pub details: Option<Value>,
    pub added_tool_names: Vec<String>,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            terminate: false,
            details: None,
            added_tool_names: Vec::new(),
        }
    }

    pub fn terminating(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            terminate: true,
            details: None,
            added_tool_names: Vec::new(),
        }
    }

    pub fn with_details(content: impl Into<String>, details: Value) -> Self {
        Self {
            content: content.into(),
            terminate: false,
            details: Some(details),
            added_tool_names: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Sequential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolImpact {
    Observe,
    ReversibleMutation,
    IrreversibleMutation,
    ExternalSideEffect,
    Control,
}

impl ToolImpact {
    pub fn requires_decision(self) -> bool {
        matches!(self, Self::IrreversibleMutation | Self::ExternalSideEffect)
    }
}

pub type ToolImpactResolver = fn(&Value) -> ToolImpact;
