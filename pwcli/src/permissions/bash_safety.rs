//! bash 命令的黑名单评估（service 模式 / 无交互 UI 时使用）。
//!
//! 策略：**默认 Allow，只拦黑名单**。理由是 service 模式下 AI 经常需要
//! 跑 skill 脚本 / 数据查询 / 文件操作 等等多样命令，白名单越列越长还总
//! 漏，最终 AI 反复试错卡死。改成黑名单后只防最高危的破坏性命令。
//!
//! 当前黑名单只保留 **rm 强制递归删除** 的各种变体（`rm -rf` 等）。其他命令
//! （包括 mkfs / dd / curl|sh 这类破坏性或潜在风险动作）一律放行 —— 用户
//! 明确要求"除 rm 外都放开"，按用户决策执行。
//!
//! 黑名单是 **保守 substring 匹配**，可能误伤包含字面量 "rm -rf" 的命令。
//! 这是设计选择 —— 宁可 false-positive 也不漏放。误伤时 AI 看到错误信息会
//! 换写法。

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(String),
    Prompt(String),
}

/// 黑名单：substring 命中即拒绝，无视前后是否被引号包住或 pipe 连接。
const BLACKLIST: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "rm -r -f",
    "rm -f -r",
    "rm -Rf",
    "rm -fR",
    "rm --recursive --force",
    "rm --force --recursive",
];

pub fn evaluate(command: &str) -> Verdict {
    let cmd = command.trim();
    if cmd.is_empty() {
        return Verdict::Deny("空命令".to_string());
    }

    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_bash::LANGUAGE.into();
    if parser.set_language(&language).is_err() {
        return Verdict::Prompt("bash AST 初始化失败，需要人工确认".to_string());
    }
    let Some(tree) = parser.parse(cmd, None) else {
        return Verdict::Prompt("bash AST 解析失败，需要人工确认".to_string());
    };
    if tree.root_node().has_error() {
        return Verdict::Prompt("bash 命令包含无法可靠解析的语法，需要人工确认".to_string());
    }
    for segment in command_segments(tree.root_node(), cmd.as_bytes()) {
        for &blocked in BLACKLIST {
            if segment.contains(blocked) {
                return Verdict::Deny(format!("黑名单命中：`{}` 不允许执行", blocked));
            }
        }
    }

    Verdict::Allow
}

fn command_segments<'a>(root: tree_sitter::Node<'_>, source: &'a [u8]) -> Vec<&'a str> {
    let mut cursor = root.walk();
    let mut segments = Vec::new();
    loop {
        let node = cursor.node();
        if matches!(node.kind(), "command" | "redirected_statement") {
            if let Ok(text) = node.utf8_text(source) {
                segments.push(text);
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return segments;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_blocks_rm_rf_variants() {
        for cmd in [
            "rm -rf /",
            "rm -rf ~/Documents",
            "echo a && rm -rf b",
            "rm -fr ./x",
            "rm -r -f tmp",
            "rm --recursive --force foo",
            "FOO=bar rm -rf .",
        ] {
            assert!(
                matches!(evaluate(cmd), Verdict::Deny(r) if r.contains("rm")),
                "should deny: {}",
                cmd
            );
        }
    }

    #[test]
    fn allows_everything_else_by_default() {
        for cmd in [
            "echo hello",
            "ls -la /tmp",
            "cat /etc/hostname",
            "git status",
            "python3 ~/.agents/skills/yuque-kit/scripts/yuque_doc.py create",
            "if [ -n \"$X\" ]; then echo yes; fi",
            "ls && cat /etc/hostname",
            "echo a; echo b",
            "ls | grep foo",
            "rsync src dst",
            "tar xzf foo.tar.gz",
            "make install",
            "rm file.txt",    // 单文件 rm 放行
            "rm -f file.txt", // -f 单文件放行
            "rm -r tmp",      // -r 不带 -f 放行（仍可能风险，但按用户策略放行）
            "FOO=bar ls /tmp",
            "curl https://example.com/data.json",
            "wget http://example.com/file",
        ] {
            assert_eq!(evaluate(cmd), Verdict::Allow, "should allow: {}", cmd);
        }
    }

    #[test]
    fn empty_command_denied() {
        assert!(matches!(evaluate(""), Verdict::Deny(_)));
        assert!(matches!(evaluate("   "), Verdict::Deny(_)));
    }

    #[test]
    fn malformed_bash_requires_manual_review() {
        assert!(matches!(evaluate("echo $("), Verdict::Prompt(_)));
    }
}
