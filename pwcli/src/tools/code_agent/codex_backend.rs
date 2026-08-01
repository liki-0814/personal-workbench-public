//! OpenAI Codex backend through an explicitly discovered native ACP adapter.

use super::backend::{AgentTransportKind, ModelInfo, PermissionModeInfo, SubAgentBackend};

fn parse_models(raw: &str) -> Vec<ModelInfo> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| {
            model
                .get("visibility")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|visibility| visibility == "list")
        })
        .filter_map(|model| {
            let id = model.get("slug")?.as_str()?.to_string();
            let context_window = model
                .get("max_context_window")
                .or_else(|| model.get("context_window"))
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or_default();
            let effort_options = model
                .get("supported_reasoning_levels")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|level| level.get("effort"))
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect();
            Some(ModelInfo {
                id,
                is_alias: false,
                context_window_options: Vec::new(),
                default_context_window: context_window,
                effort_options,
                default_effort: model
                    .get("default_reasoning_level")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                display_name: model
                    .get("display_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

pub struct CodexBackend {
    bin: String,
}

impl CodexBackend {
    pub fn new(bin: String) -> Self {
        Self { bin }
    }
}

impl SubAgentBackend for CodexBackend {
    fn name(&self) -> &str {
        "codex"
    }

    fn bin_path(&self) -> &str {
        &self.bin
    }

    fn install_hint(&self) -> &str {
        "npm install -g @agentclientprotocol/codex-acp"
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        let Some(path) = dirs::home_dir().map(|home| home.join(".codex/models_cache.json")) else {
            return Vec::new();
        };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        parse_models(&raw)
    }

    fn permission_modes(&self) -> Vec<PermissionModeInfo> {
        vec![
            PermissionModeInfo {
                id: "default".into(),
                display_name: "Ask for approval".into(),
                description: "Show Codex ACP permission requests".into(),
                dangerous: false,
            },
            PermissionModeInfo {
                id: "bypass".into(),
                display_name: "Bypass permissions".into(),
                description: "Automatically select the persistent allow option when available"
                    .into(),
                dangerous: true,
            },
        ]
    }

    fn transport_kind(&self) -> AgentTransportKind {
        AgentTransportKind::Acp
    }

    fn acp_command(&self) -> Option<(String, Vec<String>)> {
        Some((self.bin.clone(), Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_adapter_uses_only_acp_transport() {
        let backend = CodexBackend::new("codex-acp".into());
        assert_eq!(backend.transport_kind(), AgentTransportKind::Acp);
        assert!(backend.acp_command().unwrap().1.is_empty());
    }

    #[test]
    fn parses_visible_models_from_codex_cache() {
        let models = parse_models(
            r#"{"models":[
                {"slug":"gpt-test","display_name":"GPT Test","visibility":"list",
                 "max_context_window":272000,"default_reasoning_level":"medium",
                 "supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"}]},
                {"slug":"hidden","visibility":"hide"}
            ]}"#,
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-test");
        assert_eq!(models[0].display_name, "GPT Test");
        assert_eq!(models[0].default_context_window, 272_000);
        assert_eq!(models[0].effort_options, ["low", "medium"]);
    }
}
