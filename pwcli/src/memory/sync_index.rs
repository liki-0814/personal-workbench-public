// 向量索引增量同步：save/delete/extract 后 best-effort 更新 index.db
use crate::memory::embed::MemoryEmbedder;
use crate::memory::index::MemoryIndexDb;
use crate::memory::store::MemoryStore;
use crate::memory::types::MemoryEntry;

pub fn index_db_exists(store: &MemoryStore) -> bool {
    store.base_dir().join("index.db").exists()
}

fn passage_text(entry: &MemoryEntry) -> String {
    format!("{}\n{}", entry.summary, entry.content)
}

/// 单条 upsert；embedder/DB 失败时静默跳过（不阻塞文件真源写入）
pub fn best_effort_upsert(store: &MemoryStore, entry: &MemoryEntry) {
    if !index_db_exists(store) {
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
