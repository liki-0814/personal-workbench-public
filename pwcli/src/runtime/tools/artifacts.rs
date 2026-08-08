use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TOOL_OUTPUT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_READ_CHARS: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutputArtifact {
    pub id: String,
    pub sha256: String,
    pub chars: usize,
    pub created_at: String,
}

pub struct RuntimeArtifactStore;

impl crate::agent_core::contracts::ports::ArtifactStorePort for RuntimeArtifactStore {
    fn store_tool_output(
        &self,
        content: &str,
    ) -> anyhow::Result<crate::agent_core::contracts::ports::StoredArtifact> {
        let artifact = store_tool_output(content)?;
        Ok(crate::agent_core::contracts::ports::StoredArtifact {
            id: artifact.id,
            sha256: artifact.sha256,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ArtifactChunk {
    pub artifact_id: String,
    pub offset: usize,
    pub next_offset: usize,
    pub total_chars: usize,
    pub eof: bool,
    pub content: String,
}

fn root() -> PathBuf {
    crate::runtime::settings::local_config::data_dir()
        .join("artifacts")
        .join("tool-output")
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() != 36 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        bail!("invalid artifact_id");
    }
    Ok(())
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

fn write_private(path: &Path, content: &[u8]) -> Result<()> {
    fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn store_tool_output(content: &str) -> Result<ToolOutputArtifact> {
    let id = uuid::Uuid::now_v7().to_string();
    let dir = root().join(&id);
    create_private_dir(&dir)?;

    let artifact = ToolOutputArtifact {
        id,
        sha256: hex::encode(Sha256::digest(content.as_bytes())),
        chars: content.chars().count(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    write_private(&dir.join("content.txt"), content.as_bytes())?;
    write_private(
        &dir.join("manifest.json"),
        &serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(artifact)
}

pub fn read_tool_output(id: &str, offset: usize, limit: usize) -> Result<ArtifactChunk> {
    validate_id(id)?;
    let root = root();
    let path = root.join(id).join("content.txt");
    if !path.is_file() {
        let chunk = crate::runtime::task::continuation::read_chunk(
            &crate::runtime::settings::local_config::data_dir(),
            id,
            offset,
            limit,
        )?;
        return Ok(ArtifactChunk {
            artifact_id: chunk.artifact_id,
            offset: chunk.offset,
            next_offset: chunk.next_offset,
            total_chars: chunk.total_chars,
            eof: chunk.eof,
            content: chunk.content,
        });
    }
    let canonical_root = fs::canonicalize(&root).context("artifact store is unavailable")?;
    let canonical_path = fs::canonicalize(&path).context("artifact not found or expired")?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!("artifact path escaped artifact store");
    }

    let content = fs::read_to_string(canonical_path).context("artifact is not UTF-8 text")?;
    let total_chars = content.chars().count();
    let offset = offset.min(total_chars);
    let limit = limit.clamp(1, MAX_READ_CHARS);
    let chunk: String = content.chars().skip(offset).take(limit).collect();
    let next_offset = offset + chunk.chars().count();
    Ok(ArtifactChunk {
        artifact_id: id.to_string(),
        offset,
        next_offset,
        total_chars,
        eof: next_offset >= total_chars,
        content: chunk,
    })
}

pub fn cleanup_expired_tool_outputs() -> Result<usize> {
    let root = root();
    let Ok(entries) = fs::read_dir(&root) else {
        return Ok(0);
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let modified = entry.metadata().and_then(|meta| meta.modified());
        if modified
            .ok()
            .and_then(|time| now.duration_since(time).ok())
            .is_some_and(|age| age > TOOL_OUTPUT_TTL)
            && fs::remove_dir_all(path).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_disguised_as_ids() {
        assert!(validate_id("../../etc/passwd").is_err());
        assert!(validate_id("/tmp/pwcli_tool_output.txt").is_err());
    }
}
