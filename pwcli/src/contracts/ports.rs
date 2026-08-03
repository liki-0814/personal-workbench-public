use anyhow::Result;
use async_trait::async_trait;

use crate::contracts::{SessionId, WorkItemId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionExecutionContext {
    pub session_id: SessionId,
    pub workspace: String,
    pub generation: u64,
    pub work_item_id: Option<WorkItemId>,
}

#[derive(Debug, Clone)]
pub struct TaskPublishRequest {
    pub idempotency_key: String,
    pub join: String,
    pub work_item_id: Option<WorkItemId>,
    pub tasks: serde_json::Value,
    pub publisher_model: Option<PublisherModelContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherModelContext {
    pub provider_id: Option<String>,
    pub model: String,
    pub effort: Option<String>,
    pub thinking: bool,
}

#[async_trait]
pub trait SessionContextPort: Send + Sync {
    async fn resolve(&self, session_id: &SessionId) -> Result<SessionExecutionContext>;
}

#[async_trait]
pub trait TaskPublisherPort: Send + Sync {
    async fn publish(
        &self,
        context: &SessionExecutionContext,
        request: TaskPublishRequest,
    ) -> Result<serde_json::Value>;
}
