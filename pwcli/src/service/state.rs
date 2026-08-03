use crate::backend::BackendClient;
use crate::config::RuntimeConfig;
use crate::llm::LlmClient;
use crate::permissions::{PermissionBroker, PermissionEngine};
use crate::tools::registry::ToolRegistry;
use crate::tools::web_cache::WebFetchCache;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::session_manager::SessionManager;
use crate::background::BackgroundTaskManager;

#[derive(Clone)]
pub(crate) struct RuntimeInputActivation {
    pub queued: crate::session::QueuedInput,
    pub task_ids: Vec<String>,
    pub work_item_id: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub session_manager: SessionManager,
    pub web_cache: Arc<WebFetchCache>,
    pub tool_registry: Arc<ToolRegistry>,
    pub llm_client: Arc<LlmClient>,
    pub auth_manager: Arc<crate::provider_ai::AuthManager>,
    pub permission_engine: Arc<PermissionEngine>,
    pub permission_broker: Arc<PermissionBroker>,
    pub config: Arc<RuntimeConfig>,
    pub backend: Arc<BackendClient>,
    pub background_tasks: Arc<BackgroundTaskManager>,
    pub task_broker: Arc<crate::task::TaskBroker>,
    /// Session-scoped interactive harnesses shared by streaming chat and
    /// control endpoints (steer/follow-up/abort).
    pub harnesses: Arc<Mutex<HashMap<String, Arc<crate::harness::HarnessControl>>>>,
    /// One daemon-owned input actor per session. Both queued user messages and
    /// RuntimeTask callbacks use it when an idle session must be reactivated.
    pub(crate) runtime_input_actors:
        Arc<Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<RuntimeInputActivation>>>>,
    /// Present only when the HTTP service is owned by the native daemon.
    pub daemon_runtime: Option<crate::daemon::DaemonRuntime>,
    pub web: Option<Arc<super::web::WebState>>,
}
