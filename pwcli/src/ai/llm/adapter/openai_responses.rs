use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::ai::config::ProviderConfig;
use crate::ai::llm::models::*;

use super::responses_wire;
use super::LlmAdapter;

pub struct OpenAiResponsesAdapter {
    http: Client,
    provider: ProviderConfig,
    backend_url: String,
    session_id: Option<String>,
}

impl OpenAiResponsesAdapter {
    pub fn new(provider: ProviderConfig, backend_url: String, session_id: Option<String>) -> Self {
        Self {
            http: crate::ai::http::default_client(crate::ai::http::ClientProfile::Llm),
            provider,
            backend_url,
            session_id,
        }
    }

    fn endpoint_and_headers(&self) -> Result<(String, reqwest::header::HeaderMap)> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse()?);
        if self.provider.uses_proxy() {
            let url = format!("{}/api/proxy/responses", self.backend_url);
            headers.insert(
                "X-Provider-Id",
                crate::ai::provider::provider_id(&self.provider).parse()?,
            );
            Ok((url, headers))
        } else {
            let base = self.provider.base_url.trim_end_matches('/');
            // Accept either https://api.openai.com/v1 or a responses-ready base.
            let url = if base.ends_with("/responses") {
                base.to_string()
            } else {
                format!("{base}/responses")
            };
            headers.insert(
                "Authorization",
                format!("Bearer {}", self.provider.api_key).parse()?,
            );
            Ok((url, headers))
        }
    }

    fn build_payload(&self, request: &LlmRequest, stream: bool) -> Result<Value> {
        let mut input = Vec::new();
        if let Some(system) = request.system_prompt.as_deref().filter(|s| !s.is_empty()) {
            input.push(responses_wire::system_item(system));
        }
        for message in &request.messages {
            if message.role == "system" {
                if !message.content.is_empty() {
                    input.push(responses_wire::system_item(&message.content));
                }
            } else {
                input.extend(responses_wire::message_items(message, false));
            }
        }

        let mut payload = json!({
            "model": self.provider.model,
            "input": input,
            "stream": stream,
            "max_output_tokens": self.provider.effective_max_tokens(request.max_tokens),
        });
        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if let Some(tools) = &request.tools {
            payload["tools"] = Value::Array(responses_wire::function_tools(tools));
        }
        if request.thinking {
            if let Some(params) = self
                .provider
                .current_model_entry()
                .and_then(|model| model.thinking_params.as_ref())
            {
                if let Some(object) = payload.as_object_mut() {
                    for (key, value) in params {
                        object.insert(key.clone(), value.clone());
                    }
                }
            } else {
                payload["reasoning"] = json!({"effort": "medium"});
            }
        }
        if let Some(params) = self
            .provider
            .current_model_entry()
            .and_then(|model| model.request_params.as_ref())
        {
            if let Some(object) = payload.as_object_mut() {
                for (key, value) in params {
                    object.insert(key.clone(), value.clone());
                }
            }
        }
        Ok(payload)
    }

    fn parse_response(value: &Value) -> Result<AiResponse> {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        if let Some(output) = value.get("output").and_then(Value::as_array) {
            for item in output {
                match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "message" => {
                        if let Some(parts) = item.get("content").and_then(Value::as_array) {
                            for part in parts {
                                if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                                        content.push_str(text);
                                    }
                                }
                            }
                        }
                    }
                    "function_call" => {
                        let id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let arguments = item
                            .get("arguments")
                            .map(|value| match value {
                                Value::String(text) => text.clone(),
                                other => other.to_string(),
                            })
                            .unwrap_or_else(|| "{}".into());
                        tool_calls.push(ToolCall {
                            id,
                            kind: "function".into(),
                            function: FunctionCall { name, arguments },
                            thought_signature: None,
                        });
                    }
                    _ => {}
                }
            }
        }
        let usage = value.get("usage").map(|usage| TokenUsage {
            prompt_tokens: usage
                .get("input_tokens")
                .or_else(|| usage.get("prompt_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            completion_tokens: usage
                .get("output_tokens")
                .or_else(|| usage.get("completion_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            total_tokens: usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
        });
        Ok(AiResponse {
            content,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            usage,
        })
    }
}

#[async_trait]
impl LlmAdapter for OpenAiResponsesAdapter {
    fn protocol(&self) -> ProviderProtocol {
        ProviderProtocol::OpenAiResponses
    }

    async fn chat(&self, request: &LlmRequest) -> Result<AiResponse> {
        let (url, headers) = self.endpoint_and_headers()?;
        let payload = self.build_payload(request, false)?;
        debug!(
            model = %self.provider.model,
            protocol = "openai_responses",
            url = %url,
            "openai responses chat request"
        );
        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("openai responses request failed")?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            error!(%status, body = %text, "openai responses HTTP error");
            anyhow::bail!("OpenAI Responses error {status}: {text}");
        }
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("invalid OpenAI Responses JSON: {text}"))?;
        Self::parse_response(&value)
    }

    fn chat_stream(&self, request: LlmRequest) -> BoxStream<'static, StreamEvent> {
        let provider = self.provider.clone();
        let backend_url = self.backend_url.clone();
        let session_id = self.session_id.clone();
        let http = self.http.clone();
        Box::pin(async_stream::stream! {
            let adapter = OpenAiResponsesAdapter {
                http,
                provider: provider.clone(),
                backend_url,
                session_id,
            };
            let (url, headers) = match adapter.endpoint_and_headers() {
                Ok(value) => value,
                Err(error) => {
                    yield StreamEvent::Error(error.to_string());
                    return;
                }
            };
            let payload = match adapter.build_payload(&request, true) {
                Ok(value) => value,
                Err(error) => {
                    yield StreamEvent::Error(error.to_string());
                    return;
                }
            };
            debug!(
                model = %provider.model,
                protocol = "openai_responses",
                url = %url,
                "openai responses stream request"
            );
            let response = match adapter.http.post(&url).headers(headers).json(&payload).send().await {
                Ok(response) => response,
                Err(error) => {
                    yield StreamEvent::Error(error.to_string());
                    return;
                }
            };
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                yield StreamEvent::Error(format!("OpenAI Responses error {status}: {text}"));
                return;
            }
            yield StreamEvent::FirstToken;
            let mut body = response.bytes_stream();
            let mut buffer = String::new();
            let mut content = String::new();
            let mut tool_calls: std::collections::HashMap<String, (String, String)> =
                std::collections::HashMap::new();
            // Responses events identify the same function call with two ids:
            // `item.id` on argument deltas and `item.call_id` on conversation
            // replay. Keep the public stream keyed by call_id.
            let mut item_to_call = std::collections::HashMap::<String, String>::new();
            while let Some(chunk) = body.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        yield StreamEvent::Error(error.to_string());
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find("

") {
                    let event_block = buffer[..pos].to_string();
                    buffer.drain(..pos + 2);
                    let mut data = None;
                    for line in event_block.lines() {
                        let line = line.trim_start();
                        if let Some(rest) = line.strip_prefix("data:") {
                            data = Some(rest.trim().to_string());
                        }
                    }
                    let Some(data) = data else { continue; };
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(&data) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let event_type = value
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    match event_type {
                        "response.output_text.delta" => {
                            if let Some(text) = value.get("delta").and_then(Value::as_str) {
                                content.push_str(text);
                                yield StreamEvent::TextDelta(text.to_string());
                            }
                        }
                        "response.reasoning_summary_text.delta"
                        | "response.reasoning.delta" => {
                            if let Some(text) = value.get("delta").and_then(Value::as_str) {
                                yield StreamEvent::ThinkingDelta(text.to_string());
                            }
                        }
                        "response.output_item.added" => {
                            if let Some(item) = value.get("item") {
                                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                                    let id = item
                                        .get("call_id")
                                        .or_else(|| item.get("id"))
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    let item_id = item
                                        .get("id")
                                        .and_then(Value::as_str)
                                        .unwrap_or(&id)
                                        .to_string();
                                    item_to_call.insert(item_id, id.clone());
                                    let name = item
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    tool_calls.insert(id.clone(), (name.clone(), String::new()));
                                    yield StreamEvent::ToolCallStart {
                                        id: id.clone(),
                                        name,
                                        thought_signature: None,
                                    };
                                }
                            }
                        }
                        "response.function_call_arguments.delta" => {
                            let wire_id = value
                                .get("call_id")
                                .or_else(|| value.get("item_id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let id = item_to_call
                                .get(&wire_id)
                                .cloned()
                                .unwrap_or(wire_id);
                            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                                if let Some(entry) = tool_calls.get_mut(&id) {
                                    entry.1.push_str(delta);
                                }
                                yield StreamEvent::ToolCallDelta {
                                    id,
                                    arguments_delta: delta.to_string(),
                                };
                            }
                        }
                        "response.function_call_arguments.done" => {
                            let wire_id = value
                                .get("call_id")
                                .or_else(|| value.get("item_id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let id = item_to_call
                                .get(&wire_id)
                                .cloned()
                                .unwrap_or(wire_id);
                            if tool_calls
                                .get(&id)
                                .is_some_and(|(_, arguments)| arguments.is_empty())
                            {
                                if let Some(arguments) = value.get("arguments").and_then(Value::as_str) {
                                    if let Some(entry) = tool_calls.get_mut(&id) {
                                        entry.1.push_str(arguments);
                                    }
                                    yield StreamEvent::ToolCallDelta {
                                        id: id.clone(),
                                        arguments_delta: arguments.to_string(),
                                    };
                                }
                            }
                            yield StreamEvent::ToolCallEnd { id };
                        }
                        "response.completed" => {
                            let usage = value
                                .pointer("/response/usage")
                                .map(|usage| TokenUsage {
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
                                });
                            yield StreamEvent::ResponseStop(StopReason::from_provider(
                                value
                                    .pointer("/response/status")
                                    .and_then(Value::as_str)
                                    .unwrap_or("completed"),
                            ));
                            yield StreamEvent::Done(usage);
                        }
                        "error" => {
                            let message = value
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("openai responses stream error");
                            yield StreamEvent::Error(message.to_string());
                            return;
                        }
                        _ => {}
                    }
                }
            }
            if content.is_empty() && tool_calls.is_empty() {
                // Some gateways only emit a final response object.
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use mockito::Server;

    fn provider() -> ProviderConfig {
        ProviderConfig {
            name: "test".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "sk".into(),
            protocol: "openai_responses".into(),
            model: "model".into(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        }
    }

    fn provider_at(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: base_url.into(),
            ..provider()
        }
    }

    fn message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn build_payload_keeps_function_calls_as_standalone_items() {
        let adapter = OpenAiResponsesAdapter::new(provider(), "http://127.0.0.1:9".into(), None);
        let mut assistant = message("assistant", "");
        assistant.tool_calls = Some(vec![ToolCall {
            id: "c1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "web_query".into(),
                arguments: "{}".into(),
            },
            thought_signature: None,
        }]);
        let mut tool_result = message("tool", "ok");
        tool_result.tool_call_id = Some("c1".into());
        let request = LlmRequest {
            messages: vec![message("user", "hi"), assistant, tool_result],
            system_prompt: Some("sys".into()),
            tools: None,
            stream: false,
            tool_choice: None,
            thinking: false,
            max_tokens: None,
            temperature: None,
        };
        let payload = adapter.build_payload(&request, false).unwrap();
        let input = payload["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[3]["type"], "function_call_output");
        // Contract: no message item may embed a function_call content part.
        for item in input {
            if let Some(parts) = item.get("content").and_then(Value::as_array) {
                assert!(parts.iter().all(|part| part["type"] != "function_call"));
            }
        }
    }

    #[test]
    fn parses_function_call_output() {
        let value = serde_json::json!({
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "hello"}]},
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "read_file",
                    "arguments": "{\"path\":\"a.rs\"}"
                }
            ],
            "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
        });
        let response = OpenAiResponsesAdapter::parse_response(&value).unwrap();
        assert_eq!(response.content, "hello");
        let tools = response.tool_calls.unwrap();
        assert_eq!(tools[0].id, "call_1");
        assert_eq!(tools[0].function.name, "read_file");
        assert_eq!(response.usage.unwrap().total_tokens, 5);
    }

    #[tokio::test]
    async fn stream_maps_item_id_deltas_back_to_call_id() {
        let mut server = Server::new_async().await;
        let body = [
            r#"data: {"type":"response.output_item.added","item":{"type":"function_call","id":"item_1","call_id":"call_1","name":"find"}}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"item_1","delta":"{\"pattern\":"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"item_1","delta":"\"**/*.rs\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","item_id":"item_1"}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
        ]
        .join("\n\n");
        let request = server
            .mock("POST", "/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;
        let adapter = OpenAiResponsesAdapter::new(
            provider_at(&server.url()),
            "http://127.0.0.1:9".into(),
            None,
        );
        let events = adapter
            .chat_stream(LlmRequest {
                messages: Vec::new(),
                system_prompt: None,
                tools: None,
                stream: true,
                tool_choice: None,
                thinking: false,
                max_tokens: None,
                temperature: None,
            })
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolCallStart { id, name, .. }
                if id == "call_1" && name == "find"
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
        assert_eq!(arguments, r#"{"pattern":"**/*.rs"}"#);
        request.assert_async().await;
    }
}
