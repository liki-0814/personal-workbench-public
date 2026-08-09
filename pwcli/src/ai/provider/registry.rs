use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use anyhow::Context;
use async_trait::async_trait;
use serde_json::Value;

use crate::ai::config::{ModelCapabilities, ModelEntry, ProviderConfig};

use super::{provider_kind, ProviderKind, ResolvedAuth};

// This is the Codex model-catalog schema level PWCLI has validated, not the
// PWCLI package version. The server filters out models whose
// `minimal_client_version` exceeds this value.
const OPENAI_CODEX_MODELS_CLIENT_VERSION: &str = "0.147.0";

#[derive(Clone, Copy)]
struct ProviderDefinition {
    kind: ProviderKind,
    name: &'static str,
    base_url: &'static str,
    api: &'static str,
    models_dev_key: Option<&'static str>,
    env_key: Option<&'static str>,
    default_model: &'static str,
}

/// Runtime provider contract modeled after pi's Provider interface.
#[async_trait]
pub trait ProviderService: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn base_url(&self) -> &'static str;
    fn default_protocol(&self) -> &'static str;
    fn env_key(&self) -> Option<&'static str>;
    fn default_model(&self) -> &'static str;
    fn get_models(&self) -> Vec<ModelEntry>;
    fn seed_models(&self, models: &[ModelEntry]);
    async fn refresh_models(&self, auth: Option<&ResolvedAuth>) -> anyhow::Result<Vec<ModelEntry>>;
    /// Where the current `get_models()` snapshot came from: `"native"`
    /// (the provider's own /models endpoint), `"models-dev"` (third-party
    /// catalog), or `"dedicated"` (provider-specific discovery API).
    /// Callers use this to decide whether a missing model really means the
    /// provider retired it, or merely that a degraded fallback ran.
    fn discovery_source(&self) -> Option<&'static str> {
        None
    }

    fn materialize(&self, configured: &ProviderConfig) -> ProviderConfig {
        let mut resolved = configured.clone();
        resolved.name = self.name().to_string();
        resolved.base_url = self.base_url().to_string();
        resolved.protocol = self.default_protocol().to_string();
        if resolved.model.trim().is_empty() {
            resolved.model = self.default_model().to_string();
        }
        if resolved.models.is_empty() {
            resolved.models = self.get_models();
        } else {
            // Discovery metadata evolves (vendors correct context limits
            // upstream), so configured entries borrow the freshest known
            // enrichment from the current discovery snapshot instead of
            // freezing whatever values were captured at adoption time.
            let discovered = self.get_models();
            for model in &mut resolved.models {
                let Some(fresh) = discovered.iter().find(|entry| entry.id == model.id) else {
                    continue;
                };
                if fresh.context_window.is_some() {
                    model.context_window = fresh.context_window;
                }
                if fresh.max_output.is_some() {
                    model.max_output = fresh.max_output;
                }
                if model.name == model.id {
                    model.name = fresh.name.clone();
                }
                if let Some(fresh_caps) = &fresh.capabilities {
                    let caps = model.capabilities.get_or_insert_with(Default::default);
                    if fresh_caps.vision.is_some() {
                        caps.vision = fresh_caps.vision;
                    }
                    if fresh_caps.thinking.is_some() {
                        caps.thinking = fresh_caps.thinking;
                    }
                    if fresh_caps.image.is_some() {
                        caps.image = fresh_caps.image;
                    }
                }
            }
        }
        let transport = resolved
            .current_model_entry()
            .map(|model| (model.api.clone(), model.base_url.clone()));
        if let Some((api, base_url)) = transport {
            if let Some(api) = api.as_deref() {
                resolved.protocol = canonical_protocol(api).to_string();
            }
            if let Some(base_url) = base_url.as_deref() {
                resolved.base_url = base_url.to_string();
            }
        }
        resolved
    }
}

struct DynamicProvider {
    definition: ProviderDefinition,
    models: RwLock<Vec<ModelEntry>>,
    http: reqwest::Client,
    refresh_lock: tokio::sync::Mutex<()>,
    refresh_after: RwLock<Option<std::time::Instant>>,
    discovery_source: RwLock<Option<&'static str>>,
}

impl DynamicProvider {
    fn new(definition: ProviderDefinition) -> Self {
        let models = load_cached_models(definition.kind).unwrap_or_default();
        Self {
            definition,
            models: RwLock::new(models),
            http: crate::ai::http::default_client(crate::ai::http::ClientProfile::Llm),
            refresh_lock: tokio::sync::Mutex::new(()),
            refresh_after: RwLock::new(None),
            // A cache-restored list has no live source; discovery_source is
            // only trustworthy once this process performs a real refresh.
            discovery_source: RwLock::new(None),
        }
    }

    /// Best-effort metadata lookup from models.dev.  Discovery no longer
    /// depends on this catalog: it only enriches natively discovered models
    /// with context limits, capabilities, and costs.
    async fn models_dev_metadata(&self) -> anyhow::Result<BTreeMap<String, Value>> {
        let Some(source) = self.definition.models_dev_key else {
            return Ok(BTreeMap::new());
        };
        let body = self
            .http
            .get("https://models.dev/api.json")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let raw = body
            .get(source)
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("models.dev has no `{source}` provider"))?;
        Ok(raw
            .iter()
            .map(|(id, value)| (normalize_model_id(self.kind(), id, value), value.clone()))
            .collect())
    }

    /// Existence discovery against the provider's own OpenAI-compatible
    /// `GET /models` endpoint.  This is the primary source of truth, so a
    /// provider can add or retire models without any pwcli update.
    async fn refresh_native_openai(
        &self,
        auth: Option<&ResolvedAuth>,
    ) -> anyhow::Result<Vec<ModelEntry>> {
        let auth = auth.ok_or_else(|| {
            anyhow::anyhow!("{} native discovery requires credentials", self.id())
        })?;
        let body = self
            .http
            .get(format!("{}/models", self.base_url().trim_end_matches('/')))
            .bearer_auth(&auth.api_key)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let raw = body
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("{} /models has no data array", self.id()))?;
        let metadata = self.models_dev_metadata().await.unwrap_or_default();
        let models = raw
            .iter()
            .filter_map(|value| value.get("id").and_then(Value::as_str))
            .map(|id| self.model_entry(id, metadata.get(id)))
            .collect::<Vec<_>>();
        if models.is_empty() {
            anyhow::bail!("{} /models returned no models", self.id());
        }
        Ok(models)
    }

    async fn refresh_models_dev(&self) -> anyhow::Result<Vec<ModelEntry>> {
        let metadata = self.models_dev_metadata().await?;
        let models = metadata
            .iter()
            .filter_map(|(id, value)| self.model_from_models_dev(id, value))
            .map(|model| (model.id.clone(), model))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect::<Vec<_>>();
        if models.is_empty() {
            anyhow::bail!(
                "models.dev returned no tool-capable models for `{}`",
                self.id()
            );
        }
        Ok(models)
    }

    fn model_from_models_dev(&self, id: &str, value: &Value) -> Option<ModelEntry> {
        if value.get("tool_call").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        // `models_dev_metadata` already normalizes map keys; pass the value
        // through as metadata enrichment.
        Some(self.model_entry(id, Some(value)))
    }

    /// Builds a model entry for `id`, enriching it with models.dev metadata
    /// (context limits, capabilities, costs) when available.  A model that
    /// the provider serves but models.dev does not know about is still fully
    /// usable; only the enrichment is missing.
    fn model_entry(&self, id: &str, metadata: Option<&Value>) -> ModelEntry {
        let (headers, compat) = provider_model_compat(self.kind(), id);
        let mut entry = ModelEntry {
            id: id.to_string(),
            name: id.to_string(),
            api: Some(self.model_api(id).to_string()),
            provider: Some(self.id().to_string()),
            base_url: Some(self.base_url().to_string()),
            enabled: Some(true),
            headers,
            compat,
            ..Default::default()
        };
        if let Some(value) = metadata {
            let input = value
                .pointer("/modalities/input")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec!["text".into()]);
            let reasoning = value
                .get("reasoning")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            // A model whose output modalities include images is an image
            // generator even when its id carries no hint (e.g. gemini
            // multimodal models that emit images).
            let outputs_images = value
                .pointer("/modalities/output")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|modality| modality == "image")
                })
                .unwrap_or(false);
            entry.name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string();
            entry.max_output = value
                .pointer("/limit/output")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok());
            entry.context_window = value.pointer("/limit/context").and_then(Value::as_u64);
            if outputs_images {
                entry
                    .capabilities
                    .get_or_insert_with(Default::default)
                    .image = Some(true);
            }
            entry
                .capabilities
                .get_or_insert_with(Default::default)
                .vision = Some(input.iter().any(|modality| modality == "image"));
            entry
                .capabilities
                .get_or_insert_with(Default::default)
                .thinking = Some(reasoning);
            entry.reasoning = Some(reasoning);
            entry.input = input;
            entry.cost = value.get("cost").cloned();
            entry.thinking_level_map = reasoning_level_map(value);
        }
        // Image-generation classification must work on every discovery path,
        // including native /models where no metadata exists: fall back to the
        // id heuristic.
        if entry
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.image)
            != Some(true)
            && crate::ai::config::is_image_generation_model_id(id)
        {
            entry
                .capabilities
                .get_or_insert_with(Default::default)
                .image = Some(true);
        }
        // Bundled fallback metadata: the provider's /models endpoint carries
        // no limits and models.dev may be unreachable or unaware of regional
        // token-plan catalogs, so consult the shipped spec table.
        apply_bundled_fallbacks(self.kind(), id, &mut entry);
        entry
    }

    async fn refresh_openai_codex(&self, auth: &ResolvedAuth) -> anyhow::Result<Vec<ModelEntry>> {
        let client_version = std::env::var("PWCLI_CODEX_MODELS_CLIENT_VERSION")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| OPENAI_CODEX_MODELS_CLIENT_VERSION.to_string());
        let url = format!(
            "{}/codex/models?client_version={}",
            self.base_url().trim_end_matches('/'),
            client_version
        );
        let mut request = self
            .http
            .get(url)
            .timeout(std::time::Duration::from_secs(5))
            .bearer_auth(&auth.api_key)
            .header("originator", "pwcli")
            .header("User-Agent", concat!("pwcli/", env!("CARGO_PKG_VERSION")));
        if let Some(account_id) = auth.account_id.as_deref() {
            request = request.header("chatgpt-account-id", account_id);
        }
        let body = request
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let raw = body
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("OpenAI Codex response has no models array"))?;
        let models = raw
            .iter()
            .filter(|value| {
                value.get("supported_in_api").and_then(Value::as_bool) != Some(false)
                    && value.get("visibility").and_then(Value::as_str) != Some("hide")
            })
            .filter_map(|value| {
                let id = value.get("slug").and_then(Value::as_str)?;
                let efforts = value
                    .get("supported_reasoning_levels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|level| level.get("effort").and_then(Value::as_str))
                    .map(|effort| (effort.to_string(), Value::String(effort.to_string())))
                    .collect::<serde_json::Map<_, _>>();
                let input = value
                    .get("input_modalities")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .filter(|items| !items.is_empty())
                    .unwrap_or_else(|| vec!["text".into()]);
                Some(ModelEntry {
                    id: id.into(),
                    name: value
                        .get("display_name")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .into(),
                    api: Some("openai-codex-responses".into()),
                    provider: Some(self.id().into()),
                    base_url: Some(self.base_url().into()),
                    enabled: Some(true),
                    max_output: value
                        .get("max_output_tokens")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok()),
                    context_window: value.get("context_window").and_then(Value::as_u64),
                    capabilities: Some(ModelCapabilities {
                        vision: Some(input.iter().any(|modality| modality == "image")),
                        thinking: Some(!efforts.is_empty()),
                        image: None,
                    }),
                    reasoning: Some(!efforts.is_empty()),
                    input,
                    thinking_level_map: (!efforts.is_empty()).then_some(efforts),
                    ..Default::default()
                })
            })
            .collect::<Vec<_>>();
        if models.is_empty() {
            anyhow::bail!("OpenAI Codex returned no visible API models");
        }
        Ok(models)
    }

    fn model_api(&self, id: &str) -> &'static str {
        match self.kind() {
            ProviderKind::KimiCoding => "anthropic-messages",
            ProviderKind::Xai if id == "grok-4.5" => "openai-responses",
            ProviderKind::Xai => "openai-completions",
            ProviderKind::OpenAiCodex => "openai-codex-responses",
            ProviderKind::QwenTokenPlanCn => "openai-completions",
            ProviderKind::GoogleAntigravity => "google-antigravity",
            ProviderKind::Custom => self.definition.api,
        }
    }

    async fn refresh_antigravity(&self, auth: &ResolvedAuth) -> anyhow::Result<Vec<ModelEntry>> {
        let project = auth
            .project_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Antigravity authentication has no project id"))?;
        let body = self
            .http
            .post(format!(
                "{}/v1internal:fetchAvailableModels",
                self.base_url().trim_end_matches('/')
            ))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                "antigravity/cli/1.0.13 (aidev_client; os_type=darwin; arch=arm64)",
            )
            .timeout(std::time::Duration::from_secs(10))
            .bearer_auth(&auth.api_key)
            .json(&serde_json::json!({ "project": project }))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let models = body
            .get("models")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("Antigravity response has no models object"))?;
        let mut collapsed = BTreeMap::<String, ModelEntry>::new();
        for (wire_id, value) in models {
            let id = collapse_antigravity_model(wire_id);
            let context_window = value.get("maxTokens").and_then(Value::as_u64);
            let entry = collapsed
                .entry(id.to_string())
                .or_insert_with(|| ModelEntry {
                    id: id.to_string(),
                    name: value
                        .get("displayName")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    api: Some("google-antigravity".into()),
                    provider: Some(self.id().into()),
                    base_url: Some(self.base_url().into()),
                    enabled: Some(true),
                    capabilities: Some(ModelCapabilities {
                        vision: Some(true),
                        thinking: Some(antigravity_efforts(id).is_some()),
                        image: None,
                    }),
                    reasoning: Some(antigravity_efforts(id).is_some()),
                    input: vec!["text".into(), "image".into()],
                    ..Default::default()
                });
            if context_window > entry.context_window {
                entry.context_window = context_window;
            }
            if let Some(efforts) = antigravity_efforts(id) {
                entry.thinking_level_map = Some(
                    efforts
                        .iter()
                        .map(|effort| ((*effort).to_string(), Value::String((*effort).to_string())))
                        .collect(),
                );
            }
        }
        let models = collapsed.into_values().collect::<Vec<_>>();
        if models.is_empty() {
            anyhow::bail!("Antigravity returned no available models");
        }
        Ok(models)
    }
}

#[async_trait]
impl ProviderService for DynamicProvider {
    fn kind(&self) -> ProviderKind {
        self.definition.kind
    }
    fn id(&self) -> &'static str {
        self.definition.kind.as_str()
    }
    fn name(&self) -> &'static str {
        self.definition.name
    }
    fn base_url(&self) -> &'static str {
        self.definition.base_url
    }
    fn default_protocol(&self) -> &'static str {
        canonical_protocol(self.definition.api)
    }
    fn env_key(&self) -> Option<&'static str> {
        self.definition.env_key
    }
    fn default_model(&self) -> &'static str {
        self.definition.default_model
    }
    fn get_models(&self) -> Vec<ModelEntry> {
        self.models
            .read()
            .expect("provider models poisoned")
            .clone()
    }
    fn seed_models(&self, models: &[ModelEntry]) {
        if models.is_empty() {
            return;
        }
        let mut current = self.models.write().expect("provider models poisoned");
        if current.is_empty() {
            *current = models.to_vec();
        }
    }
    async fn refresh_models(&self, auth: Option<&ResolvedAuth>) -> anyhow::Result<Vec<ModelEntry>> {
        let Ok(_guard) = self.refresh_lock.try_lock() else {
            return Ok(self.get_models());
        };
        let now = std::time::Instant::now();
        if self
            .refresh_after
            .read()
            .expect("provider refresh deadline poisoned")
            .is_some_and(|deadline| deadline > now)
        {
            return Ok(self.get_models());
        }
        // Failed discovery receives a short cooldown; successful discovery
        // extends it below. This keeps catalog reads non-blocking without
        // hammering an unavailable upstream.
        *self
            .refresh_after
            .write()
            .expect("provider refresh deadline poisoned") =
            Some(now + std::time::Duration::from_secs(60));
        let (models, source) = match self.kind() {
            ProviderKind::GoogleAntigravity => (
                self.refresh_antigravity(auth.ok_or_else(|| {
                    anyhow::anyhow!("Antigravity refresh requires authentication")
                })?)
                .await?,
                "dedicated",
            ),
            ProviderKind::OpenAiCodex => (
                self.refresh_openai_codex(auth.ok_or_else(|| {
                    anyhow::anyhow!("OpenAI Codex refresh requires authentication")
                })?)
                .await?,
                "dedicated",
            ),
            // OpenAI-compatible providers are discovered from their own
            // /models endpoint first; models.dev only enriches metadata and
            // serves as the fallback when native discovery is unavailable.
            ProviderKind::Xai | ProviderKind::QwenTokenPlanCn => {
                match self.refresh_native_openai(auth).await {
                    Ok(models) => (models, "native"),
                    Err(error) => {
                        tracing::warn!(
                            provider = self.id(),
                            %error,
                            "native model discovery failed; falling back to models.dev"
                        );
                        (self.refresh_models_dev().await?, "models-dev")
                    }
                }
            }
            _ => (self.refresh_models_dev().await?, "models-dev"),
        };
        *self
            .discovery_source
            .write()
            .expect("provider discovery source poisoned") = Some(source);
        *self.models.write().expect("provider models poisoned") = models.clone();
        *self
            .refresh_after
            .write()
            .expect("provider refresh deadline poisoned") =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(5 * 60));
        if let Err(error) = persist_cached_models(self.kind(), &models) {
            tracing::warn!(provider = self.id(), %error, "failed to persist provider model cache");
        }
        Ok(models)
    }
    fn discovery_source(&self) -> Option<&'static str> {
        *self
            .discovery_source
            .read()
            .expect("provider discovery source poisoned")
    }
}

pub struct ProviderRegistry {
    providers: BTreeMap<ProviderKind, Arc<dyn ProviderService>>,
}

impl ProviderRegistry {
    fn builtins() -> Self {
        let definitions = [
            ProviderDefinition {
                kind: ProviderKind::KimiCoding,
                name: "Kimi Coding",
                base_url: "https://api.kimi.com/coding",
                api: "anthropic-messages",
                models_dev_key: Some("kimi-for-coding"),
                env_key: Some("KIMI_API_KEY"),
                default_model: "kimi-for-coding",
            },
            ProviderDefinition {
                kind: ProviderKind::Xai,
                name: "Grok / xAI",
                base_url: "https://api.x.ai/v1",
                api: "openai-responses",
                models_dev_key: Some("xai"),
                env_key: Some("XAI_API_KEY"),
                default_model: "grok-4.5",
            },
            ProviderDefinition {
                kind: ProviderKind::OpenAiCodex,
                name: "OpenAI Codex",
                base_url: "https://chatgpt.com/backend-api",
                api: "openai-codex-responses",
                models_dev_key: None,
                env_key: None,
                default_model: "gpt-5.5",
            },
            ProviderDefinition {
                kind: ProviderKind::GoogleAntigravity,
                name: "Google Antigravity",
                base_url: "https://daily-cloudcode-pa.googleapis.com",
                api: "google-antigravity",
                models_dev_key: None,
                env_key: None,
                default_model: "gemini-3.6-flash",
            },
            ProviderDefinition {
                kind: ProviderKind::QwenTokenPlanCn,
                name: "Qwen Token Plan CN",
                base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
                api: "openai-completions",
                models_dev_key: Some("alibaba-token-plan-cn"),
                env_key: Some("QWEN_TOKEN_PLAN_CN_API_KEY"),
                default_model: "qwen3.7-max",
            },
        ];
        Self {
            providers: definitions
                .into_iter()
                .map(|definition| {
                    (
                        definition.kind,
                        Arc::new(DynamicProvider::new(definition)) as Arc<dyn ProviderService>,
                    )
                })
                .collect(),
        }
    }

    pub fn global() -> &'static Self {
        static REGISTRY: OnceLock<ProviderRegistry> = OnceLock::new();
        REGISTRY.get_or_init(Self::builtins)
    }
    pub fn all(&self) -> Vec<Arc<dyn ProviderService>> {
        self.providers.values().cloned().collect()
    }
    pub fn get(&self, kind: ProviderKind) -> Option<Arc<dyn ProviderService>> {
        self.providers.get(&kind).cloned()
    }
    pub fn materialize(&self, configured: &ProviderConfig) -> ProviderConfig {
        self.get(provider_kind(configured))
            .map(|provider| {
                provider.seed_models(&configured.models);
                provider.materialize(configured)
            })
            .unwrap_or_else(|| configured.clone())
    }
}

fn normalize_model_id(kind: ProviderKind, id: &str, _value: &Value) -> String {
    if kind == ProviderKind::KimiCoding && matches!(id, "k2p5" | "k2p6" | "k2p7") {
        "kimi-for-coding".into()
    } else {
        id.into()
    }
}

/// Bundled fallback spec table (`resources/model_metadata.csv`): context and
/// output limits for models that neither the provider's `/models` endpoint
/// nor models.dev describe.  Keys are normalized (lowercased, vendor suffixes
/// such as `-dogfooding`/`-preview` and `bailian/` prefixes stripped) so
/// `Qwen3.7-Max-DogFooding` matches the served id `qwen3.7-max`.
fn bundled_model_metadata(id: &str) -> Option<(u64, u32)> {
    static TABLE: OnceLock<BTreeMap<String, (u64, u32)>> = OnceLock::new();
    fn normalize(id: &str) -> String {
        let mut normalized = id.to_ascii_lowercase();
        for suffix in ["-dogfooding", "-dogfood", "-preview"] {
            if let Some(stripped) = normalized.strip_suffix(suffix) {
                normalized = stripped.to_string();
            }
        }
        normalized.trim_start_matches("bailian/").to_string()
    }
    let table = TABLE.get_or_init(|| {
        let mut map = BTreeMap::new();
        for line in include_str!("../../../resources/model_metadata.csv")
            .lines()
            .skip(1)
        {
            let mut cols = line.split(',');
            let (Some(model), Some(context), Some(_input), Some(output)) =
                (cols.next(), cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let (Ok(context), Ok(output)) =
                (context.trim().parse::<u64>(), output.trim().parse::<u32>())
            else {
                continue;
            };
            map.insert(normalize(model.trim()), (context, output));
        }
        map
    });
    table.get(&normalize(id)).copied()
}

/// Post-discovery enrichment shared by every path (native `/models`,
/// models.dev, cached snapshots): fills limits from the bundled spec table
/// when the discovery source carries none, and defaults modality marks for
/// qwen token-plan, whose mainline models are multimodal but whose `/models`
/// endpoint reports no modality metadata.
fn apply_bundled_fallbacks(kind: ProviderKind, id: &str, entry: &mut ModelEntry) {
    if entry.context_window.is_none() {
        if let Some((context, output)) = bundled_model_metadata(id) {
            entry.context_window = Some(context);
            if entry.max_output.is_none() {
                entry.max_output = Some(output);
            }
        }
    }
    if kind == ProviderKind::QwenTokenPlanCn
        && id.to_ascii_lowercase().starts_with("qwen")
        && !crate::ai::config::is_image_generation_model_id(id)
    {
        // Specialist single-purpose models (speech, translation, code, ...)
        // are not general multimodal chat models.
        let specialist = [
            "audio",
            "tts",
            "asr",
            "speech",
            "ocr",
            "mt",
            "translation",
            "coder",
            "code",
            "math",
            "embedding",
            "rerank",
        ];
        let is_specialist = id
            .split(|c: char| c == '-' || c == '.' || c == '_')
            .any(|segment| specialist.contains(&segment));
        if !is_specialist {
            let capabilities = entry.capabilities.get_or_insert_with(Default::default);
            // Force rather than default: third-party catalogs (models.dev)
            // describe other regions' lineups and misreport these models as
            // text-only, while the token-plan mainline is multimodal.
            capabilities.vision = Some(true);
            if capabilities.thinking.is_none() {
                capabilities.thinking = Some(true);
            }
        }
    }
}

fn provider_model_compat(
    kind: ProviderKind,
    id: &str,
) -> (Option<serde_json::Map<String, Value>>, Option<Value>) {
    match kind {
        ProviderKind::KimiCoding => (
            Some(serde_json::Map::from_iter([(
                "User-Agent".into(),
                Value::String("KimiCLI/1.5".into()),
            )])),
            Some(serde_json::json!({
                "allowEmptySignature": id == "k3" || id == "kimi-for-coding",
                "forceAdaptiveThinking": true,
            })),
        ),
        ProviderKind::QwenTokenPlanCn => (
            None,
            Some(serde_json::json!({
                "thinkingFormat": "qwen",
                "supportsDeveloperRole": false,
                "supportsStore": false,
                "supportsReasoningEffort": true,
            })),
        ),
        _ => (None, None),
    }
}

fn reasoning_level_map(value: &Value) -> Option<serde_json::Map<String, Value>> {
    let values = value
        .get("reasoning_options")
        .and_then(Value::as_array)?
        .iter()
        .filter(|option| option.get("type").and_then(Value::as_str) == Some("effort"))
        .filter_map(|option| option.get("values").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .filter(|effort| {
            matches!(
                *effort,
                "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
            )
        })
        .map(|effort| (effort.to_string(), Value::String(effort.to_string())))
        .collect::<serde_json::Map<_, _>>();
    (!values.is_empty()).then_some(values)
}

fn model_cache_path(kind: ProviderKind) -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".pwcli")
            .join("cache")
            .join("provider-models")
            .join(format!("{}.json", kind.as_str()))
    })
}

fn load_cached_models(kind: ProviderKind) -> anyhow::Result<Vec<ModelEntry>> {
    let path = model_cache_path(kind).context("home directory is unavailable")?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse {}", path.display()))
}

fn persist_cached_models(kind: ProviderKind, models: &[ModelEntry]) -> anyhow::Result<()> {
    let path = model_cache_path(kind).context("home directory is unavailable")?;
    let parent = path.parent().context("provider cache has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(models)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn canonical_protocol(api: &str) -> &str {
    match api {
        "openai-completions" => "openai_chat",
        "openai-responses" => "openai_responses",
        "openai-codex-responses" => "openai_codex_responses",
        "anthropic-messages" => "anthropic_messages",
        "google-generative-ai" => "google_generative",
        "google-antigravity" => "google_antigravity",
        other => other,
    }
}

fn collapse_antigravity_model(id: &str) -> &str {
    match id {
        "gemini-3.6-flash-low" | "gemini-3.6-flash-medium" | "gemini-3.6-flash-high" => {
            "gemini-3.6-flash"
        }
        "gemini-3.1-pro-low" | "gemini-pro-agent" => "gemini-3.1-pro",
        other => other,
    }
}

fn antigravity_efforts(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "gemini-3.6-flash" => Some(&["low", "medium", "high"]),
        "gemini-3.1-pro" => Some(&["low", "high"]),
        "claude-sonnet-4-6" | "claude-opus-4-6-thinking" => Some(&["low", "medium", "high", "max"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exposes_five_dynamic_provider_services() {
        let providers = ProviderRegistry::global().all();
        assert_eq!(providers.len(), 5);
        let mut ids: Vec<&str> = providers.iter().map(|provider| provider.id()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![
                "google-antigravity",
                "kimi-coding",
                "openai-codex",
                "qwen-token-plan-cn",
                "xai"
            ]
        );
        // Note: model lists may be populated from the on-disk cache on a
        // used machine; that is runtime behavior by design, not a code
        // contract, so it must not be asserted here.
    }

    #[test]
    fn antigravity_wire_models_collapse_to_picker_models() {
        assert_eq!(
            collapse_antigravity_model("gemini-3.6-flash-high"),
            "gemini-3.6-flash"
        );
        assert_eq!(
            collapse_antigravity_model("gemini-pro-agent"),
            "gemini-3.1-pro"
        );
    }
}
