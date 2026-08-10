use crate::ai::llm::models::*;
use anyhow::Result;
use futures::stream::{BoxStream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use tracing::{debug, error};

fn effective_temperature(
    _provider: &crate::ai::config::ProviderConfig,
    requested: Option<f32>,
) -> Option<f32> {
    requested
}

// 默认输出 token 上限 = 128_000（十进制，不是 128 KiB）。OpenAI 协议
// DashScope 等代理）传 max_tokens 让服务端按模型上限 clamp，避免上游用极小默认值
// （部分网关默认 2K/4K）把 long tool_call arguments JSON 截断 → 反序列化失败 →
// 模型重复同一个失败 tool call 的死循环。max_tokens 兜底走
// `crate::ai::llm::default_max_tokens_for(model)`：Opus → 128K，其他模型（Gemini/
// GPT/Qwen/Kimi 等）→ 64K，避免 Gemini 65537 上限被超返 INVALID_ARGUMENT 400。

/// 构造 OpenAI Chat Completions API 的 messages 数组。
///
/// 关键不变量：
/// - assistant 消息必须保留 `tool_calls`（OpenAI 规范要求 tool 消息前导有 tool_calls）
/// - tool 消息必须带 `tool_call_id`，对应上一条 assistant.tool_calls 中的某一个 id
/// - 不合并相邻同 role 的消息（之前的合并逻辑会把多个 tool 结果拼成一条，破坏 tool_call_id 1:1 对应）
/// - 跳过 (无文本 + 无 tool_calls) 的空 assistant 消息（部分代理对此报错）
fn build_openai_messages_json(messages: &[ChatMessage]) -> Vec<Value> {
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        match m.role.as_str() {
            "system" => {
                let content = decode_compaction_content(&m.content);
                out.push(serde_json::json!({"role": "system", "content": content}));
            }
            "user" => {
                if m.images.is_empty() {
                    out.push(serde_json::json!({"role": "user", "content": m.content}));
                } else {
                    let mut parts = vec![serde_json::json!({"type": "text", "text": m.content})];
                    for img in &m.images {
                        parts.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", img.media_type, img.data),
                            }
                        }));
                    }
                    out.push(serde_json::json!({"role": "user", "content": parts}));
                }
            }
            "assistant" => {
                let has_text = !m.content.trim().is_empty();
                let has_tool_calls = m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());
                if !has_text && !has_tool_calls {
                    // 空 assistant 跳过——部分代理（含 OpenAI 自身）会拒绝
                    continue;
                }
                let mut obj = serde_json::Map::new();
                obj.insert("role".to_string(), Value::String("assistant".to_string()));
                obj.insert("content".to_string(), Value::String(m.content.clone()));
                if let Some(tc) = &m.tool_calls {
                    if !tc.is_empty() {
                        if let Ok(v) = serde_json::to_value(tc) {
                            obj.insert("tool_calls".to_string(), v);
                        }
                    }
                }
                out.push(Value::Object(obj));
            }
            "tool" => {
                let (_, clean_content) = crate::ai::llm::deferred_tools::split(&m.content);
                let mut obj = serde_json::Map::new();
                obj.insert("role".to_string(), Value::String("tool".to_string()));
                if crate::ai::llm::image_marker::contains_marker(clean_content) {
                    // 含 read_file 注入的图片标记 → 转 OpenAI 多模态 content array
                    let segs = crate::ai::llm::image_marker::parse_segments(clean_content);
                    let mut parts: Vec<Value> = Vec::with_capacity(segs.len());
                    for seg in segs {
                        match seg {
                            crate::ai::llm::image_marker::Segment::Text(t) => {
                                parts.push(serde_json::json!({"type":"text","text": t}));
                            }
                            crate::ai::llm::image_marker::Segment::Image { mime, base64 } => {
                                parts.push(serde_json::json!({
                                    "type":"image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", mime, base64)
                                    }
                                }));
                            }
                        }
                    }
                    if parts.is_empty() {
                        obj.insert("content".to_string(), Value::String(String::new()));
                    } else {
                        obj.insert("content".to_string(), Value::Array(parts));
                    }
                } else {
                    obj.insert(
                        "content".to_string(),
                        Value::String(clean_content.to_string()),
                    );
                }
                if let Some(id) = &m.tool_call_id {
                    obj.insert("tool_call_id".to_string(), Value::String(id.clone()));
                }
                out.push(Value::Object(obj));
            }
            _ => {
                // 未知 role：保守按字符串 content 透传
                out.push(serde_json::json!({"role": m.role, "content": m.content}));
            }
        }
    }
    out
}

fn decode_compaction_content(content: &str) -> &str {
    const PREFIX: &str = "\u{001e}pwcli-compaction:";
    const SUFFIX: char = '\u{001e}';
    content
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.split_once(SUFFIX).map(|(_, summary)| summary))
        .unwrap_or(content)
}

fn build_openai_request_parts(
    messages: &[ChatMessage],
    tools: Option<&[ToolSchema]>,
    deferred_tools: bool,
) -> (Vec<Value>, Option<Vec<ToolSchema>>) {
    if !deferred_tools {
        return (
            build_openai_messages_json(messages),
            tools.map(<[_]>::to_vec),
        );
    }
    let deferred_names = messages
        .iter()
        .flat_map(|message| crate::ai::llm::deferred_tools::split(&message.content).0)
        .collect::<std::collections::HashSet<_>>();
    let immediate = tools.map(|schemas| {
        schemas
            .iter()
            .filter(|schema| !deferred_names.contains(&schema.function.name))
            .cloned()
            .collect::<Vec<_>>()
    });
    let mut out = Vec::new();
    for message in messages {
        out.extend(build_openai_messages_json(std::slice::from_ref(message)));
        if message.role != "tool" {
            continue;
        }
        let (names, _) = crate::ai::llm::deferred_tools::split(&message.content);
        let activated = tools
            .into_iter()
            .flatten()
            .filter(|schema| names.contains(&schema.function.name))
            .cloned()
            .collect::<Vec<_>>();
        if !activated.is_empty() {
            out.push(serde_json::json!({ "role": "system", "tools": activated }));
        }
    }
    (out, immediate)
}

pub struct OpenAiClient {
    http: Client,
    provider: ProviderConfig,
    backend_url: String,
    session_id: Option<String>,
}

impl OpenAiClient {
    pub fn new(provider: ProviderConfig, backend_url: String) -> Self {
        Self {
            http: crate::ai::http::default_client(crate::ai::http::ClientProfile::Llm),
            provider,
            backend_url,
            session_id: None,
        }
    }

    pub fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }

    pub async fn chat(&self, request: &LlmRequest) -> Result<AiResponse> {
        let use_proxy = needs_proxy(&self.provider);
        let (url, headers) = if use_proxy {
            let url = format!("{}/api/proxy/openai", self.backend_url);
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("Content-Type", "application/json".parse()?);
            headers.insert(
                "X-Provider-Id",
                crate::ai::provider::provider_id(&self.provider).parse()?,
            );
            (url, headers)
        } else {
            let url = format!("{}/chat/completions", self.provider.base_url);
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("Content-Type", "application/json".parse()?);
            headers.insert(
                "Authorization",
                format!("Bearer {}", self.provider.api_key).parse()?,
            );
            (url, headers)
        };
        let (messages_json, request_tools) = build_openai_request_parts(
            &request.messages,
            request.tools.as_deref(),
            self.provider.deferred_tools_mode().is_some(),
        );

        let mut payload = serde_json::json!({
            "model": self.provider.model,
            "messages": messages_json,
            "stream": request.stream,
            "max_tokens": self.provider.effective_max_tokens(request.max_tokens),
        });
        if let Some(temperature) = effective_temperature(&self.provider, request.temperature) {
            payload["temperature"] = serde_json::json!(temperature);
        }

        if let Some(sp) = &request.system_prompt {
            let msgs = payload["messages"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("payload messages is not an array"))?;
            msgs.insert(0, serde_json::json!({"role": "system", "content": sp}));
        }

        if let Some(tools) = &request_tools {
            payload["tools"] = serde_json::to_value(tools)?;
        }
        if let Some(tool_choice) = &request.tool_choice {
            payload["tool_choice"] = serde_json::to_value(tool_choice)?;
        }
        apply_provider_options(&mut payload, &self.provider, request.thinking, false);

        debug!(model = %self.provider.model, url = %url, "openai chat request");
        let mut attempt: u32 = 0;
        let res = loop {
            let res = self
                .http
                .post(&url)
                .headers(headers.clone())
                .json(&payload)
                .send()
                .await?;
            if res.status().is_success() {
                break res;
            }
            let status = res.status();
            let retry_after = res
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let text = res.text().await.unwrap_or_default();
            if super::retry::is_provider_error(status.as_u16(), &text) {
                if let Some(delay) =
                    super::retry::next_backoff_with_hint(attempt, retry_after.as_deref())
                {
                    attempt += 1;
                    tracing::info!(
                        model = %self.provider.model,
                        attempt,
                        max = super::retry::MAX_ATTEMPTS,
                        delay_ms = delay.as_millis() as u64,
                        "provider-error 400, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }
            error!(model = %self.provider.model, status = %status, body = %text, "openai chat HTTP error");
            anyhow::bail!("AI request failed ({}): {}", status, text);
        };

        let data: Value = res.json().await?;
        let choice = data["choices"].get(0);
        let content = choice
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string();
        let tool_calls = if let Some(c) = choice {
            c["message"]["tool_calls"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(parse_openai_tool_call)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
                .map_err(|e| anyhow::anyhow!("failed to parse tool_calls: {}", e))?
        } else {
            None
        };

        let usage = match &data["usage"] {
            serde_json::Value::Null => None,
            u => Some(TokenUsage {
                prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
                total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as u32,
            }),
        };

        Ok(AiResponse {
            content,
            tool_calls,
            usage,
        })
    }
}

fn needs_proxy(provider: &ProviderConfig) -> bool {
    provider.uses_proxy()
}

fn apply_provider_options(
    payload: &mut Value,
    provider: &ProviderConfig,
    thinking: bool,
    stream: bool,
) {
    const RESERVED: &[&str] = &[
        "model",
        "messages",
        "stream",
        "tools",
        "tool_choice",
        "max_tokens",
        "temperature",
    ];

    let merge_params =
        |payload: &mut Value, params: &serde_json::Map<String, Value>, source: &str| {
            if let Some(object) = payload.as_object_mut() {
                for (key, value) in params {
                    if RESERVED.contains(&key.as_str()) {
                        debug!(
                            protocol = "openai_chat",
                            source,
                            key = %key,
                            "ignored reserved request knob"
                        );
                        continue;
                    }
                    // Soft guidance only: these keys are commonly from other protocols.
                    if matches!(
                        key.as_str(),
                        "budget_tokens" | "anthropic-version" | "user-agent" | "generationConfig"
                    ) {
                        debug!(
                            protocol = "openai_chat",
                            source,
                            key = %key,
                            "request knob is unusual for openai_chat and may be ignored upstream"
                        );
                    }
                    object.insert(key.clone(), value.clone());
                }
            }
        };

    let model_entry = provider.current_model_entry();
    if let Some(params) = model_entry.and_then(|model| model.request_params.as_ref()) {
        merge_params(payload, params, "requestParams");
    }
    let custom_thinking = model_entry.and_then(|model| model.thinking_params.as_ref());

    if stream {
        payload["stream_options"] = serde_json::json!({"include_usage": true});
    }
    if thinking {
        if let Some(params) = custom_thinking {
            merge_params(payload, params, "thinkingParams");
        } else {
            payload["enable_thinking"] = serde_json::json!(true);
        }
    }
}

/// OpenAI specifies `function.arguments` as a JSON string, but some compatible
/// compatible gateways stream a JSON object instead. Normalize both shapes at
/// the protocol boundary so the agent executor receives the canonical string.
fn normalize_tool_arguments(value: &Value) -> Option<String> {
    match value {
        Value::String(arguments) if !arguments.is_empty() => Some(arguments.clone()),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).ok(),
        _ => None,
    }
}

fn parse_openai_tool_call(value: &Value) -> Result<ToolCall, serde_json::Error> {
    let mut normalized = value.clone();
    if let Some(arguments) = normalize_tool_arguments(&normalized["function"]["arguments"]) {
        normalized["function"]["arguments"] = Value::String(arguments);
    }
    serde_json::from_value(normalized)
}

impl OpenAiClient {
    /// 流式对话：消费 SSE，逐 delta 产出 StreamEvent
    pub fn chat_stream(&self, request: &LlmRequest) -> BoxStream<'_, StreamEvent> {
        let request = LlmRequest {
            stream: true,
            ..request.clone()
        };
        let provider = self.provider.clone();
        let backend_url = self.backend_url.clone();
        let http = self.http.clone();

        let s = async_stream::stream! {
            // ---- 构造请求（与 chat() 保持一致） ----
            let use_proxy = needs_proxy(&provider);
            let (url, headers) = if use_proxy {
                let url = format!("{}/api/proxy/openai", backend_url);
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("Content-Type", "application/json".parse().unwrap());
                if let Ok(v) = crate::ai::provider::provider_id(&provider).parse() {
                    h.insert("X-Provider-Id", v);
                }
                (url, h)
            } else {
                let url = format!("{}/chat/completions", provider.base_url);
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("Content-Type", "application/json".parse().unwrap());
                if let Ok(v) = format!("Bearer {}", provider.api_key).parse() {
                    h.insert("Authorization", v);
                }
                (url, h)
            };
            let (messages_json, request_tools) = build_openai_request_parts(
                &request.messages,
                request.tools.as_deref(),
                provider.deferred_tools_mode().is_some(),
            );

            let mut payload = serde_json::json!({
                "model": provider.model,
                "messages": messages_json,
                "stream": true,
                "max_tokens": provider.effective_max_tokens(request.max_tokens),
            });
            if let Some(temperature) = effective_temperature(&provider, request.temperature) {
                payload["temperature"] = serde_json::json!(temperature);
            }

            if let Some(sp) = &request.system_prompt {
                if let Some(arr) = payload["messages"].as_array_mut() {
                    arr.insert(0, serde_json::json!({"role": "system", "content": sp}));
                }
            }
            if let Some(tools) = &request_tools {
                if let Ok(v) = serde_json::to_value(tools) { payload["tools"] = v; }
            }
            if let Some(tc) = &request.tool_choice {
                if let Ok(v) = serde_json::to_value(tc) { payload["tool_choice"] = v; }
            }
            apply_provider_options(&mut payload, &provider, request.thinking, true);

            debug!(model = %provider.model, url = %url, "openai stream request");
            let res = {
                let mut attempt: u32 = 0;
                loop {
                    let res = match http.post(&url).headers(headers.clone()).json(&payload).send().await {
                        Ok(r) => r,
                        Err(e) => {
                            error!(model = %provider.model, error = %e, "openai stream network error");
                            yield StreamEvent::Error(format!("network error: {}", e));
                            return;
                        }
                    };
                    if res.status().is_success() {
                        break res;
                    }
                    let status = res.status();
                    let retry_after = res
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    let text = res.text().await.unwrap_or_default();
                    if super::retry::is_provider_error(status.as_u16(), &text) {
                        if let Some(delay) = super::retry::next_backoff_with_hint(attempt, retry_after.as_deref()) {
                            attempt += 1;
                            tracing::info!(
                                model = %provider.model,
                                attempt,
                                max = super::retry::MAX_ATTEMPTS,
                                delay_ms = delay.as_millis() as u64,
                                "provider-error 400, retrying"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                    error!(model = %provider.model, status = %status, body = %text, "openai stream HTTP error");
                    yield StreamEvent::Error(format!("AI request failed ({}): {}", status, text));
                    return;
                }
            };

            // ---- 解析 SSE ----
            let mut byte_stream = res.bytes_stream();
            let mut buffer = String::new();
            let mut emitted_first = false;
            // 工具调用按 index 累积：idx -> (id, name, thoughtSignature)
            let mut tool_calls_state: std::collections::HashMap<u64, (String, String, Option<String>)> = std::collections::HashMap::new();

            while let Some(chunk) = byte_stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => { yield StreamEvent::Error(format!("stream read: {}", e)); return; }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // 按 \n\n 切分 SSE 事件块
                while let Some(pos) = buffer.find("\n\n") {
                    let event_block = buffer[..pos].to_string();
                    buffer.drain(..pos + 2);

                    for line in event_block.lines() {
                        let line = line.trim_start();
                        if !line.starts_with("data:") { continue; }
                        let data = line[5..].trim();
                        if data == "[DONE]" {
                            yield StreamEvent::Done(None);
                            return;
                        }
                        let v: Value = match serde_json::from_str(data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        // 业务级错误（HTTP 200 但 body 含 error 对象）
                        // 例：{"error": {"message": "...", "type": "..."}, "type": "error"}
                        if let Some(err_obj) = v.get("error").filter(|e| e.is_object()) {
                            let msg = err_obj["message"].as_str().unwrap_or("LLM 业务错误");
                            yield StreamEvent::Error(msg.to_string());
                            return;
                        }

                        let delta = &v["choices"][0]["delta"];

                        // reasoning_content：reasoning model 的推理流（qwen3-max、
                        // DeepSeek-R1、QwQ 等会先吐推理再吐答复）。无论开关是否开启，
                        // 只要模型实际吐出推理内容，就如实转发给前端做面板渲染 ——
                        // 开关只决定请求侧是否 push `enable_thinking: true`，不影响展示。
                        if let Some(r) = delta["reasoning_content"].as_str().filter(|s| !s.is_empty()) {
                            if !emitted_first {
                                emitted_first = true;
                                yield StreamEvent::FirstToken;
                            }
                            yield StreamEvent::ThinkingDelta(r.to_string());
                        }

                        // content：真正的答复，正常逐字 stream
                        if let Some(content) = delta["content"].as_str().filter(|s| !s.is_empty()) {
                            if !emitted_first {
                                emitted_first = true;
                                yield StreamEvent::FirstToken;
                            }
                            yield StreamEvent::TextDelta(content.to_string());
                        }

                        // 工具调用 delta
                        if let Some(tc_arr) = v["choices"][0]["delta"]["tool_calls"].as_array() {
                            for tc in tc_arr {
                                let idx = tc["index"].as_u64().unwrap_or(0);
                                let id = tc["id"].as_str().unwrap_or("").to_string();
                                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                                let args_delta = normalize_tool_arguments(&tc["function"]["arguments"])
                                    .unwrap_or_default();
                                let thought_sig = tc["thoughtSignature"].as_str().map(|s| s.to_string());

                                let entry = tool_calls_state.entry(idx).or_insert_with(|| (String::new(), String::new(), None));
                                if !id.is_empty() && entry.0.is_empty() {
                                    entry.0 = id.clone();
                                }
                                // Some compatible gateways omit tool_call id — synthesize one.
                                if entry.0.is_empty() && !name.is_empty() {
                                    entry.0 = format!("call_{}", idx);
                                }
                                if thought_sig.is_some() && entry.2.is_none() {
                                    entry.2 = thought_sig;
                                }
                                if !name.is_empty() && entry.1.is_empty() {
                                    entry.1 = name.clone();
                                    yield StreamEvent::ToolCallStart {
                                        id: entry.0.clone(),
                                        name: name.clone(),
                                        thought_signature: entry.2.clone(),
                                    };
                                }
                                if !args_delta.is_empty() && !entry.0.is_empty() {
                                    yield StreamEvent::ToolCallDelta {
                                        id: entry.0.clone(),
                                        arguments_delta: args_delta,
                                    };
                                }
                            }
                        }

                        // finish_reason 触发 ToolCallEnd
                        if let Some(finish_reason) = v["choices"][0]["finish_reason"].as_str() {
                            for (_, (id, _, _)) in tool_calls_state.drain() {
                                if !id.is_empty() {
                                    yield StreamEvent::ToolCallEnd { id };
                                }
                            }
                            yield StreamEvent::ResponseStop(StopReason::from_provider(finish_reason));
                        }

                        // usage（include_usage 时在 [DONE] 前的 chunk 中）
                        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                            let usage = TokenUsage {
                                prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                                completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
                                total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as u32,
                            };
                            yield StreamEvent::Done(Some(usage));
                            return;
                        }
                    }
                }
            }

            yield StreamEvent::Done(None);
        };

        Box::pin(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            images: Vec::new(),
            generated_images: Vec::new(),
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn schema(name: &str) -> ToolSchema {
        ToolSchema {
            kind: "function".into(),
            function: FunctionSchema {
                name: name.into(),
                description: name.into(),
                parameters: serde_json::json!({"type":"object"}),
            },
        }
    }

    #[test]
    fn deferred_tools_move_from_top_level_to_activation_point() {
        let mut tool_result = msg(
            "tool",
            &crate::ai::llm::deferred_tools::attach("loaded".into(), &["late_tool".to_string()]),
        );
        tool_result.tool_call_id = Some("call_1".into());
        let tools = vec![schema("always"), schema("late_tool")];
        let (messages, immediate) = build_openai_request_parts(&[tool_result], Some(&tools), true);
        assert_eq!(immediate.unwrap()[0].function.name, "always");
        assert_eq!(messages[0]["content"], "loaded");
        assert_eq!(messages[1]["role"], "system");
        assert_eq!(messages[1]["tools"][0]["function"]["name"], "late_tool");
        assert!(messages[1].get("content").is_none());
    }

    #[test]
    fn disabled_deferred_mode_keeps_request_parts_unchanged() {
        let messages = vec![msg("user", "hello")];
        let tools = vec![schema("tool")];
        let (actual_messages, actual_tools) =
            build_openai_request_parts(&messages, Some(&tools), false);
        assert_eq!(actual_messages, build_openai_messages_json(&messages));
        assert_eq!(
            serde_json::to_value(actual_tools).unwrap(),
            serde_json::to_value(Some(tools)).unwrap()
        );
    }

    fn provider(base_url: &str, model: &str) -> ProviderConfig {
        ProviderConfig {
            name: "test".to_string(),
            base_url: base_url.to_string(),
            api_key: "sk-test".to_string(),
            protocol: "openai".to_string(),
            model: model.to_string(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        }
    }

    #[test]
    fn test_standard_provider_keeps_stream_options() {
        let provider = provider("https://api.openai.com/v1", "gpt-5.4");
        let mut payload = serde_json::json!({"stream": true});

        apply_provider_options(&mut payload, &provider, false, true);

        assert_eq!(payload["stream_options"]["include_usage"], true);
        assert!(payload.get("extendParams").is_none());
    }

    #[test]
    fn sampling_temperature_is_passthrough() {
        let provider = provider("https://api.openai.com/v1", "gpt-5.4");
        assert_eq!(effective_temperature(&provider, Some(0.2)), Some(0.2));
        assert_eq!(effective_temperature(&provider, None), None);
    }

    #[test]
    fn test_model_custom_params_replace_default_thinking_param_and_protect_core_fields() {
        let mut provider = provider("https://api.example.com/v1", "example-model");
        provider.models = vec![crate::ai::config::ModelEntry {
            id: "example-model".to_string(),
            name: "Example Model".to_string(),
            enabled: None,
            max_output: None,
            context_window: None,
            capabilities: None,
            request_params: Some(serde_json::Map::from_iter([
                ("top_p".to_string(), serde_json::json!(0.95)),
                ("model".to_string(), serde_json::json!("must-not-win")),
            ])),
            thinking_params: Some(serde_json::Map::from_iter([(
                "reasoning_effort".to_string(),
                serde_json::json!("max"),
            )])),
            deferred_tools_mode: None,
            ..Default::default()
        }];
        let mut payload = serde_json::json!({"model": "example-model", "stream": true});

        apply_provider_options(&mut payload, &provider, true, true);

        assert_eq!(payload["model"], "example-model");
        assert_eq!(payload["top_p"], 0.95);
        assert_eq!(payload["reasoning_effort"], "max");
        assert!(payload.get("enable_thinking").is_none());
        assert_eq!(payload["stream_options"]["include_usage"], true);
    }

    #[test]
    fn test_build_messages_preserves_tool_calls_and_tool_call_id() {
        // 模拟 agent_runner 在多轮中产生的真实序列：
        // user → assistant(text + tool_calls) → tool(result) → user(下一轮)
        let messages = vec![
            msg("user", "加个书签"),
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "assistant".into(),
                content: "好的，让我查一下当前导航分类".into(),
                tool_calls: Some(vec![ToolCall {
                    id: "tc_1".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "data_crud".into(),
                        arguments: r#"{"domain":"nav","action":"query"}"#.into(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
            },
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "tool".into(),
                content: "找到 3 个分类".into(),
                tool_calls: None,
                tool_call_id: Some("tc_1".into()),
            },
            msg("user", "继续吧"),
        ];

        let payload = build_openai_messages_json(&messages);
        assert_eq!(payload.len(), 4);

        // 第 0 条：user
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[0]["content"], "加个书签");

        // 第 1 条：assistant，必须含 tool_calls
        assert_eq!(payload[1]["role"], "assistant");
        let tool_calls = payload[1]["tool_calls"]
            .as_array()
            .expect("assistant 消息必须保留 tool_calls 字段");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "tc_1");
        assert_eq!(tool_calls[0]["function"]["name"], "data_crud");

        // 第 2 条：tool，必须含 tool_call_id 且与上面对应
        assert_eq!(payload[2]["role"], "tool");
        assert_eq!(payload[2]["tool_call_id"], "tc_1");
        assert_eq!(payload[2]["content"], "找到 3 个分类");

        // 第 3 条：user
        assert_eq!(payload[3]["role"], "user");
    }

    #[test]
    fn test_build_messages_skips_empty_assistant() {
        let messages = vec![
            msg("user", "hi"),
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "assistant".into(),
                content: "".into(),
                tool_calls: None,
                tool_call_id: None,
            },
            msg("user", "still there?"),
        ];
        let payload = build_openai_messages_json(&messages);
        // 空 assistant 被跳过
        assert_eq!(payload.len(), 2);
        assert_eq!(payload[0]["content"], "hi");
        assert_eq!(payload[1]["content"], "still there?");
    }

    #[test]
    fn test_build_messages_does_not_merge_consecutive_tool() {
        // 一次 turn 多个工具调用 → 多条独立的 tool 消息，不能合并
        let messages = vec![
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "assistant".into(),
                content: "".into(),
                tool_calls: Some(vec![
                    ToolCall {
                        id: "tc_a".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "f".into(),
                            arguments: "{}".into(),
                        },
                        thought_signature: None,
                    },
                    ToolCall {
                        id: "tc_b".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "g".into(),
                            arguments: "{}".into(),
                        },
                        thought_signature: None,
                    },
                ]),
                tool_call_id: None,
            },
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "tool".into(),
                content: "result A".into(),
                tool_calls: None,
                tool_call_id: Some("tc_a".into()),
            },
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "tool".into(),
                content: "result B".into(),
                tool_calls: None,
                tool_call_id: Some("tc_b".into()),
            },
        ];
        let payload = build_openai_messages_json(&messages);
        assert_eq!(payload.len(), 3);
        assert_eq!(payload[1]["tool_call_id"], "tc_a");
        assert_eq!(payload[1]["content"], "result A");
        assert_eq!(payload[2]["tool_call_id"], "tc_b");
        assert_eq!(payload[2]["content"], "result B");
    }

    #[test]
    fn test_openai_tool_call_accepts_object_arguments() {
        let parsed = parse_openai_tool_call(&serde_json::json!({
            "id": "call_1",
            "type": "function",
            "function": {
                "name": "data_query",
                "arguments": {
                    "query_type": "base_5m",
                    "account_id": "211399118",
                    "bizdate": "20260709"
                }
            }
        }))
        .unwrap();

        assert_eq!(parsed.function.name, "data_query");
        let arguments: Value = serde_json::from_str(&parsed.function.arguments).unwrap();
        assert_eq!(arguments["query_type"], "base_5m");
        assert_eq!(arguments["account_id"], "211399118");
    }

    #[tokio::test]
    async fn test_chat_openai() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "choices": [{"message": {"content": "Hello!", "role": "assistant"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
            }"#,
            )
            .create();

        let client = OpenAiClient::new(
            ProviderConfig {
                name: "test".to_string(),
                base_url: server.url(),
                api_key: "sk-test".to_string(),
                protocol: "openai".to_string(),
                model: "gpt-4".to_string(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: None,
            },
            server.url(),
        );

        let response = client
            .chat(&LlmRequest {
                messages: vec![ChatMessage {
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    role: "user".to_string(),
                    content: "Hi".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                }],
                system_prompt: None,
                tools: None,
                stream: false,
                tool_choice: None,
                thinking: false,
                max_tokens: None,
                temperature: None,
            })
            .await
            .unwrap();
        assert_eq!(response.content, "Hello!");
        assert!(response.usage.is_some());
        let u = response.usage.unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.total_tokens, 15);
        mock.assert();
    }

    #[tokio::test]
    async fn test_chat_stream_openai_text_deltas() {
        let mut server = Server::new_async().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n\
                   data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
                   data: [DONE]\n\n";
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create();

        let client = OpenAiClient::new(
            ProviderConfig {
                name: "t".into(),
                base_url: server.url(),
                api_key: "sk".into(),
                protocol: "openai".into(),
                model: "gpt-4".into(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: None,
            },
            server.url(),
        );

        let request = LlmRequest {
            messages: vec![ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "user".to_string(),
                content: "hi".to_string(),
                tool_calls: None,
                tool_call_id: None,
            }],
            system_prompt: None,
            tools: None,
            stream: true,
            tool_choice: None,
            thinking: false,
            max_tokens: None,
            temperature: None,
        };

        let mut events: Vec<StreamEvent> = Vec::new();
        let mut s = client.chat_stream(&request);
        while let Some(ev) = s.next().await {
            events.push(ev);
        }

        // 期望：FirstToken → TextDelta("Hello") → TextDelta(" world") → Done(Some(usage))
        let texts: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, "Hello world");

        let first_token_count = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::FirstToken))
            .count();
        assert_eq!(first_token_count, 1, "FirstToken 应只触发一次");

        let usage = events.iter().find_map(|e| match e {
            StreamEvent::Done(u) => *u,
            _ => None,
        });
        assert_eq!(usage.map(|u| u.total_tokens), Some(5));
    }

    #[tokio::test]
    async fn tool_call_does_not_copy_reasoning_into_visible_text() {
        let mut server = Server::new_async().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"internal planning\"},\"finish_reason\":null}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"generate_image\",\"arguments\":\"{\\\"prompt\\\":\\\"图\\\"}\"}}]},\"finish_reason\":null}]}\n\n\
                   data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
                   data: [DONE]\n\n";
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create();
        let client = OpenAiClient::new(
            ProviderConfig {
                name: "t".into(),
                base_url: server.url(),
                api_key: "sk".into(),
                protocol: "openai".into(),
                model: "qwen".into(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: None,
            },
            server.url(),
        );
        let request = LlmRequest {
            messages: vec![msg("user", "生成图片")],
            system_prompt: Some("使用简体中文".into()),
            tools: None,
            stream: true,
            tool_choice: None,
            thinking: true,
            max_tokens: None,
            temperature: None,
        };

        let events = client.chat_stream(&request).collect::<Vec<_>>().await;
        assert!(events
            .iter()
            .any(|event| matches!(event, StreamEvent::ThinkingDelta(value) if value == "internal planning")));
        assert!(!events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(_))));
        assert!(events.iter().any(
            |event| matches!(event, StreamEvent::ToolCallStart { name, .. } if name == "generate_image")
        ));
    }

    #[tokio::test]
    async fn reasoning_only_without_tool_call_does_not_produce_text_delta() {
        let mut server = Server::new_async().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"deep thought\"},\"finish_reason\":null}]}\n\n\
                   data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n\
                   data: [DONE]\n\n";
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create();
        let client = OpenAiClient::new(
            ProviderConfig {
                name: "t".into(),
                base_url: server.url(),
                api_key: "sk".into(),
                protocol: "openai".into(),
                model: "qwen".into(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: None,
            },
            server.url(),
        );
        let request = LlmRequest {
            messages: vec![msg("user", "你好")],
            system_prompt: None,
            tools: None,
            stream: true,
            tool_choice: None,
            thinking: true,
            max_tokens: None,
            temperature: None,
        };

        let events = client.chat_stream(&request).collect::<Vec<_>>().await;
        // 思考内容只以 ThinkingDelta 出现一次，绝不顶替成正文
        assert!(events.iter().any(
            |event| matches!(event, StreamEvent::ThinkingDelta(value) if value == "deep thought")
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::TextDelta(_))),
            "仅 reasoning_content 时不应产生 TextDelta"
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, StreamEvent::Done(_))));
    }
}
