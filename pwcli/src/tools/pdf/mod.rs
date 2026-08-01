//! `read_pdf` 工具：三级 fallback 抓取 PDF → markdown，默认永久保存到 Artifacts。
//!
//! 默认 **精度优先**（`prefer_speed=false`）：
//! 1. [`mineru`]: MinerU API Precision (有 token) 或 Agent (无 token)
//! 2. [`arxiv_html`]: arxiv pdf URL 启发兜底（仅 arxiv，最快）
//! 3. [`pymupdf`]: 本地 `python3 + fitz`（需 `pip install pymupdf`）
//!
//! 速度优先（`prefer_speed=true`）：
//! 1. [`arxiv_html`] → 2. [`mineru`] → 3. [`pymupdf`]
//!
//! 落地策略：
//! - 默认 → `<dataDir>/artifacts/pdf/<source-slug>-<hash>/`，永久保留
//! - `ephemeral=true` → `~/.pwcli/tmp/pdf/<sha8>/`，24h 懒清理
//! - `output_dir="/path"` → 用户指定目录，永久保留

pub mod arxiv_html;
pub mod mineru;
pub mod pymupdf;
pub mod tmp_dir;

use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::warn;

use crate::memory::{MemoryEntry, MemoryIndexLine, MemorySource, MemoryStore};
use crate::tools::progress;
use crate::tools::registry::{ToolImpact, ToolRegistry};

/// 注册 `read_pdf` 工具到 registry。
pub fn register_read_pdf(registry: &mut ToolRegistry) {
    registry.register_with_impact(
        "read_pdf",
        "解析 PDF 为 markdown，并以稳定的来源 slug + hash 保存。默认精度优先：MinerU API → arxiv-html（仅 arxiv）→ PyMuPDF。\
         传 `prefer_speed=true` 切换为速度优先。\
         **默认永久保存**：产物写到 `<dataDir>/artifacts/pdf/<source-slug>-<hash>/`。\
         传 `ephemeral=true` 改为一次性模式（落 `~/.pwcli/tmp/pdf/<hash>/`，24h 自动清理）；传 `output_dir` 指定自定义目录（不清理）。\
         图片资源用 `read_file` 按需查看（需视觉模型）。",
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "PDF 的 http(s) URL，或本地绝对路径（仅 PyMuPDF 兜底支持）"
                },
                "ephemeral": {
                    "type": "boolean",
                    "description": "可选。true 时落 ~/.pwcli/tmp/pdf/<hash>/，24h 自动清理；false（默认）保存到 dataDir/artifacts/pdf。",
                    "default": false
                },
                "output_dir": {
                    "type": "string",
                    "description": "可选。指定输出目录（绝对路径），不参与清理；优先级高于 ephemeral 与 Artifacts 默认。"
                },
                "prefer_speed": {
                    "type": "boolean",
                    "description": "可选。true 时切换为速度优先（arxiv-html → MinerU → PyMuPDF），适合 arxiv 论文快速预览。默认 false 走精度优先（MinerU 首选）。",
                    "default": false
                }
            },
            "required": ["url"]
        }),
        ToolImpact::ReversibleMutation,
        Box::new(move |args: &Value| {
            let url = args
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let output_dir = args
                .get("output_dir")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let prefer_speed = args
                .get("prefer_speed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let ephemeral = args
                .get("ephemeral")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Box::pin(async move {
                let url = url.ok_or_else(|| anyhow::anyhow!("url is required"))?;
                run_read_pdf(&url, output_dir, prefer_speed, ephemeral).await
            })
        }),
    );
}
/// 启动时调一次：清理 `~/.pwcli/tmp/pdf` 下 24h 过期目录。失败静默。
pub async fn cleanup_on_startup() {
    let _ = tmp_dir::cleanup_expired().await;
}

/// 解析 PDF 为 markdown，三级 fallback。返回正文、解析来源与图片数量。
pub(crate) struct PdfParseResult {
    pub markdown: String,
    pub source: String,
    pub image_count: usize,
}

/// 带 `prefer_speed` 控制的版本：true 时切换为旧顺序（速度优先）。
pub(crate) async fn parse_pdf_to_markdown_with_options(
    url: &str,
    output_dir: Option<PathBuf>,
    prefer_speed: bool,
) -> Result<PdfParseResult> {
    parse_pdf_to_markdown_impl(url, output_dir, prefer_speed).await
}

async fn parse_pdf_to_markdown_impl(
    url: &str,
    output_dir: Option<PathBuf>,
    prefer_speed: bool,
) -> Result<PdfParseResult> {
    // 懒清理过期临时目录
    let _ = tmp_dir::cleanup_expired().await;

    let out_dir: PathBuf = match output_dir {
        Some(d) => d,
        None => tmp_dir::tmp_dir_for_url(url),
    };
    tokio::fs::create_dir_all(&out_dir).await.ok();
    let _ = tmp_dir::write_meta(&out_dir, url);

    let mut errors: Vec<String> = Vec::new();

    // ─ Step: MinerU API ─
    async fn try_mineru(
        url: &str,
        out_dir: &Path,
        errors: &mut Vec<String>,
    ) -> Option<PdfParseResult> {
        match mineru::fetch_pdf_via_mineru(url, out_dir).await {
            Ok(md) => {
                let source = if crate::config::mineru::get_mineru_token().is_some() {
                    "mineru-precision"
                } else {
                    "mineru-agent"
                };
                let image_count = count_images(out_dir);
                Some(PdfParseResult {
                    markdown: md,
                    source: source.to_string(),
                    image_count,
                })
            }
            Err(e) => {
                progress::emit(&format!("↩ MinerU API 失败: {}", e));
                errors.push(format!("mineru-api: {}", e));
                None
            }
        }
    }

    // ─ Step: arxiv-html ─
    async fn try_arxiv_html(
        url: &str,
        out_dir: &Path,
        errors: &mut Vec<String>,
    ) -> Option<PdfParseResult> {
        match arxiv_html::try_fetch_arxiv_html(url, out_dir).await {
            Ok(md) => {
                let image_count = count_images(out_dir);
                Some(PdfParseResult {
                    markdown: md,
                    source: "arxiv-html".to_string(),
                    image_count,
                })
            }
            Err(e) => {
                progress::emit(&format!("↩ arxiv HTML 跳过: {}", e));
                errors.push(format!("arxiv-html: {}", e));
                None
            }
        }
    }

    // ─ Step: PyMuPDF ─
    async fn try_pymupdf(
        url: &str,
        out_dir: &Path,
        errors: &mut Vec<String>,
    ) -> Option<PdfParseResult> {
        match pymupdf::fetch_pdf_via_pymupdf(url, out_dir).await {
            Ok(md) => {
                let image_count = count_images(out_dir);
                Some(PdfParseResult {
                    markdown: md,
                    source: "pymupdf".to_string(),
                    image_count,
                })
            }
            Err(e) => {
                errors.push(format!("pymupdf: {}", e));
                None
            }
        }
    }

    if prefer_speed {
        // 速度优先：arxiv-html → MinerU API → PyMuPDF
        progress::emit("🔍 速度优先模式：尝试 arxiv-html 启发…");
        if let Some(r) = try_arxiv_html(url, &out_dir, &mut errors).await {
            return Ok(r);
        }
        progress::emit("⏭ 尝试 MinerU API…");
        if let Some(r) = try_mineru(url, &out_dir, &mut errors).await {
            return Ok(r);
        }
        progress::emit("⏭ 本地 PyMuPDF 兜底…");
        if let Some(r) = try_pymupdf(url, &out_dir, &mut errors).await {
            return Ok(r);
        }
    } else {
        // 精度优先（默认）：MinerU API → arxiv-html → PyMuPDF
        progress::emit("🔬 尝试 MinerU API…");
        if let Some(r) = try_mineru(url, &out_dir, &mut errors).await {
            return Ok(r);
        }
        progress::emit("⏭ MinerU 失败 → 尝试 arxiv-html 启发兜底…");
        if let Some(r) = try_arxiv_html(url, &out_dir, &mut errors).await {
            return Ok(r);
        }
        progress::emit("⏭ arxiv-html 失败 → 本地 PyMuPDF 兜底…");
        if let Some(r) = try_pymupdf(url, &out_dir, &mut errors).await {
            return Ok(r);
        }
    }

    anyhow::bail!(
        "read_pdf 三级 fallback 全部失败：\n  - {}",
        errors.join("\n  - ")
    )
}

// ────────────────────────────────────────────────────────────────────────────
// 输出模式决策
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum OutputMode {
    Artifact {
        artifact_id: String,
        dir: PathBuf,
        document: PathBuf,
    },
    Custom {
        dir: PathBuf,
    },
    Ephemeral {
        dir: PathBuf,
    },
}

fn normalized_source(source: &str) -> String {
    if source.starts_with("http://") || source.starts_with("https://") {
        return source.trim().to_string();
    }
    std::fs::canonicalize(source)
        .unwrap_or_else(|_| PathBuf::from(source))
        .to_string_lossy()
        .to_string()
}

fn artifact_id(source: &str) -> String {
    use sha2::{Digest, Sha256};

    let normalized = normalized_source(source);
    let slug = generate_source_id_from_url(&normalized);
    let digest = Sha256::digest(normalized.as_bytes());
    let hash = hex::encode(&digest[..4]);
    if slug.is_empty() {
        hash
    } else {
        format!("{slug}-{hash}")
    }
}

fn artifact_dir(data_dir: &Path, source: &str) -> PathBuf {
    data_dir
        .join("artifacts")
        .join("pdf")
        .join(artifact_id(source))
}

#[cfg(test)]
fn determine_output_with(args: &Value, source: &str, data_dir: &Path) -> (PathBuf, OutputMode) {
    if let Some(dir) = args.get("output_dir").and_then(|value| value.as_str()) {
        let dir = PathBuf::from(dir);
        return (dir.clone(), OutputMode::Custom { dir });
    }
    if args
        .get("ephemeral")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        let dir = tmp_dir::tmp_dir_for_url(source);
        return (dir.clone(), OutputMode::Ephemeral { dir });
    }

    let artifact_id = artifact_id(source);
    let dir = data_dir.join("artifacts").join("pdf").join(&artifact_id);
    let document = dir.join("document.md");
    (
        dir.clone(),
        OutputMode::Artifact {
            artifact_id,
            dir,
            document,
        },
    )
}

/// 从已解析的 markdown 中提取论文标题。
/// 策略：前 50 行中第一个 `# ` 行，或第一个 `**...**` 独占行。
pub(crate) fn extract_paper_title(markdown: &str) -> Option<String> {
    for line in markdown.lines().take(50) {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            let title = heading.trim();
            if !title.is_empty() && title.len() > 3 {
                return Some(title.to_string());
            }
        }
    }
    for line in markdown.lines().take(50) {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Title:") {
            let title = value.trim();
            if !title.is_empty() && title.len() > 3 {
                return Some(title.to_string());
            }
        }
    }
    for line in markdown.lines().take(50) {
        let trimmed = line.trim();
        if trimmed.starts_with("**") && trimmed.ends_with("**") && trimmed.len() > 6 {
            let inner = &trimmed[2..trimmed.len() - 2];
            if !inner.is_empty() {
                return Some(inner.to_string());
            }
        }
    }
    None
}

/// 将论文标题转为文件系统安全的 source_id。
/// 保留 CJK 字符，英文小写，空格/特殊符号 → `-`，截断 60 字符。
pub(crate) fn title_to_source_id(title: &str) -> String {
    let mut result = String::with_capacity(title.len());
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
        } else if !ch.is_ascii() && ch.is_alphanumeric() {
            // CJK and other non-ASCII letters/digits: keep as-is
            result.push(ch);
        } else {
            result.push('-');
        }
    }
    // Collapse consecutive dashes, trim edges
    let collapsed: String = result
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    collapsed.chars().take(60).collect()
}

/// 从 URL 或文件路径生成稳定的 source slug。
fn generate_source_id_from_url(url: &str) -> String {
    // 本地文件路径：用文件名 stem
    if !url.starts_with("http://") && !url.starts_with("https://") {
        let path = std::path::Path::new(url);
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let id = title_to_source_id(stem);
            if !id.is_empty() {
                return id;
            }
        }
    }
    // arxiv URL
    if let Some(captures) = arxiv_pdf_regex().captures(url.trim()) {
        if let Some(id) = captures.get(1) {
            return format!("arxiv_{}", id.as_str());
        }
    }
    // Generic URL → slug
    let slug = url_to_slug(url);
    if slug.is_empty() {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(url.as_bytes());
        hex::encode(&hash[..4])
    } else {
        slug
    }
}

/// 提取 URL 末段、去扩展名、非字母数字替换 `-`、左右剥 `-`、截 40 字符。
fn url_to_slug(url: &str) -> String {
    // 去掉 query / hash 再切末段
    let core = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let last = core.rsplit('/').next().unwrap_or("");
    let stem = last.rsplit_once('.').map(|(s, _)| s).unwrap_or(last);
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').chars().take(40).collect()
}

/// 仅在 source_id 生成中使用的 arxiv pdf URL 正则；与 `arxiv_html::extract_arxiv_id`
/// 保持等价（兼容 `.pdf` 后缀 / 版本号 / query / fragment）。
fn arxiv_pdf_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^https?://arxiv\.org/pdf/([^/\s?#]+?)(\.pdf)?(?:\?.*)?(?:#.*)?$")
            .unwrap()
    })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn remember_pdf_artifact_in_store(
    store: &MemoryStore,
    title: &str,
    url: &str,
    artifact_id: &str,
    document: &Path,
    parser: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let slug = format!("paper-{artifact_id}");
    let summary = truncate_chars(&format!("已解析论文《{title}》｜{url}"), 150);
    let content = format!(
        "# 已解析论文\n\n\
         - 标题：{title}\n\
         - 原始地址：{url}\n\
         - artifact_id：{artifact_id}\n\
         - 正文路径：{}\n\
         - 解析器：{parser}\n\
         - 最近解析时间：{}\n",
        document.display(),
        chrono::Utc::now().to_rfc3339(),
    );

    let mut entry = store.read_entry_raw(&slug).unwrap_or_else(|_| {
        MemoryEntry::new_active(slug.clone(), summary.clone(), content.clone(), now, None)
    });
    entry.summary = summary.clone();
    entry.content = content;
    entry.updated_at = now;
    entry.deleted_at = None;
    entry.kind = "knowledge".to_string();
    entry.tags = vec![
        "paper".to_string(),
        "pdf".to_string(),
        "artifact-index".to_string(),
    ];
    entry.sources = vec![MemorySource {
        kind: "pdf".to_string(),
        uri: url.to_string(),
        title: Some(title.to_string()),
        content_hash: None,
        captured_at: Some(now),
    }];
    entry.ensure_id();

    store.write_entry_atomic(&entry)?;
    store.upsert_index_line(&MemoryIndexLine {
        slug,
        summary,
        updated_at: now,
    })?;
    crate::memory::sync_index::best_effort_upsert(store, &entry);
    Ok(())
}

fn remember_pdf_artifact(
    title: &str,
    url: &str,
    artifact_id: &str,
    document: &Path,
    parser: &str,
) -> Result<()> {
    let config = crate::config::RuntimeConfig::load();
    let user_slug = config
        .user
        .and_then(|user| user.slug)
        .unwrap_or_else(|| "local".to_string());
    let store = MemoryStore::new(&user_slug)?;
    remember_pdf_artifact_in_store(&store, title, url, artifact_id, document, parser)
}

fn frontmatter_value(markdown: &str, key: &str) -> Option<String> {
    let mut lines = markdown.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let prefix = format!("{key}:");
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(value) = line.trim().strip_prefix(&prefix) {
            let value = value.trim();
            return serde_json::from_str::<String>(value)
                .ok()
                .or_else(|| Some(value.trim_matches('"').to_string()));
        }
    }
    None
}

/// 为升级前已经落盘的 PDF Artifact 补建轻量 Memory 索引。
/// 已存在同 artifact_id 的记忆会跳过，避免每次启动刷新时间。
pub fn backfill_pdf_artifact_memories() -> usize {
    let config = crate::config::RuntimeConfig::load();
    let user_slug = config
        .user
        .and_then(|user| user.slug)
        .unwrap_or_else(|| "local".to_string());
    let Ok(store) = MemoryStore::new(&user_slug) else {
        return 0;
    };
    let root = crate::config::local_config::data_dir()
        .join("artifacts")
        .join("pdf");
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };

    let mut indexed = 0;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let artifact_id = entry.file_name().to_string_lossy().to_string();
        let document = entry.path().join("document.md");
        let Ok(markdown) = std::fs::read_to_string(&document) else {
            continue;
        };
        let Some(url) = frontmatter_value(&markdown, "source") else {
            continue;
        };
        let frontmatter_title =
            frontmatter_value(&markdown, "title").unwrap_or_else(|| artifact_id.clone());
        let title = if frontmatter_title == artifact_id {
            extract_paper_title(&markdown).unwrap_or(frontmatter_title)
        } else {
            frontmatter_title
        };
        if let Ok(existing) = store.read_entry_raw(&format!("paper-{artifact_id}")) {
            let placeholder = format!("- 标题：{artifact_id}");
            if title == artifact_id || !existing.content.contains(&placeholder) {
                continue;
            }
        }
        let parser = frontmatter_value(&markdown, "parser").unwrap_or_else(|| "unknown".into());
        if remember_pdf_artifact_in_store(&store, &title, &url, &artifact_id, &document, &parser)
            .is_ok()
        {
            indexed += 1;
        }
    }
    indexed
}

// ────────────────────────────────────────────────────────────────────────────
// Handler 主流程
// ────────────────────────────────────────────────────────────────────────────

async fn run_read_pdf(
    url: &str,
    output_dir: Option<String>,
    prefer_speed: bool,
    ephemeral: bool,
) -> Result<String> {
    if let Some(dir) = output_dir.as_ref() {
        let out = PathBuf::from(dir);
        let result =
            parse_pdf_to_markdown_with_options(url, Some(out.clone()), prefer_speed).await?;
        let mode = OutputMode::Custom { dir: out };
        return Ok(format!(
            "{}{}",
            result.markdown,
            build_footer(&mode, &result)
        ));
    }

    if ephemeral {
        let dir = tmp_dir::tmp_dir_for_url(url);
        let result =
            parse_pdf_to_markdown_with_options(url, Some(dir.clone()), prefer_speed).await?;
        let mode = OutputMode::Ephemeral { dir };
        return Ok(format!(
            "{}{}",
            result.markdown,
            build_footer(&mode, &result)
        ));
    }

    let final_dir = artifact_dir(&crate::config::local_config::data_dir(), url);
    let artifact_id = artifact_id(url);
    let parent = final_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid artifact directory"))?;
    tokio::fs::create_dir_all(parent).await?;
    let staging = parent.join(format!(".{artifact_id}.tmp-{}", std::process::id()));
    if staging.exists() {
        tokio::fs::remove_dir_all(&staging).await?;
    }

    let result =
        match parse_pdf_to_markdown_with_options(url, Some(staging.clone()), prefer_speed).await {
            Ok(result) => result,
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(error);
            }
        };

    let title = extract_paper_title(&result.markdown).unwrap_or_else(|| artifact_id.clone());
    let frontmatter = format!(
        "---\ntitle: \"{}\"\nsource: \"{}\"\nartifact_id: \"{}\"\nfetched_at: \"{}\"\nparser: \"{}\"\n---\n\n",
        title.replace('"', "\\\""),
        url.replace('"', "\\\""),
        artifact_id,
        chrono::Utc::now().to_rfc3339(),
        result.source.replace('"', "\\\""),
    );
    if let Err(error) = tokio::fs::write(
        staging.join("document.md"),
        format!("{frontmatter}{}", result.markdown),
    )
    .await
    {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(error.into());
    }
    let _ = tokio::fs::remove_file(staging.join(".meta.json")).await;
    replace_artifact_dir(&staging, &final_dir).await?;

    let final_result = PdfParseResult {
        markdown: result.markdown,
        source: result.source,
        image_count: count_images(&final_dir),
    };
    let mode = OutputMode::Artifact {
        artifact_id: artifact_id.clone(),
        document: final_dir.join("document.md"),
        dir: final_dir,
    };
    if let OutputMode::Artifact { document, .. } = &mode {
        match remember_pdf_artifact(&title, url, &artifact_id, document, &final_result.source) {
            Ok(()) => progress::emit("🧠 已记录论文索引到长期记忆"),
            Err(error) => warn!(
                artifact_id = %artifact_id,
                error = %error,
                "记录 PDF 长期记忆索引失败"
            ),
        }
    }
    Ok(format!(
        "{}{}",
        final_result.markdown,
        build_footer(&mode, &final_result)
    ))
}

async fn replace_artifact_dir(staging: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid artifact target"))?;
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact");
    let backup = parent.join(format!(".{name}.backup-{}", std::process::id()));
    if backup.exists() {
        tokio::fs::remove_dir_all(&backup).await?;
    }
    let had_previous = target.exists();
    if had_previous {
        tokio::fs::rename(target, &backup).await?;
    }
    if let Err(error) = tokio::fs::rename(staging, target).await {
        if had_previous {
            let _ = tokio::fs::rename(&backup, target).await;
        }
        return Err(error.into());
    }
    if had_previous {
        let _ = tokio::fs::remove_dir_all(backup).await;
    }
    Ok(())
}

fn build_footer(mode: &OutputMode, result: &PdfParseResult) -> String {
    match mode {
        OutputMode::Artifact {
            artifact_id,
            dir,
            document,
        } => format!(
            "\n\n---\n[✅ 已保存到资料库 | 来源: {src} | artifact_id: {id}]\n\
             📄 文档: {document}\n\
             📎 图片: {dir}/images/（{n} 张）",
            src = result.source,
            id = artifact_id,
            document = document.display(),
            dir = dir.display(),
            n = result.image_count,
        ),
        OutputMode::Ephemeral { dir } => format!(
            "\n\n---\n[⏰ 临时模式 | 来源: {src} | 资源目录: {dir} | 24h 后自动清理]\n\
             如需查看图片，调 read_file 路径 {dir}/images/<filename>（需视觉模型）。",
            src = result.source,
            dir = dir.display(),
        ),
        OutputMode::Custom { dir } => format!(
            "\n\n---\n[📁 自定义目录 | 来源: {src} | 目录: {dir} | 不会自动清理]",
            src = result.source,
            dir = dir.display(),
        ),
    }
}

fn count_images(out_dir: &std::path::Path) -> usize {
    let imgs = out_dir.join("images");
    let entries = match std::fs::read_dir(&imgs) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::ToolRegistry;

    fn fake_result(source: &str, image_count: usize) -> PdfParseResult {
        PdfParseResult {
            markdown: "BODY".into(),
            source: source.into(),
            image_count,
        }
    }

    #[test]
    fn description_exposes_precision_and_artifact_defaults() {
        let mut registry = ToolRegistry::new();
        register_read_pdf(&mut registry);
        let pdf = registry
            .get_definition("read_pdf")
            .expect("read_pdf should be registered");
        assert!(pdf.description.contains("精度优先"));
        assert!(pdf.description.contains("artifacts/pdf"));
        assert!(!pdf.description.to_lowercase().contains("wiki"));
        assert_eq!(
            pdf.parameters
                .pointer("/properties/ephemeral/default")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn artifact_id_is_stable_and_source_specific() {
        let source = "https://example.com/papers/cool-paper.pdf";
        let first = artifact_id(source);
        assert_eq!(first, artifact_id(source));
        assert!(first.starts_with("cool-paper-"));
        assert_ne!(
            first,
            artifact_id("https://mirror.example.com/cool-paper.pdf")
        );
    }

    #[test]
    fn remembered_pdf_stores_metadata_without_copying_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new_with_dir(dir.path().join("memory")).unwrap();
        let document = dir.path().join("artifacts/pdf/paper/document.md");

        remember_pdf_artifact_in_store(
            &store,
            "A Useful Paper",
            "https://example.com/paper.pdf",
            "paper-abcd1234",
            &document,
            "mineru-precision",
        )
        .unwrap();

        let entry = store.read_entry("paper-paper-abcd1234").unwrap();
        assert!(entry.content.contains("https://example.com/paper.pdf"));
        assert!(entry.content.contains(document.to_string_lossy().as_ref()));
        assert!(!entry.content.contains("论文正文内容"));
        assert_eq!(entry.tags, ["paper", "pdf", "artifact-index"]);
        assert_eq!(entry.sources[0].uri, "https://example.com/paper.pdf");
        assert!(store
            .read_index()
            .unwrap()
            .entries
            .iter()
            .any(|line| line.slug == "paper-paper-abcd1234"));
    }

    #[test]
    fn reads_pdf_artifact_frontmatter_fields() {
        let markdown = r#"---
title: "A Useful Paper"
source: "https://example.com/paper.pdf"
parser: "mineru-precision"
---

Body
"#;
        assert_eq!(
            frontmatter_value(markdown, "title").as_deref(),
            Some("A Useful Paper")
        );
        assert_eq!(
            frontmatter_value(markdown, "source").as_deref(),
            Some("https://example.com/paper.pdf")
        );
        assert_eq!(frontmatter_value(markdown, "missing"), None);
    }

    #[test]
    fn determine_output_respects_precedence() {
        let data_dir = Path::new("/tmp/pwcli-data");
        let source = "https://example.com/paper.pdf";

        let (custom_path, custom) =
            determine_output_with(&json!({"output_dir": "/tmp/custom"}), source, data_dir);
        assert_eq!(custom_path, PathBuf::from("/tmp/custom"));
        assert!(matches!(custom, OutputMode::Custom { .. }));

        let (temporary_path, temporary) =
            determine_output_with(&json!({"ephemeral": true}), source, data_dir);
        assert_eq!(temporary_path, tmp_dir::tmp_dir_for_url(source));
        assert!(matches!(temporary, OutputMode::Ephemeral { .. }));

        let (artifact_path, artifact) = determine_output_with(&json!({}), source, data_dir);
        assert!(artifact_path.starts_with("/tmp/pwcli-data/artifacts/pdf"));
        match artifact {
            OutputMode::Artifact { document, .. } => {
                assert_eq!(document, artifact_path.join("document.md"));
            }
            other => panic!("expected Artifact, got {other:?}"),
        }
    }

    #[test]
    fn extracts_titles_and_builds_safe_slugs() {
        assert_eq!(
            extract_paper_title("# Attention Is All You Need\n\nAbstract"),
            Some("Attention Is All You Need".to_string())
        );
        assert_eq!(
            extract_paper_title(
                "Title: Generative Auto-Bidding with Value-Guided Explorations\n\nAbstract"
            ),
            Some("Generative Auto-Bidding with Value-Guided Explorations".to_string())
        );
        assert_eq!(
            title_to_source_id("MinerU: 高精度文档解析引擎"),
            "mineru-高精度文档解析引擎"
        );
    }

    #[test]
    fn artifact_footer_reports_document_and_images() {
        let dir = PathBuf::from("/tmp/data/artifacts/pdf/paper-abcd1234");
        let mode = OutputMode::Artifact {
            artifact_id: "paper-abcd1234".into(),
            document: dir.join("document.md"),
            dir: dir.clone(),
        };
        let footer = build_footer(&mode, &fake_result("pymupdf", 3));
        assert!(footer.contains("paper-abcd1234"));
        assert!(footer.contains("document.md"));
        assert!(footer.contains("images/（3 张）"));
        assert!(!footer.to_lowercase().contains("wiki"));
    }

    #[tokio::test]
    async fn replacing_artifact_removes_stale_files() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("artifact");
        let staging = temp.path().join("staging");
        tokio::fs::create_dir_all(&target).await.unwrap();
        tokio::fs::write(target.join("stale.txt"), "old")
            .await
            .unwrap();
        tokio::fs::create_dir_all(&staging).await.unwrap();
        tokio::fs::write(staging.join("document.md"), "new")
            .await
            .unwrap();

        replace_artifact_dir(&staging, &target).await.unwrap();

        assert!(!target.join("stale.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(target.join("document.md"))
                .await
                .unwrap(),
            "new"
        );
    }
}
