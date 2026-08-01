//! 注入给原生 CLI 子 agent 的任务约束。
//!
//! 三件事必须告知子 agent：
//! 1. 你是子 agent，不能与人交互（哪怕 dontAsk 也别"问问"）
//! 2. 时间预算
//! 3. 真正卡住时按 `<<<DECISION_REQUIRED>>>` 协议把问题抛回 pwcli
//!

use crate::tools::code_agent::decision::{DECISION_END, DECISION_START};

/// 构造追加给 CLI 子 agent 的任务约束。
pub fn build_subagent_addendum(
    timeout_secs: u64,
    spec: Option<&str>,
    project_rules: Option<&str>,
) -> String {
    let mut result = format!(
        r##"
你正以 pwcli 的子 agent 身份运行（不在交互终端里）。约束如下：

【时间预算】≤ {timeout_secs} 秒。剩余时间不足时，立即输出"已知信息 + 阶段性结论"，不要陷入完美主义。

【决策原则】
1. 你不能与用户交互。permission-mode 已设为不询问；遇到模糊点请自己按最稳妥方式拍板，并在输出里说明你的选择和理由。
2. 仅当遇到无法靠代码 / 文档信息自己判断、且必须由人决定的真正分叉时，在最终输出末尾按这个格式声明（独立成段）：

   {DECISION_START}
   question: <一句话问题>
   options:
     - <候选 A>
     - <候选 B>
   {DECISION_END}

3. 触发 DECISION_REQUIRED 的标准：影响代码安全 / 方向、不可逆操作、风格强偏好。
   单纯执行细节（先扫描还是先深读、用哪个 grep 模式）不要触发，自己定。

【任务能力】优先使用 CLI 当前会话实际提供的原生工具、命令和 skills；不要假设不存在的能力。

【输出风格】结构化、简洁。结论先行，必要时给关键证据（文件路径 + 行号）。

【完成前检查】（仅 edit 模式）
- 修改的文件能通过 lint / 类型检查（有现成命令就跑一下）
- 有关联测试时，测试仍通过
- 不引入明文密钥、硬编码凭据、不安全的 eval/exec
- 不改动任务范围外的文件
- 如果任务涉及 UI，确认样式在 dark/light 双模式下均合理
"##,
        timeout_secs = timeout_secs,
        DECISION_START = DECISION_START,
        DECISION_END = DECISION_END,
    );

    if spec.is_some() || project_rules.is_some() {
        result.push_str("\n\n【任务上下文】\n");
        if let Some(s) = spec {
            result.push_str(&format!("规格（目标 + 约束 + 验收标准）：\n{}\n\n", s));
        }
        if let Some(r) = project_rules {
            result.push_str(&format!("项目约束：\n{}\n", r));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addendum_contains_timeout_and_marker() {
        let s = build_subagent_addendum(600, None, None);
        assert!(s.contains("600 秒"));
        assert!(s.contains("<<<DECISION_REQUIRED>>>"));
        assert!(s.contains("<<<END_DECISION>>>"));
    }

    #[test]
    fn addendum_contains_quality_checklist() {
        let s = build_subagent_addendum(600, None, None);
        assert!(s.contains("完成前检查"));
        assert!(s.contains("不引入明文密钥"));
        assert!(s.contains("dark/light"));
    }

    #[test]
    fn addendum_does_not_mention_money_budget() {
        let s = build_subagent_addendum(600, None, None);
        assert!(!s.contains("钱预算"));
        assert!(!s.contains("$"));
    }

    #[test]
    fn addendum_includes_spec_and_rules() {
        let s =
            build_subagent_addendum(600, Some("实现用户登录功能"), Some("不使用第三方 auth 库"));
        assert!(s.contains("任务上下文"));
        assert!(s.contains("实现用户登录功能"));
        assert!(s.contains("不使用第三方 auth 库"));
    }

    #[test]
    fn addendum_omits_context_when_none() {
        let s = build_subagent_addendum(600, None, None);
        assert!(!s.contains("任务上下文"));
    }
}
