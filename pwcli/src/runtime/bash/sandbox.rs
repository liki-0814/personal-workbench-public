use std::path::Path;

/// 沙箱配置
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// 允许执行的命令前缀（空列表表示全部允许）
    pub allowed_commands: Vec<String>,
    /// 禁止执行的命令
    pub blocked_commands: Vec<String>,
    /// 允许访问的路径前缀
    pub allowed_paths: Vec<String>,
    /// 禁止访问的路径
    pub blocked_paths: Vec<String>,
    /// 敏感环境变量（执行前清除）
    pub sensitive_env_vars: Vec<String>,
    /// 最大输出字节数
    pub max_output_bytes: usize,
    /// 是否允许网络访问
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            allowed_commands: Vec::new(),
            blocked_commands: vec![
                "rm -rf /".to_string(),
                "rm -rf ~".to_string(),
                "rm -rf $HOME".to_string(),
                ":(){ :|:& };:".to_string(), // fork bomb
                "mkfs".to_string(),
                "dd if=/dev/zero".to_string(),
            ],
            allowed_paths: Vec::new(),
            blocked_paths: vec![
                "/etc/shadow".to_string(),
                "/etc/passwd".to_string(),
                "/etc/ssh".to_string(),
            ],
            sensitive_env_vars: vec![
                "ANTHROPIC_API_KEY".to_string(),
                "OPENAI_API_KEY".to_string(),
                "GITHUB_TOKEN".to_string(),
                "AWS_ACCESS_KEY_ID".to_string(),
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "PRIVATE_KEY".to_string(),
                "PASSWORD".to_string(),
                "TOKEN".to_string(),
            ],
            max_output_bytes: 100_000,
            allow_network: true,
        }
    }
}

impl SandboxConfig {
    /// 创建严格的沙箱（只允许特定命令）
    pub fn strict() -> Self {
        Self {
            allowed_commands: vec![
                "git ".to_string(),
                "ls ".to_string(),
                "cat ".to_string(),
                "echo ".to_string(),
                "pwd".to_string(),
                "find ".to_string(),
                "grep ".to_string(),
                "mkdir ".to_string(),
                "touch ".to_string(),
                "npm ".to_string(),
                "cargo ".to_string(),
                "node ".to_string(),
                "python ".to_string(),
                "python3 ".to_string(),
                "rustc ".to_string(),
            ],
            blocked_commands: Vec::new(),
            allowed_paths: Vec::new(),
            blocked_paths: vec!["/etc".to_string(), "/var/log".to_string()],
            sensitive_env_vars: vec![
                "ANTHROPIC_API_KEY".to_string(),
                "OPENAI_API_KEY".to_string(),
                "GITHUB_TOKEN".to_string(),
            ],
            max_output_bytes: 50_000,
            allow_network: false,
        }
    }

    /// 验证路径是否在允许范围内
    pub fn validate_path(&self, path: &str) -> anyhow::Result<()> {
        let path = Path::new(path);

        // 检查是否在禁止路径中
        for blocked in &self.blocked_paths {
            if path.starts_with(blocked) {
                return Err(anyhow::anyhow!("路径被禁止访问: {}", path.display()));
            }
        }

        // 如果设置了允许路径列表，检查是否在范围内
        if !self.allowed_paths.is_empty() {
            let allowed = self.allowed_paths.iter().any(|p| path.starts_with(p));
            if !allowed {
                return Err(anyhow::anyhow!("路径不在允许范围内: {}", path.display()));
            }
        }

        Ok(())
    }

    /// 验证命令是否在允许范围内。
    /// 注意：这是"防误操作 + 显然恶意"的轻量防御层，不是安全边界。
    /// 真正的攻击者通过 base64 解码、`sh -c $VAR`、PATH 注入等方式都能绕过。
    pub fn validate_command(&self, command: &str) -> anyhow::Result<()> {
        let cmd_trimmed = command.trim();

        // 1) 规范化空白后做前缀匹配（防御 `rm -rf  /` 这类多空格变体）
        let normalized = cmd_trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        for blocked in &self.blocked_commands {
            let blocked_norm = blocked.split_whitespace().collect::<Vec<_>>().join(" ");
            if blocked_norm.is_empty() {
                continue;
            }
            if normalized == blocked_norm || normalized.starts_with(&format!("{} ", blocked_norm)) {
                return Err(anyhow::anyhow!("命令被禁止: {}", command));
            }
        }

        // 2) 结构性检查：捕获 flag 顺序无关的 `rm -rf <根级路径>`
        let tokens: Vec<&str> = cmd_trimmed.split_whitespace().collect();
        if is_destructive_rm(&tokens) {
            return Err(anyhow::anyhow!(
                "命令被禁止（递归强删根级路径）: {}",
                command
            ));
        }

        // 3) 如果设置了允许列表，检查是否在范围内
        if !self.allowed_commands.is_empty() {
            let allowed = self
                .allowed_commands
                .iter()
                .any(|prefix| cmd_trimmed.starts_with(prefix));
            if !allowed {
                return Err(anyhow::anyhow!("命令不在允许列表中: {}", command));
            }
        }

        Ok(())
    }

    /// 检查是否是敏感环境变量
    pub fn is_sensitive_env(&self, key: &str) -> bool {
        self.sensitive_env_vars.iter().any(|s| {
            key.eq_ignore_ascii_case(s)
                || key.to_uppercase().ends_with("_API_KEY")
                || key.to_uppercase().ends_with("_SECRET")
                || key.to_uppercase().ends_with("_TOKEN")
        })
    }
}

/// True if `tokens` is `rm <flags...>` where `<flags>` includes both `r/R` (recursive)
/// and `f` (force) — across separate or combined flags — AND a target argument is
/// a "root-level" sensitive path (`/`, `~`, `$HOME`, `/*`).
/// This is order-invariant: `rm -rf /`, `rm -fr /`, `rm -r -f /` all match.
fn is_destructive_rm(tokens: &[&str]) -> bool {
    if tokens.first() != Some(&"rm") {
        return false;
    }
    let mut has_recursive = false;
    let mut has_force = false;
    let mut targets_root = false;
    for &t in &tokens[1..] {
        if let Some(rest) = t.strip_prefix("--") {
            if rest == "recursive" {
                has_recursive = true;
            }
            if rest == "force" {
                has_force = true;
            }
        } else if let Some(flags) = t.strip_prefix('-') {
            for ch in flags.chars() {
                if ch == 'r' || ch == 'R' {
                    has_recursive = true;
                }
                if ch == 'f' {
                    has_force = true;
                }
            }
        } else {
            // Positional argument — check for sensitive paths.
            if matches!(t, "/" | "~" | "$HOME" | "/*" | "~/*") {
                targets_root = true;
            }
        }
    }
    has_recursive && has_force && targets_root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SandboxConfig::default();
        assert!(!config.blocked_commands.is_empty());
        assert!(!config.sensitive_env_vars.is_empty());
    }

    #[test]
    fn test_validate_blocked_command() {
        let config = SandboxConfig::default();
        assert!(config.validate_command("rm -rf /").is_err());
        assert!(config.validate_command("echo hello").is_ok());
    }

    #[test]
    fn test_destructive_rm_variants_all_caught() {
        let config = SandboxConfig::default();
        // All of these should be blocked, regardless of flag order or whitespace.
        assert!(config.validate_command("rm -rf /").is_err());
        assert!(config.validate_command("rm -fr /").is_err());
        assert!(config.validate_command("rm -r -f /").is_err());
        assert!(config.validate_command("rm -rf  /").is_err()); // double space
        assert!(config.validate_command("rm --recursive --force /").is_err());
        assert!(config.validate_command("rm -rf ~").is_err());
        assert!(config.validate_command("rm -rf $HOME").is_err());
        // Negative cases — these are not destructive root deletes.
        assert!(config.validate_command("rm /tmp/foo").is_ok());
        assert!(config.validate_command("rm -rf /tmp/foo").is_ok());
    }

    #[test]
    fn test_strict_mode() {
        let config = SandboxConfig::strict();
        assert!(config.validate_command("git status").is_ok());
        assert!(config.validate_command("rm file").is_err());
    }

    #[test]
    fn test_sensitive_env() {
        let config = SandboxConfig::default();
        assert!(config.is_sensitive_env("ANTHROPIC_API_KEY"));
        assert!(config.is_sensitive_env("MY_API_KEY"));
        assert!(!config.is_sensitive_env("PATH"));
    }

    #[test]
    fn test_validate_path() {
        let config = SandboxConfig::default();
        assert!(config.validate_path("/etc/shadow").is_err());
        assert!(config.validate_path("/tmp/test").is_ok());
    }
}
