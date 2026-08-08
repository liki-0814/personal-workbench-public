use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::any::Any;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::agent_core::contracts::tool::{ToolExecutionMode, ToolImpact, ToolOutput};
use crate::agent_core::contracts::{SessionId, WorkItemId};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifact {
    pub id: String,
    pub sha256: String,
}

pub trait ArtifactStorePort: Send + Sync {
    fn store_tool_output(&self, content: &str) -> Result<StoredArtifact>;
}

pub type ProgressEmitter = Arc<dyn Fn(&str) + Send + Sync + 'static>;
pub type ImageEmitter = Arc<dyn Fn(&str, &str, Option<&Value>) + Send + Sync + 'static>;
pub type TextDeltaEmitter = Arc<dyn Fn(&str) + Send + Sync + 'static>;
pub type OpaqueToolContext = Arc<dyn Any + Send + Sync>;

#[derive(Clone)]
pub struct ToolInvocationContext {
    pub session_id: Option<String>,
    pub cancellation: CancellationToken,
    pub progress: Option<ProgressEmitter>,
    pub image: Option<ImageEmitter>,
    pub opaque: Option<OpaqueToolContext>,
}

#[async_trait]
pub trait ToolExecutorPort: Send + Sync {
    fn has_tool(&self, name: &str) -> bool;
    fn validate_arguments(&self, name: &str, args: &Value) -> Result<()>;
    fn execution_mode(&self, name: &str) -> Option<ToolExecutionMode>;
    fn impact_for_call(&self, name: &str, args: &Value) -> Option<ToolImpact>;
    async fn execute(
        &self,
        name: &str,
        args: &Value,
        context: ToolInvocationContext,
    ) -> Result<ToolOutput>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOutcome {
    Allow,
    Deny,
    Prompt,
}

pub trait PermissionPort: Send + Sync {
    fn check_call(&self, tool_name: &str, arguments: &str, impact: ToolImpact)
        -> PermissionOutcome;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundRunOutcome {
    Completed(String),
    Promoted { task_id: String },
}

#[async_trait]
pub trait BackgroundTaskPort: Send + Sync {
    async fn spawn_tool(
        &self,
        session_id: String,
        tool_name: String,
        description: String,
        arguments: Value,
    ) -> Result<String>;

    async fn execute_with_promotion(
        &self,
        tool_name: String,
        description: String,
        arguments: Value,
        context: ToolInvocationContext,
        timeout: std::time::Duration,
    ) -> Result<BackgroundRunOutcome>;
}
