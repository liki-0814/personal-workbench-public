//! Shared wire serialization for the OpenAI Responses protocol family.
//!
//! This module is the single source of truth for how the internal chat
//! contract maps to Responses `input` items and function tool definitions.
//! Both [`super::openai_responses`] and [`super::openai_codex_responses`]
//! must serialize through these helpers so the wire contract cannot drift
//! between adapters.
//!
//! Contract notes (validated against strict gateways such as xAI):
//! - `input` is a flat item list: `message`, `function_call`,
//!   `function_call_output` and `reasoning` are all first-level items.
//!   A `function_call` is NEVER a content part of an assistant message.
//! - Function tool `parameters` must be a strict JSON-schema subset: the
//!   root must be an object type and every root-level anyOf/oneOf branch
//!   must itself declare `"type": "object"`.

use serde_json::{json, Value};

use crate::ai::llm::models::{ChatMessage, ToolSchema};

/// System prompt as a first-level input item.
pub fn system_item(text: &str) -> Value {
    json!({
        "role": "system",
        "content": [{"type": "input_text", "text": text}]
    })
}

/// Serialize one conversation message into Responses `input` items.
///
/// `replay_reasoning` replays tool-call thought signatures as `reasoning`
/// items (required by the OpenAI codex endpoint; generic Responses
/// gateways do not use them).
pub fn message_items(message: &ChatMessage, replay_reasoning: bool) -> Vec<Value> {
    match message.role.as_str() {
        "user" => vec![user_item(message)],
        "assistant" => assistant_items(message, replay_reasoning),
        "tool" => vec![json!({
            "type": "function_call_output",
            "call_id": message.tool_call_id.clone().unwrap_or_default(),
            "output": message.content,
        })],
        _ => vec![json!({
            "role": message.role,
            "content": [{"type": "input_text", "text": message.content}]
        })],
    }
}

fn user_item(message: &ChatMessage) -> Value {
    let mut content = Vec::new();
    if !message.content.is_empty() {
        content.push(json!({"type": "input_text", "text": message.content}));
    }
    for image in &message.images {
        content.push(json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", image.media_type, image.data)
        }));
    }
    if content.is_empty() {
        content.push(json!({"type": "input_text", "text": ""}));
    }
    json!({"role": "user", "content": content})
}

fn assistant_items(message: &ChatMessage, replay_reasoning: bool) -> Vec<Value> {
    let mut items = Vec::new();
    if !message.content.trim().is_empty() {
        items.push(json!({
            "role": "assistant",
            "content": [{"type": "output_text", "text": message.content}]
        }));
    }
    // function_call is a standalone input item in the Responses API;
    // nesting it inside assistant content makes strict gateways (xAI)
    // reject the request with "untagged enum ModelInput".
    for call in message.tool_calls.iter().flatten() {
        if replay_reasoning {
            if let Some(encrypted) = call.thought_signature.as_deref() {
                items.push(json!({
                    "type": "reasoning",
                    "encrypted_content": encrypted,
                    "summary": [],
                }));
            }
        }
        items.push(json!({
            "type": "function_call",
            "call_id": call.id,
            "name": call.function.name,
            "arguments": call.function.arguments,
        }));
    }
    items
}

/// Serialize function tool definitions, normalizing each parameter schema
/// to the strict subset accepted by Responses gateways.
pub fn function_tools(tools: &[ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.function.name,
                "description": tool.function.description,
                "parameters": normalize_function_schema(&tool.function.parameters),
            })
        })
        .collect()
}

/// Normalize a function-tool JSON schema to the strict subset accepted by
/// Responses gateways: the root must declare an object type, and every
/// root-level anyOf/oneOf branch must itself declare `"type": "object"`.
/// Nested unions are left untouched (they are legal and sometimes needed,
/// e.g. `edits` accepting an array or a JSON string).
pub fn normalize_function_schema(schema: &Value) -> Value {
    let mut normalized = schema.clone();
    if let Some(root) = normalized.as_object_mut() {
        if !root.contains_key("type") {
            root.insert("type".into(), Value::String("object".into()));
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(branches) = root.get_mut(key).and_then(Value::as_array_mut) {
                for branch in branches.iter_mut() {
                    if let Some(branch) = branch.as_object_mut() {
                        if !branch.contains_key("type") {
                            branch.insert("type".into(), Value::String("object".into()));
                        }
                    }
                }
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::models::{FunctionCall, FunctionSchema, ToolCall};
    use crate::runtime::backend::BackendClient;
    use crate::runtime::tools::register::register_all_tools;
    use crate::runtime::tools::registry::ToolRegistry;
    use std::collections::HashSet;
    use std::sync::Arc;

    fn message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
            thought_signature: None,
        }
    }

    #[test]
    fn assistant_tool_calls_are_standalone_items() {
        let mut msg = message("assistant", "thinking done");
        msg.tool_calls = Some(vec![tool_call("web_query")]);

        let items = message_items(&msg, false);
        assert_eq!(items.len(), 2);
        // Message item carries only output_text parts.
        let content = items[0]["content"].as_array().unwrap();
        assert!(content
            .iter()
            .all(|part| part["type"] == Value::String("output_text".into())));
        // function_call is its own first-level item, never nested in content.
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(items[1]["name"], "web_query");
        assert!(items[0].get("content").unwrap().as_array().unwrap().len() == 1);
    }

    #[test]
    fn assistant_without_content_emits_only_function_call_items() {
        let mut msg = message("assistant", "");
        msg.tool_calls = Some(vec![tool_call("web_query")]);
        let items = message_items(&msg, false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "function_call");
    }

    #[test]
    fn tool_result_maps_to_function_call_output() {
        let mut msg = message("tool", "result body");
        msg.tool_call_id = Some("call_9".into());
        let items = message_items(&msg, false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "function_call_output");
        assert_eq!(items[0]["call_id"], "call_9");
        assert_eq!(items[0]["output"], "result body");
    }

    #[test]
    fn reasoning_replay_only_when_enabled() {
        let mut msg = message("assistant", "");
        let mut call = tool_call("web_query");
        call.thought_signature = Some("sig".into());
        msg.tool_calls = Some(vec![call]);

        let without = message_items(&msg, false);
        assert_eq!(without.len(), 1);
        assert_eq!(without[0]["type"], "function_call");

        let with = message_items(&msg, true);
        assert_eq!(with.len(), 2);
        assert_eq!(with[0]["type"], "reasoning");
        assert_eq!(with[0]["encrypted_content"], "sig");
        assert_eq!(with[1]["type"], "function_call");
    }

    #[test]
    fn normalize_schema_adds_object_type_to_root_union_branches() {
        let schema = json!({
            "type": "object",
            "properties": { "domain": {"type": "string"} },
            "oneOf": [ { "required": ["domain"] }, { "required": ["other"] } ],
            "additionalProperties": false
        });
        let normalized = normalize_function_schema(&schema);
        for branch in normalized["oneOf"].as_array().unwrap() {
            assert_eq!(branch["type"], "object");
        }
        // Non-union schemas keep their shape.
        assert_eq!(normalized["properties"], schema["properties"]);
    }

    #[test]
    fn normalize_schema_leaves_nested_unions_untouched() {
        let schema = json!({
            "type": "object",
            "properties": {
                "edits": { "oneOf": [ {"type": "array"}, {"type": "string"} ] }
            }
        });
        let normalized = normalize_function_schema(&schema);
        let branches = normalized["properties"]["edits"]["oneOf"]
            .as_array()
            .unwrap();
        assert_eq!(branches[0]["type"], "array");
        assert_eq!(branches[1]["type"], "string");
    }

    #[test]
    fn function_tools_apply_normalization() {
        let tools = vec![ToolSchema {
            kind: "function".into(),
            function: FunctionSchema {
                name: "t".into(),
                description: "d".into(),
                parameters: json!({ "oneOf": [ {"required": ["a"]} ] }),
            },
        }];
        let serialized = function_tools(&tools);
        assert_eq!(serialized[0]["type"], "function");
        assert_eq!(serialized[0]["parameters"]["type"], "object");
        assert_eq!(serialized[0]["parameters"]["oneOf"][0]["type"], "object");
    }

    #[tokio::test]
    async fn every_registered_tool_serializes_for_responses_gateways() {
        let backend = Arc::new(BackendClient::new("http://127.0.0.1:9"));
        let mut registry = ToolRegistry::new();
        register_all_tools(&mut registry, backend);
        let schemas = registry.to_schemas();
        assert!(schemas.len() >= 30, "unexpectedly small tool registry");

        let serialized = function_tools(&schemas);
        assert_eq!(serialized.len(), schemas.len());
        let mut names = HashSet::new();
        for tool in serialized {
            let name = tool["name"].as_str().expect("tool name");
            assert!(
                names.insert(name.to_string()),
                "duplicate tool name: {name}"
            );
            assert_eq!(tool["type"], "function", "{name}");
            assert_eq!(tool["parameters"]["type"], "object", "{name}");
            assert!(
                tool["description"]
                    .as_str()
                    .is_some_and(|value| !value.trim().is_empty()),
                "empty description: {name}"
            );
        }
    }
}
