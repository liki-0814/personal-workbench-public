use std::sync::Arc;

use anyhow::Context;
use serde_json::{json, Value};

use crate::runtime::backend::BackendClient;
use crate::runtime::tools::progress;
use crate::runtime::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};

/// Register all built-in tools into the given [`ToolRegistry`].
///
/// This includes data CRUD tools, file system tools (read, write, list, search),
/// shell execution and dynamically loaded skills from `~/.agents/skills/`.
/// Persistent CLI delegation is registered later with the RuntimeTask broker.
pub fn register_all_tools(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    crate::runtime::tools::data_crud::register(registry, Arc::clone(&backend));
    crate::runtime::tools::documents::register(registry);
    crate::runtime::tools::archify::register(registry);
    crate::runtime::tools::illustrate::register(registry);
    crate::runtime::tools::jobs::register(registry, Arc::clone(&backend));
    register_fs_tools(registry, Arc::clone(&backend));
    crate::runtime::tools::fs_search::register(registry);
    crate::runtime::tools::text_search::register(registry);
    register_artifact_tools(registry);
    crate::runtime::tools::edit_file::register(registry);
    register_generate_image_tool(registry);
    register_shell_tools(registry);
    register_decision_control_tool(registry);
    // Research and analysis use the ordinary Agent loop plus atomic
    // web/filesystem/shell tools; no monolithic workflow pipeline is exposed.

    // 注册 SSH 远程操作工具
    crate::runtime::tools::ssh::register(registry, Arc::clone(&backend));
    tracing::debug!("SSH tools registered");

    // 加载 skill 并注册为工具
    let skills = crate::runtime::skills::load_skills();
    if !skills.is_empty() {
        tracing::debug!(count = skills.len(), "skill tools registered");
        crate::runtime::skills::register_skill_tools(registry, skills);
    }

    // AnySearch 提供公开网页搜索与正文抽取。
    crate::runtime::tools::web::register(registry);

    // 注册 read_pdf 工具（三级 fallback：arxiv-html → MinerU → PyMuPDF）
    crate::runtime::tools::pdf::register_read_pdf(registry);
    // 启动时懒清理一次 24h 过期 PDF 临时目录
    if crate::runtime::tools::fs_local::worker_execution_policy().is_none() {
        tokio::spawn(async {
            crate::runtime::tools::pdf::cleanup_on_startup().await;
            if let Err(error) = crate::runtime::tools::artifacts::cleanup_expired_tool_outputs() {
                tracing::warn!(%error, "failed to clean expired tool output artifacts");
            }
            let indexed = crate::runtime::tools::pdf::backfill_pdf_artifact_memories();
            if indexed > 0 {
                tracing::info!(indexed, "backfilled PDF artifact memory indexes");
            }
        });
    } else {
        let allow_mutation = crate::runtime::tools::fs_local::worker_execution_policy()
            .is_some_and(|policy| policy.allow_mutation());
        restrict_to_internal_worker_tools(registry, allow_mutation);
    }
    tracing::debug!("read_pdf tool registered");
}

fn restrict_to_internal_worker_tools(registry: &ToolRegistry, allow_mutation: bool) {
    let mut allowed = vec![
        "proceed_without_clarification",
        "request_decision_review",
        "request_user_choice",
        "ls",
        "read",
        "find",
        "grep",
        "search_file_content",
        "bash",
        "read_artifact",
        "web_read",
        "web_search_domains",
        "web_query",
        "web_fetch_batch",
    ];
    if allow_mutation {
        allowed.extend(["write", "remove_file", "edit"]);
    }
    registry.retain_only(&allowed);
}

fn register_artifact_tools(registry: &mut ToolRegistry) {
    registry.register(
        "read_artifact",
        "按字符偏移分块读取 pwcli 管理的大型工具输出或 RuntimeTask continuation。仅接受系统返回的 artifact_id，不读取任意文件路径。",
        serde_json::json!({
            "type": "object",
            "properties": {
                "artifact_id": { "type": "string", "description": "工具输出或 RuntimeTask continuation 返回的 artifact_id" },
                "offset": { "type": "integer", "minimum": 0, "description": "起始字符偏移，默认 0" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 10000, "description": "最多读取字符数，默认 8000" }
            },
            "required": ["artifact_id"],
            "additionalProperties": false
        }),
        Box::new(|args: &Value| {
            let artifact_id = args["artifact_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("artifact_id is required"))
                .map(str::to_string);
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let limit = args["limit"].as_u64().unwrap_or(8000) as usize;
            Box::pin(async move {
                let chunk = crate::runtime::tools::artifacts::read_tool_output(&artifact_id?, offset, limit)?;
                Ok(format!(
                    "artifact_id={} offset={} next_offset={} total_chars={} eof={}\n\n{}",
                    chunk.artifact_id,
                    chunk.offset,
                    chunk.next_offset,
                    chunk.total_chars,
                    chunk.eof,
                    chunk.content
                ))
            })
        }),
    );
}

fn register_decision_control_tool(registry: &mut ToolRegistry) {
    registry.register_structured_with_impact(
        "proceed_without_clarification",
        "Resolve the mandatory first-tool clarification gate only when the user's request already provides enough context to act safely. Call this alone before any memory, file, web, command, analysis, code, or document tool. Never use it when a missing choice would materially change the source, population, method, safety boundary, or deliverable. Group comparisons, trend claims, or statistical differences require request_user_choice when the population, geography or school stage, source boundary, or decision use is not established; an available memory or cached source does not establish user intent.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "rationale": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 300,
                    "description": "One concise reason the request can be executed safely without asking the user."
                }
            },
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::Control,
        Box::new(|_| {
            Box::pin(async {
                Ok(crate::runtime::tools::registry::ToolOutput {
                    content: "Clarification gate resolved. Continue with the requested work.".into(),
                    terminate: false,
                    details: None,
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
    registry.register_with_mode_and_impact(
        "request_decision_review",
        "You may request an independent Harness MoA second opinion when your own judgment says it would materially improve the result: for example genuine ambiguity, conflicting evidence, consequential tradeoffs, or materially different approaches. Do not use it merely because a task has many steps, tools, shell commands, failures, or a long answer; routine work should proceed without advisors.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The precise disputed decision." },
                "proposal": { "type": "string", "description": "The acting agent's current recommendation." },
                "evidence": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Short evidence items relevant to the decision."
                }
            },
            "required": ["question", "proposal"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::Control,
        Box::new(|_| Box::pin(async { Ok("Harness decision review requested".to_string()) })),
    );
    registry.register_structured_with_impact(
        "request_user_choice",
        "Pause this turn and ask one concise multiple-choice question in the Chat composer. Use only when the answer materially changes the result. For complex reports or multi-page Slides, use this before generation to confirm outline, audience, length, or visual direction. Put the recommended option first and explain each tradeoff. Ask one question per call.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "minLength": 1, "maxLength": 160 },
                "rationale": { "type": "string", "maxLength": 500 },
                "options": {
                    "type": "array", "minItems": 2, "maxItems": 3,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "label": { "type": "string", "minLength": 1, "maxLength": 80 },
                            "description": { "type": "string", "maxLength": 240 }
                        },
                        "required": ["id", "label", "description"],
                        "additionalProperties": false
                    }
                },
                "step": { "type": "integer", "minimum": 1 },
                "total": { "type": "integer", "minimum": 1 },
                "allowCustom": { "type": "boolean" },
                "allowSkip": { "type": "boolean" }
            },
            "required": ["title", "options"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::Control,
        Box::new(|args| {
            let details = serde_json::json!({
                "decisionPrompt": {
                    "id": uuid::Uuid::new_v4().simple().to_string(),
                    "title": args.get("title").cloned().unwrap_or_default(),
                    "rationale": args.get("rationale").cloned().unwrap_or_default(),
                    "options": args.get("options").cloned().unwrap_or_else(|| serde_json::json!([])),
                    "step": args.get("step").cloned().unwrap_or_else(|| serde_json::json!(1)),
                    "total": args.get("total").cloned().unwrap_or_else(|| serde_json::json!(1)),
                    "allowCustom": args.get("allowCustom").cloned().unwrap_or(serde_json::Value::Bool(true)),
                    "allowSkip": args.get("allowSkip").cloned().unwrap_or(serde_json::Value::Bool(true))
                }
            });
            Box::pin(async move {
                Ok(crate::runtime::tools::registry::ToolOutput {
                    content: "Waiting for the user's structured choice.".into(),
                    terminate: true,
                    details: Some(details),
                    added_tool_names: Vec::new(),
                })
            })
        }),
    );
}

// 单独入口：调用方决定 user_slug，内部从 RuntimeConfig 解析；不与 register_all_tools 合并防止破坏现有 server 接线签名
pub fn register_memory_tools(registry: &mut ToolRegistry, user_slug: String) {
    if crate::runtime::tools::fs_local::worker_execution_policy().is_some() {
        return;
    }
    crate::runtime::tools::memory::register(registry, user_slug);
}

pub async fn register_runtime_tools(
    registry: Arc<ToolRegistry>,
    _llm: Arc<crate::ai::llm::LlmClient>,
) {
    // Durable child work is registered by `dispatch_tasks` after the daemon's
    // RuntimeTask broker exists. The retired in-process `task` executor must not
    // be reintroduced here because it retained parent Futures and in-memory state.
    if crate::runtime::tools::fs_local::worker_execution_policy().is_some() {
        return;
    }
    match crate::runtime::tools::extensions::register_trusted(Arc::clone(&registry)).await {
        Ok(count) if count > 0 => tracing::debug!(count, "trusted extension tools registered"),
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "trusted extension loading failed"),
    }
}

fn register_fs_tools(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    let b = Arc::clone(&backend);
    registry.register_contextual_structured_with_impact(
        "ls",
        "列出目录内容（默认最多 500 条，目录在前）",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "目录路径（可选，默认当前沙箱根目录）" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "最多列出的条目数，默认 500" }
            }
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(move |context, args: &Value| {
            let b = Arc::clone(&b);
            let path = contextual_tool_path(context, args["path"].as_str().unwrap_or("."));
            let limit = args["limit"].as_u64().unwrap_or(500).clamp(1, 1000) as usize;
            Box::pin(async move {
                let entries = match crate::runtime::tools::fs_local::FsSandbox::from_config()
                    .and_then(|sandbox| sandbox.resolve(&path))
                {
                    Ok(local_path) => {
                        match crate::runtime::tools::fs_local::list_directory(&local_path).await {
                            Ok(entries) => entries
                                .into_iter()
                                .map(|entry| crate::runtime::backend::DirEntry {
                                    name: entry.name,
                                    kind: if entry.is_dir {
                                        "directory".into()
                                    } else {
                                        "file".into()
                                    },
                                    size: entry.size,
                                })
                                .collect(),
                            Err(error) => {
                                if crate::runtime::tools::fs_local::worker_execution_policy()
                                    .is_some()
                                {
                                    return Err(error);
                                }
                                b.list_dir(&path).await?
                            }
                        }
                    }
                    Err(error) => return Err(error),
                };
                if entries.is_empty() {
                    return Ok(ToolOutput::text("（空目录）"));
                }
                let total = entries.len();
                let truncated = total > limit;
                let lines: Vec<String> = entries
                    .iter()
                    .take(limit)
                    .map(|e| {
                        let icon = if e.kind == "directory" {
                            "📁"
                        } else {
                            "📄"
                        };
                        let size_str = e
                            .size
                            .map(|s| format!(" ({} bytes)", s))
                            .unwrap_or_default();
                        format!("{} {}{}", icon, e.name, size_str)
                    })
                    .collect();
                let mut output = lines.join("\n");
                if truncated {
                    output.push_str(&format!(
                        "\n[已显示前 {limit} 条，共 {total} 条；可用 find/grep 精确定位]"
                    ));
                }
                Ok(ToolOutput::text(output))
            })
        }),
    );

    let b = Arc::clone(&backend);
    registry.register_contextual_structured_with_impact(
        "read",
        "读取文件内容。文本文件返回原文，可用 offset（1-based 起始行号）和 limit（行数）读取片段；大文件先读头部片段再按需续读。图片（png/jpg/jpeg/gif/webp，≤5MB）在当前模型支持视觉时以多模态形式注入对话，否则拒绝并提示切换视觉模型。",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" },
                "offset": { "type": "integer", "minimum": 1, "description": "起始行号（1-based），默认 1；仅对文本文件生效" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 10000, "description": "最多读取的行数，默认整文件；仅对文本文件生效" }
            },
            "required": ["path"]
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(move |context, args: &Value| {
            let b = Arc::clone(&b);
            let active_model = context.active_model.clone();
            let path = args["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("path is required"))
                .map(|s| contextual_tool_path(context, s));
            let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
            let limit = args["limit"].as_u64().map(|value| value.max(1).min(10000) as usize);
            Box::pin(async move {
                let result: anyhow::Result<String> = async move {
                let path = path?;
                let sandbox = crate::runtime::tools::fs_local::FsSandbox::from_config()?;
                let local_path = sandbox.resolve(&path)?;
                // ── 图片分支：本地直读 + 视觉门控 ──
                if crate::runtime::tools::read_file_image::detect_image_ext(&path).is_some() {
                    if !crate::runtime::tools::read_file_image::model_supports_vision(active_model.as_ref()) {
                        let model = crate::runtime::tools::read_file_image::model_label(active_model.as_ref());
                        let abs = std::fs::canonicalize(&path)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|_| path.clone());
                        return Ok(format!(
                            "❌ 当前模型 `{model}` 不支持视觉输入。\n图片绝对路径: {abs}\n建议：切换到带 vision capability 的模型（如 gpt-4o / gemini-2.5-pro / claude-sonnet-4 / qwen-vl 等）后重试。"
                        ));
                    }
                    return crate::runtime::tools::read_file_image::read_image_as_marker(&local_path.to_string_lossy()).await;
                }
                match crate::runtime::tools::fs_local::read_text(&local_path).await {
                    Ok(content) => Ok(if content.is_empty() {
                        "（空文件）".to_string()
                    } else {
                        slice_text_lines(content, offset, limit)
                    }),
                    Err(local_error) => {
                        if crate::runtime::tools::fs_local::worker_execution_policy().is_some() {
                            return Err(local_error);
                        }
                        let result = match b.read_file(&path).await {
                            Ok(result) => result,
                            Err(original_error) => {
                        let Some(resource) = crate::runtime::skills::resolve_resource_path(&path) else {
                                    return Err(local_error.context(original_error.to_string()));
                        };
                        let metadata = tokio::fs::metadata(&resource).await?;
                        if metadata.len() > 10 * 1024 * 1024 {
                            anyhow::bail!("Skill resource exceeds 10MB limit");
                        }
                        let bytes = tokio::fs::read(&resource).await?;
                        if bytes.contains(&0) {
                            return Ok(format!("Binary file, size: {} bytes", bytes.len()));
                        }
                        let content = String::from_utf8(bytes)
                            .map_err(|error| anyhow::anyhow!("Skill resource is not UTF-8: {error}"))?;
                        return Ok(slice_text_lines(content, offset, limit));
                            }
                        };
                        if result.is_binary {
                            let size = result.meta.map(|m| m.size).unwrap_or(0);
                            return Ok(format!("Binary file, size: {} bytes", size));
                        }
                        let content = result.content.unwrap_or_else(|| "（空文件）".to_string());
                        Ok(if content == "（空文件）" {
                            content
                        } else {
                            slice_text_lines(content, offset, limit)
                        })
                    }
                }
                }
                .await;
                result.map(ToolOutput::text)
            })
        }),
    );

    registry.register_contextual_structured_with_impact(
        "write",
        "写入文件（覆盖已有内容，父目录不存在时自动创建）",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" },
                "content": { "type": "string", "description": "文件内容" }
            },
            "required": ["path", "content"]
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(move |context, args: &Value| {
            let path = contextual_tool_path(context, args["path"].as_str().unwrap_or(""));
            let content = args["content"].as_str().unwrap_or("").to_string();
            Box::pin(async move {
                let sandbox = crate::runtime::tools::fs_local::FsSandbox::from_config()?;
                let local_path = sandbox.resolve(&path)?;
                crate::runtime::tools::edit_file::write_path(&local_path, &content).await?;
                Ok(ToolOutput::text(format!(
                    "Written to {}",
                    local_path.display()
                )))
            })
        }),
    );

    registry.register_contextual_structured_with_impact(
        "remove_file",
        "删除文件或目录",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件或目录路径" }
            },
            "required": ["path"]
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::IrreversibleMutation,
        Box::new(move |context, args: &Value| {
            let path = contextual_tool_path(context, args["path"].as_str().unwrap_or(""));
            Box::pin(async move {
                let sandbox = crate::runtime::tools::fs_local::FsSandbox::from_config()?;
                let removed = sandbox.remove(&path).await?;
                Ok(ToolOutput::text(format!("Removed {}", removed.display())))
            })
        }),
    );

    let b = Arc::clone(&backend);
    registry.register_contextual_structured_with_impact(
        "get_file_info",
        "获取文件元信息",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" }
            },
            "required": ["path"]
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::Observe,
        Box::new(move |context, args: &Value| {
            let b = Arc::clone(&b);
            let path = args["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("path is required"))
                .map(|s| contextual_tool_path(context, s));
            Box::pin(async move {
                let path = path?;
                let info = b.get_file_info(&path).await?;
                Ok(ToolOutput::text(format!(
                    "名称: {}\n类型: {}\n大小: {} bytes\n修改: {}\n创建: {}",
                    info.name, info.kind, info.size, info.modified, info.created
                )))
            })
        }),
    );
}

/// Resolve relative tool paths from the active chat workspace. `tools.fsBase`
/// remains the sandbox boundary; it must not also become every session's cwd.
pub(super) fn contextual_tool_path(
    context: &crate::runtime::tools::context::ToolExecutionContext,
    path: &str,
) -> String {
    let expanded = shellexpand::tilde(path);
    let path = std::path::Path::new(expanded.as_ref());
    if path.is_absolute() {
        expanded.into_owned()
    } else {
        context.cwd.join(path).to_string_lossy().into_owned()
    }
}

/// 按 pi 的行号语义截取文本：`offset` 为 1-based 起始行号，`limit` 为行数。
/// 截取时附带总行数与续读提示，便于模型按需继续读取大文件。
fn slice_text_lines(content: String, offset: usize, limit: Option<usize>) -> String {
    if offset == 1 && limit.is_none() {
        return content;
    }
    let total = content.lines().count();
    let start = offset.saturating_sub(1);
    if start >= total {
        return format!("[offset {} 超出文件末尾（共 {} 行）]", offset, total);
    }
    let mut lines: Vec<&str> = content.lines().skip(start).collect();
    let mut note = String::new();
    if let Some(limit) = limit {
        if lines.len() > limit {
            lines.truncate(limit);
            let next = offset + limit;
            note = format!(
                "\n[已显示第 {offset}-{} 行，共 {total} 行；续读请传 offset={next}]",
                offset + limit - 1
            );
        }
    }
    format!("{}{}", lines.join("\n"), note)
}

/// `generate_image` — the single image-generation entry exposed to the chat Agent.
///
/// The chat model plans composition, writes the final prompt, and selects an aspect ratio.
/// The daemon selects the configured default image model and adapts its upstream protocol.
fn register_generate_image_tool(registry: &mut ToolRegistry) {
    registry.register_contextual_structured_with_impact(
        "generate_image",
        "生成、参考、编辑或派生一张高质量栅格图片。先识别视觉意图和场景，再把用户事实整理为结构化参数；不要在 prompt 中编造用户未提供的数据、结论或科学关系。真实实验结果、显微数据和证据性图表应改用真实数据绘图。当前消息中如有 imgref_*，只按实际用途选择并放入 imageRefs。一次调用只生成一张图片；自动验收只报告问题，不会自动修复。验收建议修改时，必须先向用户说明问题并得到明确确认，才能再次调用本工具。",
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "提交给生图模型的最终提示词；必须使用设置中的回复语言，并包含主体、构图、视觉风格、关键细节和必要文字要求。"
                },
                "intent": {
                    "type": "string",
                    "enum": ["generate", "reference", "edit", "variant"],
                    "description": "新图、参考图、局部编辑或相似变体。"
                },
                "scene": {
                    "type": "string",
                    "enum": ["auto", "photo", "illustration", "poster", "infographic", "scientific", "product", "ui-mockup", "asset"],
                    "description": "视觉场景；不确定时使用 auto。"
                },
                "purpose": { "type": "string", "description": "图片用途。" },
                "audience": { "type": "string", "description": "目标受众。" },
                "exactText": {
                    "type": "array", "items": { "type": "string" },
                    "description": "必须逐字出现的文字；没有则留空。"
                },
                "aspectRatio": {
                    "type": "string",
                    "enum": ["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"],
                    "description": "根据内容构图选择的宽高比；省略时为 1:1。"
                },
                "negativePrompt": {
                    "type": "string",
                    "description": "可选负面提示词；仅在上游协议支持时使用。"
                },
                "imageRefs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "pattern": "^imgref_" },
                            "role": { "type": "string", "enum": ["edit-target", "content", "style", "composition"] }
                        },
                        "required": ["id", "role"],
                        "additionalProperties": false
                    }
                },
                "invariants": { "type": "array", "items": { "type": "string" }, "description": "编辑或参考时必须保持不变的内容。" },
                "exclusions": { "type": "array", "items": { "type": "string" }, "description": "明确禁止出现的内容。" },
                "factualConstraints": { "type": "array", "items": { "type": "string" }, "description": "不可改写的事实、术语和关系。" }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
        // Image generation is the explicitly requested, reversible product
        // action. Routing it through the MoA pre-action gate allowed a quality
        // reviewer to replace the requested raster result with SVG before the
        // image model ever ran. The visual QA stage remains the quality gate.
        ToolExecutionMode::Parallel,
        ToolImpact::ReversibleMutation,
        Box::new(move |context, args: &Value| {
            let request = image_generation_request(context, args);
            let active_model = context.active_model.clone();
            Box::pin(async move {
                let request = request?;

                progress::emit("🎨 正在解析视觉需求…");
                let data_dir = crate::runtime::settings::local_config::data_dir();
                let result = crate::runtime::image_generation::ImageGenerationService::new(&data_dir)
                    .generate(&request, active_model.as_ref())
                    .await?;
                let image_url = result
                    .image_urls
                    .first()
                    .map(String::as_str)
                    .context("生图服务未返回图片")?;
                let model = &result.model;
                let qa_guidance = result
                    .generated_image_record
                    .qa
                    .repair_prompt
                    .as_deref()
                    .map(|repair_prompt| {
                        format!(
                            " 自动验收建议修改：{repair_prompt}。请先向用户说明并询问是否修改；未得到明确确认前，不得再次调用 generate_image。"
                        )
                    })
                    .unwrap_or_default();

                progress::emit_generated_image(
                    image_url,
                    &request.prompt,
                    &result.generated_image_record,
                );
                progress::emit(&format!("✅ 图片已生成（{model}）"));
                Ok(ToolOutput::text(format!(
                    "已生成 1 张图片并展示在对话中（model={model}，url={image_url}）。{qa_guidance}"
                )))
            })
        }),
    );
}

fn image_generation_request(
    context: &crate::runtime::tools::context::ToolExecutionContext,
    args: &Value,
) -> anyhow::Result<crate::runtime::image_generation::ImageGenerationRequest> {
    let session_id = context
        .chat_session_id
        .as_ref()
        .or(context.session_id.as_ref())
        .map(ToString::to_string)
        .unwrap_or_else(|| "default".to_string());
    let mut request_value = args.clone();
    request_value["sessionId"] = Value::String(session_id);
    let mut request: crate::runtime::image_generation::ImageGenerationRequest =
        serde_json::from_value(request_value).context("生图参数无效")?;
    if !request.image_refs.is_empty() {
        let registry = context
            .image_references
            .as_ref()
            .context("当前任务没有可用的图片引用")?;
        request.resolved_images = request
            .image_refs
            .iter()
            .map(|reference| {
                Ok(crate::runtime::image_generation::ResolvedImageInput {
                    reference: registry.resolve(&reference.id)?,
                    role: reference.role,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
    }
    Ok(request)
}

fn register_shell_tools(registry: &mut ToolRegistry) {
    registry.register_contextual_structured_with_impact_resolver(
        "bash",
        "执行本地 bash/shell 命令（支持 ls、cat、grep、git 等常用命令）。命令在独立进程组中执行，超时或取消时会清理整个进程树；不传 timeout 时不限时（执行层有 30 分钟硬上限）。",
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "要执行的 bash 命令，例如 'ls -la' 或 'git status'" },
                "cwd": { "type": "string", "description": "工作目录（可选，默认为当前目录）" },
                "timeout": { "type": "integer", "minimum": 1, "description": "执行超时秒数（可选；不传表示不限时，硬上限 1800 秒）" }
            },
            "required": ["command"]
        }),
        ToolExecutionMode::Parallel,
        ToolImpact::ExternalSideEffect,
        bash_impact,
        Box::new(move |context, args: &Value| {
            let command = args["command"].as_str().unwrap_or("").to_string();
            let cwd = Some(contextual_tool_path(
                context,
                args["cwd"].as_str().unwrap_or("."),
            ));
            let timeout_secs = args["timeout"].as_u64();
            let session_id = shell_session_id(context);
            Box::pin(async move {
                if command.is_empty() {
                    anyhow::bail!("命令不能为空");
                }
                let sandbox = crate::runtime::bash::SandboxConfig::default();
                let output = if let Some(session_id) = session_id {
                    crate::runtime::bash::shell_state::execute(
                        &session_id,
                        command,
                        cwd,
                        timeout_secs,
                        &sandbox,
                    )
                    .await?
                } else {
                    let mut input = crate::runtime::bash::BashCommandInput::new(command);
                    if let Some(timeout_secs) = timeout_secs {
                        input = input.with_timeout(timeout_secs);
                    }
                    if let Some(c) = cwd {
                        input = input.with_cwd(c);
                    }
                    crate::runtime::bash::execute_bash(input, &sandbox).await?
                };

                let mut result = output.combined_output();
                if output.timed_out {
                    result.push_str(&format!(
                        "\n[超时（{}s）已终止，进程组已清理]",
                        output.effective_timeout_secs.unwrap_or(0)
                    ));
                } else if !output.is_success() {
                    result.push_str(&format!(
                        "\n[退出码: {}]",
                        output.exit_code.unwrap_or(-1)
                    ));
                }
                Ok(ToolOutput::text(result))
            })
        }),
    );
}

fn bash_impact(_args: &Value) -> ToolImpact {
    ToolImpact::ExternalSideEffect
}

fn shell_session_id(
    context: &crate::runtime::tools::context::ToolExecutionContext,
) -> Option<String> {
    context.session_id.as_ref().map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::backend::BackendClient;
    use crate::runtime::tools::registry::ToolRegistry;
    use mockito::Server;

    fn merge_example(target: &mut Value, addition: Value) {
        if let (Some(target), Value::Object(addition)) = (target.as_object_mut(), addition) {
            target.extend(addition);
        }
    }

    #[test]
    fn relative_tool_paths_follow_the_session_workspace() {
        let context = crate::runtime::tools::context::ToolExecutionContext {
            cwd: std::path::PathBuf::from("/tmp/pwcli-project"),
            ..Default::default()
        };
        assert_eq!(
            contextual_tool_path(&context, "src/main.rs"),
            "/tmp/pwcli-project/src/main.rs"
        );
        assert_eq!(
            contextual_tool_path(&context, "/tmp/explicit"),
            "/tmp/explicit"
        );
    }

    fn schema_example(schema: &Value) -> Value {
        if let Some(value) = schema
            .get("examples")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
        {
            return value.clone();
        }
        if let Some(value) = schema.get("default").or_else(|| schema.get("const")) {
            return value.clone();
        }
        if let Some(value) = schema
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
        {
            return value.clone();
        }
        let kind = schema
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                if schema.get("properties").is_some() {
                    "object"
                } else {
                    "string"
                }
            });
        match kind {
            "object" => {
                let mut value = Value::Object(Default::default());
                fill_required_properties(&mut value, schema, schema);
                for key in ["oneOf", "anyOf"] {
                    if let Some(branch) = schema
                        .get(key)
                        .and_then(Value::as_array)
                        .and_then(|v| v.first())
                    {
                        merge_example(&mut value, schema_example(branch));
                        fill_required_properties(&mut value, branch, schema);
                    }
                }
                if let Some(branches) = schema.get("allOf").and_then(Value::as_array) {
                    for branch in branches {
                        if let (Some(condition), Some(then_schema)) =
                            (branch.get("if"), branch.get("then"))
                        {
                            let matches = jsonschema::validator_for(condition)
                                .map(|validator| validator.is_valid(&value))
                                .unwrap_or(false);
                            if matches {
                                merge_example(&mut value, schema_example(then_schema));
                                fill_required_properties(&mut value, then_schema, schema);
                            }
                        } else {
                            merge_example(&mut value, schema_example(branch));
                            fill_required_properties(&mut value, branch, schema);
                        }
                    }
                }
                value
            }
            "array" => {
                let count = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
                let item = schema
                    .get("items")
                    .map(schema_example)
                    .unwrap_or(Value::Null);
                Value::Array(std::iter::repeat_n(item, count).collect())
            }
            "integer" => Value::from(schema.get("minimum").and_then(Value::as_i64).unwrap_or(0)),
            "number" => Value::from(schema.get("minimum").and_then(Value::as_f64).unwrap_or(0.0)),
            "boolean" => Value::Bool(false),
            "null" => Value::Null,
            _ => {
                let pattern = schema
                    .get("pattern")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let value = if pattern.starts_with("^/") {
                    "/x"
                } else if pattern.starts_with("^frag-") {
                    "frag-0000000000000000"
                } else if pattern.starts_with("^imgref_") {
                    "imgref_x"
                } else {
                    "x"
                };
                let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
                Value::String(if value.len() >= min {
                    value.into()
                } else {
                    "x".repeat(min)
                })
            }
        }
    }

    fn fill_required_properties(
        target: &mut Value,
        requirement_schema: &Value,
        property_schema: &Value,
    ) {
        let Some(target) = target.as_object_mut() else {
            return;
        };
        let properties = property_schema.get("properties").and_then(Value::as_object);
        for name in requirement_schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let schema = requirement_schema
                .pointer(&format!("/properties/{name}"))
                .or_else(|| properties.and_then(|properties| properties.get(name)));
            if let Some(schema) = schema {
                target.insert(name.to_string(), schema_example(schema));
            } else {
                target
                    .entry(name.to_string())
                    .or_insert(Value::String("x".into()));
            }
        }
    }

    #[test]
    fn shell_commands_are_external_side_effects() {
        let direct = serde_json::json!({
            "command": "example-tool status"
        });
        assert_eq!(bash_impact(&direct), ToolImpact::ExternalSideEffect);

        let composed = serde_json::json!({
            "command": "example-tool status | jq ."
        });
        assert_eq!(bash_impact(&composed), ToolImpact::ExternalSideEffect);
    }

    #[test]
    fn read_slicing_uses_one_based_line_numbers() {
        let content = "line1\nline2\nline3\nline4".to_string();
        assert_eq!(slice_text_lines(content.clone(), 1, None), content);
        assert_eq!(
            slice_text_lines(content.clone(), 2, None),
            "line2\nline3\nline4"
        );
        let sliced = slice_text_lines(content.clone(), 2, Some(2));
        assert!(sliced.starts_with("line2\nline3"));
        assert!(sliced.contains("共 4 行"));
        assert!(sliced.contains("offset=4"));
        assert!(slice_text_lines(content, 10, None).contains("超出文件末尾"));
    }

    #[test]
    fn run_command_selects_shell_state_only_from_explicit_context() {
        let context = crate::runtime::tools::context::ToolExecutionContext {
            session_id: Some("shell-context".to_string().into()),
            ..crate::runtime::tools::context::ToolExecutionContext::default()
        };
        assert_eq!(shell_session_id(&context).as_deref(), Some("shell-context"));
        assert_eq!(
            shell_session_id(&crate::runtime::tools::context::ToolExecutionContext::default()),
            None
        );
    }

    #[test]
    fn image_generation_context_preserves_cli_default_and_fails_closed_for_missing_refs() {
        let request = image_generation_request(
            &crate::runtime::tools::context::ToolExecutionContext::default(),
            &serde_json::json!({"prompt": "test"}),
        )
        .unwrap();
        assert_eq!(request.session_id, "default");

        let error = image_generation_request(
            &crate::runtime::tools::context::ToolExecutionContext::default(),
            &serde_json::json!({
                "prompt": "edit",
                "imageRefs": [{"id": "imgref_missing", "role": "edit-target"}]
            }),
        )
        .unwrap_err();
        assert!(error.to_string().contains("没有可用的图片引用"));
    }

    #[test]
    fn internal_worker_allowlist_removes_external_mutators() {
        let registry = ToolRegistry::new();
        let schema = serde_json::json!({ "type": "object" });
        for name in ["read", "write", "edit", "bash", "manage_jobs", "ssh_exec"] {
            registry.register(
                name,
                "test",
                schema.clone(),
                Box::new(|_| Box::pin(async { Ok(String::new()) })),
            );
        }

        restrict_to_internal_worker_tools(&registry, false);

        assert!(registry.has_tool("read"));
        assert!(registry.has_tool("bash"));
        assert!(!registry.has_tool("write"));
        assert!(!registry.has_tool("manage_jobs"));
        assert!(!registry.has_tool("ssh_exec"));

        let mutating_registry = ToolRegistry::new();
        for name in ["write", "edit", "manage_jobs"] {
            mutating_registry.register(
                name,
                "test",
                schema.clone(),
                Box::new(|_| Box::pin(async { Ok(String::new()) })),
            );
        }
        restrict_to_internal_worker_tools(&mutating_registry, true);
        assert!(mutating_registry.has_tool("write"));
        assert!(mutating_registry.has_tool("edit"));
        assert!(!mutating_registry.has_tool("manage_jobs"));
    }

    #[tokio::test]
    async fn test_registry_has_only_unified_data_crud() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/data/todos")
            .with_status(200)
            .with_body(r#"{"data": []}"#)
            .create();

        let backend = Arc::new(BackendClient::new(server.url()));
        let mut registry = ToolRegistry::new();
        register_all_tools(&mut registry, backend);

        // data_crud should be present
        let defs = registry.list_definitions();
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.contains(&"data_crud"),
            "data_crud should be registered"
        );
        assert!(
            names.contains(&"web_query"),
            "atomic web tools stay available"
        );
        assert!(
            names.contains(&"web_search_domains"),
            "AnySearch vertical-domain discovery stays available"
        );
        assert!(!names.contains(&"search_route"));
        assert!(!names.contains(&"web_research_papers"));
        assert!(
            names.contains(&"bash"),
            "atomic analysis tools stay available"
        );
        assert!(
            names.contains(&"grep") && names.contains(&"find"),
            "pi-aligned grep/find search tools stay available"
        );
        assert!(
            !names.contains(&"read_file")
                && !names.contains(&"list_directory")
                && !names.contains(&"search_files")
                && !names.contains(&"write_file")
                && !names.contains(&"edit_file")
                && !names.contains(&"run_command"),
            "legacy tool names are fully replaced by pi names"
        );
        assert!(
            names.contains(&"proceed_without_clarification"),
            "the mandatory clarification gate needs an explicit proceed tool"
        );
        let read_tool = defs
            .iter()
            .find(|definition| definition.name == "read")
            .expect("read should be registered");
        let read_properties = read_tool.parameters["properties"]
            .as_object()
            .expect("read properties");
        assert!(read_properties.contains_key("offset"));
        assert!(read_properties.contains_key("limit"));
        let bash_tool = defs
            .iter()
            .find(|definition| definition.name == "bash")
            .expect("bash should be registered");
        let bash_required = bash_tool.parameters["required"]
            .as_array()
            .expect("bash required");
        assert!(
            bash_required
                .iter()
                .all(|name| name.as_str() != Some("timeout")),
            "bash timeout must stay optional"
        );
        let image_tool = defs
            .iter()
            .find(|definition| definition.name == "generate_image")
            .expect("generate_image should be registered");
        let image_properties = image_tool.parameters["properties"]
            .as_object()
            .expect("generate_image properties");
        assert!(image_properties.contains_key("prompt"));
        assert!(image_properties.contains_key("aspectRatio"));
        assert!(image_properties.contains_key("negativePrompt"));
        assert!(!image_properties.contains_key("model"));
        assert!(!image_properties.contains_key("size"));
        assert!(!image_properties.contains_key("n"));
        assert!(image_tool.description.contains("得到明确确认"));
        assert_eq!(
            registry.impact("generate_image"),
            Some(ToolImpact::ReversibleMutation)
        );
        assert!(!registry
            .impact("generate_image")
            .expect("generate_image impact")
            .requires_decision());
        assert!(
            !names.contains(&"deep_research") && !names.contains(&"data_analysis"),
            "research and analysis must use free atomic-tool orchestration"
        );

        // legacy data tools should NOT be registered (they confuse the LLM)
        assert!(
            !names.contains(&"list_todos"),
            "list_todos should be removed"
        );
        assert!(
            !names.contains(&"list_bookmarks"),
            "list_bookmarks should be removed"
        );
    }

    #[tokio::test]
    async fn every_registered_tool_accepts_generated_minimal_arguments() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/data/todos")
            .with_status(200)
            .with_body(r#"{"data": []}"#)
            .create();
        let backend = Arc::new(BackendClient::new(server.url()));
        let mut registry = ToolRegistry::new();
        register_all_tools(&mut registry, backend);
        let definitions = registry.list_definitions();
        assert!(definitions.len() >= 30, "unexpectedly small tool registry");
        let contract_registry = ToolRegistry::new();
        let mut failures = Vec::new();
        for definition in &definitions {
            let arguments = schema_example(&definition.parameters);
            if let Err(error) = registry.validate_arguments(&definition.name, &arguments) {
                failures.push(format!(
                    "{} args={} error={error}",
                    definition.name, arguments
                ));
                continue;
            }

            let wire_arguments = serde_json::to_string(&arguments).unwrap_or_else(|error| {
                panic!("{} arguments do not serialize: {error}", definition.name)
            });
            let decoded_arguments: Value =
                serde_json::from_str(&wire_arguments).unwrap_or_else(|error| {
                    panic!("{} arguments do not round-trip: {error}", definition.name)
                });
            assert_eq!(
                decoded_arguments, arguments,
                "{} argument round-trip",
                definition.name
            );

            contract_registry.register_structured_with_impact(
                definition.name.clone(),
                definition.description.clone(),
                definition.parameters.clone(),
                definition.execution_mode,
                definition.impact,
                Box::new(|args| {
                    let args = args.clone();
                    Box::pin(async move { Ok(ToolOutput::text(args.to_string())) })
                }),
            );
        }
        assert!(
            failures.is_empty(),
            "tool contract failures:\n{}",
            failures.join("\n")
        );
        for definition in &definitions {
            let arguments = schema_example(&definition.parameters);
            let output = contract_registry
                .execute(&definition.name, &arguments)
                .await
                .unwrap_or_else(|error| panic!("{} dispatch failed: {error}", definition.name));
            let echoed: Value = serde_json::from_str(&output).unwrap_or_else(|error| {
                panic!("{} returned invalid JSON: {error}", definition.name)
            });
            assert_eq!(echoed, arguments, "{} dispatch round-trip", definition.name);
        }
        let proceed = registry
            .execute("proceed_without_clarification", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(proceed.contains("Clarification gate resolved"));
    }
}
