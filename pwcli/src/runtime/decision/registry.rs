use crate::ai::config::ProviderConfig;

/// Resolve a model id to a per-request ProviderConfig (clone provider + set model).
pub fn resolve_model_provider(
    providers: &[ProviderConfig],
    model_id: &str,
) -> Option<ProviderConfig> {
    for p in providers {
        for m in &p.models {
            if m.id == model_id || m.name == model_id {
                let mut cfg = p.clone();
                cfg.model = m.id.clone();
                return Some(cfg);
            }
        }
    }
    None
}

pub fn resolve_model_ref(
    providers: &[ProviderConfig],
    model_ref: &crate::runtime::decision::config::MoaModelRef,
) -> Option<ProviderConfig> {
    providers.iter().find_map(|p| {
        if !model_ref.provider.is_empty() && p.name != model_ref.provider {
            return None;
        }
        p.models.iter().find_map(|m| {
            (m.id == model_ref.model || m.name == model_ref.model).then(|| {
                let mut cfg = p.clone();
                cfg.model = m.id.clone();
                cfg
            })
        })
    })
}

pub fn all_model_ids(providers: &[ProviderConfig]) -> Vec<String> {
    providers
        .iter()
        .flat_map(|p| p.models.iter().map(|m| m.id.clone()))
        .collect()
}

pub fn is_vision_model(providers: &[ProviderConfig], model_id: &str) -> bool {
    for p in providers {
        for m in &p.models {
            if m.id == model_id || m.name == model_id {
                return m
                    .capabilities
                    .as_ref()
                    .and_then(|c| c.vision)
                    .unwrap_or(false);
            }
        }
    }
    false
}

pub async fn load_providers(
    backend: &crate::runtime::backend::BackendClient,
) -> Vec<ProviderConfig> {
    // RuntimeConfig reads the unmasked local source of truth. The backend's
    // per-key API intentionally masks credentials and must only be a legacy
    // fallback when no local providers exist.
    if let Some(providers) = crate::runtime::settings::RuntimeConfig::load().providers {
        if !providers.is_empty() {
            return providers;
        }
    }
    match backend.get_data("ai_providers").await {
        Ok(v) => parse_providers_json(&v),
        Err(_) => Vec::new(),
    }
}

fn parse_providers_json(value: &serde_json::Value) -> Vec<ProviderConfig> {
    use crate::ai::config::{ModelCapabilities, ModelEntry};
    let arr = match value {
        serde_json::Value::Array(a) => a,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
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
                            .and_then(|value| value.as_u64())
                            .filter(|value| *value > 0);
                        let capabilities = m.get("capabilities").map(|c| ModelCapabilities {
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
            model: String::new(),
            models,
            use_proxy,
            compat_profile,
        };
        let _ = provider.normalize_in_place();
        out.push(provider);
    }
    out
}
