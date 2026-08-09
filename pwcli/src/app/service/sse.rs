use std::convert::Infallible;

use axum::response::sse::Event;
use futures::Stream;

use crate::ai::llm::models::StreamEvent;

/// SSE stream type alias used by service handlers.
pub type SseStream = axum::response::Sse<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Convert an internal [`StreamEvent`] into an axum SSE [`Event`].
pub fn stream_event_to_sse(event: StreamEvent) -> Event {
    match event {
        StreamEvent::FirstToken => Event::default().event("first_token").data("{}"),
        StreamEvent::TextDelta(delta) => Event::default()
            .event("text_delta")
            .data(format!(r#"{{"delta":{}}}"#, serde_json::json!(delta))),
        StreamEvent::AssistantSegmentStart { round } => Event::default()
            .event("assistant_segment_start")
            .data(serde_json::json!({ "round": round }).to_string()),
        StreamEvent::AssistantSegmentEnd {
            round,
            has_tool_calls,
        } => Event::default().event("assistant_segment_end").data(
            serde_json::json!({
                "round": round,
                "has_tool_calls": has_tool_calls,
            })
            .to_string(),
        ),
        StreamEvent::AssistantSegmentClassified { round, kind } => Event::default()
            .event("assistant_segment_classified")
            .data(serde_json::json!({ "round": round, "kind": kind }).to_string()),
        StreamEvent::AssistantCandidateDisposition { round, disposition } => Event::default()
            .event("assistant_candidate_disposition")
            .data(serde_json::json!({ "round": round, "disposition": disposition }).to_string()),
        StreamEvent::RuntimeUpdate {
            call_index,
            thinking_level,
        } => Event::default().event("runtime_update").data(
            serde_json::json!({
                "call_index": call_index,
                "thinking_level": thinking_level,
            })
            .to_string(),
        ),
        StreamEvent::ThinkingDelta(delta) => Event::default()
            .event("thinking_delta")
            .data(format!(r#"{{"delta":{}}}"#, serde_json::json!(delta))),
        StreamEvent::ToolCallStart { id, name, .. } => {
            Event::default().event("tool_call_start").data(format!(
                r#"{{"id":{},"name":{}}}"#,
                serde_json::json!(id),
                serde_json::json!(name)
            ))
        }
        StreamEvent::ToolCallDelta {
            id,
            arguments_delta,
        } => Event::default().event("tool_call_delta").data(format!(
            r#"{{"id":{},"arguments_delta":{}}}"#,
            serde_json::json!(id),
            serde_json::json!(arguments_delta)
        )),
        StreamEvent::ToolCallEnd { id } => Event::default()
            .event("tool_call_end")
            .data(format!(r#"{{"id":{}}}"#, serde_json::json!(id))),
        StreamEvent::ToolResult {
            id,
            name,
            result,
            is_error,
            failure,
        } => Event::default().event("tool_result").data(format!(
            r#"{{"id":{},"name":{},"result":{},"is_error":{},"failure":{}}}"#,
            serde_json::json!(id),
            serde_json::json!(name),
            serde_json::json!(result),
            is_error,
            serde_json::to_string(&failure).unwrap_or_else(|_| "null".into())
        )),
        StreamEvent::ToolRecovery {
            id,
            name,
            failure,
            phase,
        } => Event::default().event("tool_recovery").data(format!(
            r#"{{"id":{},"name":{},"phase":{},"failure":{}}}"#,
            serde_json::json!(id),
            serde_json::json!(name),
            serde_json::json!(phase),
            serde_json::to_string(&failure).unwrap_or_else(|_| "null".into())
        )),
        StreamEvent::ToolProgress { id, line } => {
            Event::default().event("tool_progress").data(format!(
                r#"{{"id":{},"line":{}}}"#,
                serde_json::json!(id),
                serde_json::json!(line)
            ))
        }
        StreamEvent::ToolImage {
            id,
            url,
            alt,
            record,
        } => Event::default().event("tool_image").data(format!(
            r#"{{"id":{},"url":{},"alt":{},"record":{}}}"#,
            serde_json::json!(id),
            serde_json::json!(url),
            serde_json::json!(alt),
            serde_json::to_string(&record).unwrap_or_else(|_| "null".into())
        )),
        StreamEvent::ToolDocument { id, document } => Event::default()
            .event("tool_document")
            .data(serde_json::json!({ "id": id, "document": document }).to_string()),
        StreamEvent::ToolDecision { id, decision } => Event::default()
            .event("tool_decision")
            .data(serde_json::json!({ "id": id, "decision": decision }).to_string()),
        StreamEvent::ResponseStop(reason) => Event::default()
            .event("response_stop")
            .data(serde_json::to_string(&reason).unwrap_or_else(|_| "null".to_string())),
        StreamEvent::ContextUsage { usage, call_index } => {
            Event::default().event("context_usage").data(
                serde_json::json!({
                    "prompt_tokens": usage.prompt_tokens,
                    "completion_tokens": usage.completion_tokens,
                    "total_tokens": usage.total_tokens,
                    "call_index": call_index,
                    "source": "provider",
                })
                .to_string(),
            )
        }
        StreamEvent::Done(Some(usage)) => Event::default()
            .event("done")
            .data(serde_json::to_string(&usage).unwrap_or_else(|_| "{}".to_string())),
        StreamEvent::Done(None) => Event::default().event("done").data("{}"),
        StreamEvent::Error(msg) => Event::default()
            .event("error")
            .data(format!(r#"{{"message":{}}}"#, serde_json::json!(msg))),
        StreamEvent::StreamReset { reason } => Event::default()
            .event("stream_reset")
            .data(serde_json::json!({ "reason": reason }).to_string()),
        StreamEvent::DecisionStarted { id, trigger, risk } => Event::default()
            .event("decision_started")
            .data(serde_json::json!({ "id": id, "trigger": trigger, "risk": risk }).to_string()),
        StreamEvent::DecisionAdvisor { id, model, status, round, summary } => Event::default()
            .event("decision_advisor")
            .data(serde_json::json!({ "id": id, "model": model, "status": status, "round": round, "summary": summary }).to_string()),
        StreamEvent::DecisionResolved {
            id,
            outcome,
            confidence,
            consensus,
            rationale,
        } => Event::default().event("decision_resolved").data(
            serde_json::json!({
                "id": id,
                "outcome": outcome,
                "confidence": confidence,
                "consensus": consensus,
                "rationale": rationale,
            })
            .to_string(),
        ),
        StreamEvent::DecisionEscalated {
            id,
            rationale,
            options,
        } => Event::default().event("decision_escalated").data(
            serde_json::json!({ "id": id, "rationale": rationale, "options": options }).to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::models::TokenUsage;

    /// Serialize a single [`Event`] through the axum SSE pipeline and return its wire bytes.
    async fn event_to_bytes(event: Event) -> bytes::Bytes {
        use axum::response::sse::Sse;
        use axum::response::IntoResponse;

        let sse = Sse::new(futures::stream::iter(vec![Ok::<_, Infallible>(event)]));
        let response = sse.into_response();
        let body = response.into_body();
        http_body_util::BodyExt::collect(body)
            .await
            .unwrap()
            .to_bytes()
    }

    /// Parse a raw SSE payload into a map of field-name -> value.
    fn parse_sse_fields(text: &str) -> std::collections::HashMap<String, String> {
        let mut fields = std::collections::HashMap::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once(": ").unwrap_or((line, ""));
            let key = if key.is_empty() { "comment" } else { key };
            fields.insert(key.to_string(), value.to_string());
        }
        fields
    }

    #[tokio::test]
    async fn test_first_token() {
        let evt = stream_event_to_sse(StreamEvent::FirstToken);
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "first_token");
        assert_eq!(fields.get("data").unwrap(), "{}");
    }

    #[tokio::test]
    async fn test_text_delta() {
        let evt = stream_event_to_sse(StreamEvent::TextDelta("hello".to_string()));
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "text_delta");
        assert_eq!(fields.get("data").unwrap(), r#"{"delta":"hello"}"#);
    }

    #[tokio::test]
    async fn test_text_delta_with_quotes() {
        let evt = stream_event_to_sse(StreamEvent::TextDelta(r#"say "hello""#.to_string()));
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "text_delta");
        assert_eq!(fields.get("data").unwrap(), r#"{"delta":"say \"hello\""}"#);
    }

    #[tokio::test]
    async fn test_assistant_segment_boundaries() {
        let start = stream_event_to_sse(StreamEvent::AssistantSegmentStart { round: 2 });
        let start_bytes = event_to_bytes(start).await;
        let start_fields = parse_sse_fields(std::str::from_utf8(&start_bytes).unwrap());
        assert_eq!(
            start_fields.get("event").unwrap(),
            "assistant_segment_start"
        );
        assert_eq!(start_fields.get("data").unwrap(), r#"{"round":2}"#);

        let end = stream_event_to_sse(StreamEvent::AssistantSegmentEnd {
            round: 2,
            has_tool_calls: true,
        });
        let end_bytes = event_to_bytes(end).await;
        let end_fields = parse_sse_fields(std::str::from_utf8(&end_bytes).unwrap());
        assert_eq!(end_fields.get("event").unwrap(), "assistant_segment_end");
        assert_eq!(
            end_fields.get("data").unwrap(),
            r#"{"has_tool_calls":true,"round":2}"#
        );

        let classified = stream_event_to_sse(StreamEvent::AssistantSegmentClassified {
            round: 2,
            kind: "candidate".into(),
        });
        let classified_bytes = event_to_bytes(classified).await;
        let classified_fields = parse_sse_fields(std::str::from_utf8(&classified_bytes).unwrap());
        assert_eq!(
            classified_fields.get("event").unwrap(),
            "assistant_segment_classified"
        );

        let promoted = stream_event_to_sse(StreamEvent::AssistantCandidateDisposition {
            round: 2,
            disposition: "promoted".into(),
        });
        let promoted_bytes = event_to_bytes(promoted).await;
        let promoted_fields = parse_sse_fields(std::str::from_utf8(&promoted_bytes).unwrap());
        assert_eq!(
            promoted_fields.get("event").unwrap(),
            "assistant_candidate_disposition"
        );
    }

    #[tokio::test]
    async fn test_stream_reset() {
        let evt = stream_event_to_sse(StreamEvent::StreamReset {
            reason: "repeated block".into(),
        });
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "stream_reset");
        assert_eq!(
            fields.get("data").unwrap(),
            r#"{"reason":"repeated block"}"#
        );
    }

    #[tokio::test]
    async fn test_tool_call_start() {
        let evt = stream_event_to_sse(StreamEvent::ToolCallStart {
            id: "call_1".to_string(),
            name: "search".to_string(),
            thought_signature: None,
        });
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "tool_call_start");
        assert_eq!(
            fields.get("data").unwrap(),
            r#"{"id":"call_1","name":"search"}"#
        );
    }

    #[tokio::test]
    async fn test_tool_call_delta() {
        let evt = stream_event_to_sse(StreamEvent::ToolCallDelta {
            id: "call_1".to_string(),
            arguments_delta: r#"{"q": "rust"}"#.to_string(),
        });
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "tool_call_delta");
        assert_eq!(
            fields.get("data").unwrap(),
            r#"{"id":"call_1","arguments_delta":"{\"q\": \"rust\"}"}"#
        );
    }

    #[tokio::test]
    async fn test_tool_call_end() {
        let evt = stream_event_to_sse(StreamEvent::ToolCallEnd {
            id: "call_1".to_string(),
        });
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "tool_call_end");
        assert_eq!(fields.get("data").unwrap(), r#"{"id":"call_1"}"#);
    }

    #[tokio::test]
    async fn test_done_with_usage() {
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        };
        let evt = stream_event_to_sse(StreamEvent::Done(Some(usage)));
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "done");
        assert_eq!(
            fields.get("data").unwrap(),
            r#"{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}"#
        );
    }

    #[tokio::test]
    async fn test_context_usage_is_distinct_from_done() {
        let evt = stream_event_to_sse(StreamEvent::ContextUsage {
            usage: TokenUsage {
                prompt_tokens: 120,
                completion_tokens: 8,
                total_tokens: 128,
            },
            call_index: 3,
        });
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "context_usage");
        let data: serde_json::Value = serde_json::from_str(fields.get("data").unwrap()).unwrap();
        assert_eq!(data["prompt_tokens"], 120);
        assert_eq!(data["call_index"], 3);
        assert_eq!(data["source"], "provider");
    }

    #[tokio::test]
    async fn test_done_without_usage() {
        let evt = stream_event_to_sse(StreamEvent::Done(None));
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "done");
        assert_eq!(fields.get("data").unwrap(), "{}");
    }

    #[tokio::test]
    async fn test_error() {
        let evt = stream_event_to_sse(StreamEvent::Error("something went wrong".to_string()));
        let bytes = event_to_bytes(evt).await;
        let fields = parse_sse_fields(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(fields.get("event").unwrap(), "error");
        assert_eq!(
            fields.get("data").unwrap(),
            r#"{"message":"something went wrong"}"#
        );
    }
}
