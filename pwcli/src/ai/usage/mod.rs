pub mod pricing;

pub use pricing::*;

use crate::ai::llm::TokenUsage;

/// 用量追踪器
#[derive(Debug, Clone, Default)]
pub struct UsageTracker {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_cost_usd: f64,
    pub turn_count: u32,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_turn(&mut self, usage: TokenUsage, model: &str) {
        self.total_prompt_tokens += usage.prompt_tokens as u64;
        self.total_completion_tokens += usage.completion_tokens as u64;
        self.turn_count += 1;

        let pricing = pricing_for_model(model);
        let cost = pricing.estimate_cost(usage.prompt_tokens, usage.completion_tokens);
        self.total_cost_usd += cost;
    }

    pub fn total_tokens(&self) -> u64 {
        self.total_prompt_tokens + self.total_completion_tokens
    }

    pub fn summary(&self) -> String {
        format!(
            "Turns: {} | Tokens: {} prompt + {} completion = {} total | Cost: ${:.4}",
            self.turn_count,
            self.total_prompt_tokens,
            self.total_completion_tokens,
            self.total_tokens(),
            self.total_cost_usd
        )
    }
}

/// 单次对话快照
#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cost_usd: f64,
    pub model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_tracker() {
        let mut tracker = UsageTracker::new();
        tracker.record_turn(
            TokenUsage {
                prompt_tokens: 1000,
                completion_tokens: 500,
                total_tokens: 1500,
            },
            "gpt-4o",
        );
        assert_eq!(tracker.total_prompt_tokens, 1000);
        assert_eq!(tracker.total_completion_tokens, 500);
        assert!(tracker.total_cost_usd > 0.0);
        assert!(tracker.summary().contains("Turns: 1"));
    }
}
