use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::runtime::settings::local_config::{DelegationSection, ExecutorDefaultSection};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingRequest {
    pub role: String,
    /// `None`, an empty value, or `auto` selects from the configured role routes.
    pub executor: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
}

impl RoutingRequest {
    pub fn new(role: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutorCapabilities {
    pub executor_id: String,
    pub installed: bool,
    /// Empty means the executor accepts every configured delegation role.
    pub supported_roles: Vec<String>,
    /// Empty capability lists mean the executor did not publish that capability.
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    pub permission_modes: Vec<String>,
    pub native_default_model: Option<String>,
    pub native_default_effort: Option<String>,
    pub native_default_permission_mode: Option<String>,
}

impl ExecutorCapabilities {
    pub fn installed(executor_id: impl Into<String>) -> Self {
        Self {
            executor_id: executor_id.into(),
            installed: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionSnapshot {
    pub resolved_executor_kind: String,
    pub resolved_executor_id: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub routing_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WaitingConfiguration {
    pub requested_executor: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "status", content = "details")]
pub enum RoutingDecision {
    Resolved(ResolutionSnapshot),
    WaitingConfiguration(WaitingConfiguration),
}

/// Resolve a delegation using only persisted configuration and static executor capabilities.
/// Runtime load and transient worker capacity deliberately are not routing inputs.
pub fn resolve_routing(
    config: &DelegationSection,
    request: &RoutingRequest,
    capabilities: &[ExecutorCapabilities],
) -> RoutingDecision {
    let role = configured_role(config, &request.role);
    let explicit = request
        .executor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"));
    if let Some(executor) = explicit {
        return resolve_explicit(config, request, role, executor, capabilities);
    }

    let routes = config
        .roles
        .get(role)
        .map(|entry| entry.routes.as_slice())
        .unwrap_or(&[]);
    for route in routes {
        let route = route.trim();
        if let Some(route_role) = route.strip_prefix("pwcli:") {
            if let Some(snapshot) = resolve_candidate(
                config,
                request,
                route_role,
                "pwcli",
                "pwcli",
                &format!("pwcli:{route_role}"),
                capabilities,
                format!("role {role} selected route {route}"),
            ) {
                return RoutingDecision::Resolved(snapshot);
            }
            continue;
        }
        if route == "cli:auto" {
            for executor in configured_cli_priority(config) {
                if let Some(snapshot) = resolve_candidate(
                    config,
                    request,
                    role,
                    "cli",
                    &executor,
                    &executor,
                    capabilities,
                    format!("role {role} selected cli:auto via {executor}"),
                ) {
                    return RoutingDecision::Resolved(snapshot);
                }
            }
            continue;
        }
        if let Some(executor) = route.strip_prefix("cli:") {
            let executor = canonical_executor(executor);
            if let Some(snapshot) = resolve_candidate(
                config,
                request,
                role,
                "cli",
                &executor,
                &executor,
                capabilities,
                format!("role {role} selected route {route}"),
            ) {
                return RoutingDecision::Resolved(snapshot);
            }
        }
    }

    RoutingDecision::WaitingConfiguration(WaitingConfiguration {
        requested_executor: "auto".to_string(),
        reason: format!("no enabled, installed, capability-compatible route for role {role}"),
    })
}

fn resolve_explicit(
    config: &DelegationSection,
    request: &RoutingRequest,
    role: &str,
    requested_executor: &str,
    capabilities: &[ExecutorCapabilities],
) -> RoutingDecision {
    let executor = canonical_executor(requested_executor);
    let (kind, resolved_id) = if executor == "pwcli" {
        ("pwcli", format!("pwcli:{role}"))
    } else {
        ("cli", executor.clone())
    };
    match candidate_error(config, request, role, &executor, capabilities) {
        Some(reason) => RoutingDecision::WaitingConfiguration(WaitingConfiguration {
            requested_executor: requested_executor.to_string(),
            reason,
        }),
        None => RoutingDecision::Resolved(build_snapshot(
            config,
            request,
            kind,
            &executor,
            &resolved_id,
            capabilities,
            format!("explicit executor {requested_executor}"),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_candidate(
    config: &DelegationSection,
    request: &RoutingRequest,
    role: &str,
    kind: &str,
    executor: &str,
    resolved_id: &str,
    capabilities: &[ExecutorCapabilities],
    routing_reason: String,
) -> Option<ResolutionSnapshot> {
    candidate_error(config, request, role, executor, capabilities).map_or_else(
        || {
            Some(build_snapshot(
                config,
                request,
                kind,
                executor,
                resolved_id,
                capabilities,
                routing_reason,
            ))
        },
        |_| None,
    )
}

fn candidate_error(
    config: &DelegationSection,
    request: &RoutingRequest,
    role: &str,
    executor: &str,
    capabilities: &[ExecutorCapabilities],
) -> Option<String> {
    if config.manage_executors_explicitly && !contains_executor(&config.enabled_executors, executor)
    {
        return Some(format!("executor {executor} is not enabled"));
    }
    let Some(capability) = find_capability(capabilities, executor) else {
        return Some(format!("executor {executor} did not publish capabilities"));
    };
    if !capability.installed {
        return Some(format!("executor {executor} is not installed"));
    }
    if !capability.supported_roles.is_empty()
        && !capability
            .supported_roles
            .iter()
            .any(|supported| supported.eq_ignore_ascii_case(role))
    {
        return Some(format!("executor {executor} does not support role {role}"));
    }

    let defaults = config.executor_defaults.get(executor);
    let model = select_setting(
        request.model.as_deref(),
        defaults.map(|value| value.model.as_str()),
        capability.native_default_model.as_deref(),
    );
    if !compatible(&capability.models, model.as_deref(), false) {
        return Some(format!(
            "executor {executor} does not support model {}",
            model.unwrap()
        ));
    }
    let effort = select_setting(
        request.effort.as_deref(),
        defaults.map(|value| value.effort.as_str()),
        capability.native_default_effort.as_deref(),
    );
    if !compatible(&capability.efforts, effort.as_deref(), true) {
        return Some(format!(
            "executor {executor} does not support effort {}",
            effort.unwrap()
        ));
    }
    let permission_mode = select_setting(
        request.permission_mode.as_deref(),
        defaults.map(|value| value.permission_mode.as_str()),
        capability.native_default_permission_mode.as_deref(),
    );
    if permission_mode.as_deref() != Some("default")
        && !compatible(
            &capability.permission_modes,
            permission_mode.as_deref(),
            true,
        )
    {
        return Some(format!(
            "executor {executor} does not support permission mode {}",
            permission_mode.unwrap()
        ));
    }
    None
}

fn build_snapshot(
    config: &DelegationSection,
    request: &RoutingRequest,
    kind: &str,
    executor: &str,
    resolved_id: &str,
    capabilities: &[ExecutorCapabilities],
    routing_reason: String,
) -> ResolutionSnapshot {
    let capability = find_capability(capabilities, executor)
        .expect("candidate capability was validated before snapshot creation");
    let defaults = config
        .executor_defaults
        .get(executor)
        .unwrap_or(&EMPTY_EXECUTOR_DEFAULT);
    ResolutionSnapshot {
        resolved_executor_kind: kind.to_string(),
        resolved_executor_id: resolved_id.to_string(),
        model: select_setting(
            request.model.as_deref(),
            Some(defaults.model.as_str()),
            capability.native_default_model.as_deref(),
        ),
        effort: select_setting(
            request.effort.as_deref(),
            Some(defaults.effort.as_str()),
            capability.native_default_effort.as_deref(),
        ),
        permission_mode: select_setting(
            request.permission_mode.as_deref(),
            Some(defaults.permission_mode.as_str()),
            capability.native_default_permission_mode.as_deref(),
        ),
        routing_reason,
    }
}

static EMPTY_EXECUTOR_DEFAULT: ExecutorDefaultSection = ExecutorDefaultSection {
    model: String::new(),
    effort: String::new(),
    permission_mode: String::new(),
};

fn configured_role<'a>(config: &'a DelegationSection, requested: &'a str) -> &'a str {
    let requested = requested.trim();
    if config.roles.contains_key(requested) {
        requested
    } else {
        "general"
    }
}

fn configured_cli_priority(config: &DelegationSection) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut priority = config
        .cli_priority
        .iter()
        .map(|executor| canonical_executor(executor))
        .filter(|executor| executor != "pwcli" && seen.insert(executor.clone()))
        .collect::<Vec<_>>();
    if !config.manage_executors_explicitly {
        for executor in ["codex", "qoder", "kimi"] {
            if seen.insert(executor.to_string()) {
                priority.push(executor.to_string());
            }
        }
    }
    priority
}

fn canonical_executor(executor: &str) -> String {
    match executor.trim().to_ascii_lowercase().as_str() {
        "qodercli" => "qoder".to_string(),
        "kimi-code" => "kimi".to_string(),
        executor => executor.to_string(),
    }
}

fn contains_executor(configured: &[String], executor: &str) -> bool {
    configured
        .iter()
        .any(|candidate| canonical_executor(candidate) == executor)
}

fn find_capability<'a>(
    capabilities: &'a [ExecutorCapabilities],
    executor: &str,
) -> Option<&'a ExecutorCapabilities> {
    capabilities
        .iter()
        .find(|candidate| canonical_executor(&candidate.executor_id) == executor)
}

fn select_setting(
    explicit: Option<&str>,
    executor_default: Option<&str>,
    native_default: Option<&str>,
) -> Option<String> {
    [explicit, executor_default, native_default]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn compatible(supported: &[String], selected: Option<&str>, case_insensitive: bool) -> bool {
    let Some(selected) = selected else {
        return true;
    };
    supported.is_empty()
        || supported.iter().any(|candidate| {
            if case_insensitive {
                candidate.eq_ignore_ascii_case(selected)
            } else {
                candidate == selected
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(executor: &str) -> ExecutorCapabilities {
        ExecutorCapabilities::installed(executor)
    }

    fn resolved(decision: RoutingDecision) -> ResolutionSnapshot {
        match decision {
            RoutingDecision::Resolved(snapshot) => snapshot,
            other => panic!("expected resolved routing, got {other:?}"),
        }
    }

    #[test]
    fn explicit_executor_never_falls_back() {
        let mut config = DelegationSection::default();
        config.manage_executors_explicitly = true;
        config
            .enabled_executors
            .retain(|executor| executor != "codex");
        let mut request = RoutingRequest::new("engineer");
        request.executor = Some("codex".to_string());
        let decision = resolve_routing(
            &config,
            &request,
            &[capability("codex"), capability("qoder")],
        );
        assert_eq!(
            decision,
            RoutingDecision::WaitingConfiguration(WaitingConfiguration {
                requested_executor: "codex".to_string(),
                reason: "executor codex is not enabled".to_string(),
            })
        );
    }

    #[test]
    fn migrated_executor_list_does_not_hide_newly_detected_cli() {
        let mut config = DelegationSection::default();
        config.enabled_executors = vec!["pwcli".into(), "qoder".into()];
        config.cli_priority = vec!["qoder".into()];
        let mut request = RoutingRequest::new("engineer");
        request.executor = Some("codex".into());

        let snapshot = resolved(resolve_routing(&config, &request, &[capability("codex")]));

        assert_eq!(snapshot.resolved_executor_id, "codex");
    }

    #[test]
    fn explicit_uninstalled_executor_waits_without_fallback() {
        let mut request = RoutingRequest::new("engineer");
        request.executor = Some("codex".to_string());
        let decision = resolve_routing(
            &DelegationSection::default(),
            &request,
            &[
                ExecutorCapabilities {
                    executor_id: "codex".to_string(),
                    installed: false,
                    ..ExecutorCapabilities::default()
                },
                capability("qoder"),
            ],
        );
        assert!(matches!(
            decision,
            RoutingDecision::WaitingConfiguration(WaitingConfiguration {
                requested_executor,
                reason,
            }) if requested_executor == "codex" && reason.contains("not installed")
        ));
    }

    #[test]
    fn auto_uses_role_routes_and_cli_priority() {
        let config = DelegationSection {
            cli_priority: vec!["qoder".into(), "codex".into()],
            ..DelegationSection::default()
        };
        let qoder = capability("qoder");
        let snapshot = resolved(resolve_routing(
            &config,
            &RoutingRequest::new("engineer"),
            &[capability("pwcli"), capability("codex"), qoder],
        ));
        assert_eq!(snapshot.resolved_executor_kind, "cli");
        assert_eq!(snapshot.resolved_executor_id, "qoder");
        assert!(snapshot.routing_reason.contains("cli:auto via qoder"));
    }

    #[test]
    fn auto_skips_uninstalled_or_role_incompatible_candidates() {
        let config = DelegationSection::default();
        let codex = ExecutorCapabilities {
            supported_roles: vec!["reviewer".into()],
            ..capability("codex")
        };
        let qoder = ExecutorCapabilities {
            installed: false,
            ..capability("qoder")
        };
        let snapshot = resolved(resolve_routing(
            &config,
            &RoutingRequest::new("engineer"),
            &[codex, qoder, capability("kimi")],
        ));
        assert_eq!(snapshot.resolved_executor_id, "kimi");
    }

    #[test]
    fn pwcli_route_keeps_the_selected_role_in_the_executor_id() {
        let snapshot = resolved(resolve_routing(
            &DelegationSection::default(),
            &RoutingRequest::new("researcher"),
            &[capability("pwcli")],
        ));
        assert_eq!(snapshot.resolved_executor_kind, "pwcli");
        assert_eq!(snapshot.resolved_executor_id, "pwcli:researcher");
    }

    #[test]
    fn snapshot_precedence_is_explicit_then_executor_then_native() {
        let mut config = DelegationSection::default();
        config.executor_defaults.insert(
            "codex".into(),
            ExecutorDefaultSection {
                model: "configured-model".into(),
                effort: "configured-effort".into(),
                permission_mode: String::new(),
            },
        );
        let mut request = RoutingRequest::new("engineer");
        request.executor = Some("codex".into());
        request.model = Some("explicit-model".into());
        let codex = ExecutorCapabilities {
            models: vec!["explicit-model".into()],
            efforts: vec!["configured-effort".into()],
            native_default_model: Some("native-model".into()),
            native_default_effort: Some("native-effort".into()),
            native_default_permission_mode: Some("native-permission".into()),
            ..capability("codex")
        };
        let snapshot = resolved(resolve_routing(&config, &request, &[codex]));
        assert_eq!(snapshot.model.as_deref(), Some("explicit-model"));
        assert_eq!(snapshot.effort.as_deref(), Some("configured-effort"));
        assert_eq!(
            snapshot.permission_mode.as_deref(),
            Some("native-permission")
        );
    }

    #[test]
    fn explicit_incompatible_setting_waits_for_configuration() {
        let mut request = RoutingRequest::new("engineer");
        request.executor = Some("codex".into());
        request.model = Some("missing".into());
        let codex = ExecutorCapabilities {
            models: vec!["available".into()],
            ..capability("codex")
        };
        let decision = resolve_routing(&DelegationSection::default(), &request, &[codex]);
        assert!(matches!(
            decision,
            RoutingDecision::WaitingConfiguration(WaitingConfiguration {
                requested_executor,
                reason,
            }) if requested_executor == "codex" && reason.contains("model missing")
        ));
    }
}
