//! Kimi Code backend using the shared native ACP stdio transport.

use std::process::Command as StdCommand;

use super::backend::{AgentTransportKind, ModelInfo, PermissionModeInfo, SubAgentBackend};

pub struct KimiBackend {
    bin: String,
}

impl KimiBackend {
    pub fn new(bin: String) -> Self {
        Self { bin }
    }
}

impl SubAgentBackend for KimiBackend {
    fn name(&self) -> &str {
        "kimi"
    }

    fn bin_path(&self) -> &str {
        &self.bin
    }

    fn install_hint(&self) -> &str {
        "Install Kimi Code from https://moonshotai.github.io/kimi-code/"
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        let output = StdCommand::new(&self.bin)
            .args(["provider", "list", "--json"])
            .output();
        let models = output
            .ok()
            .filter(|result| result.status.success())
            .and_then(|result| serde_json::from_slice::<serde_json::Value>(&result.stdout).ok())
            .and_then(|value| {
                value
                    .get("models")
                    .and_then(|models| models.as_object())
                    .cloned()
            })
            .unwrap_or_default();
        models
            .into_iter()
            .map(|(id, value)| {
                let context = value
                    .get("maxContextSize")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(0);
                let efforts = value
                    .get("supportEfforts")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let default_effort = value
                    .get("defaultEffort")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| efforts.first().cloned())
                    .unwrap_or_default();
                ModelInfo {
                    display_name: value
                        .get("displayName")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&id)
                        .to_string(),
                    id,
                    is_alias: true,
                    context_window_options: (context > 0).then_some(context).into_iter().collect(),
                    default_context_window: context,
                    effort_options: efforts,
                    default_effort,
                }
            })
            .collect()
    }

    fn permission_modes(&self) -> Vec<PermissionModeInfo> {
        vec![
            permission_mode("default", "Default", "Ask before sensitive actions", false),
            permission_mode("auto", "Auto", "Use Kimi Code automatic approval", false),
            permission_mode("plan", "Plan", "Read-only planning mode", false),
            permission_mode(
                "yolo",
                "Bypass permissions",
                "Automatically approve every action",
                true,
            ),
        ]
    }

    fn transport_kind(&self) -> AgentTransportKind {
        AgentTransportKind::Acp
    }

    fn acp_command(&self) -> Option<(String, Vec<String>)> {
        Some((self.bin.clone(), vec!["acp".into()]))
    }
}

fn permission_mode(
    id: &str,
    display_name: &str,
    description: &str,
    dangerous: bool,
) -> PermissionModeInfo {
    PermissionModeInfo {
        id: id.into(),
        display_name: display_name.into(),
        description: description.into(),
        dangerous,
    }
}
