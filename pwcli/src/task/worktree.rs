use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_GIT_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PATCH_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REVIEW_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_REVIEW_DIFF_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
const MAX_MANIFEST_ENTRIES: usize = 20_000;
const MAX_PATH_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "sha256")]
pub enum ManifestHash {
    File(String),
    Symlink(String),
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeLease {
    pub task_id: String,
    pub lease_key: String,
    pub repo_root: PathBuf,
    pub bound_relative: PathBuf,
    pub original_head: String,
    pub baseline_commit: String,
    pub baseline_ref: String,
    pub lease_dir: PathBuf,
    pub worktree_dir: PathBuf,
    pub worker_cwd: PathBuf,
    pub manifest: BTreeMap<PathBuf, ManifestHash>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewArtifact {
    pub task_id: String,
    pub lease_key: String,
    pub attempt_id: String,
    pub baseline_commit: String,
    pub result_commit: String,
    pub patch_path: PathBuf,
    pub patch_bytes: u64,
    pub patch_sha256: String,
    pub affected_paths: Vec<PathBuf>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTextFile {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub is_binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffFile {
    pub path: String,
    pub status: String,
    pub size: u64,
    pub binary: bool,
    pub truncated: bool,
    pub patch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ApplyOutcome {
    Applied { affected_paths: Vec<PathBuf> },
    MergeRequired { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedWorktree {
    pub archive_dir: PathBuf,
    pub recover_until: DateTime<Utc>,
}

/// Captures the caller's dirty Git state in a private commit and creates an
/// isolated worktree without changing the caller's worktree or index.
pub fn prepare_mutating_task(
    data_dir: &Path,
    task_id: &str,
    bound_cwd: &Path,
) -> Result<WorktreeLease> {
    validate_identifier(task_id, "task id")?;
    let bound_cwd = bound_cwd
        .canonicalize()
        .with_context(|| format!("bound cwd does not exist: {}", bound_cwd.display()))?;
    if !bound_cwd.is_dir() {
        bail!("bound cwd must be a directory: {}", bound_cwd.display());
    }

    let repo_root_text = git_text(&bound_cwd, ["rev-parse", "--show-toplevel"])
        .context("mutating tasks require a Git repository")?;
    let repo_root = PathBuf::from(repo_root_text.trim())
        .canonicalize()
        .context("resolve Git repository root")?;
    let bound_relative = bound_cwd
        .strip_prefix(&repo_root)
        .context("bound cwd is outside the Git repository")?
        .to_path_buf();
    validate_relative_path(&bound_relative)?;

    let original_head = git_text(&repo_root, ["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_string();
    let lease_key = hex::encode(Sha256::digest(task_id.as_bytes()));
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create task data directory {}", data_dir.display()))?;
    let data_dir = data_dir
        .canonicalize()
        .with_context(|| format!("resolve task data directory {}", data_dir.display()))?;
    let task_worktrees = data_dir.join("task-worktrees");
    let lease_dir = task_worktrees.join("active").join(&lease_key);
    if lease_dir.exists() {
        bail!("task worktree already exists for {task_id}");
    }
    fs::create_dir_all(&lease_dir)
        .with_context(|| format!("create lease directory {}", lease_dir.display()))?;

    let result = (|| {
        let baseline_ref = format!("refs/pwcli/task-worktrees/{lease_key}/baseline");
        let index_path = lease_dir.join("baseline.index");
        let baseline_commit = snapshot_commit(
            &repo_root,
            &index_path,
            &original_head,
            &bound_relative,
            true,
            "pwcli task baseline",
        )?;
        git_status(
            &repo_root,
            [
                OsString::from("update-ref"),
                OsString::from(&baseline_ref),
                OsString::from(&baseline_commit),
            ],
        )?;

        let worktree_dir = lease_dir.join("worktree");
        git_status(
            &repo_root,
            [
                OsString::from("worktree"),
                OsString::from("add"),
                OsString::from("--detach"),
                worktree_dir.as_os_str().to_owned(),
                OsString::from(&baseline_commit),
            ],
        )?;
        let worker_cwd = worktree_dir.join(&bound_relative);
        if !worker_cwd.is_dir() {
            bail!(
                "bound directory is absent from the captured baseline: {}",
                bound_relative.display()
            );
        }

        let manifest = build_manifest(&repo_root, &bound_relative)?;
        let lease = WorktreeLease {
            task_id: task_id.to_string(),
            lease_key,
            repo_root,
            bound_relative,
            original_head,
            baseline_commit,
            baseline_ref,
            lease_dir: lease_dir.clone(),
            worktree_dir,
            worker_cwd,
            manifest,
            created_at: Utc::now(),
        };
        write_json(&lease_dir.join("lease.json"), &lease)?;
        Ok(lease)
    })();

    if result.is_err() {
        let _ = fs::remove_file(lease_dir.join("baseline.index"));
    }
    result
}

/// Freezes the worker's current bound state and emits a binary-capable review patch.
pub fn finalize_attempt(lease: &WorktreeLease, attempt_id: &str) -> Result<ReviewArtifact> {
    validate_identifier(attempt_id, "attempt id")?;
    validate_lease(lease)?;

    let attempt_key = hex::encode(Sha256::digest(attempt_id.as_bytes()));
    let attempt_dir = lease.lease_dir.join("attempts").join(&attempt_key);
    if attempt_dir.exists() {
        bail!("attempt artifact already exists for {attempt_id}");
    }
    fs::create_dir_all(&attempt_dir)
        .with_context(|| format!("create attempt directory {}", attempt_dir.display()))?;

    let index_path = attempt_dir.join("result.index");
    let result_commit = snapshot_commit(
        &lease.worktree_dir,
        &index_path,
        &lease.baseline_commit,
        &lease.bound_relative,
        false,
        "pwcli task attempt",
    )?;
    let attempt_ref = format!(
        "refs/pwcli/task-worktrees/{}/attempts/{attempt_key}",
        lease.lease_key
    );
    git_status(
        &lease.repo_root,
        [
            OsString::from("update-ref"),
            OsString::from(attempt_ref),
            OsString::from(&result_commit),
        ],
    )?;

    let patch_path = attempt_dir.join("changes.patch");
    write_diff(
        &lease.repo_root,
        &lease.baseline_commit,
        &result_commit,
        &lease.bound_relative,
        &patch_path,
    )?;
    let patch_bytes = fs::metadata(&patch_path)?.len();
    if patch_bytes > MAX_PATCH_BYTES {
        bail!(
            "review patch exceeds {} bytes (got {patch_bytes})",
            MAX_PATCH_BYTES
        );
    }
    let patch_sha256 = hash_file_hex(&patch_path)?;

    let affected_paths = diff_paths(
        &lease.repo_root,
        &lease.baseline_commit,
        &result_commit,
        &lease.bound_relative,
    )?;
    let artifact = ReviewArtifact {
        task_id: lease.task_id.clone(),
        lease_key: lease.lease_key.clone(),
        attempt_id: attempt_id.to_string(),
        baseline_commit: lease.baseline_commit.clone(),
        result_commit,
        patch_path,
        patch_bytes,
        patch_sha256,
        affected_paths,
        created_at: Utc::now(),
    };
    write_json(&attempt_dir.join("review.json"), &artifact)?;
    Ok(artifact)
}

/// Applies an accepted artifact once, leaving changes unstaged and creating no commit.
pub fn verify_and_apply(lease: &WorktreeLease, artifact: &ReviewArtifact) -> Result<ApplyOutcome> {
    validate_lease(lease)?;
    validate_artifact(lease, artifact)?;
    if artifact.affected_paths.is_empty() {
        return Ok(ApplyOutcome::Applied {
            affected_paths: Vec::new(),
        });
    }

    let already_applied = artifact
        .affected_paths
        .iter()
        .map(|relative| {
            Ok(hash_path(&lease.repo_root.join(relative))?
                == hash_path_at_commit(&lease.repo_root, &artifact.result_commit, relative)?)
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .all(|matches| matches);
    if already_applied {
        return Ok(ApplyOutcome::Applied {
            affected_paths: artifact.affected_paths.clone(),
        });
    }

    let current_head = git_text(&lease.repo_root, ["rev-parse", "--verify", "HEAD"])?;
    if current_head.trim() != lease.original_head {
        return Ok(ApplyOutcome::MergeRequired {
            reason: "repository HEAD changed after task preparation".to_string(),
        });
    }

    for relative in &artifact.affected_paths {
        let expected = lease
            .manifest
            .get(relative)
            .cloned()
            .unwrap_or(ManifestHash::Missing);
        let actual = hash_path(&lease.repo_root.join(relative))?;
        if actual != expected {
            return Ok(ApplyOutcome::MergeRequired {
                reason: format!("affected path changed: {}", relative.display()),
            });
        }
    }

    let check = git_output(
        &lease.repo_root,
        [
            OsString::from("apply"),
            OsString::from("--check"),
            OsString::from("--binary"),
            artifact.patch_path.as_os_str().to_owned(),
        ],
    )?;
    if !check.status.success() {
        return Ok(ApplyOutcome::MergeRequired {
            reason: format!(
                "patch no longer applies cleanly: {}",
                String::from_utf8_lossy(&check.stderr).trim()
            ),
        });
    }

    git_status(
        &lease.repo_root,
        [
            OsString::from("apply"),
            OsString::from("--binary"),
            artifact.patch_path.as_os_str().to_owned(),
        ],
    )?;
    Ok(ApplyOutcome::Applied {
        affected_paths: artifact.affected_paths.clone(),
    })
}

fn hash_path_at_commit(repo_root: &Path, commit: &str, path: &Path) -> Result<ManifestHash> {
    let output = git_output(
        repo_root,
        [
            OsString::from("ls-tree"),
            OsString::from("-z"),
            OsString::from(commit),
            OsString::from("--"),
            pathspec(path)?,
        ],
    )?;
    let bytes = check_output_bytes(output, "git ls-tree revision path")?;
    let Some(entry) = bytes
        .split(|byte| *byte == 0)
        .find(|entry| !entry.is_empty())
    else {
        return Ok(ManifestHash::Missing);
    };
    let separator = entry
        .iter()
        .position(|byte| *byte == b'\t')
        .context("invalid git ls-tree revision output")?;
    let header = std::str::from_utf8(&entry[..separator])?;
    let fields = header.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 3 || fields[1] != "blob" {
        bail!("revision path is not a file: {}", path.display());
    }
    let output = git_output(
        repo_root,
        [
            OsString::from("cat-file"),
            OsString::from("blob"),
            OsString::from(fields[2]),
        ],
    )?;
    let digest = hex::encode(Sha256::digest(check_output_bytes(
        output,
        "git cat-file revision path",
    )?));
    Ok(if fields[0] == "120000" {
        ManifestHash::Symlink(digest)
    } else {
        ManifestHash::File(digest)
    })
}

/// Reads one affected text file from the immutable result commit captured for review.
/// `path` is relative to the task's bound cwd, matching `changedFiles[].path`.
pub fn read_review_text_file(
    lease: &WorktreeLease,
    artifact: &ReviewArtifact,
    path: &Path,
) -> Result<ReviewTextFile> {
    validate_lease(lease)?;
    validate_artifact(lease, artifact)?;
    validate_relative_path(path)?;
    if path.as_os_str().is_empty() {
        bail!("review file path is required");
    }

    let repo_relative = lease.bound_relative.join(path);
    validate_scoped_path(&repo_relative, &lease.bound_relative)?;
    if !artifact
        .affected_paths
        .iter()
        .any(|affected| affected == &repo_relative)
    {
        bail!(
            "path is not affected by this review artifact: {}",
            path.display()
        );
    }

    let output = git_output(
        &lease.repo_root,
        [
            OsString::from("ls-tree"),
            OsString::from("-z"),
            OsString::from(&artifact.result_commit),
            OsString::from("--"),
            pathspec(&repo_relative)?,
        ],
    )?;
    let bytes = check_output_bytes(output, "git ls-tree review file")?;
    let entries = bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    if entries.len() != 1 {
        bail!(
            "review file does not exist in the captured revision: {}",
            path.display()
        );
    }
    let separator = entries[0]
        .iter()
        .position(|byte| *byte == b'\t')
        .context("invalid git ls-tree output")?;
    let (header, returned_path) = entries[0].split_at(separator);
    let returned_path = &returned_path[1..];
    if parse_git_path(returned_path)? != repo_relative {
        bail!("captured review file path does not match the request");
    }
    let header = std::str::from_utf8(header).context("invalid git ls-tree header")?;
    let fields = header.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 3 || fields[1] != "blob" {
        bail!("review path is not a regular file: {}", path.display());
    }
    if fields[0] == "120000" {
        bail!("review file symlinks are not readable");
    }
    if !matches!(fields[0], "100644" | "100755") {
        bail!("review path is not a regular file: {}", path.display());
    }

    let size = git_text(&lease.repo_root, ["cat-file", "-s", fields[2]])?
        .trim()
        .parse::<u64>()
        .context("invalid review file size")?;
    if size > MAX_REVIEW_TEXT_BYTES {
        bail!("review file exceeds {MAX_REVIEW_TEXT_BYTES} bytes");
    }
    let output = git_output(
        &lease.repo_root,
        [
            OsString::from("cat-file"),
            OsString::from("blob"),
            OsString::from(fields[2]),
        ],
    )?;
    let bytes = check_output_bytes(output, "git cat-file review file")?;
    if bytes.len() as u64 != size {
        bail!("review file size changed while reading");
    }
    if bytes.contains(&0) {
        bail!("review file is binary");
    }
    let content = String::from_utf8(bytes).context("review file is binary")?;
    Ok(ReviewTextFile {
        path: path.to_string_lossy().replace('\\', "/"),
        content,
        size,
        is_binary: false,
    })
}

/// Builds one on-demand diff from the immutable baseline/result commits. This
/// remains available for binary, large, or late-list files that were omitted
/// from the eager `changedFiles` text preview.
pub fn read_review_diff_file(
    lease: &WorktreeLease,
    artifact: &ReviewArtifact,
    path: &Path,
) -> Result<ReviewDiffFile> {
    validate_lease(lease)?;
    validate_artifact(lease, artifact)?;
    validate_relative_path(path)?;
    if path.as_os_str().is_empty() {
        bail!("review diff path is required");
    }
    let repo_relative = lease.bound_relative.join(path);
    validate_scoped_path(&repo_relative, &lease.bound_relative)?;
    if !artifact
        .affected_paths
        .iter()
        .any(|affected| affected == &repo_relative)
    {
        bail!(
            "path is not affected by this review artifact: {}",
            path.display()
        );
    }

    let name_status = git_output(
        &lease.repo_root,
        [
            OsString::from("diff"),
            OsString::from("--name-status"),
            OsString::from("--no-renames"),
            OsString::from("-z"),
            artifact.baseline_commit.clone().into(),
            artifact.result_commit.clone().into(),
            OsString::from("--"),
            pathspec(&repo_relative)?,
        ],
    )?;
    let name_status = check_output_bytes(name_status, "git diff --name-status")?;
    let status = match name_status.first().copied() {
        Some(b'A') => "added",
        Some(b'D') => "deleted",
        Some(b'T') => "type_changed",
        Some(b'M') => "modified",
        _ => "changed",
    }
    .to_string();

    let numstat = git_output(
        &lease.repo_root,
        [
            OsString::from("diff"),
            OsString::from("--numstat"),
            OsString::from("--no-renames"),
            artifact.baseline_commit.clone().into(),
            artifact.result_commit.clone().into(),
            OsString::from("--"),
            pathspec(&repo_relative)?,
        ],
    )?;
    let numstat = check_output_bytes(numstat, "git diff --numstat")?;
    let binary = numstat
        .split(|byte| *byte == b'\t')
        .take(2)
        .any(|field| field == b"-");

    let patch_output = git_output(
        &lease.repo_root,
        [
            OsString::from("diff"),
            OsString::from("--binary"),
            OsString::from("--no-ext-diff"),
            OsString::from("--full-index"),
            OsString::from("--no-renames"),
            artifact.baseline_commit.clone().into(),
            artifact.result_commit.clone().into(),
            OsString::from("--"),
            pathspec(&repo_relative)?,
        ],
    )?;
    if !patch_output.status.success() {
        bail!(
            "git diff review file failed: {}",
            String::from_utf8_lossy(&patch_output.stderr).trim()
        );
    }
    if patch_output.stdout.len() as u64 > MAX_PATCH_BYTES {
        bail!("review file diff exceeds {MAX_PATCH_BYTES} bytes");
    }
    let truncated = patch_output.stdout.len() > MAX_REVIEW_DIFF_PREVIEW_BYTES;
    let preview = if truncated {
        &patch_output.stdout[..MAX_REVIEW_DIFF_PREVIEW_BYTES]
    } else {
        &patch_output.stdout
    };

    let size =
        revision_file_size(&lease.repo_root, &artifact.result_commit, &repo_relative)?.unwrap_or(0);
    Ok(ReviewDiffFile {
        path: path.to_string_lossy().replace('\\', "/"),
        status,
        size,
        binary,
        truncated,
        patch: String::from_utf8_lossy(preview).into_owned(),
    })
}

fn revision_file_size(repo_root: &Path, revision: &str, path: &Path) -> Result<Option<u64>> {
    let output = git_output(
        repo_root,
        [
            OsString::from("ls-tree"),
            OsString::from("-z"),
            OsString::from(revision),
            OsString::from("--"),
            pathspec(path)?,
        ],
    )?;
    let bytes = check_output_bytes(output, "git ls-tree review diff")?;
    let Some(entry) = bytes
        .split(|byte| *byte == 0)
        .find(|entry| !entry.is_empty())
    else {
        return Ok(None);
    };
    let separator = entry
        .iter()
        .position(|byte| *byte == b'\t')
        .context("invalid git ls-tree review diff output")?;
    let fields = std::str::from_utf8(&entry[..separator])?
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields.len() != 3 || fields[1] != "blob" || fields[0] == "120000" {
        return Ok(None);
    }
    Ok(Some(
        git_text(repo_root, ["cat-file", "-s", fields[2]])?
            .trim()
            .parse::<u64>()
            .context("invalid review diff file size")?,
    ))
}

/// Moves a rejected lease and its registered worktree into a seven-day recovery archive.
pub fn archive_rejected(lease: &WorktreeLease) -> Result<ArchivedWorktree> {
    validate_lease(lease)?;
    if !lease.lease_dir.exists() {
        let archive_root = lease
            .lease_dir
            .parent()
            .and_then(Path::parent)
            .context("invalid lease directory layout")?
            .join("archive");
        let mut candidates = fs::read_dir(&archive_root)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{}-", lease.lease_key))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|entry| entry.file_name());
        if let Some(existing) = candidates.pop() {
            return serde_json::from_slice(&fs::read(existing.path().join("archive.json"))?)
                .context("read archived task worktree metadata");
        }
        bail!("task worktree is neither active nor archived");
    }
    let archive_dir = lease
        .lease_dir
        .parent()
        .and_then(Path::parent)
        .context("invalid lease directory layout")?
        .join("archive")
        .join(format!(
            "{}-{}",
            lease.lease_key,
            Utc::now().format("%Y%m%dT%H%M%SZ")
        ));
    if archive_dir.exists() {
        bail!("archive already exists: {}", archive_dir.display());
    }
    fs::create_dir_all(&archive_dir)?;

    let archived_worktree = archive_dir.join("worktree");
    git_status(
        &lease.repo_root,
        [
            OsString::from("worktree"),
            OsString::from("move"),
            lease.worktree_dir.as_os_str().to_owned(),
            archived_worktree.as_os_str().to_owned(),
        ],
    )?;
    for entry in fs::read_dir(&lease.lease_dir)? {
        let entry = entry?;
        if entry.file_name() == OsStr::new("worktree") {
            continue;
        }
        fs::rename(entry.path(), archive_dir.join(entry.file_name()))?;
    }
    fs::remove_dir(&lease.lease_dir)?;

    let archived = ArchivedWorktree {
        archive_dir,
        recover_until: Utc::now() + Duration::days(7),
    };
    write_json(&archived.archive_dir.join("archive.json"), &archived)?;
    Ok(archived)
}

pub fn archived_review_lease(lease: &WorktreeLease, archived: &ArchivedWorktree) -> WorktreeLease {
    let mut archived_lease = lease.clone();
    archived_lease.lease_dir = archived.archive_dir.clone();
    archived_lease.worktree_dir = archived.archive_dir.join("worktree");
    archived_lease.worker_cwd = archived_lease.worktree_dir.join(&lease.bound_relative);
    archived_lease
}

pub fn archived_review_artifact(
    lease: &WorktreeLease,
    archived: &ArchivedWorktree,
    artifact: &ReviewArtifact,
) -> ReviewArtifact {
    let mut archived_artifact = artifact.clone();
    if let Ok(relative) = artifact.patch_path.strip_prefix(&lease.lease_dir) {
        archived_artifact.patch_path = archived.archive_dir.join(relative);
    }
    archived_artifact
}

fn snapshot_commit(
    cwd: &Path,
    index_path: &Path,
    parent: &str,
    scope: &Path,
    include_all_tracked: bool,
    message: &str,
) -> Result<String> {
    if index_path.exists() {
        fs::remove_file(index_path)?;
    }
    git_with_index(
        cwd,
        index_path,
        [OsString::from("read-tree"), parent.into()],
    )?;

    if include_all_tracked {
        git_with_index(
            cwd,
            index_path,
            [
                OsString::from("add"),
                OsString::from("-u"),
                OsString::from("--"),
                pathspec(Path::new(""))?,
            ],
        )?;
    }

    let mut add_args = vec![
        OsString::from("add"),
        OsString::from("-A"),
        OsString::from("--"),
    ];
    add_args.push(pathspec(scope)?);
    git_with_index(cwd, index_path, add_args)?;

    let tree = git_text_with_index(cwd, index_path, ["write-tree"])?;
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["commit-tree", tree.trim(), "-p", parent])
        .env("GIT_AUTHOR_NAME", "pwcli")
        .env("GIT_AUTHOR_EMAIL", "pwcli@localhost")
        .env("GIT_COMMITTER_NAME", "pwcli")
        .env("GIT_COMMITTER_EMAIL", "pwcli@localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn git commit-tree")?;
    let mut child = output;
    use std::io::Write;
    child
        .stdin
        .take()
        .context("open git commit-tree stdin")?
        .write_all(message.as_bytes())?;
    let output = child.wait_with_output()?;
    Ok(check_output(output, "git commit-tree")?.trim().to_string())
}

fn build_manifest(repo_root: &Path, scope: &Path) -> Result<BTreeMap<PathBuf, ManifestHash>> {
    let output = git_output(
        repo_root,
        [
            OsString::from("ls-files"),
            OsString::from("-z"),
            OsString::from("--cached"),
            OsString::from("--others"),
            OsString::from("--exclude-standard"),
            OsString::from("--"),
            pathspec(scope)?,
        ],
    )?;
    let bytes = check_output_bytes(output, "git ls-files")?;
    let mut manifest = BTreeMap::new();
    for raw in bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        if manifest.len() >= MAX_MANIFEST_ENTRIES {
            bail!("bound manifest exceeds {MAX_MANIFEST_ENTRIES} entries");
        }
        let relative = parse_git_path(raw)?;
        validate_scoped_path(&relative, scope)?;
        manifest.insert(relative.clone(), hash_path(&repo_root.join(relative))?);
    }
    Ok(manifest)
}

fn write_diff(
    repo_root: &Path,
    baseline: &str,
    result: &str,
    scope: &Path,
    destination: &Path,
) -> Result<()> {
    let file = File::create(destination)?;
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["diff", "--binary", "--full-index", "--no-renames"])
        .arg(baseline)
        .arg(result)
        .arg("--")
        .arg(pathspec(scope)?)
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .status()
        .context("run git diff")?;
    if !status.success() {
        bail!("git diff failed with {status}");
    }
    Ok(())
}

fn diff_paths(
    repo_root: &Path,
    baseline: &str,
    result: &str,
    scope: &Path,
) -> Result<Vec<PathBuf>> {
    let output = git_output(
        repo_root,
        [
            OsString::from("diff"),
            OsString::from("--name-only"),
            OsString::from("-z"),
            OsString::from("--no-renames"),
            OsString::from(baseline),
            OsString::from(result),
            OsString::from("--"),
            pathspec(scope)?,
        ],
    )?;
    let bytes = check_output_bytes(output, "git diff --name-only")?;
    let mut paths = Vec::new();
    for raw in bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        if paths.len() >= MAX_MANIFEST_ENTRIES {
            bail!("review artifact exceeds {MAX_MANIFEST_ENTRIES} affected paths");
        }
        let relative = parse_git_path(raw)?;
        validate_scoped_path(&relative, scope)?;
        paths.push(relative);
    }
    Ok(paths)
}

fn validate_lease(lease: &WorktreeLease) -> Result<()> {
    validate_identifier(&lease.task_id, "task id")?;
    validate_relative_path(&lease.bound_relative)?;
    if !lease.repo_root.is_absolute()
        || !lease.lease_dir.is_absolute()
        || !lease.worktree_dir.is_absolute()
        || !lease.worker_cwd.is_absolute()
    {
        bail!("lease paths must be absolute");
    }
    if lease.worktree_dir != lease.lease_dir.join("worktree")
        || lease.worker_cwd != lease.worktree_dir.join(&lease.bound_relative)
    {
        bail!("lease paths do not match their validated layout");
    }
    Ok(())
}

fn validate_artifact(lease: &WorktreeLease, artifact: &ReviewArtifact) -> Result<()> {
    validate_identifier(&artifact.attempt_id, "attempt id")?;
    if artifact.task_id != lease.task_id
        || artifact.lease_key != lease.lease_key
        || artifact.baseline_commit != lease.baseline_commit
    {
        bail!("review artifact does not belong to this lease");
    }
    let attempt_key = hex::encode(Sha256::digest(artifact.attempt_id.as_bytes()));
    let expected_patch = lease
        .lease_dir
        .join("attempts")
        .join(attempt_key)
        .join("changes.patch");
    if !artifact.patch_path.is_absolute() || artifact.patch_path != expected_patch {
        bail!("review patch is outside the lease artifact directory");
    }
    let actual_size = fs::metadata(&artifact.patch_path)?.len();
    if actual_size != artifact.patch_bytes || actual_size > MAX_PATCH_BYTES {
        bail!("review patch size does not match its metadata");
    }
    if hash_file_hex(&artifact.patch_path)? != artifact.patch_sha256 {
        bail!("review patch digest does not match its metadata");
    }
    for path in &artifact.affected_paths {
        validate_scoped_path(path, &lease.bound_relative)?;
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        bail!("{label} must be 1..=256 non-control characters");
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.is_absolute() || path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES {
        bail!("invalid relative path: {}", path.display());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            bail!(
                "relative path contains an unsafe component: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn validate_scoped_path(path: &Path, scope: &Path) -> Result<()> {
    validate_relative_path(path)?;
    if !scope.as_os_str().is_empty() && !path.starts_with(scope) {
        bail!("path escapes bound scope: {}", path.display());
    }
    Ok(())
}

fn pathspec(path: &Path) -> Result<OsString> {
    validate_relative_path(path)?;
    Ok(if path.as_os_str().is_empty() {
        OsString::from(":(top)")
    } else {
        let mut value = OsString::from(":(top,literal)");
        value.push(path);
        value
    })
}

fn parse_git_path(bytes: &[u8]) -> Result<PathBuf> {
    if bytes.len() > MAX_PATH_BYTES {
        bail!("Git path exceeds {MAX_PATH_BYTES} bytes");
    }
    let text = std::str::from_utf8(bytes).context("non-UTF-8 Git paths are unsupported")?;
    let path = PathBuf::from(text);
    validate_relative_path(&path)?;
    Ok(path)
}

fn hash_path(path: &Path) -> Result<ManifestHash> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManifestHash::Missing)
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        return Ok(ManifestHash::Symlink(hex::encode(Sha256::digest(
            target.as_os_str().as_encoded_bytes(),
        ))));
    }
    if !metadata.is_file() {
        bail!("manifest path is not a regular file: {}", path.display());
    }
    Ok(ManifestHash::File(hash_file_hex(path)?))
}

fn hash_file_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest)?;
    Ok(hex::encode(digest.finalize()))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() > MAX_GIT_OUTPUT_BYTES {
        bail!("metadata exceeds {MAX_GIT_OUTPUT_BYTES} bytes");
    }
    fs::write(path, bytes).with_context(|| format!("write metadata {}", path.display()))
}

fn git_text<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<String> {
    check_output(git_output(cwd, args.map(OsString::from))?, "git")
}

fn git_text_with_index<const N: usize>(
    cwd: &Path,
    index: &Path,
    args: [&str; N],
) -> Result<String> {
    check_output(
        git_output_with_index(cwd, index, args.map(OsString::from))?,
        "git",
    )
}

fn git_status<I, S>(cwd: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    check_output(git_output(cwd, args)?, "git").map(|_| ())
}

fn git_with_index<I, S>(cwd: &Path, index: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    check_output(git_output_with_index(cwd, index, args)?, "git").map(|_| ())
}

fn git_output<I, S>(cwd: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .context("run git")
}

fn git_output_with_index<I, S>(cwd: &Path, index: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .output()
        .context("run git with private index")
}

fn check_output(output: Output, operation: &str) -> Result<String> {
    let bytes = check_output_bytes(output, operation)?;
    String::from_utf8(bytes).with_context(|| format!("{operation} returned non-UTF-8 output"))
}

fn check_output_bytes(output: Output, operation: &str) -> Result<Vec<u8>> {
    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES || output.stderr.len() > MAX_GIT_OUTPUT_BYTES {
        bail!("{operation} output exceeds {MAX_GIT_OUTPUT_BYTES} bytes");
    }
    if !output.status.success() {
        bail!(
            "{operation} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Repo {
        temp: TempDir,
        root: PathBuf,
        data: PathBuf,
    }

    impl Repo {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("repo");
            let data = temp.path().join("data");
            fs::create_dir_all(root.join("bound")).unwrap();
            run(&root, ["init", "-q"]);
            run(&root, ["config", "user.name", "Test"]);
            run(&root, ["config", "user.email", "test@example.com"]);
            fs::write(root.join("bound/a.txt"), "base\n").unwrap();
            fs::write(root.join("bound/b.txt"), "base b\n").unwrap();
            fs::write(root.join("outside.txt"), "outside base\n").unwrap();
            run(&root, ["add", "."]);
            run(&root, ["commit", "-qm", "initial"]);
            Self { temp, root, data }
        }

        fn bound(&self) -> PathBuf {
            self.root.join("bound")
        }
    }

    fn run<const N: usize>(cwd: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn dirty_baseline_is_visible_and_worker_is_isolated() {
        let repo = Repo::new();
        fs::write(repo.root.join("bound/a.txt"), "staged\n").unwrap();
        run(&repo.root, ["add", "bound/a.txt"]);
        fs::write(repo.root.join("bound/a.txt"), "unstaged\n").unwrap();
        fs::write(repo.root.join("bound/new.txt"), "untracked\n").unwrap();
        fs::write(repo.root.join("outside.txt"), "outside dirty\n").unwrap();
        fs::write(repo.root.join("outside-new.txt"), "outside untracked\n").unwrap();

        let lease = prepare_mutating_task(&repo.data, "task-1", &repo.bound()).unwrap();
        assert_eq!(
            fs::read_to_string(lease.worker_cwd.join("a.txt")).unwrap(),
            "unstaged\n"
        );
        assert_eq!(
            fs::read_to_string(lease.worker_cwd.join("new.txt")).unwrap(),
            "untracked\n"
        );
        assert_eq!(
            fs::read_to_string(lease.worktree_dir.join("outside.txt")).unwrap(),
            "outside dirty\n"
        );
        assert!(!lease.worktree_dir.join("outside-new.txt").exists());
        let staged = git_text(&repo.root, ["show", ":bound/a.txt"]).unwrap();
        assert_eq!(staged, "staged\n");
        fs::write(lease.worker_cwd.join("a.txt"), "worker\n").unwrap();
        assert_eq!(
            fs::read_to_string(repo.root.join("bound/a.txt")).unwrap(),
            "unstaged\n"
        );
        assert!(repo.temp.path().exists());
    }

    #[test]
    fn finalizes_and_applies_text_and_binary_without_committing() {
        let repo = Repo::new();
        let lease = prepare_mutating_task(&repo.data, "task-2", &repo.bound()).unwrap();
        fs::write(lease.worker_cwd.join("a.txt"), "worker edit\n").unwrap();
        fs::write(lease.worker_cwd.join("binary.bin"), [0, 1, 0xff, 3]).unwrap();

        let artifact = finalize_attempt(&lease, "attempt-1").unwrap();
        assert!(artifact.patch_bytes > 0);
        let outcome = verify_and_apply(&lease, &artifact).unwrap();
        assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
        let replay = verify_and_apply(&lease, &artifact).unwrap();
        assert!(matches!(replay, ApplyOutcome::Applied { .. }));
        assert_eq!(
            fs::read_to_string(repo.root.join("bound/a.txt")).unwrap(),
            "worker edit\n"
        );
        assert_eq!(
            fs::read(repo.root.join("bound/binary.bin")).unwrap(),
            [0, 1, 0xff, 3]
        );
        let head = git_text(&repo.root, ["rev-parse", "HEAD"]).unwrap();
        assert_eq!(head.trim(), lease.original_head);
        let status = git_text(&repo.root, ["status", "--porcelain"]).unwrap();
        assert!(status.contains(" M bound/a.txt"));
        assert!(status.contains("?? bound/binary.bin"));
    }

    #[test]
    fn review_file_reads_the_frozen_worker_revision() {
        let repo = Repo::new();
        let lease = prepare_mutating_task(&repo.data, "task-review-read", &repo.bound()).unwrap();
        fs::write(lease.worker_cwd.join("a.txt"), "worker final\n").unwrap();
        let artifact = finalize_attempt(&lease, "attempt-1").unwrap();

        fs::write(
            lease.worker_cwd.join("a.txt"),
            "changed after finalization\n",
        )
        .unwrap();
        let file = read_review_text_file(&lease, &artifact, Path::new("a.txt")).unwrap();

        assert_eq!(file.path, "a.txt");
        assert_eq!(file.content, "worker final\n");
        assert_eq!(
            fs::read_to_string(repo.bound().join("a.txt")).unwrap(),
            "base\n"
        );
    }

    #[test]
    fn review_diff_is_generated_on_demand_from_the_frozen_commits() {
        let repo = Repo::new();
        let lease = prepare_mutating_task(&repo.data, "task-review-diff", &repo.bound()).unwrap();
        fs::write(lease.worker_cwd.join("a.txt"), "worker final\n").unwrap();
        fs::write(lease.worker_cwd.join("binary.bin"), [0, 1, 0xff, 3]).unwrap();
        let artifact = finalize_attempt(&lease, "attempt-1").unwrap();

        let text = read_review_diff_file(&lease, &artifact, Path::new("a.txt")).unwrap();
        assert_eq!(text.status, "modified");
        assert!(!text.binary);
        assert!(!text.truncated);
        assert!(text.patch.contains("-base"));
        assert!(text.patch.contains("+worker final"));

        let binary = read_review_diff_file(&lease, &artifact, Path::new("binary.bin")).unwrap();
        assert_eq!(binary.status, "added");
        assert!(binary.binary);
        assert_eq!(binary.size, 4);
        assert!(read_review_diff_file(&lease, &artifact, Path::new("b.txt")).is_err());
    }

    #[test]
    fn review_file_rejects_unaffected_and_unsafe_paths() {
        let repo = Repo::new();
        let lease = prepare_mutating_task(&repo.data, "task-review-scope", &repo.bound()).unwrap();
        fs::write(lease.worker_cwd.join("a.txt"), "worker final\n").unwrap();
        let artifact = finalize_attempt(&lease, "attempt-1").unwrap();

        assert!(read_review_text_file(&lease, &artifact, Path::new("b.txt")).is_err());
        assert!(read_review_text_file(&lease, &artifact, Path::new("../outside.txt")).is_err());
        assert!(read_review_text_file(&lease, &artifact, Path::new("/etc/passwd")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn review_file_rejects_symlinks_and_binary_content() {
        use std::os::unix::fs::symlink;

        let repo = Repo::new();
        let lease =
            prepare_mutating_task(&repo.data, "task-review-file-types", &repo.bound()).unwrap();
        symlink("../../outside.txt", lease.worker_cwd.join("escape.txt")).unwrap();
        fs::write(lease.worker_cwd.join("binary.bin"), [0, 1, 2, 3]).unwrap();
        fs::write(
            lease.worker_cwd.join("large.txt"),
            vec![b'a'; MAX_REVIEW_TEXT_BYTES as usize + 1],
        )
        .unwrap();
        let artifact = finalize_attempt(&lease, "attempt-1").unwrap();

        assert!(read_review_text_file(&lease, &artifact, Path::new("escape.txt")).is_err());
        assert!(read_review_text_file(&lease, &artifact, Path::new("binary.bin")).is_err());
        assert!(read_review_text_file(&lease, &artifact, Path::new("large.txt")).is_err());
    }

    #[test]
    fn conflict_returns_merge_required_without_partial_write() {
        let repo = Repo::new();
        let lease = prepare_mutating_task(&repo.data, "task-3", &repo.bound()).unwrap();
        fs::write(lease.worker_cwd.join("a.txt"), "worker a\n").unwrap();
        fs::write(lease.worker_cwd.join("b.txt"), "worker b\n").unwrap();
        let artifact = finalize_attempt(&lease, "attempt-1").unwrap();

        fs::write(repo.root.join("bound/a.txt"), "concurrent\n").unwrap();
        let outcome = verify_and_apply(&lease, &artifact).unwrap();
        assert!(matches!(outcome, ApplyOutcome::MergeRequired { .. }));
        assert_eq!(
            fs::read_to_string(repo.root.join("bound/a.txt")).unwrap(),
            "concurrent\n"
        );
        assert_eq!(
            fs::read_to_string(repo.root.join("bound/b.txt")).unwrap(),
            "base b\n"
        );
    }

    #[test]
    fn rejects_non_git_directory() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        assert!(prepare_mutating_task(&data, "task", temp.path()).is_err());
    }

    #[test]
    fn repository_root_can_be_the_bound_scope() {
        let repo = Repo::new();
        let lease = prepare_mutating_task(&repo.data, "root-task", &repo.root).unwrap();
        fs::write(lease.worker_cwd.join("bound/a.txt"), "root edit\n").unwrap();
        let artifact = finalize_attempt(&lease, "attempt-1").unwrap();
        assert!(matches!(
            verify_and_apply(&lease, &artifact).unwrap(),
            ApplyOutcome::Applied { .. }
        ));
        assert_eq!(
            fs::read_to_string(repo.root.join("bound/a.txt")).unwrap(),
            "root edit\n"
        );
    }

    #[test]
    fn rejected_worktree_is_archived_for_seven_days() {
        let repo = Repo::new();
        let lease = prepare_mutating_task(&repo.data, "task-4", &repo.bound()).unwrap();
        let before = Utc::now() + Duration::days(7);
        let archived = archive_rejected(&lease).unwrap();
        assert!(archived.archive_dir.join("worktree/bound/a.txt").exists());
        assert!(archived.archive_dir.join("lease.json").exists());
        assert!(archived.recover_until >= before);
        assert!(!lease.lease_dir.exists());
    }
}
