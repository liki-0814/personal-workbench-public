//! Stable cross-layer data contracts and ports.
//!
//! During the single-crate migration, legacy modules re-export these types so
//! serialized data and existing call sites remain compatible.

pub mod event;
pub mod failure;
pub mod ids;
pub mod model;
pub mod ports;
pub mod tool;

pub use ids::{AttemptId, SessionId, TaskId, WorkItemId};

#[cfg(test)]
mod tests {
    #[test]
    fn compatibility_paths_share_the_same_contract_types() {
        let output = crate::contracts::tool::ToolOutput::text("ok");
        let legacy: crate::tools::registry::ToolOutput = output;
        assert_eq!(legacy.content, "ok");

        let failure = crate::reliability::FailureEnvelope::new(
            "test",
            crate::reliability::FailureClass::Permanent,
            crate::reliability::FailureSource::RuntimeTask,
            "failed",
        );
        let contract: crate::contracts::failure::FailureEnvelope = failure.clone();
        assert_eq!(
            serde_json::to_value(contract).unwrap(),
            serde_json::to_value(failure).unwrap()
        );
    }
}
