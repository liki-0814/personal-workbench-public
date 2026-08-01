//! Vision capability gating + base64 image marker for `read_file`.
//!
//! `read_file` 工具收到图片扩展名（png/jpg/jpeg/gif/webp）时调本模块：
//! - 检查当前 active model 是否支持 vision；不支持直接拒绝
//! - 支持 → 本地 fs 读字节 → base64 → 返回特殊标记 `<image_data mime="...">BASE64</image_data>`
//! - llm/openai.rs + llm/anthropic.rs 在构建 tool message 时检测此标记，转成各自 provider 的多模态格式
//!
//! 模型识别（按优先级）：
//! 1. `llm::model_context` task-local 注入（service 模式下 per-request 真实 model）→ 直接用
//! 2. fallback：load `~/.pwcli/config.json` 的 active_provider → 在 models 列表中找 model id →
//!    看 `capabilities.vision == Some(true)`
//! - load 失败 / model 不在列表 → 保守返 false

use anyhow::{anyhow, Result};
use base64::Engine;

use crate::config::RuntimeConfig;

const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// 图片扩展（小写不带点）。
pub fn detect_image_ext(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    for ext in ["png", "jpg", "jpeg", "gif", "webp"] {
        if lower.ends_with(&format!(".{ext}")) {
            return Some(ext);
        }
    }
    None
}

/// 把扩展名映射到 MIME。
pub fn ext_to_mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// 当前 active model 是否支持 vision。两层：
/// 1. service 模式注入的 [`crate::llm::model_context`] task-local → 直接用
/// 2. fallback：查 `~/.pwcli/config.json` 的 active_provider → models[id].capabilities.vision
///    - 失败一律返 false（保守）
pub fn current_model_supports_vision() -> bool {
    if let Some(v) = crate::llm::model_context::try_current_supports_vision() {
        return v;
    }
    fallback_supports_vision_from_config()
}

fn fallback_supports_vision_from_config() -> bool {
    let cfg = RuntimeConfig::load();
    let Some(provider) = cfg.active_provider() else {
        return false;
    };
    provider
        .models
        .iter()
        .find(|m| m.id == provider.model)
        .and_then(|m| m.capabilities.as_ref())
        .and_then(|c| c.vision)
        .unwrap_or(false)
}

/// 拿到当前 active model id（用于错误提示）。优先 task-local 注入，回退 config.json。
pub fn current_model_label() -> String {
    if let Some(id) = crate::llm::model_context::try_current_model_id() {
        return id;
    }
    RuntimeConfig::load()
        .active_provider()
        .map(|p| p.model.clone())
        .unwrap_or_else(|| "<未配置>".to_string())
}

/// 读 image 文件返回 `<image_data>` 标记字符串。
/// 调用方先调 [`current_model_supports_vision`] 把关。
pub async fn read_image_as_marker(path: &str) -> Result<String> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| anyhow!("无法读取 {}: {}", path, e))?;
    if !meta.is_file() {
        anyhow::bail!("路径不是文件: {}", path);
    }
    if meta.len() > MAX_IMAGE_BYTES {
        anyhow::bail!(
            "图片过大 ({:.1} MB)，上限 5 MB: {}",
            meta.len() as f64 / 1024.0 / 1024.0,
            path
        );
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| anyhow!("读取 {} 失败: {}", path, e))?;
    let ext = detect_image_ext(path).unwrap_or("png");
    let mime = ext_to_mime(ext);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!(
        "<image_data mime=\"{}\">{}</image_data>",
        mime, b64
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_image_ext_case_insensitive() {
        assert_eq!(detect_image_ext("foo.PNG"), Some("png"));
        assert_eq!(detect_image_ext("foo.jpeg"), Some("jpeg"));
        assert_eq!(detect_image_ext("foo.txt"), None);
    }

    #[test]
    fn ext_to_mime_mapping() {
        assert_eq!(ext_to_mime("png"), "image/png");
        assert_eq!(ext_to_mime("jpg"), "image/jpeg");
        assert_eq!(ext_to_mime("jpeg"), "image/jpeg");
        assert_eq!(ext_to_mime("webp"), "image/webp");
    }

    #[tokio::test]
    async fn task_local_overrides_config_fallback() {
        use crate::llm::model_context::{with_active_model, ActiveModelContext};
        let ctx = ActiveModelContext {
            provider_id: Some("openai".to_string()),
            model_id: "gpt-4o".to_string(),
            effort: None,
            thinking: false,
            supports_vision: true,
        };
        let (vision_yes, label) = with_active_model(ctx, async {
            (current_model_supports_vision(), current_model_label())
        })
        .await;
        assert!(
            vision_yes,
            "task-local supports_vision=true 应直接返回 true"
        );
        assert_eq!(label, "gpt-4o");

        let ctx_no = ActiveModelContext {
            provider_id: None,
            model_id: "text-only".to_string(),
            effort: None,
            thinking: false,
            supports_vision: false,
        };
        let vision_no = with_active_model(ctx_no, async { current_model_supports_vision() }).await;
        assert!(
            !vision_no,
            "task-local supports_vision=false 应直接返回 false，不回退 config"
        );
    }
}
