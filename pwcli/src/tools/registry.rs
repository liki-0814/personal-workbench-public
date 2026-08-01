use crate::llm::{FunctionSchema, ToolSchema};
use std::sync::{Arc, RwLock};

use crate::tools::progress::{self, ImageEmitter, ProgressEmitter};
use crate::tools::web_cache::WebFetchCache;
use crate::tools::web_context;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// 工具 handler 类型（异步）
pub type ToolHandler =
    Box<dyn Fn(&Value) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync>;

pub type StructuredToolHandler =
    Box<dyn Fn(&Value) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send>> + Send + Sync>;

type SharedToolHandler =
    Arc<dyn Fn(&Value) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync>;
type SharedStructuredToolHandler =
    Arc<dyn Fn(&Value) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send>> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    /// Requests agent termination after the current batch. A mixed batch only
    /// terminates when every result opts in, matching Pi's batch semantics.
    pub terminate: bool,
    pub details: Option<Value>,
    pub added_tool_names: Vec<String>,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            terminate: false,
            details: None,
            added_tool_names: Vec::new(),
        }
    }

    pub fn terminating(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            terminate: true,
            details: None,
            added_tool_names: Vec::new(),
        }
    }

    pub fn with_details(content: impl Into<String>, details: Value) -> Self {
        Self {
            content: content.into(),
            terminate: false,
            details: Some(details),
            added_tool_names: Vec::new(),
        }
    }
}

#[derive(Clone)]
enum RegisteredToolHandler {
    Legacy(SharedToolHandler),
    Structured(SharedStructuredToolHandler),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Sequential,
}

/// Shared effect classification used by permission policy and the Harness.
/// Permissions decide whether a call needs approval, while the Harness uses
/// the same impact to decide whether independent review is warranted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolImpact {
    Observe,
    ReversibleMutation,
    IrreversibleMutation,
    ExternalSideEffect,
    Control,
}

/// Resolve the impact of a specific call from its validated arguments. Tools
/// such as `run_command` can be either observational or mutating, so a static
/// classification alone is too coarse for Harness decision routing.
pub type ToolImpactResolver = fn(&Value) -> ToolImpact;

impl ToolImpact {
    pub fn requires_decision(self) -> bool {
        matches!(self, Self::IrreversibleMutation | Self::ExternalSideEffect)
    }
}

/// 工具定义（内部格式）
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub execution_mode: ToolExecutionMode,
    pub impact: ToolImpact,
    impact_resolver: Option<ToolImpactResolver>,
    validator: Option<jsonschema::Validator>,
    schema_error: Option<String>,
}

/// 工具注册表
pub struct ToolRegistry {
    definitions: RwLock<HashMap<String, ToolDefinition>>,
    handlers: RwLock<HashMap<String, RegisteredToolHandler>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            definitions: RwLock::new(HashMap::new()),
            handlers: RwLock::new(HashMap::new()),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        handler: ToolHandler,
    ) {
        self.register_with_mode(
            name,
            description,
            parameters,
            ToolExecutionMode::Parallel,
            handler,
        );
    }

    pub fn register_with_mode(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execution_mode: ToolExecutionMode,
        handler: ToolHandler,
    ) {
        self.register_with_mode_and_impact(
            name,
            description,
            parameters,
            execution_mode,
            ToolImpact::Observe,
            handler,
        );
    }

    pub fn register_with_impact(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        impact: ToolImpact,
        handler: ToolHandler,
    ) {
        self.register_with_mode_and_impact(
            name,
            description,
            parameters,
            ToolExecutionMode::Parallel,
            impact,
            handler,
        );
    }

    pub fn register_with_mode_and_impact(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execution_mode: ToolExecutionMode,
        impact: ToolImpact,
        handler: ToolHandler,
    ) {
        let name = name.into();
        let (validator, schema_error) = match jsonschema::validator_for(&parameters) {
            Ok(validator) => (Some(validator), None),
            Err(error) => (None, Some(error.to_string())),
        };
        self.definitions
            .write()
            .expect("tool definitions poisoned")
            .insert(
                name.clone(),
                ToolDefinition {
                    name: name.clone(),
                    description: description.into(),
                    parameters,
                    execution_mode,
                    impact,
                    impact_resolver: None,
                    validator,
                    schema_error,
                },
            );
        self.handlers
            .write()
            .expect("tool handlers poisoned")
            .insert(name, RegisteredToolHandler::Legacy(Arc::from(handler)));
    }

    pub fn register_with_impact_resolver(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        default_impact: ToolImpact,
        impact_resolver: ToolImpactResolver,
        handler: ToolHandler,
    ) {
        let name = name.into();
        self.register_with_impact(
            name.clone(),
            description,
            parameters,
            default_impact,
            handler,
        );
        if let Some(definition) = self
            .definitions
            .write()
            .expect("tool definitions poisoned")
            .get_mut(&name)
        {
            definition.impact_resolver = Some(impact_resolver);
        }
    }

    pub fn register_structured(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execution_mode: ToolExecutionMode,
        handler: StructuredToolHandler,
    ) {
        self.register_structured_with_impact(
            name,
            description,
            parameters,
            execution_mode,
            ToolImpact::Observe,
            handler,
        );
    }

    pub fn register_structured_with_impact(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execution_mode: ToolExecutionMode,
        impact: ToolImpact,
        handler: StructuredToolHandler,
    ) {
        let name = name.into();
        let (validator, schema_error) = match jsonschema::validator_for(&parameters) {
            Ok(validator) => (Some(validator), None),
            Err(error) => (None, Some(error.to_string())),
        };
        self.definitions
            .write()
            .expect("tool definitions poisoned")
            .insert(
                name.clone(),
                ToolDefinition {
                    name: name.clone(),
                    description: description.into(),
                    parameters,
                    execution_mode,
                    impact,
                    impact_resolver: None,
                    validator,
                    schema_error,
                },
            );
        self.handlers
            .write()
            .expect("tool handlers poisoned")
            .insert(name, RegisteredToolHandler::Structured(Arc::from(handler)));
    }

    pub fn remove(&self, name: &str) {
        self.definitions
            .write()
            .expect("tool definitions poisoned")
            .remove(name);
        self.handlers
            .write()
            .expect("tool handlers poisoned")
            .remove(name);
    }

    pub fn remove_prefix(&self, prefix: &str) {
        self.definitions
            .write()
            .expect("tool definitions poisoned")
            .retain(|name, _| !name.starts_with(prefix));
        self.handlers
            .write()
            .expect("tool handlers poisoned")
            .retain(|name, _| !name.starts_with(prefix));
    }

    pub fn retain_only(&self, allowed: &[&str]) {
        self.definitions
            .write()
            .expect("tool definitions poisoned")
            .retain(|name, _| allowed.contains(&name.as_str()));
        self.handlers
            .write()
            .expect("tool handlers poisoned")
            .retain(|name, _| allowed.contains(&name.as_str()));
    }

    pub fn get_definition(&self, name: &str) -> Option<ToolDefinition> {
        self.definitions
            .read()
            .expect("tool definitions poisoned")
            .get(name)
            .cloned()
    }

    pub fn list_definitions(&self) -> Vec<ToolDefinition> {
        self.definitions
            .read()
            .expect("tool definitions poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub fn to_schemas(&self) -> Vec<ToolSchema> {
        self.definitions
            .read()
            .expect("tool definitions poisoned")
            .values()
            .map(|d| ToolSchema {
                kind: "function".to_string(),
                function: FunctionSchema {
                    name: d.name.clone(),
                    description: d.description.clone(),
                    parameters: d.parameters.clone(),
                },
            })
            .collect()
    }

    pub async fn execute(&self, name: &str, args: &Value) -> Result<String> {
        Ok(self
            .execute_with_emitters_output(name, args, None, None, None, None)
            .await?
            .content)
    }

    /// Same as [`execute`], but inject a progress + image emitter into task-local storage
    /// so long-running tools can stream events back to the caller's UI:
    /// - `progress` 接收 "🔧 Read foo.rs" 这种进度行（`code_agent` 用）
    /// - `image` 接收生成的图片 URL（`generate_image` 用），前端 push 到 `generatedImages[]`
    ///
    /// Tools that don't call the corresponding `progress::emit*` helpers silently
    /// ignore the emitters.
    pub async fn execute_with_emitters(
        &self,
        name: &str,
        args: &Value,
        progress: Option<ProgressEmitter>,
        image: Option<ImageEmitter>,
        web_cache: Option<Arc<WebFetchCache>>,
        session_id: Option<String>,
    ) -> Result<String> {
        Ok(self
            .execute_with_emitters_output(name, args, progress, image, web_cache, session_id)
            .await?
            .content)
    }

    pub async fn execute_with_emitters_output(
        &self,
        name: &str,
        args: &Value,
        progress: Option<ProgressEmitter>,
        image: Option<ImageEmitter>,
        web_cache: Option<Arc<WebFetchCache>>,
        session_id: Option<String>,
    ) -> Result<ToolOutput> {
        let normalized_args = normalize_arguments_before_validation(name, args)?;
        let args = normalized_args.as_ref().unwrap_or(args);
        self.validate_arguments(name, args)?;
        let handler = self
            .handlers
            .read()
            .expect("tool handlers poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;
        match handler {
            RegisteredToolHandler::Legacy(handler) => Ok(ToolOutput::text(
                web_context::with_web_context(
                    web_cache,
                    session_id,
                    progress::with_emitters(progress, image, handler(args)),
                )
                .await?,
            )),
            RegisteredToolHandler::Structured(handler) => {
                web_context::with_web_context(
                    web_cache,
                    session_id,
                    progress::with_emitters(progress, image, handler(args)),
                )
                .await
            }
        }
    }

    pub fn validate_arguments(&self, name: &str, args: &Value) -> Result<()> {
        let definitions = self.definitions.read().expect("tool definitions poisoned");
        let definition = definitions
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;
        if let Some(error) = &definition.schema_error {
            anyhow::bail!("Invalid schema for tool '{}': {}", name, error);
        }
        if let Some(validator) = &definition.validator {
            if let Err(error) = validator.validate(args) {
                anyhow::bail!("Invalid arguments for tool '{}': {}", name, error);
            }
        }
        Ok(())
    }

    pub fn execution_mode(&self, name: &str) -> Option<ToolExecutionMode> {
        self.definitions
            .read()
            .expect("tool definitions poisoned")
            .get(name)
            .map(|definition| definition.execution_mode)
    }

    pub fn impact(&self, name: &str) -> Option<ToolImpact> {
        self.definitions
            .read()
            .expect("tool definitions poisoned")
            .get(name)
            .map(|definition| definition.impact)
    }

    pub fn impact_for_call(&self, name: &str, args: &Value) -> Option<ToolImpact> {
        self.definitions
            .read()
            .expect("tool definitions poisoned")
            .get(name)
            .map(|definition| {
                definition
                    .impact_resolver
                    .map(|resolver| resolver(args))
                    .unwrap_or(definition.impact)
            })
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.definitions
            .read()
            .expect("tool definitions poisoned")
            .contains_key(name)
    }
}

fn normalize_arguments_before_validation(name: &str, args: &Value) -> Result<Option<Value>> {
    if name != "create_document" {
        return Ok(None);
    }
    let Some(raw) = args.get("content").and_then(Value::as_str) else {
        return Ok(None);
    };
    let content: Value = serde_json::from_str(raw).map_err(|error| {
        anyhow::anyhow!(
            "Invalid arguments for tool 'create_document' at /content: expected a JSON object; received a JSON string that could not be decoded: {error}"
        )
    })?;
    if !content.is_object() {
        anyhow::bail!(
            "Invalid arguments for tool 'create_document' at /content: decoded value must be an object, received {}",
            match content {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            }
        );
    }
    let mut normalized = args.clone();
    normalized["content"] = content;
    Ok(Some(normalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_execute() {
        let registry = ToolRegistry::new();
        registry.register(
            "echo",
            "Echo tool",
            serde_json::json!({"type": "object", "properties": {}}),
            Box::new(|args| {
                let msg = args["msg"].as_str().unwrap_or("").to_string();
                Box::pin(async move { Ok(msg) })
            }),
        );

        assert!(registry.has_tool("echo"));
        let result = registry
            .execute("echo", &serde_json::json!({"msg": "hello"}))
            .await
            .unwrap();
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_to_schemas() {
        let registry = ToolRegistry::new();
        registry.register(
            "test",
            "Test tool",
            serde_json::json!({"type": "object"}),
            Box::new(|_| Box::pin(async move { Ok("".to_string()) })),
        );
        let schemas = registry.to_schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].function.name, "test");
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let registry = ToolRegistry::new();
        let result = registry.execute("unknown", &serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rejects_arguments_that_do_not_match_schema() {
        let registry = ToolRegistry::new();
        registry.register(
            "required_string",
            "Requires a string value",
            serde_json::json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
                "additionalProperties": false
            }),
            Box::new(|_| Box::pin(async move { Ok("executed".to_string()) })),
        );

        let missing = registry
            .execute("required_string", &serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(missing.contains("Invalid arguments"));

        let wrong_type = registry
            .execute("required_string", &serde_json::json!({ "value": 42 }))
            .await
            .unwrap_err()
            .to_string();
        assert!(wrong_type.contains("Invalid arguments"));
    }

    #[test]
    fn create_document_decodes_stringified_content_before_validation() {
        let args = serde_json::json!({
            "kind": "report",
            "title": "Report",
            "content": "{\"markdown\":\"# Title\",\"style\":{}}"
        });
        let normalized = normalize_arguments_before_validation("create_document", &args)
            .unwrap()
            .unwrap();
        assert_eq!(normalized["content"]["markdown"], "# Title");
        assert!(
            normalize_arguments_before_validation("inspect_document", &args)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn create_document_rejects_invalid_stringified_content_with_pointer() {
        let error = normalize_arguments_before_validation(
            "create_document",
            &serde_json::json!({ "content": "not-json" }),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("/content"));
        assert!(error.contains("expected a JSON object"));
    }

    #[tokio::test]
    async fn test_invalid_schema_never_reaches_handler() {
        let registry = ToolRegistry::new();
        registry.register(
            "broken",
            "Broken schema",
            serde_json::json!({ "type": 42 }),
            Box::new(|_| Box::pin(async move { Ok("executed".to_string()) })),
        );

        let error = registry
            .execute("broken", &serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Invalid schema"));
    }

    #[test]
    fn test_execution_mode_defaults_to_parallel_and_can_be_sequential() {
        let registry = ToolRegistry::new();
        registry.register(
            "parallel",
            "parallel",
            serde_json::json!({}),
            Box::new(|_| Box::pin(async move { Ok(String::new()) })),
        );
        registry.register_with_mode(
            "sequential",
            "sequential",
            serde_json::json!({}),
            ToolExecutionMode::Sequential,
            Box::new(|_| Box::pin(async move { Ok(String::new()) })),
        );
        assert_eq!(
            registry.execution_mode("parallel"),
            Some(ToolExecutionMode::Parallel)
        );
        assert_eq!(
            registry.execution_mode("sequential"),
            Some(ToolExecutionMode::Sequential)
        );
    }

    #[test]
    fn only_irreversible_or_external_tools_require_independent_review() {
        assert!(!ToolImpact::Observe.requires_decision());
        assert!(!ToolImpact::ReversibleMutation.requires_decision());
        assert!(ToolImpact::IrreversibleMutation.requires_decision());
        assert!(ToolImpact::ExternalSideEffect.requires_decision());
        assert!(!ToolImpact::Control.requires_decision());
    }

    #[test]
    fn call_impact_resolver_can_refine_a_static_default() {
        fn resolver(args: &Value) -> ToolImpact {
            if args.get("read_only").and_then(Value::as_bool) == Some(true) {
                ToolImpact::Observe
            } else {
                ToolImpact::ExternalSideEffect
            }
        }

        let registry = ToolRegistry::new();
        registry.register_with_impact_resolver(
            "dynamic",
            "dynamic",
            serde_json::json!({ "type": "object" }),
            ToolImpact::ExternalSideEffect,
            resolver,
            Box::new(|_| Box::pin(async move { Ok(String::new()) })),
        );

        assert_eq!(
            registry.impact_for_call("dynamic", &serde_json::json!({ "read_only": true })),
            Some(ToolImpact::Observe)
        );
        assert_eq!(
            registry.impact_for_call("dynamic", &serde_json::json!({})),
            Some(ToolImpact::ExternalSideEffect)
        );
    }

    #[tokio::test]
    async fn structured_handler_preserves_termination_signal() {
        let registry = ToolRegistry::new();
        registry.register_structured(
            "quit",
            "quit",
            serde_json::json!({ "type": "object" }),
            ToolExecutionMode::Parallel,
            Box::new(|_| Box::pin(async { Ok(ToolOutput::terminating("bye")) })),
        );

        let output = registry
            .execute_with_emitters_output("quit", &serde_json::json!({}), None, None, None, None)
            .await
            .unwrap();
        assert_eq!(output.content, "bye");
        assert!(output.terminate);
        assert_eq!(
            registry
                .execute("quit", &serde_json::json!({}))
                .await
                .unwrap(),
            "bye"
        );
    }
}
