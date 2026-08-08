//! ChatGPT subscription-backed Codex Responses protocol.
//!
//! The wire contract differs from the public OpenAI Responses API: it uses a
//! dedicated endpoint, requires the ChatGPT account id, and only supports the
//! streaming response flow used here. The protocol behavior is based on pi's
//! MIT-licensed `openai-codex-responses` provider, without its WebSocket/zstd
//! transport optimizations.

use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use reqwest::{header::HeaderMap, Client, Response};
use serde_json::{json, Value};
use tracing::debug;

use crate::ai::config::ProviderConfig;
use crate::ai::llm::models::*;

use super::LlmAdapter;

pub struct OpenAiCodexResponsesAdapter {
    http: Client,
    provider: ProviderConfig,
    session_id: Option<String>,
}

impl OpenAiCodexResponsesAdapter {
    pub fn new(provider: ProviderConfig, session_id: Option<String>) -> Self {
        Self {
            http: crate::ai::http::default_client(crate::ai::http::ClientProfile::Llm),
            provider,
            session_id,
        }
    }

    fn endpoint(&self) -> String {
        let base = self.provider.base_url.trim_end_matches('/');
        if base.ends_with("/codex/responses") {
            base.to_string()
        } else if base.ends_with("/codex") {
            format!("{base}/responses")
        } else {
            format!("{base}/codex/responses")
        }
    }

    fn headers(&self) -> Result<HeaderMap> {
        let account_id = crate::ai::provider::extract_codex_account_id(&self.provider.api_key)
            .context("Codex OAuth token does not contain a ChatGPT account id; login again")?;
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.provider.api_key).parse()?,
        );
        headers.insert("chatgpt-account-id", account_id.parse()?);
        headers.insert("originator", "pwcli".parse()?);
        headers.insert(
            "User-Agent",
            format!("pwcli/{}", env!("CARGO_PKG_VERSION")).parse()?,
        );
        headers.insert("OpenAI-Beta", "responses=experimental".parse()?);
        headers.insert("Accept", "text/event-stream".parse()?);
        headers.insert("Content-Type", "application/json".parse()?);
        if let Some(session_id) = self.session_id.as_deref().filter(|value| !value.is_empty()) {
            headers.insert("session-id", session_id.parse()?);
            headers.insert("x-client-request-id", session_id.parse()?);
        }
        Ok(headers)
    }

    fn build_payload(&self, request: &LlmRequest) -> Value {
        let mut instructions = request.system_prompt.clone().unwrap_or_default();
        let mut input = Vec::new();
        for message in &request.messages {
            match message.role.as_str() {
                "system" => {
                    if !message.content.is_empty() {
                        if !instructions.is_empty() {
                            instructions.push_str("\n\n");
                        }
                        instructions.push_str(&message.content);
                    }
                }
                _ => input.extend(super::responses_wire::message_items(message, true)),
            }
        }

        let mut payload = json!({
            "model": self.provider.model,
            "store": false,
            "stream": true,
            "instructions": if instructions.is_empty() { "You are a helpful assistant." } else { &instructions },
            "input": input,
            "text": {"verbosity": "low"},
            "include": ["reasoning.encrypted_content"],
            "tool_choice": parse_tool_choice(request.tool_choice.as_deref()),
            "parallel_tool_calls": true,
        });
        if let Some(session_id) = self.session_id.as_deref().filter(|value| !value.is_empty()) {
            payload["prompt_cache_key"] = json!(session_id);
        }
        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if let Some(tools) = &request.tools {
            payload["tools"] = Value::Array(super::responses_wire::function_tools(tools));
        }
        if request.thinking {
            if let Some(params) = self
                .provider
                .current_model_entry()
                .and_then(|model| model.thinking_params.as_ref())
            {
                if let Some(object) = payload.as_object_mut() {
                    object.extend(params.clone());
                }
            } else {
                payload["reasoning"] = json!({"effort": "medium", "summary": "auto"});
            }
        }
        if let Some(params) = self
            .provider
            .current_model_entry()
            .and_then(|model| model.request_params.as_ref())
        {
            if let Some(object) = payload.as_object_mut() {
                object.extend(params.clone());
            }
        }
        // Provider request params may tune behavior, but cannot change the
        // subscription endpoint's transport and retention invariants.
        payload["store"] = json!(false);
        payload["stream"] = json!(true);
        payload
    }

    fn stream_request(&self, request: LlmRequest) -> BoxStream<'static, StreamEvent> {
        let http = self.http.clone();
        let provider = self.provider.clone();
        let session_id = self.session_id.clone();
        Box::pin(async_stream::stream! {
            let adapter = OpenAiCodexResponsesAdapter { http, provider, session_id };
            let headers = match adapter.headers() {
                Ok(headers) => headers,
                Err(error) => {
                    yield StreamEvent::Error(error.to_string());
                    return;
                }
            };
            let url = adapter.endpoint();
            let payload = adapter.build_payload(&request);
            debug!(model = %adapter.provider.model, protocol = "openai_codex_responses", %url, "Codex Responses stream request");
            let response = match adapter.http.post(&url).headers(headers).json(&payload).send().await {
                Ok(response) => response,
                Err(error) => {
                    yield StreamEvent::Error(format!("Codex request failed: {error}"));
                    return;
                }
            };
            if !response.status().is_success() {
                yield StreamEvent::Error(codex_http_error(response).await);
                return;
            }

            yield StreamEvent::FirstToken;
            let mut body = response.bytes_stream();
            let mut buffer = String::new();
            let mut item_to_call = HashMap::<String, String>::new();
            let mut pending_reasoning = None::<String>;
            let mut completed = false;
            while let Some(chunk) = body.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        yield StreamEvent::Error(format!("Codex stream read failed: {error}"));
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                buffer = buffer.replace("\r\n", "\n");
                while let Some(position) = buffer.find("\n\n") {
                    let block = buffer[..position].to_string();
                    buffer.drain(..position + 2);
                    let data = block
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(&data) {
                        Ok(value) => value,
                        Err(error) => {
                            yield StreamEvent::Error(format!("Invalid Codex SSE JSON: {error}"));
                            return;
                        }
                    };
                    let event_type = value.get("type").and_then(Value::as_str).unwrap_or_default();
                    match event_type {
                        "response.output_text.delta" => {
                            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                                yield StreamEvent::TextDelta(delta.to_string());
                            }
                        }
                        "response.reasoning_summary_text.delta" | "response.reasoning.delta" => {
                            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                                yield StreamEvent::ThinkingDelta(delta.to_string());
                            }
                        }
                        "response.output_item.added" | "response.output_item.done" => {
                            if let Some(item) = value.get("item") {
                                match item.get("type").and_then(Value::as_str) {
                                    Some("reasoning") => {
                                        if let Some(encrypted) = item.get("encrypted_content").and_then(Value::as_str) {
                                            pending_reasoning = Some(encrypted.to_string());
                                        }
                                    }
                                    Some("function_call") if event_type.ends_with("added") => {
                                        let call_id = item.get("call_id").or_else(|| item.get("id"))
                                            .and_then(Value::as_str).unwrap_or_default().to_string();
                                        let item_id = item.get("id").and_then(Value::as_str).unwrap_or(&call_id).to_string();
                                        item_to_call.insert(item_id, call_id.clone());
                                        let name = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                                        yield StreamEvent::ToolCallStart {
                                            id: call_id,
                                            name,
                                            thought_signature: pending_reasoning.take(),
                                        };
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "response.function_call_arguments.delta" => {
                            let wire_id = value.get("call_id").or_else(|| value.get("item_id"))
                                .and_then(Value::as_str).unwrap_or_default();
                            let id = item_to_call.get(wire_id).cloned().unwrap_or_else(|| wire_id.to_string());
                            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                                yield StreamEvent::ToolCallDelta { id, arguments_delta: delta.to_string() };
                            }
                        }
                        "response.function_call_arguments.done" => {
                            let wire_id = value.get("call_id").or_else(|| value.get("item_id"))
                                .and_then(Value::as_str).unwrap_or_default();
                            let id = item_to_call.get(wire_id).cloned().unwrap_or_else(|| wire_id.to_string());
                            yield StreamEvent::ToolCallEnd { id };
                        }
                        "response.completed" | "response.done" | "response.incomplete" => {
                            let usage = codex_usage(&value);
                            let status = value.pointer("/response/status").and_then(Value::as_str)
                                .unwrap_or(if event_type == "response.incomplete" { "incomplete" } else { "completed" });
                            yield StreamEvent::ResponseStop(StopReason::from_provider(status));
                            yield StreamEvent::Done(usage);
                            completed = true;
                            break;
                        }
                        "response.failed" => {
                            yield StreamEvent::Error(codex_event_error(&value, "Codex response failed"));
                            return;
                        }
                        "error" => {
                            yield StreamEvent::Error(codex_event_error(&value, "Codex stream error"));
                            return;
                        }
                        _ => {}
                    }
                    if completed {
                        break;
                    }
                }
                if completed {
                    return;
                }
            }
            yield StreamEvent::Error("Codex stream ended before a completion event".into());
        })
    }
}

#[async_trait]
impl LlmAdapter for OpenAiCodexResponsesAdapter {
    fn protocol(&self) -> ProviderProtocol {
        ProviderProtocol::OpenAiCodexResponses
    }

    async fn chat(&self, request: &LlmRequest) -> Result<AiResponse> {
        let mut stream = self.stream_request(request.clone());
        let mut content = String::new();
        let mut usage = None;
        let mut active_calls = HashMap::<String, ToolCall>::new();
        let mut call_order = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::TextDelta(delta) => content.push_str(&delta),
                StreamEvent::ToolCallStart {
                    id,
                    name,
                    thought_signature,
                } => {
                    call_order.push(id.clone());
                    active_calls.insert(
                        id.clone(),
                        ToolCall {
                            id,
                            kind: "function".into(),
                            function: FunctionCall {
                                name,
                                arguments: String::new(),
                            },
                            thought_signature,
                        },
                    );
                }
                StreamEvent::ToolCallDelta {
                    id,
                    arguments_delta,
                } => {
                    if let Some(call) = active_calls.get_mut(&id) {
                        call.function.arguments.push_str(&arguments_delta);
                    }
                }
                StreamEvent::Done(value) => {
                    usage = value;
                    break;
                }
                StreamEvent::Error(error) => anyhow::bail!(error),
                _ => {}
            }
        }
        let tool_calls = call_order
            .into_iter()
            .filter_map(|id| active_calls.remove(&id))
            .collect::<Vec<_>>();
        Ok(AiResponse {
            content,
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            usage,
        })
    }

    fn chat_stream(&self, request: LlmRequest) -> BoxStream<'static, StreamEvent> {
        self.stream_request(request)
    }
}

fn parse_tool_choice(choice: Option<&str>) -> Value {
    choice
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_else(|| json!(choice.unwrap_or("auto")))
}

fn codex_usage(event: &Value) -> Option<TokenUsage> {
    event.pointer("/response/usage").map(|usage| TokenUsage {
        prompt_tokens: usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        completion_tokens: usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        total_tokens: usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    })
}

fn codex_event_error(value: &Value, fallback: &str) -> String {
    value
        .pointer("/error/message")
        .or_else(|| value.pointer("/response/error/message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

async fn codex_http_error(response: Response) -> String {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        let code = value
            .pointer("/error/code")
            .or_else(|| value.pointer("/error/type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status.as_u16() == 429
            || matches!(
                code,
                "usage_limit_reached" | "usage_not_included" | "rate_limit_exceeded"
            )
        {
            let plan = value
                .pointer("/error/plan_type")
                .and_then(Value::as_str)
                .map(|plan| format!(" ({plan} plan)"))
                .unwrap_or_default();
            return format!("ChatGPT usage limit reached{plan}; login with another account or wait for the limit to reset");
        }
        if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
            return format!("Codex error {status}: {message}");
        }
    }
    format!("Codex error {status}: {text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    fn provider(base_url: &str) -> ProviderConfig {
        // JWT payload: {"https://api.openai.com/auth":{"chatgpt_account_id":"acct_1"}}
        ProviderConfig {
            name: "codex".into(),
            base_url: base_url.into(),
            api_key: "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdF8xIn19.sig".into(),
            protocol: "openai_codex_responses".into(),
            model: "gpt-5.4".into(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: Some("builtin:openai-codex".into()),
        }
    }

    #[test]
    fn builds_codex_endpoint_headers_and_invariants() {
        let adapter = OpenAiCodexResponsesAdapter::new(
            provider("https://chatgpt.com/backend-api"),
            Some("session-1".into()),
        );
        assert_eq!(
            adapter.endpoint(),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        let headers = adapter.headers().unwrap();
        assert_eq!(headers["chatgpt-account-id"], "acct_1");
        assert_eq!(headers["originator"], "pwcli");
        assert_eq!(headers["session-id"], "session-1");
        let payload = adapter.build_payload(&LlmRequest {
            messages: vec![],
            system_prompt: Some("system".into()),
            tools: None,
            stream: false,
            tool_choice: None,
            thinking: true,
            max_tokens: Some(100),
            temperature: None,
        });
        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["instructions"], "system");
        assert_eq!(payload["prompt_cache_key"], "session-1");
        assert_eq!(payload["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn round_trips_encrypted_reasoning_with_tool_call() {
        let adapter = OpenAiCodexResponsesAdapter::new(
            provider("https://example.test/codex/responses"),
            None,
        );
        let payload = adapter.build_payload(&LlmRequest {
            messages: vec![ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "read_file".into(),
                        arguments: "{}".into(),
                    },
                    thought_signature: Some("encrypted".into()),
                }]),
                tool_call_id: None,
            }],
            system_prompt: None,
            tools: None,
            stream: true,
            tool_choice: None,
            thinking: false,
            max_tokens: None,
            temperature: None,
        });
        assert_eq!(payload["input"][0]["type"], "reasoning");
        assert_eq!(payload["input"][0]["encrypted_content"], "encrypted");
        assert_eq!(payload["input"][1]["type"], "function_call");
    }

    #[tokio::test]
    async fn parses_codex_sse_and_sends_subscription_headers() {
        let mut server = Server::new_async().await;
        let response = [
            r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","encrypted_content":"enc_1"}}"#,
            r#"data: {"type":"response.output_item.added","item":{"type":"function_call","id":"item_1","call_id":"call_1","name":"read_file"}}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"item_1","delta":"{\"path\":"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"item_1","delta":"\"a.rs\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","item_id":"item_1"}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}"#,
        ]
        .join("\n\n");
        let request = server
            .mock("POST", "/codex/responses")
            .match_header("authorization", mockito::Matcher::Regex("^Bearer ".into()))
            .match_header("chatgpt-account-id", "acct_1")
            .match_header("originator", "pwcli")
            .match_header("session-id", "session-1")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(format!("{response}\n\n"))
            .create_async()
            .await;
        let adapter =
            OpenAiCodexResponsesAdapter::new(provider(&server.url()), Some("session-1".into()));
        let events = adapter
            .chat_stream(LlmRequest {
                messages: Vec::new(),
                system_prompt: None,
                tools: None,
                stream: true,
                tool_choice: None,
                thinking: true,
                max_tokens: None,
                temperature: None,
            })
            .collect::<Vec<_>>()
            .await;

        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolCallStart {
                id,
                name,
                thought_signature: Some(signature),
            } if id == "call_1" && name == "read_file" && signature == "enc_1"
        )));
        let arguments = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ToolCallDelta {
                    id,
                    arguments_delta,
                } if id == "call_1" => Some(arguments_delta.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(arguments, r#"{"path":"a.rs"}"#);
        assert!(events.iter().any(
            |event| matches!(event, StreamEvent::Done(Some(usage)) if usage.total_tokens == 5)
        ));
        request.assert_async().await;
    }
}
