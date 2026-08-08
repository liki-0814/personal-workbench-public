use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_READ_CHARS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationArtifactRef {
    pub artifact_id: String,
    pub sha256: String,
    pub chars: usize,
    pub child_batch_id: String,
    pub join_mode: String,
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationDocumentRef {
    pub document_id: Option<String>,
    pub title: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildTaskOutcome {
    pub task_id: String,
    pub objective: String,
    pub status: String,
    pub output_status: String,
    pub review_status: String,
    pub executor_id: Option<String>,
    pub document: Option<ContinuationDocumentRef>,
    pub result_hash: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationArtifact {
    pub schema_version: u32,
    pub parent_task_id: String,
    pub child_batch_id: String,
    pub join_mode: String,
    pub task_ids: Vec<String>,
    pub outcomes: Vec<ChildTaskOutcome>,
}

#[derive(Debug, Serialize)]
pub struct ContinuationChunk {
    pub artifact_id: String,
    pub offset: usize,
    pub next_offset: usize,
    pub total_chars: usize,
    pub eof: bool,
    pub content: String,
}

fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("artifacts").join("runtime-continuations")
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() != 36 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        bail!("invalid continuation artifact id");
    }
    Ok(())
}

fn artifact_id(hash: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &hash[0..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    )
}

fn set_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &Path, content: &[u8]) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

pub fn store(data_dir: &Path, artifact: &ContinuationArtifact) -> Result<ContinuationArtifactRef> {
    let content = serde_json::to_string(artifact)?;
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    let reference = ContinuationArtifactRef {
        artifact_id: artifact_id(&hash),
        sha256: hash,
        chars: content.chars().count(),
        child_batch_id: artifact.child_batch_id.clone(),
        join_mode: artifact.join_mode.clone(),
        task_ids: artifact.task_ids.clone(),
    };
    let root = root(data_dir);
    set_private_dir(&root)?;
    let final_dir = root.join(&reference.artifact_id);
    if final_dir.is_dir() {
        load(data_dir, &reference)?;
        return Ok(reference);
    }

    let staging = root.join(format!(
        ".{}-{}-{}",
        reference.artifact_id,
        std::process::id(),
        uuid::Uuid::now_v7().simple()
    ));
    set_private_dir(&staging)?;
    let write_result = (|| -> Result<()> {
        write_private(&staging.join("content.json"), content.as_bytes())?;
        write_private(
            &staging.join("manifest.json"),
            &serde_json::to_vec_pretty(&reference)?,
        )?;
        match fs::rename(&staging, &final_dir) {
            Ok(()) => Ok(()),
            Err(_error) if final_dir.is_dir() => {
                let _ = fs::remove_dir_all(&staging);
                load(data_dir, &reference).map(|_| ())
            }
            Err(error) => Err(error.into()),
        }
    })();
    if write_result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    write_result?;
    Ok(reference)
}

pub fn load(data_dir: &Path, reference: &ContinuationArtifactRef) -> Result<ContinuationArtifact> {
    validate_id(&reference.artifact_id)?;
    let (manifest, content) = read_verified_content(data_dir, &reference.artifact_id)?;
    if manifest != *reference {
        bail!("continuation artifact reference does not match manifest");
    }
    let artifact = serde_json::from_str::<ContinuationArtifact>(&content)?;
    if artifact.child_batch_id != reference.child_batch_id
        || artifact.join_mode != reference.join_mode
        || artifact.task_ids != reference.task_ids
    {
        bail!("continuation artifact manifest does not match content");
    }
    Ok(artifact)
}

pub fn read_chunk(
    data_dir: &Path,
    id: &str,
    offset: usize,
    limit: usize,
) -> Result<ContinuationChunk> {
    validate_id(id)?;
    let (_, content) = read_verified_content(data_dir, id)?;
    let total_chars = content.chars().count();
    let offset = offset.min(total_chars);
    let limit = limit.clamp(1, MAX_READ_CHARS);
    let chunk: String = content.chars().skip(offset).take(limit).collect();
    let next_offset = offset + chunk.chars().count();
    Ok(ContinuationChunk {
        artifact_id: id.to_string(),
        offset,
        next_offset,
        total_chars,
        eof: next_offset >= total_chars,
        content: chunk,
    })
}

fn read_verified_content(data_dir: &Path, id: &str) -> Result<(ContinuationArtifactRef, String)> {
    validate_id(id)?;
    let root = root(data_dir);
    let artifact_dir = root.join(id);
    let path = artifact_dir.join("content.json");
    let manifest_path = artifact_dir.join("manifest.json");
    let canonical_root =
        fs::canonicalize(&root).context("continuation artifact store is unavailable")?;
    let canonical_path = fs::canonicalize(&path).context("continuation artifact not found")?;
    let canonical_manifest =
        fs::canonicalize(&manifest_path).context("continuation artifact manifest not found")?;
    if !canonical_path.starts_with(&canonical_root)
        || !canonical_manifest.starts_with(&canonical_root)
    {
        bail!("continuation artifact escaped artifact store");
    }
    let manifest =
        serde_json::from_slice::<ContinuationArtifactRef>(&fs::read(canonical_manifest)?)?;
    if manifest.artifact_id != id {
        bail!("continuation artifact manifest id does not match path");
    }
    let content =
        fs::read_to_string(canonical_path).context("continuation artifact is not UTF-8")?;
    if hex::encode(Sha256::digest(content.as_bytes())) != manifest.sha256
        || content.chars().count() != manifest.chars
    {
        bail!("continuation artifact integrity check failed");
    }
    Ok((manifest, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ContinuationArtifact {
        ContinuationArtifact {
            schema_version: 1,
            parent_task_id: "parent".to_string(),
            child_batch_id: "batch".to_string(),
            join_mode: "all".to_string(),
            task_ids: vec!["child".to_string()],
            outcomes: vec![ChildTaskOutcome {
                task_id: "child".to_string(),
                objective: "research".to_string(),
                status: "succeeded".to_string(),
                output_status: "ready".to_string(),
                review_status: "not_required".to_string(),
                executor_id: Some("pwcli".to_string()),
                document: Some(ContinuationDocumentRef {
                    document_id: Some("document".to_string()),
                    title: Some("Result".to_string()),
                    sha256: Some("document-hash".to_string()),
                }),
                result_hash: Some("result-hash".to_string()),
                result: Some(serde_json::json!({
                    "summary": "complete result",
                    "changedFiles": [{ "path": "src/lib.rs", "status": "modified" }],
                    "verification": [{ "label": "test", "status": "passed" }],
                })),
                error: None,
            }],
        }
    }

    #[test]
    fn content_addressed_store_is_idempotent_and_verified() {
        let directory = tempfile::tempdir().unwrap();
        let first = store(directory.path(), &sample()).unwrap();
        let second = store(directory.path(), &sample()).unwrap();
        assert_eq!(first, second);
        assert_eq!(load(directory.path(), &first).unwrap().outcomes.len(), 1);
    }

    #[test]
    fn continuation_can_be_read_in_bounded_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let reference = store(directory.path(), &sample()).unwrap();
        let first = read_chunk(directory.path(), &reference.artifact_id, 0, 20).unwrap();
        assert_eq!(first.content.chars().count(), 20);
        assert!(!first.eof);
        let rest = read_chunk(
            directory.path(),
            &reference.artifact_id,
            first.next_offset,
            usize::MAX,
        )
        .unwrap();
        assert!(rest.eof || rest.next_offset < rest.total_chars);
    }

    #[test]
    fn rejects_tampered_content_for_full_and_chunked_reads() {
        let directory = tempfile::tempdir().unwrap();
        let reference = store(directory.path(), &sample()).unwrap();
        fs::write(
            root(directory.path())
                .join(&reference.artifact_id)
                .join("content.json"),
            b"{}",
        )
        .unwrap();

        assert!(load(directory.path(), &reference).is_err());
        assert!(read_chunk(directory.path(), &reference.artifact_id, 0, 20).is_err());
    }
}
