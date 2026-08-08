use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error};

use crate::ai::config::ProviderConfig;
use crate::ai::llm::models::*;

use super::LlmAdapter;

pub struct GoogleGenerativeAdapter {
    http: Client,
    provider: ProviderConfig,
    backend_url: String,
}

impl GoogleGenerativeAdapter {
    pub fn new(provider: ProviderConfig, backend_url: String) -> Self {
        Self {
            http: crate::ai::http::default_client(crate::ai::http::ClientProfile::Llm),
            provider,
            backend_url,
        }
    }

    fn is_antigravity(&self) -> bool {
        self.provider.protocol == "google_antigravity"
    }

    fn antigravity_project(&self) -> Result<String> {
        self.provider
            .compat_profile
            .as_deref()
            .into_iter()
            .flat_map(|value| value.split(';'))
            .find_map(|value| value.strip_prefix("project:"))
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .context("Antigravity credential has no Cloud Code Assist project; log in again")
    }

    fn antigravity_effort(&self) -> Option<&str> {
        let model = self.provider.current_model_entry()?;
        model
            .thinking_params
            .as_ref()
            .and_then(|params| params.get("reasoning_effort"))
            .or_else(|| {
                model
                    .request_params
                    .as_ref()
                    .and_then(|params| params.get("reasoning_effort"))
            })
            .and_then(Value::as_str)
    }

    fn antigravity_wire_model(&self) -> (String, Option<String>) {
        let model = self.provider.model.as_str();
        let effort = self.antigravity_effort();
        let alias = match model {
            "gemini-3.1-pro-high" | "gemini-3.1-pro-preview" => "gemini-pro-agent",
            "gemini-3.5-flash-extra-low" => "gemini-3.6-flash-low",
            "gemini-3.5-flash-low" | "gemini-3.5-flash-mid" => "gemini-3.6-flash-medium",
            "gemini-3.5-flash-high" | "gemini-3-flash-agent" => "gemini-3.6-flash-high",
            other => other,
        };
        if alias != model
            || model.starts_with("gemini-3.6-flash-")
            || model == "gemini-3.1-pro-low"
            || model == "gemini-pro-agent"
        {
            return (alias.to_string(), None);
        }
        match model {
            "gemini-3.6-flash" => {
                let level = effort.filter(|value| matches!(*value, "low" | "medium" | "high"));
                let level = level.unwrap_or("medium");
                (
                    format!("gemini-3.6-flash-{level}"),
                    effort.map(str::to_string),
                )
            }
            "gemini-3.1-pro" => {
                let level = effort.filter(|value| matches!(*value, "low" | "high"));
                let level = level.unwrap_or("high");
                let wire = if level == "low" {
                    "gemini-3.1-pro-low"
                } else {
                    "gemini-pro-agent"
                };
                (wire.to_string(), effort.map(str::to_string))
            }
            model if model.starts_with("claude-") => {
                let level = effort.and_then(|value| match value {
                    "minimal" | "low" | "medium" | "high" => Some(value),
                    "xhigh" | "max" | "ultra" => Some("high"),
                    _ => None,
                });
                (model.to_string(), level.map(str::to_string))
            }
            _ => (model.to_string(), None),
        }
    }

    fn antigravity_session_id(&self, request: &LlmRequest) -> String {
        use sha2::{Digest, Sha256};
        let first_user_text = request
            .messages
            .iter()
            .find(|message| message.role == "user" && !message.content.is_empty())
            .map(|message| message.content.as_bytes());
        let Some(text) = first_user_text else {
            return format!("-{}", uuid::Uuid::now_v7().as_u128());
        };
        let digest = Sha256::digest(text);
        let mut prefix = [0_u8; 8];
        prefix.copy_from_slice(&digest[..8]);
        format!("-{}", u64::from_be_bytes(prefix) & 0x7fff_ffff_ffff_ffff)
    }

    fn model_path(&self, action: &str) -> String {
        let base = self.provider.base_url.trim_end_matches('/');
        // Accept either full generateContent URL template or Generative Language root.
        if base.contains("{model}") {
            return base
                .replace("{model}", &self.provider.model)
                .replace("{action}", action);
        }
        if base.contains(":generateContent") || base.contains(":streamGenerateContent") {
            return base.to_string();
        }
        if base.ends_with(&format!("/models/{}", self.provider.model)) {
            return format!("{base}:{action}");
        }
        format!("{base}/models/{}:{action}", self.provider.model)
    }

    fn endpoint_and_headers(&self, stream: bool) -> Result<(String, reqwest::header::HeaderMap)> {
        let action = if stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse()?);
        if self.is_antigravity() {
            let suffix = if stream { "?alt=sse" } else { "" };
            let url = format!(
                "{}/v1internal:{action}{suffix}",
                self.provider.base_url.trim_end_matches('/')
            );
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.provider.api_key).parse()?,
            );
            headers.insert(
                reqwest::header::USER_AGENT,
                "antigravity/cli/1.0.13 (aidev_client; os_type=darwin; arch=arm64)".parse()?,
            );
            headers.insert(
                "x-goog-api-client",
                "google-api-nodejs-client/10.3.0".parse()?,
            );
            return Ok((url, headers));
        }
        if self.provider.uses_proxy() {
            let url = format!("{}/api/proxy/openai", self.backend_url);
            headers.insert("X-Base-Url", self.model_path(action).parse()?);
            headers.insert("X-Api-Key", self.provider.api_key.parse()?);
            Ok((url, headers))
        } else {
            let mut url = self.model_path(action);
            if stream && !url.contains("alt=sse") {
                if url.contains('?') {
                    url.push_str("&alt=sse");
                } else {
                    url.push_str("?alt=sse");
                }
            }
            // Generative Language API commonly uses query key or header.
            if !url.contains("key=") {
                headers.insert("x-goog-api-key", self.provider.api_key.parse()?);
            }
            Ok((url, headers))
        }
    }

    fn build_payload(&self, request: &LlmRequest) -> Result<Value> {
        let mut contents = Vec::new();
        let mut system_instruction = None;
        if let Some(system) = request.system_prompt.as_deref().filter(|s| !s.is_empty()) {
            system_instruction = Some(json!({
                "parts": [{"text": system}]
            }));
        }
        for message in &request.messages {
            match message.role.as_str() {
                "system" => {
                    if system_instruction.is_none() && !message.content.is_empty() {
                        system_instruction = Some(json!({
                            "parts": [{"text": message.content}]
                        }));
                    }
                }
                "user" | "tool" => {
                    let mut parts = Vec::new();
                    if message.role == "tool" {
                        parts.push(json!({
                            "functionResponse": {
                                "name": "tool",
                                "response": {
                                    "result": message.content,
                                    "toolCallId": message.tool_call_id.clone().unwrap_or_default(),
                                },
                            }
                        }));
                    } else {
                        if !message.content.is_empty() {
                            parts.push(json!({"text": message.content}));
                        }
                        for image in &message.images {
                            parts.push(json!({
                                "inlineData": {
                                    "mimeType": image.media_type,
                                    "data": image.data,
                                }
                            }));
                        }
                    }
                    contents.push(json!({
                        "role": "user",
                        "parts": parts,
                    }));
                }
                "assistant" => {
                    let mut parts = Vec::new();
                    if !message.content.trim().is_empty() {
                        parts.push(json!({"text": message.content}));
                    }
                    if let Some(tool_calls) = &message.tool_calls {
                        for call in tool_calls {
                            let args: Value = serde_json::from_str(&call.function.arguments)
                                .unwrap_or_else(|_| json!({"raw": call.function.arguments}));
                            let mut part = json!({
                                "functionCall": {
                                    "name": call.function.name,
                                    "args": args,
                                }
                            });
                            if let Some(signature) = &call.thought_signature {
                                part["thoughtSignature"] = json!(signature);
                            }
                            parts.push(part);
                        }
                    }
                    contents.push(json!({
                        "role": "model",
                        "parts": parts,
                    }));
                }
                _ => {}
            }
        }

        let mut payload = json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": self.provider.effective_max_tokens(request.max_tokens),
            }
        });
        if let Some(temperature) = request.temperature {
            payload["generationConfig"]["temperature"] = json!(temperature);
        }
        if let Some(system_instruction) = system_instruction {
            payload["systemInstruction"] = system_instruction;
        }
        if let Some(tools) = &request.tools {
            payload["tools"] = json!([{
                "functionDeclarations": tools.iter().map(|tool| json!({
                    "name": tool.function.name,
                    "description": tool.function.description,
                    "parameters": tool.function.parameters,
                })).collect::<Vec<_>>()
            }]);
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
                payload["generationConfig"]["thinkingConfig"] = json!({"includeThoughts": true});
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
        if self.is_antigravity() {
            let (wire_model, thinking_level) = self.antigravity_wire_model();
            payload["sessionId"] = json!(self.antigravity_session_id(request));
            if let Some(level) = thinking_level {
                payload["generationConfig"]["thinkingConfig"] =
                    json!({"thinkingLevel": level, "includeThoughts": true});
            }
            return Ok(json!({
                "model": wire_model,
                "userAgent": "antigravity",
                "requestType": "agent",
                "project": self.antigravity_project()?,
                "requestId": format!("agent-{}", uuid::Uuid::now_v7()),
                "request": payload,
            }));
        }
        Ok(payload)
    }

    fn parse_candidate(value: &Value) -> Result<AiResponse> {
        let value = value.get("response").unwrap_or(value);
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let parts = value
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if part
                    .get("thought")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    continue;
                }
                content.push_str(text);
            }
            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = call
                    .get("args")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "{}".into());
                let signature = part
                    .get("thoughtSignature")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                tool_calls.push(ToolCall {
                    id: format!("call_{index}"),
                    kind: "function".into(),
                    function: FunctionCall { name, arguments },
                    thought_signature: signature,
                });
            }
        }
        let usage = value.get("usageMetadata").map(|usage| TokenUsage {
            prompt_tokens: usage
                .get("promptTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            completion_tokens: usage
                .get("candidatesTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            total_tokens: usage
                .get("totalTokenCount")
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
impl LlmAdapter for GoogleGenerativeAdapter {
    fn protocol(&self) -> ProviderProtocol {
        if self.is_antigravity() {
            ProviderProtocol::GoogleAntigravity
        } else {
            ProviderProtocol::GoogleGenerative
        }
    }

    async fn chat(&self, request: &LlmRequest) -> Result<AiResponse> {
        let (url, headers) = self.endpoint_and_headers(false)?;
        let payload = self.build_payload(request)?;
        debug!(
            model = %self.provider.model,
            protocol = "google_generative",
            url = %url,
            "google generative chat request"
        );
        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("google generative request failed")?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            error!(%status, body = %text, "google generative HTTP error");
            anyhow::bail!("Google Generative error {status}: {text}");
        }
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("invalid Google Generative JSON: {text}"))?;
        Self::parse_candidate(&value)
    }

    fn chat_stream(&self, request: LlmRequest) -> BoxStream<'static, StreamEvent> {
        let provider = self.provider.clone();
        let backend_url = self.backend_url.clone();
        let http = self.http.clone();
        Box::pin(async_stream::stream! {
            let adapter = GoogleGenerativeAdapter {
                http,
                provider: provider.clone(),
                backend_url,
            };
            let (url, headers) = match adapter.endpoint_and_headers(true) {
                Ok(value) => value,
                Err(error) => {
                    yield StreamEvent::Error(error.to_string());
                    return;
                }
            };
            let payload = match adapter.build_payload(&request) {
                Ok(value) => value,
                Err(error) => {
                    yield StreamEvent::Error(error.to_string());
                    return;
                }
            };
            debug!(
                model = %provider.model,
                protocol = "google_generative",
                url = %url,
                "google generative stream request"
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
                yield StreamEvent::Error(format!("Google Generative error {status}: {text}"));
                return;
            }
            yield StreamEvent::FirstToken;
            let mut body = response.bytes_stream();
            let mut buffer = String::new();
            let mut emitted_tool = false;
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
                    if data.is_empty() {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(&data) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let response_value = value.get("response").unwrap_or(&value);
                    let parts = response_value
                        .pointer("/candidates/0/content/parts")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for (index, part) in parts.iter().enumerate() {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if part.get("thought").and_then(Value::as_bool).unwrap_or(false) {
                                yield StreamEvent::ThinkingDelta(text.to_string());
                            } else if !text.is_empty() {
                                yield StreamEvent::TextDelta(text.to_string());
                            }
                        }
                        if let Some(call) = part.get("functionCall") {
                            let id = format!("call_{index}");
                            let name = call
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let arguments = call
                                .get("args")
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "{}".into());
                            let signature = part
                                .get("thoughtSignature")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            yield StreamEvent::ToolCallStart {
                                id: id.clone(),
                                name,
                                thought_signature: signature,
                            };
                            yield StreamEvent::ToolCallDelta {
                                id: id.clone(),
                                arguments_delta: arguments,
                            };
                            yield StreamEvent::ToolCallEnd { id };
                            emitted_tool = true;
                        }
                    }
                    if let Some(usage) = response_value.get("usageMetadata") {
                        yield StreamEvent::Done(Some(TokenUsage {
                            prompt_tokens: usage
                                .get("promptTokenCount")
                                .and_then(Value::as_u64)
                                .unwrap_or(0) as u32,
                            completion_tokens: usage
                                .get("candidatesTokenCount")
                                .and_then(Value::as_u64)
                                .unwrap_or(0) as u32,
                            total_tokens: usage
                                .get("totalTokenCount")
                                .and_then(Value::as_u64)
                                .unwrap_or(0) as u32,
                        }));
                    }
                }
            }
            if emitted_tool {
                yield StreamEvent::ResponseStop(StopReason::ToolUse);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LlmRequest {
        LlmRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello".into(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: None,
                tool_call_id: None,
            }],
            system_prompt: Some("be helpful".into()),
            tools: None,
            stream: true,
            tool_choice: None,
            thinking: true,
            max_tokens: None,
            temperature: None,
        }
    }

    #[test]
    fn parses_function_call_and_text() {
        let value = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "hi"},
                        {"functionCall": {"name": "list_directory", "args": {"path": "."}}}
                    ]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        });
        let response = GoogleGenerativeAdapter::parse_candidate(&value).unwrap();
        assert_eq!(response.content, "hi");
        let tools = response.tool_calls.unwrap();
        assert_eq!(tools[0].function.name, "list_directory");
        assert!(tools[0].function.arguments.contains("path"));
        assert_eq!(response.usage.unwrap().total_tokens, 3);
    }

    #[test]
    fn builds_antigravity_cloud_code_assist_envelope() {
        let adapter = GoogleGenerativeAdapter::new(
            ProviderConfig {
                name: "Google Antigravity".into(),
                base_url: "https://daily-cloudcode-pa.googleapis.com".into(),
                api_key: "oauth-token".into(),
                protocol: "google_antigravity".into(),
                model: "gemini-3.6-flash".into(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: Some(
                    "builtin:google-antigravity;credential:p;project:project-a".into(),
                ),
            },
            "http://127.0.0.1:9".into(),
        );
        let payload = adapter.build_payload(&request()).unwrap();
        assert_eq!(payload["model"], "gemini-3.6-flash-medium");
        assert_eq!(payload["project"], "project-a");
        assert_eq!(payload["requestType"], "agent");
        assert_eq!(payload["request"]["contents"][0]["role"], "user");
    }

    #[test]
    fn parses_antigravity_response_envelope() {
        let response = GoogleGenerativeAdapter::parse_candidate(&serde_json::json!({
            "response": {"candidates": [{"content": {"parts": [{"text": "hi"}]}}]}
        }))
        .unwrap();
        assert_eq!(response.content, "hi");
    }
}
