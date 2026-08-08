use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

const MATPLOTLIB_REQUIREMENT: &str = "matplotlib>=3.8,<4";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
static ENVIRONMENT: OnceCell<std::result::Result<PythonEnvironment, String>> =
    OnceCell::const_new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonEnvironment {
    pub executable: String,
    pub python_version: String,
    pub matplotlib_version: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonEnvironmentStatus {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<PythonEnvironment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub requirement: String,
    pub updated_at: String,
}

/// Start installation without delaying daemon readiness. A plot requested
/// while setup is running awaits this same process-wide OnceCell.
pub fn start_background_bootstrap() {
    if bootstrap_disabled() {
        return;
    }
    tokio::spawn(async {
        match ensure_ready().await {
            Ok(environment) => tracing::info!(
                python = %environment.executable,
                python_version = %environment.python_version,
                matplotlib_version = %environment.matplotlib_version,
                source = %environment.source,
                "illustration Python environment is ready"
            ),
            Err(error) => tracing::warn!(%error, "illustration Python bootstrap failed"),
        }
    });
}

pub async fn ensure_ready() -> Result<PythonEnvironment> {
    let result = ENVIRONMENT
        .get_or_init(|| async {
            match tokio::task::spawn_blocking(bootstrap_sync).await {
                Ok(Ok(environment)) => Ok(environment),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(format!("Python bootstrap worker stopped: {error}")),
            }
        })
        .await;
    result.clone().map_err(anyhow::Error::msg)
}

pub fn status() -> PythonEnvironmentStatus {
    let path = status_path();
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| PythonEnvironmentStatus {
            state: if ENVIRONMENT.initialized() {
                "checking".into()
            } else {
                "not_checked".into()
            },
            environment: None,
            error: None,
            requirement: MATPLOTLIB_REQUIREMENT.into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
}

fn bootstrap_sync() -> Result<PythonEnvironment> {
    let root = environment_root();
    fs::create_dir_all(&root)?;
    write_status(&PythonEnvironmentStatus {
        state: "checking".into(),
        environment: None,
        error: None,
        requirement: MATPLOTLIB_REQUIREMENT.into(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    })?;

    let lock_path = root.join("bootstrap.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.lock_exclusive()?;

    let result = bootstrap_locked(&root);
    let status = match &result {
        Ok(environment) => PythonEnvironmentStatus {
            state: "ready".into(),
            environment: Some(environment.clone()),
            error: None,
            requirement: MATPLOTLIB_REQUIREMENT.into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
        Err(error) => PythonEnvironmentStatus {
            state: "failed".into(),
            environment: None,
            error: Some(error.to_string()),
            requirement: MATPLOTLIB_REQUIREMENT.into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    };
    let _ = write_status(&status);
    let _ = FileExt::unlock(&lock);
    result
}

fn bootstrap_locked(root: &Path) -> Result<PythonEnvironment> {
    let mut candidates = vec![venv_python(&root.join("venv"))];
    candidates.extend(python_candidates());
    for candidate in &candidates {
        if let Ok(mut environment) = probe(candidate, true) {
            environment.source = "existing-environment".into();
            return Ok(environment);
        }
    }

    let base = candidates
        .iter()
        .find(|candidate| probe(candidate, false).is_ok())
        .context("未找到 Python 3.9+；请安装 Python，或设置 PWCLI_ILLUSTRATION_PYTHON")?;
    let venv = root.join("venv");
    write_status(&PythonEnvironmentStatus {
        state: "installing".into(),
        environment: None,
        error: None,
        requirement: MATPLOTLIB_REQUIREMENT.into(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    })?;
    let log = root.join("install.log");
    run_logged(
        Command::new(base)
            .args(["-m", "venv", "--clear"])
            .arg(&venv),
        &log,
        INSTALL_TIMEOUT,
        "create illustration Python virtual environment",
    )?;
    let python = venv_python(&venv);
    let pip_ready = Command::new(&python)
        .args(["-m", "pip", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !pip_ready {
        run_logged(
            Command::new(&python).args(["-m", "ensurepip", "--upgrade"]),
            &log,
            INSTALL_TIMEOUT,
            "bootstrap pip",
        )?;
    }
    run_logged(
        Command::new(&python).args([
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "--no-input",
            MATPLOTLIB_REQUIREMENT,
        ]),
        &log,
        INSTALL_TIMEOUT,
        "install matplotlib",
    )?;
    let mut environment = probe(&python, true).with_context(|| {
        format!(
            "matplotlib installation did not produce a usable environment; inspect {}",
            log.display()
        )
    })?;
    environment.source = "pwcli-managed-venv".into();
    Ok(environment)
}

fn probe(executable: &Path, require_matplotlib: bool) -> Result<PythonEnvironment> {
    let script = if require_matplotlib {
        "import json,sys,matplotlib\nparts=tuple(int(p) for p in matplotlib.__version__.split('.')[:2])\nassert parts >= (3,8)\nprint(json.dumps({'python':sys.version.split()[0],'matplotlib':matplotlib.__version__}))"
    } else {
        "import json,sys\nassert sys.version_info >= (3,9)\nprint(json.dumps({'python':sys.version.split()[0],'matplotlib':''}))"
    };
    let output = Command::new(executable)
        .args(["-c", script])
        .output()
        .with_context(|| format!("run {}", executable.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "{} is not a compatible Python/matplotlib environment: {}",
        executable.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(PythonEnvironment {
        executable: executable.to_string_lossy().into_owned(),
        python_version: value["python"].as_str().unwrap_or_default().into(),
        matplotlib_version: value["matplotlib"].as_str().unwrap_or_default().into(),
        source: String::new(),
    })
}

fn run_logged(
    command: &mut Command,
    log_path: &Path,
    timeout: Duration,
    operation: &str,
) -> Result<()> {
    let stdout = File::create(log_path)?;
    let stderr = stdout.try_clone()?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| operation.to_string())?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            anyhow::ensure!(
                status.success(),
                "{operation} failed with {status}; inspect {}",
                log_path.display()
            );
            return Ok(());
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "{operation} exceeded {} seconds; inspect {}",
                timeout.as_secs(),
                log_path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn python_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("PWCLI_ILLUSTRATION_PYTHON") {
        candidates.push(value.into());
    }
    for variable in ["VIRTUAL_ENV", "CONDA_PREFIX"] {
        if let Some(root) = std::env::var_os(variable) {
            candidates.push(venv_python(Path::new(&root)));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(venv_python(&cwd.join(".venv")));
        candidates.push(venv_python(&cwd.join("venv")));
    }
    candidates.extend([
        PathBuf::from("python3"),
        PathBuf::from("python"),
        PathBuf::from("/opt/homebrew/bin/python3"),
        PathBuf::from("/usr/local/bin/python3"),
        PathBuf::from("/usr/bin/python3"),
    ]);
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn venv_python(root: &Path) -> PathBuf {
    #[cfg(windows)]
    return root.join("Scripts/python.exe");
    #[cfg(not(windows))]
    root.join("bin/python")
}

fn environment_root() -> PathBuf {
    crate::runtime::settings::local_config::data_dir().join("illustration/python")
}

fn status_path() -> PathBuf {
    environment_root().join("status.json")
}

fn write_status(status: &PythonEnvironmentStatus) -> Result<()> {
    let root = environment_root();
    fs::create_dir_all(&root)?;
    let temporary = root.join(".status.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(status)?)?;
    fs::rename(temporary, root.join("status.json"))?;
    Ok(())
}

fn bootstrap_disabled() -> bool {
    std::env::var("PWCLI_SKIP_ILLUSTRATION_BOOTSTRAP")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_list_prioritizes_explicit_python() {
        let previous = std::env::var_os("PWCLI_ILLUSTRATION_PYTHON");
        std::env::set_var("PWCLI_ILLUSTRATION_PYTHON", "/custom/python");
        assert_eq!(
            python_candidates().first(),
            Some(&PathBuf::from("/custom/python"))
        );
        if let Some(value) = previous {
            std::env::set_var("PWCLI_ILLUSTRATION_PYTHON", value);
        } else {
            std::env::remove_var("PWCLI_ILLUSTRATION_PYTHON");
        }
    }
}
