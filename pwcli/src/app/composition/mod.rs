use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::agent_core::contracts::{SessionId, ThinkingLevel};
use crate::agent_core::harness::{HarnessControl, HarnessInputs, HarnessSpec};
use crate::agent_core::hooks::HookRunner;
use crate::agent_core::runner::{AgentRunner, HarnessRunOptions, ToolEventSink};
use crate::ai::config::ProviderConfig;
use crate::ai::llm::{ChatMessage, LlmClient, ToolSchema};
use crate::ai::usage::UsageTracker;
use crate::app::config::RuntimeConfig;
use crate::runtime::backend::BackendClient;
use crate::runtime::background::BackgroundTaskManager;
use crate::runtime::decision::moa::MoaRuntime;
use crate::runtime::permissions::{AgentPermissionMode, PermissionEngine};
use crate::runtime::session::Session;
use crate::runtime::task::{DelegatedModelSnapshot, WorkerDispatchContext, WorkerDispatchSignal};
use crate::runtime::tools::context::ToolExecutionContext;
use crate::runtime::tools::register::{register_all_tools, register_memory_tools};
use crate::runtime::tools::registry::ToolRegistry;
use crate::runtime::tools::web_cache::WebFetchCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfile {
    CliOneshot,
    InternalWorker,
    WebMain,
}

#[derive(Debug, Clone)]
pub struct ProviderSelection {
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub thinking: bool,
    pub resolved_provider: Option<ProviderConfig>,
}

impl ProviderSelection {
    pub fn resolved(provider: ProviderConfig) -> Self {
        Self {
            provider_id: Some(provider.name.clone()),
            model: Some(provider.model.clone()),
            effort: None,
            thinking: false,
            resolved_provider: Some(provider),
        }
    }
}

#[derive(Clone)]
pub struct RuntimeRequest {
    pub profile: RuntimeProfile,
    pub provider_override: Option<ProviderSelection>,
    pub workspace: PathBuf,
    pub permission_mode: AgentPermissionMode,
    pub thinking: bool,
    pub thinking_level: ThinkingLevel,
    pub session_id: Option<SessionId>,
    pub worker_dispatch: Option<(WorkerDispatchContext, WorkerDispatchSignal)>,
    pub system_prompt: String,
    pub harness: Option<Arc<HarnessControl>>,
    pub tool_context: ToolExecutionContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapabilitySnapshot {
    pub profile: RuntimeProfile,
    pub tools: Vec<String>,
    pub middleware: Vec<String>,
    pub harness_fingerprint: String,
    pub permission_mode: String,
    pub provider: String,
    pub model: String,
    pub protocol: String,
    pub auth_source: String,
    pub memory_mode: String,
    pub reviewer_enabled: bool,
    pub background_enabled: bool,
}

pub struct RuntimeFactory {
    config: Arc<RuntimeConfig>,
    backend: Arc<BackendClient>,
    tools: Option<Arc<ToolRegistry>>,
    permissions: Option<Arc<PermissionEngine>>,
    web_cache: Option<Arc<WebFetchCache>>,
    background_tasks: Option<Arc<BackgroundTaskManager>>,
    auth_manager: Arc<crate::ai::provider::AuthManager>,
}

impl RuntimeFactory {
    pub async fn load_local() -> Result<Self> {
        let mut config = RuntimeConfig::load();
        let backend = Arc::new(BackendClient::new(&config.backend_url));
        let _ = config.override_providers_from_backend(&backend).await;
        Ok(Self {
            config: Arc::new(config),
            backend,
            tools: None,
            permissions: None,
            web_cache: None,
            background_tasks: None,
            auth_manager: Arc::new(crate::ai::provider::AuthManager::default()),
        })
    }

    pub fn from_shared(
        config: Arc<RuntimeConfig>,
        backend: Arc<BackendClient>,
        tools: Arc<ToolRegistry>,
        permissions: Arc<PermissionEngine>,
        web_cache: Arc<WebFetchCache>,
        background_tasks: Arc<BackgroundTaskManager>,
        auth_manager: Arc<crate::ai::provider::AuthManager>,
    ) -> Self {
        Self {
            config,
            backend,
            tools: Some(tools),
            permissions: Some(permissions),
            web_cache: Some(web_cache),
            background_tasks: Some(background_tasks),
            auth_manager,
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        self.config.as_ref()
    }

    pub fn backend(&self) -> &Arc<BackendClient> {
        &self.backend
    }

    /// Build the same per-invocation model context used by the Agent runtime
    /// for direct product adapters such as `pwcli illustrate`.
    pub async fn illustration_context(&self, workspace: PathBuf) -> Result<ToolExecutionContext> {
        let config = self.config.as_ref();
        let selected_provider = config
            .active_provider()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No AI provider configured. Run: pwcli config"))?;
        let llm = Arc::new(
            LlmClient::from_provider_set(
                config.active_provider.as_deref(),
                config.providers.as_deref(),
                config.backend_url.clone(),
            )?
            .with_auth_manager(Arc::clone(&self.auth_manager)),
        );
        let active = resolve_active_model_context(config, &selected_provider, false);
        let vision_client = if active.supports_vision {
            None
        } else {
            configured_vision_provider(config).map(|provider| {
                Arc::new(
                    LlmClient::with_provider(provider, config.backend_url.clone())
                        .with_auth_manager(Arc::clone(&self.auth_manager)),
                )
            })
        };
        Ok(ToolExecutionContext {
            cwd: workspace,
            active_model: Some(active.clone()),
            illustration_model: Some(Arc::new(
                crate::runtime::illustration::LlmIllustrationModel::new(
                    llm,
                    active.supports_vision,
                    vision_client,
                ),
            )),
            ..ToolExecutionContext::default()
        })
    }

    pub async fn create(&self, request: RuntimeRequest) -> Result<AgentRuntime> {
        let mut config = self.config.as_ref().clone();
        apply_provider_selection(&mut config, request.provider_override.as_ref())?;
        let selected_provider = request
            .provider_override
            .as_ref()
            .and_then(|selection| selection.resolved_provider.clone())
            .or_else(|| config.active_provider().cloned())
            .ok_or_else(|| anyhow::anyhow!("active provider unavailable"))?;
        let mut llm = if request.provider_override.is_some() {
            LlmClient::with_provider(selected_provider.clone(), config.backend_url.clone())
        } else {
            LlmClient::from_provider_set(
                config.active_provider.as_deref(),
                config.providers.as_deref(),
                config.backend_url.clone(),
            )
            .map_err(|error| {
                anyhow::anyhow!("AI Provider 未配置（{}）。运行 `pwcli config`。", error)
            })?
        };
        if let Some(session_id) = request.session_id.as_ref() {
            llm = llm.with_session_id(session_id.to_string());
        }
        llm = llm.with_auth_manager(Arc::clone(&self.auth_manager));
        let llm = Arc::new(llm);

        let user_slug = config
            .user
            .as_ref()
            .and_then(|user| user.slug.clone())
            .unwrap_or_else(|| "local".to_string());
        let tools = if let Some(tools) = self.tools.as_ref() {
            Arc::clone(tools)
        } else {
            let mut registry = ToolRegistry::new();
            register_all_tools(&mut registry, Arc::clone(&self.backend));
            register_memory_tools(&mut registry, user_slug.clone());
            let tools = Arc::new(registry);
            crate::runtime::tools::register::register_runtime_tools(
                Arc::clone(&tools),
                Arc::clone(&llm),
            )
            .await;
            tools
        };
        if let Some((context, signal)) = request.worker_dispatch {
            crate::runtime::task::register_worker_dispatch_tool(&tools, context, signal);
        }

        let tool_schemas = tools.to_schemas();
        let permission_engine = self
            .permissions
            .as_ref()
            .cloned()
            .unwrap_or_else(|| Arc::new(PermissionEngine::default_engine()));
        let hook_runner = HookRunner::new();
        let harness = request
            .harness
            .unwrap_or_else(|| Arc::new(HarnessControl::default()));
        let reviewer = crate::runtime::decision::moa::active_runtime(
            &self.backend,
            config.backend_url.clone(),
        )
        .await?;
        let mut run_options = match request.profile {
            RuntimeProfile::WebMain => {
                HarnessRunOptions::main_with_thinking_level(request.thinking_level)
            }
            RuntimeProfile::CliOneshot | RuntimeProfile::InternalWorker => {
                HarnessRunOptions::oneshot_with_thinking_level(
                    request.permission_mode == AgentPermissionMode::Full,
                    request.thinking_level,
                )
            }
        };
        run_options.unified_recovery = crate::app::config::local_config::get()
            .features
            .unified_recovery;
        let auth_source = self
            .auth_manager
            .resolve(&selected_provider)
            .await?
            .source
            .to_string();
        let snapshot = capability_snapshot(
            request.profile,
            &config,
            &tool_schemas,
            &run_options,
            reviewer.is_some(),
            request.permission_mode,
            self.background_tasks.is_some(),
            auth_source,
        )?;

        let mut tool_context = request.tool_context;
        tool_context.session_id = request.session_id.clone();
        tool_context.cwd = request.workspace.clone();
        if tool_context.web_cache.is_none() {
            tool_context.web_cache = self.web_cache.as_ref().cloned();
        }
        let active_model_context = resolve_active_model_context(
            &config,
            &selected_provider,
            request.thinking_level.is_enabled(),
        );
        let vision_client = if active_model_context.supports_vision {
            None
        } else {
            configured_vision_provider(&config).map(|provider| {
                let mut client = LlmClient::with_provider(provider, config.backend_url.clone())
                    .with_auth_manager(Arc::clone(&self.auth_manager));
                if let Some(session_id) = request.session_id.as_ref() {
                    client = client.with_session_id(session_id.to_string());
                }
                Arc::new(client)
            })
        };
        tool_context.illustration_model = Some(Arc::new(
            crate::runtime::illustration::LlmIllustrationModel::new(
                Arc::clone(&llm),
                active_model_context.supports_vision,
                vision_client,
            ),
        ));
        tool_context.active_model = Some(active_model_context);
        let background_port = self.background_tasks.as_ref().map(|manager| {
            Arc::new(
                crate::runtime::background_port::RuntimeBackgroundTaskPort::new(
                    Arc::clone(manager),
                    Arc::clone(&tools),
                ),
            ) as Arc<dyn crate::agent_core::contracts::ports::BackgroundTaskPort>
        });

        Ok(AgentRuntime {
            llm,
            tools,
            tool_schemas,
            permission_engine,
            hook_runner,
            harness,
            reviewer,
            run_options,
            session_id: request.session_id,
            snapshot,
            system_prompt: request.system_prompt,
            tool_context,
            background_port,
            artifact_store: crate::runtime::tools::artifacts::RuntimeArtifactStore,
        })
    }
}

fn configured_vision_provider(config: &RuntimeConfig) -> Option<ProviderConfig> {
    config
        .providers
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find_map(|provider| {
            provider
                .models
                .iter()
                .find(|model| {
                    model.enabled != Some(false)
                        && model
                            .capabilities
                            .as_ref()
                            .and_then(|capabilities| capabilities.vision)
                            == Some(true)
                })
                .map(|model| {
                    let mut selected = provider.clone();
                    selected.model = model.id.clone();
                    selected
                })
        })
}

fn resolve_active_model_context(
    config: &RuntimeConfig,
    provider: &ProviderConfig,
    thinking: bool,
) -> crate::ai::llm::model_context::ActiveModelContext {
    let providers = config.providers.clone().unwrap_or_default();
    crate::ai::llm::model_context::ActiveModelContext {
        provider_id: Some(provider.name.clone()),
        model_id: provider.model.clone(),
        effort: crate::ai::llm::model_context::effort_for_provider(provider, thinking),
        thinking,
        supports_vision: crate::ai::llm::model_context::compute_vision_support_for(
            &providers,
            &provider.model,
        ),
    }
}

pub struct AgentRuntime {
    llm: Arc<LlmClient>,
    tools: Arc<ToolRegistry>,
    tool_schemas: Vec<ToolSchema>,
    permission_engine: Arc<PermissionEngine>,
    hook_runner: HookRunner,
    harness: Arc<HarnessControl>,
    reviewer: Option<MoaRuntime>,
    run_options: HarnessRunOptions,
    session_id: Option<SessionId>,
    snapshot: RuntimeCapabilitySnapshot,
    system_prompt: String,
    tool_context: ToolExecutionContext,
    background_port: Option<Arc<dyn crate::agent_core::contracts::ports::BackgroundTaskPort>>,
    artifact_store: crate::runtime::tools::artifacts::RuntimeArtifactStore,
}

impl AgentRuntime {
    pub fn capability_snapshot(&self) -> &RuntimeCapabilitySnapshot {
        &self.snapshot
    }

    pub(crate) fn harness(&self) -> &HarnessControl {
        self.harness.as_ref()
    }

    pub async fn run_turn(
        &self,
        messages: &mut Vec<ChatMessage>,
        session: &mut Session,
        usage: &mut UsageTracker,
        sink: &dyn ToolEventSink,
        audit_sink: Option<&dyn crate::agent_core::harness::HarnessAuditSink>,
    ) -> Result<Option<crate::agent_core::runner::TurnSummary>> {
        AgentRunner {
            llm: &self.llm,
            tool_registry: self.tools.as_ref(),
            permission_engine: self.permission_engine.as_ref(),
            hook_runner: &self.hook_runner,
            tool_schemas: &self.tool_schemas,
            system_prompt: &self.system_prompt,
            run_options: self.run_options.clone(),
            sink: Some(sink),
            cancel_token: None,
            background_tasks: self.background_port.as_deref(),
            session_id: self.session_id.as_ref().map(ToString::to_string),
            harness: Some(self.harness.as_ref()),
            decision_reviewer: self
                .reviewer
                .as_ref()
                .map(|reviewer| reviewer as &dyn crate::agent_core::decision::DecisionReviewer),
            audit_sink,
            tool_context: Some(Arc::new(self.tool_context.clone())),
            artifact_store: Some(&self.artifact_store),
        }
        .run_turn(messages, session, usage)
        .await
    }
}

fn capability_snapshot(
    profile: RuntimeProfile,
    config: &RuntimeConfig,
    tools: &[ToolSchema],
    options: &HarnessRunOptions,
    reviewer_enabled: bool,
    permission_mode: AgentPermissionMode,
    background_enabled: bool,
    auth_source: String,
) -> Result<RuntimeCapabilitySnapshot> {
    let provider = config
        .active_provider()
        .ok_or_else(|| anyhow::anyhow!("active provider unavailable"))?;
    let context_window = provider
        .current_model_context_window()
        .map(|window| window.min(u64::from(u32::MAX)) as u32)
        .unwrap_or_else(|| crate::ai::usage::context_window_for_model(&provider.model));
    let spec = HarnessSpec::resolve(HarnessInputs {
        profile: options.profile,
        max_rounds: options.max_rounds.unwrap_or(100),
        context_window,
        thinking_level: options.thinking_level,
        yolo_mode: options.yolo_mode,
        externalize_tool_outputs: options.externalize_tool_outputs,
        unified_recovery: options.unified_recovery,
        reviewer_enabled,
        background_enabled,
        steer_queue_mode: crate::agent_core::harness::spec::QueueModeSpec::OneAtATime,
        follow_up_queue_mode: crate::agent_core::harness::spec::QueueModeSpec::OneAtATime,
        tool_schemas: tools,
    })?;
    let mut tool_names = tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect::<Vec<_>>();
    tool_names.sort();
    Ok(RuntimeCapabilitySnapshot {
        profile,
        tools: tool_names,
        middleware: spec
            .middleware
            .iter()
            .map(|middleware| middleware.name().to_string())
            .collect(),
        harness_fingerprint: spec.fingerprint()?.to_string(),
        permission_mode: permission_mode.as_str().to_string(),
        provider: provider.name.clone(),
        model: provider.model.clone(),
        protocol: provider.protocol.clone(),
        auth_source,
        memory_mode: config.features.memory_injection.mode.clone(),
        reviewer_enabled,
        background_enabled,
    })
}

pub(crate) fn apply_provider_selection(
    config: &mut RuntimeConfig,
    selection: Option<&ProviderSelection>,
) -> Result<()> {
    let Some(selection) = selection else {
        return Ok(());
    };
    if let Some(provider) = selection.resolved_provider.as_ref() {
        config.active_provider = Some(provider.name.clone());
        let providers = config.providers.get_or_insert_with(Vec::new);
        if let Some(existing) = providers
            .iter_mut()
            .find(|existing| existing.name == provider.name)
        {
            *existing = provider.clone();
        } else {
            providers.push(provider.clone());
        }
        return Ok(());
    }
    let provider_id = selection
        .provider_id
        .as_deref()
        .or(config.active_provider.as_deref())
        .ok_or_else(|| anyhow::anyhow!("委派任务没有可恢复的 provider 快照"))?
        .to_string();
    config.active_provider = Some(provider_id.clone());
    let provider = config
        .providers
        .as_mut()
        .and_then(|providers| {
            providers.iter_mut().find(|provider| {
                provider.name == provider_id
                    || crate::ai::provider::provider_id(provider) == provider_id
            })
        })
        .ok_or_else(|| anyhow::anyhow!("委派任务的 provider `{provider_id}` 已不可用"))?;
    if let Some(model) = selection
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
    {
        provider.model = model.to_string();
    }
    if let Some(effort) = selection
        .effort
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let model_id = provider.model.clone();
        if !provider.models.iter().any(|model| model.id == model_id) {
            provider.models.push(crate::ai::config::ModelEntry {
                id: model_id.clone(),
                name: model_id.clone(),
                enabled: None,
                max_output: None,
                context_window: None,
                capabilities: None,
                request_params: None,
                thinking_params: None,
                deferred_tools_mode: None,
                ..Default::default()
            });
        }
        let model = provider
            .models
            .iter_mut()
            .find(|model| model.id == model_id)
            .unwrap();
        model
            .request_params
            .get_or_insert_with(serde_json::Map::new)
            .insert(
                "reasoning_effort".into(),
                serde_json::Value::String(effort.into()),
            );
        if selection.thinking {
            model
                .thinking_params
                .get_or_insert_with(serde_json::Map::new)
                .insert(
                    "reasoning_effort".into(),
                    serde_json::Value::String(effort.into()),
                );
        }
    }
    Ok(())
}

impl From<&DelegatedModelSnapshot> for ProviderSelection {
    fn from(snapshot: &DelegatedModelSnapshot) -> Self {
        Self {
            provider_id: snapshot.provider_id.clone(),
            model: (!snapshot.model.is_empty()).then(|| snapshot.model.clone()),
            effort: snapshot.effort.clone(),
            thinking: snapshot.thinking,
            resolved_provider: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RuntimeConfig {
        RuntimeConfig {
            providers: Some(vec![crate::ai::config::ProviderConfig {
                name: "provider".into(),
                base_url: "https://example.invalid/v1".into(),
                api_key: "secret-must-not-leak".into(),
                protocol: "openai_chat".into(),
                model: "model-a".into(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: None,
            }]),
            active_provider: Some("provider".into()),
            ..RuntimeConfig::default()
        }
    }

    #[test]
    fn capability_snapshot_is_sorted_stable_and_secret_free() {
        let tools = vec![
            ToolSchema {
                kind: "function".into(),
                function: crate::ai::llm::FunctionSchema {
                    name: "zeta".into(),
                    description: "z".into(),
                    parameters: serde_json::json!({"type":"object"}),
                },
            },
            ToolSchema {
                kind: "function".into(),
                function: crate::ai::llm::FunctionSchema {
                    name: "alpha".into(),
                    description: "a".into(),
                    parameters: serde_json::json!({"type":"object"}),
                },
            },
        ];
        let first = capability_snapshot(
            RuntimeProfile::CliOneshot,
            &config(),
            &tools,
            &HarnessRunOptions::oneshot(false),
            false,
            AgentPermissionMode::Risk,
            false,
            "legacy_config".into(),
        )
        .unwrap();
        let second = capability_snapshot(
            RuntimeProfile::CliOneshot,
            &config(),
            &tools,
            &HarnessRunOptions::oneshot(false),
            false,
            AgentPermissionMode::Risk,
            false,
            "legacy_config".into(),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.tools, ["alpha", "zeta"]);
        assert!(!first.middleware.is_empty());
        assert!(!first.harness_fingerprint.is_empty());
        assert_eq!(first.permission_mode, "risk");
        assert_eq!(first.memory_mode, config().features.memory_injection.mode);
        assert_eq!(first.auth_source, "legacy_config");
        assert!(!serde_json::to_string(&first)
            .unwrap()
            .contains("secret-must-not-leak"));
    }

    #[test]
    fn web_main_snapshot_uses_main_harness_and_background_capabilities() {
        let snapshot = capability_snapshot(
            RuntimeProfile::WebMain,
            &config(),
            &[],
            &HarnessRunOptions::main(true),
            true,
            AgentPermissionMode::Risk,
            true,
            "legacy_config".into(),
        )
        .unwrap();

        assert_eq!(snapshot.profile, RuntimeProfile::WebMain);
        assert!(snapshot.reviewer_enabled);
        assert!(snapshot.background_enabled);
        assert_eq!(snapshot.provider, "provider");
        assert_eq!(snapshot.model, "model-a");
        assert_eq!(snapshot.protocol, "openai_chat");
        assert!(!snapshot.middleware.is_empty());
        assert!(!snapshot.harness_fingerprint.is_empty());
    }

    #[test]
    fn internal_worker_snapshot_keeps_oneshot_policy_and_profile_identity() {
        let snapshot = capability_snapshot(
            RuntimeProfile::InternalWorker,
            &config(),
            &[],
            &HarnessRunOptions::oneshot_with_thinking(false, true),
            false,
            AgentPermissionMode::Risk,
            false,
            "legacy_config".into(),
        )
        .unwrap();

        assert_eq!(snapshot.profile, RuntimeProfile::InternalWorker);
        assert_eq!(snapshot.permission_mode, "risk");
        assert!(!snapshot.reviewer_enabled);
        assert!(!snapshot.background_enabled);
        assert_eq!(snapshot.provider, "provider");
        assert_eq!(snapshot.model, "model-a");
        assert_eq!(
            snapshot.memory_mode,
            config().features.memory_injection.mode
        );
        assert!(!snapshot.middleware.is_empty());
        assert!(!snapshot.harness_fingerprint.is_empty());
    }

    #[test]
    fn provider_selection_preserves_wire_effort_and_thinking() {
        let mut config = config();
        apply_provider_selection(
            &mut config,
            Some(&ProviderSelection {
                provider_id: Some("provider".into()),
                model: Some("model-b".into()),
                effort: Some("high".into()),
                thinking: true,
                resolved_provider: None,
            }),
        )
        .unwrap();
        let provider = config.active_provider().unwrap();
        assert_eq!(provider.model, "model-b");
        let model = provider.current_model_entry().unwrap();
        assert_eq!(
            model.request_params.as_ref().unwrap()["reasoning_effort"],
            "high"
        );
        assert_eq!(
            model.thinking_params.as_ref().unwrap()["reasoning_effort"],
            "high"
        );
    }

    #[test]
    fn factory_model_context_uses_effective_provider_and_ignores_boundary_defaults() {
        let mut config = config();
        let provider = config.providers.as_mut().unwrap().first_mut().unwrap();
        provider.models.push(crate::ai::config::ModelEntry {
            id: "model-a".into(),
            name: "Model A".into(),
            enabled: None,
            max_output: None,
            context_window: None,
            capabilities: Some(crate::ai::config::ModelCapabilities {
                vision: Some(true),
                ..Default::default()
            }),
            request_params: Some(
                serde_json::json!({"reasoning_effort": "medium"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            thinking_params: Some(
                serde_json::json!({"reasoning_effort": "high"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            deferred_tools_mode: None,
            ..Default::default()
        });
        let provider = config.active_provider().unwrap();
        let context = resolve_active_model_context(&config, provider, true);
        assert_eq!(context.provider_id.as_deref(), Some("provider"));
        assert_eq!(context.model_id, "model-a");
        assert_eq!(context.effort.as_deref(), Some("high"));
        assert!(context.thinking);
        assert!(context.supports_vision);

        let selection = ProviderSelection::resolved(provider.clone());
        assert_eq!(selection.effort, None);
        assert!(!selection.thinking);
    }
}
