use anyhow::Result;
use async_trait::async_trait;

use crate::ai::llm::{ChatMessage, LlmClient};

#[async_trait]
pub trait IllustrationModelPort: Send + Sync {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        system_prompt: &str,
        max_tokens: u32,
        temperature: Option<f32>,
    ) -> Result<String>;
    async fn complete_vision(
        &self,
        messages: &[ChatMessage],
        system_prompt: &str,
        max_tokens: u32,
    ) -> Result<Option<String>>;
    fn model_id(&self) -> &str;
    fn supports_vision(&self) -> bool;
}

pub struct LlmIllustrationModel {
    client: std::sync::Arc<LlmClient>,
    model_id: String,
    supports_vision: bool,
    vision_client: Option<std::sync::Arc<LlmClient>>,
}

impl LlmIllustrationModel {
    pub fn new(
        client: std::sync::Arc<LlmClient>,
        supports_vision: bool,
        vision_client: Option<std::sync::Arc<LlmClient>>,
    ) -> Self {
        let model_id = client.provider().model.clone();
        Self {
            client,
            model_id,
            supports_vision,
            vision_client,
        }
    }
}

#[async_trait]
impl IllustrationModelPort for LlmIllustrationModel {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        system_prompt: &str,
        max_tokens: u32,
        temperature: Option<f32>,
    ) -> Result<String> {
        Ok(self
            .client
            .chat_with_sampling(messages, Some(system_prompt), Some(max_tokens), temperature)
            .await?
            .content)
    }

    async fn complete_vision(
        &self,
        messages: &[ChatMessage],
        system_prompt: &str,
        max_tokens: u32,
    ) -> Result<Option<String>> {
        let client = if self.supports_vision {
            &self.client
        } else if let Some(client) = self.vision_client.as_ref() {
            client
        } else {
            return Ok(None);
        };
        Ok(Some(
            client
                .chat_with_sampling(messages, Some(system_prompt), Some(max_tokens), None)
                .await?
                .content,
        ))
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn supports_vision(&self) -> bool {
        self.supports_vision || self.vision_client.is_some()
    }
}
