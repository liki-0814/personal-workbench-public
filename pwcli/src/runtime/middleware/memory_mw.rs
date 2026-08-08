use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;
use crate::ai::usage::UsageTracker;
use crate::runtime::memory::compactor::should_auto_compact;
use crate::runtime::memory::extractor::{
    process_pending_memories, queue_memory_candidate, MemoryExtractOutcome,
};
use crate::runtime::memory::store::MemoryStore;

use crate::agent_core::middleware::AgentMiddleware;

/// Post-turn middleware: candidate-batched memory extraction + threshold compaction.
pub struct MemoryMiddleware {
    user_slug: String,
    auto_extract: bool,
}

impl MemoryMiddleware {
    pub fn new(user_slug: String, auto_extract: bool) -> Self {
        Self {
            user_slug,
            auto_extract,
        }
    }
}

#[async_trait]
impl AgentMiddleware for MemoryMiddleware {
    fn name(&self) -> &str {
        "memory"
    }

    async fn after_turn(&self, _state: &mut GraphState, ctx: &GraphContext<'_>) -> Result<()> {
        let store = match MemoryStore::new(&self.user_slug) {
            Ok(s) => s,
            Err(e) => {
                debug!(error = %e, "memory store init failed, skipping extraction");
                return Ok(());
            }
        };

        // Read session messages for turn-end extraction
        let session = ctx.session.lock().await;
        let session_messages = session.messages.clone();
        drop(session);

        // 1. Cheap local candidate gate; the LLM runs only when a batch is ready.
        if self.auto_extract {
            if let Err(error) = queue_memory_candidate(&store, &session_messages) {
                warn!(%error, "memory middleware: failed to queue candidate");
            }
            let mut tracker = UsageTracker::new();
            if let Some(outcome) = process_pending_memories(&store, ctx.llm, &mut tracker).await {
                match outcome {
                    MemoryExtractOutcome::Done { added, .. } if added > 0 => {
                        info!(added, "memory middleware: extracted new facts");
                    }
                    MemoryExtractOutcome::Failed(ref msg) => {
                        warn!(error = %msg, "memory middleware: extraction failed");
                    }
                    _ => {}
                }
            }
        }

        // 2. Memory compaction (best-effort)
        if should_auto_compact(&store).unwrap_or(false) {
            let mut tracker = UsageTracker::new();
            let outcome = crate::runtime::memory::compactor::maybe_compact_memory(
                &store,
                ctx.llm,
                &mut tracker,
                false,
            )
            .await;
            match outcome {
                crate::runtime::memory::compactor::MemoryCompactOutcome::Done {
                    archived_count,
                    summary_chars,
                } => {
                    info!(
                        archived_count,
                        summary_chars, "memory middleware: compaction done"
                    );
                }
                crate::runtime::memory::compactor::MemoryCompactOutcome::Failed(ref msg) => {
                    warn!(error = %msg, "memory middleware: compaction failed");
                }
                crate::runtime::memory::compactor::MemoryCompactOutcome::Skipped(_) => {}
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_middleware_name() {
        let mw = MemoryMiddleware::new("test_user".to_string(), true);
        assert_eq!(mw.name(), "memory");
    }

    #[test]
    fn memory_middleware_disabled() {
        let mw = MemoryMiddleware::new("test_user".to_string(), false);
        assert!(!mw.auto_extract);
    }
}
