// 摘要/压缩共享 helper：session compact + memory compactor 都走这里
use crate::ai::llm::{ChatMessage, LlmClient};
use crate::ai::usage::UsageTracker;

#[derive(Debug)]
pub enum SummarizeError {
    LlmError(String),
    EmptyResponse,
}

impl std::fmt::Display for SummarizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SummarizeError::LlmError(e) => write!(f, "LLM 摘要失败: {}", e),
            SummarizeError::EmptyResponse => write!(f, "AI 返回空摘要"),
        }
    }
}

impl std::error::Error for SummarizeError {}

// 摘要场景沿用 llm::default_max_tokens_for —— Opus 128K，其他全部 64K
// （Sonnet/Haiku/Gemini/Qwen 等输出上限均≤64K，超 API 直接 400）
fn clamp_max_tokens_for_model(model: &str) -> u32 {
    crate::ai::llm::default_max_tokens_for(model)
}

pub async fn summarize_via_llm(
    items: &str,
    system_prompt: &str,
    llm: &LlmClient,
    usage_tracker: &mut UsageTracker,
) -> Result<String, SummarizeError> {
    summarize_via_llm_limited(items, system_prompt, llm, usage_tracker, None).await
}

pub async fn summarize_via_llm_limited(
    items: &str,
    system_prompt: &str,
    llm: &LlmClient,
    usage_tracker: &mut UsageTracker,
    max_tokens: Option<u32>,
) -> Result<String, SummarizeError> {
    let model = llm.provider().model.clone();
    let max_tokens = max_tokens.or_else(|| Some(clamp_max_tokens_for_model(&model)));
    let messages = [ChatMessage {
        role: "user".to_string(),
        content: items.to_string(),
        images: Vec::new(),
        generated_images: Vec::new(),
        tool_calls: None,
        tool_call_id: None,
    }];
    let response = llm
        .chat_with_max_tokens(&messages, Some(system_prompt), max_tokens)
        .await
        .map_err(|e| SummarizeError::LlmError(e.to_string()))?;

    if let Some(u) = response.usage {
        usage_tracker.record_turn(u, &model);
    }
    let text = response.content.trim().to_string();
    if text.is_empty() {
        return Err(SummarizeError::EmptyResponse);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_known_models_use_static_table() {
        assert_eq!(clamp_max_tokens_for_model("gemini-3.5-flash"), 65536);
        assert_eq!(clamp_max_tokens_for_model("qwen3.7-max"), 65536);
    }

    #[test]
    fn clamp_unknown_non_opus_to_64k() {
        for m in [
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "Claude-Sonnet",
            "gpt-4o",
        ] {
            assert_eq!(clamp_max_tokens_for_model(m), 64_000, "{} should be 64K", m);
        }
    }

    #[test]
    fn clamp_opus_keeps_128k() {
        assert_eq!(clamp_max_tokens_for_model("claude-opus-4-5"), 128_000);
        assert_eq!(clamp_max_tokens_for_model("Claude-Opus-4-7"), 128_000);
    }
}
