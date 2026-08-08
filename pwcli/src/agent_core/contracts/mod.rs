//! Stable cross-layer data contracts and ports.
//!
//! During the single-crate migration, legacy modules re-export these types so
//! serialized data and existing call sites remain compatible.

pub mod event;
pub mod failure;
pub mod ids;
pub mod message;
pub mod model;
pub mod ports;
pub mod runtime;
pub mod session;
pub mod tool;

pub use ids::{AttemptId, SessionId, TaskId, WorkItemId};
pub use message::{ContentBlock, ConversationMessage, MessageRole};
pub use runtime::{
    QueuedInput, QueuedInputDelivery, QueuedInputPriority, QueuedInputSource, QueuedInputStatus,
    SessionRuntimeSnapshot,
};
pub use session::{Session, SessionState, WorkspaceBinding};

#[cfg(test)]
mod tests {
    #[test]
    fn compatibility_paths_share_the_same_contract_types() {
        let output = crate::agent_core::contracts::tool::ToolOutput::text("ok");
        let legacy: crate::runtime::tools::registry::ToolOutput = output;
        assert_eq!(legacy.content, "ok");

        let failure = crate::agent_core::reliability::FailureEnvelope::new(
            "test",
            crate::agent_core::reliability::FailureClass::Permanent,
            crate::agent_core::reliability::FailureSource::RuntimeTask,
            "failed",
        );
        let contract: crate::agent_core::contracts::failure::FailureEnvelope = failure.clone();
        assert_eq!(
            serde_json::to_value(contract).unwrap(),
            serde_json::to_value(failure).unwrap()
        );
    }
}
