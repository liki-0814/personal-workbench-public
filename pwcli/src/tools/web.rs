use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::tools::progress;
use crate::tools::registry::{ToolExecutionMode, ToolImpact, ToolOutput, ToolRegistry};
use crate::tools::web_cache::WebCacheKey;
use crate::tools::web_context;

const READ_LIMIT: usize = 12000;
const BATCH_MAX_URLS: usize = 10;
const QUERY_LIMIT: usize = 16000;
const CACHE_HIT_NOTE: &str = "\n\n[会话缓存命中]";
const WEB_DOWNLOAD_MAX_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn resolve_web_proxy() -> Option<String> {
    std::env::var("WEB_PROXY")
        .or_else(|_| std::env::var("HTTPS_PROXY"))
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("ALL_PROXY"))
        .or_else(|_| std::env::var("all_proxy"))
        .ok()
        .filter(|v| !v.is_empty())
}

/// Web profile 只保留当前配置指纹；代理热更新后旧 client 随在途请求自然释放。
pub(crate) fn shared_http_client() -> anyhow::Result<reqwest::Client> {
    crate::http_client::client(
        crate::http_client::ClientProfile::Web,
        crate::http_client::ClientFingerprint {
            timeout_secs: Duration::from_secs(60).as_secs(),
            proxy: resolve_web_proxy(),
            ..Default::default()
        },
    )
}

fn paginate_chars(content: &str, offset: usize, limit: usize) -> (String, Option<String>) {
    let total_chars = content.chars().count();
    let page: String = content.chars().skip(offset).take(limit).collect();
    let taken = page.chars().count();
    let end = offset + taken;

    let suffix = if end < total_chars {
        Some(format!(
            "\n\n[...显示第 {}-{} 字，共约 {} 字。可传 offset={} 获取下一段]",
            offset, end, total_chars, end
        ))
    } else if offset > 0 && taken == 0 {
        Some(format!(
            "\n\n[...offset={} 超出内容长度（共约 {} 字）]",
            offset, total_chars
        ))
    } else {
        None
    };
    (page, suffix)
}

/// 从头截取并附加截断提示。
fn truncate_with_prefix_hint(content: &str, limit: usize, label: &str) -> String {
    let total_chars = content.chars().count();
    if total_chars <= limit {
        return content.to_string();
    }
    let truncated: String = content.chars().take(limit).collect();
    format!(
        "{}\n\n[...{}，显示前 {} 字，共约 {} 字]",
        truncated, label, limit, total_chars
    )
}

async fn try_cache_get(
    mode: &str,
    url: &str,
    offset: usize,
    selector: Option<&str>,
) -> Option<String> {
    let cache = web_context::web_cache()?;
    let sid = web_context::session_id()?;
    let key = WebCacheKey::new(sid, mode, url, offset, selector.map(|s| s.to_string()));
    cache.get(&key).await
}

async fn cache_put(mode: &str, url: &str, offset: usize, selector: Option<&str>, content: String) {
    if let (Some(cache), Some(sid)) = (web_context::web_cache(), web_context::session_id()) {
        let key = WebCacheKey::new(sid, mode, url, offset, selector.map(|s| s.to_string()));
        cache.put(key, content).await;
    }
}

fn with_cache_note(content: String, cached: bool) -> String {
    if cached {
        format!("{}{}", content, CACHE_HIT_NOTE)
    } else {
        content
    }
}

async fn cached_fetch_read(url: &str, offset: usize) -> anyhow::Result<String> {
    cached_fetch_read_with_limit(url, offset, READ_LIMIT).await
}

pub(crate) async fn cached_fetch_read_with_limit(
    url: &str,
    offset: usize,
    char_limit: usize,
) -> anyhow::Result<String> {
    let cache_tag = format!("cl={}", char_limit);
    let content = if let Some(hit) = try_cache_get("read", url, offset, Some(&cache_tag)).await {
        with_cache_note(hit, true)
    } else {
        let content = crate::tools::anysearch::extract(url).await?;
        let fresh = if offset == 0 {
            truncate_with_prefix_hint(&content, char_limit, "内容已截断")
        } else {
            let (page, suffix) = paginate_chars(&content, offset, char_limit);
            suffix.map_or(page.clone(), |suffix| format!("{page}{suffix}"))
        };
        cache_put("read", url, offset, Some(&cache_tag), fresh.clone()).await;
        fresh
    };
    Ok(content)
}

fn validate_public_http_url(input: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(input).context("无效 URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        anyhow::bail!("只支持不含凭据的 http/https URL");
    }
    let host = url.host_str().context("URL 缺少 host")?;
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| !crate::media::is_public_ip(ip))
    {
        anyhow::bail!("拒绝访问本机或私有网络地址");
    }
    Ok(())
}

async fn fetch_public_response(url: &str) -> anyhow::Result<reqwest::Response> {
    validate_public_http_url(url)?;
    let mut current = reqwest::Url::parse(url).context("无效 URL")?;
    for redirect_count in 0..=3 {
        let (client, checked_url) = crate::media::restricted_client(&current, false)
            .await
            .map_err(anyhow::Error::msg)?;
        let response = client
            .get(checked_url.clone())
            .header(
                "User-Agent",
                "Mozilla/5.0 (compatible; PersonalWorkbench/1.0)",
            )
            .send()
            .await
            .context("下载公开数据文件失败")?;
        if response.status().is_redirection() {
            if redirect_count == 3 {
                anyhow::bail!("公开数据文件重定向次数过多");
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .context("公开数据文件重定向缺少有效位置")?;
            current = checked_url
                .join(location)
                .context("公开数据文件重定向 URL 无效")?;
            continue;
        }
        return response
            .error_for_status()
            .context("公开数据文件返回错误状态");
    }
    anyhow::bail!("无法下载公开数据文件")
}

async fn download_public_data_file(
    url: &str,
    suggested_name: Option<&str>,
) -> anyhow::Result<Value> {
    let response = fetch_public_response(url).await?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_ascii_lowercase();
    if let Some(length) = response.content_length() {
        if length > WEB_DOWNLOAD_MAX_BYTES as u64 {
            anyhow::bail!("下载文件过大（{} bytes）", length);
        }
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("读取公开数据文件失败")?;
        if bytes.len().saturating_add(chunk.len()) > WEB_DOWNLOAD_MAX_BYTES {
            anyhow::bail!("下载文件过大（超过 {} bytes）", WEB_DOWNLOAD_MAX_BYTES);
        }
        bytes.extend_from_slice(&chunk);
    }
    let filename = safe_download_filename(url, suggested_name, &content_type)?;
    let digest = Sha256::digest(&bytes);
    let sha256 = hex::encode(digest);
    let directory = crate::config::local_config::data_dir()
        .join("artifacts")
        .join("web-downloads")
        .join(&sha256[..16]);
    std::fs::create_dir_all(&directory).context("创建公开数据下载目录失败")?;
    let path = directory.join(&filename);
    if !path.exists() {
        let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&temporary, &bytes).context("写入公开数据临时文件失败")?;
        std::fs::rename(&temporary, &path).context("原子保存公开数据文件失败")?;
    }
    Ok(json!({
        "url": url,
        "path": path,
        "filename": filename,
        "contentType": content_type,
        "bytes": bytes.len(),
        "sha256": sha256,
        "usageHint": "后续工具必须使用 path 的完整绝对路径；不要假设 shell 当前目录，也不要只传 filename。",
    }))
}

fn safe_download_filename(
    url: &str,
    suggested_name: Option<&str>,
    content_type: &str,
) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(url).context("无效下载 URL")?;
    let from_url = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty());
    let raw = suggested_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or(from_url)
        .unwrap_or("download");
    let mut filename = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(120)
        .collect::<String>();
    if filename.is_empty() || filename == "." || filename == ".." {
        filename = "download".into();
    }
    if !filename.contains('.') {
        if let Some(extension) = extension_for_content_type(content_type) {
            filename.push('.');
            filename.push_str(extension);
        }
    }
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    const ALLOWED_EXTENSIONS: &[&str] = &[
        "csv", "tsv", "json", "xml", "txt", "html", "htm", "xlsx", "xls", "pdf", "zip", "parquet",
    ];
    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        anyhow::bail!(
            "不支持的公开数据文件扩展名 `{extension}`；允许 {}",
            ALLOWED_EXTENSIONS.join(", ")
        );
    }
    Ok(filename)
}

fn extension_for_content_type(content_type: &str) -> Option<&'static str> {
    match content_type {
        "text/csv" => Some("csv"),
        "text/tab-separated-values" => Some("tsv"),
        "application/json" => Some("json"),
        "application/xml" | "text/xml" => Some("xml"),
        "text/plain" => Some("txt"),
        "text/html" => Some("html"),
        "application/pdf" => Some("pdf"),
        "application/zip" | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("xlsx")
        }
        "application/vnd.ms-excel" => Some("xls"),
        "application/vnd.apache.parquet" => Some("parquet"),
        _ => None,
    }
}

fn query_cache_key(query: &str, num: u8, site: Option<&str>) -> String {
    format!("{}|num={}|site={}", query, num, site.unwrap_or(""))
}

pub(crate) async fn cached_fetch_query(
    query: &str,
    num: u8,
    site: Option<&str>,
) -> anyhow::Result<String> {
    let cache_key = query_cache_key(query, num, site);
    let content = if let Some(hit) = try_cache_get("query", &cache_key, 0, None).await {
        with_cache_note(hit, true)
    } else {
        let fresh =
            crate::tools::anysearch::search_markdown(query, num as usize, site, None).await?;
        let fresh = truncate_with_prefix_hint(&fresh, QUERY_LIMIT, "搜索结果已截断");
        cache_put("query", &cache_key, 0, None, fresh.clone()).await;
        fresh
    };
    Ok(content)
}

/// 单 URL hydrate 超时（秒）。失败的 URL 不报错，占位提示。
const HYDRATE_PER_URL_TIMEOUT_SECS: u64 = 20;

fn hydrate_cache_tag(hydrate: u8, char_limit: usize) -> String {
    format!("hyd={}|cl={}", hydrate, char_limit)
}

/// hydrate 版搜索：搜完后并发抽取 top-N 命中的正文，嵌入每条结果下方。
/// - hydrate 已被调用方 clamp 到 1-5
/// - char_limit 控制每个 hydrated 页字符上限
/// - 失败的页不报错，标 `[hydrate failed: ...]`
pub(crate) async fn cached_fetch_query_with_hydrate(
    query: &str,
    num: u8,
    site: Option<&str>,
    hydrate: u8,
    char_limit: usize,
) -> anyhow::Result<String> {
    let cache_key = query_cache_key(query, num, site);
    let cache_tag = hydrate_cache_tag(hydrate, char_limit);
    if let Some(hit) = try_cache_get("query", &cache_key, 0, Some(&cache_tag)).await {
        return Ok(with_cache_note(hit, true));
    }

    let items = crate::tools::anysearch::search_items(query, num as usize, site, None).await?;
    let take_n = (hydrate as usize).min(items.len());

    let hydrated: Vec<Option<String>> = if take_n == 0 {
        Vec::new()
    } else {
        progress::emit(&format!("📰 抽取 {} 个结果正文...", take_n));
        let cache = web_context::web_cache();
        let sid = web_context::session_id();
        let mut handles = Vec::with_capacity(take_n);
        for item in items.iter().take(take_n) {
            let url = item.url.clone();
            let cache = cache.clone();
            let sid = sid.clone();
            handles.push(tokio::spawn(async move {
                let fut = async {
                    let timeout_fut = tokio::time::timeout(
                        Duration::from_secs(HYDRATE_PER_URL_TIMEOUT_SECS),
                        cached_fetch_read_with_limit(&url, 0, char_limit),
                    );
                    match timeout_fut.await {
                        Ok(Ok(text)) => Some(text),
                        Ok(Err(e)) => Some(format!("[hydrate failed: {}]", e)),
                        Err(_) => Some(format!(
                            "[hydrate failed: timeout {}s]",
                            HYDRATE_PER_URL_TIMEOUT_SECS
                        )),
                    }
                };
                if cache.is_some() || sid.is_some() {
                    web_context::with_web_context(cache, sid, fut).await
                } else {
                    fut.await
                }
            }));
        }
        let mut out = Vec::with_capacity(take_n);
        for h in handles {
            out.push(
                h.await
                    .unwrap_or_else(|e| Some(format!("[hydrate failed: task panic: {}]", e))),
            );
        }
        out
    };

    let mut buf = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            buf.push_str("\n---\n\n");
        }
        let title = if item.title.is_empty() {
            "(无标题)"
        } else {
            item.title.as_str()
        };
        buf.push_str(&format!("### {}. {}\nURL: {}\n", i + 1, title, item.url));
        if !item.description.is_empty() {
            buf.push_str(&format!("{}\n", item.description));
        }
        if i < take_n {
            if let Some(Some(body)) = hydrated.get(i) {
                let char_count = body.chars().count();
                buf.push_str(&format!(
                    "\n<details>\n<summary>正文摘要 ({} chars)</summary>\n\n{}\n\n</details>\n",
                    char_count, body
                ));
            }
        }
    }
    if items.is_empty() {
        buf = "（无搜索结果）".to_string();
    }

    let truncated = truncate_with_prefix_hint(&buf, QUERY_LIMIT, "搜索结果已截断");
    cache_put("query", &cache_key, 0, Some(&cache_tag), truncated.clone()).await;
    Ok(truncated)
}

/// 2-5 条查询并行调用 AnySearch；单条失败不阻塞其余。
pub(crate) async fn batch_fetch_queries(
    queries: Vec<String>,
    num: u8,
    site: Option<&str>,
    hydrate: u8,
    hydrate_char_limit: usize,
) -> String {
    let total = queries.len();
    let cache = web_context::web_cache();
    let sid = web_context::session_id();
    let mut handles = Vec::with_capacity(total);
    for (i, q) in queries.into_iter().enumerate() {
        let site_owned = site.map(|s| s.to_string());
        let cache = cache.clone();
        let sid = sid.clone();
        handles.push(tokio::spawn(async move {
            let label = format!("[{}/{}] {}", i + 1, total, q);
            let fut = async {
                let res = if hydrate > 0 {
                    cached_fetch_query_with_hydrate(
                        &q,
                        num,
                        site_owned.as_deref(),
                        hydrate,
                        hydrate_char_limit,
                    )
                    .await
                } else {
                    cached_fetch_query(&q, num, site_owned.as_deref()).await
                };
                match res {
                    Ok(body) => format!("## 搜索 {label}\n\n{body}"),
                    Err(e) => format!("## 搜索 {label}\n\n❌ {e}"),
                }
            };
            if cache.is_some() || sid.is_some() {
                web_context::with_web_context(cache, sid, fut).await
            } else {
                fut.await
            }
        }));
    }
    let mut sections = Vec::with_capacity(total);
    for h in handles {
        sections.push(match h.await {
            Ok(s) => s,
            Err(e) => format!("❌ 任务异常: {e}"),
        });
    }
    sections.join("\n\n---\n\n")
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        "web_read",
        "通过 AnySearch 抽取公开 URL 的正文并返回 Markdown；搜索请使用 web_query。\
         适用场景：文章、博客、文档、PDF 等公开且以正文为核心的页面。\
         返回 Markdown（默认每页 12000 字，可用 offset 分页续读）。\
         同一会话内重复读取会命中缓存；不支持登录态页面或页面交互。\
         长文阅读如论文 HTML 可把 char_limit 调到 50000-100000 一次拿全。",
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要由 AnySearch 抽取的公开 URL"
                },
                "offset": {
                    "type": "integer",
                    "description": "内容起始字符偏移（默认 0），用于分页续读长页面"
                },
                "char_limit": {
                    "type": "integer",
                    "description": "单页返回字符上限（默认 12000，长文阅读如论文 HTML 可调到 50000-100000，最大 200000）",
                    "minimum": 500,
                    "maximum": 200000
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
        Box::new(move |args: &Value| {
            let url = args["url"].as_str().unwrap_or("").to_string();
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let char_limit = args
                .get("char_limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(READ_LIMIT)
                .clamp(500, 200_000);
            Box::pin(async move {
                if url.is_empty() {
                    anyhow::bail!("url is required");
                }
                progress::emit(&format!("🔍 读取: {}", &url[..url.len().min(60)]));
                cached_fetch_read_with_limit(&url, offset, char_limit).await
            })
        }),
    );

    registry.register_structured_with_impact(
        "download_web_file",
        "把已知公开 URL 的二进制数据或资料文件受控下载到 pwcli artifacts，供后续数据分析或文档取证。适合 XLSX、XLS、PDF、ZIP、Parquet，以及需要保留原文件的 CSV/JSON；纯文本正文仍优先 web_read。返回可供 read_file、read_pdf 或 run_command 分析的本地绝对路径、MIME、大小和 SHA-256。只接受 http/https，拒绝本机/私网地址，单文件上限 32 MiB。",
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "已经从可信页面或用户材料中获得的公开文件 URL"
                },
                "filename": {
                    "type": "string",
                    "description": "可选安全文件名；仅用于 URL 没有明确扩展名时"
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
        ToolExecutionMode::Sequential,
        ToolImpact::ReversibleMutation,
        Box::new(|args: &Value| {
            let url = args["url"].as_str().unwrap_or("").to_string();
            let filename = args["filename"].as_str().map(str::to_string);
            Box::pin(async move {
                if url.is_empty() {
                    anyhow::bail!("url is required");
                }
                progress::emit(&format!(
                    "⬇️ 下载公开数据: {}",
                    url.chars().take(80).collect::<String>()
                ));
                let result = download_public_data_file(&url, filename.as_deref()).await?;
                Ok(ToolOutput::text(serde_json::to_string_pretty(&result)?))
            })
        }),
    );

    registry.register(
        "web_search_domains",
        "发现 AnySearch 垂直检索能力。只在问题明确属于某个垂直领域、且尚不知道正确 sub_domain 或必填参数时调用；普通网页搜索不要调用。\
         返回后从中选择一个 sub_domain，并把所有标记 required 的参数原样填入 web_query.sub_domain_params。一次可查询 1-5 个领域。",
        json!({
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "enum": ["general", "resource", "social_media", "finance", "academic", "legal", "health", "business", "security", "ip", "code", "energy", "environment", "agriculture", "travel", "film", "gaming"],
                    "description": "单个 AnySearch 顶级领域（与 domains 二选一）"
                },
                "domains": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["general", "resource", "social_media", "finance", "academic", "legal", "health", "business", "security", "ip", "code", "energy", "environment", "agriculture", "travel", "film", "gaming"]
                    },
                    "minItems": 1,
                    "maxItems": 5,
                    "uniqueItems": true,
                    "description": "多个 AnySearch 顶级领域（与 domain 二选一）"
                }
            },
            "oneOf": [
                { "required": ["domain"] },
                { "required": ["domains"] }
            ],
            "additionalProperties": false
        }),
        Box::new(|args: &Value| {
            let domain = args["domain"].as_str().unwrap_or("").trim().to_string();
            let domains = args["domains"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|domain| !domain.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Box::pin(async move {
                if !domain.is_empty() && !domains.is_empty() {
                    anyhow::bail!("domain 与 domains 不能同时传");
                }
                let domains = if domain.is_empty() {
                    domains
                } else {
                    vec![domain]
                };
                crate::tools::anysearch::get_sub_domains(&domains).await
            })
        }),
    );

    registry.register(
        "web_query",
        "唯一联网搜索入口，由 AnySearch 提供。单个检索问题用 query；只有 2-5 个彼此独立、可以并行回答的问题才用 queries，后者调用 AnySearch batch_search。\
         垂直搜索先用 web_search_domains 查询 sub_domain 和必填参数，再传 domain、sub_domain、sub_domain_params。\
         深读 URL 用 web_read，批量读 URL 用 web_fetch_batch。\
         传 `hydrate=3` 可一并抽取前 3 个结果的正文，常用于深度调研免去后续 web_read 调用。\
         注意：工具名 web_query（非 web_search，Anthropic 网关保留名会 400）。",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": "一个完整检索问题（与 queries 二选一）；相关关键词应合并在这一条中"
                },
                "queries": {
                    "type": "array",
                    "items": { "type": "string", "minLength": 1 },
                    "minItems": 2,
                    "maxItems": 5,
                    "description": "2-5 个彼此独立、无需依赖前一条结果的问题；调用 AnySearch batch_search（与 query 二选一）"
                },
                "num": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "description": "每个问题返回结果数量，默认 5"
                },
                "site": {
                    "type": "string",
                    "description": "可选，限制站点，如 https://github.com"
                },
                "domain": {
                    "type": "string",
                    "enum": ["general", "resource", "social_media", "finance", "academic", "legal", "health", "business", "security", "ip", "code", "energy", "environment", "agriculture", "travel", "film", "gaming"],
                    "description": "可选，AnySearch 顶级领域；垂直检索必须先调用 web_search_domains"
                },
                "sub_domain": {
                    "type": "string",
                    "description": "可选，AnySearch 子领域，如 finance.quote"
                },
                "sub_domain_params": {
                    "type": "object",
                    "description": "可选，web_search_domains 返回的子领域参数；标记 required 的字段必须全部传入",
                    "additionalProperties": true
                },
                "hydrate": {
                    "type": "integer",
                    "description": "可选，0-5。搜完后并发抽取 top-N 命中页正文随结果一起返回，省一轮 round-trip。默认 0（不抽）。",
                    "minimum": 0,
                    "maximum": 5
                },
                "hydrate_char_limit": {
                    "type": "integer",
                    "description": "每个 hydrated 页的字符上限，默认 4000",
                    "minimum": 500,
                    "maximum": 20000
                }
            },
            "oneOf": [
                { "required": ["query"] },
                { "required": ["queries"] }
            ],
            "additionalProperties": false
        }),
        Box::new(move |args: &Value| {
            let queries: Vec<String> = args["queries"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let query = args["query"].as_str().unwrap_or("").trim().to_string();
            let num = args["num"].as_u64().unwrap_or(5).min(10) as u8;
            let site = args["site"].as_str().map(|s| s.to_string());
            let domain = args["domain"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let sub_domain = args["sub_domain"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let sub_domain_params = args
                .get("sub_domain_params")
                .filter(|value| !value.is_null())
                .cloned();
            let hydrate = args
                .get("hydrate")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .min(5) as u8;
            let hydrate_char_limit = args
                .get("hydrate_char_limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(4000)
                .clamp(500, 20_000);
            Box::pin(async move {
                if domain.is_none() && (sub_domain.is_some() || sub_domain_params.is_some()) {
                    anyhow::bail!("sub_domain/sub_domain_params 需要同时提供 domain");
                }
                let vertical = domain.is_some();
                if vertical && sub_domain.is_none() {
                    anyhow::bail!(
                        "垂直检索必须提供 sub_domain；请先调用 web_search_domains 查询该 domain"
                    );
                }
                if vertical && hydrate > 0 {
                    anyhow::bail!("AnySearch 垂直搜索暂不支持 hydrate，请先搜索再用 web_read 精读");
                }
                if !queries.is_empty() {
                    if queries.len() < 2 {
                        anyhow::bail!("queries 至少 2 条，最多 5 条");
                    }
                    if queries.len() > 5 {
                        anyhow::bail!("queries 最多 5 条，当前 {}", queries.len());
                    }
                    if !query.is_empty() {
                        anyhow::bail!("query 与 queries 不能同时传");
                    }
                    progress::emit(&format!("🔎 批量搜索 {} 条", queries.len()));
                    if hydrate == 0 {
                        match crate::tools::anysearch::batch_search_markdown(
                            &queries,
                            num as usize,
                            site.as_deref(),
                            domain.as_deref(),
                            sub_domain.as_deref(),
                            sub_domain_params.as_ref(),
                        )
                        .await
                        {
                            Ok(result) => {
                                return Ok(truncate_with_prefix_hint(
                                    &result,
                                    QUERY_LIMIT,
                                    "批量搜索结果已截断",
                                ));
                            }
                            Err(error) if vertical => {
                                return Err(error).context(
                                    "AnySearch 垂直 batch_search 失败；未降级为普通搜索以避免改变检索语义",
                                );
                            }
                            Err(error) => progress::emit(&format!(
                                "⚠️ AnySearch batch_search 失败，改为逐条调用 AnySearch: {error}"
                            )),
                        }
                    }
                    return Ok(batch_fetch_queries(
                        queries,
                        num,
                        site.as_deref(),
                        hydrate,
                        hydrate_char_limit,
                    )
                    .await);
                }
                if query.is_empty() {
                    anyhow::bail!("需要 query 或 queries（2-5 条）");
                }
                let preview: String = query.chars().take(60).collect();
                progress::emit(&format!("🔎 搜索: {}", preview));
                if vertical {
                    let result = crate::tools::anysearch::search_markdown_with_options(
                        &query,
                        num as usize,
                        site.as_deref(),
                        domain.as_deref(),
                        sub_domain.as_deref(),
                        sub_domain_params.as_ref(),
                    )
                    .await?;
                    return Ok(truncate_with_prefix_hint(
                        &result,
                        QUERY_LIMIT,
                        "搜索结果已截断",
                    ));
                }
                if hydrate > 0 {
                    cached_fetch_query_with_hydrate(
                        &query,
                        num,
                        site.as_deref(),
                        hydrate,
                        hydrate_char_limit,
                    )
                    .await
                } else {
                    cached_fetch_query(&query, num, site.as_deref()).await
                }
            })
        }),
    );

    registry.register(
        "web_fetch_batch",
        "通过 AnySearch 批量抽取多个公开 URL 的正文。\
         同一会话内已读过的 URL 会命中缓存。适合对比多个文档、调研多来源。\
         最多 10 个 URL，并行读取；不支持登录态页面或页面交互。",
        json!({
            "type": "object",
            "properties": {
                "urls": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": 10,
                    "description": "要读取的 URL 列表（最多 10 个）"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "每个 URL 的起始字符偏移（默认 0）"
                }
            },
            "required": ["urls"],
            "additionalProperties": false
        }),
        Box::new(move |args: &Value| {
            let urls: Vec<String> = args["urls"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            Box::pin(async move {
                if urls.is_empty() {
                    anyhow::bail!("urls is required and must be non-empty");
                }
                if urls.len() > BATCH_MAX_URLS {
                    anyhow::bail!("最多支持 {} 个 URL，当前 {}", BATCH_MAX_URLS, urls.len());
                }

                progress::emit(&format!("📚 批量读取 {} 个 URL", urls.len()));

                let total = urls.len();
                let mut sections = Vec::with_capacity(total);
                let cache = web_context::web_cache();
                let sid = web_context::session_id();
                let mut handles = Vec::with_capacity(total);
                for url in urls {
                    let cache = cache.clone();
                    let sid = sid.clone();
                    handles.push(tokio::spawn(async move {
                        let fut = async { cached_fetch_read(&url, offset).await.map(|c| (url, c)) };
                        if cache.is_some() || sid.is_some() {
                            web_context::with_web_context(cache, sid, fut).await
                        } else {
                            fut.await
                        }
                    }));
                }
                for (i, handle) in handles.into_iter().enumerate() {
                    let section = match handle.await {
                        Ok(Ok((url, content))) => {
                            format!("### [{}/{}] {}\n\n{}", i + 1, total, url, content)
                        }
                        Ok(Err(e)) => {
                            format!("### [{}/{}] unknown\n\n❌ {}", i + 1, total, e)
                        }
                        Err(e) => {
                            format!("### [{}/{}] unknown\n\n❌ 任务异常: {}", i + 1, total, e)
                        }
                    };
                    sections.push(section);
                }

                Ok(sections.join("\n\n---\n\n"))
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_reader_rejects_local_and_non_http_urls() {
        assert!(validate_public_http_url("file:///tmp/private").is_err());
        assert!(validate_public_http_url("http://localhost:3456/api").is_err());
        assert!(validate_public_http_url("http://127.0.0.1/private").is_err());
        assert!(validate_public_http_url("http://10.0.0.8/private").is_err());
        assert!(validate_public_http_url("https://public.wmo.int/report").is_ok());
    }

    #[test]
    fn public_download_filename_is_bounded_and_allowlisted() {
        assert_eq!(
            safe_download_filename(
                "https://www.bea.gov/files/gdp4q24-adv.xlsx?download=1",
                None,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            )
            .unwrap(),
            "gdp4q24-adv.xlsx"
        );
        assert_eq!(
            safe_download_filename("https://api.example.com/export", None, "text/csv",).unwrap(),
            "export.csv"
        );
        assert!(safe_download_filename(
            "https://example.com/payload",
            Some("../../payload.exe"),
            "application/octet-stream",
        )
        .is_err());
    }

    #[test]
    fn anysearch_tools_expose_agent_safe_schemas() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);

        let domains = registry
            .get_definition("web_search_domains")
            .expect("web_search_domains");
        assert_eq!(domains.parameters["oneOf"].as_array().unwrap().len(), 2);
        assert_eq!(domains.parameters["properties"]["domains"]["maxItems"], 5);
        assert!(domains
            .description
            .contains("所有标记 required 的参数原样填入"));

        let query = registry.get_definition("web_query").expect("web_query");
        assert_eq!(query.parameters["properties"]["queries"]["minItems"], 2);
        assert_eq!(query.parameters["properties"]["queries"]["maxItems"], 5);
        assert!(query.description.contains("唯一联网搜索入口"));
        assert!(query.description.contains("AnySearch batch_search"));
    }
}
