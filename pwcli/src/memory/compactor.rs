// 用户长期记忆自动压缩 v2：归档旧条目 + LLM 生成 PROFILE.md
use crate::llm::{summarize_via_llm_limited, LlmClient, SummarizeError};
use crate::memory::store::{MemoryMeta, MemoryStore};
use crate::memory::types::MemoryEntry;
use crate::usage::UsageTracker;
use chrono::Local;

pub const MEMORY_COMPACT_SIZE_THRESHOLD: usize = 512 * 1024;
pub const MEMORY_COMPACT_LINE_THRESHOLD: usize = 80;
pub const MEMORY_COMPACT_ENTRY_THRESHOLD: usize = 80;
pub const ACTIVE_KEEP: usize = 40;
pub const ACTIVE_SIZE_KEEP: usize = 256 * 1024;
const PROFILE_MAX_TOKENS: u32 = 2048;
const FAILURE_DISABLE_THRESHOLD: u32 = 3;
const DISABLE_DURATION_SECS: i64 = 30 * 60;

#[derive(Debug)]
pub enum MemoryCompactOutcome {
    Done {
        archived_count: usize,
        summary_chars: usize,
    },
    Skipped(String),
    Failed(String),
}

const PROFILE_SYSTEM_PROMPT: &str = r#"你是用户长期记忆的整理专家。我会给你当前保留的记忆条目原文。
请输出一份用户画像 PROFILE（纯 markdown，不要 JSON、不要代码块包裹），涵盖：
- 身份与背景（职业、技术栈、常用工具）
- 偏好与习惯（编码风格、沟通方式、工作流）
- 长期目标与项目上下文
- 重要决策与约束

要求：
- 使用简洁的中文 markdown（可用 ## 小节标题）
- 合并重复信息，去掉过时细节
- 总长度不超过 3500 字符
- 只输出 PROFILE 正文，不要前言或解释"#;

pub async fn maybe_compact_memory(
    store: &MemoryStore,
    llm: &LlmClient,
    tracker: &mut UsageTracker,
    force: bool,
) -> MemoryCompactOutcome {
    let raw = match store.read_index_raw() {
        Ok(s) => s,
        Err(e) => return MemoryCompactOutcome::Failed(format!("读取索引失败: {}", e)),
    };
    let line_count = raw.lines().filter(|l| l.trim().starts_with("- [")).count();
    let entry_count = match store.list_entries() {
        Ok(v) => v.len(),
        Err(e) => return MemoryCompactOutcome::Failed(format!("list_entries 失败: {}", e)),
    };
    let active_size = match store.active_entries_total_size() {
        Ok(size) => size,
        Err(e) => return MemoryCompactOutcome::Failed(format!("统计记忆大小失败: {}", e)),
    };

    if !force
        && active_size < MEMORY_COMPACT_SIZE_THRESHOLD
        && line_count < MEMORY_COMPACT_LINE_THRESHOLD
        && entry_count < MEMORY_COMPACT_ENTRY_THRESHOLD
    {
        return MemoryCompactOutcome::Skipped(format!(
            "活跃记忆未达阈值 (size {}/{} bytes, lines {}/{}, entries {}/{})",
            active_size,
            MEMORY_COMPACT_SIZE_THRESHOLD,
            line_count,
            MEMORY_COMPACT_LINE_THRESHOLD,
            entry_count,
            MEMORY_COMPACT_ENTRY_THRESHOLD
        ));
    }

    let now = Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let mut meta = match store.read_meta() {
        Ok(m) => m,
        Err(e) => return MemoryCompactOutcome::Failed(format!("读取 meta 失败: {}", e)),
    };

    if !force {
        if let Some(disabled_until) = meta.disabled_until {
            if now.timestamp() < disabled_until {
                return MemoryCompactOutcome::Skipped(format!(
                    "压缩临时禁用至 {}（连续失败保护）",
                    disabled_until
                ));
            }
        }
        if meta
            .last_auto_compact_date
            .as_ref()
            .map(|d| d == &today)
            .unwrap_or(false)
        {
            return MemoryCompactOutcome::Skipped(
                "daily quota exhausted, use /memory compact for manual run".to_string(),
            );
        }
    }

    let entry_slugs = match store.list_entries() {
        Ok(v) => v,
        Err(e) => return MemoryCompactOutcome::Failed(format!("list_entries 失败: {}", e)),
    };
    if entry_slugs.is_empty() {
        return MemoryCompactOutcome::Skipped("无条目可压缩".to_string());
    }

    let mut entry_stats: Vec<(String, i64, usize)> = Vec::new();
    let mut retained_entries: Vec<MemoryEntry> = Vec::new();
    for slug in &entry_slugs {
        match store.read_entry(slug) {
            Ok(entry) => {
                let size = match store.active_entry_size(slug) {
                    Ok(size) => size,
                    Err(e) => {
                        return MemoryCompactOutcome::Failed(format!(
                            "统计条目 {} 大小失败: {}",
                            slug, e
                        ));
                    }
                };
                entry_stats.push((entry.slug.clone(), entry.updated_at, size));
                retained_entries.push(entry);
            }
            Err(e) => {
                return MemoryCompactOutcome::Failed(format!("读取条目 {} 失败: {}", slug, e));
            }
        }
    }

    let (keep_slugs, archive_slugs) =
        select_slugs_to_archive_by_limits(&entry_stats, ACTIVE_KEEP, ACTIVE_SIZE_KEEP);
    if archive_slugs.is_empty() {
        return MemoryCompactOutcome::Skipped("没有超过低水位的条目可归档".to_string());
    }

    let ts_dir = match store.snapshot_to_archive() {
        Ok(d) => d,
        Err(e) => return MemoryCompactOutcome::Failed(format!("snapshot 失败: {}", e)),
    };

    retained_entries.retain(|entry| keep_slugs.contains(&entry.slug));

    let mut entries_text = String::new();
    for entry in &retained_entries {
        entries_text.push_str(&format!("\n## {}\n", entry.slug));
        entries_text.push_str(&format!("_{}_\n\n", entry.summary));
        entries_text.push_str(&entry.content);
        entries_text.push('\n');
    }

    let profile_text = match summarize_via_llm_limited(
        &entries_text,
        PROFILE_SYSTEM_PROMPT,
        llm,
        tracker,
        Some(PROFILE_MAX_TOKENS),
    )
    .await
    {
        Ok(s) => s,
        Err(SummarizeError::EmptyResponse) => {
            record_failure(store, &mut meta, &now);
            return MemoryCompactOutcome::Failed("AI 返回空响应".to_string());
        }
        Err(SummarizeError::LlmError(e)) => {
            record_failure(store, &mut meta, &now);
            return MemoryCompactOutcome::Failed(format!("LLM 失败: {}", e));
        }
    };

    let archived_count = match store.move_entries_to_archive(&archive_slugs, &ts_dir) {
        Ok(n) => n,
        Err(e) => {
            record_failure(store, &mut meta, &now);
            return MemoryCompactOutcome::Failed(format!("归档条目失败: {}", e));
        }
    };

    if let Err(e) = store.write_profile_atomic(&profile_text) {
        record_failure(store, &mut meta, &now);
        return MemoryCompactOutcome::Failed(format!("写 PROFILE 失败: {}", e));
    }

    let _ = crate::memory::sync_index::best_effort_reindex(store);

    meta.last_compact_at = Some(now.timestamp());
    meta.last_auto_compact_date = Some(today);
    meta.failed_compact_count = 0;
    meta.disabled_until = None;
    if let Err(e) = store.write_meta_atomic(&meta) {
        return MemoryCompactOutcome::Failed(format!("写 meta 失败: {}", e));
    }

    MemoryCompactOutcome::Done {
        archived_count,
        summary_chars: profile_text.chars().count(),
    }
}

/// 按 updated_at 降序保留最新 `keep` 条，其余 slug 待归档。
pub fn select_slugs_to_archive(
    slug_timestamps: &[(String, i64)],
    keep: usize,
) -> (Vec<String>, Vec<String>) {
    let mut sorted: Vec<(String, i64)> = slug_timestamps.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let keep_slugs: Vec<String> = sorted.iter().take(keep).map(|(s, _)| s.clone()).collect();
    let archive_slugs: Vec<String> = sorted.iter().skip(keep).map(|(s, _)| s.clone()).collect();
    (keep_slugs, archive_slugs)
}

pub fn select_slugs_to_archive_by_limits(
    entries: &[(String, i64, usize)],
    keep_entries: usize,
    keep_bytes: usize,
) -> (Vec<String>, Vec<String>) {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut kept_bytes = 0usize;
    let mut keep = Vec::new();
    let mut archive = Vec::new();
    let mut limit_reached = false;
    for (slug, _, size) in sorted {
        if !limit_reached
            && keep.len() < keep_entries
            && kept_bytes.saturating_add(size) <= keep_bytes
        {
            kept_bytes = kept_bytes.saturating_add(size);
            keep.push(slug);
        } else {
            limit_reached = true;
            archive.push(slug);
        }
    }
    (keep, archive)
}

/// Turn-end 是否应尝试自动压缩（与 maybe_compact_memory 阈值一致，不含 force/daily 判断）
pub fn should_auto_compact(store: &MemoryStore) -> Result<bool, crate::memory::store::MemoryError> {
    let raw = store.read_index_raw()?;
    let line_count = raw.lines().filter(|l| l.trim().starts_with("- [")).count();
    let entry_count = store.list_entries()?.len();
    let active_size = store.active_entries_total_size()?;
    Ok(active_size >= MEMORY_COMPACT_SIZE_THRESHOLD
        || line_count >= MEMORY_COMPACT_LINE_THRESHOLD
        || entry_count >= MEMORY_COMPACT_ENTRY_THRESHOLD)
}

fn record_failure(store: &MemoryStore, meta: &mut MemoryMeta, now: &chrono::DateTime<Local>) {
    meta.failed_compact_count = meta.failed_compact_count.saturating_add(1);
    if meta.failed_compact_count >= FAILURE_DISABLE_THRESHOLD {
        meta.disabled_until = Some(now.timestamp() + DISABLE_DURATION_SECS);
    }
    let _ = store.write_meta_atomic(meta);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_slugs_keeps_newest_by_updated_at() {
        let input: Vec<(String, i64)> = (0..45).map(|i| (format!("e{:02}", i), i as i64)).collect();
        let (keep, archive) = select_slugs_to_archive(&input, ACTIVE_KEEP);
        assert_eq!(keep.len(), ACTIVE_KEEP);
        assert_eq!(archive.len(), 5);
        assert_eq!(keep[0], "e44");
        assert_eq!(keep[39], "e05");
        assert!(archive.contains(&"e04".to_string()));
        assert!(archive.contains(&"e00".to_string()));
    }

    #[test]
    fn select_slugs_no_archive_when_at_or_below_keep() {
        let input: Vec<(String, i64)> = (0..10).map(|i| (format!("e{}", i), i as i64)).collect();
        let (keep, archive) = select_slugs_to_archive(&input, ACTIVE_KEEP);
        assert_eq!(keep.len(), 10);
        assert!(archive.is_empty());
    }

    #[test]
    fn select_slugs_tiebreaks_by_slug_name() {
        let input = vec![
            ("beta".to_string(), 100),
            ("alpha".to_string(), 100),
            ("gamma".to_string(), 50),
        ];
        let (keep, archive) = select_slugs_to_archive(&input, 2);
        assert_eq!(keep, vec!["alpha", "beta"]);
        assert_eq!(archive, vec!["gamma"]);
    }

    #[test]
    fn select_slugs_uses_entry_and_size_low_watermarks() {
        let input = vec![
            ("newest".to_string(), 30, 60),
            ("middle".to_string(), 20, 50),
            ("oldest".to_string(), 10, 40),
        ];
        let (keep, archive) = select_slugs_to_archive_by_limits(&input, 2, 100);
        assert_eq!(keep, vec!["newest"]);
        assert_eq!(archive, vec!["middle", "oldest"]);
    }
}
