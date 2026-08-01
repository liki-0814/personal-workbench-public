use tracing::{debug, info, warn};

use crate::llm::LlmClient;
use crate::memory::extractor::{
    process_pending_memories, queue_memory_candidate, MemoryCandidateOutcome, MemoryExtractOutcome,
    EXTRACT_BATCH_TURNS, EXTRACT_IDLE_SECS,
};
use crate::memory::{maybe_compact_memory, should_auto_compact, MemoryCompactOutcome, MemoryStore};
use crate::session::ConversationMessage;
use crate::usage::UsageTracker;

pub fn schedule_after_turn(
    user_slug: String,
    llm: LlmClient,
    session_messages: Vec<ConversationMessage>,
    auto_extract: bool,
) {
    tokio::spawn(async move {
        let store = match MemoryStore::new(&user_slug) {
            Ok(store) => store,
            Err(error) => {
                debug!(%error, "memory maintenance store unavailable");
                return;
            }
        };

        let queued = if auto_extract {
            match queue_memory_candidate(&store, &session_messages) {
                Ok(MemoryCandidateOutcome::Queued(count)) => Some(count),
                Ok(MemoryCandidateOutcome::Skipped) => None,
                Err(error) => {
                    warn!(%error, "failed to queue memory candidate");
                    None
                }
            }
        } else {
            None
        };

        run_maintenance(&store, &llm, &session_messages).await;

        if queued.is_some_and(|count| count < EXTRACT_BATCH_TURNS) {
            tokio::time::sleep(std::time::Duration::from_secs(EXTRACT_IDLE_SECS as u64)).await;
            run_maintenance(&store, &llm, &session_messages).await;
        }
    });
}

/// Preserve a clone of the durable candidate before session compaction mutates
/// the live history. Queueing is asynchronous and never blocks compaction.
pub fn schedule_before_compaction(
    user_slug: String,
    session_messages: Vec<ConversationMessage>,
    auto_extract: bool,
) {
    if !auto_extract {
        return;
    }
    tokio::spawn(async move {
        let Ok(store) = MemoryStore::new(&user_slug) else {
            return;
        };
        if let Err(error) = queue_memory_candidate(&store, &session_messages) {
            warn!(%error, "failed to preserve pre-compaction memory candidate");
        }
    });
}

async fn run_maintenance(
    store: &MemoryStore,
    llm: &LlmClient,
    session_messages: &[ConversationMessage],
) {
    let _maintenance_guard = match store.try_acquire_maintenance() {
        Ok(Some(guard)) => guard,
        Ok(None) => return,
        Err(error) => {
            warn!(%error, "failed to acquire memory maintenance lock");
            return;
        }
    };

    let mut tracker = UsageTracker::new();
    if let Some(outcome) = process_pending_memories(store, llm, &mut tracker).await {
        match outcome {
            MemoryExtractOutcome::Done { added, .. } if added > 0 => {
                info!(added, "background memory extraction completed");
            }
            MemoryExtractOutcome::Failed(error) => {
                warn!(%error, "background memory extraction failed");
            }
            _ => {}
        }
    }

    let config = crate::config::local_config::get();
    match crate::memory::maybe_dream(
        store,
        llm,
        &mut tracker,
        session_messages,
        config.memory.dream_enabled,
        &config.memory.dream_project,
    )
    .await
    {
        crate::memory::DreamOutcome::Completed => info!("background Dream completed"),
        crate::memory::DreamOutcome::Failed(error) => warn!(%error, "background Dream failed"),
        crate::memory::DreamOutcome::Disabled | crate::memory::DreamOutcome::Skipped(_) => {}
    }

    if !should_auto_compact(store).unwrap_or(false) {
        return;
    }
    match maybe_compact_memory(store, llm, &mut tracker, false).await {
        MemoryCompactOutcome::Done {
            archived_count,
            summary_chars,
        } => {
            info!(
                archived_count,
                summary_chars, "background memory compaction completed"
            );
        }
        MemoryCompactOutcome::Failed(error) => {
            warn!(%error, "background memory compaction failed");
        }
        MemoryCompactOutcome::Skipped(_) => {}
    }
}
