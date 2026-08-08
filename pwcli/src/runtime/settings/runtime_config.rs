use crate::ai::config::ProviderConfig;
use crate::runtime::settings::ConfigRepository;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeConfig {
    #[serde(default = "default_backend_url")]
    pub backend_url: String,
    pub providers: Option<Vec<ProviderConfig>>,
    pub active_provider: Option<String>,
    #[serde(default)]
    pub features: RuntimeFeatureConfig,
    #[serde(default)]
    pub permissions: RuntimePermissionConfig,
    #[serde(default)]
    pub user: Option<UserConfig>,
}

// 解析后的多源用户身份；缺字段视为未知，由 identity 链回退
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UserConfig {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryInjectionConfig {
    #[serde(default = "default_memory_injection_mode")]
    pub mode: String,
    #[serde(default = "default_true")]
    pub auto_retrieve: bool,
    #[serde(default = "default_memory_max_results")]
    pub max_results: usize,
    #[serde(default = "default_max_digest_bytes")]
    pub max_digest_bytes: usize,
}

impl Default for MemoryInjectionConfig {
    fn default() -> Self {
        Self {
            mode: default_memory_injection_mode(),
            auto_retrieve: true,
            max_results: default_memory_max_results(),
            max_digest_bytes: default_max_digest_bytes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeFeatureConfig {
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default = "default_true")]
    pub session_persistence: bool,
    #[serde(default = "default_true")]
    pub auto_compact: bool,
    #[serde(default = "default_true")]
    pub auto_memory_extract: bool,
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: usize,
    #[serde(default)]
    pub memory_injection: MemoryInjectionConfig,
}

impl Default for RuntimeFeatureConfig {
    fn default() -> Self {
        Self {
            streaming: true,
            session_persistence: true,
            auto_compact: true,
            auto_memory_extract: true,
            max_tool_rounds: 5,
            memory_injection: MemoryInjectionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimePermissionConfig {
    #[serde(default = "default_agent_permission_mode")]
    pub agent_mode: String,
    #[serde(default = "default_permission_mode")]
    pub default_mode: String,
    #[serde(default)]
    pub tool_overrides: HashMap<String, String>,
    #[serde(default)]
    pub allow_rules: Vec<PermissionAllowRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionAllowRule {
    pub id: String,
    pub tool: String,
    pub arguments: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for RuntimePermissionConfig {
    fn default() -> Self {
        Self {
            agent_mode: default_agent_permission_mode(),
            default_mode: "auto".to_string(),
            tool_overrides: HashMap::new(),
            allow_rules: Vec::new(),
        }
    }
}

fn default_agent_permission_mode() -> String {
    "risk".to_string()
}

/// Read only the permission section from the shared config file.
/// This avoids deserializing or rewriting frontend-owned sibling sections.
pub fn load_permission_config() -> RuntimePermissionConfig {
    let path = RuntimeConfig::config_file();
    let Ok(raw) = fs::read_to_string(path) else {
        return RuntimePermissionConfig::default();
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return RuntimePermissionConfig::default();
    };
    root.get("permissions")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Atomically replace only `permissions`, preserving providers, tools and all
/// other keys owned by the frontend or another pwcli config view.
pub fn save_permission_config(permissions: &RuntimePermissionConfig) -> anyhow::Result<()> {
    let value = serde_json::to_value(permissions)?;
    ConfigRepository::standard().update(move |root| {
        root.as_object_mut()
            .expect("ConfigRepository guarantees an object")
            .insert("permissions".into(), value);
        Ok(())
    })
}

fn default_backend_url() -> String {
    "http://localhost:3456".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_tool_rounds() -> usize {
    5
}

fn default_permission_mode() -> String {
    "auto".to_string()
}

fn default_memory_injection_mode() -> String {
    "retrieval".to_string()
}

fn default_memory_max_results() -> usize {
    5
}

fn default_max_digest_bytes() -> usize {
    1024
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            backend_url: default_backend_url(),
            providers: None,
            active_provider: None,
            features: RuntimeFeatureConfig::default(),
            permissions: RuntimePermissionConfig::default(),
            user: None,
        }
    }
}

impl RuntimeConfig {
    pub fn config_dir() -> PathBuf {
        dirs::home_dir().expect("home dir not found").join(".pwcli")
    }

    pub fn config_file() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_file();
        let config: Self = if path.exists() {
            let raw = fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            Self::default()
        };

        config
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let me = serde_json::to_value(self)?;
        ConfigRepository::standard().update(move |root| {
            if let (Some(root_obj), Some(me_obj)) = (root.as_object_mut(), me.as_object()) {
                for (key, value) in me_obj {
                    root_obj.insert(key.clone(), value.clone());
                }
            }
            Ok(())
        })
    }

    pub fn active_provider(&self) -> Option<&ProviderConfig> {
        let providers = self.providers.as_ref()?;
        if let Some(id) = &self.active_provider {
            providers.iter().find(|provider| {
                provider.name == *id || crate::ai::provider::provider_id(provider) == id
            })
        } else {
            providers.first()
        }
    }

    /// Pull `ai_providers` from the backend cache (seeded from config.json)
    /// and replace local providers wholesale. This is the
    /// canonical config flow when the daemon is running. Falls back silently
    /// to whatever `load()` produced if the backend is unreachable.
    ///
    /// Preserves the user's `active_provider` selection and per-provider
    /// `model` selection when the names/ids still exist in the backend's list.
    pub async fn override_providers_from_backend(
        &mut self,
        backend: &crate::runtime::backend::BackendClient,
    ) -> anyhow::Result<()> {
        let value = backend.get_data("ai_providers").await?;
        let backend_providers = parse_backend_providers(&value)?;
        if backend_providers.is_empty() {
            return Ok(()); // backend has nothing — keep local
        }

        // The backend intentionally masks API keys. Preserve the local secret
        // as well as the user's per-provider model selection by name.
        let old_provider_by_name: HashMap<String, ProviderConfig> = self
            .providers
            .as_ref()
            .map(|ps| ps.iter().map(|p| (p.name.clone(), p.clone())).collect())
            .unwrap_or_default();

        let merged: Vec<ProviderConfig> = backend_providers
            .into_iter()
            .filter_map(|bp| {
                let local = old_provider_by_name.get(&bp.name);
                merge_backend_provider(bp, local)
            })
            .collect();

        // Preserve active_provider if it still exists in the new list.
        let active_still_valid = self
            .active_provider
            .as_ref()
            .map(|id| {
                merged.iter().any(|provider| {
                    provider.name == *id || crate::ai::provider::provider_id(provider) == id
                })
            })
            .unwrap_or(false);
        if !active_still_valid {
            self.active_provider = merged
                .first()
                .map(|provider| crate::ai::provider::provider_id(provider).to_string());
        }
        self.providers = Some(merged);
        Ok(())
    }
}

fn merge_backend_provider(
    mut backend: ProviderConfig,
    local: Option<&ProviderConfig>,
) -> Option<ProviderConfig> {
    if is_masked_api_key(&backend.api_key) {
        backend.api_key = local.map(|provider| provider.api_key.clone())?;
    }
    if backend.api_key.is_empty() || is_masked_api_key(&backend.api_key) {
        return None;
    }
    let preferred = local
        .map(|provider| &provider.model)
        .filter(|model| backend.models.iter().any(|entry| &entry.id == *model))
        .cloned();
    backend.model = preferred
        .or_else(|| backend.models.first().map(|entry| entry.id.clone()))
        .unwrap_or_default();
    Some(backend)
}

fn is_masked_api_key(value: &str) -> bool {
    value.contains("****") || value == "******"
}

/// Parse the JSON returned by `GET /api/data/ai_providers` (frontend's
/// camelCase `AiProvider[]`) into pwcli's snake_case `ProviderConfig[]`.
fn parse_backend_providers(value: &serde_json::Value) -> anyhow::Result<Vec<ProviderConfig>> {
    use crate::ai::config::ModelEntry;
    let arr = match value {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Null => return Ok(Vec::new()),
        _ => return Err(anyhow::anyhow!("ai_providers is not an array")),
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let base_url = item
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let api_key = item
            .get("apiKey")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let protocol = item
            .get("protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("openai");
        let use_proxy = item.get("useProxy").and_then(|v| v.as_bool());
        let compat_profile = item
            .get("compatProfile")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if name.is_empty() || base_url.is_empty() || api_key.is_empty() {
            continue;
        }
        let models: Vec<ModelEntry> = item
            .get("models")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        let id = m.get("id").and_then(|v| v.as_str())?.to_string();
                        let mn = m
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&id)
                            .to_string();
                        let enabled = m.get("enabled").and_then(|v| v.as_bool());
                        let max_output =
                            m.get("maxOutput").and_then(|v| v.as_u64()).and_then(|n| {
                                if n > 0 && n <= u64::from(u32::MAX) {
                                    Some(n as u32)
                                } else {
                                    None
                                }
                            });
                        let context_window = m
                            .get("contextWindow")
                            .and_then(|v| v.as_u64())
                            .filter(|value| *value > 0);
                        let capabilities =
                            m.get("capabilities")
                                .map(|c| crate::ai::config::ModelCapabilities {
                                    vision: c.get("vision").and_then(|v| v.as_bool()),
                                    thinking: c.get("thinking").and_then(|v| v.as_bool()),
                                    image: c.get("image").and_then(|v| v.as_bool()),
                                });
                        let request_params = m
                            .get("requestParams")
                            .and_then(|value| value.as_object())
                            .cloned();
                        let thinking_params = m
                            .get("thinkingParams")
                            .and_then(|value| value.as_object())
                            .cloned();
                        let deferred_tools_mode = m
                            .get("deferredToolsMode")
                            .and_then(|value| value.as_str())
                            .map(str::to_string);
                        Some(ModelEntry {
                            id,
                            name: mn,
                            enabled,
                            max_output,
                            context_window,
                            capabilities,
                            request_params,
                            thinking_params,
                            deferred_tools_mode,
                            ..Default::default()
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut provider = ProviderConfig {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            protocol: protocol.to_string(),
            model: String::new(), // filled by override_providers_from_backend
            models,
            use_proxy,
            compat_profile,
        };
        let _ = provider.normalize_in_place();
        out.push(provider);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = RuntimeConfig::default();
        assert_eq!(cfg.backend_url, "http://localhost:3456");
        assert!(cfg.providers.is_none());
        assert!(cfg.features.streaming);
        assert!(cfg.features.session_persistence);
        assert!(cfg.features.auto_compact);
        assert!(cfg.features.auto_memory_extract);
        assert_eq!(cfg.features.memory_injection.mode, "retrieval");
        assert_eq!(cfg.features.max_tool_rounds, 5);
        assert_eq!(cfg.permissions.default_mode, "auto");
    }

    #[test]
    fn test_active_provider_fallback_to_first() {
        let cfg = RuntimeConfig {
            backend_url: "http://localhost:3456".to_string(),
            providers: Some(vec![ProviderConfig {
                name: "test".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: "sk-test".to_string(),
                protocol: "openai".to_string(),
                model: "gpt-4".to_string(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: None,
            }]),
            active_provider: None,
            features: RuntimeFeatureConfig::default(),
            permissions: RuntimePermissionConfig::default(),
            user: None,
        };
        let active = cfg.active_provider();
        assert!(active.is_some());
        assert_eq!(active.unwrap().name, "test");
    }

    #[test]
    fn test_active_provider_by_name() {
        let cfg = RuntimeConfig {
            backend_url: "http://localhost:3456".to_string(),
            providers: Some(vec![
                ProviderConfig {
                    name: "openai".to_string(),
                    base_url: "https://api.openai.com/v1".to_string(),
                    api_key: "sk-o".to_string(),
                    protocol: "openai".to_string(),
                    model: "gpt-4".to_string(),
                    models: Vec::new(),
                    use_proxy: None,
                    compat_profile: None,
                },
                ProviderConfig {
                    name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    api_key: "sk-a".to_string(),
                    protocol: "anthropic".to_string(),
                    model: "claude-sonnet-4-6".to_string(),
                    models: Vec::new(),
                    use_proxy: None,
                    compat_profile: None,
                },
            ]),
            active_provider: Some("anthropic".to_string()),
            features: RuntimeFeatureConfig::default(),
            permissions: RuntimePermissionConfig::default(),
            user: None,
        };
        let active = cfg.active_provider().unwrap();
        assert_eq!(active.name, "anthropic");
        assert_eq!(active.protocol, "anthropic");
    }

    #[test]
    fn backend_provider_mask_keeps_local_api_key() {
        let local = ProviderConfig {
            name: "Example".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key: "real-secret-key".into(),
            protocol: "openai".into(),
            model: "qwen-max".into(),
            models: vec![crate::ai::config::ModelEntry {
                id: "qwen-max".into(),
                name: "Qwen Max".into(),
                enabled: None,
                max_output: None,
                context_window: None,
                capabilities: None,
                request_params: None,
                thinking_params: None,
                deferred_tools_mode: None,
                ..Default::default()
            }],
            use_proxy: None,
            compat_profile: None,
        };
        let mut masked = local.clone();
        masked.api_key = "real****-key".into();
        masked.model.clear();

        let merged = merge_backend_provider(masked, Some(&local)).unwrap();
        assert_eq!(merged.api_key, "real-secret-key");
        assert_eq!(merged.model, "qwen-max");
    }

    #[test]
    fn masked_backend_provider_without_local_secret_is_ignored() {
        let masked = ProviderConfig {
            name: "Example".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key: "003b****9ef4".into(),
            protocol: "openai".into(),
            model: String::new(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        };

        assert!(merge_backend_provider(masked, None).is_none());
    }
}
