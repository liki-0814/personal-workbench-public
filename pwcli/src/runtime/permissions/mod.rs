pub mod bash_safety;
pub mod broker;
pub mod policy;

pub use broker::*;
pub use policy::*;

pub use crate::agent_core::contracts::ports::PermissionOutcome;
use crate::runtime::tools::registry::ToolImpact;

/// 权限请求
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub arguments: String,
    pub mode: PermissionMode,
}

/// 权限引擎
#[derive(Debug, Clone)]
pub struct PermissionEngine {
    policy: PermissionPolicy,
}

impl crate::agent_core::contracts::ports::PermissionPort for PermissionEngine {
    fn check_call(
        &self,
        tool_name: &str,
        arguments: &str,
        impact: ToolImpact,
    ) -> PermissionOutcome {
        PermissionEngine::check_call(self, tool_name, arguments, impact)
    }
}

impl PermissionEngine {
    pub fn new(policy: PermissionPolicy) -> Self {
        Self { policy }
    }

    pub fn default_engine() -> Self {
        Self::new(PermissionPolicy::default())
    }

    pub fn check(&self, tool_name: &str) -> PermissionOutcome {
        match self.policy.mode_for_tool(tool_name) {
            PermissionMode::Auto => PermissionOutcome::Allow,
            PermissionMode::Deny => PermissionOutcome::Deny,
            PermissionMode::Prompt => PermissionOutcome::Prompt,
        }
    }

    /// Check a concrete tool call. `data_crud` multiplexes read and mutation
    /// actions, so tool-name-only policy is not sufficient for it.
    pub fn check_call(
        &self,
        tool_name: &str,
        arguments: &str,
        impact: ToolImpact,
    ) -> PermissionOutcome {
        self.check_call_for_mode(
            current_agent_permission_mode(),
            tool_name,
            arguments,
            impact,
        )
    }

    fn check_call_for_mode(
        &self,
        mode: AgentPermissionMode,
        tool_name: &str,
        arguments: &str,
        impact: ToolImpact,
    ) -> PermissionOutcome {
        if tool_name == "data_crud" {
            return data_crud_outcome(mode, arguments);
        }
        match mode {
            AgentPermissionMode::Full if tool_name != "bash" => PermissionOutcome::Allow,
            AgentPermissionMode::Full => PermissionOutcome::Prompt,
            AgentPermissionMode::Prompt
                if tool_name == "bash"
                    || tool_name.starts_with("web_")
                    || tool_name.starts_with("ssh_") =>
            {
                PermissionOutcome::Prompt
            }
            AgentPermissionMode::Prompt => match impact {
                ToolImpact::Observe | ToolImpact::Control => PermissionOutcome::Allow,
                ToolImpact::ReversibleMutation
                | ToolImpact::IrreversibleMutation
                | ToolImpact::ExternalSideEffect => PermissionOutcome::Prompt,
            },
            AgentPermissionMode::Risk if tool_name == "bash" => {
                let parsed: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
                let command = parsed
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                match bash_safety::evaluate(command) {
                    bash_safety::Verdict::Allow => PermissionOutcome::Allow,
                    bash_safety::Verdict::Prompt(_) | bash_safety::Verdict::Deny(_) => {
                        PermissionOutcome::Prompt
                    }
                }
            }
            AgentPermissionMode::Risk if tool_name.starts_with("ssh_") => PermissionOutcome::Prompt,
            AgentPermissionMode::Risk => match impact {
                ToolImpact::IrreversibleMutation | ToolImpact::ExternalSideEffect => {
                    PermissionOutcome::Prompt
                }
                ToolImpact::Observe | ToolImpact::Control | ToolImpact::ReversibleMutation => {
                    PermissionOutcome::Allow
                }
            },
        }
    }

    /// 批量检查多个工具
    pub fn check_batch<'a>(&self, tool_names: &'a [String]) -> Vec<(&'a str, PermissionOutcome)> {
        tool_names
            .iter()
            .map(|name| (name.as_str(), self.check(name)))
            .collect()
    }
}

fn data_crud_outcome(mode: AgentPermissionMode, arguments: &str) -> PermissionOutcome {
    let Ok(arguments) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return PermissionOutcome::Deny;
    };
    classify_data_crud_value(mode, &arguments)
}

fn classify_data_crud_value(
    mode: AgentPermissionMode,
    arguments: &serde_json::Value,
) -> PermissionOutcome {
    let action = arguments
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match action {
        "read" | "query" => PermissionOutcome::Allow,
        "create" | "update" => match mode {
            AgentPermissionMode::Prompt => PermissionOutcome::Prompt,
            AgentPermissionMode::Risk | AgentPermissionMode::Full => PermissionOutcome::Allow,
        },
        "delete" => match mode {
            AgentPermissionMode::Full => PermissionOutcome::Allow,
            AgentPermissionMode::Prompt | AgentPermissionMode::Risk => PermissionOutcome::Prompt,
        },
        "batch" => {
            let Some(operations) = arguments
                .pointer("/payload/operations")
                .and_then(serde_json::Value::as_array)
            else {
                return PermissionOutcome::Deny;
            };
            operations
                .iter()
                .map(|operation| classify_data_crud_value(mode, operation))
                .fold(PermissionOutcome::Allow, |result, outcome| {
                    if result == PermissionOutcome::Deny || outcome == PermissionOutcome::Deny {
                        PermissionOutcome::Deny
                    } else if result == PermissionOutcome::Prompt
                        || outcome == PermissionOutcome::Prompt
                    {
                        PermissionOutcome::Prompt
                    } else {
                        PermissionOutcome::Allow
                    }
                })
        }
        _ => PermissionOutcome::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_engine_allow() {
        let engine = PermissionEngine::default_engine();
        assert_eq!(engine.check("list_todos"), PermissionOutcome::Allow);
    }

    #[test]
    fn test_permission_engine_prompt() {
        let engine = PermissionEngine::default_engine();
        assert_eq!(engine.check("write"), PermissionOutcome::Prompt);
    }

    #[test]
    fn test_permission_engine_batch() {
        let engine = PermissionEngine::default_engine();
        let names = vec!["list_todos".to_string(), "write".to_string()];
        let results = engine.check_batch(&names);
        assert_eq!(results[0].1, PermissionOutcome::Allow);
        assert_eq!(results[1].1, PermissionOutcome::Prompt);
    }

    #[test]
    fn data_crud_is_classified_by_action_and_mode() {
        assert_eq!(
            data_crud_outcome(
                AgentPermissionMode::Risk,
                r#"{"domain":"todo","action":"query","payload":{}}"#
            ),
            PermissionOutcome::Allow
        );
        assert_eq!(
            data_crud_outcome(
                AgentPermissionMode::Risk,
                r#"{"domain":"todo","action":"update","payload":{}}"#
            ),
            PermissionOutcome::Allow
        );
        assert_eq!(
            data_crud_outcome(
                AgentPermissionMode::Risk,
                r#"{"domain":"todo","action":"delete","payload":{}}"#
            ),
            PermissionOutcome::Prompt
        );
        assert_eq!(
            data_crud_outcome(
                AgentPermissionMode::Prompt,
                r#"{"domain":"todo","action":"create","payload":{}}"#
            ),
            PermissionOutcome::Prompt
        );
        assert_eq!(
            data_crud_outcome(
                AgentPermissionMode::Full,
                r#"{"domain":"todo","action":"delete","payload":{}}"#
            ),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn risky_data_crud_batch_prompts_when_any_operation_is_risky() {
        let arguments = r#"{
            "action":"batch",
            "payload":{"operations":[
                {"domain":"todo","action":"update","payload":{}},
                {"domain":"todo","action":"delete","payload":{}}
            ]}
        }"#;
        assert_eq!(
            data_crud_outcome(AgentPermissionMode::Risk, arguments),
            PermissionOutcome::Prompt
        );
    }

    #[test]
    fn agent_modes_follow_tool_impact_without_name_allowlists() {
        let engine = PermissionEngine::default_engine();
        assert_eq!(
            engine.check_call_for_mode(
                AgentPermissionMode::Prompt,
                "create_document",
                "{}",
                ToolImpact::ReversibleMutation,
            ),
            PermissionOutcome::Prompt
        );
        assert_eq!(
            engine.check_call_for_mode(
                AgentPermissionMode::Risk,
                "manage_jobs",
                r#"{"action":"run"}"#,
                ToolImpact::ExternalSideEffect,
            ),
            PermissionOutcome::Prompt
        );
        assert_eq!(
            engine.check_call_for_mode(
                AgentPermissionMode::Risk,
                "manage_jobs",
                r#"{"action":"list"}"#,
                ToolImpact::Observe,
            ),
            PermissionOutcome::Allow
        );
        assert_eq!(
            engine.check_call_for_mode(
                AgentPermissionMode::Full,
                "remove_file",
                "{}",
                ToolImpact::IrreversibleMutation,
            ),
            PermissionOutcome::Allow
        );
    }
}
