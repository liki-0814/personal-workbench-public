pub mod acp_routes;
pub mod memory_routes;
pub mod routes;
pub mod sse;
pub mod state;
pub mod static_assets;
mod tool_ports;
pub mod web;

use std::sync::Arc;

use crate::ai::llm::LlmClient;
use crate::app::config::RuntimeConfig;
use crate::runtime::backend::BackendClient;
use crate::runtime::permissions::{PermissionEngine, PermissionPolicy};
use crate::runtime::tools::registry::ToolRegistry;

use state::AppState;

pub async fn run_daemon_server<F>(
    config: RuntimeConfig,
    port: u16,
    runtime: crate::app::platform::daemon::DaemonRuntime,
    register_tools: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&mut ToolRegistry, Arc<BackendClient>),
{
    run_server_inner(config, port, runtime, register_tools).await
}

async fn run_server_inner<F>(
    config: RuntimeConfig,
    port: u16,
    daemon_runtime: crate::app::platform::daemon::DaemonRuntime,
    register_tools: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&mut ToolRegistry, Arc<BackendClient>),
{
    // Ensure skills cache + watcher is initialized (idempotent)
    crate::runtime::skills::init();

    let backend = Arc::new(BackendClient::new(&config.backend_url));
    let mut tool_registry = ToolRegistry::new();
    register_tools(&mut tool_registry, Arc::clone(&backend));

    let tool_registry = Arc::new(tool_registry);
    let auth_manager = Arc::new(crate::ai::provider::AuthManager::default());
    let llm_client = Arc::new(match LlmClient::from_provider_set(config.active_provider.as_deref(), config.providers.as_deref(), config.backend_url.clone()) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "daemon started without an AI provider; configure one in Web settings");
            LlmClient::with_provider(
                crate::ai::config::ProviderConfig {
                    name: "unconfigured".to_string(),
                    base_url: "http://127.0.0.1:9".to_string(),
                    api_key: String::new(),
                    protocol: "openai".to_string(),
                    model: "unconfigured".to_string(),
                    models: Vec::new(),
                    use_proxy: None,
                    compat_profile: None,
                },
                config.backend_url.clone(),
            )
        }
    }
    .with_auth_manager(Arc::clone(&auth_manager)));
    crate::runtime::tools::register::register_runtime_tools(
        Arc::clone(&tool_registry),
        Arc::clone(&llm_client),
    )
    .await;
    let permission_engine = Arc::new(PermissionEngine::new(PermissionPolicy::default()));
    let session_manager = crate::runtime::session::SessionManager::new();

    let data_dir = crate::app::config::local_config::data_dir();
    // Provision matplotlib without delaying daemon health/readiness. A plot
    // requested during setup awaits the same process-wide bootstrap result.
    crate::runtime::illustration::python_env::start_background_bootstrap();
    let permission_broker = Arc::new(crate::runtime::permissions::PermissionBroker::new(
        &data_dir,
    )?);
    let task_broker =
        crate::runtime::task::TaskBroker::new(&data_dir, format!("http://127.0.0.1:{port}"))?;
    let background_tasks = Arc::new(
        crate::runtime::background::BackgroundTaskManager::new(8)
            .with_task_broker(Arc::clone(&task_broker)),
    );
    crate::runtime::tools::dispatch_tasks::register(
        tool_registry.as_ref(),
        Arc::new(tool_ports::DaemonSessionContextPort::new(
            session_manager.clone(),
        )),
        Arc::new(tool_ports::DaemonTaskPublisherPort::new(Arc::clone(
            &task_broker,
        ))),
    );
    let background_sessions = session_manager.clone();
    // 启动时清理超 24h 的旧日志
    crate::runtime::background::BackgroundTaskManager::cleanup_old_logs();
    let web = Some(web::WebState::new(&data_dir)?);
    let state = AppState {
        session_manager,
        web_cache: Arc::new(crate::runtime::tools::web_cache::WebFetchCache::new()),
        tool_registry,
        llm_client,
        auth_manager,
        permission_engine,
        permission_broker,
        config: Arc::new(config),
        backend,
        background_tasks: Arc::clone(&background_tasks),
        task_broker,
        harnesses: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        runtime_input_actors: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        daemon_runtime: Some(daemon_runtime.clone()),
        web,
    };
    routes::task::spawn_outbox_dispatcher(state.clone());
    routes::work_items::spawn_capture_recovery(state.clone());
    let mut background_events = background_tasks.subscribe();
    tokio::spawn(async move {
        loop {
            match background_events.recv().await {
                Ok(crate::runtime::background::TaskEvent::Completed(result)) => {
                    if let Some(mut session) = background_sessions.get(&result.session_id) {
                        session.add_message(
                            crate::runtime::session::ConversationMessage::new_system(format!(
                                "[后台任务完成 id={} tool={} success={}]\n{}",
                                result.task_id, result.tool_name, result.success, result.summary
                            )),
                        );
                        background_sessions.update(session);
                    }
                }
                Ok(crate::runtime::background::TaskEvent::Started { .. })
                | Ok(crate::runtime::background::TaskEvent::Cancelled(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "background result projection lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    // The daemon owns both the same-origin Web UI and APIs. Loopback binding
    // keeps tools and provider credentials unavailable to other LAN devices.
    let agent_routes = routes::routes().merge(acp_routes::routes());
    let app = axum::Router::new()
        .nest("/api/agent", agent_routes.clone())
        .merge(agent_routes)
        .merge(web::routes())
        .merge(static_assets::routes())
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("pwcli daemon listening on http://{}", addr);
    println!("pwcli daemon listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let acp_shutdown_signal = daemon_runtime.shutdown.clone();
    let acp_shutdown = tokio::spawn(async move {
        acp_shutdown_signal.cancelled().await;
        crate::runtime::tools::code_agent::acp_runner::shutdown_all_sessions().await;
    });
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(daemon_runtime.shutdown.cancelled_owned())
        .await;
    let _ = acp_shutdown.await;
    serve_result?;

    Ok(())
}
