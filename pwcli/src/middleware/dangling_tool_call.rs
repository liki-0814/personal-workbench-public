use std::collections::HashSet;

use anyhow::Result;
use async_trait::async_trait;

use super::types::HookAction;
use super::AgentMiddleware;
use crate::graph::state::GraphState;
use crate::graph::GraphContext;

#[derive(Default)]
pub struct DanglingToolCallMiddleware;

impl DanglingToolCallMiddleware {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentMiddleware for DanglingToolCallMiddleware {
    fn name(&self) -> &str {
        "dangling_tool_call"
    }

    async fn before_llm(
        &self,
        state: &mut GraphState,
        ctx: &GraphContext<'_>,
    ) -> Result<HookAction> {
        let repaired = patch_dangling_tool_calls(&mut state.messages);
        if repaired > 0 {
            ctx.record_intervention(
                self.name(),
                self.revision(),
                crate::harness::MiddlewareHook::BeforeLlm,
                crate::harness::InterventionKind::StateMutation,
                "dangling_tool_calls_repaired",
                state.round_count,
                None,
                std::collections::BTreeMap::from([(
                    "repaired_count".to_string(),
                    serde_json::Value::from(repaired as u64),
                )]),
            )
            .await;
        }
        Ok(HookAction::Continue)
    }
}

fn patch_dangling_tool_calls(messages: &mut Vec<crate::llm::ChatMessage>) -> usize {
    let answered: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.tool_call_id.clone())
        .collect();

    let mut result: Vec<crate::llm::ChatMessage> = Vec::with_capacity(messages.len());
    let mut placed: HashSet<String> = HashSet::new();
    let mut repaired = 0;

    let mut i = 0;
    while i < messages.len() {
        let msg = &messages[i];

        if msg.role == "assistant" {
            if let Some(tool_calls) = &msg.tool_calls {
                if !tool_calls.is_empty() {
                    result.push(msg.clone());
                    i += 1;

                    // Collect trailing tool messages that belong to this assistant message
                    while i < messages.len() && messages[i].role == "tool" {
                        if let Some(id) = &messages[i].tool_call_id {
                            placed.insert(id.clone());
                        }
                        result.push(messages[i].clone());
                        i += 1;
                    }

                    // Inject synthetic results for any dangling tool calls
                    for tc in tool_calls {
                        if !answered.contains(&tc.id) && !placed.contains(&tc.id) {
                            let content = if tc.function.name == "write_file" {
                                "[Tool call was interrupted. 请改用 assistant text 直接输出内容，不要重试大文件写入。]"
                            } else {
                                "[Tool call was interrupted and did not return a result.]"
                            };
                            result.push(crate::llm::ChatMessage {
                                role: "tool".to_string(),
                                content: content.to_string(),
                                images: Vec::new(),
                                generated_images: Vec::new(),
                                tool_calls: None,
                                tool_call_id: Some(tc.id.clone()),
                            });
                            placed.insert(tc.id.clone());
                            repaired += 1;
                        }
                    }
                    continue;
                }
            }
        }

        // Skip tool messages already placed after their assistant message
        if msg.role == "tool" {
            if let Some(id) = &msg.tool_call_id {
                if placed.contains(id) {
                    i += 1;
                    continue;
                }
                placed.insert(id.clone());
            }
        }

        result.push(msg.clone());
        i += 1;
    }

    *messages = result;
    repaired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, FunctionCall, ToolCall};

    fn user(content: &str) -> ChatMessage {
        ChatMessage {
            role: "user".to_string(),
            content: content.to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn assistant_with_tools(content: &str, calls: Vec<ToolCall>) -> ChatMessage {
        ChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: Some(calls),
            tool_call_id: None,
        }
    }

    fn tool_result(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: "tool".to_string(),
            content: content.to_string(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
        }
    }

    fn tc(id: &str, name: &str) -> ToolCall {
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
    fn no_dangling_calls() {
        let mut msgs = vec![
            user("hi"),
            assistant_with_tools("", vec![tc("c1", "read_file")]),
            tool_result("c1", "file content"),
        ];
        patch_dangling_tool_calls(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].role, "tool");
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn single_dangling_call() {
        let mut msgs = vec![
            user("do something"),
            assistant_with_tools("", vec![tc("c1", "search_files")]),
        ];
        patch_dangling_tool_calls(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].role, "tool");
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c1"));
        assert!(msgs[2].content.contains("interrupted"));
    }

    #[test]
    fn write_file_special_message() {
        let mut msgs = vec![
            user("write"),
            assistant_with_tools("", vec![tc("c1", "write_file")]),
        ];
        patch_dangling_tool_calls(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert!(msgs[2].content.contains("不要重试大文件写入"));
    }

    #[test]
    fn partial_dangling() {
        let mut msgs = vec![
            user("multi"),
            assistant_with_tools("", vec![tc("c1", "read_file"), tc("c2", "search_files")]),
            tool_result("c1", "ok"),
        ];
        patch_dangling_tool_calls(&mut msgs);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(msgs[3].tool_call_id.as_deref(), Some("c2"));
        assert!(msgs[3].content.contains("interrupted"));
    }

    #[test]
    fn preserves_order_with_multiple_assistants() {
        let mut msgs = vec![
            user("first"),
            assistant_with_tools("", vec![tc("c1", "read_file")]),
            tool_result("c1", "ok"),
            user("second"),
            assistant_with_tools("", vec![tc("c2", "list_directory")]),
            // c2 dangling
        ];
        patch_dangling_tool_calls(&mut msgs);
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[0].content, "first");
        assert_eq!(msgs[3].content, "second");
        assert_eq!(msgs[5].tool_call_id.as_deref(), Some("c2"));
        assert!(msgs[5].content.contains("interrupted"));
    }

    #[test]
    fn no_tool_calls_pass_through() {
        let mut msgs = vec![
            user("hello"),
            ChatMessage {
                role: "assistant".to_string(),
                content: "hi there".to_string(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        patch_dangling_tool_calls(&mut msgs);
        assert_eq!(msgs.len(), 2);
    }
}
