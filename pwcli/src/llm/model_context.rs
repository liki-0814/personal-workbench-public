//! Task-local active-model context used by tools such as image-aware `read_file`.

use tokio::task_local;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveModelContext {
    pub provider_id: Option<String>,
    pub model_id: String,
    pub effort: Option<String>,
    pub thinking: bool,
    pub supports_vision: bool,
}

task_local! {
    pub static ACTIVE_MODEL: ActiveModelContext;
}

pub async fn with_active_model<F, T>(context: ActiveModelContext, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    ACTIVE_MODEL.scope(context, future).await
}

pub fn compute_vision_support_for(
    providers: &[crate::config::ProviderConfig],
    model_id: &str,
) -> bool {
    crate::fusion::registry::is_vision_model(providers, model_id)
}

pub fn try_current_supports_vision() -> Option<bool> {
    ACTIVE_MODEL
        .try_with(|context| context.supports_vision)
        .ok()
}

pub fn try_current_model_id() -> Option<String> {
    ACTIVE_MODEL
        .try_with(|context| context.model_id.clone())
        .ok()
}

pub fn try_current_provider_id() -> Option<String> {
    ACTIVE_MODEL
        .try_with(|context| context.provider_id.clone())
        .ok()
        .flatten()
}

pub fn try_current() -> Option<ActiveModelContext> {
    ACTIVE_MODEL.try_with(Clone::clone).ok()
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

    #[tokio::test]
    async fn active_context_is_scoped() {
        assert_eq!(try_current_supports_vision(), None);
        let context = ActiveModelContext {
            provider_id: Some("openai".into()),
            model_id: "gpt-4o".into(),
            effort: Some("high".into()),
            thinking: true,
            supports_vision: true,
        };
        let observed = with_active_model(context, async {
            (try_current_supports_vision(), try_current())
        })
        .await;
        assert_eq!(observed.0, Some(true));
        let observed = observed.1.unwrap();
        assert_eq!(observed.provider_id.as_deref(), Some("openai"));
        assert_eq!(observed.model_id, "gpt-4o");
        assert_eq!(observed.effort.as_deref(), Some("high"));
        assert!(observed.thinking);
        assert_eq!(try_current_supports_vision(), None);
    }

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
        }];
        assert!(compute_vision_support_for(&providers, "vision"));
        assert!(!compute_vision_support_for(&providers, "missing"));
    }
}
