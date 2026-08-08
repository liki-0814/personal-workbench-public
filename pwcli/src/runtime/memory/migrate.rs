// schema v3 迁移：补 UUID，并重写带类型/标签/来源的 frontmatter。
use crate::runtime::memory::store::{MemoryError, MemoryStore};
use crate::runtime::memory::types::CURRENT_SCHEMA_VERSION;

#[derive(Debug, Clone)]
pub struct MemoryMigrateReport {
    pub from_version: u32,
    pub to_version: u32,
    pub ids_assigned: usize,
    pub entries_rewritten: usize,
}

#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub schema_version: u32,
    pub active_entries: usize,
    pub soft_deleted_entries: usize,
    pub archived_entries: usize,
    pub index_bytes: usize,
    pub profile_bytes: usize,
    pub index_lines: usize,
    pub vector_indexed: Option<usize>,
    pub has_profile: bool,
}

pub fn collect_stats(store: &MemoryStore) -> Result<MemoryStats, MemoryError> {
    let index_raw = store.read_index_raw()?;
    let profile = store.read_profile()?;
    collect_stats_with_preread(store, &index_raw, &profile)
}

/// Fast stats collection reusing already-read index/profile strings.
/// Single directory scan counts active vs deleted entries by checking only
/// the frontmatter header (avoids full-file parse).
pub fn collect_stats_with_preread(
    store: &MemoryStore,
    index_raw: &str,
    profile: &str,
) -> Result<MemoryStats, MemoryError> {
    let meta = store.read_meta()?;

    let (mut active, mut deleted) = (0usize, 0usize);
    for slug in store.list_all_entry_slugs()? {
        if store.entry_has_deleted_at(&slug) {
            deleted += 1;
        } else {
            active += 1;
        }
    }

    let archived = store.count_archived_entries()?;
    let index_lines = index_raw
        .lines()
        .filter(|l| l.trim().starts_with("- ["))
        .count();
    let vector_indexed = store.count_vector_index();

    Ok(MemoryStats {
        schema_version: meta.schema_version,
        active_entries: active,
        soft_deleted_entries: deleted,
        archived_entries: archived,
        index_bytes: index_raw.len(),
        profile_bytes: profile.len(),
        index_lines,
        vector_indexed,
        has_profile: !profile.trim().is_empty(),
    })
}

/// 将 store 升级到 `CURRENT_SCHEMA_VERSION`（幂等）。
pub fn migrate_store(store: &MemoryStore) -> Result<MemoryMigrateReport, MemoryError> {
    let mut meta = store.read_meta()?;
    let from = meta.schema_version;
    if from >= CURRENT_SCHEMA_VERSION {
        return Ok(MemoryMigrateReport {
            from_version: from,
            to_version: from,
            ids_assigned: 0,
            entries_rewritten: 0,
        });
    }

    let mut ids_assigned = 0usize;
    let mut entries_rewritten = 0usize;

    for slug in store.list_all_entry_slugs()? {
        let mut entry = store.read_entry_raw(&slug)?;
        let needs_v3_rewrite = from < 3;
        if entry.id.is_none() {
            entry.ensure_id();
            ids_assigned += 1;
        }
        if entry.id.is_some() && (needs_v3_rewrite || from < 2) {
            store.write_entry_atomic(&entry)?;
            entries_rewritten += 1;
        }
    }

    if meta.schema_version < CURRENT_SCHEMA_VERSION {
        meta.schema_version = CURRENT_SCHEMA_VERSION;
        store.write_meta_atomic(&meta)?;
    }

    Ok(MemoryMigrateReport {
        from_version: from,
        to_version: meta.schema_version,
        ids_assigned,
        entries_rewritten,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn migrate_assigns_ids_to_legacy_entries() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            dir.path().join("entries/legacy.md"),
            "---\nslug: legacy\nsummary: old\ncreated_at: 1\nupdated_at: 1\n---\nbody\n",
        )
        .unwrap();
        store
            .upsert_index_line(&crate::runtime::memory::types::MemoryIndexLine {
                slug: "legacy".to_string(),
                summary: "old".to_string(),
                updated_at: 1,
            })
            .unwrap();

        let report = migrate_store(&store).unwrap();
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.ids_assigned, 1);

        let loaded = store.read_entry("legacy").unwrap();
        assert!(loaded.id.is_some());
        assert_eq!(
            store.read_meta().unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
    }
}
