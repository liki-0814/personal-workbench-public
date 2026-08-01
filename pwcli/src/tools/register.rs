use std::sync::Arc;

use anyhow::Context;
use serde_json::{json, Value};

use crate::backend::BackendClient;
use crate::tools::progress;
use crate::tools::registry::{ToolExecutionMode, ToolImpact, ToolRegistry};

/// Register all built-in tools into the given [`ToolRegistry`].
///
/// This includes data CRUD tools, file system tools (read, write, list, search),
/// shell execution and dynamically loaded skills from `~/.agents/skills/`.
/// Persistent CLI delegation is registered later with the RuntimeTask broker.
pub fn register_all_tools(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    crate::tools::data_crud::register(registry, Arc::clone(&backend));
    crate::tools::documents::register(registry);
    crate::tools::archify::register(registry);
    crate::tools::jobs::register(registry, Arc::clone(&backend));
    register_fs_tools(registry, Arc::clone(&backend));
    crate::tools::text_search::register(registry);
    register_artifact_tools(registry);
    crate::tools::edit_file::register(registry);
    register_generate_image_tool(registry);
    register_shell_tools(registry);
    register_decision_control_tool(registry);
    // Research and analysis use the ordinary Agent loop plus atomic
    // web/filesystem/shell tools; no monolithic workflow pipeline is exposed.

    // 注册 SSH 远程操作工具
    crate::tools::ssh::register(registry, Arc::clone(&backend));
    tracing::debug!("SSH tools registered");

    // 加载 skill 并注册为工具
    let skills = crate::skills::load_skills();
    if !skills.is_empty() {
        tracing::debug!(count = skills.len(), "skill tools registered");
        crate::skills::register_skill_tools(registry, skills);
    }

    // AnySearch 提供公开网页搜索与正文抽取。
    crate::tools::web::register(registry);

    // 注册 read_pdf 工具（三级 fallback：arxiv-html → MinerU → PyMuPDF）
    crate::tools::pdf::register_read_pdf(registry);
    // 启动时懒清理一次 24h 过期 PDF 临时目录
    if crate::tools::fs_local::worker_execution_policy().is_none() {
        tokio::spawn(async {
            crate::tools::pdf::cleanup_on_startup().await;
            if let Err(error) = crate::tools::artifacts::cleanup_expired_tool_outputs() {
                tracing::warn!(%error, "failed to clean expired tool output artifacts");
            }
            let indexed = crate::tools::pdf::backfill_pdf_artifact_memories();
            if indexed > 0 {
                tracing::info!(indexed, "backfilled PDF artifact memory indexes");
            }
        });
    } else {
        let allow_mutation = crate::tools::fs_local::worker_execution_policy()
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
        "list_directory",
        "read_file",
        "search_files",
        "search_file_content",
        "run_command",
        "read_artifact",
        "web_read",
        "web_search_domains",
        "web_query",
        "web_fetch_batch",
    ];
    if allow_mutation {
        allowed.extend(["write_file", "remove_file", "edit_file"]);
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
                let chunk = crate::tools::artifacts::read_tool_output(&artifact_id?, offset, limit)?;
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
            "required": ["rationale"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::Control,
        Box::new(|_| {
            Box::pin(async {
                Ok(crate::tools::registry::ToolOutput {
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
        "Request an independent Harness MoA review when the current task contains a genuine ambiguity, conflicting evidence, or materially different approaches. Do not use for routine steps.",
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
                Ok(crate::tools::registry::ToolOutput {
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
    if crate::tools::fs_local::worker_execution_policy().is_some() {
        return;
    }
    crate::tools::memory::register(registry, user_slug);
}

pub async fn register_runtime_tools(registry: Arc<ToolRegistry>, _llm: Arc<crate::llm::LlmClient>) {
    // Durable child work is registered by `dispatch_tasks` after the daemon's
    // RuntimeTask broker exists. The retired in-process `task` executor must not
    // be reintroduced here because it retained parent Futures and in-memory state.
    if crate::tools::fs_local::worker_execution_policy().is_some() {
        return;
    }
    match crate::tools::extensions::register_trusted(Arc::clone(&registry)).await {
        Ok(count) if count > 0 => tracing::debug!(count, "trusted extension tools registered"),
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "trusted extension loading failed"),
    }
}

fn register_fs_tools(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    let b = Arc::clone(&backend);
    registry.register(
        "list_directory",
        "列出目录内容",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "目录路径（可选）" }
            }
        }),
        Box::new(move |args: &Value| {
            let b = Arc::clone(&b);
            let path = args["path"].as_str().unwrap_or(".").to_string();
            Box::pin(async move {
                let entries = match crate::tools::fs_local::FsSandbox::from_config()
                    .and_then(|sandbox| sandbox.resolve(&path))
                {
                    Ok(local_path) => {
                        match crate::tools::fs_local::list_directory(&local_path).await {
                            Ok(entries) => entries
                                .into_iter()
                                .map(|entry| crate::backend::DirEntry {
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
                                if crate::tools::fs_local::worker_execution_policy().is_some() {
                                    return Err(error);
                                }
                                b.list_dir(&path).await?
                            }
                        }
                    }
                    Err(error) => return Err(error),
                };
                if entries.is_empty() {
                    return Ok("（空目录）".to_string());
                }
                let lines: Vec<String> = entries
                    .iter()
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
                Ok(lines.join("\n"))
            })
        }),
    );

    let b = Arc::clone(&backend);
    registry.register(
        "read_file",
        "读取文件内容。文本文件返回原文；图片（png/jpg/jpeg/gif/webp，≤5MB）在当前模型支持视觉时以多模态形式注入对话，否则拒绝并提示切换视觉模型。",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" }
            },
            "required": ["path"]
        }),
        Box::new(move |args: &Value| {
            let b = Arc::clone(&b);
            let path = args["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("path is required"))
                .map(|s| s.to_string());
            Box::pin(async move {
                let path = path?;
                let sandbox = crate::tools::fs_local::FsSandbox::from_config()?;
                let local_path = sandbox.resolve(&path)?;
                // ── 图片分支：本地直读 + 视觉门控 ──
                if crate::tools::read_file_image::detect_image_ext(&path).is_some() {
                    if !crate::tools::read_file_image::current_model_supports_vision() {
                        let model = crate::tools::read_file_image::current_model_label();
                        let abs = std::fs::canonicalize(&path)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|_| path.clone());
                        return Ok(format!(
                            "❌ 当前模型 `{model}` 不支持视觉输入。\n图片绝对路径: {abs}\n建议：切换到带 vision capability 的模型（如 gpt-4o / gemini-2.5-pro / claude-sonnet-4 / qwen-vl 等）后重试。"
                        ));
                    }
                    return crate::tools::read_file_image::read_image_as_marker(&local_path.to_string_lossy()).await;
                }
                match crate::tools::fs_local::read_text(&local_path).await {
                    Ok(content) => Ok(if content.is_empty() {
                        "（空文件）".to_string()
                    } else {
                        content
                    }),
                    Err(local_error) => {
                        if crate::tools::fs_local::worker_execution_policy().is_some() {
                            return Err(local_error);
                        }
                        let result = match b.read_file(&path).await {
                            Ok(result) => result,
                            Err(original_error) => {
                        let Some(resource) = crate::skills::resolve_resource_path(&path) else {
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
                        return String::from_utf8(bytes)
                            .map_err(|error| anyhow::anyhow!("Skill resource is not UTF-8: {error}"));
                            }
                        };
                        if result.is_binary {
                            let size = result.meta.map(|m| m.size).unwrap_or(0);
                            return Ok(format!("Binary file, size: {} bytes", size));
                        }
                        Ok(result.content.unwrap_or_else(|| "（空文件）".to_string()))
                    }
                }
            })
        }),
    );

    registry.register_with_impact(
        "write_file",
        "写入文件",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" },
                "content": { "type": "string", "description": "文件内容" }
            },
            "required": ["path", "content"]
        }),
        ToolImpact::ReversibleMutation,
        Box::new(move |args: &Value| {
            let path = args["path"].as_str().unwrap_or("").to_string();
            let content = args["content"].as_str().unwrap_or("").to_string();
            Box::pin(async move {
                let sandbox = crate::tools::fs_local::FsSandbox::from_config()?;
                let local_path = sandbox.resolve(&path)?;
                crate::tools::edit_file::write_path(&local_path, &content).await?;
                Ok(format!("Written to {}", local_path.display()))
            })
        }),
    );

    registry.register_with_impact(
        "remove_file",
        "删除文件或目录",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件或目录路径" }
            },
            "required": ["path"]
        }),
        ToolImpact::IrreversibleMutation,
        Box::new(move |args: &Value| {
            let path = args["path"].as_str().unwrap_or("").to_string();
            Box::pin(async move {
                let sandbox = crate::tools::fs_local::FsSandbox::from_config()?;
                let removed = sandbox.remove(&path).await?;
                Ok(format!("Removed {}", removed.display()))
            })
        }),
    );

    let b = Arc::clone(&backend);
    registry.register(
        "search_files",
        "搜索文件",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索关键词" },
                "path": { "type": "string", "description": "搜索目录（可选）" }
            },
            "required": ["query"]
        }),
        Box::new(move |args: &Value| {
            let b = Arc::clone(&b);
            let query = args["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("query is required"))
                .map(|s| s.to_string());
            let search_path = args["path"].as_str().map(|s| s.to_string());
            Box::pin(async move {
                let query = query?;
                let sandbox = crate::tools::fs_local::FsSandbox::from_config()?;
                let local_path = sandbox.resolve(search_path.as_deref().unwrap_or("."))?;
                let files = match crate::tools::fs_local::search_files(&local_path, &query).await {
                    Ok(files) => files,
                    Err(error) => {
                        if crate::tools::fs_local::worker_execution_policy().is_some() {
                            return Err(error);
                        }
                        b.search_files(&query, search_path.as_deref()).await?
                    }
                };
                if files.is_empty() {
                    return Ok("未找到匹配的文件".to_string());
                }
                Ok(files.join("\n"))
            })
        }),
    );

    let b = Arc::clone(&backend);
    registry.register(
        "get_file_info",
        "获取文件元信息",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" }
            },
            "required": ["path"]
        }),
        Box::new(move |args: &Value| {
            let b = Arc::clone(&b);
            let path = args["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("path is required"))
                .map(|s| s.to_string());
            Box::pin(async move {
                let path = path?;
                let info = b.get_file_info(&path).await?;
                Ok(format!(
                    "名称: {}\n类型: {}\n大小: {} bytes\n修改: {}\n创建: {}",
                    info.name, info.kind, info.size, info.modified, info.created
                ))
            })
        }),
    );
}

/// `generate_image` — the single image-generation entry exposed to the chat Agent.
///
/// The chat model plans composition, writes the final prompt, and selects an aspect ratio.
/// The daemon selects the configured default image model and adapts its upstream protocol.
fn register_generate_image_tool(registry: &mut ToolRegistry) {
    registry.register_with_impact(
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
        ToolImpact::ReversibleMutation,
        Box::new(move |args: &Value| {
            let args = args.clone();
            Box::pin(async move {
                let session_id = crate::tools::web_context::chat_session_id()
                    .or_else(crate::tools::web_context::session_id)
                    .unwrap_or_else(|| "default".to_string());
                let mut request_value = args;
                request_value["sessionId"] = Value::String(session_id);
                let mut request: crate::image_generation::ImageGenerationRequest =
                    serde_json::from_value(request_value).context("生图参数无效")?;
                if !request.image_refs.is_empty() {
                    let registry = crate::tools::web_context::image_references()
                        .context("当前任务没有可用的图片引用")?;
                    request.resolved_images = request
                        .image_refs
                        .iter()
                        .map(|reference| {
                            Ok(crate::image_generation::ResolvedImageInput {
                                reference: registry.resolve(&reference.id)?,
                                role: reference.role,
                            })
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?;
                }

                progress::emit("🎨 正在解析视觉需求…");
                let data_dir = crate::config::local_config::data_dir();
                let result = crate::image_generation::ImageGenerationService::new(&data_dir)
                    .generate(&request)
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
                Ok(format!(
                    "已生成 1 张图片并展示在对话中（model={model}，url={image_url}）。{qa_guidance}"
                ))
            })
        }),
    );
}

fn register_shell_tools(registry: &mut ToolRegistry) {
    registry.register_with_impact_resolver(
        "run_command",
        "执行本地 bash/shell 命令（支持 ls、cat、grep、git 等常用命令）",
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "要执行的 bash 命令，例如 'ls -la' 或 'git status'" },
                "cwd": { "type": "string", "description": "工作目录（可选，默认为当前目录）" },
                "timeout_secs": { "type": "integer", "description": "执行超时秒数（默认 30）" }
            },
            "required": ["command"]
        }),
        ToolImpact::ExternalSideEffect,
        run_command_impact,
        Box::new(move |args: &Value| {
            let command = args["command"].as_str().unwrap_or("").to_string();
            let cwd = args["cwd"].as_str().map(|s| s.to_string());
            let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(30);
            if command.is_empty() {
                return Box::pin(async move { Err(anyhow::anyhow!("命令不能为空")) });
            }
            Box::pin(async move {
                let sandbox = crate::bash::SandboxConfig::default();
                let output = if let Some(session_id) = crate::tools::web_context::session_id() {
                    crate::bash::shell_state::execute(
                        &session_id,
                        command,
                        cwd,
                        timeout_secs,
                        &sandbox,
                    )
                    .await?
                } else {
                    let mut input = crate::bash::BashCommandInput::new(command)
                        .with_timeout(timeout_secs);
                    if let Some(c) = cwd {
                        input = input.with_cwd(c);
                    }
                    crate::bash::execute_bash(input, &sandbox).await?
                };

                let mut result = output.combined_output();
                if output.timed_out {
                    result.push_str(&format!(
                        "\n[超时（{}s）已终止]",
                        timeout_secs
                    ));
                } else if !output.is_success() {
                    result.push_str(&format!(
                        "\n[退出码: {}]",
                        output.exit_code.unwrap_or(-1)
                    ));
                }
                Ok(result)
            })
        }),
    );
}

fn run_command_impact(_args: &Value) -> ToolImpact {
    ToolImpact::ExternalSideEffect
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendClient;
    use crate::tools::registry::ToolRegistry;
    use mockito::Server;

    #[test]
    fn shell_commands_are_external_side_effects() {
        let direct = serde_json::json!({
            "command": "example-tool status"
        });
        assert_eq!(run_command_impact(&direct), ToolImpact::ExternalSideEffect);

        let composed = serde_json::json!({
            "command": "example-tool status | jq ."
        });
        assert_eq!(
            run_command_impact(&composed),
            ToolImpact::ExternalSideEffect
        );
    }

    #[test]
    fn internal_worker_allowlist_removes_external_mutators() {
        let registry = ToolRegistry::new();
        let schema = serde_json::json!({ "type": "object" });
        for name in [
            "read_file",
            "write_file",
            "edit_file",
            "run_command",
            "manage_jobs",
            "ssh_exec",
        ] {
            registry.register(
                name,
                "test",
                schema.clone(),
                Box::new(|_| Box::pin(async { Ok(String::new()) })),
            );
        }

        restrict_to_internal_worker_tools(&registry, false);

        assert!(registry.has_tool("read_file"));
        assert!(registry.has_tool("run_command"));
        assert!(!registry.has_tool("write_file"));
        assert!(!registry.has_tool("manage_jobs"));
        assert!(!registry.has_tool("ssh_exec"));

        let mutating_registry = ToolRegistry::new();
        for name in ["write_file", "edit_file", "manage_jobs"] {
            mutating_registry.register(
                name,
                "test",
                schema.clone(),
                Box::new(|_| Box::pin(async { Ok(String::new()) })),
            );
        }
        restrict_to_internal_worker_tools(&mutating_registry, true);
        assert!(mutating_registry.has_tool("write_file"));
        assert!(mutating_registry.has_tool("edit_file"));
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
            names.contains(&"run_command"),
            "atomic analysis tools stay available"
        );
        assert!(
            names.contains(&"proceed_without_clarification"),
            "the mandatory clarification gate needs an explicit proceed tool"
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
}
