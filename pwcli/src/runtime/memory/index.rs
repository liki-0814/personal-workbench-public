// 派生向量索引层：index.db 与 entries/*.md 同 base_dir，entries 为真源
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::runtime::memory::embed::MemoryEmbedder;
use crate::runtime::memory::hash::fact_content_hash;
use crate::runtime::memory::store::MemoryStore;
use crate::runtime::memory::types::MemoryEntry;

pub struct MemoryIndexDb {
    db_path: PathBuf,
    conn: Connection,
}

impl MemoryIndexDb {
    /// 在 MemoryStore 同目录打开（或创建）index.db
    pub fn open(base_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(base_dir).context("create memory base dir")?;
        let db_path = base_dir.join("index.db");
        let conn = Connection::open(&db_path).context("open index.db")?;
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS memory_docs (
                slug TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS memory_vec (
                slug TEXT PRIMARY KEY,
                embedding BLOB NOT NULL
            );
            ",
        )
        .context("init index schema")?;
        Ok(Self { db_path, conn })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 写入/更新单条记忆及其向量
    pub fn upsert_entry(&self, entry: &MemoryEntry, embedding: &[f32]) -> Result<()> {
        let content_hash = fact_content_hash(&entry.summary, &entry.content);
        let blob = embedding_to_blob(embedding);
        let tx = self.conn.unchecked_transaction().context("begin tx")?;
        tx.execute(
            "INSERT INTO memory_docs (slug, summary, content, content_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(slug) DO UPDATE SET
               summary = excluded.summary,
               content = excluded.content,
               content_hash = excluded.content_hash,
               created_at = excluded.created_at,
               updated_at = excluded.updated_at",
            params![
                entry.slug,
                entry.summary,
                entry.content,
                content_hash,
                entry.created_at,
                entry.updated_at,
            ],
        )
        .context("upsert memory_docs")?;
        tx.execute(
            "INSERT INTO memory_vec (slug, embedding) VALUES (?1, ?2)
             ON CONFLICT(slug) DO UPDATE SET embedding = excluded.embedding",
            params![entry.slug, blob],
        )
        .context("upsert memory_vec")?;
        tx.commit().context("commit upsert")?;
        Ok(())
    }

    /// 删除索引中的 slug（docs + vec）
    pub fn delete_slug(&self, slug: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction().context("begin tx")?;
        tx.execute("DELETE FROM memory_vec WHERE slug = ?1", params![slug])
            .context("delete memory_vec")?;
        tx.execute("DELETE FROM memory_docs WHERE slug = ?1", params![slug])
            .context("delete memory_docs")?;
        tx.commit().context("commit delete")?;
        Ok(())
    }

    /// 从 MemoryStore 全量重建索引（embed + upsert，清理孤儿 slug）
    pub fn rebuild_from_store(
        &self,
        store: &MemoryStore,
        embedder: &MemoryEmbedder,
    ) -> Result<usize> {
        let slugs = store
            .list_entries()
            .map_err(|e| anyhow::anyhow!("list entries: {}", e))?;
        let mut seen = HashSet::new();
        let mut count = 0usize;

        for slug in &slugs {
            let entry = store
                .read_entry(slug)
                .map_err(|e| anyhow::anyhow!("read entry {}: {}", slug, e))?;
            let text = format!("{}\n{}", entry.summary, entry.content);
            let embedding = embedder.embed_text(&text)?;
            self.upsert_entry(&entry, &embedding)?;
            seen.insert(slug.clone());
            count += 1;
        }

        for orphan in self.list_indexed_slugs()? {
            if !seen.contains(&orphan) {
                self.delete_slug(&orphan)?;
            }
        }

        Ok(count)
    }

    /// 向量检索：纯 Rust cosine similarity，内存扫描 memory_vec 表
    pub fn search_vector(&self, query_emb: &[f32], top_k: usize) -> Result<Vec<(String, f32)>> {
        if top_k == 0 || query_emb.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self
            .conn
            .prepare("SELECT slug, embedding FROM memory_vec")
            .context("prepare vector search")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .context("query memory_vec")?;

        let mut scored = Vec::new();
        for row in rows {
            let (slug, blob) = row.context("read vector row")?;
            let emb = blob_to_embedding(&blob).with_context(|| format!("decode {}", slug))?;
            let score = cosine_similarity(query_emb, &emb);
            scored.push((slug, score));
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    fn list_indexed_slugs(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT slug FROM memory_docs")
            .context("prepare list slugs")?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .context("query memory_docs slugs")?;
        let mut out = Vec::new();
        for slug in rows {
            out.push(slug.context("read slug")?);
        }
        Ok(out)
    }

    pub fn count_docs(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_docs", [], |row| row.get(0))
            .context("count memory_docs")?;
        Ok(count as usize)
    }
}

fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(v.len() * 4);
    for f in v {
        blob.extend_from_slice(&f.to_le_bytes());
    }
    blob
}

fn blob_to_embedding(blob: &[u8]) -> Result<Vec<f32>> {
    if !blob.len().is_multiple_of(4) {
        anyhow::bail!("invalid embedding blob length: {}", blob.len());
    }
    Ok(blob
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::memory::MemoryStore;
    use std::path::PathBuf;

    fn isolated_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pwcli_mem_idx_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn cosine_identical_vectors_score_one() {
        let v = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn upsert_and_search_vector_without_embedder() {
        let dir = isolated_dir();
        let store = MemoryStore::new_with_dir(dir.clone()).unwrap();
        let index = MemoryIndexDb::open(&dir).unwrap();

        let entry = MemoryEntry::new_active(
            "test_slug".to_string(),
            "summary".to_string(),
            "content body".to_string(),
            1,
            None,
        );
        let emb = vec![1.0f32, 0.0, 0.0];
        index.upsert_entry(&entry, &emb).unwrap();

        let hits = index.search_vector(&emb, 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "test_slug");
        assert!((hits[0].1 - 1.0).abs() < 1e-6);

        index.delete_slug("test_slug").unwrap();
        let hits = index.search_vector(&emb, 1).unwrap();
        assert!(hits.is_empty());

        let _ = store;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
