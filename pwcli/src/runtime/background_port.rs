use std::sync::Arc;

use async_trait::async_trait;

use crate::agent_core::contracts::ports::{
    BackgroundRunOutcome, BackgroundTaskPort, ToolExecutorPort, ToolInvocationContext,
};
use crate::runtime::background::BackgroundTaskManager;
use crate::runtime::tools::registry::ToolRegistry;

pub struct RuntimeBackgroundTaskPort {
    manager: Arc<BackgroundTaskManager>,
    tools: Arc<ToolRegistry>,
}

impl RuntimeBackgroundTaskPort {
    pub fn new(manager: Arc<BackgroundTaskManager>, tools: Arc<ToolRegistry>) -> Self {
        Self { manager, tools }
    }
}

#[async_trait]
impl BackgroundTaskPort for RuntimeBackgroundTaskPort {
    async fn spawn_tool(
        &self,
        session_id: String,
        tool_name: String,
        description: String,
        arguments: serde_json::Value,
    ) -> anyhow::Result<String> {
        self.manager
            .spawn_tool(
                session_id,
                tool_name,
                description,
                arguments,
                Arc::clone(&self.tools),
            )
            .await
    }

    async fn execute_with_promotion(
        &self,
        tool_name: String,
        description: String,
        arguments: serde_json::Value,
        context: ToolInvocationContext,
        timeout: std::time::Duration,
    ) -> anyhow::Result<BackgroundRunOutcome> {
        let session_id = context
            .session_id
            .clone()
            .unwrap_or_else(|| "unknown".into());
        let tools = Arc::clone(&self.tools);
        let execute_name = tool_name.clone();
        let (result_tx, mut result_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let result =
                ToolExecutorPort::execute(tools.as_ref(), &execute_name, &arguments, context)
                    .await
                    .map(|output| output.content);
            let _ = result_tx.send(result);
        });
        match tokio::time::timeout(timeout, &mut result_rx).await {
            Ok(Ok(result)) => result.map(BackgroundRunOutcome::Completed),
            Ok(Err(_)) => anyhow::bail!("tool task panicked"),
            Err(_) => {
                let task_id = self
                    .manager
                    .adopt(session_id, tool_name, description, handle, result_rx)
                    .await?;
                Ok(BackgroundRunOutcome::Promoted { task_id })
            }
        }
    }
}
