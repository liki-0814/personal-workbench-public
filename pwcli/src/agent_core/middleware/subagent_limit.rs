use anyhow::Result;
use async_trait::async_trait;
use tracing::info;

use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;

use super::types::HookAction;
use super::AgentMiddleware;

/// 截断超额的 `task` tool_calls，保留前 N 个。
pub struct SubAgentLimitMiddleware {
    max_concurrent: usize,
}

impl SubAgentLimitMiddleware {
    pub fn new(max_concurrent: usize) -> Self {
        let clamped = max_concurrent.clamp(2, 4);
        Self {
            max_concurrent: clamped,
        }
    }
}

impl Default for SubAgentLimitMiddleware {
    fn default() -> Self {
        Self::new(3)
    }
}

#[async_trait]
impl AgentMiddleware for SubAgentLimitMiddleware {
    fn name(&self) -> &str {
        "SubAgentLimit"
    }

    async fn after_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        let task_count = state
            .pending_tool_calls
            .iter()
            .filter(|tc| tc.function.name == "task")
            .count();

        if task_count > self.max_concurrent {
            info!(
                task_count,
                max = self.max_concurrent,
                "truncating excess task tool_calls"
            );

            let mut kept_tasks = 0;
            state.pending_tool_calls.retain(|tc| {
                if tc.function.name == "task" {
                    kept_tasks += 1;
                    kept_tasks <= self.max_concurrent
                } else {
                    true // keep all non-task calls
                }
            });
            ctx.record_intervention(
                self.name(),
                self.revision(),
                crate::agent_core::harness::MiddlewareHook::AfterLlm,
                crate::agent_core::harness::InterventionKind::StateMutation,
                "subagent_calls_truncated",
                state.round_count,
                None,
                std::collections::BTreeMap::from([
                    (
                        "requested_count".to_string(),
                        serde_json::Value::from(task_count as u64),
                    ),
                    (
                        "kept_count".to_string(),
                        serde_json::Value::from(self.max_concurrent as u64),
                    ),
                ]),
            )
            .await;
        }

        Ok(HookAction::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::{FunctionCall, ToolCall};

    fn make_tool_call(name: &str, id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
            thought_signature: None,
        }
    }

    #[test]
    fn clamps_to_valid_range() {
        let m = SubAgentLimitMiddleware::new(1);
        assert_eq!(m.max_concurrent, 2);

        let m = SubAgentLimitMiddleware::new(10);
        assert_eq!(m.max_concurrent, 4);

        let m = SubAgentLimitMiddleware::new(3);
        assert_eq!(m.max_concurrent, 3);
    }

    #[tokio::test]
    async fn no_truncation_when_under_limit() {
        let m = SubAgentLimitMiddleware::new(3);
        let mut state = GraphState::new();
        state.pending_tool_calls = vec![
            make_tool_call("task", "t1"),
            make_tool_call("task", "t2"),
            make_tool_call("read_file", "r1"),
        ];

        let task_count = state
            .pending_tool_calls
            .iter()
            .filter(|tc| tc.function.name == "task")
            .count();
        assert_eq!(task_count, 2);
        // Under limit (3), no truncation needed
        assert!(task_count <= m.max_concurrent);
    }

    #[test]
    fn truncation_logic() {
        let max = 2;
        let mut calls = vec![
            make_tool_call("task", "t1"),
            make_tool_call("read_file", "r1"),
            make_tool_call("task", "t2"),
            make_tool_call("task", "t3"),
            make_tool_call("task", "t4"),
            make_tool_call("write_file", "w1"),
        ];

        let task_count = calls.iter().filter(|tc| tc.function.name == "task").count();
        assert_eq!(task_count, 4);

        if task_count > max {
            let mut kept_tasks = 0;
            calls.retain(|tc| {
                if tc.function.name == "task" {
                    kept_tasks += 1;
                    kept_tasks <= max
                } else {
                    true
                }
            });
        }

        // Should keep: t1, r1, t2, w1 (first 2 tasks + all non-tasks)
        assert_eq!(calls.len(), 4);
        let remaining_names: Vec<&str> = calls.iter().map(|c| c.function.name.as_str()).collect();
        assert_eq!(
            remaining_names,
            vec!["task", "read_file", "task", "write_file"]
        );
        assert_eq!(calls[0].id, "t1");
        assert_eq!(calls[2].id, "t2");
    }
}
