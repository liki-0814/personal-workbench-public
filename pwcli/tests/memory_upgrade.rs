use std::collections::HashSet;

use pwcli::app::cli::commands::{parse_memory_injection_mode, MemoryInjectionMode};
use pwcli::runtime::memory::compactor::{select_slugs_to_archive, ACTIVE_KEEP};
use pwcli::runtime::memory::extractor::format_turn_messages;
use pwcli::runtime::memory::hash::{fact_content_hash, normalize_for_hash};
use pwcli::runtime::memory::search::{fuse_rrf_for_test, hybrid_search, HybridSearchOptions};
use pwcli::runtime::memory::types::MemoryIndexLine;
use pwcli::runtime::memory::{MemoryEntry, MemoryStore};
use pwcli::runtime::session::ConversationMessage;

const RRF_K: f64 = 60.0;
const W_GREP: f64 = 0.4;
const W_VEC: f64 = 0.6;

fn grep_rrf(rank_1based: usize) -> f64 {
    W_GREP / (RRF_K + rank_1based as f64)
}

fn vec_rrf(rank_1based: usize) -> f64 {
    W_VEC / (RRF_K + rank_1based as f64)
}

#[test]
fn injection_mode_parsing() {
    assert_eq!(
        parse_memory_injection_mode("full"),
        MemoryInjectionMode::Full
    );
    assert_eq!(parse_memory_injection_mode("off"), MemoryInjectionMode::Off);
    assert_eq!(
        parse_memory_injection_mode("retrieval"),
        MemoryInjectionMode::Retrieval
    );
}

#[test]
fn build_digest_limits_lines() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
    for i in 0..15 {
        let slug = format!("entry_{:02}", i);
        let entry = MemoryEntry::new_active(
            slug.clone(),
            format!("summary {}", i),
            format!("content {}", i),
            i,
            None,
        );
        store.write_entry_atomic(&entry).unwrap();
        store
            .upsert_index_line(&MemoryIndexLine {
                slug,
                summary: entry.summary,
                updated_at: i,
            })
            .unwrap();
    }

    let digest = store.build_digest(1024, 10).unwrap();
    assert!(!digest.is_empty());
    assert!(digest.contains("entry_14"));
    assert!(digest.lines().count() <= 10);
}

#[test]
fn hash_normalization() {
    assert_eq!(normalize_for_hash("  a   b "), "a b");
    let h1 = fact_content_hash("s", "hello");
    let h2 = fact_content_hash("s", "hello ");
    assert_eq!(h1, h2);
}

#[test]
fn select_slugs_to_archive_keeps_newest() {
    let input: Vec<(String, i64)> = (0..45).map(|i| (format!("e{:02}", i), i as i64)).collect();
    let (keep, archive) = select_slugs_to_archive(&input, ACTIVE_KEEP);
    assert_eq!(keep.len(), ACTIVE_KEEP);
    assert_eq!(archive.len(), 5);
    assert!(keep.contains(&"e44".to_string()));
    assert!(archive.contains(&"e00".to_string()));
}

#[test]
fn format_turn_messages_roundtrip() {
    let msgs = vec![
        ConversationMessage::new_user("以后默认用 Rust 写 CLI 工具"),
        ConversationMessage::new_assistant("好的，我会记住这个偏好"),
    ];
    let text = format_turn_messages(&msgs).unwrap();
    assert!(text.contains("Rust"));
}

#[test]
fn hybrid_search_grep_only_without_index_db() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        !dir.path().join("index.db").exists(),
        "fixture should not ship an index.db"
    );

    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
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
    assert_eq!(hits[0].summary, "偏好 Rust");
    assert!((hits[0].score - grep_rrf(1)).abs() < 1e-12);
}

#[test]
fn hybrid_search_empty_query_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();

    let hits = hybrid_search(
        &store,
        HybridSearchOptions {
            query: "   ".to_string(),
            max_results: 5,
            exclude_slugs: HashSet::new(),
            candidate_top_n: None,
            include_archived: false,
        },
    )
    .unwrap();

    assert!(hits.is_empty());
}

#[test]
fn rrf_fuse_overlap_slug_ranks_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();

    use pwcli::runtime::memory::retrieval::MemoryHit as GrepHit;
    let grep_hits = vec![
        GrepHit {
            slug: "alpha".to_string(),
            summary: "A".to_string(),
            archive_ts: None,
        },
        GrepHit {
            slug: "both".to_string(),
            summary: "Both".to_string(),
            archive_ts: None,
        },
    ];
    let vector_hits = vec![("both".to_string(), 0.95), ("gamma".to_string(), 0.8)];

    let hits = fuse_rrf_for_test(&store, &grep_hits, &vector_hits, 3).unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].slug, "both");

    let expected_both = grep_rrf(2) + vec_rrf(1);
    assert!((hits[0].score - expected_both).abs() < 1e-12);
    assert_eq!(hits[1].slug, "gamma");
    assert!((hits[1].score - vec_rrf(2)).abs() < 1e-12);
    assert_eq!(hits[2].slug, "alpha");
    assert!((hits[2].score - grep_rrf(1)).abs() < 1e-12);
}

#[test]
fn soft_delete_hides_from_active_list() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
    let entry = MemoryEntry::new_active(
        "gone".to_string(),
        "will delete".to_string(),
        "secret".to_string(),
        1,
        None,
    );
    store.write_entry_atomic(&entry).unwrap();
    store
        .upsert_index_line(&MemoryIndexLine {
            slug: "gone".to_string(),
            summary: entry.summary.clone(),
            updated_at: 1,
        })
        .unwrap();

    store.soft_delete_entry("gone", 99).unwrap();
    assert!(store.list_active_entries().unwrap().is_empty());
    assert_eq!(store.list_soft_deleted_entries().unwrap(), vec!["gone"]);
    assert!(store.read_entry("gone").is_err());
}

#[test]
fn archived_entries_searchable_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
    let archive_dir = dir.path().join("archive").join("20250101T000000");
    std::fs::create_dir_all(archive_dir.join("entries")).unwrap();
    let archived = MemoryEntry::new_active(
        "old_pref".to_string(),
        "旧偏好".to_string(),
        "深色模式".to_string(),
        1,
        None,
    );
    let body = format!(
        "---\nslug: {}\nsummary: {}\ncreated_at: 1\nupdated_at: 1\n---\n{}",
        archived.slug, archived.summary, archived.content
    );
    std::fs::write(archive_dir.join("entries").join("old_pref.md"), body).unwrap();

    let without = hybrid_search(
        &store,
        HybridSearchOptions {
            query: "深色".to_string(),
            max_results: 5,
            exclude_slugs: HashSet::new(),
            candidate_top_n: None,
            include_archived: false,
        },
    )
    .unwrap();
    assert!(without.is_empty());

    let with = hybrid_search(
        &store,
        HybridSearchOptions {
            query: "深色".to_string(),
            max_results: 5,
            exclude_slugs: HashSet::new(),
            candidate_top_n: None,
            include_archived: true,
        },
    )
    .unwrap();
    assert_eq!(with.len(), 1);
    assert_eq!(with[0].slug, "old_pref");
    assert_eq!(with[0].archive_ts.as_deref(), Some("20250101T000000"));
}

#[test]
fn migrate_assigns_uuid() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.path().join("entries/legacy.md"),
        "---\nslug: legacy\nsummary: s\ncreated_at: 1\nupdated_at: 1\n---\nc\n",
    )
    .unwrap();
    let report = pwcli::runtime::memory::migrate_store(&store).unwrap();
    assert_eq!(report.ids_assigned, 1);
    assert!(store.read_entry_raw("legacy").unwrap().id.is_some());
}

#[test]
fn hybrid_search_respects_exclude_slugs() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
    for (slug, needle) in [("keep_me", "needle"), ("skip_me", "needle")] {
        let entry = MemoryEntry::new_active(
            slug.to_string(),
            format!("summary {slug}"),
            format!("content {needle}"),
            1,
            None,
        );
        store.write_entry_atomic(&entry).unwrap();
        store
            .upsert_index_line(&MemoryIndexLine {
                slug: slug.to_string(),
                summary: entry.summary.clone(),
                updated_at: 1,
            })
            .unwrap();
    }

    let mut exclude = HashSet::new();
    exclude.insert("skip_me".to_string());

    let hits = hybrid_search(
        &store,
        HybridSearchOptions {
            query: "needle".to_string(),
            max_results: 5,
            exclude_slugs: exclude,
            candidate_top_n: None,
            include_archived: false,
        },
    )
    .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].slug, "keep_me");
}
