use anyhow::Result;
use async_trait::async_trait;

use super::types::{ToolCallRequest, ToolResult};
use super::AgentMiddleware;
use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;

pub struct ToolOutputBudgetConfig {
    pub externalize_threshold: usize,
    pub fallback_truncate: usize,
    pub preview_head: usize,
    pub preview_tail: usize,
}

impl Default for ToolOutputBudgetConfig {
    fn default() -> Self {
        Self {
            externalize_threshold: 12000,
            fallback_truncate: 8000,
            preview_head: 2000,
            preview_tail: 1000,
        }
    }
}

#[derive(Default)]
pub struct ToolOutputBudgetMiddleware {
    config: ToolOutputBudgetConfig,
}

impl ToolOutputBudgetMiddleware {
    pub fn from_config(config: ToolOutputBudgetConfig) -> Self {
        Self { config }
    }

    pub fn new(externalize_threshold: usize, fallback_truncate: usize) -> Self {
        Self {
            config: ToolOutputBudgetConfig {
                externalize_threshold,
                fallback_truncate,
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl AgentMiddleware for ToolOutputBudgetMiddleware {
    fn name(&self) -> &str {
        "tool_output_budget"
    }

    async fn after_tool(
        &self,
        _state: &mut GraphState,
        ctx: &GraphContext<'_>,
        request: &ToolCallRequest,
        result: &mut ToolResult,
    ) -> Result<()> {
        let total_chars = result.content.chars().count();
        if total_chars <= self.config.externalize_threshold {
            return Ok(());
        }

        match ctx
            .artifact_store
            .ok_or_else(|| anyhow::anyhow!("artifact store unavailable"))
            .and_then(|store| store.store_tool_output(&result.content))
        {
            Ok(artifact) => {
                let artifact_id = artifact.id.clone();
                let artifact_sha256 = artifact.sha256.clone();
                let head = truncate_chars(&result.content, self.config.preview_head);
                let tail = take_last_chars(&result.content, self.config.preview_tail);
                let tail_start = total_chars.saturating_sub(self.config.preview_tail);
                let search_hint = file_search_hint(request);
                result.content = format!(
                    "{}\n\n... [完整输出已保存为 artifact_id={}，共 {} 字符，sha256={}。当前已展示 0..{} 与 {}..{}；{}只有确实需要连续全文时，才使用 read_artifact 从 offset={} 分块读取，不要从 0 重读。] ...\n\n{}",
                    head,
                    artifact.id,
                    total_chars,
                    artifact.sha256,
                    self.config.preview_head,
                    tail_start,
                    total_chars,
                    search_hint,
                    self.config.preview_head,
                    tail,
                );
                ctx.record_intervention(
                    self.name(),
                    self.revision(),
                    crate::agent_core::harness::MiddlewareHook::AfterTool,
                    crate::agent_core::harness::InterventionKind::ResultMutation,
                    "tool_output_externalized",
                    _state.round_count,
                    Some(request.id.clone()),
                    std::collections::BTreeMap::from([
                        (
                            "tool_name".to_string(),
                            serde_json::Value::from(request.name.clone()),
                        ),
                        (
                            "original_chars".to_string(),
                            serde_json::Value::from(total_chars as u64),
                        ),
                        (
                            "artifact_id".to_string(),
                            serde_json::Value::from(artifact_id),
                        ),
                        (
                            "artifact_sha256".to_string(),
                            serde_json::Value::from(artifact_sha256),
                        ),
                    ]),
                )
                .await;
            }
            Err(_) => {
                let max = self.config.fallback_truncate;
                let head_len = max / 2;
                let tail_len = max / 2;
                let head = truncate_chars(&result.content, head_len);
                let tail = take_last_chars(&result.content, tail_len);
                let omitted = total_chars - max;
                result.content = format!(
                    "{}\n\n…[省略 {} 字符（共 {} 字符）]…\n\n{}",
                    head, omitted, total_chars, tail,
                );
                ctx.record_intervention(
                    self.name(),
                    self.revision(),
                    crate::agent_core::harness::MiddlewareHook::AfterTool,
                    crate::agent_core::harness::InterventionKind::ResultMutation,
                    "tool_output_fallback_truncated",
                    _state.round_count,
                    Some(request.id.clone()),
                    std::collections::BTreeMap::from([
                        (
                            "tool_name".to_string(),
                            serde_json::Value::from(request.name.clone()),
                        ),
                        (
                            "original_chars".to_string(),
                            serde_json::Value::from(total_chars as u64),
                        ),
                        (
                            "kept_chars".to_string(),
                            serde_json::Value::from(max as u64),
                        ),
                    ]),
                )
                .await;
            }
        }

        Ok(())
    }
}

fn file_search_hint(request: &ToolCallRequest) -> String {
    if request.name != "read" {
        return String::new();
    }
    let Some(path) = request
        .arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
    else {
        return String::new();
    };
    format!(
        "若只需查找特定证据，优先调用 search_file_content，paths 传 [{}] 并用证据缺口生成 queries；",
        serde_json::json!(path)
    )
}

fn truncate_chars(s: &str, max: usize) -> &str {
    if s.chars().count() <= max {
        return s;
    }
    let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    &s[..end]
}

fn take_last_chars(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if total <= n {
        return s;
    }
    let skip = total - n;
    let start = s.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    &s[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncate_chars_within_limit() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_exact() {
        assert_eq!(truncate_chars("hello", 5), "hello");
    }

    #[test]
    fn truncate_chars_cuts() {
        assert_eq!(truncate_chars("hello world", 5), "hello");
    }

    #[test]
    fn truncate_chars_unicode() {
        let s = "你好世界测试";
        assert_eq!(truncate_chars(s, 3), "你好世");
    }

    #[test]
    fn take_last_chars_within_limit() {
        assert_eq!(take_last_chars("hello", 10), "hello");
    }

    #[test]
    fn take_last_chars_exact() {
        assert_eq!(take_last_chars("hello", 5), "hello");
    }

    #[test]
    fn take_last_chars_takes_tail() {
        assert_eq!(take_last_chars("hello world", 5), "world");
    }

    #[test]
    fn take_last_chars_unicode() {
        let s = "你好世界测试";
        assert_eq!(take_last_chars(s, 2), "测试");
    }

    #[test]
    fn read_file_hint_routes_fact_lookup_to_content_search() {
        let request = ToolCallRequest {
            id: "call-1".into(),
            name: "read".into(),
            arguments: json!({"path": "/data/report.txt"}),
        };
        let hint = file_search_hint(&request);
        assert!(hint.contains("search_file_content"));
        assert!(hint.contains("/data/report.txt"));
    }

    #[test]
    fn non_file_tool_has_no_content_search_hint() {
        let request = ToolCallRequest {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: json!({"command": "printf test"}),
        };
        assert!(file_search_hint(&request).is_empty());
    }
}
