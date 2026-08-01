// 混合检索：grep + 向量 RRF 融合（k=60, w_grep=0.4, w_vec=0.6）
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::memory::embed::MemoryEmbedder;
use crate::memory::index::MemoryIndexDb;
use crate::memory::retrieval::{grep_search_top_k, MemoryHit as GrepHit};
use crate::memory::store::{MemoryError, MemoryStore};

const RRF_K: f64 = 60.0;
const W_GREP: f64 = 0.4;
const W_VEC: f64 = 0.6;
const MMR_LAMBDA: f64 = 0.75;

pub struct HybridSearchOptions {
    pub query: String,
    pub max_results: usize,
    pub exclude_slugs: HashSet<String>,
    /// grep / vector 各取候选 top-N（默认 max(max_results, 10)）
    pub candidate_top_n: Option<usize>,
    /// 是否搜索 archive/<ts>/entries/（仅 grep，无向量）
    pub include_archived: bool,
}

pub struct MemoryHit {
    pub slug: String,
    pub summary: String,
    pub score: f64,
    pub archive_ts: Option<String>,
}

pub fn hybrid_search(
    store: &MemoryStore,
    opts: HybridSearchOptions,
) -> Result<Vec<MemoryHit>, MemoryError> {
    let query = opts.query.trim();
    if query.is_empty() || opts.max_results == 0 {
        return Ok(Vec::new());
    }

    let top_n = opts
        .candidate_top_n
        .unwrap_or_else(|| opts.max_results.max(10));

    let grep_hits = grep_search_top_k(
        store,
        query,
        top_n,
        &opts.exclude_slugs,
        opts.include_archived,
    )?;

    let vector_hits = vector_search_top_n(store.base_dir(), query, top_n, &opts.exclude_slugs);

    rrf_fuse(store, &grep_hits, &vector_hits, opts.max_results)
}

/// index.db 存在时尝试向量检索；embed 失败或索引不可用则返回空（降级为仅 grep）
fn vector_search_top_n(
    base_dir: &Path,
    query: &str,
    top_n: usize,
    exclude_slugs: &HashSet<String>,
) -> Vec<(String, f32)> {
    let db_path = base_dir.join("index.db");
    if !db_path.exists() {
        return Vec::new();
    }

    let embedder = match MemoryEmbedder::new() {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let query_emb = match embedder.embed_query(query) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let index = match MemoryIndexDb::open(base_dir) {
        Ok(i) => i,
        Err(_) => return Vec::new(),
    };

    let hits = match index.search_vector(&query_emb, top_n) {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };

    hits.into_iter()
        .filter(|(slug, _)| !exclude_slugs.contains(slug))
        .collect()
}

fn rrf_fuse(
    store: &MemoryStore,
    grep_hits: &[GrepHit],
    vector_hits: &[(String, f32)],
    max_results: usize,
) -> Result<Vec<MemoryHit>, MemoryError> {
    if grep_hits.is_empty() && vector_hits.is_empty() {
        return Ok(Vec::new());
    }

    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut summaries: HashMap<String, String> = HashMap::new();

    for (rank, hit) in grep_hits.iter().enumerate() {
        let r = rank + 1;
        let key = hit_key(&hit.slug, &hit.archive_ts);
        scores
            .entry(key.clone())
            .and_modify(|s| *s += W_GREP / (RRF_K + r as f64))
            .or_insert(W_GREP / (RRF_K + r as f64));
        summaries.insert(key.clone(), hit.summary.clone());
    }

    for (rank, (slug, _)) in vector_hits.iter().enumerate() {
        let r = rank + 1;
        let key = hit_key(slug, &None);
        scores
            .entry(key.clone())
            .and_modify(|s| *s += W_VEC / (RRF_K + r as f64))
            .or_insert(W_VEC / (RRF_K + r as f64));
    }

    for key in scores.keys() {
        if summaries.contains_key(key) {
            continue;
        }
        if let Some(slug) = key.split('@').next() {
            if let Ok(entry) = store.read_entry(slug) {
                summaries.insert(key.clone(), entry.summary);
            }
        }
    }

    let now = chrono::Utc::now().timestamp();
    for (key, score) in &mut scores {
        if key.contains('@') {
            continue;
        }
        if let Some(slug) = key.split('@').next() {
            if let Ok(entry) = store.read_entry(slug) {
                *score *= recency_factor(&entry, now);
            }
        }
    }
    let mut ranked: Vec<(String, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let ranked = mmr_select(ranked, &summaries, max_results);
    let mut out = Vec::new();
    for (key, score) in ranked {
        let (slug, archive_ts) = parse_hit_key(&key);
        let summary = summaries.get(&key).cloned().unwrap_or_else(|| slug.clone());
        out.push(MemoryHit {
            slug,
            summary,
            score,
            archive_ts,
        });
    }
    Ok(out)
}

fn recency_factor(entry: &crate::memory::types::MemoryEntry, now: i64) -> f64 {
    if entry.tags.iter().any(|tag| tag == "evergreen") || entry.updated_at < 946_684_800 {
        return 1.0;
    }
    let age_days = now.saturating_sub(entry.updated_at) as f64 / 86_400.0;
    (0.5f64).powf(age_days / 365.0).clamp(0.2, 1.0)
}

fn mmr_select(
    mut candidates: Vec<(String, f64)>,
    summaries: &HashMap<String, String>,
    limit: usize,
) -> Vec<(String, f64)> {
    let mut selected: Vec<(String, f64)> = Vec::new();
    while !candidates.is_empty() && selected.len() < limit {
        let max_relevance = candidates
            .iter()
            .map(|(_, score)| *score)
            .fold(f64::EPSILON, f64::max);
        let best = candidates
            .iter()
            .enumerate()
            .map(|(index, (key, score))| {
                let novelty_penalty = selected
                    .iter()
                    .map(|(selected_key, _)| {
                        lexical_similarity(
                            summaries.get(key).map(String::as_str).unwrap_or(key),
                            summaries
                                .get(selected_key)
                                .map(String::as_str)
                                .unwrap_or(selected_key),
                        )
                    })
                    .fold(0.0, f64::max);
                let mmr =
                    MMR_LAMBDA * (*score / max_relevance) - (1.0 - MMR_LAMBDA) * novelty_penalty;
                (index, mmr)
            })
            .max_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        selected.push(candidates.remove(best));
    }
    selected
}

fn lexical_similarity(left: &str, right: &str) -> f64 {
    let tokens = |value: &str| {
        value
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>()
    };
    let left = tokens(left);
    let right = tokens(right);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(&right).count();
    let union = left.union(&right).count();
    intersection as f64 / union as f64
}

fn hit_key(slug: &str, archive_ts: &Option<String>) -> String {
    match archive_ts {
        Some(ts) => format!("{}@{}", slug, ts),
        None => slug.to_string(),
    }
}

fn parse_hit_key(key: &str) -> (String, Option<String>) {
    if let Some((slug, ts)) = key.rsplit_once('@') {
        if !ts.is_empty() && ts.chars().all(|c| c.is_ascii_digit() || c == 'T') {
            return (slug.to_string(), Some(ts.to_string()));
        }
    }
    (key.to_string(), None)
}

/// 供 `tests/memory_upgrade.rs` 验证 RRF 融合逻辑（不依赖 fastembed / index.db）。
#[doc(hidden)]
pub fn fuse_rrf_for_test(
    store: &MemoryStore,
    grep_hits: &[GrepHit],
    vector_hits: &[(String, f32)],
    max_results: usize,
) -> Result<Vec<MemoryHit>, MemoryError> {
    rrf_fuse(store, grep_hits, vector_hits, max_results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{MemoryEntry, MemoryIndexLine};
    use std::path::PathBuf;

    fn isolated_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pwcli_mem_hybrid_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn hybrid_grep_only_when_no_index_db() {
        let dir = isolated_dir();
        let store = MemoryStore::new_with_dir(dir.clone()).unwrap();
        let entry = MemoryEntry::new_active(
            "rust_pref".to_string(),
            "偏好 Rust".to_string(),
            "CLI 用 Rust 写".to_string(),
            1,
            None,
        );
        store.write_entry_atomic(&entry).unwrap();
        store
            .upsert_index_line(&MemoryIndexLine {
                slug: "rust_pref".to_string(),
                summary: entry.summary.clone(),
                updated_at: 1,
            })
            .unwrap();

        let hits = hybrid_search(
            &store,
            HybridSearchOptions {
                query: "Rust".to_string(),
                max_results: 5,
                exclude_slugs: HashSet::new(),
                candidate_top_n: None,
                include_archived: false,
            },
        )
        .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "rust_pref");
        assert!(hits[0].score > 0.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rrf_combines_grep_and_vector_ranks() {
        let dir = isolated_dir();
        let store = MemoryStore::new_with_dir(dir.clone()).unwrap();

        let grep_hits = vec![
            GrepHit {
                slug: "a".to_string(),
                summary: "A".to_string(),
                archive_ts: None,
            },
            GrepHit {
                slug: "b".to_string(),
                summary: "B".to_string(),
                archive_ts: None,
            },
        ];
        let vector_hits = vec![("b".to_string(), 0.9), ("c".to_string(), 0.8)];

        let hits = rrf_fuse(&store, &grep_hits, &vector_hits, 3).unwrap();
        assert_eq!(hits.len(), 3);
        // b 在两路都出现，应排第一
        assert_eq!(hits[0].slug, "b");

        let score_b = W_GREP / (RRF_K + 2.0) + W_VEC / (RRF_K + 1.0);
        let score_a = W_GREP / (RRF_K + 1.0);
        let score_c = W_VEC / (RRF_K + 2.0);
        assert!((hits[0].score - score_b).abs() < 1e-12);
        assert_eq!(hits[1].slug, "c");
        assert!((hits[1].score - score_c).abs() < 1e-12);
        assert_eq!(hits[2].slug, "a");
        assert!((hits[2].score - score_a).abs() < 1e-12);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rrf_grep_only_assigns_rank_scores() {
        let dir = isolated_dir();
        let store = MemoryStore::new_with_dir(dir.clone()).unwrap();
        let grep_hits = vec![GrepHit {
            slug: "solo".to_string(),
            summary: "only grep".to_string(),
            archive_ts: None,
        }];

        let hits = rrf_fuse(&store, &grep_hits, &[], 1).unwrap();
        assert_eq!(hits.len(), 1);
        let expected = W_GREP / (RRF_K + 1.0);
        assert!((hits[0].score - expected).abs() < 1e-12);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evergreen_entries_are_exempt_from_time_decay() {
        let mut entry = MemoryEntry::new_active(
            "old".into(),
            "old".into(),
            "old".into(),
            1_600_000_000,
            None,
        );
        let now = 1_800_000_000;
        assert!(recency_factor(&entry, now) < 1.0);
        entry.tags.push("evergreen".into());
        assert_eq!(recency_factor(&entry, now), 1.0);
    }

    #[test]
    fn mmr_prefers_distinct_evidence_when_relevance_is_close() {
        let candidates = vec![("a".into(), 1.0), ("b".into(), 0.99), ("c".into(), 0.98)];
        let summaries = HashMap::from([
            ("a".into(), "rust cli terminal rendering".into()),
            ("b".into(), "rust cli terminal rendering details".into()),
            ("c".into(), "memory retrieval diversity ranking".into()),
        ]);
        let selected = mmr_select(candidates, &summaries, 2);
        assert_eq!(selected[0].0, "a");
        assert_eq!(selected[1].0, "c");
    }
}
