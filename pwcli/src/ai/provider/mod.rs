//! Provider identity, built-in catalog, credentials, and authentication.
//!
//! Runtime composition asks this module for an authenticated `ProviderConfig`.
//! HTTP/UI code may manage provider records and login flows, but never receives
//! access tokens or refresh tokens.

mod auth;
mod credentials;
mod registry;

pub use auth::{
    antigravity_oauth_configured, extract_codex_account_id, AuthFlowMethod, AuthFlowSnapshot,
    AuthFlowState, AuthManager, AuthStatus, ResolvedAuth,
};
pub use credentials::{Credential, CredentialStore, OAuthCredential};
pub use registry::{ProviderRegistry, ProviderService};

use crate::ai::config::ProviderConfig;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    KimiCoding,
    Xai,
    #[serde(rename = "openai-codex", alias = "open-ai-codex")]
    OpenAiCodex,
    GoogleAntigravity,
    QwenTokenPlanCn,
    #[default]
    Custom,
}

#[cfg(test)]
mod tests {
    use super::ProviderKind;

    #[test]
    fn codex_kind_uses_stable_wire_name_and_accepts_legacy_name() {
        assert_eq!(
            serde_json::to_string(&ProviderKind::OpenAiCodex).unwrap(),
            "\"openai-codex\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderKind>("\"open-ai-codex\"").unwrap(),
            ProviderKind::OpenAiCodex
        );
    }
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KimiCoding => "kimi-coding",
            Self::Xai => "xai",
            Self::OpenAiCodex => "openai-codex",
            Self::GoogleAntigravity => "google-antigravity",
            Self::QwenTokenPlanCn => "qwen-token-plan-cn",
            Self::Custom => "custom",
        }
    }

    pub fn is_builtin(self) -> bool {
        self != Self::Custom
    }
}

pub fn provider_kind(provider: &ProviderConfig) -> ProviderKind {
    let marker = provider
        .compat_profile
        .as_deref()
        .into_iter()
        .flat_map(|value| value.split(';'))
        .find(|value| value.starts_with("builtin:"));
    match marker {
        Some("builtin:kimi-coding") => ProviderKind::KimiCoding,
        Some("builtin:xai") => ProviderKind::Xai,
        Some("builtin:openai-codex") => ProviderKind::OpenAiCodex,
        Some("builtin:google-antigravity") => ProviderKind::GoogleAntigravity,
        Some("builtin:qwen-token-plan-cn") => ProviderKind::QwenTokenPlanCn,
        _ => ProviderKind::Custom,
    }
}

pub fn provider_id(provider: &ProviderConfig) -> &str {
    provider
        .compat_profile
        .as_deref()
        .into_iter()
        .flat_map(|value| value.split(';'))
        .find_map(|value| value.strip_prefix("credential:"))
        .filter(|value| !value.is_empty())
        .unwrap_or(provider.name.as_str())
}
