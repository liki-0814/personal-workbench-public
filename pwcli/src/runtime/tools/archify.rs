use anyhow::{bail, Context, Result};
use quick_js::Context as JsContext;
use serde_json::{json, Value};

use super::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

const TEMPLATE: &str = include_str!("../../../resources/archify/assets/template.html");
const ARCHITECTURE_RUNTIME: &str =
    include_str!("../../../resources/archify/generated/architecture.js");
const WORKFLOW_RUNTIME: &str = include_str!("../../../resources/archify/generated/workflow.js");
const SEQUENCE_RUNTIME: &str = include_str!("../../../resources/archify/generated/sequence.js");
const DATAFLOW_RUNTIME: &str = include_str!("../../../resources/archify/generated/dataflow.js");
const LIFECYCLE_RUNTIME: &str = include_str!("../../../resources/archify/generated/lifecycle.js");
const MAX_SPEC_BYTES: usize = 2 * 1024 * 1024;

pub fn register(registry: &mut ToolRegistry) {
    registry.register_with_mode_and_impact(
        "archify_reference",
        "在调用 render_archify 前必须调用。按图表类型一次性返回紧凑必填字段、低失败率布局规则和已通过渲染的完整示例。",
        json!({
            "type": "object",
            "properties": {
                "diagram_type": {
                    "type": "string",
                    "enum": ["architecture", "workflow", "sequence", "dataflow", "lifecycle"]
                }
            },
            "required": ["diagram_type"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|arguments| {
            let diagram_type = arguments
                .get("diagram_type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Box::pin(async move {
                let (_, example) = reference_for(&diagram_type)?;
                let example = standard_example(example)?;
                Ok(format!(
                    "# Required contract\n\n{}\n\n# Low-failure construction rules\n\n\
                     - First render with `meta.quality_profile: \"standard\"`; use `showcase` only after a standard render succeeds.\n\
                     - Start from the complete example below and preserve its spacing/layout pattern.\n\
                     - Keep the first pass to at most 10 primary nodes; put secondary facts in cards.\n\
                     - Omit relationship labels unless they explain async, approval, error, return, or cross-boundary behavior.\n\
                     - Do not route through nodes or along container borders; keep nodes at least 16px apart.\n\
                     - For architecture components use `type` plus `pos: [x,y]`; never use component-level `kind`, `x`, `y`, `boundary`, or `card`.\n\
                     - Submit one complete diagram, not a partial patch. A successful render is final; never call render_archify again only to preserve evidence.\n\n\
                     # Complete validated example\n\n{example}",
                    required_contract(&diagram_type)?
                ))
            })
        }),
    );

    registry.register_structured_with_impact(
        "render_archify",
        "生成 Archify 技术图。调用前必须先用 archify_reference 获取所选类型的必填字段、低失败率规则和完整示例。首次渲染使用 standard 质量档、≤10 个主节点和最少关系标签；showcase 严格门禁失败时会自动回退 standard。",
        json!({
            "type": "object",
            "properties": {
                "diagram": {
                    "type": "object",
                    "description": "完整 Archify typed JSON。先调用 archify_reference 获取所选类型的必填契约与完整示例。",
                    "properties": {
                        "schema_version": { "type": "integer", "const": 1 },
                        "diagram_type": {
                            "type": "string",
                            "enum": ["architecture", "workflow", "sequence", "dataflow", "lifecycle"]
                        },
                        "meta": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "minLength": 1 },
                                "quality_profile": {
                                    "type": "string",
                                    "enum": ["standard", "showcase"],
                                    "description": "首次渲染必须使用 standard；showcase 会启用更严格的交叉、间距和标签门禁。"
                                }
                            },
                            "required": ["title"]
                        },
                        "components": {
                            "type": "array",
                            "description": "architecture 必填。禁止 kind/x/y/boundary/card。",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "minLength": 1 },
                                    "type": {
                                        "type": "string",
                                        "enum": ["frontend", "backend", "database", "cloud", "security", "messagebus", "external"]
                                    },
                                    "label": { "type": "string", "minLength": 1 },
                                    "sublabel": { "type": "string" },
                                    "tag": { "type": "string" },
                                    "row": { "type": "integer", "minimum": 0 },
                                    "col": { "type": "integer", "minimum": 0 },
                                    "pos": {
                                        "type": "array",
                                        "items": { "type": "number" },
                                        "minItems": 2,
                                        "maxItems": 2
                                    },
                                    "size": {
                                        "type": "array",
                                        "items": { "type": "number", "exclusiveMinimum": 0 },
                                        "minItems": 2,
                                        "maxItems": 2
                                    }
                                },
                                "required": ["id", "type", "label"],
                                "additionalProperties": true
                            }
                        },
                        "connections": {
                            "type": "array",
                            "description": "architecture 关系。优先省略 label，避免标签与节点重叠；无关系时可为空数组。",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "from": { "type": "string", "minLength": 1 },
                                    "to": { "type": "string", "minLength": 1 },
                                    "label": { "type": "string" },
                                    "variant": { "type": "string" },
                                    "fromSide": { "type": "string", "enum": ["left", "right", "top", "bottom"] },
                                    "toSide": { "type": "string", "enum": ["left", "right", "top", "bottom"] },
                                    "labelAt": {
                                        "type": "array",
                                        "items": { "type": "number" },
                                        "minItems": 2,
                                        "maxItems": 2
                                    },
                                    "labelDx": { "type": "number" },
                                    "labelDy": { "type": "number" }
                                },
                                "required": ["from", "to"],
                                "additionalProperties": true
                            }
                        },
                        "lanes": { "type": "array", "description": "workflow/lifecycle 必填" },
                        "nodes": { "type": "array", "description": "workflow/dataflow 必填" },
                        "edges": { "type": "array", "description": "workflow 必填" },
                        "participants": { "type": "array", "description": "sequence 必填" },
                        "messages": { "type": "array", "description": "sequence 必填" },
                        "stages": { "type": "array", "description": "dataflow 必填" },
                        "flows": { "type": "array", "description": "dataflow 必填" },
                        "states": { "type": "array", "description": "lifecycle 必填" },
                        "transitions": { "type": "array", "description": "lifecycle 必填" }
                    },
                    "required": ["schema_version", "diagram_type", "meta"],
                    "allOf": [
                        {
                            "if": { "properties": { "diagram_type": { "const": "architecture" } } },
                            "then": { "required": ["components"] }
                        },
                        {
                            "if": { "properties": { "diagram_type": { "const": "workflow" } } },
                            "then": { "required": ["lanes", "nodes", "edges"] }
                        },
                        {
                            "if": { "properties": { "diagram_type": { "const": "sequence" } } },
                            "then": { "required": ["participants", "messages"] }
                        },
                        {
                            "if": { "properties": { "diagram_type": { "const": "dataflow" } } },
                            "then": { "required": ["stages", "nodes", "flows"] }
                        },
                        {
                            "if": { "properties": { "diagram_type": { "const": "lifecycle" } } },
                            "then": { "required": ["lanes", "states", "transitions"] }
                        }
                    ],
                    "additionalProperties": true
                }
            },
            "required": ["diagram"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|arguments| {
            let diagram = arguments.get("diagram").cloned().unwrap_or(Value::Null);
            Box::pin(async move {
                let (diagram, normalized_components) = normalize_common_architecture_input(diagram);
                let title = diagram
                    .pointer("/meta/title")
                    .and_then(Value::as_str)
                    .context("diagram.meta.title is required")?
                    .trim()
                    .to_owned();
                if title.is_empty() {
                    bail!("diagram.meta.title is required");
                }
                let (html, fell_back_to_standard, removed_overlapping_labels) =
                    tokio::task::spawn_blocking(move || render_with_quality_fallback(&diagram))
                    .await
                    .context("Archify renderer task failed")??;
                let document = crate::runtime::documents::create_archify(
                    &crate::runtime::settings::local_config::data_dir(),
                    &title,
                    html,
                )?;
                let manifest = document.manifest;
                let mut output = ToolOutput::with_details(
                    format!(
                        "Archify 图表已生成：{}（document_id={}）",
                        manifest.title, manifest.id
                    ),
                    json!({
                        "qualityFallback": fell_back_to_standard.then_some("standard"),
                        "normalizedComponents": normalized_components,
                        "removedOverlappingLabels": removed_overlapping_labels,
                        "documentRef": {
                            "id": manifest.id,
                            "kind": manifest.kind,
                            "title": manifest.title,
                            "revision": manifest.revision,
                            "status": manifest.status,
                            "qaStatus": manifest.qa_status,
                            "runtime": manifest.runtime,
                        }
                    }),
                );
                output.terminate = true;
                Ok(output)
            })
        }),
    );
}

fn required_contract(diagram_type: &str) -> Result<&'static str> {
    match diagram_type {
        "architecture" => Ok(
            "`diagram` requires `schema_version`, `diagram_type`, `meta.title`, and `components`; use `connections` for relationships.",
        ),
        "workflow" => Ok(
            "`diagram` requires `schema_version`, `diagram_type`, `meta.title`, `lanes`, `nodes`, and `edges`.",
        ),
        "sequence" => Ok(
            "`diagram` requires `schema_version`, `diagram_type`, `meta.title`, `participants`, and `messages`.",
        ),
        "dataflow" => Ok(
            "`diagram` requires `schema_version`, `diagram_type`, `meta.title`, `stages`, `nodes`, and `flows`.",
        ),
        "lifecycle" => Ok(
            "`diagram` requires `schema_version`, `diagram_type`, `meta.title`, `lanes`, `states`, and `transitions`.",
        ),
        _ => bail!("unsupported diagram_type {diagram_type:?}"),
    }
}

fn reference_for(diagram_type: &str) -> Result<(&'static str, &'static str)> {
    match diagram_type {
        "architecture" => Ok((
            include_str!("../../../resources/archify/schemas/architecture.schema.json"),
            include_str!("../../../resources/archify/examples/web-app.architecture.json"),
        )),
        "workflow" => Ok((
            include_str!("../../../resources/archify/schemas/workflow.schema.json"),
            include_str!("../../../resources/archify/examples/agent-tool-call.workflow.json"),
        )),
        "sequence" => Ok((
            include_str!("../../../resources/archify/schemas/sequence.schema.json"),
            include_str!("../../../resources/archify/examples/cache-miss-request.sequence.json"),
        )),
        "dataflow" => Ok((
            include_str!("../../../resources/archify/schemas/dataflow.schema.json"),
            include_str!("../../../resources/archify/examples/product-analytics.dataflow.json"),
        )),
        "lifecycle" => Ok((
            include_str!("../../../resources/archify/schemas/lifecycle.schema.json"),
            include_str!("../../../resources/archify/examples/agent-run.lifecycle.json"),
        )),
        _ => bail!("unsupported diagram_type {diagram_type:?}"),
    }
}

fn standard_example(source: &str) -> Result<String> {
    let mut example: Value =
        serde_json::from_str(source).context("decode embedded Archify example")?;
    let meta = example
        .get_mut("meta")
        .and_then(Value::as_object_mut)
        .context("embedded Archify example is missing meta")?;
    meta.insert(
        "quality_profile".to_string(),
        Value::String("standard".to_string()),
    );
    serde_json::to_string_pretty(&example).context("encode embedded Archify example")
}

fn runtime_for(diagram_type: &str) -> Result<&'static str> {
    match diagram_type {
        "architecture" => Ok(ARCHITECTURE_RUNTIME),
        "workflow" => Ok(WORKFLOW_RUNTIME),
        "sequence" => Ok(SEQUENCE_RUNTIME),
        "dataflow" => Ok(DATAFLOW_RUNTIME),
        "lifecycle" => Ok(LIFECYCLE_RUNTIME),
        _ => bail!(
            "unsupported diagram_type {diagram_type:?}; expected architecture, workflow, sequence, dataflow, or lifecycle"
        ),
    }
}

pub fn render_html(diagram: &Value) -> Result<String> {
    validate_required_fields(diagram)?;
    let serialized = serde_json::to_string(diagram)?;
    if serialized.len() > MAX_SPEC_BYTES {
        bail!("Archify diagram exceeds 2 MiB");
    }
    let diagram_type = diagram
        .get("diagram_type")
        .and_then(Value::as_str)
        .context("diagram.diagram_type is required")?;
    let runtime = runtime_for(diagram_type)?;
    let script = format!(
        "if (!Array.prototype.at) {{ Array.prototype.at = function(index) {{\
           index = Math.trunc(index) || 0;\
           if (index < 0) index += this.length;\
           return this[index];\
         }}; }}\
         if (!Object.hasOwn) {{ Object.hasOwn = function(object, key) {{\
           return Object.prototype.hasOwnProperty.call(object, key);\
         }}; }}\
         if (!String.prototype.replaceAll) {{ String.prototype.replaceAll = function(search, replacement) {{\
           return this.split(search).join(replacement);\
         }}; }}\
         globalThis.__ARCHIFY_INPUT__={serialized};\
         globalThis.__ARCHIFY_TEMPLATE__={template};\
         globalThis.__ARCHIFY_OUTPUT__=null;\
         globalThis.__ARCHIFY_ERROR__=null;\
         try {{{runtime}}} catch (error) {{\
           globalThis.__ARCHIFY_ERROR__=String(error && error.message ? error.message : error);\
         }}\
         JSON.stringify({{output:globalThis.__ARCHIFY_OUTPUT__,error:globalThis.__ARCHIFY_ERROR__}});",
        template = serde_json::to_string(TEMPLATE)?,
    );
    let context = JsContext::new().context("initialize embedded Archify JavaScript runtime")?;
    let encoded = context.eval_as::<String>(&script).map_err(|error| {
        anyhow::anyhow!(
            "Archify rendering failed for {diagram_type}: {error}. \
             Call archify_reference with diagram_type={diagram_type:?}, then correct the reported schema or layout constraints."
        )
    })?;
    let result: Value =
        serde_json::from_str(&encoded).context("decode embedded Archify renderer result")?;
    if let Some(error) = result.get("error").and_then(Value::as_str) {
        bail!(
            "Archify rendering failed for {diagram_type}: {error}. \
             Call archify_reference with diagram_type={diagram_type:?}, then correct the reported schema or layout constraints."
        );
    }
    result
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("Archify renderer returned no HTML output")
}

fn render_with_quality_fallback(diagram: &Value) -> Result<(String, bool, bool)> {
    let mut candidate = diagram.clone();
    let mut fell_back_to_standard = false;
    let mut latest_error = match render_html(&candidate) {
        Ok(html) => return Ok((html, false, false)),
        Err(error) => error,
    };
    if candidate
        .pointer("/meta/quality_profile")
        .and_then(Value::as_str)
        == Some("showcase")
    {
        let meta = candidate
            .get_mut("meta")
            .and_then(Value::as_object_mut)
            .context("diagram.meta must be an object")?;
        meta.insert(
            "quality_profile".to_string(),
            Value::String("standard".to_string()),
        );
        fell_back_to_standard = true;
        match render_html(&candidate) {
            Ok(html) => return Ok((html, true, false)),
            Err(error) => latest_error = error,
        }
    }
    if candidate.get("diagram_type").and_then(Value::as_str) == Some("architecture")
        && is_connection_label_overlap(&latest_error)
        && remove_connection_labels(&mut candidate)
    {
        return render_html(&candidate)
            .map(|html| (html, fell_back_to_standard, true))
            .map_err(|fallback_error| {
                anyhow::anyhow!(
                    "Archify render failed: {latest_error}; label-free fallback also failed: {fallback_error}"
                )
            });
    }
    Err(latest_error)
}

fn is_connection_label_overlap(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("Label \"") && message.contains("overlaps component")
}

fn normalize_common_architecture_input(mut diagram: Value) -> (Value, usize) {
    if diagram.get("diagram_type").and_then(Value::as_str) != Some("architecture") {
        return (diagram, 0);
    }
    let Some(components) = diagram.get_mut("components").and_then(Value::as_array_mut) else {
        return (diagram, 0);
    };
    let mut normalized = 0;
    for component in components {
        let Some(component) = component.as_object_mut() else {
            continue;
        };
        let mut changed = false;
        let legacy_kind = component
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if !component.contains_key("type") {
            if let Some(kind) = legacy_kind.as_deref().and_then(normalize_component_type) {
                component.insert("type".into(), Value::String(kind.into()));
                changed = true;
            }
        }
        if !component.contains_key("pos") {
            if let (Some(x), Some(y)) = (
                component.get("x").and_then(Value::as_f64),
                component.get("y").and_then(Value::as_f64),
            ) {
                component.insert("pos".into(), json!([x, y]));
                changed = true;
            }
        }
        if !component.contains_key("sublabel") {
            if let Some(title) = component
                .get("card")
                .and_then(|card| card.get("title"))
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                component.insert("sublabel".into(), Value::String(title));
                changed = true;
            }
        }
        for unsupported in ["kind", "x", "y", "boundary", "card"] {
            changed |= component.remove(unsupported).is_some();
        }
        if changed {
            normalized += 1;
        }
    }
    (diagram, normalized)
}

fn normalize_component_type(kind: &str) -> Option<&'static str> {
    match kind {
        "frontend" => Some("frontend"),
        "service" | "backend" => Some("backend"),
        "datastore" | "database" => Some("database"),
        "cloud" => Some("cloud"),
        "security" => Some("security"),
        "messagebus" | "queue" => Some("messagebus"),
        "external" => Some("external"),
        _ => None,
    }
}

fn remove_connection_labels(diagram: &mut Value) -> bool {
    let Some(connections) = diagram.get_mut("connections").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut removed = false;
    for connection in connections {
        let Some(connection) = connection.as_object_mut() else {
            continue;
        };
        removed |= connection.remove("label").is_some();
        connection.remove("labelAt");
        connection.remove("labelDx");
        connection.remove("labelDy");
        connection.remove("labelSegment");
    }
    removed
}

fn validate_required_fields(diagram: &Value) -> Result<()> {
    if diagram.get("schema_version").and_then(Value::as_u64) != Some(1) {
        bail!("diagram.schema_version must be 1");
    }
    let diagram_type = diagram
        .get("diagram_type")
        .and_then(Value::as_str)
        .context("diagram.diagram_type is required")?;
    if diagram
        .pointer("/meta/title")
        .and_then(Value::as_str)
        .is_none_or(|title| title.trim().is_empty())
    {
        bail!("diagram.meta.title is required");
    }
    let required = match diagram_type {
        "architecture" => &["components"][..],
        "workflow" => &["lanes", "nodes", "edges"][..],
        "sequence" => &["participants", "messages"][..],
        "dataflow" => &["stages", "nodes", "flows"][..],
        "lifecycle" => &["lanes", "states", "transitions"][..],
        _ => bail!("unsupported diagram_type {diagram_type:?}"),
    };
    for field in required {
        if !diagram.get(*field).is_some_and(Value::is_array) {
            bail!("diagram.{field} is required and must be an array for {diagram_type}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_renderer_matches_archify_contract() {
        let diagram: Value = serde_json::from_str(include_str!(
            "../../../resources/archify/examples/web-app.architecture.json"
        ))
        .unwrap();
        let html = render_html(&diagram).unwrap();
        assert!(html.contains("data-node-id=\"users\""));
        assert!(html.contains("data-edge-from=\"users\" data-edge-to=\"cdn\""));
        assert!(html.contains("Archify.guidedViews = (function ()"));
        assert!(html.contains("Archify.exportMenu"));
    }

    #[test]
    fn escapes_untrusted_node_labels_in_rendered_html() {
        let mut diagram: Value = serde_json::from_str(include_str!(
            "../../../resources/archify/examples/web-app.architecture.json"
        ))
        .unwrap();
        diagram["components"][0]["label"] = json!("<img onerror=x>");

        let html = render_html(&diagram).unwrap();

        assert!(!html.contains("<img onerror=x>"));
        assert!(html.contains("&lt;img onerror=x&gt;"));
    }

    #[test]
    fn rejects_unknown_diagram_type() {
        let error = render_html(&json!({
            "schema_version": 1,
            "diagram_type": "mindmap",
            "meta": { "title": "Nope" }
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unsupported diagram_type"));
    }

    #[test]
    fn rejects_missing_type_specific_fields_before_rendering() {
        let error = render_html(&json!({
            "schema_version": 1,
            "diagram_type": "workflow",
            "meta": { "title": "Incomplete" },
            "lanes": []
        }))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "diagram.nodes is required and must be an array for workflow"
        );
    }

    #[test]
    fn reference_example_is_returned_with_standard_quality() {
        let (_, source) = reference_for("architecture").unwrap();
        let example = standard_example(source).unwrap();
        let decoded: Value = serde_json::from_str(&example).unwrap();
        assert_eq!(
            decoded
                .pointer("/meta/quality_profile")
                .and_then(Value::as_str),
            Some("standard")
        );
        render_html(&decoded).unwrap();
    }

    #[test]
    fn showcase_layout_failure_falls_back_to_standard() {
        let mut diagram: Value = serde_json::from_str(include_str!(
            "../../../resources/archify/examples/web-app.architecture.json"
        ))
        .unwrap();
        diagram["connections"][0]["via"] = json!([[229, 136], [237, 136]]);
        let (_, fell_back, removed_labels) = render_with_quality_fallback(&diagram).unwrap();
        assert!(fell_back);
        assert!(!removed_labels);
    }

    #[test]
    fn normalizes_common_legacy_architecture_component_fields() {
        let (diagram, normalized_components) = normalize_common_architecture_input(json!({
            "schema_version": 1,
            "diagram_type": "architecture",
            "meta": { "title": "Legacy input", "quality_profile": "standard" },
            "components": [{
                "id": "frontend",
                "label": "Frontend",
                "kind": "service",
                "x": 40,
                "y": 40,
                "boundary": "client",
                "card": { "title": "React" }
            }],
            "connections": []
        }));
        assert_eq!(normalized_components, 1);
        assert_eq!(
            diagram
                .pointer("/components/0/type")
                .and_then(Value::as_str),
            Some("backend")
        );
        assert_eq!(
            diagram.pointer("/components/0/pos"),
            Some(&json!([40.0, 40.0]))
        );
        assert_eq!(
            diagram
                .pointer("/components/0/sublabel")
                .and_then(Value::as_str),
            Some("React")
        );
        for field in ["kind", "x", "y", "boundary", "card"] {
            assert!(diagram.pointer(&format!("/components/0/{field}")).is_none());
        }
        render_html(&diagram).unwrap();
    }

    #[test]
    fn connection_label_overlap_retries_without_labels() {
        let diagram = json!({
            "schema_version": 1,
            "diagram_type": "architecture",
            "meta": { "title": "Label overlap", "quality_profile": "standard" },
            "components": [
                {
                    "id": "flows",
                    "type": "backend",
                    "label": "Debug Flows",
                    "pos": [740, 280],
                    "size": [150, 64]
                },
                {
                    "id": "warehouse",
                    "type": "database",
                    "label": "Data Warehouse",
                    "pos": [740, 460],
                    "size": [150, 60]
                }
            ],
            "connections": [{
                "id": "flows-to-warehouse",
                "from": "flows",
                "to": "warehouse",
                "fromSide": "bottom",
                "toSide": "top",
                "label": "只读 SQL"
            }]
        });
        let direct_error = render_html(&diagram).unwrap_err();
        assert!(is_connection_label_overlap(&direct_error));
        let (_, fell_back, removed_labels) = render_with_quality_fallback(&diagram).unwrap();
        assert!(!fell_back);
        assert!(removed_labels);
    }

    #[test]
    fn renderer_failure_preserves_actionable_layout_diagnostics() {
        let error = render_html(&json!({
            "schema_version": 1,
            "diagram_type": "architecture",
            "meta": { "title": "Overlapping nodes" },
            "components": [
                { "id": "a", "type": "backend", "label": "A", "pos": [40, 40] },
                { "id": "b", "type": "backend", "label": "B", "pos": [40, 40] }
            ],
            "connections": []
        }))
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Archify rendering failed for architecture"));
        assert!(message.contains("less than 8px apart"), "{message}");
        assert!(message.contains("archify_reference"));
    }

    #[test]
    fn embedded_runtime_supports_all_five_renderers() {
        for source in [
            include_str!("../../../resources/archify/examples/web-app.architecture.json"),
            include_str!("../../../resources/archify/examples/agent-tool-call.workflow.json"),
            include_str!("../../../resources/archify/examples/cache-miss-request.sequence.json"),
            include_str!("../../../resources/archify/examples/product-analytics.dataflow.json"),
            include_str!("../../../resources/archify/examples/agent-run.lifecycle.json"),
        ] {
            let diagram: Value = serde_json::from_str(source).unwrap();
            let html = render_html(&diagram).unwrap();
            assert!(html.contains("Built with Archify"));
            assert!(html.contains("data-node-id="));
            assert!(html.contains("Archify.exportMenu"));
        }
    }
}
