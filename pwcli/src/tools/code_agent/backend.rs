//! SubAgentBackend trait — the abstraction boundary for pluggable CLI sub-agents.
//!
//! Implementations provide native transport startup, model discovery and permissions.

use std::process::Command as StdCommand;

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentTransportKind {
    Acp,
}

/// A model entry returned by `list_models()`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub is_alias: bool,
    /// Supported context window sizes (token counts). Empty = not configurable.
    pub context_window_options: Vec<u32>,
    /// Default context window (0 = not applicable).
    pub default_context_window: u32,
    /// Supported reasoning effort levels. Empty = not configurable for this model.
    pub effort_options: Vec<String>,
    /// Default effort level (empty = not applicable).
    pub default_effort: String,
    /// Human-readable display name (e.g. "Opus 4.8"). Empty = use id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
}

/// A native permission/approval mode exposed by a CLI backend.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionModeInfo {
    pub id: String,
    pub display_name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dangerous: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Native transport capability for a code-agent backend.
pub trait SubAgentBackend: Send + Sync {
    /// Human-readable name for logs and progress messages.
    fn name(&self) -> &str;

    /// Absolute path to the backend binary.
    fn bin_path(&self) -> &str;

    /// Install instructions shown when the binary is not found.
    fn install_hint(&self) -> &str;

    /// List available models. Backends with dynamic discovery (e.g. `--model-list`)
    /// run the CLI; others return a static list of known aliases.
    fn list_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    /// Permission modes are backend-native; callers must not reuse values
    /// between different CLIs.
    fn permission_modes(&self) -> Vec<PermissionModeInfo> {
        Vec::new()
    }

    fn transport_kind(&self) -> AgentTransportKind;

    /// Program and arguments for a native ACP stdio server.
    fn acp_command(&self) -> Option<(String, Vec<String>)> {
        None
    }
}

/// Helper: run a CLI command and parse stdout lines as model names.
/// Skips header lines (all-uppercase like "MODEL").
pub fn run_model_list_command(bin: &str, flag: &str) -> Vec<String> {
    run_model_list_with_names(bin, flag)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Helper: run a CLI command and parse stdout lines as `(id, display_name)` pairs.
/// Lines in `id - display_name` format are split; lines without ` - ` use the full
/// line as both id and display_name. Skips header lines and the trailing "Tip:" line.
pub fn run_model_list_with_names(bin: &str, flag: &str) -> Vec<(String, String)> {
    let output = StdCommand::new(bin)
        .arg(flag)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .filter(|l| {
                !l.starts_with("Available") && !l.starts_with("Tip:") && l != "MODEL" && l != "---"
            })
            .map(|l| {
                if let Some((id, name)) = l.split_once(" - ") {
                    (id.trim().to_string(), name.trim().to_string())
                } else {
                    (l.clone(), l)
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::ModelInfo;

    #[test]
    fn model_capabilities_serialize_for_the_web_client() {
        let value = serde_json::to_value(ModelInfo {
            id: "model".into(),
            is_alias: false,
            context_window_options: vec![128_000],
            default_context_window: 128_000,
            effort_options: vec!["high".into()],
            default_effort: "high".into(),
            display_name: "Model".into(),
        })
        .unwrap();
        assert_eq!(value["contextWindowOptions"][0], 128_000);
        assert_eq!(value["defaultEffort"], "high");
        assert!(value.get("context_window_options").is_none());
    }
}
