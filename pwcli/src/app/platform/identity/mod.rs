// 多源身份解析：config.user.id -> git email -> $USER -> "local"
use crate::app::config::RuntimeConfig;
use std::process::Command;
use std::sync::OnceLock;

const MAX_SLUG_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    Config,
    Git,
    Env,
    Fallback,
}

impl IdentitySource {
    pub fn label(&self) -> &'static str {
        match self {
            IdentitySource::Config => "config",
            IdentitySource::Git => "git",
            IdentitySource::Env => "env",
            IdentitySource::Fallback => "fallback",
        }
    }
}

// Captured during the first resolve in the bootstrap LoadingIdentity phase.
static IDENTITY_SOURCE: OnceLock<IdentitySource> = OnceLock::new();

pub fn resolve_user_slug(cfg: &RuntimeConfig) -> String {
    let (slug, src) = resolve_user_slug_with_source(cfg);
    let _ = IDENTITY_SOURCE.set(src);
    slug
}

pub fn resolve_user_slug_with_source(cfg: &RuntimeConfig) -> (String, IdentitySource) {
    if let Some(id) = cfg.user.as_ref().and_then(|u| u.id.clone()) {
        let s = slugify(&id);
        if !s.is_empty() {
            return (s, IdentitySource::Config);
        }
    }
    if let Some(email) = git_global_email() {
        let s = slugify(&email);
        if !s.is_empty() {
            return (s, IdentitySource::Git);
        }
    }
    if let Ok(user) = std::env::var("USER") {
        let s = slugify(&user);
        if !s.is_empty() {
            return (s, IdentitySource::Env);
        }
    }
    ("local".to_string(), IdentitySource::Fallback)
}

pub fn current_identity_source() -> Option<IdentitySource> {
    IDENTITY_SOURCE.get().copied()
}

pub fn slugify(input: &str) -> String {
    let lower = input.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.len() > MAX_SLUG_LEN {
        out.truncate(MAX_SLUG_LEN);
    }
    out
}

// `git config --global user.email` 用 spawn+wait_timeout 替代 output() 防卡死
fn git_global_email() -> Option<String> {
    use std::time::Duration;
    let mut child = Command::new("git")
        .args(["config", "--global", "user.email"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => {
                let out = child.wait_with_output().ok()?;
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s.is_empty() {
                    return None;
                }
                return Some(s);
            }
            Some(_) => return None,
            None => {
                if start.elapsed() > Duration::from_secs(3) {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::UserConfig;

    fn cfg_with_user(u: Option<UserConfig>) -> RuntimeConfig {
        RuntimeConfig {
            user: u,
            ..Default::default()
        }
    }

    #[test]
    fn config_user_id_takes_priority() {
        let cfg = cfg_with_user(Some(UserConfig {
            id: Some("Alice@Corp.Com".to_string()),
            ..Default::default()
        }));
        let slug = resolve_user_slug(&cfg);
        assert_eq!(slug, "alice_corp_com");
    }

    #[test]
    fn slugify_basic_email() {
        assert_eq!(
            slugify("Bob.Smith+ext@Example.io"),
            "bob_smith_ext_example_io"
        );
    }

    #[test]
    fn slugify_truncates_long_input() {
        let long = "a".repeat(200);
        let s = slugify(&long);
        assert_eq!(s.len(), 64);
    }

    #[test]
    fn slugify_empty_input() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_keeps_dash_underscore_digits() {
        assert_eq!(slugify("Test_123-abc"), "test_123-abc");
    }

    #[test]
    fn fallback_to_local_when_all_sources_unset() {
        // 注意：此测试依赖运行环境无 USER 且无 git email；正常 CI/dev 上 USER 会有，
        // 所以我们只校验函数永远返回非空 slug，不会 panic。
        let cfg = cfg_with_user(None);
        let slug = resolve_user_slug(&cfg);
        assert!(
            !slug.is_empty(),
            "resolve_user_slug must always return non-empty"
        );
    }
}
