use anyhow::Context;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const MAX_BASH_ARTIFACT_BYTES: usize = 10 * 1024 * 1024;
/// 无显式超时时的硬上限，防止 runaway 命令（30 分钟）。
const HARD_TIMEOUT_SECS: u64 = 30 * 60;

pub mod sandbox;
pub mod shell_state;
pub mod validation;

pub use sandbox::*;
pub use validation::*;

/// Bash 命令输入
#[derive(Debug, Clone)]
pub struct BashCommandInput {
    pub command: String,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    /// `None` 表示不限时（仍有 [`HARD_TIMEOUT_SECS`] 硬上限防 runaway）。
    pub timeout_secs: Option<u64>,
    /// 会话 ID，用于登记 spawn 的子进程 pid，便于后续清理。
    pub session_id: Option<String>,
}

impl BashCommandInput {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            env: Vec::new(),
            timeout_secs: None,
            session_id: None,
        }
    }

    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// Bash 命令输出
#[derive(Debug, Clone)]
pub struct BashCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// 实际生效的超时秒数（含硬上限收敛）。
    pub effective_timeout_secs: Option<u64>,
}

impl BashCommandOutput {
    pub fn is_success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    pub fn combined_output(&self) -> String {
        let mut out = String::new();
        if !self.stdout.is_empty() {
            out.push_str(&self.stdout);
        }
        if !self.stderr.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("[stderr]\n");
            out.push_str(&self.stderr);
        }
        out
    }
}

// ── 后台进程跟踪 ──
//
// 参考 pi 的 trackDetachedChildPid：把每次 spawn 的直接子进程 pid 登记到
// 会话级表中（登记前清理已退出条目），便于会话结束或 daemon 关闭时统一回收。
mod child_tracking {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Mutex, OnceLock};

    fn table() -> &'static Mutex<HashMap<String, HashSet<u32>>> {
        static TABLE: OnceLock<Mutex<HashMap<String, HashSet<u32>>>> = OnceLock::new();
        TABLE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn track(session_id: &str, pid: u32) {
        let mut table = table().lock().unwrap_or_else(|error| error.into_inner());
        let pids = table.entry(session_id.to_string()).or_default();
        pids.retain(|pid| process_alive(*pid));
        pids.insert(pid);
    }

    pub fn pids_for(session_id: &str) -> Vec<u32> {
        let mut table = table().lock().unwrap_or_else(|error| error.into_inner());
        let Some(pids) = table.get_mut(session_id) else {
            return Vec::new();
        };
        pids.retain(|pid| process_alive(*pid));
        pids.iter().copied().collect()
    }

    /// 杀掉会话登记的全部进程组并清空记录（幂等）。
    pub fn kill_tracked(session_id: &str) {
        for pid in std::mem::take(&mut pids_for(session_id)) {
            kill_process_group(pid);
        }
        table()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(session_id);
    }

    fn process_alive(pid: u32) -> bool {
        #[cfg(unix)]
        {
            unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    pub(super) fn kill_process_group(pid: u32) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
        }
    }
}

pub use child_tracking::{kill_tracked, pids_for, track};

/// 执行 bash 命令（带沙箱、进程组隔离与可选超时）
pub async fn execute_bash(
    input: BashCommandInput,
    sandbox: &SandboxConfig,
) -> anyhow::Result<BashCommandOutput> {
    // 验证命令安全性
    let validation = BashValidation::default();
    validation.validate(&input.command)?;

    // 沙箱路径检查
    if let Some(ref cwd) = input.cwd {
        sandbox.validate_path(cwd)?;
    }

    let start = std::time::Instant::now();

    // 用 sh -c 跑命令：daemon 继承启动它的 shell 环境，子进程继续继承
    // PATH 和显式 export 的变量。
    //
    // 不在每次执行时再用 zsh -ic：oh-my-zsh autoload 单次启动 ~200ms+，
    // AI 一轮 30 个 bash 直接被 timeout 拖死。
    // 代价：alias / function 这种只在 interactive shell 才生效的特性
    // 不可用 —— 用绝对路径或 `~/.../script.py` 即可。
    let worker_policy = crate::runtime::tools::fs_local::worker_execution_policy();
    let worker_cwd = worker_policy
        .map(|policy| resolve_worker_cwd(input.cwd.as_deref(), policy.root()))
        .transpose()?;
    let mut cmd = worker_shell_command(&input.command, worker_policy)?;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // 进程组隔离：子进程成为新进程组 leader，超时/取消时可按组 kill，
    // 确保 sh -c 派生的整个进程树被清理。
    #[cfg(unix)]
    cmd.process_group(0);

    // 设置工作目录
    if let Some(cwd) = worker_cwd.as_ref() {
        cmd.current_dir(cwd);
    } else if let Some(ref cwd) = input.cwd {
        cmd.current_dir(cwd);
    }

    // 设置环境变量（过滤敏感变量）
    for (key, value) in &input.env {
        if !sandbox.is_sensitive_env(key) {
            cmd.env(key, value);
        }
    }

    // 清除敏感环境变量
    for key in &sandbox.sensitive_env_vars {
        cmd.env_remove(key);
    }

    let mut child = cmd.spawn()?;

    // 登记直接子进程 pid，便于会话级清理后台/长跑进程
    if let (Some(session_id), Some(pid)) = (input.session_id.as_deref(), child.id()) {
        child_tracking::track(session_id, pid);
    }

    let effective_timeout_secs = input.timeout_secs.map(|secs| secs.min(HARD_TIMEOUT_SECS));
    let result = timeout(
        Duration::from_secs(effective_timeout_secs.unwrap_or(HARD_TIMEOUT_SECS)),
        child.wait(),
    )
    .await;

    let (exit_code, timed_out) = match result {
        Ok(Ok(status)) => (status.code(), false),
        Ok(Err(e)) => return Err(anyhow::anyhow!("进程错误: {}", e)),
        Err(_) => {
            // 杀掉整个进程组，而不仅是直接子进程
            if let Some(pid) = child.id() {
                child_tracking::kill_process_group(pid);
            }
            let _ = child.start_kill();
            (None, true)
        }
    };

    // 读取输出（即使超时也尝试读取已产生的输出）
    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(mut out) = child.stdout.take() {
        use tokio::io::AsyncReadExt;
        let _ = tokio::time::timeout(Duration::from_secs(1), out.read_to_string(&mut stdout)).await;
    }
    if let Some(mut err) = child.stderr.take() {
        use tokio::io::AsyncReadExt;
        let _ = tokio::time::timeout(Duration::from_secs(1), err.read_to_string(&mut stderr)).await;
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    let artifact_path = if stdout.len().saturating_add(stderr.len()) > sandbox.max_output_bytes {
        persist_large_output(&stdout, &stderr).ok()
    } else {
        None
    };

    // 截断过长的输出（按 char 边界，避免 UTF-8 中间切断 panic）
    let max_len = sandbox.max_output_bytes;
    let mut stdout = if stdout.len() > max_len {
        let mut end = max_len;
        while end > 0 && !stdout.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}\n[输出已截断，共 {} bytes]",
            &stdout[..end],
            stdout.len()
        )
    } else {
        stdout
    };
    let stderr = if stderr.len() > max_len {
        let mut end = max_len;
        while end > 0 && !stderr.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}\n[错误输出已截断，共 {} bytes]",
            &stderr[..end],
            stderr.len()
        )
    } else {
        stderr
    };

    if let Some(path) = artifact_path {
        stdout.push_str(&format!(
            "\n[完整输出已落盘：{}；单文件上限 10 MiB]",
            path.display()
        ));
    }

    Ok(BashCommandOutput {
        stdout,
        stderr,
        exit_code,
        timed_out,
        duration_ms,
        effective_timeout_secs,
    })
}

fn resolve_worker_cwd(
    input: Option<&str>,
    root: &std::path::Path,
) -> anyhow::Result<std::path::PathBuf> {
    let root = root.canonicalize().with_context(|| {
        format!(
            "worker execution root is not accessible: {}",
            root.display()
        )
    })?;
    let candidate = input.map_or_else(
        || root.clone(),
        |value| {
            let expanded = std::path::PathBuf::from(shellexpand::tilde(value).as_ref());
            if expanded.is_absolute() {
                expanded
            } else {
                root.join(expanded)
            }
        },
    );
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("shell cwd is not accessible: {}", candidate.display()))?;
    if !canonical.starts_with(&root) {
        anyhow::bail!(
            "shell cwd is outside worker execution root: {}",
            canonical.display()
        );
    }
    if !canonical.is_dir() {
        anyhow::bail!("shell cwd is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

fn worker_shell_command(
    command: &str,
    policy: Option<&crate::runtime::tools::fs_local::WorkerExecutionPolicy>,
) -> anyhow::Result<Command> {
    crate::runtime::tools::fs_local::worker_subprocess_command(
        "/bin/sh",
        &["-c".to_string(), command.to_string()],
        policy,
        true,
    )
}

fn persist_large_output(stdout: &str, stderr: &str) -> anyhow::Result<std::path::PathBuf> {
    use std::io::Write;

    let dir = crate::runtime::settings::local_config::data_dir().join("artifacts/bash");
    std::fs::create_dir_all(&dir)?;
    cleanup_bash_artifacts(&dir);
    let path = dir.join(format!("{}.log", uuid::Uuid::now_v7().simple()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    let body = format!("[stdout]\n{stdout}\n[stderr]\n{stderr}");
    let bytes = body.as_bytes();
    file.write_all(&bytes[..bytes.len().min(MAX_BASH_ARTIFACT_BYTES)])?;
    if bytes.len() > MAX_BASH_ARTIFACT_BYTES {
        file.write_all(b"\n[artifact truncated at 10 MiB]\n")?;
    }
    Ok(path)
}

fn cleanup_bash_artifacts(dir: &std::path::Path) {
    let now = std::time::SystemTime::now();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let age = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok());
        if path.extension().and_then(|value| value.to_str()) == Some("log")
            && age.is_some_and(|age| age.as_secs() > 7 * 24 * 60 * 60)
        {
            let tombstone = path.with_extension("tombstone.json");
            let record = serde_json::json!({
                "removedAt": chrono::Utc::now(),
                "originalPath": path.file_name().and_then(|value| value.to_str()),
                "reason": "retention_expired"
            });
            if std::fs::write(&tombstone, format!("{record}\n")).is_ok() {
                let _ = std::fs::remove_file(path);
            }
        } else if path.to_string_lossy().ends_with(".tombstone.json")
            && age.is_some_and(|age| age.as_secs() > 30 * 24 * 60 * 60)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_echo() {
        let input = BashCommandInput::new("echo hello");
        let sandbox = SandboxConfig::default();
        let output = execute_bash(input, &sandbox).await.unwrap();
        assert!(output.is_success());
        assert!(output.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let input = BashCommandInput::new("sleep 5").with_timeout(1);
        let sandbox = SandboxConfig::default();
        let output = execute_bash(input, &sandbox).await.unwrap();
        assert!(output.timed_out);
    }

    #[tokio::test]
    async fn test_execute_with_cwd() {
        let input = BashCommandInput::new("pwd").with_cwd("/tmp");
        let sandbox = SandboxConfig::default();
        let output = execute_bash(input, &sandbox).await.unwrap();
        assert!(output.stdout.contains("tmp"));
    }

    #[tokio::test]
    async fn test_combined_output() {
        let output = BashCommandOutput {
            stdout: "out".to_string(),
            stderr: "err".to_string(),
            exit_code: Some(0),
            timed_out: false,
            duration_ms: 10,
            effective_timeout_secs: None,
        };
        let combined = output.combined_output();
        assert!(combined.contains("out"));
        assert!(combined.contains("err"));
    }

    #[tokio::test]
    async fn timeout_defaults_to_unbounded_with_hard_cap() {
        let input = BashCommandInput::new("true");
        assert_eq!(input.timeout_secs, None);
        let sandbox = SandboxConfig::default();
        let output = execute_bash(input, &sandbox).await.unwrap();
        assert!(output.is_success());
        assert_eq!(output.effective_timeout_secs, None);
    }

    #[tokio::test]
    async fn explicit_timeout_is_capped_by_hard_limit() {
        let input = BashCommandInput::new("true").with_timeout(HARD_TIMEOUT_SECS + 600);
        let sandbox = SandboxConfig::default();
        let output = execute_bash(input, &sandbox).await.unwrap();
        assert_eq!(output.effective_timeout_secs, Some(HARD_TIMEOUT_SECS));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_the_entire_process_group() {
        let marker = tempfile::tempdir().unwrap();
        let marker_file = marker.path().join("child.pid");
        // sh -c 派生一个后台子进程；超时后整个进程组都应被 SIGKILL
        let command = format!(
            "sleep 30 & echo $! > '{}' && wait",
            marker_file.to_string_lossy().replace('\'', "'\\''")
        );
        let input = BashCommandInput::new(command).with_timeout(1);
        let sandbox = SandboxConfig::default();
        let output = execute_bash(input, &sandbox).await.unwrap();
        assert!(output.timed_out);
        // 等待片刻让 SIGKILL 生效，再确认派生的 sleep 已不在运行
        tokio::time::sleep(Duration::from_millis(200)).await;
        let child_pid: i32 = std::fs::read_to_string(&marker_file)
            .expect("child pid marker should exist")
            .trim()
            .parse()
            .expect("child pid marker should be numeric");
        let alive = unsafe { libc::kill(child_pid as libc::pid_t, 0) == 0 };
        assert!(!alive, "detached child process should be killed with the group");
    }

    #[tokio::test]
    async fn spawned_children_are_tracked_per_session() {
        let session_id = format!("bash-track-{}", uuid::Uuid::now_v7());
        let input = BashCommandInput::new("true").with_session_id(session_id.clone());
        let sandbox = SandboxConfig::default();
        execute_bash(input, &sandbox).await.unwrap();
        // 进程已退出，登记表应在下次访问时自动清理
        assert!(child_tracking::pids_for(&session_id).is_empty());
        child_tracking::kill_tracked(&session_id);
    }

    #[test]
    fn worker_cwd_rejects_parent_and_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert!(resolve_worker_cwd(Some(outside.path().to_str().unwrap()), root.path()).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
            assert!(resolve_worker_cwd(Some("escape"), root.path()).is_err());
        }
        assert_eq!(
            resolve_worker_cwd(None, root.path()).unwrap(),
            root.path().canonicalize().unwrap()
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn native_worker_shell_blocks_write_escape() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("escaped.txt");
        let policy = crate::runtime::tools::fs_local::WorkerExecutionPolicy::new(
            root.path(),
            data_dir.path(),
            true,
        )
        .unwrap();
        let command = format!(
            "printf allowed > inside.txt && printf blocked > '{}'",
            outside_file.to_string_lossy().replace('\'', "'\\''")
        );
        let mut child = worker_shell_command(&command, Some(&policy)).unwrap();
        let output = child.current_dir(root.path()).output().await.unwrap();
        if output.status.code() == Some(71)
            && String::from_utf8_lossy(&output.stderr)
                .contains("sandbox_apply: Operation not permitted")
        {
            assert!(!outside_file.exists());
            return;
        }
        assert!(!output.status.success());
        assert!(!outside_file.exists());
        assert!(root.path().join("inside.txt").exists());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn worker_shell_fails_closed_without_native_sandbox() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let policy = crate::runtime::tools::fs_local::WorkerExecutionPolicy::new(
            root.path(),
            data_dir.path(),
            true,
        )
        .unwrap();
        assert!(worker_shell_command("true", Some(&policy)).is_err());
    }
}
