//! Vision capability gating + base64 image marker for `read_file`.
//!
//! `read_file` 工具收到图片扩展名（png/jpg/jpeg/gif/webp）时调本模块：
//! - 检查当前 active model 是否支持 vision；不支持直接拒绝
//! - 支持 → 本地 fs 读字节 → base64 → 返回特殊标记 `<image_data mime="...">BASE64</image_data>`
//! - llm/openai.rs + llm/anthropic.rs 在构建 tool message 时检测此标记，转成各自 provider 的多模态格式
//!
//! 模型能力只能来自本次 ToolExecutionContext；缺失时保守拒绝图片读取。

use anyhow::{anyhow, Result};
use base64::Engine;

use crate::llm::model_context::ActiveModelContext;

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

/// 缺失 acting model 时 fail-closed，避免直接调用 ToolRegistry 绕过视觉门禁。
pub fn model_supports_vision(model: Option<&ActiveModelContext>) -> bool {
    model.is_some_and(|model| model.supports_vision)
}

pub fn model_label(model: Option<&ActiveModelContext>) -> &str {
    model.map_or("<未配置>", |model| model.model_id.as_str())
}

/// 读 image 文件返回 `<image_data>` 标记字符串。
/// 调用方先调 [`model_supports_vision`] 把关。
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

    #[test]
    fn explicit_model_context_controls_vision_and_missing_context_fails_closed() {
        let ctx = ActiveModelContext {
            provider_id: Some("openai".to_string()),
            model_id: "gpt-4o".to_string(),
            effort: None,
            thinking: false,
            supports_vision: true,
        };
        assert!(model_supports_vision(Some(&ctx)));
        let label = model_label(Some(&ctx));
        assert_eq!(label, "gpt-4o");

        let ctx_no = ActiveModelContext {
            provider_id: None,
            model_id: "text-only".to_string(),
            effort: None,
            thinking: false,
            supports_vision: false,
        };
        assert!(!model_supports_vision(Some(&ctx_no)));
        assert!(!model_supports_vision(None));
        assert_eq!(model_label(None), "<未配置>");
    }
}
