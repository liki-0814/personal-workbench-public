// 用户长期记忆 4 工具：recall / save / delete / search（同步刷新 MEMORY.md 索引）
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use std::collections::HashSet;

use crate::memory::{
    extractor::MAX_SEEN_HASHES,
    hash::fact_content_hash,
    resolve_slug, sync_index,
    types::{MemoryEntry, MemoryIndexLine},
    HybridSearchOptions, MemoryError, MemoryStore, MAX_ENTRY_BYTES, MAX_SUMMARY_CHARS,
};
use crate::tools::registry::{ToolImpact, ToolRegistry};

pub fn register(registry: &mut ToolRegistry, user_slug: String) {
    register_recall(registry, user_slug.clone());
    register_save(registry, user_slug.clone());
    register_delete(registry, user_slug.clone());
    register_search(registry, user_slug);
}

fn open_store(user_slug: &str) -> Result<MemoryStore> {
    MemoryStore::new(user_slug).map_err(|e| anyhow!("无法打开记忆库: {}", e))
}

fn map_memory_error(e: MemoryError) -> anyhow::Error {
    match e {
        MemoryError::SummaryTooLong { got, max } => {
            anyhow!("summary 超长 ({} chars > {})，请缩短", got, max)
        }
        MemoryError::EntryTooLarge { got, max } => {
            anyhow!("记忆条目超大 ({} bytes > {} bytes)，请精简", got, max)
        }
        MemoryError::IndexFull { got, max } => {
            anyhow!(
                "索引已满 ({} bytes > {} bytes)，请合并或删除旧条目",
                got,
                max
            )
        }
        MemoryError::ProfileTooLarge { got, max } => {
            anyhow!("PROFILE 超大 ({} bytes > {} bytes)，请精简", got, max)
        }
        MemoryError::Io(e) => anyhow!("IO 错误: {}", e),
    }
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn register_recall(registry: &mut ToolRegistry, user_slug: String) {
    registry.register(
        "memory_recall",
        "读取用户长期记忆条目的完整 markdown 内容（按 name/slug 精确匹配）。digest/relevant 摘要已注入 system prompt；不确定时先用 memory_search。",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "条目 slug，必须与 MEMORY.md 索引行中的 [name] 完全一致"
                }
            },
            "required": ["name"]
        }),
        Box::new(move |args: &Value| {
            let slug = user_slug.clone();
            let name = args["name"].as_str().unwrap_or("").trim().to_string();
            Box::pin(async move {
                if name.is_empty() {
                    return Err(anyhow!("memory_recall 需要 name 参数"));
                }
                let store = open_store(&slug)?;
                match store.read_entry(&name) {
                    Ok(entry) => Ok(format!(
                        "# {}\n\n_{}_\n\n{}",
                        entry.slug, entry.summary, entry.content
                    )),
                    Err(MemoryError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                        Err(anyhow!("未找到记忆条目「{}」", name))
                    }
                    Err(e) => Err(map_memory_error(e)),
                }
            })
        }),
    );
}

fn register_save(registry: &mut ToolRegistry, user_slug: String) {
    registry.register_with_impact(
        "memory_save",
        "保存一条用户长期记忆（ADD-only：同名 slug 已存在时会自动分配 foo_2 等新 slug；仅当 name 精确匹配且条目已存在时才允许更新）。跨会话偏好通常会在 turn 结束自动记录；紧急/精确场景用本工具。summary ≤150 字，content ≤16KB markdown。",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "条目 slug：英文+下划线+短横，建议 snake_case（例：feedback_ui_dark_mode）"
                },
                "summary": {
                    "type": "string",
                    "description": format!("一行简介，≤{} 字符；将出现在 MEMORY.md 索引里供未来快速 grep", MAX_SUMMARY_CHARS)
                },
                "content": {
                    "type": "string",
                    "description": format!("详细原文 markdown，≤{} bytes", MAX_ENTRY_BYTES)
                }
            },
            "required": ["name", "summary", "content"]
        }),
        ToolImpact::ReversibleMutation,
        Box::new(move |args: &Value| {
            let slug = user_slug.clone();
            let name = args["name"].as_str().unwrap_or("").trim().to_string();
            let summary = args["summary"].as_str().unwrap_or("").trim().to_string();
            let content = args["content"].as_str().unwrap_or("").to_string();
            Box::pin(async move {
                if name.is_empty() || summary.is_empty() || content.is_empty() {
                    return Err(anyhow!("memory_save 需要 name/summary/content 三参数"));
                }
                let store = open_store(&slug)?;
                let now = now_ts();
                let resolved = resolve_slug(&store, &name);
                let existing = store.read_entry_raw(&resolved).ok();
                let is_same_name_overwrite =
                    resolved == name && existing.as_ref().is_some_and(|e| e.is_active());
                let hash = fact_content_hash(&summary, &content);
                let mut meta = store.read_meta().map_err(map_memory_error)?;
                if meta.seen_content_hashes.contains(&hash) && !is_same_name_overwrite {
                    return Ok("内容重复，已跳过".to_string());
                }
                let supersedes = if resolved != name {
                    Some(name.clone())
                } else {
                    None
                };
                let entry = if is_same_name_overwrite {
                    let mut e = existing.unwrap();
                    e.summary = summary.clone();
                    e.content = content;
                    e.updated_at = now;
                    e.ensure_id();
                    e
                } else {
                    MemoryEntry::new_active(
                        resolved.clone(),
                        summary.clone(),
                        content,
                        now,
                        supersedes,
                    )
                };
                store.write_entry_atomic(&entry).map_err(map_memory_error)?;

                let line = MemoryIndexLine {
                    slug: resolved.clone(),
                    summary,
                    updated_at: now,
                };
                store.upsert_index_line(&line).map_err(map_memory_error)?;
                meta.seen_content_hashes.push(hash);
                if meta.seen_content_hashes.len() > MAX_SEEN_HASHES {
                    let drain = meta.seen_content_hashes.len() - MAX_SEEN_HASHES;
                    meta.seen_content_hashes.drain(0..drain);
                }
                store.write_meta_atomic(&meta).map_err(map_memory_error)?;
                sync_index::best_effort_upsert(&store, &entry);
                if resolved != name {
                    Ok(format!(
                        "已保存记忆条目「{}」（请求的 name「{}」已存在，已自动分配新 slug）",
                        resolved, name
                    ))
                } else if is_same_name_overwrite {
                    Ok(format!("已更新记忆条目「{}」", resolved))
                } else {
                    Ok(format!("已保存记忆条目「{}」", resolved))
                }
            })
        }),
    );
}

fn register_delete(registry: &mut ToolRegistry, user_slug: String) {
    registry.register_with_impact(
        "memory_delete",
        "删除一条用户长期记忆（连带从 MEMORY.md 索引移除）。仅在用户明确要求「忘记/删除/不要再记」时使用。",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "条目 slug，必须与 MEMORY.md 索引中的 name 一致"
                }
            },
            "required": ["name"]
        }),
        ToolImpact::IrreversibleMutation,
        Box::new(move |args: &Value| {
            let slug = user_slug.clone();
            let name = args["name"].as_str().unwrap_or("").trim().to_string();
            Box::pin(async move {
                if name.is_empty() {
                    return Err(anyhow!("memory_delete 需要 name 参数"));
                }
                let store = open_store(&slug)?;
                store
                    .soft_delete_entry(&name, now_ts())
                    .map_err(map_memory_error)?;
                sync_index::best_effort_delete(&store, &name);
                Ok(format!("已软删除记忆条目「{}」", name))
            })
        }),
    );
}

fn register_search(registry: &mut ToolRegistry, user_slug: String) {
    registry.register(
        "memory_search",
        "在用户长期记忆里混合检索（grep + 向量 RRF 融合；无 index.db 或 embed 失败时降级为 grep）。可选 include_archived 搜索归档条目。",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词（中英文皆可）"
                },
                "max_results": {
                    "type": "integer",
                    "description": "最多返回条数，默认 5",
                    "default": 5
                },
                "include_archived": {
                    "type": "boolean",
                    "description": "是否包含 archive/ 下已归档条目，默认 false",
                    "default": false
                }
            },
            "required": ["query"]
        }),
        Box::new(move |args: &Value| {
            let slug = user_slug.clone();
            let query = args["query"].as_str().unwrap_or("").trim().to_string();
            let max_results = args["max_results"].as_u64().unwrap_or(5).max(1) as usize;
            let include_archived = args["include_archived"].as_bool().unwrap_or(false);
            Box::pin(async move {
                if query.is_empty() {
                    return Err(anyhow!("memory_search 需要 query 参数"));
                }
                let query_label = query.clone();
                let store = open_store(&slug)?;
                let hits = crate::memory::hybrid_search(
                    &store,
                    HybridSearchOptions {
                        query,
                        max_results,
                        exclude_slugs: HashSet::new(),
                        candidate_top_n: None,
                        include_archived,
                    },
                )
                .map_err(map_memory_error)?;
                if hits.is_empty() {
                    return Ok(format!("未命中「{}」", query_label));
                }
                let lines: Vec<String> = hits
                    .iter()
                    .map(|hit| {
                        let arch = hit
                            .archive_ts
                            .as_ref()
                            .map(|ts| format!(" @{}", ts))
                            .unwrap_or_default();
                        format!(
                            "- [{}]{} ({:.4}) — {}",
                            hit.slug, arch, hit.score, hit.summary
                        )
                    })
                    .collect();
                Ok(format!(
                    "命中 {} 条（RRF 融合排序）:\n{}",
                    hits.len(),
                    lines.join("\n")
                ))
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryStore;
    use crate::tools::registry::ToolRegistry;
    use tempfile::TempDir;

    struct TestMemoryRoot {
        _dir: TempDir,
    }

    impl TestMemoryRoot {
        fn install() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            MemoryStore::set_test_memory_root(Some(dir.path().to_path_buf()));
            Self { _dir: dir }
        }
    }

    impl Drop for TestMemoryRoot {
        fn drop(&mut self) {
            MemoryStore::set_test_memory_root(None);
        }
    }

    fn isolated_user_slug() -> String {
        format!("test_mem_{}", std::process::id())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn save_and_recall_roundtrip() {
        let _root = TestMemoryRoot::install();
        let slug = isolated_user_slug();
        let mut reg = ToolRegistry::new();
        register(&mut reg, slug.clone());

        let r = reg
            .execute(
                "memory_save",
                &json!({"name": "alpha", "summary": "first", "content": "hello world"}),
            )
            .await
            .unwrap();
        assert!(r.contains("alpha"));

        let recalled = reg
            .execute("memory_recall", &json!({"name": "alpha"}))
            .await
            .unwrap();
        assert!(recalled.contains("hello world"));
        assert!(recalled.contains("first"));

        let deleted = reg
            .execute("memory_delete", &json!({"name": "alpha"}))
            .await
            .unwrap();
        assert!(deleted.contains("软删除"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn save_rejects_long_summary() {
        let _root = TestMemoryRoot::install();
        let slug = isolated_user_slug();
        let mut reg = ToolRegistry::new();
        register(&mut reg, slug.clone());
        let long_summary = "字".repeat(MAX_SUMMARY_CHARS + 5);
        let err = reg
            .execute(
                "memory_save",
                &json!({"name": "x", "summary": long_summary, "content": "ok"}),
            )
            .await
            .unwrap_err();
        let s = err.to_string();
        assert!(s.contains("summary 超长"), "got: {}", s);
    }
}
