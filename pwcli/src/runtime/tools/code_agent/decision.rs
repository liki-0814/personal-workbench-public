//! 决策上浮协议解析。
//!
//! CLI 子 agent 真正卡在分叉时，会在最终输出末尾按以下格式声明（独立段落）：
//!
//! ```text
//! <<<DECISION_REQUIRED>>>
//! question: 一句话问题
//! options:
//!   - 候选 A
//!   - 候选 B
//! <<<END_DECISION>>>
//! ```
//!
//! pwcli 检测到该标记后，把 question / options 解构出来，让 pwcli 主 agent 决策
//! （能决定就用同一 session_id 续聊；不能决定就回到 chat 让用户选）。

pub const DECISION_START: &str = "<<<DECISION_REQUIRED>>>";
pub const DECISION_END: &str = "<<<END_DECISION>>>";

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedDecision {
    /// 原文剥掉决策段后的部分（trim 末尾空白）。
    pub stripped_output: String,
    pub question: String,
    pub options: Vec<String>,
}

/// 在文本里寻找 `<<<DECISION_REQUIRED>>> ... <<<END_DECISION>>>` 段并解析。
/// 找不到返回 None，留给调用方按 status='ok' 处理。
pub fn parse_decision_marker(text: &str) -> Option<ParsedDecision> {
    let start = text.rfind(DECISION_START)?;
    let after_start = start + DECISION_START.len();
    let end_rel = text[after_start..].find(DECISION_END)?;
    let body = &text[after_start..after_start + end_rel];
    let stripped_output = text[..start].trim_end().to_string();

    let mut question = String::new();
    let mut options = Vec::new();
    let mut in_options = false;

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("question:") {
            question = rest.trim().to_string();
            in_options = false;
            continue;
        }
        if line == "options:" || line.starts_with("options:") {
            in_options = true;
            // 兼容 "options: A | B" 内联写法
            let inline = line.trim_start_matches("options:").trim();
            if !inline.is_empty() {
                for piece in inline.split(['|', ',']) {
                    let p = piece.trim();
                    if !p.is_empty() {
                        options.push(p.to_string());
                    }
                }
            }
            continue;
        }
        if in_options {
            // 接受 "- xxx" / "* xxx" / "1. xxx" 等列表前缀
            let opt = line
                .trim_start_matches('-')
                .trim_start_matches('*')
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
                .trim();
            if !opt.is_empty() {
                options.push(opt.to_string());
            }
        }
    }

    if question.is_empty() {
        return None;
    }

    Some(ParsedDecision {
        stripped_output,
        question,
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_decision() {
        let text = r#"我已分析 src/foo.ts，看到默认导出有歧义。

<<<DECISION_REQUIRED>>>
question: foo.ts 当前默认导出 X 和 Y，你想保留哪个为命名 default?
options:
  - X
  - Y
<<<END_DECISION>>>
"#;
        let parsed = parse_decision_marker(text).unwrap();
        assert_eq!(
            parsed.question,
            "foo.ts 当前默认导出 X 和 Y，你想保留哪个为命名 default?"
        );
        assert_eq!(parsed.options, vec!["X".to_string(), "Y".to_string()]);
        assert!(parsed.stripped_output.contains("我已分析 src/foo.ts"));
        assert!(!parsed.stripped_output.contains("DECISION_REQUIRED"));
    }

    #[test]
    fn returns_none_when_no_marker() {
        assert_eq!(parse_decision_marker("just a normal answer"), None);
        // 半个标记也不算
        assert_eq!(parse_decision_marker("<<<DECISION_REQUIRED>>> oops"), None);
    }

    #[test]
    fn picks_last_marker_if_multiple() {
        // 文档里如果讨论了 marker 格式自身，要取最后那次实际触发的
        let text = r#"我说明一下：触发 <<<DECISION_REQUIRED>>> 的格式是这样。

实际我现在需要决策：

<<<DECISION_REQUIRED>>>
question: 真正的问题
options:
  - 是
  - 否
<<<END_DECISION>>>"#;
        let parsed = parse_decision_marker(text).unwrap();
        assert_eq!(parsed.question, "真正的问题");
        assert_eq!(parsed.options.len(), 2);
    }

    #[test]
    fn handles_inline_options_and_various_bullets() {
        let text = r#"<<<DECISION_REQUIRED>>>
question: 选哪个?
options: A | B | C
<<<END_DECISION>>>"#;
        let parsed = parse_decision_marker(text).unwrap();
        assert_eq!(parsed.options, vec!["A", "B", "C"]);
    }

    #[test]
    fn missing_question_returns_none() {
        let text = r#"<<<DECISION_REQUIRED>>>
options:
  - A
<<<END_DECISION>>>"#;
        assert_eq!(parse_decision_marker(text), None);
    }
}
