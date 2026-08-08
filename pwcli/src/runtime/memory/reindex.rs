// 全量重建向量索引：entries/*.md 为真源，index.db 为派生层
use anyhow::Result;

use crate::runtime::memory::embed::MemoryEmbedder;
use crate::runtime::memory::index::MemoryIndexDb;
use crate::runtime::memory::store::MemoryStore;

/// 打开 index.db 并从 store 全量 re-embed，返回成功索引条数
pub fn reindex_all(store: &MemoryStore, embedder: &MemoryEmbedder) -> Result<usize> {
    let index = MemoryIndexDb::open(store.base_dir())?;
    index.rebuild_from_store(store, embedder)
}
