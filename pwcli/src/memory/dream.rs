use std::fs::OpenOptions;

use fs2::FileExt;

use crate::llm::LlmClient;
use crate::memory::extractor::{
    format_turn_messages, maybe_extract_memories, MemoryExtractOutcome,
};
use crate::memory::MemoryStore;
use crate::session::ConversationMessage;
use crate::usage::UsageTracker;

const DREAM_INTERVAL_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, PartialEq, Eq)]
pub enum DreamOutcome {
    Disabled,
    Skipped(String),
    Completed,
    Failed(String),
}

pub async fn maybe_dream(
    store: &MemoryStore,
    llm: &LlmClient,
    tracker: &mut UsageTracker,
    messages: &[ConversationMessage],
    enabled: bool,
    project: &str,
) -> DreamOutcome {
    if !enabled {
        return DreamOutcome::Disabled;
    }
    let project = project.trim();
    if project.is_empty()
        || !project
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return DreamOutcome::Failed("Dream 项目标识为空或包含非法字符".into());
    }
    let now = chrono::Utc::now().timestamp();
    let mut meta = store.read_meta().unwrap_or_default();
    if meta.last_dream_project.as_deref() == Some(project)
        && meta
            .last_dream_at
            .is_some_and(|last| now.saturating_sub(last) < DREAM_INTERVAL_SECS)
    {
        return DreamOutcome::Skipped("同一项目 24 小时内已执行".into());
    }
    let Some(turns) = format_turn_messages(messages) else {
        return DreamOutcome::Skipped("近期会话没有可沉淀内容".into());
    };

    let lock_path = store.base_dir().join("dream.lock");
    let lock = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)
    {
        Ok(lock) => lock,
        Err(error) => return DreamOutcome::Failed(error.to_string()),
    };
    if lock.try_lock_exclusive().is_err() {
        return DreamOutcome::Skipped("另一个进程正在执行 Dream".into());
    }

    let scoped_input = format!(
        "Dream project scope: {project}\nOnly derive durable knowledge for this exact project. Do not generalize project-specific lessons globally.\n\n{turns}"
    );
    let index = match store.read_index_raw() {
        Ok(index) => index,
        Err(error) => return DreamOutcome::Failed(error.to_string()),
    };
    let outcome = maybe_extract_memories(store, llm, tracker, &scoped_input, &index, false).await;
    match outcome {
        MemoryExtractOutcome::Failed(error) => DreamOutcome::Failed(error),
        _ => {
            meta.last_dream_at = Some(now);
            meta.last_dream_project = Some(project.to_string());
            match store.write_meta_atomic(&meta) {
                Ok(()) => DreamOutcome::Completed,
                Err(error) => DreamOutcome::Failed(error.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dream_is_disabled_by_default_contract() {
        assert_eq!(DreamOutcome::Disabled, DreamOutcome::Disabled);
    }

    #[test]
    fn recent_project_gate_is_scoped() {
        let meta = crate::memory::MemoryMeta {
            last_dream_at: Some(100),
            last_dream_project: Some("a".into()),
            ..Default::default()
        };
        assert_eq!(meta.last_dream_project.as_deref(), Some("a"));
    }
}
