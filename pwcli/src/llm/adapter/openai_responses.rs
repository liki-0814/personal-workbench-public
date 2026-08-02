use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::config::ProviderConfig;
use crate::llm::models::*;

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
            http: crate::http_client::default_client(crate::http_client::ClientProfile::Llm),
            provider,
            backend_url,
            session_id,
        }
    }

    fn endpoint_and_headers(&self) -> Result<(String, reqwest::header::HeaderMap)> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse()?);
        if self.provider.uses_proxy() {
            let url = format!("{}/api/proxy/openai", self.backend_url);
            headers.insert("X-Base-Url", self.provider.base_url.parse()?);
            headers.insert("X-Api-Key", self.provider.api_key.parse()?);
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
            input.push(json!({
                "role": "system",
                "content": [{"type": "input_text", "text": system}]
            }));
        }
        for message in &request.messages {
            match message.role.as_str() {
                "system" => {
                    if !message.content.is_empty() {
                        input.push(json!({
                            "role": "system",
                            "content": [{"type": "input_text", "text": message.content}]
                        }));
                    }
                }
                "user" => {
                    let mut content = Vec::new();
                    if !message.content.is_empty() {
                        content.push(json!({"type": "input_text", "text": message.content}));
                    }
                    for image in &message.images {
                        content.push(json!({
                            "type": "input_image",
                            "image_url": format!("data:{};base64,{}", image.media_type, image.data)
                        }));
                    }
                    if content.is_empty() {
                        content.push(json!({"type": "input_text", "text": ""}));
                    }
                    input.push(json!({"role": "user", "content": content}));
                }
                "assistant" => {
                    let mut content = Vec::new();
                    if !message.content.trim().is_empty() {
                        content.push(json!({"type": "output_text", "text": message.content}));
                    }
                    let mut item = json!({"role": "assistant", "content": content});
                    if let Some(tool_calls) = &message.tool_calls {
                        let calls: Vec<Value> = tool_calls
                            .iter()
                            .map(|call| {
                                json!({
                                    "type": "function_call",
                                    "call_id": call.id,
                                    "name": call.function.name,
                                    "arguments": call.function.arguments,
                                })
                            })
                            .collect();
                        if !calls.is_empty() {
                            item["content"] =
                                Value::Array(content.into_iter().chain(calls).collect::<Vec<_>>());
                        }
                    }
                    input.push(item);
                }
                "tool" => {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": message.tool_call_id.clone().unwrap_or_default(),
                        "output": message.content,
                    }));
                }
                _ => {
                    input.push(json!({
                        "role": message.role,
                        "content": [{"type": "input_text", "text": message.content}]
                    }));
                }
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
            payload["tools"] = Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "name": tool.function.name,
                            "description": tool.function.description,
                            "parameters": tool.function.parameters,
                        })
                    })
                    .collect(),
            );
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
                            let id = value
                                .get("call_id")
                                .or_else(|| value.get("item_id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
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
                            let id = value
                                .get("call_id")
                                .or_else(|| value.get("item_id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
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
}
