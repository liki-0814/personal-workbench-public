//! Shared failure classification and recovery protocol for tools and RuntimeTasks.
//!
//! The protocol is intentionally additive: older clients ignore unknown fields,
//! and historical messages without a FailureEnvelope remain readable.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_AUTO_RETRIES: u32 = 2;
pub const RETRY_AFTER_CAP_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Transient,
    PermissionRequired,
    InputRequired,
    Conflict,
    Interrupted,
    Validation,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDisposition {
    AutoRetrying,
    UserActionRequired,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureSource {
    ForegroundTool,
    BackgroundTool,
    RuntimeTask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryActionKind {
    RetryTask,
    SwitchExecutor,
    HandlePermission,
    ProvideInput,
    ResolveConflict,
    OpenSession,
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryAction {
    pub kind: RecoveryActionKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureEnvelope {
    pub schema_version: u32,
    pub code: String,
    pub class: FailureClass,
    pub disposition: FailureDisposition,
    pub source: FailureSource,
    pub user_message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub max_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<RecoveryAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRetryPolicy {
    pub allow_auto_retry: bool,
    pub max_attempts: u32,
}

impl Default for ToolRetryPolicy {
    fn default() -> Self {
        Self {
            allow_auto_retry: false,
            max_attempts: MAX_AUTO_RETRIES,
        }
    }
}

impl FailureEnvelope {
    pub fn new(
        code: impl Into<String>,
        class: FailureClass,
        source: FailureSource,
        user_message: impl Into<String>,
    ) -> Self {
        let mut envelope = Self {
            schema_version: SCHEMA_VERSION,
            code: code.into(),
            class,
            disposition: FailureDisposition::Terminal,
            source,
            user_message: user_message.into(),
            impact: None,
            attempt: 0,
            max_attempts: MAX_AUTO_RETRIES,
            next_retry_at: None,
            actions: Vec::new(),
            correlation_id: None,
            evidence: None,
        };
        envelope.disposition = envelope.default_disposition();
        envelope.actions = envelope.default_actions();
        envelope
    }

    pub fn with_attempt(mut self, attempt: u32, max_attempts: u32) -> Self {
        self.attempt = attempt;
        self.max_attempts = max_attempts.max(1);
        self.disposition = self.default_disposition();
        self.actions = self.default_actions();
        self
    }

    pub fn with_source(mut self, source: FailureSource) -> Self {
        self.source = source;
        self.actions = self.default_actions();
        self
    }

    pub fn with_impact(mut self, impact: impl Into<String>) -> Self {
        self.impact = Some(impact.into());
        self
    }

    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    pub fn with_evidence(mut self, evidence: Value) -> Self {
        self.evidence = Some(evidence);
        self
    }

    pub fn mark_auto_retrying(mut self, next_retry_at: Option<String>) -> Self {
        self.disposition = FailureDisposition::AutoRetrying;
        self.next_retry_at = next_retry_at;
        self.actions = Vec::new();
        self
    }

    /// Final failed state after classification. Never claims auto-retry unless
    /// the caller is about to schedule another attempt via `mark_auto_retrying`.
    pub fn finalize_for_user(mut self, _policy: ToolRetryPolicy) -> Self {
        self.disposition = match self.class {
            FailureClass::PermissionRequired
            | FailureClass::InputRequired
            | FailureClass::Conflict
            | FailureClass::Interrupted => FailureDisposition::UserActionRequired,
            // Transient tool failures only become user-action when a durable
            // task/inbox surface can offer retry. Foreground traces stay terminal.
            FailureClass::Transient
                if matches!(
                    self.source,
                    FailureSource::BackgroundTool | FailureSource::RuntimeTask
                ) =>
            {
                FailureDisposition::UserActionRequired
            }
            FailureClass::Transient | FailureClass::Validation | FailureClass::Permanent => {
                FailureDisposition::Terminal
            }
        };
        self.next_retry_at = None;
        self.actions = self.default_actions();
        self
    }

    pub fn can_auto_retry(&self, policy: ToolRetryPolicy) -> bool {
        policy.allow_auto_retry
            && matches!(self.class, FailureClass::Transient)
            && self.attempt < policy.max_attempts.min(MAX_AUTO_RETRIES)
    }

    fn default_disposition(&self) -> FailureDisposition {
        match self.class {
            FailureClass::PermissionRequired
            | FailureClass::InputRequired
            | FailureClass::Conflict
            | FailureClass::Interrupted => FailureDisposition::UserActionRequired,
            FailureClass::Transient | FailureClass::Validation | FailureClass::Permanent => {
                FailureDisposition::Terminal
            }
        }
    }

    fn default_actions(&self) -> Vec<RecoveryAction> {
        match self.class {
            FailureClass::Transient => match self.source {
                FailureSource::BackgroundTool | FailureSource::RuntimeTask => vec![
                    RecoveryAction {
                        kind: RecoveryActionKind::RetryTask,
                        label: "重试".into(),
                        recommended: Some(true),
                    },
                    RecoveryAction {
                        kind: RecoveryActionKind::OpenSession,
                        label: "打开原会话".into(),
                        recommended: None,
                    },
                ],
                FailureSource::ForegroundTool => vec![RecoveryAction {
                    kind: RecoveryActionKind::OpenSession,
                    label: "查看详情".into(),
                    recommended: Some(true),
                }],
            },
            FailureClass::PermissionRequired => vec![
                RecoveryAction {
                    kind: RecoveryActionKind::HandlePermission,
                    label: "处理权限".into(),
                    recommended: Some(true),
                },
                RecoveryAction {
                    kind: RecoveryActionKind::OpenSession,
                    label: "打开原会话".into(),
                    recommended: None,
                },
            ],
            FailureClass::InputRequired => vec![
                RecoveryAction {
                    kind: RecoveryActionKind::ProvideInput,
                    label: "补充信息".into(),
                    recommended: Some(true),
                },
                RecoveryAction {
                    kind: RecoveryActionKind::OpenSession,
                    label: "打开原会话".into(),
                    recommended: None,
                },
            ],
            FailureClass::Conflict => vec![
                RecoveryAction {
                    kind: RecoveryActionKind::ResolveConflict,
                    label: "处理冲突".into(),
                    recommended: Some(true),
                },
                RecoveryAction {
                    kind: RecoveryActionKind::Ignore,
                    label: "暂不处理".into(),
                    recommended: None,
                },
            ],
            FailureClass::Interrupted => vec![
                RecoveryAction {
                    kind: RecoveryActionKind::OpenSession,
                    label: "回到原会话".into(),
                    recommended: Some(true),
                },
                RecoveryAction {
                    kind: RecoveryActionKind::Ignore,
                    label: "忽略".into(),
                    recommended: None,
                },
            ],
            FailureClass::Validation | FailureClass::Permanent => vec![RecoveryAction {
                kind: RecoveryActionKind::OpenSession,
                label: "查看详情".into(),
                recommended: Some(true),
            }],
        }
    }
}

pub fn classify_error_message(message: &str) -> FailureClass {
    let lower = message.to_ascii_lowercase();
    if lower.contains("permission denied")
        || lower.contains("用户拒绝")
        || lower.contains("被策略拒绝")
        || lower.contains("not authorized")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
    {
        return FailureClass::PermissionRequired;
    }
    if lower.contains("merge required")
        || lower.contains("merge_conflict")
        || lower.contains("conflict")
        || lower.contains("需要合并")
    {
        return FailureClass::Conflict;
    }
    if lower.contains("recovery_required")
        || lower.contains("daemon restarted")
        || lower.contains("interrupted")
        || lower.contains("side effects unknown")
    {
        return FailureClass::Interrupted;
    }
    if lower.contains("invalid arguments")
        || lower.contains("schema validation")
        || lower.contains("must have")
        || lower.contains("missing required")
        || lower.contains("validation failed")
        || lower.contains("参数错误")
    {
        return FailureClass::Validation;
    }
    if lower.contains("waiting_user")
        || lower.contains("needs user")
        || lower.contains("requires a choice")
        || lower.contains("request_user_choice")
        || lower.contains("clarification")
    {
        return FailureClass::InputRequired;
    }
    if is_transient_message(&lower) {
        return FailureClass::Transient;
    }
    FailureClass::Permanent
}

pub fn classify_tool_failure(
    tool_name: &str,
    message: &str,
    source: FailureSource,
    attempt: u32,
    max_attempts: u32,
) -> FailureEnvelope {
    let class = classify_error_message(message);
    let code = failure_code(tool_name, class);
    let user_message = user_facing_message(class, message);
    FailureEnvelope::new(code, class, source, user_message)
        .with_attempt(attempt, max_attempts)
        .with_evidence(serde_json::json!({
            "tool": tool_name,
            "raw": truncate(message, 400),
        }))
        .finalize_for_user(ToolRetryPolicy {
            allow_auto_retry: is_idempotent_read_tool(tool_name),
            max_attempts,
        })
}

pub fn classify_runtime_failure(
    status_or_kind: &str,
    message: &str,
    attempt: u32,
    max_attempts: u32,
) -> FailureEnvelope {
    let class = if status_or_kind == "recovery_required" {
        FailureClass::Interrupted
    } else if status_or_kind == "merge_required" {
        FailureClass::Conflict
    } else if status_or_kind == "waiting_user" || status_or_kind == "waiting_configuration" {
        FailureClass::InputRequired
    } else {
        classify_error_message(message)
    };
    let code = failure_code(status_or_kind, class);
    FailureEnvelope::new(
        code,
        class,
        FailureSource::RuntimeTask,
        user_facing_message(class, message),
    )
    .with_attempt(attempt, max_attempts)
    .with_evidence(serde_json::json!({
        "status": status_or_kind,
        "raw": truncate(message, 400),
    }))
    .finalize_for_user(ToolRetryPolicy::default())
}

pub fn retry_delay(attempt: u32, retry_after_secs: Option<u64>) -> Duration {
    if let Some(secs) = retry_after_secs {
        return Duration::from_secs(secs.min(RETRY_AFTER_CAP_SECS).max(1));
    }
    let base = match attempt {
        0 => 500,
        1 => 1_500,
        _ => 3_000,
    };
    Duration::from_millis(base)
}

pub fn parse_retry_after_secs(message: &str) -> Option<u64> {
    let lower = message.to_ascii_lowercase();
    let marker = "retry-after:";
    let idx = lower.find(marker)?;
    let rest = message[idx + marker.len()..].trim_start();
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits
        .parse::<u64>()
        .ok()
        .map(|secs| secs.min(RETRY_AFTER_CAP_SECS))
}

pub fn is_idempotent_read_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "ls"
            | "read"
            | "find"
            | "grep"
            | "search_file_content"
            | "read_artifact"
            | "web_read"
            | "web_search_domains"
            | "web_query"
            | "web_fetch_batch"
            | "memory_search"
            | "memory_recall"
            | "archify_reference"
            | "inspect_document"
            | "inspect_document_layout"
            | "inspect_document_evidence"
            | "inspect_document_fragments"
    )
}

fn is_transient_message(lower: &str) -> bool {
    lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("temporarily unavailable")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
        || lower.contains("network")
        || lower.contains("dns")
        || lower.contains("name resolution")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("database is busy")
        || lower.contains("database locked")
        || lower.contains("sqlite_busy")
        || lower.contains("http 408")
        || lower.contains("http 429")
        || lower.contains("http 502")
        || lower.contains("http 503")
        || lower.contains("http 504")
        || lower.contains(" 408 ")
        || lower.contains(" 429 ")
        || lower.contains(" 502 ")
        || lower.contains(" 503 ")
        || lower.contains(" 504 ")
        || lower.contains("anysearch unavailable")
}

fn failure_code(subject: &str, class: FailureClass) -> String {
    let subject = subject
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!(
        "{}_{}",
        subject.trim_matches('_'),
        match class {
            FailureClass::Transient => "transient",
            FailureClass::PermissionRequired => "permission",
            FailureClass::InputRequired => "input",
            FailureClass::Conflict => "conflict",
            FailureClass::Interrupted => "interrupted",
            FailureClass::Validation => "validation",
            FailureClass::Permanent => "permanent",
        }
    )
}

fn user_facing_message(class: FailureClass, raw: &str) -> String {
    let compact = truncate(raw.trim(), 180);
    match class {
        FailureClass::Transient => {
            if compact.is_empty() {
                "遇到临时故障，系统会保守自动重试。".into()
            } else {
                format!("遇到临时故障：{compact}")
            }
        }
        FailureClass::PermissionRequired => {
            if compact.is_empty() {
                "需要你确认权限后才能继续。".into()
            } else {
                format!("需要你确认权限：{compact}")
            }
        }
        FailureClass::InputRequired => {
            if compact.is_empty() {
                "需要你补充信息后才能继续。".into()
            } else {
                format!("需要你补充信息：{compact}")
            }
        }
        FailureClass::Conflict => {
            if compact.is_empty() {
                "自动合回出现冲突，需要你处理。".into()
            } else {
                format!("出现冲突：{compact}")
            }
        }
        FailureClass::Interrupted => {
            if compact.is_empty() {
                "执行被中断，外部副作用状态未知，需要你核对后恢复。".into()
            } else {
                format!("执行被中断：{compact}")
            }
        }
        FailureClass::Validation => {
            if compact.is_empty() {
                "请求参数无效，请修正后重试。".into()
            } else {
                format!("参数无效：{compact}")
            }
        }
        FailureClass::Permanent => {
            if compact.is_empty() {
                "操作失败，且不适合自动重试。".into()
            } else {
                format!("操作失败：{compact}")
            }
        }
    }
}

fn truncate(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_transient_and_permission_errors() {
        assert_eq!(
            classify_error_message("HTTP 503 service unavailable"),
            FailureClass::Transient
        );
        assert_eq!(
            classify_error_message("database is busy; please retry"),
            FailureClass::Transient
        );
        assert_eq!(
            classify_error_message("用户拒绝了工具执行"),
            FailureClass::PermissionRequired
        );
        assert_eq!(
            classify_error_message("Invalid arguments for tool 'read_file'"),
            FailureClass::Validation
        );
    }

    #[test]
    fn auto_retry_only_for_idempotent_transient_tools() {
        let failure =
            classify_tool_failure("web_query", "HTTP 503", FailureSource::ForegroundTool, 0, 2);
        assert!(failure.can_auto_retry(ToolRetryPolicy {
            allow_auto_retry: true,
            max_attempts: 2,
        }));
        assert!(!failure.can_auto_retry(ToolRetryPolicy::default()));

        let permanent = classify_tool_failure(
            "web_query",
            "unknown backend bug",
            FailureSource::ForegroundTool,
            0,
            2,
        );
        assert!(!permanent.can_auto_retry(ToolRetryPolicy {
            allow_auto_retry: true,
            max_attempts: 2,
        }));
    }

    #[test]
    fn retry_after_is_capped() {
        assert_eq!(parse_retry_after_secs("Retry-After: 120"), Some(30));
        assert_eq!(retry_delay(0, Some(120)), Duration::from_secs(30));
        assert_eq!(retry_delay(0, None), Duration::from_millis(500));
    }

    #[test]
    fn runtime_recovery_required_is_interrupted() {
        let envelope = classify_runtime_failure(
            "recovery_required",
            "daemon restarted before worker launch completed",
            0,
            2,
        );
        assert_eq!(envelope.class, FailureClass::Interrupted);
        assert_eq!(envelope.disposition, FailureDisposition::UserActionRequired);
    }

    #[test]
    fn classified_failures_do_not_claim_auto_retry_until_scheduled() {
        let transient =
            classify_tool_failure("web_query", "HTTP 503", FailureSource::ForegroundTool, 0, 2);
        assert_eq!(transient.disposition, FailureDisposition::Terminal);
        assert!(transient.can_auto_retry(ToolRetryPolicy {
            allow_auto_retry: true,
            max_attempts: 2,
        }));

        let recovering = transient
            .clone()
            .mark_auto_retrying(Some("2026-08-02T00:00:00Z".into()));
        assert_eq!(recovering.disposition, FailureDisposition::AutoRetrying);
        assert!(recovering.actions.is_empty());
    }
}
