use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::agent_core::contracts::ports::{
    SessionContextPort, SessionExecutionContext, TaskPublishRequest, TaskPublisherPort,
};
use crate::agent_core::contracts::SessionId;
use crate::runtime::session::manager::SessionManager;
use crate::runtime::task::{RuntimeTaskSpec, SubmitTaskBatch, TaskBroker};

pub struct DaemonSessionContextPort {
    sessions: SessionManager,
}

impl DaemonSessionContextPort {
    pub fn new(sessions: SessionManager) -> Self {
        Self { sessions }
    }
}

#[async_trait]
impl SessionContextPort for DaemonSessionContextPort {
    async fn resolve(&self, session_id: &SessionId) -> Result<SessionExecutionContext> {
        let session = self
            .sessions
            .get(session_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("active daemon session disappeared"))?;
        let workspace = session
            .workspace
            .as_ref()
            .map(|workspace| workspace.canonical_path.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow::anyhow!("session has no bound workspace"))?;
        Ok(SessionExecutionContext {
            session_id: session_id.clone(),
            workspace,
            generation: session.generation,
            work_item_id: None,
        })
    }
}

pub struct DaemonTaskPublisherPort {
    broker: Arc<TaskBroker>,
}

impl DaemonTaskPublisherPort {
    pub fn new(broker: Arc<TaskBroker>) -> Self {
        Self { broker }
    }
}

#[async_trait]
impl TaskPublisherPort for DaemonTaskPublisherPort {
    async fn publish(
        &self,
        context: &SessionExecutionContext,
        request: TaskPublishRequest,
    ) -> Result<serde_json::Value> {
        let tasks = serde_json::from_value::<Vec<RuntimeTaskSpec>>(request.tasks)?;
        let workspace = std::path::Path::new(&context.workspace);
        let publisher_model =
            request
                .publisher_model
                .map(|model| crate::runtime::task::DelegatedModelSnapshot {
                    provider_id: model.provider_id,
                    model: model.model,
                    effort: model.effort,
                    thinking: model.thinking,
                });
        let accepted = self.broker.submit_batch_in_workspace_with_model(
            SubmitTaskBatch {
                root_session_id: context.session_id.to_string(),
                parent_task_id: None,
                work_item_id: request.work_item_id.map(Into::into),
                session_generation: Some(context.generation),
                idempotency_key: request.idempotency_key,
                join: request.join,
                tasks,
            },
            workspace,
            publisher_model,
        )?;
        Ok(serde_json::to_value(accepted)?)
    }
}
