use std::collections::BTreeMap;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{HarnessFingerprint, HarnessSpec};

pub const MAX_INTERVENTION_DETAILS_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MiddlewareHook {
    BeforeTurn,
    BeforeLlm,
    AfterLlm,
    AfterTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionKind {
    ControlFlow,
    StateMutation,
    RequestMutation,
    ResultMutation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MiddlewareIntervention {
    pub spec_fingerprint: HarnessFingerprint,
    pub middleware: String,
    pub middleware_revision: u32,
    pub hook: MiddlewareHook,
    pub kind: InterventionKind,
    pub reason_code: String,
    pub round: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub details: BTreeMap<String, Value>,
}

impl MiddlewareIntervention {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        spec_fingerprint: HarnessFingerprint,
        middleware: impl Into<String>,
        middleware_revision: u32,
        hook: MiddlewareHook,
        kind: InterventionKind,
        reason_code: impl Into<String>,
        round: u32,
        tool_call_id: Option<String>,
        details: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            spec_fingerprint,
            middleware: middleware.into(),
            middleware_revision,
            hook,
            kind,
            reason_code: reason_code.into(),
            round,
            tool_call_id,
            details: bound_details(details),
        }
    }
}

#[async_trait]
pub trait HarnessAuditSink: Send + Sync {
    async fn record_spec(&self, spec: &HarnessSpec, fingerprint: &HarnessFingerprint)
        -> Result<()>;

    async fn record_intervention(&self, intervention: &MiddlewareIntervention) -> Result<()>;
}

fn bound_details(details: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let details = sanitize_details(details);
    if serde_json::to_vec(&details)
        .map(|bytes| bytes.len() <= MAX_INTERVENTION_DETAILS_BYTES)
        .unwrap_or(false)
    {
        return details;
    }
    BTreeMap::from([
        ("details_truncated".to_string(), Value::Bool(true)),
        (
            "original_field_count".to_string(),
            Value::from(details.len() as u64),
        ),
    ])
}

fn sanitize_details(details: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    details
        .into_iter()
        .map(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            let sensitive = [
                "api_key",
                "apikey",
                "authorization",
                "credential",
                "password",
                "prompt",
                "secret",
                "token",
                "content",
                "message",
            ]
            .iter()
            .any(|needle| normalized.contains(needle));
            if sensitive {
                (key, Value::String("[redacted]".to_string()))
            } else {
                (key, sanitize_value(value))
            }
        })
        .collect()
}

fn sanitize_value(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            sanitize_details(object.into_iter().collect())
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_value).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_details_are_replaced_with_bounded_metadata() {
        let details = BTreeMap::from([("unsafe".into(), Value::String("x".repeat(3_000)))]);
        let bounded = bound_details(details);
        assert_eq!(bounded.get("details_truncated"), Some(&Value::Bool(true)));
        assert!(serde_json::to_vec(&bounded).unwrap().len() <= MAX_INTERVENTION_DETAILS_BYTES);
    }

    #[test]
    fn sensitive_detail_fields_are_redacted_recursively() {
        let details = BTreeMap::from([
            ("reason".into(), Value::String("loop".into())),
            ("prompt_excerpt".into(), Value::String("private".into())),
            (
                "nested".into(),
                serde_json::json!({"api_key": "secret", "count": 2}),
            ),
        ]);
        let bounded = bound_details(details);
        assert_eq!(bounded["prompt_excerpt"], "[redacted]");
        assert_eq!(bounded["nested"]["api_key"], "[redacted]");
        assert_eq!(bounded["nested"]["count"], 2);
    }
}
