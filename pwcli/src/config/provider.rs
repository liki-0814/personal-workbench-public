use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 模型能力标记（与前端 ModelEntry.capabilities 对齐）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(default)]
    pub thinking: Option<bool>,
}

/// 模型条目
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    /// 是否在前端选择器显示。None/true 显示，false 隐藏。
    /// pwcli 目前不强制按这个字段过滤——config.json 中声明的模型都可被 provider_override 调用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 单次输出 token 上限。前端 settings 配置；pwcli 通过
    /// `ProviderConfig::effective_max_tokens` 在 LLM 调用层使用：
    /// 调用方显式 > model_entry.max_output > default_max_tokens_for 兜底。
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxOutput")]
    pub max_output: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "contextWindow"
    )]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "requestParams"
    )]
    pub request_params: Option<Map<String, Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "thinkingParams"
    )]
    pub thinking_params: Option<Map<String, Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "deferredToolsMode"
    )]
    pub deferred_tools_mode: Option<String>,
}

/// AI Provider 配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderConfig {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub protocol: String,
    pub model: String,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    /// When true, route provider HTTP through the local daemon proxy.
    /// This is an explicit transport flag, not a vendor-specific special case.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "useProxy")]
    pub use_proxy: Option<bool>,
    /// Deprecated: ignored by adapters. Kept only so old configs still deserialize.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "compatProfile"
    )]
    pub compat_profile: Option<String>,
}

impl ProviderConfig {
    pub fn current_model_entry(&self) -> Option<&ModelEntry> {
        self.models.iter().find(|model| model.id == self.model)
    }

    pub fn uses_proxy(&self) -> bool {
        self.use_proxy.unwrap_or(false)
    }

    /// Normalize legacy protocol names and generic knobs into canonical fields.
    /// Returns true when any field changed.
    ///
    /// This is field-shape migration only. It must not infer vendor-specific
    /// behavior from provider names, base URLs, or model ids.
    pub fn normalize_in_place(&mut self) -> bool {
        let mut changed = false;

        let protocol_key = self.protocol.trim().to_ascii_lowercase().replace('-', "_");
        let normalized_protocol = match protocol_key.as_str() {
            "openai" | "openai_chat" | "openai_compatible" => "openai_chat",
            "openai_responses" | "responses" => "openai_responses",
            "anthropic" | "anthropic_messages" => "anthropic_messages",
            "google" | "gemini" | "google_generative" | "generative_language" => {
                "google_generative"
            }
            other if !other.is_empty() => other,
            _ => "openai_chat",
        };
        if self.protocol != normalized_protocol {
            self.protocol = normalized_protocol.to_string();
            changed = true;
        }

        // Historical configs used vendor-ish mode strings; runtime only checks
        // "is some". Canonicalize any non-empty value to "enabled".
        for model in &mut self.models {
            if let Some(mode) = model.deferred_tools_mode.as_deref() {
                if !mode.is_empty() && mode != "enabled" {
                    model.deferred_tools_mode = Some("enabled".into());
                    changed = true;
                }
            }
        }
        changed
    }

    /// 返回当前 self.model 在 self.models 里登记的 max_output（如果有）。
    pub fn current_model_max_output(&self) -> Option<u32> {
        self.current_model_entry()
            .and_then(|model| model.max_output)
    }

    pub fn current_model_context_window(&self) -> Option<u64> {
        self.current_model_entry()
            .and_then(|model| model.context_window)
    }

    pub fn deferred_tools_mode(&self) -> Option<&str> {
        self.current_model_entry()
            .and_then(|model| model.deferred_tools_mode.as_deref())
    }

    /// 三级 fallback 决定单次请求的 max_tokens：
    /// 调用方显式 > model_entry.max_output > default_max_tokens_for（已含静态表）。
    pub fn effective_max_tokens(&self, requested: Option<u32>) -> u32 {
        if let Some(v) = requested {
            return v;
        }
        if let Some(v) = self.current_model_max_output() {
            return v;
        }
        crate::llm::default_max_tokens_for(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_config_serde() {
        let cfg = ProviderConfig {
            name: "test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            protocol: "openai".to_string(),
            model: "gpt-4".to_string(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let decoded: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, decoded);
    }

    #[test]
    fn normalizes_legacy_protocol_names_and_deferred_tools_mode() {
        let mut cfg = ProviderConfig {
            name: "example".into(),
            base_url: "https://gateway.example.com/v1".into(),
            api_key: "sk".into(),
            protocol: "openai".into(),
            model: "demo-model".into(),
            models: vec![ModelEntry {
                id: "demo-model".into(),
                name: "Demo".into(),
                enabled: None,
                max_output: None,
                context_window: None,
                capabilities: None,
                request_params: None,
                thinking_params: None,
                deferred_tools_mode: Some("legacy-on".into()),
            }],
            use_proxy: None,
            compat_profile: Some("old-label".into()),
        };
        assert!(cfg.normalize_in_place());
        assert_eq!(cfg.protocol, "openai_chat");
        // Proxy is never inferred from URL/name; only explicit useProxy counts.
        assert_eq!(cfg.use_proxy, None);
        assert_eq!(
            cfg.models[0].deferred_tools_mode.as_deref(),
            Some("enabled")
        );
        // compatProfile remains for deserialize compatibility but is unused.
        assert_eq!(cfg.compat_profile.as_deref(), Some("old-label"));
    }
}
