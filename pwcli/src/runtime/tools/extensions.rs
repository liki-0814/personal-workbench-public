use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

use super::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionInfo {
    name: String,
    #[serde(default)]
    tools: Vec<ExtensionTool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionTool {
    name: String,
    #[serde(default = "default_description")]
    description: String,
    #[serde(default = "default_parameters")]
    parameters: Value,
}

fn default_description() -> String {
    "Extension tool".to_string()
}

fn default_parameters() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub async fn register_trusted(registry: Arc<ToolRegistry>) -> Result<usize> {
    let value = crate::app::cli::rpc::extension_host(json!({ "method": "list" })).await?;
    let extensions: Vec<ExtensionInfo> = serde_json::from_value(value)?;
    registry.remove_prefix("ext__");
    let mut count = 0;
    for extension in extensions {
        for tool in extension.tools {
            register_extension_tool(&registry, &extension.name, tool);
            count += 1;
        }
    }
    Ok(count)
}

fn register_extension_tool(registry: &ToolRegistry, extension: &str, tool: ExtensionTool) {
    let extension_name = extension.to_string();
    let source_name = tool.name.clone();
    let registered_name = format!("ext__{}__{}", safe_name(extension), safe_name(&tool.name));
    registry.register_structured_with_impact(
        registered_name,
        tool.description,
        tool.parameters,
        ToolExecutionMode::Parallel,
        // Extension metadata does not currently declare side effects, so fail
        // closed until the extension protocol can provide a trusted impact.
        ToolImpact::ExternalSideEffect,
        Box::new(move |args| {
            let extension = extension_name.clone();
            let name = source_name.clone();
            let args = args.clone();
            Box::pin(async move {
                let result = crate::app::cli::rpc::extension_host(json!({
                    "method": "invokeTool",
                    "extension": extension,
                    "name": name,
                    "args": args
                }))
                .await?;
                if let Some(content) = result.get("content").and_then(Value::as_str) {
                    let mut output = ToolOutput::text(content);
                    output.details = result.get("details").cloned();
                    output.added_tool_names = result
                        .get("addedToolNames")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                    return Ok(output);
                }
                Ok(ToolOutput::text(match result {
                    Value::String(text) => text,
                    other => serde_json::to_string_pretty(&other)?,
                }))
            })
        }),
    );
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_names_are_namespaced_and_safe() {
        assert_eq!(safe_name("demo-tools"), "demo_tools");
    }
}
