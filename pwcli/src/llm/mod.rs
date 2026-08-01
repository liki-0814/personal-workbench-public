pub mod anthropic;
pub mod client;
pub mod deferred_tools;
pub mod image_marker;
pub mod model_context;
pub mod models;
pub mod openai;
pub mod retry;
pub mod streaming;
pub mod summarize;

pub use client::{LlmClient, LlmStreamOptions};
pub use models::*;
pub use summarize::{summarize_via_llm, summarize_via_llm_limited, SummarizeError};

/// 已知模型 max_output 静态表，与前端 src/core/config/modelMetadata.ts 同源
/// （pwcli/resources/model_metadata.csv 是真源 CSV）。新增模型按需补这张表。
///
/// key 已归一化：小写 + 剥离非字母数字字符。
const KNOWN_MAX_OUTPUT: &[(&str, u32)] = &[
    ("qwen37maxdogfooding", 65536),
    ("qwen36plusdogfooding", 65536),
    ("qwen37max", 65536),
    ("qwen36maxpreview", 65536),
    ("gemini35flash", 65536),
    ("gemini31flashimagepreview", 65536),
    ("bailiandeepseekv4pro", 16384),
    ("gpt5403050global", 128_000),
    ("gpt540305global", 128_000),
    ("bailianglm51", 128_000),
    ("bailianglm52", 131_072),
    ("qwenimage20pro", 1_000_000),
    ("claudeopus46", 128_000),
    ("k3", 128_000),
];

fn normalize_model_key(model: &str) -> String {
    model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 按 model_id 选安全的默认 max_tokens。
///
/// 三级优先级：
/// 1. 已知模型静态表（KNOWN_MAX_OUTPUT，归一化匹配，覆盖名字变体）
/// 2. opus 走 128K（Anthropic 官方上限）
/// 3. 其他主流模型一刀切 64K（实测对 Sonnet/Haiku/Gemini/Qwen/GPT-5/Kimi 都安全）
///
/// 调用 LLM 时如果用户没显式传 max_tokens 且 ProviderConfig.models[].max_output 也没配，
/// 所有协议（anthropic/openai）都用本函数兜底。
pub fn default_max_tokens_for(model: &str) -> u32 {
    let normalized = normalize_model_key(model);
    for (key, val) in KNOWN_MAX_OUTPUT {
        if &normalized == key || normalized.ends_with(key) {
            return *val;
        }
    }
    if model.to_lowercase().contains("opus") {
        128_000
    } else {
        64_000
    }
}

#[cfg(test)]
mod tests {
    use super::default_max_tokens_for;

    #[test]
    fn opus_gets_128k() {
        assert_eq!(default_max_tokens_for("claude-opus-4-7"), 128_000);
        assert_eq!(default_max_tokens_for("Claude-Opus-4-8"), 128_000);
        assert_eq!(default_max_tokens_for("anthropic/claude-opus"), 128_000);
    }

    #[test]
    fn known_models_use_static_table() {
        assert_eq!(default_max_tokens_for("gemini-3.5-flash"), 65536);
        assert_eq!(default_max_tokens_for("qwen3.7-max"), 65536);
        assert_eq!(default_max_tokens_for("Qwen3.6-Plus-DogFooding"), 65536);
        assert_eq!(default_max_tokens_for("bailian/deepseek-v4-pro"), 16384);
        assert_eq!(default_max_tokens_for("gpt-5.4-0305-global"), 128_000);
        assert_eq!(default_max_tokens_for("bailian/glm-5.1"), 128_000);
        assert_eq!(default_max_tokens_for("bailian/glm-5.2"), 131_072);
        assert_eq!(default_max_tokens_for("k3"), 128_000);
        assert_eq!(default_max_tokens_for("kimi-k3"), 128_000);
    }

    #[test]
    fn unknown_non_opus_falls_back_to_64k() {
        for m in [
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "gemini-2.5-pro",
            "gpt-5",
            "gpt-4o",
            "kimi-k2",
        ] {
            assert_eq!(
                default_max_tokens_for(m),
                64_000,
                "model {} should fall back to 64K",
                m
            );
        }
    }
}
