use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};

use crate::config::ProviderConfig;
use crate::llm::anthropic::AnthropicClient;
use crate::llm::models::{AiResponse, LlmRequest, ProviderProtocol, StreamEvent};

use super::LlmAdapter;

pub struct AnthropicMessagesAdapter {
    provider: ProviderConfig,
    backend_url: String,
}

impl AnthropicMessagesAdapter {
    pub fn new(provider: ProviderConfig, backend_url: String) -> Self {
        Self {
            provider,
            backend_url,
        }
    }

    fn client(&self) -> AnthropicClient {
        AnthropicClient::new(self.provider.clone(), self.backend_url.clone())
    }
}

#[async_trait]
impl LlmAdapter for AnthropicMessagesAdapter {
    fn protocol(&self) -> ProviderProtocol {
        ProviderProtocol::AnthropicMessages
    }

    async fn chat(&self, request: &LlmRequest) -> Result<AiResponse> {
        self.client().chat(request).await
    }

    fn chat_stream(&self, request: LlmRequest) -> BoxStream<'static, StreamEvent> {
        let client = self.client();
        Box::pin(async_stream::stream! {
            let mut nested = client.chat_stream(&request);
            while let Some(event) = nested.next().await {
                yield event;
            }
        })
    }
}
