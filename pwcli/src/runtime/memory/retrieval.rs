// Grep-based memory retrieval for prompt injection (Phase 1; vector in Phase 2).
use std::collections::HashSet;

use crate::runtime::memory::store::{MemoryError, MemoryStore};

pub struct MemoryHit {
    pub slug: String,
    pub summary: String,
    /// 非 None 表示来自 archive/<ts>/entries/
    pub archive_ts: Option<String>,
}

pub fn grep_search_top_k(
    store: &MemoryStore,
    query: &str,
    max_results: usize,
    exclude_slugs: &HashSet<String>,
    include_archived: bool,
) -> Result<Vec<MemoryHit>, MemoryError> {
    let q = query.trim().to_lowercase();
    if q.is_empty() || max_results == 0 {
        return Ok(Vec::new());
    }

    let mut hits = Vec::new();
    for slug in store.list_active_entries()? {
        if exclude_slugs.contains(&slug) {
            continue;
        }
        let entry = match store.read_entry(&slug) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry_matches_query(&entry, &q) {
            hits.push(MemoryHit {
                slug: entry.slug,
                summary: entry.summary,
                archive_ts: None,
            });
            if hits.len() >= max_results {
                return Ok(hits);
            }
        }
    }

    if include_archived {
        for archived in store.list_archived_entries()? {
            if exclude_slugs.contains(&archived.slug) {
                continue;
            }
            let entry = match MemoryStore::read_archived_entry(&archived.path, &archived.slug) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry_matches_query(&entry, &q) {
                hits.push(MemoryHit {
                    slug: entry.slug,
                    summary: format!("[归档 {}] {}", archived.archive_ts, entry.summary),
                    archive_ts: Some(archived.archive_ts),
                });
                if hits.len() >= max_results {
                    break;
                }
            }
        }
    }
    Ok(hits)
}

fn entry_matches_query(entry: &crate::runtime::memory::types::MemoryEntry, q: &str) -> bool {
    let hay = format!(
        "{}\n{}\n{}\n{}\n{}",
        entry.slug.to_lowercase(),
        entry.summary.to_lowercase(),
        entry.content.to_lowercase(),
        entry.tags.join(" ").to_lowercase(),
        entry
            .sources
            .iter()
            .map(|source| format!("{} {}", source.uri, source.title.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase()
    );
    hay.contains(q)
}

pub fn digest_slugs(digest: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in digest.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("- [") {
            continue;
        }
        if let Some((slug, _)) = parse_index_line_slug(trimmed) {
            out.insert(slug);
        }
    }
    out
}

fn parse_index_line_slug(line: &str) -> Option<(String, String)> {
    let after_dash = line.strip_prefix("- [")?;
    let close_bracket = after_dash.find(']')?;
    let slug = after_dash[..close_bracket].to_string();
    let after_paren = after_dash[close_bracket + 1..].strip_prefix("(")?;
    let close_paren = after_paren.find(')')?;
    let after_paren_close = &after_paren[close_paren + 1..];
    let summary = after_paren_close
        .trim_start()
        .strip_prefix('—')
        .or_else(|| after_paren_close.trim_start().strip_prefix('-'))
        .unwrap_or(after_paren_close)
        .trim()
        .to_string();
    Some((slug, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::memory::types::{MemoryEntry, MemoryIndexLine};
    use crate::runtime::memory::MemoryStore;
    use std::path::PathBuf;

    fn isolated_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pwcli_mem_ret_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn grep_finds_summary_match() {
        let dir = isolated_dir();
        let store = MemoryStore::new_with_dir(dir.clone()).unwrap();
        let entry = MemoryEntry::new_active(
            "dark_mode".to_string(),
            "偏好深色 UI".to_string(),
            "用户喜欢深色模式".to_string(),
            1,
            None,
        );
        store.write_entry_atomic(&entry).unwrap();
        let mut idx = store.read_index().unwrap();
        idx.entries.push(MemoryIndexLine {
            slug: "dark_mode".to_string(),
            summary: entry.summary.clone(),
            updated_at: 1,
        });
        store.write_index_atomic(&idx).unwrap();

        let hits = grep_search_top_k(&store, "深色", 5, &HashSet::new(), false).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "dark_mode");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
