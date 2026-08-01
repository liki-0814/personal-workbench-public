use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::memory::store::{MemoryError, MemoryStore};
use crate::memory::types::{MemoryEntry, MemoryIndexLine, MemorySource};

// MemoryStore caps the serialized entry at 16 KiB. Leave enough room for
// provenance frontmatter and multi-byte path/title metadata.
const CHUNK_BYTES: usize = 12 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryImportReport {
    pub files_scanned: usize,
    pub chunks_imported: usize,
    pub chunks_updated: usize,
    pub chunks_unchanged: usize,
    pub files_skipped: usize,
}

/// Import a Markdown tree into the memory repository without modifying the
/// source. Stable slugs + content hashes make the operation idempotent.
pub fn import_markdown_tree(
    store: &MemoryStore,
    root: &Path,
) -> Result<MemoryImportReport, MemoryError> {
    let mut files = Vec::new();
    let base = if root.is_file() {
        root.parent().unwrap_or_else(|| Path::new("."))
    } else {
        root
    };
    collect_markdown_files(root, &mut files)?;
    files.sort();
    let mut report = MemoryImportReport::default();

    for path in files {
        report.files_scanned += 1;
        let raw = match fs::read_to_string(&path) {
            Ok(raw) if !raw.trim().is_empty() => raw,
            _ => {
                report.files_skipped += 1;
                continue;
            }
        };
        let relative = path.strip_prefix(base).unwrap_or(&path);
        let title = markdown_title(&raw).unwrap_or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("document")
                .to_string()
        });
        let chunks = chunk_markdown(&raw, CHUNK_BYTES);
        for (index, content) in chunks.iter().enumerate() {
            let slug = source_slug(relative, index, chunks.len());
            let content_hash = sha256(content);
            let source = MemorySource {
                kind: "markdown".to_string(),
                uri: path.to_string_lossy().to_string(),
                title: Some(title.clone()),
                content_hash: Some(content_hash.clone()),
                captured_at: Some(chrono::Utc::now().timestamp()),
            };
            let summary = if chunks.len() == 1 {
                title.clone()
            } else {
                format!("{}（第 {}/{} 段）", title, index + 1, chunks.len())
            };

            match store.read_entry(&slug) {
                Ok(existing)
                    if existing.sources.iter().any(|existing_source| {
                        existing_source.uri == source.uri
                            && existing_source.content_hash.as_deref()
                                == Some(content_hash.as_str())
                    }) =>
                {
                    store.upsert_index_line(&MemoryIndexLine {
                        slug,
                        summary,
                        updated_at: existing.updated_at,
                    })?;
                    report.chunks_unchanged += 1;
                }
                Ok(mut existing) => {
                    existing.summary = summary.clone();
                    existing.content = content.clone();
                    existing.updated_at = chrono::Utc::now().timestamp();
                    existing.kind = "knowledge".to_string();
                    existing.tags = vec!["markdown-import".to_string()];
                    existing.sources = vec![source];
                    store.write_entry_atomic(&existing)?;
                    store.upsert_index_line(&MemoryIndexLine {
                        slug,
                        summary,
                        updated_at: existing.updated_at,
                    })?;
                    report.chunks_updated += 1;
                }
                Err(MemoryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    let now = chrono::Utc::now().timestamp();
                    let mut entry = MemoryEntry::new_active(
                        slug.clone(),
                        summary.clone(),
                        content.clone(),
                        now,
                        None,
                    )
                    .with_source(source);
                    entry.tags.push("markdown-import".to_string());
                    store.write_entry_atomic(&entry)?;
                    store.upsert_index_line(&MemoryIndexLine {
                        slug,
                        summary,
                        updated_at: now,
                    })?;
                    report.chunks_imported += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(report)
}

fn collect_markdown_files(current: &Path, output: &mut Vec<PathBuf>) -> Result<(), MemoryError> {
    if !current.exists() {
        return Err(MemoryError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("memory import path not found: {}", current.display()),
        )));
    }
    let metadata = fs::symlink_metadata(current)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        if current
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            output.push(current.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || entry.file_type()?.is_symlink() {
            continue;
        }
        if path.is_dir() {
            collect_markdown_files(&path, output)?;
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            output.push(path);
        }
    }
    Ok(())
}

fn markdown_title(raw: &str) -> Option<String> {
    raw.lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

fn chunk_markdown(raw: &str, max_bytes: usize) -> Vec<String> {
    if raw.len() <= max_bytes {
        return vec![raw.trim().to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for block in raw.split_inclusive("\n\n") {
        if !current.is_empty() && current.len() + block.len() > max_bytes {
            chunks.push(current.trim().to_string());
            current.clear();
        }
        if block.len() > max_bytes {
            for ch in block.chars() {
                if current.len() + ch.len_utf8() > max_bytes {
                    chunks.push(current.trim().to_string());
                    current.clear();
                }
                current.push(ch);
            }
        } else {
            current.push_str(block);
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}

fn source_slug(relative: &Path, chunk_index: usize, chunk_count: usize) -> String {
    let stem = relative
        .with_extension("")
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let digest = sha256(&relative.to_string_lossy());
    let suffix = if chunk_count > 1 {
        format!("_p{}", chunk_index + 1)
    } else {
        String::new()
    };
    format!(
        "markdown_{}_{}{}",
        stem.chars().take(40).collect::<String>(),
        &digest[..10],
        suffix
    )
}

fn sha256(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_is_idempotent_and_tracks_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        fs::create_dir_all(docs.join("notes")).unwrap();
        fs::write(docs.join("notes/alpha.md"), "# Alpha\n\nimportant fact").unwrap();
        let store = MemoryStore::new_with_dir(dir.path().join("memory")).unwrap();

        let first = import_markdown_tree(&store, &docs).unwrap();
        assert_eq!(first.chunks_imported, 1);
        let second = import_markdown_tree(&store, &docs).unwrap();
        assert_eq!(second.chunks_unchanged, 1);

        let slug = store.list_active_entries().unwrap().pop().unwrap();
        let entry = store.read_entry(&slug).unwrap();
        assert_eq!(entry.kind, "knowledge");
        assert_eq!(entry.tags, vec!["markdown-import"]);
        assert_eq!(entry.sources[0].kind, "markdown");
        assert!(entry.sources[0].content_hash.is_some());
    }

    #[test]
    fn imports_a_single_markdown_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one.md");
        fs::write(&path, "# One\n\nstandalone").unwrap();
        let store = MemoryStore::new_with_dir(dir.path().join("memory")).unwrap();

        let report = import_markdown_tree(&store, &path).unwrap();
        assert_eq!(report.files_scanned, 1);
        assert_eq!(report.chunks_imported, 1);
        assert!(store.list_active_entries().unwrap()[0].starts_with("markdown_one_"));
    }

    #[cfg(unix)]
    #[test]
    fn skips_hidden_paths_and_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("visible.md"), "# Visible").unwrap();
        fs::write(dir.path().join(".hidden.md"), "# Hidden").unwrap();
        symlink(dir.path().join("visible.md"), dir.path().join("linked.md")).unwrap();
        let store = MemoryStore::new_with_dir(dir.path().join("memory")).unwrap();

        let report = import_markdown_tree(&store, dir.path()).unwrap();
        assert_eq!(report.files_scanned, 1);
        assert_eq!(report.chunks_imported, 1);
    }

    #[test]
    fn large_markdown_is_split_without_losing_unicode() {
        let raw = format!("# 大文档\n\n{}", "知识段落。".repeat(20_000));
        let chunks = chunk_markdown(&raw, 1024);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 1024));
    }
}
