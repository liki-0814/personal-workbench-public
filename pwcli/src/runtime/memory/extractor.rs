// Turn-end automatic memory extraction (ADD-only, hash dedup).
use chrono::Local;
use serde::Deserialize;

use crate::ai::llm::LlmClient;
use crate::ai::llm::{summarize_via_llm_limited, SummarizeError};
use crate::ai::usage::UsageTracker;
use crate::runtime::memory::hash::fact_content_hash;
use crate::runtime::memory::slug::{resolve_slug, validate_slug};
use crate::runtime::memory::store::{MemoryError, MemoryMeta, MemoryStore};
use crate::runtime::memory::sync_index;
use crate::runtime::memory::types::{MemoryEntry, MemoryIndexLine, MemorySource};
use crate::runtime::session::{ConversationMessage, MessageRole};

pub const MAX_TURN_INPUT_CHARS: usize = 8000;
pub const MIN_USER_MSG_CHARS: usize = 8;
pub const MAX_SEEN_HASHES: usize = 500;
pub const EXTRACT_BATCH_TURNS: usize = 5;
pub const EXTRACT_IDLE_SECS: i64 = 10 * 60;
const MAX_PENDING_TURN_CHARS: usize = 2400;
const MAX_BATCH_INPUT_CHARS: usize = 12_000;
const EXTRACT_MAX_TOKENS: u32 = 1024;
const EXTRACT_FAILURE_THRESHOLD: u32 = 3;
const EXTRACT_DISABLE_SECS: i64 = 30 * 60;

const EXTRACT_SYSTEM_PROMPT: &str = r#"你是用户长期记忆抽取器。从本轮对话中识别值得跨会话保留的事实：
- 用户身份/角色、稳定偏好、长期项目上下文、明确说「以后/记得/我喜欢」的指令
- 不要记：一次性任务进度、临时路径、工具输出细节、寒暄、猜测

规则：
1. action 只能是 ADD、UPDATE、SUPERSEDE：新事实用 ADD；修正同一事实用 UPDATE；新事实完全取代旧事实用 SUPERSEDE
2. 若事实已出现在「现有记忆索引」中（语义重复），不要输出
3. 每条 fact 字段：
   - action: ADD|UPDATE|SUPERSEDE
   - slug: snake_case 英文标识，≤64 字符
   - target_slug: UPDATE/SUPERSEDE 时被修改或取代的现有 slug；ADD 省略
   - summary: 一行简介，≤150 字符
   - content: markdown 详情，≤16KB
4. 无新事实时输出 {"facts":[]}
5. 只输出一个 JSON 对象，不要 markdown 代码块、不要解释"#;

#[derive(Debug)]
pub enum MemoryExtractOutcome {
    Done {
        added: usize,
        skipped_dup: usize,
        skipped_invalid: usize,
    },
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryCandidateOutcome {
    Skipped,
    Queued(usize),
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ExtractedFact {
    #[serde(default = "default_extract_action")]
    action: String,
    slug: String,
    #[serde(default)]
    target_slug: Option<String>,
    summary: String,
    content: String,
}

fn default_extract_action() -> String {
    "ADD".into()
}

#[derive(Debug, Deserialize)]
struct ExtractResponse {
    #[serde(default)]
    facts: Vec<ExtractedFact>,
}

pub fn format_turn_messages(msgs: &[ConversationMessage]) -> Option<String> {
    if msgs.is_empty() {
        return None;
    }
    let has_assistant = msgs
        .iter()
        .any(|m| m.role == MessageRole::Assistant && !m.text_content().trim().is_empty());
    if !has_assistant {
        return None;
    }
    let user_chars: usize = msgs
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .map(|m| m.text_content().chars().count())
        .sum();
    if user_chars < MIN_USER_MSG_CHARS {
        return None;
    }

    let mut out = String::new();
    for msg in msgs {
        let text = msg.text_content();
        if text.trim().is_empty() {
            continue;
        }
        match msg.role {
            MessageRole::User => {
                out.push_str("User: ");
                out.push_str(text.trim());
                out.push('\n');
            }
            MessageRole::Assistant => {
                out.push_str("Assistant: ");
                let trimmed = text.trim();
                if trimmed.chars().count() > 2000 {
                    let short: String = trimmed.chars().take(2000).collect();
                    out.push_str(&short);
                    out.push('…');
                } else {
                    out.push_str(trimmed);
                }
                out.push('\n');
            }
            MessageRole::Tool => {
                let preview: String = text.chars().take(120).collect();
                out.push_str("Tool: ");
                out.push_str(&preview);
                if text.chars().count() > 120 {
                    out.push('…');
                }
                out.push('\n');
            }
            MessageRole::System => {}
        }
    }
    if out.trim().is_empty() {
        return None;
    }
    let mut s = out;
    if s.chars().count() > MAX_TURN_INPUT_CHARS {
        s = s.chars().take(MAX_TURN_INPUT_CHARS).collect();
        s.push('…');
    }
    Some(s)
}

pub fn turn_messages_slice(msgs: &[ConversationMessage]) -> &[ConversationMessage] {
    match msgs.iter().rposition(|m| m.role == MessageRole::User) {
        Some(i) => &msgs[i..],
        None => msgs,
    }
}

pub fn is_memory_candidate(msgs: &[ConversationMessage]) -> bool {
    let user_text = msgs
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| message.text_content())
        .collect::<Vec<_>>()
        .join("\n");
    let normalized = user_text.trim().to_lowercase();
    if normalized.chars().count() < MIN_USER_MSG_CHARS {
        return false;
    }

    const DURABLE_MARKERS: &[&str] = &[
        "记住",
        "以后",
        "长期",
        "偏好",
        "我喜欢",
        "我希望",
        "不要再",
        "默认",
        "必须",
        "我是",
        "我的项目",
        "我们决定",
        "架构约束",
        "命名规范",
        "remember",
        "from now on",
        "i prefer",
        "always",
        "never",
        "my project",
        "we decided",
    ];
    DURABLE_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

pub fn queue_memory_candidate(
    store: &MemoryStore,
    session_messages: &[ConversationMessage],
) -> Result<MemoryCandidateOutcome, MemoryError> {
    let slice = turn_messages_slice(session_messages);
    if !is_memory_candidate(slice) {
        return Ok(MemoryCandidateOutcome::Skipped);
    }
    let Some(mut turn_text) = format_turn_messages(slice) else {
        return Ok(MemoryCandidateOutcome::Skipped);
    };
    if turn_text.chars().count() > MAX_PENDING_TURN_CHARS {
        turn_text = turn_text.chars().take(MAX_PENDING_TURN_CHARS).collect();
        turn_text.push('…');
    }
    let count = store.enqueue_pending_turn(turn_text, chrono::Utc::now().timestamp())?;
    Ok(MemoryCandidateOutcome::Queued(count))
}

pub async fn process_pending_memories(
    store: &MemoryStore,
    llm: &LlmClient,
    tracker: &mut UsageTracker,
) -> Option<MemoryExtractOutcome> {
    let pending = match store.read_pending_turns() {
        Ok(pending) => pending,
        Err(error) => return Some(MemoryExtractOutcome::Failed(error.to_string())),
    };
    let last_queued_at = pending.last()?.queued_at;
    let idle_ready = chrono::Utc::now()
        .timestamp()
        .saturating_sub(last_queued_at)
        >= EXTRACT_IDLE_SECS;
    if pending.len() < EXTRACT_BATCH_TURNS && !idle_ready {
        return None;
    }

    let take = if pending.len() >= EXTRACT_BATCH_TURNS {
        EXTRACT_BATCH_TURNS
    } else {
        pending.len()
    };
    let mut batch = String::new();
    for (index, turn) in pending.iter().take(take).enumerate() {
        let separator = format!("\n\n--- candidate turn {} ---\n", index + 1);
        if batch.chars().count() + separator.chars().count() + turn.text.chars().count()
            > MAX_BATCH_INPUT_CHARS
        {
            break;
        }
        batch.push_str(&separator);
        batch.push_str(&turn.text);
    }
    let consumed = batch.matches("--- candidate turn ").count();
    if consumed == 0 {
        return None;
    }

    let index = match store.read_index_raw() {
        Ok(index) => index,
        Err(error) => return Some(MemoryExtractOutcome::Failed(error.to_string())),
    };
    let outcome = maybe_extract_memories(store, llm, tracker, &batch, &index, false).await;
    if !matches!(outcome, MemoryExtractOutcome::Failed(_)) {
        if let Err(error) = store.acknowledge_pending_turns(consumed) {
            return Some(MemoryExtractOutcome::Failed(error.to_string()));
        }
    }
    Some(outcome)
}

pub(crate) fn parse_extract_response(text: &str) -> Option<Vec<ExtractedFact>> {
    let trimmed = text.trim();
    let json_part = if let Some(start) = trimmed.find('{') {
        extract_balanced_json(&trimmed[start..])?
    } else {
        return None;
    };
    let parsed: ExtractResponse = serde_json::from_str(json_part).ok()?;
    Some(parsed.facts)
}

fn extract_balanced_json(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, b) in bytes.iter().enumerate() {
        let c = *b as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn record_extract_failure(
    store: &MemoryStore,
    meta: &mut MemoryMeta,
    now: &chrono::DateTime<Local>,
) {
    meta.failed_extract_count = meta.failed_extract_count.saturating_add(1);
    if meta.failed_extract_count >= EXTRACT_FAILURE_THRESHOLD {
        meta.disabled_extract_until = Some(now.timestamp() + EXTRACT_DISABLE_SECS);
    }
    let _ = store.write_meta_atomic(meta);
}

pub async fn maybe_extract_memories(
    store: &MemoryStore,
    llm: &LlmClient,
    tracker: &mut UsageTracker,
    turn_text: &str,
    existing_index_raw: &str,
    force: bool,
) -> MemoryExtractOutcome {
    let now = Local::now();
    let mut meta = match store.read_meta() {
        Ok(m) => m,
        Err(e) => return MemoryExtractOutcome::Failed(format!("读取 meta 失败: {}", e)),
    };

    if !force {
        if let Some(disabled_until) = meta.disabled_extract_until {
            if now.timestamp() < disabled_until {
                return MemoryExtractOutcome::Skipped(format!(
                    "抽取临时禁用至 {}（连续失败保护）",
                    disabled_until
                ));
            }
        }
    }

    let user_prompt = format!(
        "## 现有记忆索引\n{}\n\n## 本轮对话\n{}",
        if existing_index_raw.trim().is_empty() {
            "（空）".to_string()
        } else {
            existing_index_raw.trim().to_string()
        },
        turn_text
    );

    let response = match summarize_via_llm_limited(
        &user_prompt,
        EXTRACT_SYSTEM_PROMPT,
        llm,
        tracker,
        Some(EXTRACT_MAX_TOKENS),
    )
    .await
    {
        Ok(s) => s,
        Err(SummarizeError::EmptyResponse) => {
            record_extract_failure(store, &mut meta, &now);
            return MemoryExtractOutcome::Failed("AI 返回空响应".to_string());
        }
        Err(SummarizeError::LlmError(e)) => {
            record_extract_failure(store, &mut meta, &now);
            return MemoryExtractOutcome::Failed(format!("LLM 失败: {}", e));
        }
    };

    let facts = match parse_extract_response(&response) {
        Some(f) => f,
        None => {
            record_extract_failure(store, &mut meta, &now);
            return MemoryExtractOutcome::Failed("抽取 JSON 解析失败".to_string());
        }
    };

    if facts.is_empty() {
        return MemoryExtractOutcome::Skipped("无可抽取事实".to_string());
    }

    let mut added = 0usize;
    let mut skipped_dup = 0usize;
    let mut skipped_invalid = 0usize;
    let ts = chrono::Utc::now().timestamp();
    let provenance_hash = fact_content_hash("conversation", turn_text);

    for fact in facts {
        let slug_base = fact.slug.trim().to_string();
        let summary = fact.summary.trim().to_string();
        let content = fact.content.trim().to_string();
        if !validate_slug(&slug_base) || summary.is_empty() || content.is_empty() {
            skipped_invalid += 1;
            continue;
        }
        let hash = fact_content_hash(&summary, &content);
        if meta.seen_content_hashes.contains(&hash) {
            skipped_dup += 1;
            continue;
        }
        let action = fact.action.trim().to_ascii_uppercase();
        let target = fact
            .target_slug
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let (slug, mut entry, superseded_target) = match action.as_str() {
            "UPDATE" => {
                let Some(target) = target else {
                    skipped_invalid += 1;
                    continue;
                };
                let Ok(mut existing) = store.read_entry(target) else {
                    skipped_invalid += 1;
                    continue;
                };
                existing.summary = summary.clone();
                existing.content = content;
                existing.updated_at = ts;
                (target.to_string(), existing, None)
            }
            "SUPERSEDE" => {
                let Some(target) = target else {
                    skipped_invalid += 1;
                    continue;
                };
                if store.read_entry(target).is_err() {
                    skipped_invalid += 1;
                    continue;
                }
                let slug = resolve_slug(store, &slug_base);
                (
                    slug.clone(),
                    MemoryEntry::new_active(
                        slug,
                        summary.clone(),
                        content,
                        ts,
                        Some(target.to_string()),
                    ),
                    Some(target.to_string()),
                )
            }
            "ADD" => {
                let slug = resolve_slug(store, &slug_base);
                let supersedes = (slug != slug_base).then_some(slug_base);
                (
                    slug.clone(),
                    MemoryEntry::new_active(slug, summary.clone(), content, ts, supersedes),
                    None,
                )
            }
            _ => {
                skipped_invalid += 1;
                continue;
            }
        };
        entry.sources.push(MemorySource {
            kind: "conversation".into(),
            uri: format!("conversation:{}", &provenance_hash[..16]),
            title: None,
            content_hash: Some(provenance_hash.clone()),
            captured_at: Some(ts),
        });
        if let Err(e) = store.write_entry_atomic(&entry) {
            if matches!(
                e,
                MemoryError::SummaryTooLong { .. }
                    | MemoryError::EntryTooLarge { .. }
                    | MemoryError::IndexFull { .. }
            ) {
                skipped_invalid += 1;
                continue;
            }
            return MemoryExtractOutcome::Failed(format!("写入失败: {}", e));
        }
        let line = MemoryIndexLine {
            slug,
            summary,
            updated_at: ts,
        };
        if let Err(e) = store.upsert_index_line(&line) {
            return MemoryExtractOutcome::Failed(format!("更新索引失败: {}", e));
        }
        if let Some(target) = superseded_target {
            if let Err(error) = store.soft_delete_entry(&target, ts) {
                return MemoryExtractOutcome::Failed(format!("归档旧记忆失败: {}", error));
            }
        }
        sync_index::best_effort_upsert(store, &entry);
        meta.seen_content_hashes.push(hash);
        if meta.seen_content_hashes.len() > MAX_SEEN_HASHES {
            let drain = meta.seen_content_hashes.len() - MAX_SEEN_HASHES;
            meta.seen_content_hashes.drain(0..drain);
        }
        added += 1;
    }

    meta.failed_extract_count = 0;
    meta.disabled_extract_until = None;
    if let Err(e) = store.write_meta_atomic(&meta) {
        return MemoryExtractOutcome::Failed(format!("写 meta 失败: {}", e));
    }

    if added == 0 && skipped_dup > 0 {
        return MemoryExtractOutcome::Skipped(format!("{} 条重复跳过", skipped_dup));
    }
    if added == 0 {
        return MemoryExtractOutcome::Skipped("无有效新事实".to_string());
    }

    MemoryExtractOutcome::Done {
        added,
        skipped_dup,
        skipped_invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extract_json() {
        let text = r#"{"facts":[{"slug":"rust_cli","summary":"偏好 Rust","content":"用户喜欢用 Rust 写 CLI"}]}"#;
        let facts = parse_extract_response(text).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].slug, "rust_cli");
    }

    #[test]
    fn format_turn_requires_assistant() {
        let msgs = vec![ConversationMessage::new_user("hello world test")];
        assert!(format_turn_messages(&msgs).is_none());
    }

    #[test]
    fn format_turn_user_assistant() {
        let msgs = vec![
            ConversationMessage::new_user("以后默认用 Rust 写 CLI"),
            ConversationMessage::new_assistant("好的，我会记住"),
        ];
        let s = format_turn_messages(&msgs).unwrap();
        assert!(s.contains("Rust"));
        assert!(s.contains("Assistant"));
    }

    #[test]
    fn candidate_gate_skips_transient_question() {
        let msgs = vec![
            ConversationMessage::new_user("这个函数为什么会返回空值？"),
            ConversationMessage::new_assistant("因为输入为空。"),
        ];
        assert!(!is_memory_candidate(&msgs));
    }

    #[test]
    fn candidate_gate_accepts_durable_preference() {
        let msgs = vec![
            ConversationMessage::new_user("以后默认使用 Rust 编写本地 CLI"),
            ConversationMessage::new_assistant("好的。"),
        ];
        assert!(is_memory_candidate(&msgs));
    }

    #[test]
    fn candidate_queue_batches_without_duplicate_turns() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new_with_dir(dir.path().to_path_buf()).unwrap();
        let msgs = vec![
            ConversationMessage::new_user("以后默认使用 Rust 编写本地 CLI"),
            ConversationMessage::new_assistant("好的。"),
        ];
        assert_eq!(
            queue_memory_candidate(&store, &msgs).unwrap(),
            MemoryCandidateOutcome::Queued(1)
        );
        assert_eq!(
            queue_memory_candidate(&store, &msgs).unwrap(),
            MemoryCandidateOutcome::Queued(1)
        );
        assert_eq!(store.read_pending_turns().unwrap().len(), 1);
    }
}
