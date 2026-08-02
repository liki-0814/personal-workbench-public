// 向量索引增量同步：save/delete/extract 后 best-effort 更新 index.db
use crate::memory::embed::MemoryEmbedder;
use crate::memory::index::MemoryIndexDb;
use crate::memory::store::MemoryStore;
use crate::memory::types::MemoryEntry;

pub fn index_db_exists(store: &MemoryStore) -> bool {
    store.base_dir().join("index.db").exists()
}

/// Bootstrap the vector index once per process when memory exists but index.db
/// does not. Historically index creation was coupled to memory compaction, so
/// light users (especially web-only) silently stayed grep-only forever. The
/// first save/extract/import now kicks a background reindex; reindex rebuilds
/// from the store's markdown files, so it naturally includes the entry that
/// triggered the bootstrap. Embeddings stay best-effort: bootstrap failure
/// leaves grep-only search without surfacing an error.
fn maybe_bootstrap_index(store: &MemoryStore) {
    if index_db_exists(store) {
        return;
    }
    static BOOTSTRAP: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if BOOTSTRAP.set(()).is_err() {
        return;
    }
    let base_dir = store.base_dir().to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("memory-index-bootstrap".into())
        .spawn(move || {
            let user_slug = base_dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match MemoryStore::new(&user_slug) {
                Ok(store) => match best_effort_reindex(&store) {
                    Some(count) => {
                        tracing::info!(
                            indexed = count,
                            "memory vector index bootstrapped in background"
                        );
                    }
                    None => {
                        tracing::warn!("memory vector index bootstrap skipped; staying grep-only");
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, "memory vector index bootstrap failed to open store");
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "failed to spawn memory index bootstrap thread");
    }
}

fn passage_text(entry: &MemoryEntry) -> String {
    format!("{}\n{}", entry.summary, entry.content)
}

/// 单条 upsert；embedder/DB 失败时静默跳过（不阻塞文件真源写入）
pub fn best_effort_upsert(store: &MemoryStore, entry: &MemoryEntry) {
    if !index_db_exists(store) {
        // First-ever memory write on a fresh machine: kick one background
        // bootstrap reindex instead of silently skipping vector indexing
        // until the next compaction.
        maybe_bootstrap_index(store);
        return;
    }
    let Ok(embedder) = MemoryEmbedder::new() else {
        return;
    };
    let Ok(index) = MemoryIndexDb::open(store.base_dir()) else {
        return;
    };
    let Ok(embedding) = embedder.embed_text(&passage_text(entry)) else {
        return;
    };
    let _ = index.upsert_entry(entry, &embedding);
}

pub fn best_effort_delete(store: &MemoryStore, slug: &str) {
    if !index_db_exists(store) {
        return;
    }
    let Ok(index) = MemoryIndexDb::open(store.base_dir()) else {
        return;
    };
    let _ = index.delete_slug(slug);
}

/// 全量重建；供 compact / reindex 复用
pub fn best_effort_reindex(store: &MemoryStore) -> Option<usize> {
    let embedder = MemoryEmbedder::new().ok()?;
    let index = MemoryIndexDb::open(store.base_dir()).ok()?;
    index.rebuild_from_store(store, &embedder).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::MemoryEntry;

    fn entry(slug: &str) -> MemoryEntry {
        MemoryEntry::new_active(
            slug.into(),
            format!("summary of {slug}"),
            format!("content of {slug}"),
            1,
            None,
        )
    }

    #[test]
    fn bootstrap_does_not_block_or_corrupt_store_when_index_missing() {
        let dir = tempfile::tempdir().unwrap();
        MemoryStore::set_test_memory_root(Some(dir.path().to_path_buf()));
        let store = MemoryStore::new("bootstrap-user").unwrap();
        assert!(!index_db_exists(&store));
        // Repeated calls must be non-blocking and harmless even though the
        // bootstrap OnceLock only fires once per process.
        best_effort_upsert(&store, &entry("alpha"));
        best_effort_upsert(&store, &entry("beta"));
        assert!(store.base_dir().exists());
        MemoryStore::set_test_memory_root(None);
    }
}
