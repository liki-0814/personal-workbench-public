use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Retryable,
    ContextOverflow,
    Fatal,
}

pub const MAX_ATTEMPTS: u32 = 5;
const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 5000;

const BILLING_OR_AUTH: &[&str] = &[
    "insufficient_quota",
    "billing",
    "payment required",
    "subscription",
    "invalid api key",
    "unauthorized",
    "forbidden",
];
const OVERFLOW: &[&str] = &[
    "context_length_exceeded",
    "context window",
    "maximum context length",
    "too many tokens",
    "prompt is too long",
    "request too large",
    "exceeds the model limit",
    "input length",
];
const TRANSIENT: &[&str] = &[
    "模型提供方错误",
    "internal server error",
    "bad gateway",
    "gateway timeout",
    "service unavailable",
    "overloaded",
    "rate limit",
    "rate_limit",
    "throttl",
    "allocated quota exceeded",
    "connection reset",
    "connection refused",
    "timed out",
    "timeout",
    "premature",
    "unexpected eof",
    "上游服务错误",
    "服务暂时不可用",
];

pub fn classify(status: Option<u16>, message: &str) -> ErrorClass {
    let lower = message.to_lowercase();
    if BILLING_OR_AUTH.iter().any(|needle| lower.contains(needle)) {
        return ErrorClass::Fatal;
    }
    if OVERFLOW.iter().any(|needle| lower.contains(needle)) {
        return ErrorClass::ContextOverflow;
    }
    if status == Some(429)
        || status.is_some_and(|code| code >= 500)
        || TRANSIENT.iter().any(|needle| lower.contains(needle))
    {
        return ErrorClass::Retryable;
    }
    ErrorClass::Fatal
}

pub fn next_backoff(attempt: u32) -> Option<Duration> {
    if attempt + 1 >= MAX_ATTEMPTS {
        return None;
    }
    Some(Duration::from_millis(
        (INITIAL_BACKOFF_MS << attempt).min(MAX_BACKOFF_MS),
    ))
}

/// Retry delay for one incident. A provider hint may lengthen the local exponential
/// backoff, but never creates an extra attempt after the incident budget is exhausted.
pub fn next_backoff_with_hint(attempt: u32, retry_after: Option<&str>) -> Option<Duration> {
    let local = next_backoff(attempt)?;
    let hinted = retry_after.and_then(parse_retry_after);
    Some(hinted.map_or(local, |hint| hint.max(local)))
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(300)));
    }
    let deadline = chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()?
        .with_timezone(&chrono::Utc);
    let remaining = deadline.signed_duration_since(chrono::Utc::now());
    Some(Duration::from_secs(remaining.num_seconds().max(0) as u64))
}

pub fn is_provider_error(status: u16, body: &str) -> bool {
    classify(Some(status), body) == ErrorClass::Retryable
}

pub fn is_context_overflow(message: &str) -> bool {
    classify(None, message) == ErrorClass::ContextOverflow
}

pub fn is_retriable_stream_error(message: &str) -> bool {
    classify(None, message) == ErrorClass::Retryable
}

pub fn is_retriable_subprocess_failure(message: &str) -> bool {
    classify(None, message) == ErrorClass::Retryable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_http_and_transport_errors() {
        assert_eq!(classify(Some(429), "rate limited"), ErrorClass::Retryable);
        assert_eq!(classify(Some(503), "unavailable"), ErrorClass::Retryable);
        assert_eq!(
            classify(None, "connection reset by peer"),
            ErrorClass::Retryable
        );
        assert_eq!(classify(Some(400), "invalid request"), ErrorClass::Fatal);
    }

    #[test]
    fn billing_wins_over_transient_words() {
        assert_eq!(
            classify(Some(429), "rate limit: insufficient_quota; update billing"),
            ErrorClass::Fatal
        );
    }

    #[test]
    fn allocation_throttling_is_retryable_but_billing_quota_is_not() {
        assert_eq!(
            classify(
                Some(429),
                "Throttling.AllocationQuota: Allocated quota exceeded"
            ),
            ErrorClass::Retryable
        );
        assert_eq!(
            classify(Some(429), "rate limit: insufficient_quota; update billing"),
            ErrorClass::Fatal
        );
    }

    #[test]
    fn detects_context_overflow_separately() {
        assert!(is_context_overflow("maximum context length exceeded"));
        assert!(!is_provider_error(
            400,
            "max_tokens exceeds the model limit"
        ));
    }

    #[test]
    fn stream_and_subprocess_share_classifier() {
        assert!(is_retriable_stream_error("Internal server error"));
        assert!(is_retriable_subprocess_failure("Bad Gateway: 502"));
        assert!(!is_retriable_stream_error("invalid_request_error"));
    }

    #[test]
    fn backoff_caps_at_five_attempts() {
        assert_eq!(next_backoff(0), Some(Duration::from_millis(500)));
        assert_eq!(next_backoff(3), Some(Duration::from_millis(4000)));
        assert_eq!(next_backoff(4), None);
    }

    #[test]
    fn retry_after_hint_can_lengthen_incident_backoff() {
        assert_eq!(
            next_backoff_with_hint(0, Some("3")),
            Some(Duration::from_secs(3))
        );
        assert_eq!(next_backoff_with_hint(4, Some("30")), None);
    }
}
