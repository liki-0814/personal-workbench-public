use crate::runtime::memory::store::MemoryStore;

pub fn validate_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// ADD-only slug allocation: reuse `base` when free, else `base_2` … `base_99`, then timestamp suffix.
pub fn resolve_slug(store: &MemoryStore, base: &str) -> String {
    if store.read_entry(base).is_err() {
        return base.to_string();
    }
    for n in 2..=99 {
        let candidate = format!("{}_{}", base, n);
        if store.read_entry(&candidate).is_err() {
            return candidate;
        }
    }
    format!("{}_{}", base, chrono::Utc::now().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::memory::types::{MemoryEntry, MemoryIndexLine};
    use tempfile::tempdir;

    fn write_entry(store: &MemoryStore, slug: &str) {
        let entry =
            MemoryEntry::new_active(slug.to_string(), "s".to_string(), "c".to_string(), 1, None);
        store.write_entry_atomic(&entry).unwrap();
        store
            .upsert_index_line(&MemoryIndexLine {
                slug: slug.to_string(),
                summary: entry.summary,
                updated_at: 1,
            })
            .unwrap();
    }

    #[test]
    fn resolve_slug_free_base() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
        assert_eq!(resolve_slug(&store, "foo"), "foo");
    }

    #[test]
    fn resolve_slug_collision() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
        write_entry(&store, "foo");
        assert_eq!(resolve_slug(&store, "foo"), "foo_2");
        write_entry(&store, "foo_2");
        assert_eq!(resolve_slug(&store, "foo"), "foo_3");
    }
}
