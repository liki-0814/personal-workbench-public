use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};

use crate::config::ProviderConfig;
use crate::llm::models::{AiResponse, LlmRequest, ProviderProtocol, StreamEvent};
use crate::llm::openai::OpenAiClient;

use super::LlmAdapter;

pub struct OpenAiChatAdapter {
    provider: ProviderConfig,
    backend_url: String,
    session_id: Option<String>,
}

impl OpenAiChatAdapter {
    pub fn new(provider: ProviderConfig, backend_url: String, session_id: Option<String>) -> Self {
        Self {
            provider,
            backend_url,
            session_id,
        }
    }

    fn client(&self) -> OpenAiClient {
        OpenAiClient::new(self.provider.clone(), self.backend_url.clone())
            .with_session_id(self.session_id.clone())
    }
}

#[async_trait]
impl LlmAdapter for OpenAiChatAdapter {
    fn protocol(&self) -> ProviderProtocol {
        ProviderProtocol::OpenAiChat
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
