//! arxiv PDF → HTML 启发：很多 arxiv 论文有官方 `arxiv.org/html/<id>` 渲染版，
//! 优先通过 AnySearch 抽取官方 HTML，比 PDF 解析更轻量。
//!
//! 流程：
//! 1. 正则提 arxiv id（兼容 `https://arxiv.org/pdf/2401.12345` / `.pdf` 后缀 / v1 版本号）
//! 2. 通过 AnySearch extract 获取 Markdown；不可用时让上层走 MinerU
//! 3. 扫 markdown 中 `![](http(s)://...)` 远程图，并发下载到 `<out_dir>/images/img_N.<ext>`，改写为相对路径

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

use crate::runtime::tools::progress;
use crate::runtime::tools::web::shared_http_client;

static ARXIV_PDF_RE: OnceLock<Regex> = OnceLock::new();
static MD_IMG_RE: OnceLock<Regex> = OnceLock::new();

fn arxiv_re() -> &'static Regex {
    ARXIV_PDF_RE.get_or_init(|| {
        // 接受可选 .pdf；id 中可含点和短横（如 2401.12345v2 / cs.AI/0512345）
        Regex::new(r"^https?://arxiv\.org/pdf/([^/\s?#]+?)(\.pdf)?(?:\?.*)?(?:#.*)?$").unwrap()
    })
}

fn md_img_re() -> &'static Regex {
    MD_IMG_RE.get_or_init(|| Regex::new(r"!\[([^\]]*)\]\((https?://[^)\s]+)\)").unwrap())
}

/// 提取 arxiv pdf URL 中的 id（如 `2401.12345`、`2401.12345v2`、`cs.AI/0512345`）。
/// 非 arxiv pdf URL → None。供 [`crate::runtime::tools::pdf`] 复用同一正则避免漂移。
pub fn extract_arxiv_id(pdf_url: &str) -> Option<String> {
    let caps = arxiv_re().captures(pdf_url.trim())?;
    Some(caps.get(1)?.as_str().to_string())
}

/// 给定 PDF URL 解析对应 HTML URL；非 arxiv pdf URL → None
pub fn arxiv_html_url(pdf_url: &str) -> Option<String> {
    extract_arxiv_id(pdf_url).map(|id| format!("https://arxiv.org/html/{id}"))
}

/// 启发主入口。失败（非 arxiv / 提示无 HTML / AnySearch 抽取失败）一律 bail，
/// 上层捕获后顺序往 MinerU / PyMuPDF 退。
pub async fn try_fetch_arxiv_html(pdf_url: &str, out_dir: &Path) -> Result<String> {
    let html_url =
        arxiv_html_url(pdf_url).ok_or_else(|| anyhow!("非 arxiv pdf URL，跳过 HTML 启发"))?;
    progress::emit(&format!("🔎 尝试 arxiv HTML: {}", html_url));

    let raw = crate::runtime::tools::anysearch::extract(&html_url).await?;
    let lower = raw.to_lowercase();
    if lower.contains("html is not available")
        || lower.contains("compilation error")
        || lower.contains("no html for this paper")
    {
        anyhow::bail!("arxiv HTML 页面提示无可渲染内容");
    }

    // 下载/改写图片（best-effort，单图失败不阻塞）
    let images_dir = out_dir.join("images");
    let _ = tokio::fs::create_dir_all(&images_dir).await;

    let captures: Vec<(String, String)> = md_img_re()
        .captures_iter(&raw)
        .filter_map(|c| {
            let alt = c.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
            let url = c.get(2)?.as_str().to_string();
            Some((alt, url))
        })
        .collect();

    let mut url_to_rel: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if !captures.is_empty() {
        progress::emit(&format!("🖼 下载图片 {} 张", captures.len()));
        let mut tasks = Vec::new();
        for (idx, (_, url)) in captures.iter().enumerate() {
            let url_owned = url.clone();
            let images_dir = images_dir.clone();
            tasks.push(tokio::spawn(async move {
                let client = match shared_http_client() {
                    Ok(c) => c,
                    Err(_) => return None,
                };
                let resp = client.get(&url_owned).send().await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                let bytes = resp.bytes().await.ok()?;
                let ext = guess_ext(&url_owned, &bytes);
                let filename = format!("img_{}.{}", idx + 1, ext);
                let dest = images_dir.join(&filename);
                tokio::fs::write(&dest, &bytes).await.ok()?;
                Some((url_owned, format!("images/{}", filename)))
            }));
        }
        for t in tasks {
            if let Ok(Some((url, rel))) = t.await {
                url_to_rel.insert(url, rel);
            }
        }
    }

    // 改写 markdown：单趟正则 replace
    let rewritten = md_img_re()
        .replace_all(&raw, |caps: &regex::Captures| {
            let alt = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let url = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            match url_to_rel.get(url) {
                Some(rel) => format!("![{alt}]({rel})"),
                None => caps.get(0).unwrap().as_str().to_string(),
            }
        })
        .into_owned();

    let full_md = out_dir.join("full.md");
    tokio::fs::write(&full_md, &rewritten)
        .await
        .with_context(|| format!("写 {} 失败", full_md.display()))?;
    progress::emit("✅ arxiv HTML 提取成功");
    Ok(rewritten)
}

fn guess_ext(url: &str, bytes: &[u8]) -> &'static str {
    // 首选魔数
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return "png";
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "jpg";
    }
    if bytes.starts_with(b"GIF8") {
        return "gif";
    }
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "webp";
    }
    if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") {
        return "svg";
    }
    // fallback URL 后缀
    let lower = url.to_lowercase();
    for ext in ["png", "jpg", "jpeg", "gif", "webp", "svg"] {
        if lower.ends_with(&format!(".{ext}")) {
            return match ext {
                "jpeg" => "jpg",
                e => match e {
                    "png" => "png",
                    "jpg" => "jpg",
                    "gif" => "gif",
                    "webp" => "webp",
                    "svg" => "svg",
                    _ => "png",
                },
            };
        }
    }
    "png"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_arxiv_id_with_and_without_suffix() {
        assert_eq!(
            arxiv_html_url("https://arxiv.org/pdf/2401.12345"),
            Some("https://arxiv.org/html/2401.12345".to_string())
        );
        assert_eq!(
            arxiv_html_url("https://arxiv.org/pdf/2401.12345.pdf"),
            Some("https://arxiv.org/html/2401.12345".to_string())
        );
        assert_eq!(
            arxiv_html_url("https://arxiv.org/pdf/2401.12345v2.pdf"),
            Some("https://arxiv.org/html/2401.12345v2".to_string())
        );
    }

    #[test]
    fn non_arxiv_returns_none() {
        assert_eq!(arxiv_html_url("https://example.com/foo.pdf"), None);
        assert_eq!(arxiv_html_url("https://arxiv.org/abs/2401.12345"), None);
    }
}
