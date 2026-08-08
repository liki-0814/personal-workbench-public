//! `<image_data mime="...">BASE64</image_data>` 标记解析，跨 OpenAI / Anthropic 共用。
//!
//! 由 `read_file` 工具（处理图片路径时）注入到 tool message 的 content 中。
//! LLM client 构建消息时调本模块拆分文本/图片块。

use std::sync::OnceLock;

use regex::Regex;

static IMAGE_DATA_RE: OnceLock<Regex> = OnceLock::new();

fn re() -> &'static Regex {
    IMAGE_DATA_RE.get_or_init(|| {
        Regex::new(r#"<image_data mime="([^"]+)">([A-Za-z0-9+/=]+)</image_data>"#).unwrap()
    })
}

/// 单段解析结果。
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Text(String),
    Image { mime: String, base64: String },
}

/// 内容是否含至少一个 `<image_data>` 标记。
pub fn contains_marker(content: &str) -> bool {
    re().is_match(content)
}

/// 按出现顺序拆 [text, image, text, image, ...]，空文本段跳过。
pub fn parse_segments(content: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for caps in re().captures_iter(content) {
        let m = caps.get(0).unwrap();
        if m.start() > cursor {
            let txt = &content[cursor..m.start()];
            if !txt.trim().is_empty() {
                out.push(Segment::Text(txt.to_string()));
            }
        }
        let mime = caps.get(1).map(|m| m.as_str()).unwrap_or("image/png");
        let b64 = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        out.push(Segment::Image {
            mime: mime.to_string(),
            base64: b64.to_string(),
        });
        cursor = m.end();
    }
    if cursor < content.len() {
        let tail = &content[cursor..];
        if !tail.trim().is_empty() {
            out.push(Segment::Text(tail.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_marker() {
        assert!(contains_marker(
            "<image_data mime=\"image/png\">ABCD==</image_data>"
        ));
        assert!(!contains_marker("plain text"));
    }

    #[test]
    fn splits_mixed_content() {
        let s = "前缀 <image_data mime=\"image/jpeg\">XXX</image_data> 后缀";
        let segs = parse_segments(s);
        assert_eq!(segs.len(), 3);
        matches!(segs[0], Segment::Text(_));
        matches!(segs[1], Segment::Image { .. });
        matches!(segs[2], Segment::Text(_));
    }

    #[test]
    fn image_only() {
        let s = "<image_data mime=\"image/png\">YYY</image_data>";
        let segs = parse_segments(s);
        assert_eq!(segs.len(), 1);
        match &segs[0] {
            Segment::Image { mime, base64 } => {
                assert_eq!(mime, "image/png");
                assert_eq!(base64, "YYY");
            }
            _ => panic!("expected image"),
        }
    }
}
