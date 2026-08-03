use crate::config::provider::{ModelCapabilities, ModelEntry};
use crate::config::ProviderConfig;

use super::ProviderKind;

#[derive(Debug, Clone)]
pub struct BuiltinProvider {
    pub kind: ProviderKind,
    pub name: &'static str,
    pub base_url: &'static str,
    pub protocol: &'static str,
    pub env_key: Option<&'static str>,
    pub default_model: &'static str,
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderCatalog;

impl ProviderCatalog {
    pub fn all(&self) -> Vec<BuiltinProvider> {
        vec![
            self.get(ProviderKind::KimiCoding).unwrap(),
            self.get(ProviderKind::Xai).unwrap(),
            self.get(ProviderKind::OpenAiCodex).unwrap(),
            self.get(ProviderKind::QwenTokenPlanCn).unwrap(),
        ]
    }

    pub fn get(&self, kind: ProviderKind) -> Option<BuiltinProvider> {
        let model = |id: &str, thinking: bool, vision: bool, context_window: u64| ModelEntry {
            id: id.to_string(),
            name: id.to_string(),
            enabled: Some(true),
            max_output: None,
            context_window: Some(context_window),
            capabilities: Some(ModelCapabilities {
                vision: Some(vision),
                thinking: Some(thinking),
            }),
            request_params: None,
            thinking_params: None,
            deferred_tools_mode: None,
        };
        Some(match kind {
            ProviderKind::KimiCoding => BuiltinProvider {
                kind,
                name: "Kimi Coding",
                base_url: "https://api.kimi.com/coding",
                protocol: "anthropic_messages",
                env_key: Some("KIMI_API_KEY"),
                default_model: "kimi-for-coding",
                models: vec![
                    model("kimi-for-coding", true, true, 262_144),
                    model("kimi-for-coding-highspeed", true, true, 262_144),
                ],
            },
            ProviderKind::Xai => BuiltinProvider {
                kind,
                name: "Grok / xAI",
                base_url: "https://api.x.ai/v1",
                protocol: "openai_responses",
                env_key: Some("XAI_API_KEY"),
                default_model: "grok-4.5",
                models: vec![
                    model("grok-4.5", true, true, 256_000),
                    model("grok-4.3", true, true, 131_072),
                    model("grok-code-fast-1", false, false, 256_000),
                ],
            },
            ProviderKind::OpenAiCodex => BuiltinProvider {
                kind,
                name: "OpenAI Codex",
                base_url: "https://chatgpt.com/backend-api",
                protocol: "openai_codex_responses",
                env_key: None,
                default_model: "gpt-5.5",
                models: vec![
                    model("gpt-5.5", true, true, 272_000),
                    model("gpt-5.4", true, true, 272_000),
                    model("gpt-5.6-sol", true, true, 372_000),
                    model("gpt-5.6-terra", true, true, 372_000),
                    model("gpt-5.6-luna", true, true, 372_000),
                ],
            },
            ProviderKind::QwenTokenPlanCn => BuiltinProvider {
                kind,
                name: "Qwen Token Plan CN",
                base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
                protocol: "openai_chat",
                env_key: Some("QWEN_TOKEN_PLAN_CN_API_KEY"),
                default_model: "qwen3.7-max",
                models: vec![
                    model("qwen3.7-max", true, true, 262_144),
                    model("qwen3.7-plus", true, true, 262_144),
                ],
            },
            ProviderKind::Custom => return None,
        })
    }

    pub fn materialize(&self, provider: &ProviderConfig) -> ProviderConfig {
        let Some(builtin) = self.get(super::provider_kind(provider)) else {
            return provider.clone();
        };
        let mut resolved = provider.clone();
        resolved.name = builtin.name.to_string();
        resolved.base_url = builtin.base_url.to_string();
        resolved.protocol = builtin.protocol.to_string();
        if resolved.model.trim().is_empty() {
            resolved.model = builtin.default_model.to_string();
        }
        if resolved.models.is_empty() {
            resolved.models = builtin.models;
        }
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_the_four_supported_builtins() {
        let catalog = ProviderCatalog;
        let ids = catalog
            .all()
            .into_iter()
            .map(|provider| provider.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec!["kimi-coding", "xai", "openai-codex", "qwen-token-plan-cn"]
        );
    }
}
