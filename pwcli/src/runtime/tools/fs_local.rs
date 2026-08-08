use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use tokio::process::Command;
use walkdir::WalkDir;

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct WorkerExecutionPolicy {
    root: PathBuf,
    allow_mutation: bool,
    shell_write_roots: Vec<PathBuf>,
}

static WORKER_EXECUTION_POLICY: OnceLock<WorkerExecutionPolicy> = OnceLock::new();

/// Restrict filesystem and shell tools in an independent internal-agent worker.
/// The daemon never installs this process-global policy, so its behavior is unchanged.
pub(crate) fn install_worker_execution_policy(
    root: &Path,
    data_dir: &Path,
    allow_mutation: bool,
) -> Result<()> {
    let policy = WorkerExecutionPolicy::new(root, data_dir, allow_mutation)?;
    WORKER_EXECUTION_POLICY
        .set(policy)
        .map_err(|_| anyhow::anyhow!("worker execution policy is already installed"))
}

pub(crate) fn worker_execution_policy() -> Option<&'static WorkerExecutionPolicy> {
    WORKER_EXECUTION_POLICY.get()
}

/// Build a child process command that inherits the worker's native filesystem
/// boundary. RuntimeTask workers are unattended, so platforms without a native
/// process sandbox must fail closed instead of launching an unrestricted CLI.
pub(crate) fn worker_subprocess_command(
    program: &str,
    program_args: &[String],
    policy: Option<&WorkerExecutionPolicy>,
    allow_auxiliary_writes: bool,
) -> Result<Command> {
    let Some(policy) = policy else {
        let mut command = Command::new(program);
        command.args(program_args);
        return Ok(command);
    };

    #[cfg(target_os = "macos")]
    {
        const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
        if !Path::new(SANDBOX_EXEC).is_file() {
            anyhow::bail!(
                "RuntimeTask child process is disabled: macOS sandbox-exec is unavailable"
            );
        }
        let profile = macos_worker_profile(policy, allow_auxiliary_writes)?;
        let mut command = Command::new(SANDBOX_EXEC);
        command.args(["-p", &profile, program]);
        command.args(program_args);
        Ok(command)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (program, program_args, policy, allow_auxiliary_writes);
        anyhow::bail!(
            "RuntimeTask child process is disabled: no native filesystem write sandbox is available"
        )
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_worker_profile(
    policy: &WorkerExecutionPolicy,
    allow_auxiliary_writes: bool,
) -> Result<String> {
    let mut writable = vec![format!(
        "(require-not (literal \"{}\"))",
        sandbox_profile_path(Path::new("/dev/null"))?
    )];
    if policy.allow_mutation() {
        writable.push(format!(
            "(require-not (subpath \"{}\"))",
            sandbox_profile_path(policy.root())?
        ));
    }
    if allow_auxiliary_writes {
        for path in policy.shell_write_roots() {
            writable.push(format!(
                "(require-not (subpath \"{}\"))",
                sandbox_profile_path(path)?
            ));
        }
    }
    Ok(format!(
        "(version 1)\n(allow default)\n(deny file-write* (require-all {}))\n",
        writable.join(" ")
    ))
}

#[cfg(any(target_os = "macos", test))]
fn sandbox_profile_path(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .with_context(|| format!("sandbox path is not UTF-8: {}", path.display()))?;
    Ok(value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r"))
}

impl WorkerExecutionPolicy {
    pub(crate) fn new(root: &Path, data_dir: &Path, allow_mutation: bool) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("worker cwd is not accessible: {}", root.display()))?;
        if !root.is_dir() {
            anyhow::bail!("worker cwd is not a directory: {}", root.display());
        }
        let data_dir = data_dir.canonicalize().with_context(|| {
            format!(
                "worker data directory is not accessible: {}",
                data_dir.display()
            )
        })?;
        Ok(Self {
            root,
            allow_mutation,
            shell_write_roots: vec![
                data_dir.join("shell-state"),
                data_dir.join("artifacts/bash"),
            ],
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn allow_mutation(&self) -> bool {
        self.allow_mutation
    }

    pub(crate) fn shell_write_roots(&self) -> &[PathBuf] {
        &self.shell_write_roots
    }
}

#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct FsSandbox {
    root: PathBuf,
}

impl FsSandbox {
    pub fn from_config() -> Result<Self> {
        if let Some(policy) = worker_execution_policy() {
            return Ok(Self {
                root: policy.root.clone(),
            });
        }
        let configured = crate::runtime::settings::local_config::get().tools.fs_base;
        Self::from_root_setting(&configured)
    }

    /// Resolve a configured `tools.fsBase` value using the same rules as the
    /// live daemon. Legacy empty values intentionally mean the current user's
    /// home directory, never the daemon process working directory.
    pub fn from_root_setting(configured: &str) -> Result<Self> {
        let configured = if configured.trim().is_empty() {
            "~/"
        } else {
            configured.trim()
        };
        Self::new(expand_tilde(configured))
    }

    pub fn new(root: PathBuf) -> Result<Self> {
        let root = std::fs::canonicalize(&root)
            .with_context(|| format!("tools.fsBase is not accessible: {}", root.display()))?;
        if !root.is_dir() {
            anyhow::bail!("tools.fsBase is not a directory: {}", root.display());
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_existing_directory(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let resolved = self.resolve(input)?;
        let canonical = std::fs::canonicalize(&resolved)
            .with_context(|| format!("directory is not accessible: {}", resolved.display()))?;
        if !canonical.starts_with(&self.root) {
            anyhow::bail!("Access denied: path outside tools.fsBase");
        }
        if !canonical.is_dir() {
            anyhow::bail!("Path is not a directory: {}", canonical.display());
        }
        Ok(canonical)
    }

    pub fn resolve(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let input = input.as_ref();
        let expanded = expand_path(input);
        let candidate = if expanded.is_absolute() {
            expanded
        } else {
            self.root.join(expanded)
        };
        let normalized = lexical_normalize(&candidate);
        let canonical = canonicalize_existing_ancestor(&normalized)?;
        if !canonical.starts_with(&self.root) {
            anyhow::bail!("Access denied: path outside tools.fsBase");
        }
        Ok(normalized)
    }

    pub async fn remove(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let target = self.resolve(input)?;
        if std::fs::canonicalize(&target)? == self.root {
            anyhow::bail!("Access denied: cannot remove tools.fsBase");
        }
        let metadata = tokio::fs::symlink_metadata(&target).await?;
        if metadata.is_dir() {
            tokio::fs::remove_dir_all(&target).await?;
        } else {
            tokio::fs::remove_file(&target).await?;
        }
        Ok(target)
    }
}

pub async fn list_directory(path: &Path) -> Result<Vec<LocalEntry>> {
    list_directory_with_visibility(
        path,
        crate::runtime::settings::local_config::get()
            .server
            .show_hidden_files,
    )
    .await
}

async fn list_directory_with_visibility(
    path: &Path,
    show_hidden_files: bool,
) -> Result<Vec<LocalEntry>> {
    let mut reader = tokio::fs::read_dir(path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        if !show_hidden_files && is_hidden_name(&entry.file_name()) {
            continue;
        }
        let metadata = entry.metadata().await?;
        entries.push(LocalEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: metadata.is_dir(),
            size: metadata.is_file().then_some(metadata.len()),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

pub async fn read_text(path: &Path) -> Result<String> {
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        anyhow::bail!("Path is not a file: {}", path.display());
    }
    if metadata.len() > MAX_FILE_BYTES {
        anyhow::bail!("File exceeds 10MB limit");
    }
    let bytes = tokio::fs::read(path).await?;
    if bytes.contains(&0) {
        anyhow::bail!("Binary file, size: {} bytes", bytes.len());
    }
    String::from_utf8(bytes).context("File is not valid UTF-8")
}

pub async fn search_files(root: &Path, query: &str) -> Result<Vec<String>> {
    search_files_with_visibility(
        root,
        query,
        crate::runtime::settings::local_config::get()
            .server
            .show_hidden_files,
    )
    .await
}

async fn search_files_with_visibility(
    root: &Path,
    query: &str,
    show_hidden_files: bool,
) -> Result<Vec<String>> {
    let needle = query.to_lowercase();
    if command_exists("rg").await {
        let output = Command::new("rg")
            .args(["--files", "--hidden"])
            .current_dir(root)
            .stdin(Stdio::null())
            .output()
            .await?;
        if output.status.success() {
            let mut paths = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| {
                    (show_hidden_files || !has_hidden_path_segment(Path::new(line)))
                        && line.to_lowercase().contains(&needle)
                })
                .map(|line| root.join(line).display().to_string())
                .collect::<Vec<_>>();
            paths.sort();
            return Ok(paths);
        }
    }
    let mut paths = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0 || show_hidden_files || !is_hidden_name(entry.file_name())
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .contains(&needle)
        })
        .map(|entry| entry.path().display().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn is_hidden_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn has_hidden_path_segment(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => is_hidden_name(name),
        _ => false,
    })
}

async fn command_exists(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Path has no existing ancestor: {}", path.display()))?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Path has no existing ancestor: {}", path.display()))?;
    }
    let mut canonical = std::fs::canonicalize(existing)?;
    for part in suffix.iter().rev() {
        canonical.push(part);
    }
    Ok(canonical)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn expand_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    expand_tilde(&value)
}

fn expand_tilde(value: &str) -> PathBuf {
    let value = value.trim();
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_root_setting_uses_home_instead_of_process_cwd() {
        let sandbox = FsSandbox::from_root_setting("").unwrap();
        let home = dirs::home_dir().unwrap().canonicalize().unwrap();
        assert_eq!(sandbox.root(), home);
    }

    #[test]
    fn root_setting_rejects_a_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("not-a-directory");
        std::fs::write(&file, "test").unwrap();
        assert!(FsSandbox::from_root_setting(&file.to_string_lossy()).is_err());
    }

    #[test]
    fn rejects_parent_and_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        let sandbox = FsSandbox::new(root.path().to_path_buf()).unwrap();
        assert!(sandbox.resolve("../outside").is_err());
        #[cfg(unix)]
        assert!(sandbox.resolve("escape/file.txt").is_err());
    }

    #[test]
    fn allows_missing_descendant_inside_root() {
        let root = tempfile::tempdir().unwrap();
        let sandbox = FsSandbox::new(root.path().to_path_buf()).unwrap();
        assert_eq!(
            sandbox.resolve("new/child.txt").unwrap(),
            std::fs::canonicalize(root.path())
                .unwrap()
                .join("new/child.txt")
        );
    }

    #[tokio::test]
    async fn directory_listing_respects_hidden_file_visibility() {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::write(root.path().join("visible.txt"), "visible")
            .await
            .unwrap();
        tokio::fs::write(root.path().join(".hidden.txt"), "hidden")
            .await
            .unwrap();
        tokio::fs::create_dir(root.path().join(".hidden-dir"))
            .await
            .unwrap();

        let hidden = list_directory_with_visibility(root.path(), false)
            .await
            .unwrap();
        assert_eq!(
            hidden
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["visible.txt"]
        );

        let visible = list_directory_with_visibility(root.path(), true)
            .await
            .unwrap();
        assert_eq!(
            visible
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            [".hidden-dir", ".hidden.txt", "visible.txt"]
        );
    }

    #[tokio::test]
    async fn file_search_filters_every_hidden_path_segment() {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::write(root.path().join("visible-needle.txt"), "visible")
            .await
            .unwrap();
        tokio::fs::write(root.path().join(".hidden-needle.txt"), "hidden")
            .await
            .unwrap();
        tokio::fs::create_dir(root.path().join(".hidden-dir"))
            .await
            .unwrap();
        tokio::fs::write(root.path().join(".hidden-dir/nested-needle.txt"), "hidden")
            .await
            .unwrap();

        let hidden = search_files_with_visibility(root.path(), "needle", false)
            .await
            .unwrap();
        assert_eq!(
            hidden,
            [root.path().join("visible-needle.txt").display().to_string()]
        );

        let visible = search_files_with_visibility(root.path(), "needle", true)
            .await
            .unwrap();
        assert_eq!(visible.len(), 3);
        assert!(visible
            .iter()
            .any(|path| path.ends_with(".hidden-dir/nested-needle.txt")));
        assert!(visible
            .iter()
            .any(|path| path.ends_with(".hidden-needle.txt")));
    }

    #[tokio::test]
    async fn hidden_file_remains_directly_readable() {
        let root = tempfile::tempdir().unwrap();
        let hidden = root.path().join(".env");
        tokio::fs::write(&hidden, "TOKEN=test").await.unwrap();

        assert_eq!(read_text(&hidden).await.unwrap(), "TOKEN=test");
    }

    #[test]
    fn worker_root_rejects_original_workspace_sibling() {
        let parent = tempfile::tempdir().unwrap();
        let worker = parent.path().join("worker-worktree");
        let original = parent.path().join("original-workspace");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::create_dir_all(&original).unwrap();

        let policy = WorkerExecutionPolicy::new(&worker, parent.path(), true).unwrap();
        let sandbox = FsSandbox::new(policy.root().to_path_buf()).unwrap();
        assert!(sandbox.resolve(&original).is_err());
        assert!(sandbox.resolve(worker.join("inside.txt")).is_ok());
    }

    #[test]
    fn acp_profile_only_grants_the_isolated_mutating_root() {
        let parent = tempfile::tempdir().unwrap();
        let worker = parent.path().join("worker-worktree");
        let data_dir = parent.path().join("data");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let canonical_worker = worker.canonicalize().unwrap();
        let shell_state = data_dir.canonicalize().unwrap().join("shell-state");

        let read_only = WorkerExecutionPolicy::new(&worker, &data_dir, false).unwrap();
        let read_only_profile = macos_worker_profile(&read_only, false).unwrap();
        assert!(!read_only_profile.contains(canonical_worker.to_str().unwrap()));
        assert!(!read_only_profile.contains(shell_state.to_str().unwrap()));

        let mutating = WorkerExecutionPolicy::new(&worker, &data_dir, true).unwrap();
        let mutating_profile = macos_worker_profile(&mutating, false).unwrap();
        assert!(mutating_profile.contains(canonical_worker.to_str().unwrap()));
        assert!(!mutating_profile.contains(shell_state.to_str().unwrap()));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn native_acp_boundary_blocks_escape_and_read_only_writes() {
        let parent = tempfile::tempdir().unwrap();
        let worker = parent.path().join("worker-worktree");
        let data_dir = parent.path().join("data");
        let outside = parent.path().join("outside.txt");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();

        let mutating = WorkerExecutionPolicy::new(&worker, &data_dir, true).unwrap();
        let args = vec![
            "-c".to_string(),
            format!(
                "printf allowed > inside.txt; printf blocked > '{}'",
                outside.to_string_lossy().replace('\'', "'\\''")
            ),
        ];
        let output = worker_subprocess_command("/bin/sh", &args, Some(&mutating), false)
            .unwrap()
            .current_dir(&worker)
            .output()
            .await
            .unwrap();
        if output.status.code() != Some(71)
            || !String::from_utf8_lossy(&output.stderr)
                .contains("sandbox_apply: Operation not permitted")
        {
            assert!(!output.status.success());
            assert!(worker.join("inside.txt").exists());
        }
        assert!(!outside.exists());

        let read_only = WorkerExecutionPolicy::new(&worker, &data_dir, false).unwrap();
        let args = vec![
            "-c".to_string(),
            "printf blocked > readonly.txt".to_string(),
        ];
        let output = worker_subprocess_command("/bin/sh", &args, Some(&read_only), false)
            .unwrap()
            .current_dir(&worker)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(!worker.join("readonly.txt").exists());
    }

    #[tokio::test]
    async fn remove_stays_inside_root_and_supports_directories() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sandbox = FsSandbox::new(root.path().to_path_buf()).unwrap();
        let canonical_root = std::fs::canonicalize(root.path()).unwrap();
        let file = canonical_root.join("inside.txt");
        let directory = canonical_root.join("nested");
        std::fs::write(&file, "ok").unwrap();
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("child.txt"), "ok").unwrap();

        assert_eq!(sandbox.remove("inside.txt").await.unwrap(), file);
        assert_eq!(sandbox.remove("nested").await.unwrap(), directory);
        assert!(!file.exists());
        assert!(!directory.exists());
        assert!(sandbox.remove(root.path()).await.is_err());
        assert!(sandbox.remove(outside.path()).await.is_err());
    }
}
