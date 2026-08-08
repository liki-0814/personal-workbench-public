//! `~/.pwcli/tmp/pdf/<hash>/` 临时目录管理。
//!
//! - hash 用 sha256(url) 前 8 字节 hex；同 URL 多次调 read_pdf 复用同一目录（避免重复抓）
//! - 内含 `full.md` / `images/` / `.meta.json`；24h 过期由 [`cleanup_expired`] 懒清理
//! - 用户传 `output_dir` 时跳过 tmp，直接落到目标目录（不参与过期清理）

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// `~/.pwcli/tmp/pdf` 根目录
pub fn pdf_tmp_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".pwcli")
        .join("tmp")
        .join("pdf")
}

/// sha256(url) 取前 8 字节 hex（16 字符），作为目录名。
pub fn hash_url(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    hex::encode(&digest[..8])
}

/// 给定 URL 返回 `~/.pwcli/tmp/pdf/<hash>/` 路径（未必存在，调用方负责 mkdir_all）。
pub fn tmp_dir_for_url(url: &str) -> PathBuf {
    pdf_tmp_root().join(hash_url(url))
}

/// 扫 pdf_tmp_root，mtime 超过 24h 的子目录递归删除。failed I/O 静默忽略
/// （懒清理 best-effort，不应阻塞 read_pdf）。
pub async fn cleanup_expired() -> Result<()> {
    let root = pdf_tmp_root();
    if !root.exists() {
        return Ok(());
    }
    let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);
    let root_clone = root.clone();
    // 文件系统操作走 spawn_blocking（read_dir 同步）
    tokio::task::spawn_blocking(move || -> Result<()> {
        let entries = match std::fs::read_dir(&root_clone) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if modified < cutoff {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
        Ok(())
    })
    .await
    .ok();
    Ok(())
}

/// 写 `<out_dir>/.meta.json`，记录 url + fetched_at（ISO8601 UTC）。
pub fn write_meta(out_dir: &Path, url: &str) -> Result<()> {
    let meta = serde_json::json!({
        "url": url,
        "fetched_at": chrono::Utc::now().to_rfc3339(),
    });
    let path = out_dir.join(".meta.json");
    std::fs::write(&path, serde_json::to_string_pretty(&meta)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_8_bytes_hex() {
        let h = hash_url("https://arxiv.org/pdf/2401.12345");
        assert_eq!(h.len(), 16); // 8 bytes -> 16 hex chars
    }

    #[test]
    fn same_url_same_hash() {
        let a = hash_url("https://example.com/x.pdf");
        let b = hash_url("https://example.com/x.pdf");
        assert_eq!(a, b);
    }
}
