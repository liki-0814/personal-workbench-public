use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::config::ProviderConfig;
use crate::llm::models::{AiResponse, LlmRequest, ProviderProtocol, StreamEvent};

use super::anthropic_messages::AnthropicMessagesAdapter;
use super::google_generative::GoogleGenerativeAdapter;
use super::openai_chat::OpenAiChatAdapter;
use super::openai_codex_responses::OpenAiCodexResponsesAdapter;
use super::openai_responses::OpenAiResponsesAdapter;

#[async_trait]
pub trait LlmAdapter: Send + Sync {
    fn protocol(&self) -> ProviderProtocol;
    async fn chat(&self, request: &LlmRequest) -> Result<AiResponse>;
    fn chat_stream(&self, request: LlmRequest) -> BoxStream<'static, StreamEvent>;
}

pub fn create_adapter(
    provider: ProviderConfig,
    backend_url: String,
    session_id: Option<String>,
) -> Result<Box<dyn LlmAdapter>> {
    let protocol = ProviderProtocol::parse(&provider.protocol).map_err(|error| {
        anyhow::anyhow!(
            "provider '{}' declares unsupported protocol '{}': {error}",
            provider.name,
            provider.protocol
        )
    })?;

    Ok(match protocol {
        ProviderProtocol::OpenAiChat => {
            Box::new(OpenAiChatAdapter::new(provider, backend_url, session_id))
        }
        ProviderProtocol::AnthropicMessages => {
            Box::new(AnthropicMessagesAdapter::new(provider, backend_url))
        }
        ProviderProtocol::OpenAiResponses => Box::new(OpenAiResponsesAdapter::new(
            provider,
            backend_url,
            session_id,
        )),
        ProviderProtocol::OpenAiCodexResponses => {
            Box::new(OpenAiCodexResponsesAdapter::new(provider, session_id))
        }
        ProviderProtocol::GoogleGenerative => {
            Box::new(GoogleGenerativeAdapter::new(provider, backend_url))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(protocol: &str) -> ProviderConfig {
        ProviderConfig {
            name: "test".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "sk".into(),
            protocol: protocol.into(),
            model: "model".into(),
            models: Vec::new(),
            use_proxy: None,
            compat_profile: None,
        }
    }

    #[test]
    fn creates_adapters_for_supported_protocols() {
        for protocol in [
            "openai",
            "openai_chat",
            "anthropic",
            "anthropic_messages",
            "openai_responses",
            "openai_codex_responses",
            "google_generative",
        ] {
            create_adapter(provider(protocol), "http://127.0.0.1:9".into(), None).unwrap();
        }
    }

    #[test]
    fn rejects_unknown_protocol() {
        match create_adapter(provider("bedrock"), "http://127.0.0.1:9".into(), None) {
            Ok(_) => panic!("expected unsupported protocol error"),
            Err(error) => {
                let message = error.to_string();
                assert!(message.contains("unsupported protocol"), "{message}");
            }
        }
    }
}
