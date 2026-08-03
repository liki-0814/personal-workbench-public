//! Explicit active-model context propagated through ToolExecutionContext.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveModelContext {
    pub provider_id: Option<String>,
    pub model_id: String,
    pub effort: Option<String>,
    pub thinking: bool,
    pub supports_vision: bool,
}

pub fn compute_vision_support_for(
    providers: &[crate::config::ProviderConfig],
    model_id: &str,
) -> bool {
    crate::fusion::registry::is_vision_model(providers, model_id)
}

/// Return the reasoning-effort value that the provider will put on the wire.
/// Static request parameters always apply; thinking parameters take precedence
/// when extended thinking is enabled for the turn.
pub fn effort_for_provider(
    provider: &crate::config::ProviderConfig,
    thinking: bool,
) -> Option<String> {
    let entry = provider.current_model_entry()?;
    let request_effort = entry
        .request_params
        .as_ref()
        .and_then(|params| params.get("reasoning_effort"))
        .and_then(serde_json::Value::as_str);
    let thinking_effort = thinking
        .then_some(entry.thinking_params.as_ref())
        .flatten()
        .and_then(|params| params.get("reasoning_effort"))
        .and_then(serde_json::Value::as_str);
    thinking_effort
        .or(request_effort)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_effort_matches_the_effective_request_parameters() {
        use crate::config::provider::{ModelEntry, ProviderConfig};
        use serde_json::{Map, Value};

        let provider = ProviderConfig {
            name: "test".into(),
            base_url: "http://localhost".into(),
            api_key: "key".into(),
            protocol: "openai".into(),
            model: "reasoner".into(),
            models: vec![ModelEntry {
                id: "reasoner".into(),
                name: "Reasoner".into(),
                enabled: None,
                max_output: None,
                context_window: None,
                capabilities: None,
                request_params: Some(Map::from_iter([(
                    "reasoning_effort".into(),
                    Value::String("medium".into()),
                )])),
                thinking_params: Some(Map::from_iter([(
                    "reasoning_effort".into(),
                    Value::String("max".into()),
                )])),
                deferred_tools_mode: None,
            }],
            use_proxy: None,
            compat_profile: None,
        };

        assert_eq!(
            effort_for_provider(&provider, false).as_deref(),
            Some("medium")
        );
        assert_eq!(effort_for_provider(&provider, true).as_deref(), Some("max"));
    }

    #[test]
    fn model_capability_comes_from_provider_registry() {
        use crate::config::provider::{ModelCapabilities, ModelEntry};
        let providers = vec![crate::config::ProviderConfig {
            name: "test".into(),
            base_url: "http://localhost".into(),
            api_key: "key".into(),
            protocol: "openai".into(),
            model: "vision".into(),
            models: vec![ModelEntry {
                id: "vision".into(),
                name: "Vision".into(),
                enabled: None,
                max_output: None,
                context_window: None,
                capabilities: Some(ModelCapabilities {
                    vision: Some(true),
                    thinking: None,
                }),
                request_params: None,
                thinking_params: None,
                deferred_tools_mode: None,
            }],
            use_proxy: None,
            compat_profile: None,
        }];
        assert!(compute_vision_support_for(&providers, "vision"));
        assert!(!compute_vision_support_for(&providers, "missing"));
    }
}
