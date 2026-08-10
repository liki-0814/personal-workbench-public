use serde_json::Value;

use crate::ai::config::ProviderConfig;
use crate::ai::llm::models::ToolSchema;

pub fn supports_root_combinators(provider: &ProviderConfig) -> bool {
    provider
        .current_model_entry()
        .and_then(|model| model.capabilities.as_ref())
        .and_then(|capabilities| capabilities.tool_schema_top_level_combinators)
        .unwrap_or(true)
}

pub fn normalize_root(schema: &Value, supports_root_combinators: bool) -> Value {
    if supports_root_combinators {
        return schema.clone();
    }
    let mut normalized = schema.clone();
    if let Some(root) = normalized.as_object_mut() {
        root.remove("oneOf");
        root.remove("anyOf");
        root.remove("allOf");
        if !root.contains_key("type") {
            root.insert("type".into(), Value::String("object".into()));
        }
    }
    normalized
}

pub fn normalize_tools(tools: &[ToolSchema], provider: &ProviderConfig) -> Vec<ToolSchema> {
    let supports = supports_root_combinators(provider);
    tools
        .iter()
        .cloned()
        .map(|mut tool| {
            tool.function.parameters = normalize_root(&tool.function.parameters, supports);
            tool
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn restricted_root_keeps_nested_combinators() {
        let schema = json!({
            "type": "object",
            "properties": { "value": { "oneOf": [{"type": "string"}, {"type": "array"}] } },
            "oneOf": [{"required": ["value"]}],
            "anyOf": [{"required": ["value"]}],
            "allOf": [{"required": ["value"]}]
        });
        let normalized = normalize_root(&schema, false);
        assert!(normalized.get("oneOf").is_none());
        assert!(normalized.get("anyOf").is_none());
        assert!(normalized.get("allOf").is_none());
        assert!(normalized["properties"]["value"].get("oneOf").is_some());
    }
}
