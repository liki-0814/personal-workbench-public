// 用户记忆栈：types/store/compactor/extractor/retrieval/hash/embed/index/reindex
pub mod compactor;
pub mod dream;
pub mod embed;
pub mod extractor;
pub mod hash;
pub mod id;
pub mod import;
pub mod index;
pub mod maintenance;
pub mod migrate;
pub mod reindex;
pub mod retrieval;
pub mod search;
pub mod slug;
pub mod store;
pub mod sync_index;
pub mod types;

pub use compactor::{
    maybe_compact_memory, should_auto_compact, MemoryCompactOutcome, ACTIVE_KEEP,
    MEMORY_COMPACT_ENTRY_THRESHOLD, MEMORY_COMPACT_LINE_THRESHOLD, MEMORY_COMPACT_SIZE_THRESHOLD,
};
pub use dream::{maybe_dream, DreamOutcome};
pub use embed::MemoryEmbedder;
pub use extractor::{
    format_turn_messages, maybe_extract_memories, process_pending_memories, queue_memory_candidate,
    turn_messages_slice, MemoryCandidateOutcome, MemoryExtractOutcome,
};
pub use import::{import_markdown_tree, MemoryImportReport};
pub use index::MemoryIndexDb;
pub use maintenance::{schedule_after_turn, schedule_before_compaction};
pub use migrate::{
    collect_stats, collect_stats_with_preread, migrate_store, MemoryMigrateReport, MemoryStats,
};
pub use reindex::reindex_all;
pub use retrieval::grep_search_top_k;
pub use search::{hybrid_search, HybridSearchOptions, MemoryHit};
pub use slug::{resolve_slug, validate_slug};
pub use store::{
    MemoryError, MemoryMeta, MemoryStore, MAX_ENTRY_BYTES, MAX_INDEX_BYTES, MAX_PROFILE_BYTES,
    MAX_SUMMARY_CHARS,
};
pub use types::{MemoryEntry, MemoryIndex, MemoryIndexLine, MemorySource};
