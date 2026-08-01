// memory compactor 集成测试：阈值/每日上限/失败禁用/快照
use chrono::Local;
use pwcli::config::ProviderConfig;
use pwcli::llm::LlmClient;
use pwcli::memory::compactor::{
    maybe_compact_memory, MemoryCompactOutcome, MEMORY_COMPACT_ENTRY_THRESHOLD,
};
use pwcli::memory::store::{MemoryMeta, MemoryStore};
use pwcli::memory::types::{MemoryEntry, MemoryIndex, MemoryIndexLine};
use pwcli::usage::UsageTracker;

const ENOUGH_LINES: usize = MEMORY_COMPACT_ENTRY_THRESHOLD + 1;

fn make_store() -> (tempfile::TempDir, MemoryStore) {
    let tmp = tempfile::tempdir().unwrap();
    let store = MemoryStore::new_with_dir(tmp.path().to_path_buf()).unwrap();
    (tmp, store)
}

fn populate_above_threshold(store: &MemoryStore) {
    let mut idx = MemoryIndex::default();
    for i in 0..ENOUGH_LINES {
        let slug = format!("entry_{:03}", i);
        let entry = MemoryEntry::new_active(
            slug.clone(),
            format!("summary {}", i),
            format!("content body {}", i).repeat(5),
            1_700_000_000 + i as i64,
            None,
        );
        store.write_entry_atomic(&entry).unwrap();
        idx.entries.push(MemoryIndexLine {
            slug,
            summary: format!("summary {}", i),
            updated_at: entry.updated_at,
        });
    }
    store.write_index_atomic(&idx).unwrap();
}

fn dummy_llm(base_url: String) -> LlmClient {
    let provider = ProviderConfig {
        name: "test".to_string(),
        base_url: base_url.clone(),
        api_key: "sk-test".to_string(),
        protocol: "openai".to_string(),
        model: "gpt-4o".to_string(),
        models: Vec::new(),
    };
    LlmClient::with_provider(provider, base_url)
}

#[tokio::test]
async fn skipped_when_below_threshold() {
    let (_tmp, store) = make_store();
    // 只写 5 行，远低于阈值
    let mut idx = MemoryIndex::default();
    for i in 0..5 {
        let slug = format!("e{}", i);
        let entry =
            MemoryEntry::new_active(slug.clone(), format!("s{}", i), format!("c{}", i), 1, None);
        store.write_entry_atomic(&entry).unwrap();
        idx.entries.push(MemoryIndexLine {
            slug,
            summary: format!("s{}", i),
            updated_at: 1,
        });
    }
    store.write_index_atomic(&idx).unwrap();

    let llm = dummy_llm("http://127.0.0.1:1".to_string()); // 不会被调用
    let mut tracker = UsageTracker::new();
    let outcome = maybe_compact_memory(&store, &llm, &mut tracker, false).await;
    match outcome {
        MemoryCompactOutcome::Skipped(msg) => assert!(msg.contains("未达阈值"), "{}", msg),
        other => panic!("expected Skipped, got {:?}", other),
    }
}

#[tokio::test]
async fn skipped_when_daily_quota_exhausted() {
    let (_tmp, store) = make_store();
    populate_above_threshold(&store);
    let today = Local::now().format("%Y-%m-%d").to_string();
    let meta = MemoryMeta {
        schema_version: 1,
        last_auto_compact_date: Some(today),
        ..Default::default()
    };
    store.write_meta_atomic(&meta).unwrap();

    let llm = dummy_llm("http://127.0.0.1:1".to_string()); // 不会被调用
    let mut tracker = UsageTracker::new();
    let outcome = maybe_compact_memory(&store, &llm, &mut tracker, false).await;
    match outcome {
        MemoryCompactOutcome::Skipped(msg) => assert!(msg.contains("daily quota"), "{}", msg),
        other => panic!("expected Skipped, got {:?}", other),
    }
}

#[tokio::test]
async fn three_failures_disable_compaction() {
    use mockito::Server;
    let (_tmp, store) = make_store();
    populate_above_threshold(&store);

    let mut server = Server::new_async().await;
    // 让 3 次调用都返回 500
    let _mock = server
        .mock("POST", "/chat/completions")
        .with_status(500)
        .with_body("boom")
        .expect_at_least(3)
        .create();
    let llm = dummy_llm(server.url());
    let mut tracker = UsageTracker::new();

    for _ in 0..3 {
        // 每次失败后 last_auto_compact_date 没被设置（仅成功才设），所以可以连跑
        let outcome = maybe_compact_memory(&store, &llm, &mut tracker, false).await;
        assert!(matches!(outcome, MemoryCompactOutcome::Failed(_)));
    }

    let meta = store.read_meta().unwrap();
    assert!(meta.failed_compact_count >= 3);
    assert!(
        meta.disabled_until.is_some(),
        "disabled_until 应在 3 次失败后被设置"
    );

    // 第 4 次：进入 disabled 分支
    let outcome = maybe_compact_memory(&store, &llm, &mut tracker, false).await;
    match outcome {
        MemoryCompactOutcome::Skipped(msg) => assert!(msg.contains("禁用"), "{}", msg),
        other => panic!("expected Skipped(禁用), got {:?}", other),
    }
}

#[tokio::test]
async fn snapshot_created_before_llm_call() {
    use mockito::Server;
    let (_tmp, store) = make_store();
    populate_above_threshold(&store);

    let mut server = Server::new_async().await;
    // LLM 返回 500，触发 Failed —— snapshot 仍应在压缩前已经创建
    let _mock = server
        .mock("POST", "/chat/completions")
        .with_status(500)
        .with_body("boom")
        .create();
    let llm = dummy_llm(server.url());
    let mut tracker = UsageTracker::new();

    let _ = maybe_compact_memory(&store, &llm, &mut tracker, false).await;

    let archive_dir = store.base_dir().join("archive");
    let entries: Vec<_> = std::fs::read_dir(&archive_dir).unwrap().collect();
    assert!(!entries.is_empty(), "archive 目录应至少有一个快照子目录");
}
