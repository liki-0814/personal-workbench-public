use super::models::*;
use crate::config::RuntimeConfig;
use anyhow::{Context, Result};
use futures::stream::BoxStream;

const FIRST_STREAM_EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Debug, Clone, Copy, Default)]
pub struct LlmStreamOptions {
    pub thinking: bool,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// 兼容层：LlmClient 内部委托到 OpenAiClient / AnthropicClient
///
/// 保留此结构体以兼容现有调用方（main.rs、commands.rs）。
/// 新代码应直接使用 `crate::llm::openai::OpenAiClient` 或
/// `crate::llm::anthropic::AnthropicClient`。
///
/// `chat()` 自动按 (primary, ...fallbacks) 顺序重试：第一个成功的 provider
/// 返回结果。`chat_stream()` 仅在当前 provider 尚未产生文本、思考或工具调用时
/// 安全切换 fallback，避免重复已经展示给用户的内容。
#[derive(Clone)]
pub struct LlmClient {
    provider: ProviderConfig,
    /// 候选 fallback providers（按顺序尝试）。空表示无 fallback。
    fallbacks: Vec<ProviderConfig>,
    backend_url: String,
    session_id: Option<String>,
}

impl LlmClient {
    pub fn from_config(config: &RuntimeConfig) -> Result<Self> {
        let provider = config
            .active_provider()
            .context("No AI provider configured. Run: pwcli config")?
            .clone();
        // 其余 provider 按配置顺序作为 fallback
        let fallbacks: Vec<ProviderConfig> = config
            .providers
            .as_ref()
            .map(|all| {
                all.iter()
                    .filter(|p| p.name != provider.name)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            provider,
            fallbacks,
            backend_url: config.backend_url.clone(),
            session_id: None,
        })
    }

    /// 用一个独立的 ProviderConfig 构造客户端（用于 per-request 模型 / provider 切换）
    pub fn with_provider(provider: ProviderConfig, backend_url: String) -> Self {
        Self {
            provider,
            fallbacks: Vec::new(),
            backend_url,
            session_id: None,
        }
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// 当前生效的 primary provider（含 model / base_url / api_key / protocol）
    pub fn provider(&self) -> &ProviderConfig {
        &self.provider
    }

    /// 单 provider 调用（不做 fallback）
    async fn chat_one(
        &self,
        provider: &ProviderConfig,
        request: &LlmRequest,
    ) -> Result<AiResponse> {
        if provider.protocol == "anthropic" {
            let client =
                super::anthropic::AnthropicClient::new(provider.clone(), self.backend_url.clone());
            client.chat(request).await
        } else {
            let client =
                super::openai::OpenAiClient::new(provider.clone(), self.backend_url.clone())
                    .with_session_id(self.session_id.clone());
            client.chat(request).await
        }
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        system_prompt: Option<&str>,
        tools: Option<&[ToolSchema]>,
        tool_choice: Option<&str>,
    ) -> Result<AiResponse> {
        self.chat_with_options(messages, system_prompt, tools, tool_choice, false)
            .await
    }

    // 摘要/压缩等场景用，按模型 clamp max_tokens 防止 sonnet/haiku 上限 64K 被超
    pub async fn chat_with_max_tokens(
        &self,
        messages: &[ChatMessage],
        system_prompt: Option<&str>,
        max_tokens: Option<u32>,
    ) -> Result<AiResponse> {
        self.chat_with_sampling(messages, system_prompt, max_tokens, None)
            .await
    }

    pub async fn chat_with_sampling(
        &self,
        messages: &[ChatMessage],
        system_prompt: Option<&str>,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
    ) -> Result<AiResponse> {
        let request = LlmRequest {
            messages: messages.to_vec(),
            system_prompt: system_prompt.map(|s| s.to_string()),
            tools: None,
            stream: false,
            tool_choice: None,
            thinking: false,
            max_tokens,
            temperature,
        };
        let mut last_err: Option<anyhow::Error> = None;
        let candidates = std::iter::once(&self.provider).chain(self.fallbacks.iter());
        for (idx, p) in candidates.enumerate() {
            match self.chat_one(p, &request).await {
                Ok(r) => {
                    if idx > 0 {
                        eprintln!(
                            "ℹ️  primary provider failed; fell back to '{}' (model {})",
                            p.name, p.model
                        );
                    }
                    return Ok(r);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no providers configured")))
    }

    /// chat() with extended-thinking toggle. Most callers use chat(); the agent
    /// service path goes through this to forward the user's per-request flag.
    pub async fn chat_with_options(
        &self,
        messages: &[ChatMessage],
        system_prompt: Option<&str>,
        tools: Option<&[ToolSchema]>,
        tool_choice: Option<&str>,
        thinking: bool,
    ) -> Result<AiResponse> {
        self.chat_with_options_and_temperature(
            messages,
            system_prompt,
            tools,
            tool_choice,
            thinking,
            None,
        )
        .await
    }

    pub async fn chat_with_options_and_temperature(
        &self,
        messages: &[ChatMessage],
        system_prompt: Option<&str>,
        tools: Option<&[ToolSchema]>,
        tool_choice: Option<&str>,
        thinking: bool,
        temperature: Option<f32>,
    ) -> Result<AiResponse> {
        let request = LlmRequest {
            messages: messages.to_vec(),
            system_prompt: system_prompt.map(|s| s.to_string()),
            tools: tools.map(|t| t.to_vec()),
            stream: false,
            tool_choice: tool_choice.map(|s| s.to_string()),
            thinking,
            max_tokens: None,
            temperature,
        };

        // Try primary first, then each fallback in order. First success wins.
        let mut last_err: Option<anyhow::Error> = None;
        let candidates = std::iter::once(&self.provider).chain(self.fallbacks.iter());
        for (idx, p) in candidates.enumerate() {
            match self.chat_one(p, &request).await {
                Ok(r) => {
                    if idx > 0 {
                        eprintln!(
                            "ℹ️  primary provider failed; fell back to '{}' (model {})",
                            p.name, p.model
                        );
                    }
                    return Ok(r);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no providers configured")))
    }

    /// 流式对话：返回 StreamEvent 的 Stream，调用方需驱动到 Done/Error 为止
    pub fn chat_stream(
        &self,
        messages: &[ChatMessage],
        system_prompt: Option<&str>,
        tools: Option<&[ToolSchema]>,
        tool_choice: Option<&str>,
    ) -> BoxStream<'_, StreamEvent> {
        self.chat_stream_with_options(
            messages,
            system_prompt,
            tools,
            tool_choice,
            LlmStreamOptions::default(),
        )
    }

    pub fn chat_stream_with_options(
        &self,
        messages: &[ChatMessage],
        system_prompt: Option<&str>,
        tools: Option<&[ToolSchema]>,
        tool_choice: Option<&str>,
        options: LlmStreamOptions,
    ) -> BoxStream<'_, StreamEvent> {
        let request = LlmRequest {
            messages: messages.to_vec(),
            system_prompt: system_prompt.map(|s| s.to_string()),
            tools: tools.map(|t| t.to_vec()),
            stream: true,
            tool_choice: tool_choice.map(|s| s.to_string()),
            thinking: options.thinking,
            max_tokens: options.max_tokens,
            temperature: options.temperature,
        };

        let providers = std::iter::once(self.provider.clone())
            .chain(self.fallbacks.clone())
            .collect::<Vec<_>>();
        let backend_url = self.backend_url.clone();
        let session_id = self.session_id.clone();
        let stream = async_stream::stream! {
            use futures::StreamExt;

            for (index, provider) in providers.iter().enumerate() {
                let selected_provider = provider.clone();
                let selected_request = request.clone();
                let selected_backend_url = backend_url.clone();
                let selected_session_id = session_id.clone();
                let mut inner: BoxStream<'static, StreamEvent> = if selected_provider.protocol == "anthropic" {
                    Box::pin(async_stream::stream! {
                        let client = super::anthropic::AnthropicClient::new(
                            selected_provider,
                            selected_backend_url,
                        );
                        let mut nested = client.chat_stream(&selected_request);
                        while let Some(event) = nested.next().await {
                            yield event;
                        }
                    })
                } else {
                    Box::pin(async_stream::stream! {
                        let client = super::openai::OpenAiClient::new(
                            selected_provider,
                            selected_backend_url,
                        )
                        .with_session_id(selected_session_id);
                        let mut nested = client.chat_stream(&selected_request);
                        while let Some(event) = nested.next().await {
                            yield event;
                        }
                    })
                };
                let has_fallback = index + 1 < providers.len();
                let mut emitted_output = false;
                let mut try_fallback = false;

                loop {
                    let next = if emitted_output {
                        Ok(inner.next().await)
                    } else {
                        tokio::time::timeout(FIRST_STREAM_EVENT_TIMEOUT, inner.next()).await
                    };
                    let event = match next {
                        Ok(Some(event)) => event,
                        Ok(None) => break,
                        Err(_) if has_fallback => {
                            let fallback = &providers[index + 1];
                            yield StreamEvent::StreamReset {
                                reason: format!(
                                    "provider '{}' produced no stream event within {}s; safely falling back to '{}' ({})",
                                    provider.name,
                                    FIRST_STREAM_EVENT_TIMEOUT.as_secs(),
                                    fallback.name,
                                    fallback.model
                                ),
                            };
                            try_fallback = true;
                            break;
                        }
                        Err(_) => {
                            yield StreamEvent::Error(format!(
                                "provider '{}' produced no stream event within {}s",
                                provider.name,
                                FIRST_STREAM_EVENT_TIMEOUT.as_secs()
                            ));
                            break;
                        }
                    };
                    // Some OpenAI-compatible gateways return an upstream SSE error envelope as
                    // ordinary `content` instead of an HTTP/SSE error. Treat only unmistakable
                    // pre-output envelopes as provider failures so they can use the same safe
                    // fallback path without hiding legitimate user-facing text.
                    let event = match event {
                        StreamEvent::TextDelta(text)
                            if !emitted_output && is_embedded_provider_error(&text) =>
                        {
                            StreamEvent::Error(text)
                        }
                        event => event,
                    };
                    match &event {
                        StreamEvent::TextDelta(_)
                        | StreamEvent::ThinkingDelta(_)
                        | StreamEvent::ToolCallStart { .. }
                        | StreamEvent::ToolCallDelta { .. }
                        | StreamEvent::ToolCallEnd { .. } => emitted_output = true,
                        StreamEvent::Error(message)
                            if has_fallback
                                && !emitted_output
                                && crate::llm::retry::is_retriable_stream_error(message) =>
                        {
                            let fallback = &providers[index + 1];
                            yield StreamEvent::StreamReset {
                                reason: format!(
                                    "provider '{}' failed before output; falling back to '{}' ({})",
                                    provider.name, fallback.name, fallback.model
                                ),
                            };
                            try_fallback = true;
                            break;
                        }
                        _ => {}
                    }
                    yield event;
                }

                if !try_fallback {
                    return;
                }
            }
        };
        Box::pin(stream)
    }
}

fn is_embedded_provider_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("event:error") || lower.contains("http_status/"))
        && (lower.contains("throttl")
            || lower.contains("rate_limit")
            || lower.contains("allocationquota")
            || lower.contains("status/429"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use mockito::Server;

    #[tokio::test]
    async fn test_chat_openai() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                "choices": [{"message": {"content": "Hello!", "role": "assistant"}}]
            }"#,
            )
            .create();

        let client = LlmClient {
            provider: ProviderConfig {
                name: "test".to_string(),
                base_url: server.url(),
                api_key: "sk-test".to_string(),
                protocol: "openai".to_string(),
                model: "gpt-4".to_string(),
                models: Vec::new(),
            },
            backend_url: server.url(),
            fallbacks: Vec::new(),
            session_id: None,
        };

        let response = client
            .chat(
                &[ChatMessage {
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    role: "user".to_string(),
                    content: "Hi".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                }],
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.content, "Hello!");
        mock.assert();
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
                ]
            }"#,
            )
            .create();

        let client = LlmClient {
            provider: ProviderConfig {
                name: "test".to_string(),
                base_url: server.url(),
                api_key: "sk-test".to_string(),
                protocol: "anthropic".to_string(),
                model: "claude-sonnet".to_string(),
                models: Vec::new(),
            },
            backend_url: server.url(),
            fallbacks: Vec::new(),
            session_id: None,
        };

        let response = client
            .chat(
                &[ChatMessage {
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    role: "user".to_string(),
                    content: "Hi".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                }],
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.content, "Hello from Claude!");
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
                ]
            }"#)
            .create();

        let client = LlmClient {
            provider: ProviderConfig {
                name: "test".to_string(),
                base_url: server.url(),
                api_key: "sk-test".to_string(),
                protocol: "anthropic".to_string(),
                model: "claude-sonnet".to_string(),
                models: Vec::new(),
            },
            backend_url: server.url(),
            fallbacks: Vec::new(),
            session_id: None,
        };

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
            .chat(
                &[ChatMessage {
                    images: Vec::new(),
                    generated_images: Vec::new(),
                    role: "user".to_string(),
                    content: "列出待办任务".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                }],
                None,
                Some(&tools),
                None,
            )
            .await
            .unwrap();

        assert!(response.tool_calls.is_some());
        let tc = response.tool_calls.unwrap();
        assert_eq!(tc[0].function.name, "data_crud");
        mock.assert();
    }

    #[tokio::test]
    async fn stream_falls_back_after_rate_limit_before_output() {
        let mut primary_server = Server::new_async().await;
        let primary = primary_server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"error\":{\"message\":\"rate_limit\"}}\n\n")
            .create();
        let mut fallback_server = Server::new_async().await;
        let fallback = fallback_server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"fallback ok\"},\"finish_reason\":null}]}\n\n",
                "data: [DONE]\n\n"
            ))
            .create();
        let provider = |name: &str, base_url: String| ProviderConfig {
            name: name.into(),
            base_url,
            api_key: "sk-test".into(),
            protocol: "openai".into(),
            model: name.into(),
            models: Vec::new(),
        };
        let client = LlmClient {
            provider: provider("peach", primary_server.url()),
            fallbacks: vec![provider("claude", fallback_server.url())],
            backend_url: "http://localhost".into(),
            session_id: None,
        };
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: "test".into(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let events = client
            .chat_stream(&messages, None, None, None)
            .collect::<Vec<_>>()
            .await;

        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::StreamReset { reason } if reason.contains("claude")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::TextDelta(text) if text == "fallback ok"
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event, StreamEvent::Error(_))));
        primary.assert();
        fallback.assert();
    }

    #[tokio::test]
    async fn stream_falls_back_when_gateway_wraps_upstream_sse_error_as_text() {
        let mut primary_server = Server::new_async().await;
        let wrapped_error =
            "id:1\nevent:error\n:HTTP_STATUS/429\ndata:{\"code\":\"Throttling.AllocationQuota\"}";
        let primary = primary_server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":{}}},\"finish_reason\":null}}]}}\n\n",
                serde_json::to_string(wrapped_error).unwrap()
            ))
            .create();
        let mut fallback_server = Server::new_async().await;
        let fallback = fallback_server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(concat!(
                "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"},\"index\":0}\n\n",
                "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"claude ok\"},\"index\":0}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            ))
            .create();
        let provider = |name: &str, protocol: &str, base_url: String| ProviderConfig {
            name: name.into(),
            base_url,
            api_key: "sk-test".into(),
            protocol: protocol.into(),
            model: name.into(),
            models: Vec::new(),
        };
        let client = LlmClient {
            provider: provider("peach", "openai", primary_server.url()),
            fallbacks: vec![provider("claude", "anthropic", fallback_server.url())],
            backend_url: "http://localhost".into(),
            session_id: None,
        };
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: "test".into(),
            images: Vec::new(),
            generated_images: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let events = client
            .chat_stream(&messages, None, None, None)
            .collect::<Vec<_>>()
            .await;

        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::StreamReset { reason } if reason.contains("claude")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::TextDelta(text) if text == "claude ok"
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event, StreamEvent::Error(_))));
        primary.assert();
        fallback.assert();
    }
}
