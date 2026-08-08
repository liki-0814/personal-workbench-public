//! Qoder CLI backend implementation.
//!
//! QoderCLI capability discovery and ACP startup configuration.

use super::backend::{
    run_model_list_command, AgentTransportKind, ModelInfo, PermissionModeInfo, SubAgentBackend,
};

/// Per-model capability data for qodercli tier models.
struct QoderModelCaps {
    ctx_options: &'static [u32],
    default_ctx: u32,
    effort_options: &'static [&'static str],
    default_effort: &'static str,
}

fn qoder_caps(model_id: &str) -> Option<QoderModelCaps> {
    match model_id {
        "Ultimate" => Some(QoderModelCaps {
            ctx_options: &[200_000, 400_000, 1_000_000],
            default_ctx: 200_000,
            effort_options: &["low", "medium", "high", "xhigh", "max"],
            default_effort: "high",
        }),
        "Performance" => Some(QoderModelCaps {
            ctx_options: &[272_000, 400_000, 1_000_000],
            default_ctx: 272_000,
            effort_options: &["low", "medium", "high", "xhigh", "max"],
            default_effort: "medium",
        }),
        "Efficient" => Some(QoderModelCaps {
            ctx_options: &[200_000],
            default_ctx: 200_000,
            effort_options: &["low", "medium", "high"],
            default_effort: "medium",
        }),
        "Lite" => Some(QoderModelCaps {
            ctx_options: &[128_000],
            default_ctx: 128_000,
            effort_options: &["low", "medium"],
            default_effort: "medium",
        }),
        "Kimi-K3" => Some(QoderModelCaps {
            ctx_options: &[1_048_576],
            default_ctx: 1_048_576,
            effort_options: &["low", "medium", "high", "xhigh", "max"],
            default_effort: "high",
        }),
        _ => None,
    }
}

pub struct QoderBackend {
    bin: String,
}

impl QoderBackend {
    pub fn new(bin: String) -> Self {
        Self { bin }
    }
}

impl SubAgentBackend for QoderBackend {
    fn name(&self) -> &str {
        "qoder"
    }

    fn bin_path(&self) -> &str {
        &self.bin
    }

    fn install_hint(&self) -> &str {
        "npm install -g @anthropic-ai/qodercli"
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        let names = run_model_list_command(&self.bin, "--list-models");
        names
            .into_iter()
            .map(|id| {
                if let Some(caps) = qoder_caps(&id) {
                    ModelInfo {
                        id: id.clone(),
                        is_alias: false,
                        context_window_options: caps.ctx_options.to_vec(),
                        default_context_window: caps.default_ctx,
                        effort_options: caps.effort_options.iter().map(|s| s.to_string()).collect(),
                        default_effort: caps.default_effort.to_string(),
                        display_name: id,
                    }
                } else {
                    ModelInfo {
                        id: id.clone(),
                        is_alias: false,
                        context_window_options: vec![],
                        default_context_window: 0,
                        effort_options: vec![],
                        default_effort: String::new(),
                        display_name: id,
                    }
                }
            })
            .collect()
    }

    fn permission_modes(&self) -> Vec<PermissionModeInfo> {
        vec![
            permission_mode(
                "default",
                "Default",
                "Use QoderCLI's configured approval behavior",
                false,
            ),
            permission_mode(
                "accept_edits",
                "Accept edits",
                "Approve file edits while keeping other checks",
                false,
            ),
            permission_mode(
                "auto",
                "Auto",
                "Let QoderCLI choose when approval is needed",
                false,
            ),
            permission_mode(
                "dont_ask",
                "Don't ask",
                "Decline operations that require interaction",
                false,
            ),
            permission_mode(
                "bypass_permissions",
                "Bypass permissions",
                "Skip all QoderCLI permission checks",
                true,
            ),
        ]
    }

    fn transport_kind(&self) -> AgentTransportKind {
        AgentTransportKind::Acp
    }

    fn acp_command(&self) -> Option<(String, Vec<String>)> {
        Some((self.bin.clone(), vec!["--acp".into()]))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_k3_keeps_one_million_context() {
        let backend = QoderBackend::new("qodercli".into());
        let caps = qoder_caps("Kimi-K3").unwrap();
        assert_eq!(caps.default_ctx, 1_048_576);
        assert_eq!(caps.default_effort, "high");
        assert_eq!(backend.transport_kind(), AgentTransportKind::Acp);
    }

    #[test]
    fn qoder_uses_native_acp_transport() {
        let backend = QoderBackend::new("qodercli".into());
        assert_eq!(backend.acp_command().unwrap().1, ["--acp"]);
    }
}
