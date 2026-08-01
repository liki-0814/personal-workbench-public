//! MinerU PDF 解析 API 客户端。
//!
//! - 设了 `MINERU_TOKEN`：走 Precision `/api/v4/extract/task`（VLM 模型，质量高）
//! - 没设：走 Agent `/api/v1/agent/parse/url`（免 token，质量略低，限速）
//!
//! 两者协议不同：
//! - Precision：异步任务 → 轮询 `/api/v4/extract/task/{id}` → done 时返回 `full_zip_url` → 下载 zip 解压取 `full.md`
//! - Agent：异步任务 → 轮询 `/api/v1/agent/parse/{id}` → done 时返回 `markdown_url` → 直接 GET 拿 markdown
//!
//! 轮询节奏：前 60s 每 5s 一次；之后每 10s；总超时 600s。每轮 `progress::emit`。

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::mineru::get_mineru_token;
use crate::tools::progress;
use crate::tools::web::shared_http_client;

const PRECISION_BASE: &str = "https://mineru.net/api/v4/extract";
const AGENT_BASE: &str = "https://mineru.net/api/v1/agent";
const POLL_TOTAL_SECS: u64 = 600;
const POLL_FAST_SECS: u64 = 60;
const POLL_FAST_INTERVAL_MS: u64 = 5_000;
const POLL_SLOW_INTERVAL_MS: u64 = 10_000;

/// 把 MinerU `data.state` 字符串映射为带 emoji 的中文友好标签。
/// 未知值兜底为 "⏳ 处理中"，不引入新错误路径。
fn translate_state(state: &str) -> &'static str {
    match state {
        "pending" => "⏳ 排队中",
        "running" => "🔄 解析中",
        "parsing" => "📖 文本提取",
        "converting" => "🔧 格式转换",
        "uploading" => "📤 上传中",
        "downloading" => "📥 下载中",
        "done" => "✅ 完成",
        "failed" => "❌ 失败",
        _ => "⏳ 处理中",
    }
}

/// 入口：根据 token 自动选 Precision / Agent，提交解析任务并轮询结果，写 `<out_dir>/full.md` 返回 markdown 文本。
pub async fn fetch_pdf_via_mineru(url: &str, out_dir: &Path) -> Result<String> {
    if let Some(token) = get_mineru_token() {
        progress::emit("📡 MinerU Precision (VLM) 提交任务…");
        run_precision(url, out_dir, &token).await
    } else {
        progress::emit("📡 MinerU Agent (免 token) 提交任务…");
        run_agent(url, out_dir).await
    }
}

// ─── Precision (v4) ───────────────────────────────────────────

async fn run_precision(url: &str, out_dir: &Path, token: &str) -> Result<String> {
    let client = shared_http_client()?;
    let auth = format!("Bearer {}", token);
    let submit_url = format!("{PRECISION_BASE}/task");
    let body = serde_json::json!({
        "url": url,
        "model_version": "vlm",
        "enable_table": true,
        "enable_formula": true,
    });
    let resp = client
        .post(&submit_url)
        .header("Authorization", &auth)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {} 失败", submit_url))?;
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("MinerU Precision 提交 {}: {}", status, body_text);
    }
    let body_json: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);
    let task_id = body_json
        .pointer("/data/task_id")
        .or_else(|| body_json.get("task_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("MinerU Precision 返回缺 task_id: {}", body_text))?
        .to_string();
    progress::emit(&format!("⏳ Precision 任务 {} 已提交，开始轮询…", task_id));

    let poll_url = format!("{PRECISION_BASE}/task/{task_id}");
    let start = Instant::now();
    let mut last_state: Option<String> = None;
    let mut interval_count: u64 = 0;
    loop {
        if start.elapsed() >= Duration::from_secs(POLL_TOTAL_SECS) {
            anyhow::bail!("MinerU Precision 轮询超时 ({}s)", POLL_TOTAL_SECS);
        }
        let interval = if start.elapsed() < Duration::from_secs(POLL_FAST_SECS) {
            POLL_FAST_INTERVAL_MS
        } else {
            POLL_SLOW_INTERVAL_MS
        };
        tokio::time::sleep(Duration::from_millis(interval)).await;
        interval_count += 1;

        let r = client
            .get(&poll_url)
            .header("Authorization", &auth)
            .send()
            .await;
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                progress::emit(&format!("⚠ Precision 轮询请求失败（重试）: {}", e));
                continue;
            }
        };
        if !r.status().is_success() {
            progress::emit(&format!("⚠ Precision 轮询 {}（重试）", r.status()));
            continue;
        }
        let v: Value = match r.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let state = v
            .pointer("/data/state")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string();
        let elapsed = start.elapsed().as_secs();
        let state_str = state.as_str();
        // 仅在 state 变化时打印；否则每 ~6 个轮询周期心跳一次，避免长时间静默
        if last_state.as_deref() != Some(state_str) {
            progress::emit(&format!(
                "{} MinerU Precision ({}s, state={})",
                translate_state(state_str),
                elapsed,
                state_str
            ));
            last_state = Some(state.clone());
        } else if interval_count.is_multiple_of(6) {
            progress::emit(&format!(
                "{} MinerU Precision 持续中... ({}s)",
                translate_state(state_str),
                elapsed
            ));
        }
        match state_str {
            "done" => {
                let zip_url = v
                    .pointer("/data/full_zip_url")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| anyhow!("Precision done 但缺 full_zip_url: {}", v))?;
                return download_and_unzip_precision(zip_url, out_dir).await;
            }
            "failed" => {
                let err = v
                    .pointer("/data/err_msg")
                    .and_then(|s| s.as_str())
                    .unwrap_or("无错误信息");
                anyhow::bail!(
                    "MinerU Precision 解析失败 (state={}, {}): {}",
                    state_str,
                    translate_state(state_str),
                    err
                );
            }
            _ => continue, // pending / running / waiting / ...
        }
    }
}

async fn download_and_unzip_precision(zip_url: &str, out_dir: &Path) -> Result<String> {
    let client = shared_http_client()?;
    progress::emit("⬇ 下载结果 zip…");
    let bytes = client
        .get(zip_url)
        .send()
        .await
        .with_context(|| format!("GET {} 失败", zip_url))?
        .bytes()
        .await
        .context("读取 zip 失败")?;
    let zip_path = out_dir.join("download.zip");
    tokio::fs::write(&zip_path, &bytes).await.ok();

    let out_dir_buf: PathBuf = out_dir.to_path_buf();
    let zip_bytes = bytes.clone();
    let md_text = tokio::task::spawn_blocking(move || -> Result<String> {
        let cursor = Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).context("解析 zip 失败")?;
        let mut md_text: Option<String> = None;
        let mut fallback_md: Option<String> = None;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            if file.is_dir() {
                continue;
            }
            let raw_name = file.name().to_string();
            // 防止 zip-slip：剥离绝对路径与 `..`
            let safe_rel = sanitize_zip_path(&raw_name);
            if safe_rel.is_empty() {
                continue;
            }
            let dest = out_dir_buf.join(&safe_rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let mut buf = Vec::new();
            std::io::copy(&mut file, &mut buf)?;
            std::fs::write(&dest, &buf)?;

            let lower_name = safe_rel.to_lowercase();
            if md_text.is_none()
                && (lower_name.ends_with("/full.md")
                    || lower_name == "full.md"
                    || lower_name.ends_with("/out.md")
                    || lower_name == "out.md"
                    || lower_name.ends_with("/main.md")
                    || lower_name == "main.md")
            {
                md_text = Some(String::from_utf8_lossy(&buf).to_string());
            } else if fallback_md.is_none() && lower_name.ends_with(".md") {
                fallback_md = Some(String::from_utf8_lossy(&buf).to_string());
            }
        }
        md_text
            .or(fallback_md)
            .ok_or_else(|| anyhow!("zip 中未找到 .md 文件"))
    })
    .await
    .context("解压任务 panic")??;

    let _ = tokio::fs::remove_file(&zip_path).await;

    let full_md = out_dir.join("full.md");
    tokio::fs::write(&full_md, &md_text).await.ok();
    Ok(md_text)
}

/// 简单 zip-slip 防护：去除前导 `/`，丢弃任何含 `..` 的路径。
fn sanitize_zip_path(name: &str) -> String {
    let trimmed = name.trim_start_matches(['/', '\\']);
    if trimmed
        .split(['/', '\\'])
        .any(|seg| seg == ".." || seg.is_empty())
    {
        // 用 _ 替换避免完全丢
        return trimmed.replace("..", "_");
    }
    trimmed.to_string()
}

// ─── Agent (v1) ──────────────────────────────────────────────

async fn run_agent(url: &str, out_dir: &Path) -> Result<String> {
    let client = shared_http_client()?;
    let submit_url = format!("{AGENT_BASE}/parse/url");
    let body = serde_json::json!({
        "url": url,
        "enable_table": true,
        "is_ocr": false,
        "enable_formula": true,
    });
    let resp = client
        .post(&submit_url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {} 失败", submit_url))?;
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("MinerU Agent 提交 {}: {}", status, body_text);
    }
    let body_json: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);
    let task_id = body_json
        .pointer("/data/task_id")
        .or_else(|| body_json.get("task_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("MinerU Agent 返回缺 task_id: {}", body_text))?
        .to_string();
    progress::emit(&format!("⏳ Agent 任务 {} 已提交，开始轮询…", task_id));

    let poll_url = format!("{AGENT_BASE}/parse/{task_id}");
    let start = Instant::now();
    let mut last_state: Option<String> = None;
    let mut interval_count: u64 = 0;
    loop {
        if start.elapsed() >= Duration::from_secs(POLL_TOTAL_SECS) {
            anyhow::bail!("MinerU Agent 轮询超时 ({}s)", POLL_TOTAL_SECS);
        }
        let interval = if start.elapsed() < Duration::from_secs(POLL_FAST_SECS) {
            POLL_FAST_INTERVAL_MS
        } else {
            POLL_SLOW_INTERVAL_MS
        };
        tokio::time::sleep(Duration::from_millis(interval)).await;
        interval_count += 1;

        let r = match client.get(&poll_url).send().await {
            Ok(r) => r,
            Err(e) => {
                progress::emit(&format!("⚠ Agent 轮询失败（重试）: {}", e));
                continue;
            }
        };
        if !r.status().is_success() {
            progress::emit(&format!("⚠ Agent 轮询 {}（重试）", r.status()));
            continue;
        }
        let v: Value = match r.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let state = v
            .pointer("/data/state")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string();
        let elapsed = start.elapsed().as_secs();
        let state_str = state.as_str();
        if last_state.as_deref() != Some(state_str) {
            progress::emit(&format!(
                "{} MinerU Agent ({}s, state={})",
                translate_state(state_str),
                elapsed,
                state_str
            ));
            last_state = Some(state.clone());
        } else if interval_count.is_multiple_of(6) {
            progress::emit(&format!(
                "{} MinerU Agent 持续中... ({}s)",
                translate_state(state_str),
                elapsed
            ));
        }
        match state_str {
            "done" => {
                let md_url = v
                    .pointer("/data/markdown_url")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| anyhow!("Agent done 但缺 markdown_url: {}", v))?;
                progress::emit("⬇ 下载 markdown…");
                let md_text = client
                    .get(md_url)
                    .send()
                    .await
                    .with_context(|| format!("GET {} 失败", md_url))?
                    .text()
                    .await
                    .context("读取 markdown 失败")?;
                let full_md = out_dir.join("full.md");
                tokio::fs::write(&full_md, &md_text).await.ok();
                return Ok(md_text);
            }
            "failed" => {
                let err = v
                    .pointer("/data/err_msg")
                    .and_then(|s| s.as_str())
                    .unwrap_or("无错误信息");
                anyhow::bail!(
                    "MinerU Agent 解析失败 (state={}, {}): {}",
                    state_str,
                    translate_state(state_str),
                    err
                );
            }
            _ => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_zip_path_strips_leading_slash() {
        assert_eq!(sanitize_zip_path("/foo/bar.md"), "foo/bar.md");
        assert_eq!(sanitize_zip_path("foo/bar.md"), "foo/bar.md");
    }

    #[test]
    fn sanitize_zip_path_replaces_dotdot() {
        assert!(!sanitize_zip_path("../etc/passwd").contains(".."));
    }

    #[test]
    fn test_translate_state_known() {
        assert_eq!(translate_state("pending"), "⏳ 排队中");
        assert_eq!(translate_state("running"), "🔄 解析中");
        assert_eq!(translate_state("done"), "✅ 完成");
        assert_eq!(translate_state("failed"), "❌ 失败");
    }

    #[test]
    fn test_translate_state_unknown_fallback() {
        assert_eq!(translate_state("foo"), "⏳ 处理中");
        assert_eq!(translate_state(""), "⏳ 处理中");
    }
}
