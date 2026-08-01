use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::config::{local_config, RuntimeConfig};
use crate::tools::register::{register_all_tools, register_memory_tools};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const LOG_LIMIT_BYTES: u64 = 10 * 1024 * 1024;
const LOG_ROTATIONS: usize = 3;

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Start the daemon in the background. This command is idempotent.
    Start,
    /// Ask the daemon to stop gracefully.
    Stop,
    /// Stop, then start the daemon.
    Restart,
    /// Show daemon state.
    Status(StatusArgs),
    /// Show daemon logs.
    Logs(LogsArgs),
    /// Run the daemon in the foreground.
    Run,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Number of trailing lines to print.
    #[arg(short = 'n', long, default_value_t = 100)]
    pub lines: usize,
    /// Continue printing appended log lines.
    #[arg(short = 'f', long)]
    pub follow: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct DaemonPaths {
    pub data_dir: PathBuf,
    pub run_dir: PathBuf,
    pub log_dir: PathBuf,
    pub lock_path: PathBuf,
    pub metadata_path: PathBuf,
    pub log_path: PathBuf,
}

impl DaemonPaths {
    pub fn current() -> Self {
        Self::new(local_config::data_dir())
    }

    pub fn new(data_dir: PathBuf) -> Self {
        let run_dir = data_dir.join("run");
        let log_dir = data_dir.join("logs");
        Self {
            lock_path: run_dir.join("daemon.lock"),
            metadata_path: run_dir.join("daemon.json"),
            log_path: log_dir.join("daemon.log"),
            data_dir,
            run_dir,
            log_dir,
        }
    }

    fn ensure(&self) -> Result<()> {
        create_private_dir(&self.data_dir)?;
        create_private_dir(&self.run_dir)?;
        create_private_dir(&self.log_dir)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetadata {
    pub pid: u32,
    pub instance_id: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub http_address: String,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub running: bool,
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    pub version: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub http_address: String,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub active_tasks: usize,
    pub active_acp_sessions: usize,
    pub healthy: bool,
}

#[derive(Debug, Clone)]
pub struct DaemonRuntime {
    pub metadata: RuntimeMetadata,
    pub shutdown: CancellationToken,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    config_file: Check,
    data_dir: Check,
    daemon: Check,
    acp_clis: Vec<Check>,
}

#[derive(Debug, Serialize)]
struct Check {
    name: String,
    ok: bool,
    detail: String,
}

pub async fn execute(command: DaemonCommand) -> Result<()> {
    match command {
        DaemonCommand::Start => start().await,
        DaemonCommand::Stop => stop().await,
        DaemonCommand::Restart => {
            stop().await?;
            start().await
        }
        DaemonCommand::Status(args) => print_status(args.json).await,
        DaemonCommand::Logs(args) => logs(args).await,
        DaemonCommand::Run => run().await,
    }
}

pub async fn start() -> Result<()> {
    let current = status().await;
    if current.running && current.healthy {
        println!(
            "pwcli daemon is already running at {}",
            current.http_address
        );
        return Ok(());
    }

    let paths = DaemonPaths::current();
    paths.ensure()?;
    rotate_log_if_needed(&paths.log_path)?;
    let stdout = append_log(&paths.log_path)?;
    let stderr = stdout.try_clone()?;
    let executable = std::env::current_exe().context("resolve current pwcli executable")?;
    let mut command = Command::new(executable);
    command
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    detach(&mut command);
    let mut child = command.spawn().context("start pwcli daemon")?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let current = status().await;
        if current.running && current.healthy {
            println!(
                "pwcli daemon started (pid {}) at {}",
                current.pid.unwrap_or_default(),
                current.http_address
            );
            return Ok(());
        }
        if let Some(exit) = child.try_wait().context("inspect pwcli daemon process")? {
            bail!(
                "daemon process {} exited with {exit}; inspect {}",
                child.id(),
                paths.log_path.display()
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "daemon process {} did not become healthy within {}s; inspect {}",
                child.id(),
                START_TIMEOUT.as_secs(),
                paths.log_path.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn stop() -> Result<()> {
    let paths = DaemonPaths::current();
    paths.ensure()?;
    let Some(metadata) = read_metadata(&paths.metadata_path) else {
        if let Some(status) = discover_running_daemon(&paths).await {
            return stop_discovered_daemon(&paths, &status).await;
        }
        println!("pwcli daemon is already stopped");
        cleanup_stale_metadata(&paths)?;
        return Ok(());
    };
    if !probe(&metadata).await {
        cleanup_stale_metadata(&paths)?;
        println!("pwcli daemon is already stopped (removed stale runtime metadata)");
        return Ok(());
    }

    let url = format!("{}/daemon/shutdown", metadata.http_address);
    let response = reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({ "instanceId": metadata.instance_id }))
        .send()
        .await
        .context("request daemon shutdown")?;
    if !response.status().is_success() {
        bail!(
            "daemon rejected shutdown request with {}",
            response.status()
        );
    }

    let deadline = Instant::now() + STOP_TIMEOUT;
    let mut http_stopped_at: Option<Instant> = None;
    let mut term_sent = false;
    while Instant::now() < deadline {
        if !probe(&metadata).await {
            if !process_alive(metadata.pid) {
                cleanup_stale_metadata(&paths)?;
                println!("pwcli daemon stopped");
                return Ok(());
            }
            let stopped_at = *http_stopped_at.get_or_insert_with(Instant::now);
            if !term_sent && stopped_at.elapsed() >= Duration::from_secs(2) {
                let _ = Command::new("kill")
                    .args(["-TERM", &metadata.pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                term_sent = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!(
        "daemon process {} did not exit within {}s",
        metadata.pid,
        STOP_TIMEOUT.as_secs()
    )
}

fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| proc_stat_state(&stat))
        == Some('Z')
    {
        return false;
    }

    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(any(target_os = "linux", test))]
fn proc_stat_state(stat: &str) -> Option<char> {
    let (_, fields) = stat.rsplit_once(") ")?;
    fields.chars().next()
}

pub async fn run() -> Result<()> {
    let paths = DaemonPaths::current();
    paths.ensure()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.lock_path)
        .with_context(|| format!("open daemon lock {}", paths.lock_path.display()))?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("pwcli daemon is already running"))?;

    rotate_log_if_needed(&paths.log_path)?;
    crate::logging::init_logging_in(&paths.log_dir);
    let port = local_config::get().server.backend_port;
    let metadata = RuntimeMetadata {
        pid: std::process::id(),
        instance_id: uuid::Uuid::now_v7().to_string(),
        version: env!("PWCLI_VERSION_INFO").to_string(),
        started_at: Utc::now(),
        http_address: format!("http://127.0.0.1:{port}"),
        data_dir: paths.data_dir.clone(),
        config_file: config_file(),
    };
    write_metadata(&paths.metadata_path, &metadata)?;

    let shutdown = CancellationToken::new();
    let runtime = DaemonRuntime {
        metadata: metadata.clone(),
        shutdown: shutdown.clone(),
    };
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_shutdown.cancel();
    });

    let config = RuntimeConfig::load();
    let user_slug = config
        .user
        .as_ref()
        .and_then(|user| user.slug.clone())
        .unwrap_or_else(|| "local".to_string());
    let result =
        crate::service::run_daemon_server(config, port, runtime, move |registry, backend| {
            register_all_tools(registry, backend);
            register_memory_tools(registry, user_slug);
        })
        .await;
    let _ = fs::remove_file(&paths.metadata_path);
    let _ = FileExt::unlock(&lock);
    result
}

pub async fn status() -> DaemonStatus {
    let paths = DaemonPaths::current();
    let config_file = config_file();
    let fallback_address = format!(
        "http://127.0.0.1:{}",
        local_config::get().server.backend_port
    );
    let metadata = read_metadata(&paths.metadata_path);
    let address = metadata
        .as_ref()
        .map(|value| value.http_address.clone())
        .unwrap_or(fallback_address);
    let remote_status = match &metadata {
        Some(metadata) => fetch_status(metadata).await,
        None => discover_running_daemon(&paths).await,
    };
    let healthy = remote_status.is_some();
    if let Some(status) = remote_status {
        return status;
    }
    DaemonStatus {
        running: healthy,
        pid: metadata.as_ref().filter(|_| healthy).map(|value| value.pid),
        instance_id: metadata
            .as_ref()
            .filter(|_| healthy)
            .map(|value| value.instance_id.clone()),
        version: metadata
            .as_ref()
            .filter(|_| healthy)
            .map(|value| value.version.clone()),
        started_at: metadata
            .as_ref()
            .filter(|_| healthy)
            .map(|value| value.started_at),
        http_address: address,
        data_dir: paths.data_dir,
        config_file,
        active_tasks: 0,
        active_acp_sessions: 0,
        healthy,
    }
}

pub async fn print_status(json: bool) -> Result<()> {
    let status = status().await;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    println!(
        "status: {}",
        if status.running { "running" } else { "stopped" }
    );
    if let Some(pid) = status.pid {
        println!("pid: {pid}");
    }
    if let Some(version) = status.version {
        println!("version: {version}");
    }
    if let Some(started_at) = status.started_at {
        println!("started: {}", started_at.to_rfc3339());
    }
    println!("http: {}", status.http_address);
    println!("data: {}", status.data_dir.display());
    println!("config: {}", status.config_file.display());
    println!("active tasks: {}", status.active_tasks);
    println!("active ACP sessions: {}", status.active_acp_sessions);
    println!(
        "health: {}",
        if status.healthy { "ok" } else { "unavailable" }
    );
    Ok(())
}

pub async fn web(no_open: bool) -> Result<()> {
    start().await?;
    let current = status().await;
    let url = current.http_address;
    println!("{url}");
    if !no_open {
        open_browser(&url)?;
    }
    Ok(())
}

pub async fn doctor(args: DoctorArgs) -> Result<()> {
    let paths = DaemonPaths::current();
    let current = status().await;
    let mut acp_clis = Vec::new();
    for (name, candidates) in [
        ("Codex ACP", &["codex-acp"] as &[&str]),
        ("Qoder CLI", &["qodercli", "qoder"] as &[&str]),
        ("Kimi Code", &["kimi"] as &[&str]),
    ] {
        let found = candidates
            .iter()
            .find_map(|candidate| find_executable(candidate));
        acp_clis.push(Check {
            name: name.to_string(),
            ok: found.is_some(),
            detail: found
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "not found in PATH".to_string()),
        });
    }
    if find_executable("codex-acp").is_none() {
        if let Some(codex) = find_executable("codex") {
            if let Some(check) = acp_clis.first_mut() {
                check.detail = format!(
                    "{} is installed, but no codex-acp adapter was discovered",
                    codex.display()
                );
            }
        }
    }
    let report = DoctorReport {
        config_file: Check {
            name: "config".to_string(),
            ok: config_file().is_file(),
            detail: config_file().display().to_string(),
        },
        data_dir: Check {
            name: "data directory".to_string(),
            ok: paths.data_dir.is_dir() || paths.ensure().is_ok(),
            detail: paths.data_dir.display().to_string(),
        },
        daemon: Check {
            name: "daemon".to_string(),
            ok: current.healthy,
            detail: format!(
                "{} ({})",
                current.http_address,
                if current.healthy {
                    "healthy"
                } else {
                    "stopped"
                }
            ),
        },
        acp_clis,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for check in std::iter::once(&report.config_file)
            .chain(std::iter::once(&report.data_dir))
            .chain(std::iter::once(&report.daemon))
            .chain(report.acp_clis.iter())
        {
            println!(
                "{} {:<14} {}",
                if check.ok { "ok" } else { "!!" },
                check.name,
                check.detail
            );
        }
    }
    Ok(())
}

async fn logs(args: LogsArgs) -> Result<()> {
    let path = DaemonPaths::current().log_path;
    if !path.exists() {
        bail!("daemon log does not exist: {}", path.display());
    }
    for line in tail_lines(&path, args.lines)? {
        println!("{line}");
    }
    if !args.follow {
        return Ok(());
    }
    let mut offset = fs::metadata(&path)?.len();
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let length = fs::metadata(&path)
            .map(|meta| meta.len())
            .unwrap_or_default();
        if length < offset {
            offset = 0;
        }
        if length == offset {
            continue;
        }
        let mut file = File::open(&path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut appended = String::new();
        file.read_to_string(&mut appended)?;
        print!("{appended}");
        std::io::stdout().flush()?;
        offset = length;
    }
}

async fn probe(metadata: &RuntimeMetadata) -> bool {
    fetch_status(metadata).await.is_some()
}

async fn fetch_status(metadata: &RuntimeMetadata) -> Option<DaemonStatus> {
    fetch_status_at(&metadata.http_address)
        .await
        .filter(|status| status.pid == Some(metadata.pid) && status.healthy)
}

async fn fetch_status_at(address: &str) -> Option<DaemonStatus> {
    let response = match reqwest::Client::new()
        .get(format!("{address}/daemon/status"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        _ => return None,
    };
    response.json::<DaemonStatus>().await.ok()
}

async fn discover_running_daemon(paths: &DaemonPaths) -> Option<DaemonStatus> {
    let address = format!(
        "http://127.0.0.1:{}",
        local_config::get().server.backend_port
    );
    fetch_status_at(&address)
        .await
        .filter(|status| status_matches_runtime(status, paths))
}

fn status_matches_runtime(status: &DaemonStatus, paths: &DaemonPaths) -> bool {
    status.running
        && status.healthy
        && status.pid.is_some()
        && status.data_dir == paths.data_dir
        && status.config_file == config_file()
}

async fn stop_discovered_daemon(paths: &DaemonPaths, status: &DaemonStatus) -> Result<()> {
    let pid = status
        .pid
        .context("discovered daemon did not report a pid")?;
    if let Some(instance_id) = status.instance_id.as_deref() {
        let response = reqwest::Client::new()
            .post(format!("{}/daemon/shutdown", status.http_address))
            .json(&serde_json::json!({ "instanceId": instance_id }))
            .send()
            .await
            .context("request discovered daemon shutdown")?;
        if !response.status().is_success() {
            bail!(
                "discovered daemon rejected shutdown request with {}",
                response.status()
            );
        }
    } else {
        let command = process_command(pid)
            .with_context(|| format!("inspect discovered daemon process {pid}"))?;
        if !is_pwcli_daemon_command(&command) {
            bail!(
                "refusing to stop pid {pid}: process command is not `pwcli daemon run` ({command})"
            );
        }
        send_term(pid)?;
    }

    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            cleanup_stale_metadata(paths)?;
            println!("pwcli daemon stopped");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!(
        "daemon process {pid} did not exit within {}s",
        STOP_TIMEOUT.as_secs()
    )
}

fn process_command(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|command| !command.is_empty())
}

fn is_pwcli_daemon_command(command: &str) -> bool {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let Some(executable) = parts.first() else {
        return false;
    };
    let is_pwcli = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "pwcli");
    is_pwcli
        && parts
            .windows(2)
            .any(|arguments| arguments == ["daemon", "run"])
}

fn send_term(pid: u32) -> Result<()> {
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("send TERM to daemon")?;
    if !status.success() {
        bail!("failed to send TERM to daemon process {pid}");
    }
    Ok(())
}

fn read_metadata(path: &Path) -> Option<RuntimeMetadata> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_metadata(path: &Path, metadata: &RuntimeMetadata) -> Result<()> {
    let parent = path.parent().context("daemon metadata has no parent")?;
    create_private_dir(parent)?;
    let temporary = parent.join(format!(".daemon-{}.tmp", metadata.instance_id));
    fs::write(&temporary, serde_json::to_vec_pretty(metadata)?)?;
    set_private_file(&temporary)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn cleanup_stale_metadata(paths: &DaemonPaths) -> Result<()> {
    paths.ensure()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.lock_path)?;
    if lock.try_lock_exclusive().is_ok() {
        if paths.metadata_path.exists() {
            fs::remove_file(&paths.metadata_path)?;
        }
        let _ = FileExt::unlock(&lock);
    }
    Ok(())
}

fn rotate_log_if_needed(path: &Path) -> Result<()> {
    let size = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    if size < LOG_LIMIT_BYTES {
        return Ok(());
    }
    for index in (1..=LOG_ROTATIONS).rev() {
        let source = if index == 1 {
            path.to_path_buf()
        } else {
            PathBuf::from(format!("{}.{}", path.display(), index - 1))
        };
        let target = PathBuf::from(format!("{}.{}", path.display(), index));
        if source.exists() {
            if target.exists() {
                fs::remove_file(&target)?;
            }
            fs::rename(source, target)?;
        }
    }
    Ok(())
}

fn append_log(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    set_private_file(path)?;
    Ok(file)
}

fn tail_lines(path: &Path, count: usize) -> Result<Vec<String>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let lines = BufReader::new(File::open(path)?)
        .lines()
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(lines
        .into_iter()
        .rev()
        .take(count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect())
}

fn config_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pwcli/config.json")
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let status = Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let status = Command::new("cmd").args(["/C", "start", "", url]).status();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let status: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opening a browser is unsupported on this target",
    ));
    let status = status.context("open system browser")?;
    if !status.success() {
        bail!("system browser command exited with {status}");
    }
    Ok(())
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(executable_name(name)))
        .find(|candidate| candidate.is_file())
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_below_data_dir() {
        let paths = DaemonPaths::new(PathBuf::from("/tmp/pwcli-test-data"));
        assert_eq!(paths.run_dir, paths.data_dir.join("run"));
        assert_eq!(paths.log_dir, paths.data_dir.join("logs"));
        assert_eq!(paths.metadata_path, paths.run_dir.join("daemon.json"));
    }

    #[test]
    fn parses_linux_proc_state_with_parentheses_in_process_name() {
        assert_eq!(
            proc_stat_state("651 (pwcli daemon (release)) Z 1 651 651 0"),
            Some('Z')
        );
        assert_eq!(proc_stat_state("42 (pwcli) S 1 42 42 0"), Some('S'));
        assert_eq!(proc_stat_state("invalid"), None);
    }

    #[test]
    fn recognizes_only_pwcli_daemon_run_commands() {
        assert!(is_pwcli_daemon_command(
            "/Users/example/.local/bin/pwcli daemon run"
        ));
        assert!(is_pwcli_daemon_command("pwcli daemon run"));
        assert!(!is_pwcli_daemon_command("pwcli daemon status"));
        assert!(!is_pwcli_daemon_command("other daemon run"));
    }

    #[test]
    fn stale_metadata_cleanup_creates_missing_runtime_directory() {
        let directory = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::new(directory.path().join("data"));
        cleanup_stale_metadata(&paths).unwrap();
        assert!(paths.run_dir.is_dir());
    }

    #[test]
    fn metadata_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("run/daemon.json");
        let metadata = RuntimeMetadata {
            pid: 42,
            instance_id: "instance".to_string(),
            version: "1.0.0".to_string(),
            started_at: Utc::now(),
            http_address: "http://127.0.0.1:3456".to_string(),
            data_dir: directory.path().to_path_buf(),
            config_file: directory.path().join("config.json"),
        };
        write_metadata(&path, &metadata).unwrap();
        let loaded = read_metadata(&path).unwrap();
        assert_eq!(loaded.pid, 42);
        assert_eq!(loaded.instance_id, "instance");
    }

    #[test]
    fn lock_rejects_a_second_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("daemon.lock");
        let first = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let second = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        first.try_lock_exclusive().unwrap();
        assert!(second.try_lock_exclusive().is_err());
    }

    #[test]
    fn tail_is_bounded_and_ordered() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("daemon.log");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail_lines(&path, 2).unwrap(), vec!["two", "three"]);
    }
}
