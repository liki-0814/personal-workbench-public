// MemoryStore 集成测试：隔离/约束/并发/快照
use pwcli::runtime::memory::store::{MemoryError, MemoryStore, MAX_ENTRY_BYTES, MAX_SUMMARY_CHARS};
use pwcli::runtime::memory::types::{MemoryEntry, MemoryIndex, MemoryIndexLine};
use std::sync::Arc;
use std::thread;

fn make_entry(slug: &str, summary: &str, content: &str) -> MemoryEntry {
    MemoryEntry::new_active(
        slug.to_string(),
        summary.to_string(),
        content.to_string(),
        1_700_000_000,
        None,
    )
}

#[test]
fn user_slugs_are_isolated() {
    let tmp = tempfile::tempdir().unwrap();
    let alice_dir = tmp.path().join("alice");
    let bob_dir = tmp.path().join("bob");
    let alice = MemoryStore::new_with_dir(alice_dir.clone()).unwrap();
    let bob = MemoryStore::new_with_dir(bob_dir.clone()).unwrap();

    alice
        .write_entry_atomic(&make_entry("alpha", "alice secret", "alice body"))
        .unwrap();
    bob.write_entry_atomic(&make_entry("alpha", "bob secret", "bob body"))
        .unwrap();

    let a = alice.read_entry("alpha").unwrap();
    let b = bob.read_entry("alpha").unwrap();
    assert_eq!(a.summary, "alice secret");
    assert_eq!(b.summary, "bob secret");
    assert!(alice_dir.join("entries/alpha.md").exists());
    assert!(bob_dir.join("entries/alpha.md").exists());
}

#[test]
fn rejects_oversized_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(tmp.path().to_path_buf()).unwrap();
    let big = "x".repeat(MAX_ENTRY_BYTES + 1);
    let err = store
        .write_entry_atomic(&make_entry("huge", "ok", &big))
        .unwrap_err();
    match err {
        MemoryError::EntryTooLarge { got, max } => {
            assert!(got > max);
            assert_eq!(max, MAX_ENTRY_BYTES);
        }
        other => panic!("expected EntryTooLarge, got {:?}", other),
    }
    // 没写盘
    assert!(!tmp.path().join("entries/huge.md").exists());
}

#[test]
fn rejects_oversized_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(tmp.path().to_path_buf()).unwrap();
    let long_summary = "汉".repeat(MAX_SUMMARY_CHARS + 1);
    let err = store
        .write_entry_atomic(&make_entry("s", &long_summary, "ok"))
        .unwrap_err();
    match err {
        MemoryError::SummaryTooLong { got, max } => {
            assert_eq!(max, MAX_SUMMARY_CHARS);
            assert!(got > max);
        }
        other => panic!("expected SummaryTooLong, got {:?}", other),
    }
}

#[test]
fn concurrent_writes_do_not_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::new_with_dir(tmp.path().to_path_buf()).unwrap());
    let mut handles = Vec::new();
    for i in 0..5 {
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            let slug = format!("c{}", i);
            let body = format!("payload-{}", i).repeat(20);
            store
                .write_entry_atomic(&make_entry(&slug, "concurrent", &body))
                .unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 所有 5 条都落盘
    let mut slugs = store.list_entries().unwrap();
    slugs.sort();
    assert_eq!(slugs, vec!["c0", "c1", "c2", "c3", "c4"]);
    for slug in &slugs {
        let e = store.read_entry(slug).unwrap();
        assert_eq!(e.summary, "concurrent");
        assert!(e.content.starts_with("payload-"));
    }
}

#[test]
fn snapshot_to_archive_preserves_entries_and_index() {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(tmp.path().to_path_buf()).unwrap();

    store
        .write_entry_atomic(&make_entry("one", "first", "body one"))
        .unwrap();
    store
        .write_entry_atomic(&make_entry("two", "second", "body two"))
        .unwrap();
    let idx = MemoryIndex {
        entries: vec![
            MemoryIndexLine {
                slug: "one".to_string(),
                summary: "first".to_string(),
                updated_at: 1_700_000_000,
            },
            MemoryIndexLine {
                slug: "two".to_string(),
                summary: "second".to_string(),
                updated_at: 1_700_000_001,
            },
        ],
    };
    store.write_index_atomic(&idx).unwrap();

    let dst = store.snapshot_to_archive().unwrap();
    assert!(dst.starts_with(tmp.path().join("archive")));
    assert!(dst.join("MEMORY.md").exists());
    assert!(dst.join("entries/one.md").exists());
    assert!(dst.join("entries/two.md").exists());

    // 快照与原文件内容一致
    let original_one = std::fs::read(tmp.path().join("entries/one.md")).unwrap();
    let snap_one = std::fs::read(dst.join("entries/one.md")).unwrap();
    assert_eq!(original_one, snap_one);
}
