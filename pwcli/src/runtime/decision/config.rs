use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoaModelRef {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoaPreset {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub advisors: Vec<MoaModelRef>,
    #[serde(default)]
    pub judge: MoaModelRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_describer: Option<MoaModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_risk_advisors: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevated_risk_advisors: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_risk_advisors: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoaConfig {
    #[serde(default)]
    pub active_preset: String,
    #[serde(default)]
    pub presets: BTreeMap<String, MoaPreset>,
}

impl MoaConfig {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if !self.active_preset.is_empty() && !self.presets.contains_key(&self.active_preset) {
            errors.push("activePreset 必须指向已存在的 preset".into());
        }
        for (name, preset) in &self.presets {
            let valid_name = !name.is_empty()
                && name.len() <= 64
                && name.chars().enumerate().all(|(index, value)| {
                    value.is_ascii_lowercase()
                        || value.is_ascii_digit()
                        || (index > 0 && matches!(value, '.' | '_' | '-'))
                });
            if !valid_name {
                errors.push(format!("无效 preset 名称: {name}"));
            }
            if preset.enabled && preset.advisors.is_empty() {
                errors.push(format!("{name}: 至少需要一个 advisor"));
            }
            if preset.enabled && preset.judge.model.is_empty() {
                errors.push(format!("{name}: judge 未配置"));
            }
            for temperature in [preset.advisor_temperature, preset.judge_temperature]
                .into_iter()
                .flatten()
            {
                if !(0.0..=2.0).contains(&temperature) {
                    errors.push(format!("{name}: temperature 必须在 0–2 之间"));
                }
            }
            for count in [
                preset.low_risk_advisors,
                preset.elevated_risk_advisors,
                preset.high_risk_advisors,
            ]
            .into_iter()
            .flatten()
            {
                if count == 0 || count > preset.advisors.len() {
                    errors.push(format!(
                        "{name}: 风险分层 advisor 数量必须在 1–{} 之间",
                        preset.advisors.len()
                    ));
                }
            }
        }
        errors
    }
}

fn default_true() -> bool {
    true
}

pub async fn load_moa_config(_backend: &crate::runtime::backend::BackendClient) -> MoaConfig {
    crate::runtime::settings::local_config::get()
        .ai
        .moa
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_schema_uses_advisors_and_judge() {
        let config: MoaConfig = serde_json::from_value(serde_json::json!({
            "activePreset": "review",
            "presets": {
                "review": {
                    "enabled": true,
                    "advisors": [{"provider": "p", "model": "a"}],
                    "judge": {"provider": "p", "model": "j"},
                    "advisorMaxTokens": 900
                }
            }
        }))
        .unwrap();
        assert!(config.validate().is_empty());
        assert_eq!(config.presets["review"].advisors[0].model, "a");
        assert_eq!(config.presets["review"].judge.model, "j");
        assert_eq!(config.presets["review"].advisor_max_tokens, Some(900));
    }

    #[test]
    fn active_preset_must_exist() {
        let config = MoaConfig {
            active_preset: "missing".into(),
            presets: BTreeMap::new(),
        };
        assert_eq!(config.validate().len(), 1);
    }
}
