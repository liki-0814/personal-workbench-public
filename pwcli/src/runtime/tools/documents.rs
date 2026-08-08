use std::path::Path;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::runtime::documents::{CreateDocument, DocumentKind, EvidenceEntry, UpdateDocument};

use super::fs_local::FsSandbox;
use super::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateArgs {
    kind: DocumentKind,
    title: String,
    #[serde(default)]
    content: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InspectArgs {
    document_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InspectFragmentsArgs {
    document_id: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default = "default_fragment_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchArgs {
    document_id: String,
    expected_revision: u64,
    patches: Vec<PatchOperation>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct PatchOperation {
    op: PatchKind,
    path: String,
    #[serde(default, rename = "matchText")]
    match_text: Option<String>,
    #[serde(default, rename = "fragmentId")]
    fragment_id: Option<String>,
    #[serde(default)]
    value: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum PatchKind {
    Add,
    Replace,
    Remove,
    ReplaceText,
    ReplaceFragment,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachArgs {
    document_id: String,
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    image_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordEvidenceArgs {
    document_id: String,
    expected_revision: u64,
    entries: Vec<EvidenceEntry>,
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register_structured_with_impact(
        "create_document",
        "创建一个可编辑 HTML 文档。HTML 同时承载内容、排版、图表和打印 CSS；报告或演示只是版式意图，不是不同协议。图片必须先复制到 assets/，不得依赖远程 URL。",
        create_document_schema(),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let mut args: CreateArgs =
                    serde_json::from_value(value).context("invalid create_document arguments")?;
                args.content = decode_content_argument(args.content)?;
                let data_dir = crate::runtime::settings::local_config::data_dir();
                let document = crate::runtime::documents::create(
                    &data_dir,
                    CreateDocument {
                        kind: args.kind,
                        title: args.title,
                        content: args.content,
                    },
                )?;
                Ok(document_output("HTML 文档已创建", &document.manifest))
            })
        }),
    );

    registry.register_structured_with_impact(
        "inspect_document",
        "读取托管 HTML 文档的当前 revision 和完整 HTML。修改前必须回读，并使用精确片段做定点 patch。",
        id_schema(),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: InspectArgs =
                    serde_json::from_value(value).context("invalid inspect_document arguments")?;
                let document = crate::runtime::documents::read(
                    &crate::runtime::settings::local_config::data_dir(),
                    &args.document_id,
                )?;
                Ok(ToolOutput::text(serde_json::to_string_pretty(&document)?))
            })
        }),
    );

    registry.register_structured_with_impact(
        "inspect_document_evidence",
        "读取文档的 Evidence Ledger，并标明它与当前文档 revision 是 current、stale 还是 missing。复审时先复用 current 证据，只补真实缺口。",
        id_schema(),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: InspectArgs = serde_json::from_value(value)
                    .context("invalid inspect_document_evidence arguments")?;
                let view = crate::runtime::documents::read_evidence_ledger(
                    &crate::runtime::settings::local_config::data_dir(),
                    &args.document_id,
                )?;
                Ok(ToolOutput::text(serde_json::to_string_pretty(&view)?))
            })
        }),
    );

    registry.register_structured_with_impact(
        "inspect_document_fragments",
        "读取文档中可独立修改的标题、段落、列表项、引文和表格单元，返回稳定 fragmentId。复审长文档时优先用它定位内容，再用 patch_document.replaceFragment 修改，无需逐字复制旧 HTML。",
        json!({
            "type": "object",
            "properties": {
                "documentId": { "type": "string" },
                "query": { "type": "string", "description": "可选；仅返回可见文本包含此内容的片段" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 32 }
            },
            "required": ["documentId"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: InspectFragmentsArgs = serde_json::from_value(value)
                    .context("invalid inspect_document_fragments arguments")?;
                let document = crate::runtime::documents::read(
                    &crate::runtime::settings::local_config::data_dir(),
                    &args.document_id,
                )?;
                let html = document
                    .content
                    .get("html")
                    .and_then(Value::as_str)
                    .context("document content does not contain HTML")?;
                let query = args
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|query| !query.is_empty())
                    .map(str::to_lowercase);
                let fragments = editable_html_fragments(html)
                    .into_iter()
                    .filter(|fragment| {
                        query
                            .as_ref()
                            .is_none_or(|query| fragment.text.to_lowercase().contains(query))
                    })
                    .take(args.limit.min(200))
                    .map(|fragment| {
                        json!({
                            "fragmentId": fragment.id,
                            "tag": fragment.tag,
                            "text": fragment.text,
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(ToolOutput::text(serde_json::to_string_pretty(&json!({
                    "documentId": document.manifest.id,
                    "revision": document.manifest.revision,
                    "fragments": fragments,
                }))?))
            })
        }),
    );

    registry.register_structured_with_impact(
        "record_document_evidence",
        "原子替换当前 revision 的 Evidence Ledger。只记录实际保留在文档中的关键主张，区分来源、用户输入、计算、分析与假设；计算必须给出公式。",
        evidence_schema(),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: RecordEvidenceArgs = serde_json::from_value(value)
                    .context("invalid record_document_evidence arguments")?;
                let ledger = crate::runtime::documents::replace_evidence_ledger(
                    &crate::runtime::settings::local_config::data_dir(),
                    &args.document_id,
                    args.expected_revision,
                    args.entries,
                )?;
                Ok(ToolOutput::text(serde_json::to_string_pretty(&ledger)?))
            })
        }),
    );

    registry.register_structured_with_impact(
        "patch_document",
        "定点修改 HTML 文档。长文档优先先调用 inspect_document_fragments：用 path=/html 的 replaceFragment + fragmentId 修改语义块，value 只传新的内部 HTML/文字时会保留原元素标签与属性，只有显式传入同标签的完整元素时才替换外层；用 remove + fragmentId 删除整个语义块。零散短语可用 replaceText，matchText 应取能唯一匹配的最短片段，不要复制整段旧 HTML。禁止用整份覆盖替代局部修改。",
        json!({
            "type": "object",
            "properties": {
                "documentId": { "type": "string" },
                "expectedRevision": { "type": "integer", "minimum": 1 },
                "title": { "type": "string", "minLength": 1, "maxLength": 160 },
                "patches": {
                    "type": "array", "minItems": 1, "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": { "type": "string", "enum": ["add", "replace", "remove", "replaceText", "replaceFragment"] },
                            "path": { "type": "string", "pattern": "^/" },
                            "matchText": { "type": "string", "minLength": 1 },
                            "fragmentId": { "type": "string", "pattern": "^frag-[0-9a-f]{16}(?:-[0-9]+)?$" },
                            "value": {}
                        },
                        "required": ["op", "path"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["documentId", "expectedRevision", "patches"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: PatchArgs = serde_json::from_value(value)
                    .context("invalid patch_document arguments")?;
                let data_dir = crate::runtime::settings::local_config::data_dir();
                let current = crate::runtime::documents::read(&data_dir, &args.document_id)?;
                if current.manifest.revision != args.expected_revision {
                    bail!(
                        "document revision conflict: expected {}, current {}",
                        args.expected_revision,
                        current.manifest.revision
                    );
                }
                let mut content = current.content;
                apply_patches(&mut content, args.patches)?;
                let document = crate::runtime::documents::update(
                    &data_dir,
                    &args.document_id,
                    UpdateDocument {
                        expected_revision: args.expected_revision,
                        title: args.title,
                        content,
                        status: None,
                        qa_status: Some("pending".into()),
                    },
                )?;
                Ok(document_output("HTML 文档已更新", &document.manifest))
            })
        }),
    );

    registry.register_structured_with_impact(
        "attach_document_asset",
        "把 tools.fsBase 内的图片或当前会话已有的 AI 生图复制进 HTML 文档 assets/，返回可直接写入 img src 的相对路径。",
        json!({
            "type": "object",
            "properties": {
                "documentId": { "type": "string" },
                "sourcePath": { "type": "string", "description": "tools.fsBase 内的 PNG、JPEG 或 WebP 文件" },
                "imageUrl": { "type": "string", "description": "当前会话 AI 生图的 /api/image-artifacts/... URL" }
            },
            "required": ["documentId"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: AttachArgs = serde_json::from_value(value)
                    .context("invalid attach_document_asset arguments")?;
                let data_dir = crate::runtime::settings::local_config::data_dir();
                let source = match (args.source_path.as_deref(), args.image_url.as_deref()) {
                    (Some(path), None) => FsSandbox::from_config()?.resolve(path)?,
                    (None, Some(url)) => resolve_generated_image(&data_dir, url)?,
                    _ => bail!("provide exactly one of sourcePath or imageUrl"),
                };
                let bytes = tokio::fs::read(&source).await?;
                let filename = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("asset");
                let path =
                    crate::runtime::documents::store_asset(&data_dir, &args.document_id, filename, &bytes)?;
                Ok(ToolOutput::text(format!("assetPath={path}")))
            })
        }),
    );

    registry.register_structured_with_impact(
        "render_document",
        "检查并返回可编辑 HTML 文档卡片。存在 fatal 结构、资产或安全问题时拒绝渲染。",
        id_schema(),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: InspectArgs =
                    serde_json::from_value(value).context("invalid render_document arguments")?;
                let data_dir = crate::runtime::settings::local_config::data_dir();
                let document = crate::runtime::documents::read(&data_dir, &args.document_id)?;
                let issues = inspect_html(&data_dir, &document.manifest.id, &document.content);
                if has_fatal_issue(&issues) {
                    bail!(
                        "HTML document has fatal issues: {}",
                        serde_json::to_string(&issues)?
                    );
                }
                let mut output = document_output("HTML 文档预览已就绪", &document.manifest);
                output.terminate = true;
                Ok(output)
            })
        }),
    );

    registry.register_structured_with_impact(
        "inspect_document_layout",
        "检查 HTML 文档结构、安全性、资产完整性、语义层级与基础可读性。浏览器工作台继续负责真实视觉和打印预览。",
        id_schema(),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(|value| {
            let value = value.clone();
            Box::pin(async move {
                let args: InspectArgs = serde_json::from_value(value)
                    .context("invalid inspect_document_layout arguments")?;
                let data_dir = crate::runtime::settings::local_config::data_dir();
                let document = crate::runtime::documents::read(&data_dir, &args.document_id)?;
                let issues =
                    inspect_html(&data_dir, &document.manifest.id, &document.content);
                let has_fatal = has_fatal_issue(&issues);
                Ok(ToolOutput::text(serde_json::to_string_pretty(&json!({
                    "documentId": document.manifest.id,
                    "revision": document.manifest.revision,
                    "verdict": if has_fatal { "fail" } else if issues.is_empty() { "pass" } else { "warning" },
                    "issues": issues,
                }))?))
            })
        }),
    );
}

fn create_document_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "const": "html" },
            "title": { "type": "string", "minLength": 1, "maxLength": 160 },
            "content": {
                "type": "object",
                "properties": {
                    "html": {
                        "type": "string",
                        "minLength": 1,
                        "description": "完整、自包含样式的 HTML 文档。必须包含 html/body；图片引用 assets/<filename>；不得包含脚本、iframe 或远程依赖。"
                    }
                },
                "required": ["html"],
                "additionalProperties": false
            }
        },
        "required": ["kind", "title", "content"],
        "additionalProperties": false
    })
}

fn decode_content_argument(content: Option<Value>) -> Result<Option<Value>> {
    let Some(content) = content else {
        return Ok(None);
    };
    let decoded = match content {
        Value::String(raw) => {
            serde_json::from_str::<Value>(&raw).context("content JSON string is invalid")?
        }
        value => value,
    };
    if !decoded.is_object() {
        bail!("content must be a JSON object or a JSON string containing an object");
    }
    Ok(Some(decoded))
}

fn resolve_generated_image(data_dir: &Path, image_url: &str) -> Result<std::path::PathBuf> {
    let path = image_url
        .split_once('?')
        .map_or(image_url, |(path, _)| path)
        .trim_end_matches('/');
    let relative = path
        .strip_prefix("/api/image-artifacts/")
        .context("imageUrl must reference a local AI image artifact")?;
    let mut segments = relative.split('/');
    let session_id = segments.next().context("missing image session id")?;
    let artifact_id = segments.next().context("missing image artifact id")?;
    if segments.next().is_some() {
        bail!("invalid image artifact URL");
    }
    crate::runtime::image_generation::artifact_path(data_dir, session_id, artifact_id)
}

fn id_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "documentId": { "type": "string" } },
        "required": ["documentId"],
        "additionalProperties": false
    })
}

fn evidence_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "documentId": { "type": "string" },
            "expectedRevision": { "type": "integer", "minimum": 1 },
            "entries": {
                "type": "array",
                "minItems": 1,
                "maxItems": 100,
                "items": {
                    "type": "object",
                    "properties": {
                        "claim": { "type": "string", "minLength": 1, "maxLength": 1000 },
                        "basis": {
                            "type": "string",
                            "enum": ["source", "user_input", "calculation", "analysis", "assumption"]
                        },
                        "source": { "type": "string", "minLength": 1, "maxLength": 1000 },
                        "locator": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "excerpt": { "type": "string", "minLength": 1, "maxLength": 2000 },
                        "formula": { "type": "string", "minLength": 1, "maxLength": 1000 },
                        "limitations": { "type": "string", "minLength": 1, "maxLength": 1000 }
                    },
                    "required": ["claim", "basis", "source"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["documentId", "expectedRevision", "entries"],
        "additionalProperties": false
    })
}

fn document_output(
    message: &str,
    manifest: &crate::runtime::documents::DocumentManifest,
) -> ToolOutput {
    ToolOutput::with_details(
        format!(
            "{message}：{}（document_id={}，revision={}）",
            manifest.title, manifest.id, manifest.revision
        ),
        json!({
            "documentRef": {
                "id": manifest.id,
                "kind": manifest.kind,
                "title": manifest.title,
                "revision": manifest.revision,
                "status": manifest.status,
                "qaStatus": manifest.qa_status,
                "runtime": manifest.runtime,
                "origin": manifest.origin,
            }
        }),
    )
}

fn apply_patch(root: &mut Value, patch: PatchOperation) -> Result<()> {
    if matches!(&patch.op, PatchKind::ReplaceFragment)
        || matches!(&patch.op, PatchKind::Remove) && patch.fragment_id.is_some()
    {
        if patch.path != "/html" {
            bail!("replaceFragment only supports path=/html");
        }
        let target = root
            .pointer_mut(&patch.path)
            .context("replaceFragment target not found: /html")?;
        let html = target
            .as_str()
            .context("replaceFragment target must be a string")?;
        let fragment_id = patch
            .fragment_id
            .as_deref()
            .context("replaceFragment requires fragmentId")?;
        let fragment = editable_html_fragments(html)
            .into_iter()
            .find(|fragment| fragment.id == fragment_id)
            .with_context(|| {
                format!(
                    "replaceFragment fragmentId not found in current revision: {fragment_id}; call inspect_document_fragments again"
                )
            })?;
        let replacement = match patch.op {
            PatchKind::ReplaceFragment => fragment_replacement(
                &fragment,
                patch
                    .value
                    .as_ref()
                    .and_then(Value::as_str)
                    .context("replaceFragment requires a string value")?,
            ),
            PatchKind::Remove => String::new(),
            _ => unreachable!("fragment operation checked above"),
        };
        let mut updated =
            String::with_capacity(html.len() - fragment.outer_html.len() + replacement.len());
        updated.push_str(&html[..fragment.start]);
        updated.push_str(&replacement);
        updated.push_str(&html[fragment.end..]);
        *target = Value::String(updated);
        return Ok(());
    }
    if matches!(&patch.op, PatchKind::ReplaceText) {
        let target = root
            .pointer_mut(&patch.path)
            .with_context(|| format!("replaceText target not found: {}", patch.path))?;
        let text = target
            .as_str()
            .context("replaceText target must be a string")?;
        let needle = patch
            .match_text
            .as_deref()
            .context("replaceText requires matchText")?;
        let replacement = patch
            .value
            .as_ref()
            .and_then(Value::as_str)
            .context("replaceText requires a string value")?;
        let occurrences = text.match_indices(needle).count();
        if occurrences != 1 {
            let hint = closest_exact_source_line(text, needle)
                .map(|line| format!("; possible exact source line: {line:?}"))
                .unwrap_or_default();
            bail!("replaceText matchText must occur exactly once, found {occurrences}{hint}");
        }
        *target = Value::String(text.replacen(needle, replacement, 1));
        return Ok(());
    }
    let tokens = patch
        .path
        .split('/')
        .skip(1)
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect::<Vec<_>>();
    if tokens.is_empty() || tokens.iter().any(|token| token.is_empty()) {
        bail!("patch path must target a document field");
    }
    let (last, parents) = tokens.split_last().context("invalid patch path")?;
    let mut parent = root;
    for token in parents {
        parent = match parent {
            Value::Object(map) => map
                .get_mut(token)
                .with_context(|| format!("patch parent not found: {token}"))?,
            Value::Array(items) => {
                let index = parse_index(token, items.len(), false)?;
                items
                    .get_mut(index)
                    .context("patch array parent not found")?
            }
            _ => bail!("patch parent is not a container"),
        };
    }
    match parent {
        Value::Object(map) => match patch.op {
            PatchKind::Add => {
                map.insert(last.clone(), patch.value.context("add requires value")?);
            }
            PatchKind::Replace => {
                *map.get_mut(last).context("replace target not found")? =
                    patch.value.context("replace requires value")?;
            }
            PatchKind::Remove => {
                map.remove(last).context("remove target not found")?;
            }
            PatchKind::ReplaceText => unreachable!("handled before pointer traversal"),
            PatchKind::ReplaceFragment => unreachable!("handled before pointer traversal"),
        },
        Value::Array(items) => match patch.op {
            PatchKind::Add => {
                let value = patch.value.context("add requires value")?;
                if last == "-" {
                    items.push(value);
                } else {
                    let index = parse_index(last, items.len(), true)?;
                    items.insert(index, value);
                }
            }
            PatchKind::Replace => {
                let index = parse_index(last, items.len(), false)?;
                items[index] = patch.value.context("replace requires value")?;
            }
            PatchKind::Remove => {
                let index = parse_index(last, items.len(), false)?;
                items.remove(index);
            }
            PatchKind::ReplaceText => unreachable!("handled before pointer traversal"),
            PatchKind::ReplaceFragment => unreachable!("handled before pointer traversal"),
        },
        _ => bail!("patch target parent is not a container"),
    }
    Ok(())
}

fn apply_patches(root: &mut Value, patches: Vec<PatchOperation>) -> Result<()> {
    let mut fragment_patches = Vec::new();
    let mut other_patches = Vec::new();
    for (index, patch) in patches.into_iter().enumerate() {
        if matches!(patch.op, PatchKind::ReplaceFragment)
            || matches!(patch.op, PatchKind::Remove) && patch.fragment_id.is_some()
        {
            fragment_patches.push((index, patch));
        } else {
            other_patches.push((index, patch));
        }
    }
    if !fragment_patches.is_empty() {
        apply_fragment_patches(root, fragment_patches)?;
    }
    for (index, patch) in other_patches {
        if let Err(error) = apply_patch(root, patch) {
            bail!("patches/{index} failed: {error:#}");
        }
    }
    Ok(())
}

fn apply_fragment_patches(root: &mut Value, patches: Vec<(usize, PatchOperation)>) -> Result<()> {
    let target = root
        .pointer_mut("/html")
        .context("replaceFragment target not found: /html")?;
    let original = target
        .as_str()
        .context("replaceFragment target must be a string")?
        .to_string();
    let fragments = editable_html_fragments(&original);
    let mut replacements = Vec::with_capacity(patches.len());
    for (index, patch) in patches {
        if patch.path != "/html" {
            bail!("patches/{index} failed: replaceFragment only supports path=/html");
        }
        let fragment_id = patch.fragment_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!("patches/{index} failed: replaceFragment requires fragmentId")
        })?;
        let fragment = fragments
            .iter()
            .find(|fragment| fragment.id == fragment_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "patches/{index} failed: replaceFragment fragmentId not found in current revision: {fragment_id}; call inspect_document_fragments again"
                )
            })?;
        let replacement = match patch.op {
            PatchKind::ReplaceFragment => fragment_replacement(
                fragment,
                patch
                    .value
                    .as_ref()
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "patches/{index} failed: replaceFragment requires a string value"
                        )
                    })?,
            ),
            PatchKind::Remove => String::new(),
            _ => unreachable!("fragment operation checked above"),
        };
        replacements.push((index, fragment.start, fragment.end, replacement));
    }
    replacements.sort_by_key(|(_, start, _, _)| std::cmp::Reverse(*start));
    for pair in replacements.windows(2) {
        let (_, later_start, _, _) = &pair[0];
        let (_, _, earlier_end, _) = &pair[1];
        if earlier_end > later_start {
            bail!(
                "patches/{} failed: replaceFragment targets overlap another patch",
                pair[0].0
            );
        }
    }
    let mut updated = original;
    for (_, start, end, replacement) in replacements {
        updated.replace_range(start..end, &replacement);
    }
    *target = Value::String(updated);
    Ok(())
}

#[derive(Debug)]
struct HtmlFragment<'a> {
    id: String,
    tag: String,
    text: String,
    outer_html: &'a str,
    start: usize,
    end: usize,
}

fn fragment_replacement(fragment: &HtmlFragment<'_>, replacement: &str) -> String {
    let trimmed = replacement.trim();
    let lower = trimmed.to_ascii_lowercase();
    let opening = format!("<{}", fragment.tag);
    let closing = format!("</{}>", fragment.tag);
    if lower.starts_with(&opening) && lower.ends_with(&closing) {
        return replacement.to_string();
    }

    let opening_end = fragment
        .outer_html
        .find('>')
        .expect("editable fragment has an opening tag")
        + 1;
    let closing_start = fragment
        .outer_html
        .to_ascii_lowercase()
        .rfind(&closing)
        .expect("editable fragment has a closing tag");
    format!(
        "{}{}{}",
        &fragment.outer_html[..opening_end],
        replacement,
        &fragment.outer_html[closing_start..]
    )
}

fn default_fragment_limit() -> usize {
    32
}

const FRAGMENT_TEXT_LIMIT: usize = 320;

fn editable_html_fragments(html: &str) -> Vec<HtmlFragment<'_>> {
    static OPEN_TAG: OnceLock<Regex> = OnceLock::new();
    let regex = OPEN_TAG.get_or_init(|| {
        Regex::new(r"(?is)<(h[1-6]|p|li|blockquote|figcaption|td|th)(?:\s[^>]*)?>")
            .expect("editable fragment regex")
    });
    let lowercase = html.to_lowercase();
    let mut duplicate_counts = std::collections::HashMap::<String, usize>::new();
    let mut fragments = Vec::new();
    let mut cursor = 0;
    while let Some(opening) = regex.captures(&html[cursor..]) {
        let whole = opening.get(0).expect("whole opening tag");
        let tag = opening
            .get(1)
            .expect("fragment tag")
            .as_str()
            .to_lowercase();
        let start = cursor + whole.start();
        let content_start = cursor + whole.end();
        let closing = format!("</{tag}>");
        let Some(relative_end) = lowercase[content_start..].find(&closing) else {
            cursor = content_start;
            continue;
        };
        let end = content_start + relative_end + closing.len();
        let outer_html = &html[start..end];
        let digest = Sha256::digest(outer_html.as_bytes());
        let base_id = format!("frag-{}", hex::encode(&digest[..8]));
        let occurrence = duplicate_counts.entry(base_id.clone()).or_default();
        let id = if *occurrence == 0 {
            base_id
        } else {
            format!("{base_id}-{occurrence}")
        };
        *occurrence += 1;
        fragments.push(HtmlFragment {
            id,
            tag,
            text: truncate_fragment_text(&visible_fragment_text(outer_html)),
            outer_html,
            start,
            end,
        });
        cursor = end;
    }
    fragments
}

fn truncate_fragment_text(text: &str) -> String {
    if text.chars().count() <= FRAGMENT_TEXT_LIMIT {
        return text.to_string();
    }
    let mut truncated = text.chars().take(FRAGMENT_TEXT_LIMIT).collect::<String>();
    truncated.push('…');
    truncated
}

fn visible_fragment_text(fragment: &str) -> String {
    static TAGS: OnceLock<Regex> = OnceLock::new();
    static WHITESPACE: OnceLock<Regex> = OnceLock::new();
    let without_tags = TAGS
        .get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("HTML tag regex"))
        .replace_all(fragment, " ");
    WHITESPACE
        .get_or_init(|| Regex::new(r"\s+").expect("whitespace regex"))
        .replace_all(without_tags.trim(), " ")
        .into_owned()
}

fn parse_index(token: &str, len: usize, allow_end: bool) -> Result<usize> {
    let index = token.parse::<usize>().context("invalid array index")?;
    if index > len || (!allow_end && index == len) {
        bail!("array index out of bounds");
    }
    Ok(index)
}

fn inspect_html(data_dir: &Path, id: &str, content: &Value) -> Vec<Value> {
    let html = content
        .get("html")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lower = html.to_ascii_lowercase();
    let mut issues = Vec::new();
    if html.trim().is_empty() {
        issues.push(json!({
            "category": "empty-document",
            "severity": "fatal",
            "message": "HTML 文档为空"
        }));
        return issues;
    }
    if html.contains('\u{fffd}') {
        issues.push(json!({
            "category": "invalid-text-encoding",
            "severity": "fatal",
            "message": "HTML 含 Unicode 替换字符（�），必须从回读原文定点修复",
            "textExcerpt": replacement_character_excerpt(html),
        }));
    }
    for tag in ["<script", "<iframe", "<object", "<embed"] {
        if lower.contains(tag) {
            issues.push(json!({
                "category": "unsafe-html",
                "severity": "fatal",
                "message": format!("HTML 不允许包含 {tag}> 元素")
            }));
        }
    }
    if unsafe_attribute_regex().is_match(&lower)
        || lower.contains("javascript:")
        || lower.contains("data:text/html")
    {
        issues.push(json!({
            "category": "unsafe-html",
            "severity": "fatal",
            "message": "HTML 含事件属性或危险 URL"
        }));
    }
    for capture in asset_regex().captures_iter(html) {
        let Some(asset) = capture.get(1).map(|value| value.as_str()) else {
            continue;
        };
        if !asset_exists(data_dir, id, asset) {
            issues.push(json!({
                "category": "missing-asset",
                "severity": "fatal",
                "message": format!("图片或本地资源不存在：assets/{asset}")
            }));
        }
    }
    if remote_asset_regex().is_match(html) {
        issues.push(json!({
            "category": "remote-asset",
            "severity": "fatal",
            "message": "HTML 仍引用远程图片或媒体；请先使用 attach_document_asset 本地化"
        }));
    }
    if lower.contains("<img") && !alt_attribute_regex().is_match(html) {
        issues.push(json!({
            "category": "image-alt",
            "severity": "warning",
            "message": "至少一张图片可能缺少 alt 文本"
        }));
    }
    if !lower.contains("<h1") {
        issues.push(json!({
            "category": "missing-primary-heading",
            "severity": "warning",
            "message": "文档缺少明确的一级标题"
        }));
    }
    if strip_markup(html)
        .chars()
        .filter(|character| !character.is_whitespace())
        .count()
        < 80
    {
        issues.push(json!({
            "category": "thin-content",
            "severity": "warning",
            "message": "可见文本很少，请确认这不是空壳版式"
        }));
    }
    issues
}

fn unsafe_attribute_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"\son[a-z]+\s*="#).expect("valid unsafe attribute regex"))
}

fn asset_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| Regex::new(r#"assets/([A-Za-z0-9._-]+)"#).expect("valid local asset regex"))
}

fn remote_asset_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?i)(?:src|poster)\s*=\s*["']\s*https?://"#)
            .expect("valid remote asset regex")
    })
}

fn alt_attribute_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"(?i)<img\b[^>]*\balt\s*="#).expect("valid alt regex"))
}

fn strip_markup(html: &str) -> String {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| Regex::new(r#"(?s)<[^>]+>"#).expect("valid markup regex"))
        .replace_all(html, " ")
        .into_owned()
}

fn has_fatal_issue(issues: &[Value]) -> bool {
    issues
        .iter()
        .any(|issue| issue.get("severity").and_then(Value::as_str) == Some("fatal"))
}

fn replacement_character_excerpt(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    let Some(index) = chars.iter().position(|character| *character == '\u{fffd}') else {
        return String::new();
    };
    let start = index.saturating_sub(40);
    let end = (index + 41).min(chars.len());
    chars[start..end].iter().collect()
}

fn closest_exact_source_line<'a>(text: &'a str, needle: &str) -> Option<&'a str> {
    let normalized_needle = normalize_match_hint(needle);
    let prefix = normalized_needle.chars().take(18).collect::<String>();
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .find(|line| {
            let normalized_line = normalize_match_hint(line);
            normalized_line.contains(&normalized_needle)
                || (!prefix.is_empty() && normalized_line.contains(&prefix))
        })
        .map(str::trim)
}

fn normalize_match_hint(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '“' | '”' => '"',
            '‘' | '’' => '\'',
            other => other,
        })
        .collect()
}

fn asset_exists(data_dir: &Path, id: &str, asset: &str) -> bool {
    crate::runtime::documents::resolve_asset(data_dir, id, asset).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(body: &str) -> Value {
        json!({ "html": format!("<!doctype html><html><body>{body}</body></html>") })
    }

    #[test]
    fn create_schema_only_accepts_html() {
        let schema = create_document_schema();
        assert_eq!(schema["properties"]["kind"]["const"], "html");
        assert!(schema.to_string().contains("\"html\""));
        assert!(!schema.to_string().contains("\"slide\""));
        assert!(!schema.to_string().contains("\"report\""));
    }

    #[test]
    fn patch_replace_text_updates_exact_html_fragment() {
        let mut content = html("<h1>Old</h1><p>Body</p>");
        apply_patch(
            &mut content,
            PatchOperation {
                op: PatchKind::ReplaceText,
                path: "/html".into(),
                match_text: Some("<h1>Old</h1>".into()),
                fragment_id: None,
                value: Some(json!("<h1>New</h1>")),
            },
        )
        .unwrap();
        assert!(content["html"].as_str().unwrap().contains("<h1>New</h1>"));
    }

    #[test]
    fn fragment_patch_replaces_semantic_block_without_copying_old_html() {
        let mut content = html(
            "<h1>Title</h1><p class=\"lead\">Unsupported <strong>42%</strong> claim.</p><p>Keep me.</p>",
        );
        let source = content["html"].as_str().unwrap();
        let fragment = editable_html_fragments(source)
            .into_iter()
            .find(|fragment| fragment.text.contains("42%"))
            .unwrap();
        let fragment_id = fragment.id;

        apply_patch(
            &mut content,
            PatchOperation {
                op: PatchKind::ReplaceFragment,
                path: "/html".into(),
                match_text: None,
                fragment_id: Some(fragment_id),
                value: Some(json!(
                    "<p class=\"lead\">The available source does not quantify this claim.</p>"
                )),
            },
        )
        .unwrap();

        let updated = content["html"].as_str().unwrap();
        assert!(!updated.contains("42%"));
        assert!(updated.contains("does not quantify"));
        assert!(updated.contains("<p>Keep me.</p>"));
    }

    #[test]
    fn fragment_patch_preserves_wrapper_for_text_or_inline_html() {
        let mut content = html("<p class=\"lead\" data-role=\"summary\">Old</p><li>First</li>");
        let fragments = editable_html_fragments(content["html"].as_str().unwrap());
        let paragraph_id = fragments[0].id.clone();
        let list_id = fragments[1].id.clone();

        apply_patches(
            &mut content,
            vec![
                PatchOperation {
                    op: PatchKind::ReplaceFragment,
                    path: "/html".into(),
                    match_text: None,
                    fragment_id: Some(paragraph_id),
                    value: Some(json!("New <strong>verified</strong> summary.")),
                },
                PatchOperation {
                    op: PatchKind::ReplaceFragment,
                    path: "/html".into(),
                    match_text: None,
                    fragment_id: Some(list_id),
                    value: Some(json!("Second")),
                },
            ],
        )
        .unwrap();

        let updated = content["html"].as_str().unwrap();
        assert!(updated.contains(
            "<p class=\"lead\" data-role=\"summary\">New <strong>verified</strong> summary.</p>"
        ));
        assert!(updated.contains("<li>Second</li>"));
    }

    #[test]
    fn fragment_remove_deletes_only_the_selected_semantic_block() {
        let mut content = html("<ul><li>Remove me</li><li>Keep me</li></ul>");
        let fragment_id = editable_html_fragments(content["html"].as_str().unwrap())[0]
            .id
            .clone();

        apply_patches(
            &mut content,
            vec![PatchOperation {
                op: PatchKind::Remove,
                path: "/html".into(),
                match_text: None,
                fragment_id: Some(fragment_id),
                value: None,
            }],
        )
        .unwrap();

        let updated = content["html"].as_str().unwrap();
        assert!(!updated.contains("Remove me"));
        assert!(updated.contains("<li>Keep me</li>"));
        assert!(updated.contains("<ul>"));
    }

    #[test]
    fn fragment_ids_are_stable_and_query_text_is_visible() {
        let source = "<h2>Section</h2><p>One <strong>important</strong> fact.</p>";
        let first = editable_html_fragments(source);
        let second = editable_html_fragments(source);
        assert_eq!(first[1].id, second[1].id);
        assert_eq!(first[1].text, "One important fact.");
    }

    #[test]
    fn fragment_batch_replaces_duplicate_cells_against_one_revision() {
        let mut content = html(
            "<table><tr><td>positive</td></tr><tr><td>positive</td></tr><tr><td>positive</td></tr><tr><td>positive</td></tr></table>",
        );
        let ids = editable_html_fragments(content["html"].as_str().unwrap())
            .into_iter()
            .map(|fragment| fragment.id)
            .collect::<Vec<_>>();
        let patches = ids
            .into_iter()
            .zip(["one", "two", "three", "four"])
            .map(|(fragment_id, value)| PatchOperation {
                op: PatchKind::ReplaceFragment,
                path: "/html".into(),
                match_text: None,
                fragment_id: Some(fragment_id),
                value: Some(json!(format!("<td>{value}</td>"))),
            })
            .collect();

        apply_patches(&mut content, patches).unwrap();

        let updated = content["html"].as_str().unwrap();
        assert!(updated.contains("<td>one</td>"));
        assert!(updated.contains("<td>two</td>"));
        assert!(updated.contains("<td>three</td>"));
        assert!(updated.contains("<td>four</td>"));
        assert!(!updated.contains("<td>positive</td>"));
    }

    #[test]
    fn patch_batch_error_includes_the_underlying_reason() {
        let mut content = html("<p>Body</p>");
        let error = apply_patches(
            &mut content,
            vec![PatchOperation {
                op: PatchKind::ReplaceFragment,
                path: "/html".into(),
                match_text: None,
                fragment_id: Some("frag-0000000000000000".into()),
                value: Some(json!("<p>Replacement</p>")),
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("patches/0 failed"));
        assert!(error.contains("fragmentId not found"));
    }

    #[test]
    fn fragment_preview_is_bounded_without_changing_stable_id() {
        let long_text = "证".repeat(FRAGMENT_TEXT_LIMIT + 50);
        let source = format!("<p>{long_text}</p>");
        let fragment = editable_html_fragments(&source).remove(0);
        assert_eq!(fragment.text.chars().count(), FRAGMENT_TEXT_LIMIT + 1);
        assert!(fragment.text.ends_with('…'));
        assert!(fragment.id.starts_with("frag-"));
    }

    #[test]
    fn html_inspection_blocks_unsafe_and_remote_content() {
        let data = tempfile::tempdir().unwrap();
        let issues = inspect_html(
            data.path(),
            "safe-id",
            &html("<h1>Title</h1><script>alert(1)</script><img src=\"https://example.com/a.png\">"),
        );
        assert!(issues
            .iter()
            .any(|issue| { issue["category"] == "unsafe-html" && issue["severity"] == "fatal" }));
        assert!(issues
            .iter()
            .any(|issue| { issue["category"] == "remote-asset" && issue["severity"] == "fatal" }));
    }

    #[test]
    fn replacement_character_is_fatal() {
        let data = tempfile::tempdir().unwrap();
        let issues = inspect_html(
            data.path(),
            "safe-id",
            &html("<h1>Title</h1><p>broken � text with enough visible content for QA</p>"),
        );
        assert!(issues.iter().any(|issue| {
            issue["category"] == "invalid-text-encoding" && issue["severity"] == "fatal"
        }));
    }
}
