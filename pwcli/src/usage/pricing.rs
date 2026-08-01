use std::collections::HashMap;

/// 模型定价（每 1K tokens 的美元价格）
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input_per_1k: f64,
    pub output_per_1k: f64,
}

impl ModelPricing {
    pub fn new(input: f64, output: f64) -> Self {
        Self {
            input_per_1k: input,
            output_per_1k: output,
        }
    }

    pub fn estimate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        (input_tokens as f64 / 1000.0) * self.input_per_1k
            + (output_tokens as f64 / 1000.0) * self.output_per_1k
    }
}

/// 获取模型定价
pub fn pricing_for_model(model: &str) -> ModelPricing {
    let mut known = HashMap::new();
    known.insert("gpt-4", ModelPricing::new(0.03, 0.06));
    known.insert("gpt-4o", ModelPricing::new(0.005, 0.015));
    known.insert("claude-sonnet-4-6", ModelPricing::new(0.003, 0.015));
    known.insert("claude-opus-4-6", ModelPricing::new(0.015, 0.075));

    *known.get(model).unwrap_or(&ModelPricing::new(0.01, 0.03))
}

/// 获取模型上下文窗口大小（tokens）
///
/// 启发式按模型名子串匹配；未知模型回退到 32k（保守值，避免误判触发自动压缩）。
/// 数据基于公开文档（2026-05），如有偏差不影响功能正确性，只影响触发时机。
pub fn context_window_for_model(model: &str) -> u32 {
    if let Some(window) = metadata_context_window(model) {
        return window;
    }
    let m = model.to_lowercase();
    if m.contains("gpt-5") {
        200_000
    } else if m.contains("gpt-4o") || m.contains("gpt-4-turbo") || m.contains("gpt-4.1") {
        128_000
    } else if m.contains("gpt-4") {
        32_000
    } else if m.contains("gpt-3.5") {
        16_000
    } else if m.contains("opus-4")
        || m.contains("sonnet-4")
        || m.contains("haiku-4")
        || m.contains("claude-3-5")
        || m.contains("claude-3.5")
    {
        200_000
    } else if m.contains("claude") {
        100_000
    } else if m.contains("gemini-1.5") || m.contains("gemini-2") || m.contains("gemini-3") {
        1_000_000
    } else if m.contains("gemini") {
        32_000
    } else if m.contains("qwen") || m.contains("kimi") {
        128_000
    } else {
        32_000
    }
}

fn metadata_context_window(model: &str) -> Option<u32> {
    let wanted = normalize_model_key(model);
    include_str!("../../resources/model_metadata.csv")
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split(',');
            let name = columns.next()?;
            let window = columns.next()?.parse::<u32>().ok()?;
            let key = normalize_model_key(name);
            (wanted == key || wanted.ends_with(&key)).then_some(window)
        })
        .next()
}

fn normalize_model_key(model: &str) -> String {
    model
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_cost() {
        let pricing = ModelPricing::new(0.03, 0.06);
        let cost = pricing.estimate_cost(1000, 500);
        assert!((cost - 0.06).abs() < 0.001, "Expected ~0.06, got {}", cost);
    }

    #[test]
    fn test_pricing_for_known_model() {
        let p = pricing_for_model("gpt-4o");
        assert_eq!(p.input_per_1k, 0.005);
    }

    #[test]
    fn test_pricing_fallback() {
        let p = pricing_for_model("unknown-model");
        assert_eq!(p.input_per_1k, 0.01);
    }

    #[test]
    fn test_context_window_known_models() {
        assert_eq!(context_window_for_model("gpt-4o"), 128_000);
        assert_eq!(context_window_for_model("gpt-4o-mini"), 128_000);
        assert_eq!(context_window_for_model("gpt-5.4-0305-global"), 1_050_000);
        assert_eq!(context_window_for_model("claude-sonnet-4-6"), 200_000);
        assert_eq!(context_window_for_model("claude-opus-4-7"), 200_000);
        assert_eq!(context_window_for_model("claude-opus-4-6"), 1_000_000);
        assert_eq!(context_window_for_model("bailian/deepseek-v4-pro"), 204_800);
        assert_eq!(context_window_for_model("gemini-2.5-pro"), 1_000_000);
        assert_eq!(context_window_for_model("qwen-max"), 128_000);
        assert_eq!(context_window_for_model("k3"), 1_048_576);
        assert_eq!(context_window_for_model("Peach-07-17-DogFooding"), 990_998);
    }

    #[test]
    fn test_context_window_fallback() {
        // 未识别模型用保守值，避免过早触发自动压缩
        assert_eq!(context_window_for_model("some-future-model"), 32_000);
    }
}
