use crate::ai::llm::LlmClient;
use crate::app::config::RuntimeConfig;
use crate::runtime::backend::BackendClient;
use crate::runtime::permissions::{PermissionBroker, PermissionEngine};
use crate::runtime::tools::registry::ToolRegistry;
use crate::runtime::tools::web_cache::WebFetchCache;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::runtime::background::BackgroundTaskManager;
use crate::runtime::session::SessionManager;

#[derive(Clone)]
pub(crate) struct RuntimeInputActivation {
    pub queued: crate::runtime::session::QueuedInput,
    pub task_ids: Vec<String>,
    pub work_item_id: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub session_manager: SessionManager,
    pub web_cache: Arc<WebFetchCache>,
    pub tool_registry: Arc<ToolRegistry>,
    pub llm_client: Arc<LlmClient>,
    pub auth_manager: Arc<crate::ai::provider::AuthManager>,
    pub permission_engine: Arc<PermissionEngine>,
    pub permission_broker: Arc<PermissionBroker>,
    pub config: Arc<RuntimeConfig>,
    pub backend: Arc<BackendClient>,
    pub background_tasks: Arc<BackgroundTaskManager>,
    pub task_broker: Arc<crate::runtime::task::TaskBroker>,
    /// Session-scoped interactive harnesses shared by streaming chat and
    /// control endpoints (steer/follow-up/abort).
    pub harnesses: Arc<Mutex<HashMap<String, Arc<crate::agent_core::harness::HarnessControl>>>>,
    /// One daemon-owned input actor per session. Both queued user messages and
    /// RuntimeTask callbacks use it when an idle session must be reactivated.
    pub(crate) runtime_input_actors:
        Arc<Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<RuntimeInputActivation>>>>,
    /// Present only when the HTTP service is owned by the native daemon.
    pub daemon_runtime: Option<crate::app::platform::daemon::DaemonRuntime>,
    pub web: Option<Arc<super::web::WebState>>,
}
