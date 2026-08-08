use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::runtime::settings::{
    load_permission_config, save_permission_config, PermissionAllowRule,
};

/// 面向用户的全局 Agent 权限档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPermissionMode {
    Prompt,
    Risk,
    Full,
}

impl AgentPermissionMode {
    pub fn from_name(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "prompt" => Self::Prompt,
            "full" | "yolo" => Self::Full,
            _ => Self::Risk,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Risk => "risk",
            Self::Full => "full",
        }
    }
}

pub fn current_agent_permission_mode() -> AgentPermissionMode {
    AgentPermissionMode::from_name(&load_permission_config().agent_mode)
}

pub fn set_agent_permission_mode(mode: AgentPermissionMode) -> anyhow::Result<()> {
    let mut permissions = load_permission_config();
    permissions.agent_mode = mode.as_str().to_string();
    save_permission_config(&permissions)
}

/// 权限模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// 自动执行（读操作、低风险）
    Auto,
    /// 每次询问用户（写操作、删除）
    Prompt,
    /// 拒绝执行（高危操作）
    Deny,
}

impl PermissionMode {
    pub fn from_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "auto" => PermissionMode::Auto,
            "deny" => PermissionMode::Deny,
            _ => PermissionMode::Prompt,
        }
    }
}

/// 权限策略配置
#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    /// 默认模式
    pub default_mode: PermissionMode,
    /// 每工具覆盖
    pub tool_overrides: HashMap<String, PermissionMode>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            default_mode: PermissionMode::Auto,
            tool_overrides: Self::default_overrides(),
        }
    }
}

impl PermissionPolicy {
    fn default_overrides() -> HashMap<String, PermissionMode> {
        let mut map = HashMap::new();
        // 读操作：自动
        map.insert("list_todos".to_string(), PermissionMode::Auto);
        map.insert("list_bookmarks".to_string(), PermissionMode::Auto);
        map.insert("read_data".to_string(), PermissionMode::Auto);
        map.insert("ls".to_string(), PermissionMode::Auto);
        map.insert("read".to_string(), PermissionMode::Auto);
        map.insert("read_artifact".to_string(), PermissionMode::Auto);
        map.insert("find".to_string(), PermissionMode::Auto);
        map.insert("grep".to_string(), PermissionMode::Auto);
        map.insert("get_file_info".to_string(), PermissionMode::Auto);
        // 写操作：询问
        map.insert("write".to_string(), PermissionMode::Prompt);
        map.insert("edit".to_string(), PermissionMode::Prompt);
        map.insert("remove_file".to_string(), PermissionMode::Prompt);
        // Shell 执行：询问（高风险）
        map.insert("bash".to_string(), PermissionMode::Prompt);
        map
    }

    pub fn mode_for_tool(&self, tool_name: &str) -> PermissionMode {
        self.tool_overrides
            .get(tool_name)
            .copied()
            .unwrap_or(self.default_mode)
    }
}

fn normalize_cwd(cwd: &str) -> PathBuf {
    let expanded = PathBuf::from(shellexpand::tilde(cwd).as_ref());
    expanded.canonicalize().unwrap_or(expanded)
}

pub fn allow_rule_matches(tool: &str, arguments: &str, cwd: &str) -> bool {
    let now = Utc::now();
    let cwd = normalize_cwd(cwd);
    load_permission_config()
        .allow_rules
        .iter()
        .any(|rule| rule_matches(rule, tool, arguments, &cwd, now))
}

fn rule_matches(
    rule: &PermissionAllowRule,
    tool: &str,
    arguments: &str,
    cwd: &Path,
    now: chrono::DateTime<Utc>,
) -> bool {
    rule.tool == tool
        && rule.arguments == arguments
        && normalize_cwd(&rule.cwd) == cwd
        && rule.expires_at.is_none_or(|expires_at| expires_at > now)
}

pub fn add_allow_rule(tool: &str, arguments: &str, cwd: &str) -> anyhow::Result<String> {
    let mut permissions = load_permission_config();
    permissions.allow_rules.retain(|rule| {
        rule.expires_at
            .is_none_or(|expires_at| expires_at > Utc::now())
    });
    let cwd = normalize_cwd(cwd).to_string_lossy().into_owned();
    if let Some(existing) = permissions.allow_rules.iter().find(|rule| {
        rule.tool == tool
            && rule.arguments == arguments
            && normalize_cwd(&rule.cwd) == Path::new(&cwd)
    }) {
        return Ok(existing.id.clone());
    }
    let id = format!("allow_rule_{}", uuid::Uuid::now_v7().simple());
    permissions.allow_rules.push(PermissionAllowRule {
        id: id.clone(),
        tool: tool.to_string(),
        arguments: arguments.to_string(),
        cwd,
        expires_at: Some(Utc::now() + chrono::Duration::days(90)),
    });
    save_permission_config(&permissions)?;
    Ok(id)
}

pub fn remove_allow_rule(id: &str) -> anyhow::Result<bool> {
    let mut permissions = load_permission_config();
    let previous = permissions.allow_rules.len();
    permissions.allow_rules.retain(|rule| rule.id != id);
    if permissions.allow_rules.len() == previous {
        return Ok(false);
    }
    save_permission_config(&permissions)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = PermissionPolicy::default();
        assert_eq!(policy.mode_for_tool("list_todos"), PermissionMode::Auto);
        assert_eq!(policy.mode_for_tool("write"), PermissionMode::Prompt);
        assert_eq!(policy.mode_for_tool("edit"), PermissionMode::Prompt);
        assert_eq!(policy.mode_for_tool("grep"), PermissionMode::Auto);
        assert_eq!(policy.mode_for_tool("unknown_tool"), PermissionMode::Auto);
    }

    #[test]
    fn test_mode_from_name() {
        assert_eq!(PermissionMode::from_name("auto"), PermissionMode::Auto);
        assert_eq!(PermissionMode::from_name("prompt"), PermissionMode::Prompt);
        assert_eq!(PermissionMode::from_name("deny"), PermissionMode::Deny);
    }

    #[test]
    fn agent_mode_defaults_to_risk_for_unknown_values() {
        assert_eq!(
            AgentPermissionMode::from_name("unknown"),
            AgentPermissionMode::Risk
        );
        assert_eq!(
            AgentPermissionMode::from_name("prompt"),
            AgentPermissionMode::Prompt
        );
        assert_eq!(
            AgentPermissionMode::from_name("full"),
            AgentPermissionMode::Full
        );
    }

    #[test]
    fn allow_rule_is_exact_and_expires() {
        let now = Utc::now();
        let cwd = std::env::current_dir().unwrap();
        let rule = PermissionAllowRule {
            id: "rule".into(),
            tool: "bash".into(),
            arguments: r#"{"command":"git status"}"#.into(),
            cwd: cwd.to_string_lossy().into_owned(),
            expires_at: Some(now + chrono::Duration::minutes(1)),
        };
        assert!(rule_matches(
            &rule,
            "bash",
            r#"{"command":"git status"}"#,
            &cwd,
            now
        ));
        assert!(!rule_matches(
            &rule,
            "bash",
            r#"{"command":"git push"}"#,
            &cwd,
            now
        ));
        assert!(!rule_matches(
            &rule,
            "bash",
            r#"{"command":"git status"}"#,
            &cwd,
            now + chrono::Duration::minutes(2)
        ));
    }
}
