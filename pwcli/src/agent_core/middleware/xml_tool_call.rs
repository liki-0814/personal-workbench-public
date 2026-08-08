use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

use super::types::HookAction;
use super::AgentMiddleware;
use crate::agent_core::graph::state::GraphState;
use crate::agent_core::graph::GraphContext;
use crate::ai::llm::{FunctionCall, ToolCall};

#[derive(Default)]
pub struct XmlToolCallMiddleware;

impl XmlToolCallMiddleware {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentMiddleware for XmlToolCallMiddleware {
    fn name(&self) -> &str {
        "xml_tool_call"
    }

    async fn after_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        if state.pending_tool_calls.is_empty() && state.last_content.contains("<tool_call>") {
            let parsed = parse_xml_tool_calls(&state.last_content);
            if !parsed.is_empty() {
                let recovered_count = parsed.len();
                state.last_content = strip_re()
                    .replace_all(&state.last_content, "")
                    .trim()
                    .to_string();
                state.pending_tool_calls = parsed;

                if let Some(last_msg) = state.messages.last_mut() {
                    if last_msg.role == "assistant" {
                        last_msg.content = state.last_content.clone();
                        last_msg.tool_calls = Some(state.pending_tool_calls.clone());
                    }
                }
                ctx.record_intervention(
                    self.name(),
                    self.revision(),
                    crate::agent_core::harness::MiddlewareHook::AfterLlm,
                    crate::agent_core::harness::InterventionKind::StateMutation,
                    "xml_tool_calls_recovered",
                    state.round_count,
                    None,
                    std::collections::BTreeMap::from([(
                        "tool_call_count".to_string(),
                        serde_json::Value::from(recovered_count as u64),
                    )]),
                )
                .await;
            }
        }
        Ok(HookAction::Continue)
    }
}

fn strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<tool_call>[\s\S]*?</\w+>").unwrap())
}

fn parse_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<tool_call>\s*([\s\S]*?)\s*</\w+>").unwrap())
}

fn parse_xml_tool_calls(content: &str) -> Vec<ToolCall> {
    let mut results = Vec::new();
    let mut idx = 0u32;

    for cap in parse_re().captures_iter(content) {
        let inner = cap[1].trim();
        let cleaned = inner
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let v: Value = match serde_json::from_str(cleaned) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let (name, arguments) = if let (Some(_domain), Some(_action)) = (
            v.get("domain").and_then(|d| d.as_str()),
            v.get("action").and_then(|a| a.as_str()),
        ) {
            (
                "data_crud".to_string(),
                serde_json::to_string(&v).unwrap_or_default(),
            )
        } else if let (Some(name), Some(args)) =
            (v.get("name").and_then(|n| n.as_str()), v.get("arguments"))
        {
            (
                name.to_string(),
                serde_json::to_string(args).unwrap_or_default(),
            )
        } else {
            continue;
        };

        results.push(ToolCall {
            id: format!("xml_tc_{}", idx),
            kind: "function".to_string(),
            function: FunctionCall { name, arguments },
            thought_signature: None,
        });
        idx += 1;
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_crud_format() {
        let content = r#"Some text <tool_call>{"domain":"nav","action":"query","payload":{}}</tool_call> more text"#;
        let calls = parse_xml_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "data_crud");
    }

    #[test]
    fn parses_generic_format() {
        let content =
            r#"<tool_call>{"name":"read_file","arguments":{"path":"/a.txt"}}</tool_call>"#;
        let calls = parse_xml_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "read_file");
    }

    #[test]
    fn strips_markdown_fences() {
        let content = "<tool_call>```json\n{\"name\":\"ls\",\"arguments\":{}}\n```</tool_call>";
        let calls = parse_xml_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "ls");
    }

    #[test]
    fn handles_wrong_closing_tag() {
        let content = r#"<tool_call>{"name":"foo","arguments":{}}</tool>"#;
        let calls = parse_xml_tool_calls(content);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn no_tool_calls_returns_empty() {
        let calls = parse_xml_tool_calls("just normal text");
        assert!(calls.is_empty());
    }
}
