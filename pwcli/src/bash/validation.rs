/// Bash 命令安全验证
#[derive(Debug, Clone)]
pub struct BashValidation {
    /// 禁止的子串模式（命令注入检测）
    pub forbidden_patterns: Vec<String>,
    /// 是否允许重定向到文件
    pub allow_redirect: bool,
    /// 是否允许管道
    pub allow_pipe: bool,
    /// 是否允许命令替换 $(...)
    pub allow_command_substitution: bool,
    /// 是否允许后台执行 &
    pub allow_background: bool,
}

impl Default for BashValidation {
    fn default() -> Self {
        Self {
            forbidden_patterns: vec![
                "rm -rf /".to_string(),
                "rm -rf /*".to_string(),
                "> /dev/sda".to_string(),
                "mkfs".to_string(),
                ":(){ :|:& };:".to_string(),
                "shutdown".to_string(),
                "reboot".to_string(),
                "halt".to_string(),
                "poweroff".to_string(),
            ],
            allow_redirect: true,
            allow_pipe: true,
            allow_command_substitution: true,
            allow_background: false,
        }
    }
}

impl BashValidation {
    /// 验证命令安全性
    pub fn validate(&self, command: &str) -> anyhow::Result<()> {
        let cmd = command.trim();

        if cmd.is_empty() {
            return Err(anyhow::anyhow!("命令不能为空"));
        }

        // 检测禁止模式
        for pattern in &self.forbidden_patterns {
            if cmd.contains(pattern) {
                return Err(anyhow::anyhow!("命令包含禁止模式: {}", pattern));
            }
        }

        // 检测危险重定向
        if !self.allow_redirect && (cmd.contains('>') || cmd.contains("<<")) {
            return Err(anyhow::anyhow!("重定向操作被禁止"));
        }

        // 检测危险的重定向目标
        if cmd.contains("> /etc/") || cmd.contains("> /sys/") || cmd.contains("> /proc/") {
            return Err(anyhow::anyhow!("向系统目录写入被禁止"));
        }

        // 后台执行检查
        if !self.allow_background && cmd.ends_with('&') {
            return Err(anyhow::anyhow!("后台执行被禁止"));
        }

        Ok(())
    }

    /// 宽松验证（允许更多操作）
    pub fn permissive() -> Self {
        Self {
            forbidden_patterns: vec![
                "rm -rf /".to_string(),
                "> /dev/sda".to_string(),
                ":(){ :|:& };:".to_string(),
            ],
            allow_redirect: true,
            allow_pipe: true,
            allow_command_substitution: true,
            allow_background: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_empty() {
        let v = BashValidation::default();
        assert!(v.validate("").is_err());
    }

    #[test]
    fn test_validate_forbidden() {
        let v = BashValidation::default();
        assert!(v.validate("rm -rf /").is_err());
        assert!(v.validate("echo hello && rm -rf /").is_err());
    }

    #[test]
    fn test_validate_safe() {
        let v = BashValidation::default();
        assert!(v.validate("echo hello").is_ok());
        assert!(v.validate("git status").is_ok());
        assert!(v.validate("ls -la | grep foo").is_ok());
    }

    #[test]
    fn test_validate_system_write() {
        let v = BashValidation::default();
        assert!(v.validate("echo x > /etc/passwd").is_err());
    }

    #[test]
    fn test_permissive() {
        let v = BashValidation::permissive();
        assert!(v.validate("sleep 10 &").is_ok());
        assert!(v.validate("echo hello").is_ok());
    }
}
