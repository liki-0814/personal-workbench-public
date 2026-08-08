use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent_core::contracts::ports::{SessionContextPort, TaskPublisherPort};
use crate::agent_core::contracts::{SessionId, WorkItemId};
use crate::ai::llm::model_context::ActiveModelContext;
use crate::runtime::tools::progress::{ImageEmitter, ProgressEmitter, TextDeltaEmitter};
use crate::runtime::tools::web_cache::WebFetchCache;
use crate::runtime::visual_generation::ImageReferenceRegistry;

#[derive(Clone)]
pub struct ToolExecutionContext {
    pub session_id: Option<SessionId>,
    pub chat_session_id: Option<SessionId>,
    pub work_item_id: Option<WorkItemId>,
    pub cwd: PathBuf,
    pub cancellation: CancellationToken,
    pub active_model: Option<ActiveModelContext>,
    /// Per-turn model access for deterministic illustration workflows. This is
    /// deliberately invocation-scoped: a shared daemon ToolRegistry must not
    /// capture the provider that happened to be active at daemon startup.
    pub illustration_model: Option<Arc<dyn crate::runtime::illustration::IllustrationModelPort>>,
    pub image_references: Option<Arc<ImageReferenceRegistry>>,
    pub web_cache: Option<Arc<WebFetchCache>>,
    pub progress: Option<ProgressEmitter>,
    pub image: Option<ImageEmitter>,
    pub text_delta: Option<TextDeltaEmitter>,
    pub session_context: Option<Arc<dyn SessionContextPort>>,
    pub task_publisher: Option<Arc<dyn TaskPublisherPort>>,
}

impl Default for ToolExecutionContext {
    fn default() -> Self {
        Self {
            session_id: None,
            chat_session_id: None,
            work_item_id: None,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            cancellation: CancellationToken::new(),
            active_model: None,
            illustration_model: None,
            image_references: None,
            web_cache: None,
            progress: None,
            image: None,
            text_delta: None,
            session_context: None,
            task_publisher: None,
        }
    }
}
