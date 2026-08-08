//! 启动期 / banner 阶段使用的少量打印工具。
//!
//! REPL 主循环已经迁移到 `tui_app.rs`（ratatui inline 模式），这里只保留：
//! - 给 banner / bootstrap 阶段用的颜色常量
//! - `visible_len`：banner 排版用，剥离 ANSI 转义后计算可见列宽
//! - `print_phase`：bootstrap 启动每个阶段的提示

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[90m";
pub const CYAN: &str = "\x1b[36m";
pub const BLUE: &str = "\x1b[34m";

/// 计算可见字符长度（忽略 ANSI 转义码）
pub fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            len += 1;
        }
    }
    len
}
