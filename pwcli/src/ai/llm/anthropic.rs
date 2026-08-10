use crate::ai::llm::models::*;
use anyhow::Result;
use futures::stream::{BoxStream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use tracing::{debug, error, info};

// 默认输出 token 上限 = 128_000（注意是十进制 128K，不是二进制 128 KiB=131072）。
// 这是 Claude Opus 4.x 系列 max_output_tokens 的精确上限——发 131072 会被 API
// 拒（"exceeds the model limit of 128000"）。设这么大是为了：
// (1) 避免 long tool_call arguments JSON 被截断，导致必填字段反序列化丢失，
//     进而触发"模型重复同一个失败 tool call"的死循环；
// (2) 让长任务（Wiki 全文重写、复杂代码生成、大段总结）一次出完，不被截尾。
//
// 兜底常量已废弃，所有 max_tokens fallback 走 `crate::ai::llm::default_max_tokens_for(model)`：
// Opus → 128K，其他（Sonnet/Haiku/Gemini/Qwen/GPT-5/Kimi 等）→ 64K。
// 单独留 thinking 路径的最低保障：2048 是 thinking budget 默认值，max_tokens 必须 > 它。

/// Build a Claude Code-compatible User-Agent. The version is detected from the
/// local CLI when available and falls back to a stable default.
const FALLBACK_CLAUDE_VERSION: &str = "2.1.150";

static DETECTED_CLAUDE_VERSION: OnceLock<String> = OnceLock::new();
static PROBE_SPAWNED: OnceLock<()> = OnceLock::new();

async fn probe_claude_version() -> Option<String> {
    let output = tokio::process::Command::new("claude")
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = std::str::from_utf8(&output.stdout).ok()?;
    // "2.1.159 (Claude Code)\n" → "2.1.159"
    let version: String = stdout
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if version.is_empty() || !version.contains('.') {
        return None;
    }
    Some(version)
}

pub(crate) fn claude_code_user_agent() -> String {
    if PROBE_SPAWNED.set(()).is_ok() {
        tokio::spawn(async {
            if let Some(v) = probe_claude_version().await {
                let _ = DETECTED_CLAUDE_VERSION.set(v.clone());
                info!(detected_claude_version = %v, "claude-cli version probed");
            } else {
                debug!(fallback = %FALLBACK_CLAUDE_VERSION, "claude --version probe failed, using fallback");
            }
        });
    }
    let version = DETECTED_CLAUDE_VERSION
        .get()
        .map(|s| s.as_str())
        .unwrap_or(FALLBACK_CLAUDE_VERSION);
    format!("claude-cli/{} (external, cli)", version)
}

fn apply_thinking(payload: &mut Value, provider: &ProviderConfig, enabled: bool) {
    if !enabled {
        return;
    }
    if let Some(params) = provider
        .current_model_entry()
        .and_then(|model| model.thinking_params.as_ref())
    {
        for key in params.keys() {
            if matches!(
                key.as_str(),
                "enable_thinking" | "reasoning_effort" | "reasoning" | "generationConfig"
            ) {
                debug!(
                    protocol = "anthropic_messages",
                    key = %key,
                    "thinkingParams key is unusual for anthropic_messages and may be ignored upstream"
                );
            }
        }
        // Explicit `thinking` wins. A configured budget opts into legacy
        // manual thinking; otherwise modern Anthropic models use adaptive
        // thinking and `output_config.effort`.
        if let Some(value) = params.get("thinking") {
            payload["thinking"] = value.clone();
        } else if let Some(budget) = params.get("budget_tokens").and_then(Value::as_u64) {
            payload["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget
            });
        } else {
            payload["thinking"] = serde_json::json!({ "type": "adaptive" });
        }
        for (key, value) in params {
            match key.as_str() {
                "thinking" | "budget_tokens" => {}
                "reasoning_effort" => {
                    payload["output_config"]["effort"] = value.clone();
                }
                _ => {
                    payload[key] = value.clone();
                }
            }
        }
    } else {
        payload["thinking"] = serde_json::json!({ "type": "adaptive" });
    }
    // Optional anthropic-version / user-agent overrides via requestParams happen
    // at request construction time.
}

fn effective_temperature(_provider: &ProviderConfig, requested: Option<f32>) -> Option<f32> {
    requested
}

/// Convert the internal tool schema to the common subset accepted by
/// Anthropic-compatible gateways.
///
/// Some gateways reject `oneOf`, `anyOf`, or `allOf` when they appear at the
/// input schema root, even though they accept those keywords for nested
/// properties. The complete schema remains in `ToolRegistry` and is used for
/// local argument validation, so this wire-only downgrade does not weaken the
/// execution boundary.
fn anthropic_tools(tools: &[ToolSchema], provider: &ProviderConfig) -> Vec<Value> {
    let supports_root_combinators = super::tool_schema::supports_root_combinators(provider);
    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.function.name,
                "description": tool.function.description,
                "input_schema": super::tool_schema::normalize_root(
                    &tool.function.parameters,
                    supports_root_combinators,
                ),
            })
        })
        .collect()
}

fn strip_root_combinators_from_payload_tools(payload: &mut Value) -> bool {
    let Some(tools) = payload.get_mut("tools").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for tool in tools {
        let Some(schema) = tool.get_mut("input_schema").and_then(Value::as_object_mut) else {
            continue;
        };
        for keyword in ["oneOf", "anyOf", "allOf"] {
            changed |= schema.remove(keyword).is_some();
        }
        if !schema.contains_key("type") {
            schema.insert("type".into(), Value::String("object".into()));
            changed = true;
        }
    }
    changed
}

fn is_root_combinator_schema_error(status: u16, body: &str) -> bool {
    status == 400
        && body.contains("input_schema")
        && body.contains("does not support")
        && ["oneOf", "anyOf", "allOf"]
            .iter()
            .any(|keyword| body.contains(keyword))
}

fn provider_request_param(provider: &ProviderConfig, key: &str) -> Option<String> {
    provider
        .current_model_entry()
        .and_then(|model| model.request_params.as_ref())
        .and_then(|params| params.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 构造 Anthropic Messages API 的 messages 数组。
///
/// Anthropic 协议关键点：
/// - assistant 调工具：content 必须是 [{type:"text"}, {type:"tool_use", id, name, input}] 数组
/// - 工具结果回灌：转成 user 消息，content = [{type:"tool_result", tool_use_id, content}]
/// - system 单独通过 `system` 参数传，不放在 messages 里
/// - 不合并相邻同 role 消息（与 OpenAI 同因——会破坏 tool_use_id 1:1 对应）
fn build_anthropic_messages_json(messages: &[ChatMessage]) -> Vec<Value> {
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        match m.role.as_str() {
            "system" => continue, // system 走 payload 顶层 system 字段
            "user" => {
                // Anthropic vision schema:
                //   content: [
                //     { "type": "text",  "text": "..." },
                //     { "type": "image", "source": { "type":"base64", "media_type":"image/png", "data":"..." } }
                //   ]
                // 形状跟 OpenAI 的 image_url 不一样：是 `image` 不是 `image_url`，
                // 而且要把 data URL 拆成 media_type + data 两个字段。
                if m.images.is_empty() {
                    out.push(serde_json::json!({"role": "user", "content": m.content}));
                } else {
                    let mut blocks: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(serde_json::json!({
                            "type": "text",
                            "text": m.content,
                        }));
                    }
                    for img in &m.images {
                        blocks.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": img.media_type,
                                "data": img.data,
                            }
                        }));
                    }
                    out.push(serde_json::json!({"role": "user", "content": blocks}));
                }
            }
            "assistant" => {
                let has_text = !m.content.trim().is_empty();
                let has_tool_calls = m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());
                if !has_text && !has_tool_calls {
                    continue;
                }
                if has_tool_calls {
                    // 多块结构：先 text（如果有），再各 tool_use 块
                    let mut blocks: Vec<Value> = Vec::new();
                    if has_text {
                        blocks.push(serde_json::json!({
                            "type": "text",
                            "text": m.content,
                        }));
                    }
                    if let Some(tcs) = &m.tool_calls {
                        for tc in tcs {
                            // arguments 是 JSON 字符串，需要解析为对象作为 input
                            let input: Value = serde_json::from_str(&tc.function.arguments)
                                .unwrap_or_else(|_| Value::Object(Default::default()));
                            blocks.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.function.name,
                                "input": input,
                            }));
                        }
                    }
                    out.push(serde_json::json!({
                        "role": "assistant",
                        "content": blocks,
                    }));
                } else {
                    out.push(serde_json::json!({
                        "role": "assistant",
                        "content": m.content,
                    }));
                }
            }
            "tool" => {
                let id = m.tool_call_id.clone().unwrap_or_default();
                let (_, clean_content) = crate::ai::llm::deferred_tools::split(&m.content);
                if crate::ai::llm::image_marker::contains_marker(clean_content) {
                    let segs = crate::ai::llm::image_marker::parse_segments(clean_content);
                    let mut blocks: Vec<Value> = Vec::with_capacity(segs.len());
                    for seg in segs {
                        match seg {
                            crate::ai::llm::image_marker::Segment::Text(t) => {
                                blocks.push(serde_json::json!({"type":"text","text": t}));
                            }
                            crate::ai::llm::image_marker::Segment::Image { mime, base64 } => {
                                blocks.push(serde_json::json!({
                                    "type":"image",
                                    "source": {
                                        "type":"base64",
                                        "media_type": mime,
                                        "data": base64,
                                    }
                                }));
                            }
                        }
                    }
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": blocks,
                        }]
                    }));
                } else {
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": clean_content,
                        }]
                    }));
                }
            }
            _ => {
                out.push(serde_json::json!({"role": m.role, "content": m.content}));
            }
        }
    }
    out
}

pub struct AnthropicClient {
    http: Client,
    provider: ProviderConfig,
    backend_url: String,
}

impl AnthropicClient {
    pub fn new(provider: ProviderConfig, backend_url: String) -> Self {
        Self {
            http: crate::ai::http::default_client(crate::ai::http::ClientProfile::Llm),
            provider,
            backend_url,
        }
    }

    pub async fn chat(&self, request: &LlmRequest) -> Result<AiResponse> {
        let use_proxy = needs_proxy(&self.provider);
        let (url, headers) = if use_proxy {
            let url = format!("{}/api/proxy/anthropic", self.backend_url);
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("Content-Type", "application/json".parse()?);
            headers.insert(
                "X-Provider-Id",
                crate::ai::provider::provider_id(&self.provider).parse()?,
            );
            (url, headers)
        } else {
            let url = format!("{}/v1/messages", self.provider.base_url);
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("Content-Type", "application/json".parse()?);
            insert_anthropic_auth_header(&mut headers, &self.provider)?;
            let anthropic_version = provider_request_param(&self.provider, "anthropic-version")
                .unwrap_or_else(|| "2023-06-01".into());
            let user_agent = provider_request_param(&self.provider, "user-agent")
                .unwrap_or_else(claude_code_user_agent);
            headers.insert("anthropic-version", anthropic_version.parse()?);
            headers.insert("User-Agent", user_agent.parse()?);
            apply_model_headers(&mut headers, &self.provider)?;
            (url, headers)
        };

        let anthropic_messages = build_anthropic_messages_json(&request.messages);

        let mut payload = serde_json::json!({
            "model": self.provider.model,
            "max_tokens": self.provider.effective_max_tokens(request.max_tokens),
            "system": request.system_prompt.as_deref().unwrap_or("You are a helpful assistant."),
            "messages": anthropic_messages,
        });
        if let Some(temperature) = effective_temperature(&self.provider, request.temperature) {
            payload["temperature"] = serde_json::json!(temperature);
        }

        if let Some(tools) = &request.tools {
            payload["tools"] = serde_json::to_value(anthropic_tools(tools, &self.provider))?;
        }
        if !super::tool_schema::supports_root_combinators(&self.provider) {
            strip_root_combinators_from_payload_tools(&mut payload);
        }

        apply_thinking(&mut payload, &self.provider, request.thinking);
        if request.thinking {
            payload["max_tokens"] =
                serde_json::json!(self.provider.effective_max_tokens(request.max_tokens));
        }

        let mut attempt: u32 = 0;
        let mut schema_fallback_attempted = false;
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
            if !schema_fallback_attempted
                && is_root_combinator_schema_error(status.as_u16(), &text)
                && strip_root_combinators_from_payload_tools(&mut payload)
            {
                schema_fallback_attempted = true;
                info!(model = %self.provider.model, "retrying with restricted Anthropic tool schemas");
                continue;
            }
            if super::retry::is_provider_error(status.as_u16(), &text) {
                if let Some(delay) =
                    super::retry::next_backoff_with_hint(attempt, retry_after.as_deref())
                {
                    attempt += 1;
                    info!(
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
            error!(model = %self.provider.model, status = %status, body = %text, "anthropic chat HTTP error");
            anyhow::bail!("AI request failed ({}): {}", status, text);
        };

        let data: Value = res.json().await?;
        let mut text = String::new();
        let mut tool_calls = Vec::new();

        if let Some(blocks) = data["content"].as_array() {
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            text.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        tool_calls.push(ToolCall {
                            id: block["id"].as_str().unwrap_or("").to_string(),
                            kind: "function".to_string(),
                            function: FunctionCall {
                                name: block["name"].as_str().unwrap_or("").to_string(),
                                arguments: serde_json::to_string(&block["input"])
                                    .unwrap_or_default(),
                            },
                            thought_signature: None,
                        });
                    }
                    _ => {}
                }
            }
        }

        let usage = match &data["usage"] {
            serde_json::Value::Null => None,
            u => {
                let prompt_tokens = anthropic_input_tokens(u);
                let completion_tokens = u["output_tokens"].as_u64().unwrap_or(0) as u32;
                Some(TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens: prompt_tokens.saturating_add(completion_tokens),
                })
            }
        };

        Ok(AiResponse {
            content: text,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            usage,
        })
    }
}

fn anthropic_input_tokens(usage: &Value) -> u32 {
    [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .into_iter()
    .fold(0_u32, |total, field| {
        total.saturating_add(usage[field].as_u64().unwrap_or(0) as u32)
    })
}

fn needs_proxy(provider: &ProviderConfig) -> bool {
    provider.uses_proxy()
}

fn insert_anthropic_auth_header(
    headers: &mut reqwest::header::HeaderMap,
    provider: &ProviderConfig,
) -> Result<()> {
    if crate::ai::provider::provider_kind(provider) == crate::ai::provider::ProviderKind::KimiCoding
    {
        headers.insert(
            "Authorization",
            format!("Bearer {}", provider.api_key).parse()?,
        );
    } else {
        headers.insert("x-api-key", provider.api_key.parse()?);
    }
    Ok(())
}

fn apply_model_headers(
    headers: &mut reqwest::header::HeaderMap,
    provider: &ProviderConfig,
) -> Result<()> {
    let Some(model_headers) = provider
        .current_model_entry()
        .and_then(|model| model.headers.as_ref())
    else {
        return Ok(());
    };
    for (name, value) in model_headers {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "x-api-key" | "content-length" | "host"
        ) {
            continue;
        }
        let Some(value) = value.as_str() else {
            continue;
        };
        headers.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
            reqwest::header::HeaderValue::from_str(value)?,
        );
    }
    Ok(())
}

impl AnthropicClient {
    /// 流式对话：消费 Anthropic SSE，逐 delta 产出 StreamEvent
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
                let url = format!("{}/api/proxy/anthropic", backend_url);
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("Content-Type", "application/json".parse().unwrap());
                if let Ok(v) = crate::ai::provider::provider_id(&provider).parse() {
                    h.insert("X-Provider-Id", v);
                }
                (url, h)
            } else {
                let url = format!("{}/v1/messages", provider.base_url);
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("Content-Type", "application/json".parse().unwrap());
                if insert_anthropic_auth_header(&mut h, &provider).is_err() {
                    yield StreamEvent::Error("invalid provider authentication header".into());
                    return;
                }
                h.insert("anthropic-version", "2023-06-01".parse().unwrap());
                h.insert("User-Agent", claude_code_user_agent().parse().unwrap());
                if apply_model_headers(&mut h, &provider).is_err() {
                    yield StreamEvent::Error("invalid provider model header".into());
                    return;
                }
                (url, h)
            };

            let anthropic_messages = build_anthropic_messages_json(&request.messages);

            let mut payload = serde_json::json!({
                "model": provider.model,
                "max_tokens": provider.effective_max_tokens(request.max_tokens),
                "system": request.system_prompt.as_deref().unwrap_or("You are a helpful assistant."),
                "messages": anthropic_messages,
                "stream": true,
            });
            if let Some(temperature) = effective_temperature(&provider, request.temperature) {
                payload["temperature"] = serde_json::json!(temperature);
            }

            if let Some(tools) = &request.tools {
                if let Ok(v) = serde_json::to_value(anthropic_tools(tools, &provider)) { payload["tools"] = v; }
            }
            if !super::tool_schema::supports_root_combinators(&provider) {
                strip_root_combinators_from_payload_tools(&mut payload);
            }

            apply_thinking(&mut payload, &provider, request.thinking);
            if request.thinking {
                payload["max_tokens"] = serde_json::json!(provider.effective_max_tokens(request.max_tokens));
            }

            debug!(model = %provider.model, url = %url, "anthropic stream request");
            let res = {
                let mut attempt: u32 = 0;
                let mut schema_fallback_attempted = false;
                loop {
                    let res = match http.post(&url).headers(headers.clone()).json(&payload).send().await {
                        Ok(r) => r,
                        Err(e) => {
                            error!(model = %provider.model, error = %e, "anthropic stream network error");
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
                    if !schema_fallback_attempted
                        && is_root_combinator_schema_error(status.as_u16(), &text)
                        && strip_root_combinators_from_payload_tools(&mut payload)
                    {
                        schema_fallback_attempted = true;
                        info!(model = %provider.model, "retrying stream with restricted Anthropic tool schemas");
                        continue;
                    }
                    if super::retry::is_provider_error(status.as_u16(), &text) {
                        if let Some(delay) = super::retry::next_backoff_with_hint(attempt, retry_after.as_deref()) {
                            attempt += 1;
                            info!(
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
                    error!(model = %provider.model, status = %status, body = %text, "anthropic stream HTTP error");
                    yield StreamEvent::Error(format!("AI request failed ({}): {}", status, text));
                    return;
                }
            };

            // ---- 解析 SSE ----
            let mut byte_stream = res.bytes_stream();
            let mut buffer = String::new();
            let mut emitted_first = false;
            // 按 content_block index 累积工具调用
            let mut tool_blocks: std::collections::HashMap<u64, (String, String, String)> = std::collections::HashMap::new(); // idx -> (id, name, args)
            let mut input_tokens: u32 = 0;

            while let Some(chunk) = byte_stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => { yield StreamEvent::Error(format!("stream read: {}", e)); return; }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(pos) = buffer.find("\n\n") {
                    let event_block = buffer[..pos].to_string();
                    buffer.drain(..pos + 2);

                    let mut data_line: Option<String> = None;
                    for line in event_block.lines() {
                        let line = line.trim_start();
                        if let Some(rest) = line.strip_prefix("data:") {
                            data_line = Some(rest.trim().to_string());
                        }
                    }
                    let Some(data) = data_line else { continue; };
                    if data.is_empty() { continue; }
                    let v: Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let event_type = v["type"].as_str().unwrap_or("");

                    // 业务级错误（HTTP 200 但 body 是 {"type":"error", "error": {...}}）
                    if event_type == "error" {
                        let msg = v["error"]["message"].as_str().unwrap_or("LLM 业务错误");
                        yield StreamEvent::Error(msg.to_string());
                        return;
                    }

                    match event_type {
                        "message_start" => {
                            input_tokens = anthropic_input_tokens(&v["message"]["usage"]);
                        }
                        "content_block_start" => {
                            let idx = v["index"].as_u64().unwrap_or(0);
                            let block = &v["content_block"];
                            if block["type"].as_str() == Some("tool_use") {
                                let id = block["id"].as_str().unwrap_or("").to_string();
                                let name = block["name"].as_str().unwrap_or("").to_string();
                                tool_blocks.insert(idx, (id.clone(), name.clone(), String::new()));
                                yield StreamEvent::ToolCallStart { id, name, thought_signature: None };
                            }
                        }
                        "content_block_delta" => {
                            let delta = &v["delta"];
                            match delta["type"].as_str() {
                                Some("text_delta") => {
                                    if let Some(t) = delta["text"].as_str() {
                                        if !t.is_empty() {
                                            if !emitted_first {
                                                emitted_first = true;
                                                yield StreamEvent::FirstToken;
                                            }
                                            yield StreamEvent::TextDelta(t.to_string());
                                        }
                                    }
                                }
                                Some("input_json_delta") => {
                                    let idx = v["index"].as_u64().unwrap_or(0);
                                    if let Some((id, _, args)) = tool_blocks.get_mut(&idx) {
                                        if let Some(part) = delta["partial_json"].as_str() {
                                            args.push_str(part);
                                            yield StreamEvent::ToolCallDelta {
                                                id: id.clone(),
                                                arguments_delta: part.to_string(),
                                            };
                                        }
                                    }
                                }
                                Some("thinking_delta") => {
                                    if let Some(t) = delta["thinking"].as_str() {
                                        if !t.is_empty() {
                                            yield StreamEvent::ThinkingDelta(t.to_string());
                                        }
                                    }
                                }
                                // signature_delta — 加密签名，不暴露给前端
                                _ => {}
                            }
                        }
                        "content_block_stop" => {
                            let idx = v["index"].as_u64().unwrap_or(0);
                            if let Some((id, _, _)) = tool_blocks.remove(&idx) {
                                if !id.is_empty() {
                                    yield StreamEvent::ToolCallEnd { id };
                                }
                            }
                        }
                        "message_delta" => {
                            if let Some(stop_reason) = v["delta"]["stop_reason"].as_str() {
                                yield StreamEvent::ResponseStop(StopReason::from_provider(stop_reason));
                            }
                            let output_tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
                            let usage = TokenUsage {
                                prompt_tokens: input_tokens,
                                completion_tokens: output_tokens,
                                total_tokens: input_tokens + output_tokens,
                            };
                            yield StreamEvent::Done(Some(usage));
                            return;
                        }
                        "message_stop" => {
                            yield StreamEvent::Done(None);
                            return;
                        }
                        _ => {}
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

    #[test]
    fn anthropic_tool_schema_removes_only_root_combinators() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "domain": { "type": "string" },
                "value": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "array" }
                    ]
                }
            },
            "required": ["domain"],
            "oneOf": [
                { "required": ["domain"] },
                { "required": ["domains"] }
            ],
            "anyOf": [{ "required": ["domain"] }],
            "allOf": [{ "required": ["domain"] }],
            "additionalProperties": false
        });

        let normalized = crate::ai::llm::tool_schema::normalize_root(&schema, false);

        assert!(normalized.get("oneOf").is_none());
        assert!(normalized.get("anyOf").is_none());
        assert!(normalized.get("allOf").is_none());
        assert_eq!(normalized["type"], "object");
        assert_eq!(normalized["required"], serde_json::json!(["domain"]));
        assert!(normalized["properties"]["value"].get("oneOf").is_some());
        assert!(
            schema.get("oneOf").is_some(),
            "source schema must stay intact"
        );
    }

    #[test]
    fn anthropic_tool_schema_is_unchanged_by_default() {
        let schema = serde_json::json!({
            "type": "object",
            "oneOf": [{ "required": ["domain"] }]
        });

        assert_eq!(
            crate::ai::llm::tool_schema::normalize_root(&schema, true),
            schema
        );
    }

    #[test]
    fn anthropic_tools_apply_configured_wire_schema_normalization() {
        let tools = vec![ToolSchema {
            kind: "function".into(),
            function: FunctionSchema {
                name: "web_search_domains".into(),
                description: "discover domains".into(),
                parameters: serde_json::json!({
                    "properties": {
                        "domain": { "type": "string" },
                        "domains": { "type": "array", "items": { "type": "string" } }
                    },
                    "oneOf": [
                        { "required": ["domain"] },
                        { "required": ["domains"] }
                    ]
                }),
            },
        }];
        let provider = ProviderConfig {
            name: "restricted-gateway".into(),
            base_url: "https://example.test".into(),
            api_key: "secret".into(),
            protocol: "anthropic_messages".into(),
            model: "claude-opus-5".into(),
            models: vec![crate::ai::config::ModelEntry {
                id: "claude-opus-5".into(),
                name: "Claude Opus 5".into(),
                capabilities: Some(crate::ai::config::ModelCapabilities {
                    tool_schema_top_level_combinators: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            use_proxy: Some(true),
            compat_profile: None,
        };

        let serialized = anthropic_tools(&tools, &provider);

        assert_eq!(serialized[0]["input_schema"]["type"], "object");
        assert!(serialized[0]["input_schema"].get("oneOf").is_none());
        assert!(tools[0].function.parameters.get("oneOf").is_some());
    }

    #[test]
    fn final_payload_guard_normalizes_every_tool() {
        let mut payload = serde_json::json!({
            "tools": (0..20).map(|index| serde_json::json!({
                "name": format!("tool_{index}"),
                "input_schema": {
                    "properties": { "value": { "type": "string" } },
                    "oneOf": [{ "required": ["value"] }],
                    "anyOf": [{ "required": ["value"] }],
                    "allOf": [{ "required": ["value"] }]
                }
            })).collect::<Vec<_>>()
        });

        assert!(strip_root_combinators_from_payload_tools(&mut payload));
        for tool in payload["tools"].as_array().unwrap() {
            let schema = &tool["input_schema"];
            assert_eq!(schema["type"], "object");
            assert!(schema.get("oneOf").is_none());
            assert!(schema.get("anyOf").is_none());
            assert!(schema.get("allOf").is_none());
        }
    }

    #[tokio::test]
    async fn every_registered_tool_passes_the_restricted_payload_gate() {
        use crate::runtime::backend::BackendClient;
        use crate::runtime::tools::register::register_all_tools;
        use crate::runtime::tools::registry::ToolRegistry;
        use std::sync::Arc;

        let backend = Arc::new(BackendClient::new("http://127.0.0.1:9"));
        let mut registry = ToolRegistry::new();
        register_all_tools(&mut registry, backend);
        let schemas = registry.to_schemas();
        assert!(schemas.len() >= 30, "unexpectedly small tool registry");

        let mut payload = serde_json::json!({
            "tools": schemas.iter().map(|tool| serde_json::json!({
                "name": tool.function.name,
                "description": tool.function.description,
                "input_schema": tool.function.parameters,
            })).collect::<Vec<_>>()
        });
        assert!(
            payload["tools"].as_array().unwrap().iter().any(|tool| {
                ["oneOf", "anyOf", "allOf"]
                    .iter()
                    .any(|keyword| tool["input_schema"].get(keyword).is_some())
            }),
            "fixture must contain at least one restricted root combinator"
        );

        strip_root_combinators_from_payload_tools(&mut payload);

        for (index, tool) in payload["tools"].as_array().unwrap().iter().enumerate() {
            let schema = &tool["input_schema"];
            let name = tool["name"].as_str().unwrap_or("unknown");
            assert_eq!(schema["type"], "object", "tools.{index} ({name})");
            for keyword in ["oneOf", "anyOf", "allOf"] {
                assert!(
                    schema.get(keyword).is_none(),
                    "tools.{index} ({name}) still contains root {keyword}"
                );
            }
        }
    }

    #[test]
    fn recognizes_only_the_targeted_provider_schema_error() {
        assert!(is_root_combinator_schema_error(
            400,
            "tools.15.custom.input_schema: input_schema does not support oneOf, allOf, or anyOf at the top level"
        ));
        assert!(!is_root_combinator_schema_error(500, "input_schema oneOf"));
        assert!(!is_root_combinator_schema_error(
            400,
            "unrelated bad request"
        ));
    }

    #[test]
    fn cached_tokens_count_toward_active_context() {
        let usage = serde_json::json!({
            "input_tokens": 10,
            "cache_creation_input_tokens": 20,
            "cache_read_input_tokens": 30
        });
        assert_eq!(anthropic_input_tokens(&usage), 60);
    }

    #[test]
    fn standard_anthropic_defaults_to_adaptive_thinking() {
        let provider = ProviderConfig {
            name: "Anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "secret".into(),
            protocol: "anthropic".into(),
            model: "claude-sonnet".into(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        };
        let mut payload = serde_json::json!({ "max_tokens": 64000 });

        apply_thinking(&mut payload, &provider, true);

        assert_eq!(
            payload["thinking"],
            serde_json::json!({ "type": "adaptive" })
        );
    }

    #[test]
    fn anthropic_maps_legacy_reasoning_effort_to_output_config() {
        let provider = ProviderConfig {
            name: "Anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "secret".into(),
            protocol: "anthropic_messages".into(),
            model: "claude-opus-5".into(),
            models: vec![crate::ai::config::ModelEntry {
                id: "claude-opus-5".into(),
                name: "Claude Opus 5".into(),
                thinking_params: Some(serde_json::Map::from_iter([(
                    "reasoning_effort".into(),
                    Value::String("high".into()),
                )])),
                ..Default::default()
            }],
            use_proxy: None,
            compat_profile: None,
        };
        let mut payload = serde_json::json!({});

        apply_thinking(&mut payload, &provider, true);

        assert_eq!(payload["thinking"], serde_json::json!({"type": "adaptive"}));
        assert_eq!(payload["output_config"]["effort"], "high");
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn standard_anthropic_keeps_requested_temperature() {
        let provider = ProviderConfig {
            name: "Anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "secret".into(),
            protocol: "anthropic".into(),
            model: "claude-sonnet".into(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        };
        assert_eq!(effective_temperature(&provider, Some(0.2)), Some(0.2));
        assert_eq!(effective_temperature(&provider, None), None);
    }

    use mockito::Server;

    #[test]
    fn kimi_uses_bearer_while_custom_anthropic_uses_api_key() {
        let provider = |compat_profile: Option<&str>| ProviderConfig {
            name: "test".into(),
            base_url: "https://example.test".into(),
            api_key: "secret".into(),
            protocol: "anthropic_messages".into(),
            model: "model".into(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: compat_profile.map(str::to_string),
        };

        let mut kimi_headers = reqwest::header::HeaderMap::new();
        insert_anthropic_auth_header(&mut kimi_headers, &provider(Some("builtin:kimi-coding")))
            .unwrap();
        assert_eq!(kimi_headers["Authorization"], "Bearer secret");
        assert!(!kimi_headers.contains_key("x-api-key"));

        let mut custom_headers = reqwest::header::HeaderMap::new();
        insert_anthropic_auth_header(&mut custom_headers, &provider(None)).unwrap();
        assert_eq!(custom_headers["x-api-key"], "secret");
        assert!(!custom_headers.contains_key("Authorization"));
    }

    #[test]
    fn model_headers_override_user_agent_without_overriding_auth() {
        let provider = ProviderConfig {
            name: "Kimi".into(),
            base_url: "https://api.kimi.com/coding".into(),
            api_key: "secret".into(),
            protocol: "anthropic_messages".into(),
            model: "k3".into(),
            models: vec![crate::ai::config::ModelEntry {
                id: "k3".into(),
                name: "K3".into(),
                headers: Some(serde_json::Map::from_iter([
                    ("User-Agent".into(), Value::String("KimiCLI/1.5".into())),
                    ("Authorization".into(), Value::String("forbidden".into())),
                ])),
                ..Default::default()
            }],
            use_proxy: None,
            compat_profile: Some("builtin:kimi-coding".into()),
        };
        let mut headers = reqwest::header::HeaderMap::new();
        insert_anthropic_auth_header(&mut headers, &provider).unwrap();
        apply_model_headers(&mut headers, &provider).unwrap();
        assert_eq!(headers["User-Agent"], "KimiCLI/1.5");
        assert_eq!(headers["Authorization"], "Bearer secret");
    }

    #[test]
    fn test_build_messages_assistant_with_tool_use_blocks() {
        // assistant 含 tool_calls 时，content 必须是 [text, tool_use] 结构
        let messages = vec![
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "user".into(),
                content: "加个书签".into(),
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "assistant".into(),
                content: "让我查一下".into(),
                tool_calls: Some(vec![ToolCall {
                    id: "tu_1".into(),
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
                content: "found 3".into(),
                tool_calls: None,
                tool_call_id: Some("tu_1".into()),
            },
        ];
        let out = build_anthropic_messages_json(&messages);
        assert_eq!(out.len(), 3);

        // user 还是单字符串 content
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "加个书签");

        // assistant: content 是 blocks 数组
        assert_eq!(out[1]["role"], "assistant");
        let blocks = out[1]["content"]
            .as_array()
            .expect("assistant content blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "让我查一下");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "tu_1");
        assert_eq!(blocks[1]["name"], "data_crud");
        assert_eq!(blocks[1]["input"]["domain"], "nav");
        assert_eq!(blocks[1]["input"]["action"], "query");

        // tool 消息变成 user with tool_result block
        assert_eq!(out[2]["role"], "user");
        let result_blocks = out[2]["content"].as_array().expect("tool_result blocks");
        assert_eq!(result_blocks[0]["type"], "tool_result");
        assert_eq!(result_blocks[0]["tool_use_id"], "tu_1");
        assert_eq!(result_blocks[0]["content"], "found 3");
    }

    #[test]
    fn test_build_messages_system_skipped() {
        let messages = vec![
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "system".into(),
                content: "ignore me".into(),
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                images: Vec::new(),
                generated_images: Vec::new(),
                role: "user".into(),
                content: "hi".into(),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let out = build_anthropic_messages_json(&messages);
        // system 走 payload 顶层，messages 里只有 user
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
    }

    #[test]
    fn test_build_messages_user_with_images_emits_vision_blocks() {
        let messages = vec![ChatMessage {
            images: vec![
                ImageAttachment {
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo".into(),
                },
                ImageAttachment {
                    media_type: "image/jpeg".into(),
                    data: "/9j/4AAQ".into(),
                },
            ],
            generated_images: vec![],
            role: "user".into(),
            content: "describe these".into(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let out = build_anthropic_messages_json(&messages);
        assert_eq!(out.len(), 1);
        let content = &out[0]["content"];
        let arr = content.as_array().expect("content should be array");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "describe these");
        assert_eq!(arr[1]["type"], "image");
        assert_eq!(arr[1]["source"]["type"], "base64");
        assert_eq!(arr[1]["source"]["media_type"], "image/png");
        assert_eq!(arr[1]["source"]["data"], "iVBORw0KGgo");
        assert_eq!(arr[2]["source"]["media_type"], "image/jpeg");
    }

    #[test]
    fn test_build_messages_user_no_images_keeps_string_content() {
        let messages = vec![ChatMessage {
            images: Vec::new(),
            generated_images: Vec::new(),
            role: "user".into(),
            content: "hello".into(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let out = build_anthropic_messages_json(&messages);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"], "hello");
    }

    #[tokio::test]
    async fn test_chat_anthropic() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "content": [
                    {"type": "text", "text": "Hello from Claude!"}
                ],
                "usage": {"input_tokens": 10, "output_tokens": 5}
            }"#,
            )
            .create();

        let client = AnthropicClient::new(
            ProviderConfig {
                name: "test".to_string(),
                base_url: server.url(),
                api_key: "sk-test".to_string(),
                protocol: "anthropic".to_string(),
                model: "claude-sonnet".to_string(),
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
        assert_eq!(response.content, "Hello from Claude!");
        assert!(response.usage.is_some());
        let u = response.usage.unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 5);
        assert_eq!(u.total_tokens, 15);
        mock.assert();
    }

    #[tokio::test]
    async fn test_chat_anthropic_with_tool_use() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "content": [
                    {"type": "text", "text": ""},
                    {"type": "tool_use", "id": "tu_1", "name": "data_crud", "input": {"domain": "todo", "action": "query"}}
                ],
                "usage": {"input_tokens": 20, "output_tokens": 10}
            }"#)
            .create();

        let client = AnthropicClient::new(
            ProviderConfig {
                name: "test".to_string(),
                base_url: server.url(),
                api_key: "sk-test".to_string(),
                protocol: "anthropic".to_string(),
                model: "claude-sonnet".to_string(),
                models: Vec::new(),
                use_proxy: None,
                compat_profile: None,
            },
            server.url(),
        );

        let tools = vec![ToolSchema {
            kind: "function".to_string(),
            function: FunctionSchema {
                name: "data_crud".to_string(),
                description: "通用数据 CRUD".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "domain": {"type": "string"},
                        "action": {"type": "string"}
                    }
                }),
            },
        }];

        let response = client
            .chat(&LlmRequest {
                messages: vec![ChatMessage {
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    role: "user".to_string(),
                    content: "列出待办任务".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                }],
                system_prompt: None,
                tools: Some(tools),
                stream: false,
                tool_choice: None,
                thinking: false,
                max_tokens: None,
                temperature: None,
            })
            .await
            .unwrap();

        assert!(response.tool_calls.is_some());
        let tc = response.tool_calls.unwrap();
        assert_eq!(tc[0].function.name, "data_crud");
        mock.assert();
    }
}
