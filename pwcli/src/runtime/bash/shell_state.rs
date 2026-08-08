use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use tokio::sync::Mutex as AsyncMutex;

use super::{execute_bash, BashCommandInput, BashCommandOutput, SandboxConfig};

#[derive(Debug, Default)]
struct ShellState {
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
}

fn states() -> &'static Mutex<HashMap<String, Arc<AsyncMutex<ShellState>>>> {
    static STATES: OnceLock<Mutex<HashMap<String, Arc<AsyncMutex<ShellState>>>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn state_for(session_id: &str) -> Arc<AsyncMutex<ShellState>> {
    let mut states = states().lock().unwrap_or_else(|error| error.into_inner());
    Arc::clone(
        states
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(ShellState::default()))),
    )
}

pub fn clear(session_id: &str) {
    states()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(session_id);
    // 会话结束时回收登记的后台/长跑子进程
    super::kill_tracked(session_id);
}

pub async fn execute(
    session_id: &str,
    command: String,
    explicit_cwd: Option<String>,
    timeout_secs: Option<u64>,
    sandbox: &SandboxConfig,
) -> Result<BashCommandOutput> {
    let root = fs_base()?;
    let dump_dir = crate::runtime::settings::local_config::data_dir().join("shell-state");
    execute_with_root(
        session_id,
        command,
        explicit_cwd,
        timeout_secs,
        sandbox,
        &root,
        &dump_dir,
    )
    .await
}

async fn execute_with_root(
    session_id: &str,
    command: String,
    explicit_cwd: Option<String>,
    timeout_secs: Option<u64>,
    sandbox: &SandboxConfig,
    root: &Path,
    dump_dir: &Path,
) -> Result<BashCommandOutput> {
    let state = state_for(session_id);
    let mut state = state.lock().await;
    let root = root
        .canonicalize()
        .with_context(|| format!("tools.fsBase is not accessible: {}", root.display()))?;
    let cwd = explicit_cwd
        .map(|value| PathBuf::from(shellexpand::tilde(&value).as_ref()))
        .or_else(|| state.cwd.clone())
        .unwrap_or_else(|| root.clone());
    let cwd = ensure_within_root(&cwd, &root, "replay 前")?;

    std::fs::create_dir_all(dump_dir)?;
    let nonce = uuid::Uuid::now_v7().simple().to_string();
    let cwd_dump = dump_dir.join(format!("{nonce}.cwd"));
    let env_dump = dump_dir.join(format!("{nonce}.env"));
    create_private_file(&cwd_dump)?;
    create_private_file(&env_dump)?;

    let wrapped = format!(
        "{{ {{\n{command}\n}}; __pwcli_status=$?; pwd -P >&3; env -0 >&4; exit \"$__pwcli_status\"; }} 3>{} 4>{}",
        shell_quote(&cwd_dump),
        shell_quote(&env_dump),
    );
    let mut input = BashCommandInput::new(wrapped)
        .with_session_id(session_id)
        .with_cwd(cwd.to_string_lossy().into_owned());
    if let Some(timeout_secs) = timeout_secs {
        input = input.with_timeout(timeout_secs);
    }
    for (key, value) in &state.env {
        input = input.with_env(key, value);
    }
    let output = execute_bash(input, sandbox).await;

    if !matches!(&output, Ok(value) if value.timed_out) {
        if let Ok(next_cwd) = std::fs::read_to_string(&cwd_dump) {
            let next_cwd = PathBuf::from(next_cwd.trim());
            state.cwd = Some(ensure_within_root(&next_cwd, &root, "replay 后")?);
        }
        if let Ok(raw_env) = std::fs::read(&env_dump) {
            state.env = parse_safe_environment(&raw_env);
        }
    }
    let _ = std::fs::remove_file(cwd_dump);
    let _ = std::fs::remove_file(env_dump);
    output
}

fn fs_base() -> Result<PathBuf> {
    let configured = crate::runtime::settings::local_config::get().tools.fs_base;
    let root = if configured.trim().is_empty() {
        dirs::home_dir().context("home directory unavailable")?
    } else {
        PathBuf::from(shellexpand::tilde(configured.trim()).as_ref())
    };
    root.canonicalize()
        .with_context(|| format!("tools.fsBase is not accessible: {}", root.display()))
}

fn ensure_within_root(path: &Path, root: &Path, phase: &str) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("shell cwd 不可访问（{phase}）：{}", path.display()))?;
    if !canonical.starts_with(root) {
        anyhow::bail!(
            "shell cwd 超出 tools.fsBase（{phase}）：{}",
            canonical.display()
        );
    }
    Ok(canonical)
}

fn create_private_file(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    Ok(())
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn parse_safe_environment(raw: &[u8]) -> HashMap<String, String> {
    let base = std::env::vars().collect::<HashMap<_, _>>();
    raw.split(|byte| *byte == 0)
        .filter_map(|entry| {
            let text = std::str::from_utf8(entry).ok()?;
            let (key, value) = text.split_once('=')?;
            (is_safe_env_name(key) && base.get(key).is_none_or(|base| base != value))
                .then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

fn is_safe_env_name(key: &str) -> bool {
    const SEMANTIC: &[&str] = &[
        "PATH",
        "CDPATH",
        "ENV",
        "BASH_ENV",
        "PROMPT_COMMAND",
        "PWD",
        "OLDPWD",
        "SHELL",
        "SHLVL",
        "IFS",
        "HOME",
        "USER",
        "LOGNAME",
        "_",
    ];
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        || SEMANTIC.contains(&key)
    {
        return false;
    }
    let upper = key.to_ascii_uppercase();
    ![
        "API_KEY",
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "PRIVATE_KEY",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_filter_keeps_only_safe_user_changes() {
        let base_path = std::env::var("PATH").unwrap_or_default();
        let raw = format!(
            "PWCLI_TEST_FLAG=visible\0PATH={base_path}/changed\0SERVICE_TOKEN=secret\0BASH_ENV=/tmp/x\0"
        );
        let parsed = parse_safe_environment(raw.as_bytes());
        assert_eq!(
            parsed.get("PWCLI_TEST_FLAG").map(String::as_str),
            Some("visible")
        );
        assert!(!parsed.contains_key("PATH"));
        assert!(!parsed.contains_key("SERVICE_TOKEN"));
        assert!(!parsed.contains_key("BASH_ENV"));
    }

    #[tokio::test]
    async fn cwd_and_safe_export_persist_within_one_session() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let session_id = format!("shell-test-{}", uuid::Uuid::now_v7());
        let sandbox = SandboxConfig::default();
        execute_with_root(
            &session_id,
            "cd sub && export PWCLI_TEST_FLAG=visible".into(),
            Some(dir.path().to_string_lossy().into_owned()),
            Some(10),
            &sandbox,
            dir.path(),
            dir.path(),
        )
        .await
        .unwrap();
        let output = execute_with_root(
            &session_id,
            "pwd; printf '%s' \"$PWCLI_TEST_FLAG\"".into(),
            None,
            Some(10),
            &sandbox,
            dir.path(),
            dir.path(),
        )
        .await
        .unwrap();
        assert!(output
            .stdout
            .contains(&dir.path().join("sub").to_string_lossy().to_string()));
        assert!(output.stdout.contains("visible"));
        clear(&session_id);
    }

    #[tokio::test]
    async fn fresh_session_defaults_inside_fs_base() {
        let root = tempfile::tempdir().unwrap();
        let session_id = format!("shell-default-cwd-test-{}", uuid::Uuid::now_v7());
        let sandbox = SandboxConfig::default();
        let output = execute_with_root(
            &session_id,
            "pwd -P".into(),
            None,
            Some(10),
            &sandbox,
            root.path(),
            root.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            output.stdout.trim(),
            root.path().canonicalize().unwrap().to_string_lossy()
        );
        clear(&session_id);
    }
}
