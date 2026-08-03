// OAuth flows adapted from the MIT-licensed pi project:
// https://github.com/badlogic/pi-mono

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

use crate::config::ProviderConfig;

use super::{
    provider_id, provider_kind, Credential, CredentialStore, OAuthCredential, ProviderCatalog,
    ProviderKind,
};

const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const KIMI_AUTH_BASE: &str = "https://auth.kimi.com";
const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTH_BASE: &str = "https://auth.openai.com";
const OPENAI_CODEX_REDIRECT: &str = "http://localhost:1455/auth/callback";
const OPENAI_CODEX_DEVICE_REDIRECT: &str = "https://auth.openai.com/deviceauth/callback";
const ANTIGRAVITY_REDIRECT: &str = "http://127.0.0.1:51121/callback";
const ANTIGRAVITY_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";
const REFRESH_SKEW_SECONDS: i64 = 5 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthFlowMethod {
    DeviceCode,
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthFlowState {
    Pending,
    Connected,
    Failed,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthFlowSnapshot {
    pub flow_id: String,
    pub provider_id: String,
    pub method: AuthFlowMethod,
    pub state: AuthFlowState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub method: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
}

#[derive(Clone)]
pub struct ResolvedAuth {
    pub api_key: String,
    pub source: &'static str,
    pub account_id: Option<String>,
    pub project_id: Option<String>,
}

enum FlowSecret {
    Device {
        kind: ProviderKind,
        device_code: String,
        interval: Duration,
        expires_at: chrono::DateTime<chrono::Utc>,
    },
    Browser {
        kind: ProviderKind,
        verifier: String,
        state: String,
        redirect_uri: String,
    },
}

struct FlowEntry {
    snapshot: AuthFlowSnapshot,
    secret: FlowSecret,
}

#[derive(Clone)]
pub struct AuthManager {
    store: CredentialStore,
    http: Client,
    flows: Arc<RwLock<HashMap<String, FlowEntry>>>,
    refresh_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new(CredentialStore::standard())
    }
}

impl AuthManager {
    pub fn new(store: CredentialStore) -> Self {
        Self {
            store,
            http: crate::http_client::default_client(crate::http_client::ClientProfile::Llm),
            flows: Arc::new(RwLock::new(HashMap::new())),
            refresh_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn store(&self) -> &CredentialStore {
        &self.store
    }

    pub fn credential_reference(provider: &ProviderConfig) -> String {
        provider_id(provider).to_string()
    }

    pub async fn status(&self, provider: &ProviderConfig) -> Result<AuthStatus> {
        let reference = Self::credential_reference(provider);
        let credential = self.store.read(&reference)?;
        Ok(match credential {
            Some(Credential::ApiKey { .. }) => AuthStatus {
                method: "api_key".into(),
                status: "connected".into(),
                account_label: None,
            },
            Some(Credential::OAuth(value)) => AuthStatus {
                method: "oauth".into(),
                status: if value.expires_at <= chrono::Utc::now() {
                    "expired".into()
                } else {
                    "connected".into()
                },
                account_label: value.account_label,
            },
            None => AuthStatus {
                method: if matches!(
                    provider_kind(provider),
                    ProviderKind::QwenTokenPlanCn | ProviderKind::Custom
                ) {
                    "api_key".into()
                } else {
                    "oauth".into()
                },
                status: "disconnected".into(),
                account_label: None,
            },
        })
    }

    pub fn set_api_key(&self, provider: &ProviderConfig, key: String) -> Result<()> {
        if key.trim().is_empty() {
            anyhow::bail!("API key must not be empty");
        }
        self.store.write(
            &Self::credential_reference(provider),
            Credential::ApiKey { key },
        )
    }

    pub fn logout(&self, provider: &ProviderConfig) -> Result<()> {
        self.store.remove(&Self::credential_reference(provider))
    }

    pub async fn resolve(&self, provider: &ProviderConfig) -> Result<ResolvedAuth> {
        let reference = Self::credential_reference(provider);
        if let Some(credential) = self.store.read(&reference)? {
            return match credential {
                Credential::ApiKey { key } => Ok(ResolvedAuth {
                    api_key: key,
                    source: "credential_store",
                    account_id: None,
                    project_id: None,
                }),
                Credential::OAuth(value) => self.resolve_oauth(provider, &reference, value).await,
            };
        }
        if !provider.api_key.trim().is_empty() && !provider.api_key.contains("****") {
            return Ok(ResolvedAuth {
                api_key: provider.api_key.clone(),
                source: "legacy_config",
                account_id: None,
                project_id: None,
            });
        }
        if let Some(env_key) = ProviderCatalog
            .get(provider_kind(provider))
            .and_then(|builtin| builtin.env_key)
        {
            if let Ok(value) = std::env::var(env_key) {
                if !value.trim().is_empty() {
                    return Ok(ResolvedAuth {
                        api_key: value,
                        source: "environment",
                        account_id: None,
                        project_id: None,
                    });
                }
            }
        }
        anyhow::bail!("provider `{}` is not authenticated", provider.name)
    }

    pub async fn authenticated_provider(
        &self,
        provider: &ProviderConfig,
    ) -> Result<ProviderConfig> {
        let mut resolved = ProviderCatalog.materialize(provider);
        let auth = self.resolve(&resolved).await?;
        resolved.api_key = auth.api_key;
        if let Some(project_id) = auth.project_id {
            let marker = format!("project:{project_id}");
            let profile = resolved.compat_profile.get_or_insert_with(String::new);
            if !profile.is_empty() {
                profile.push(';');
            }
            profile.push_str(&marker);
        }
        Ok(resolved)
    }

    async fn resolve_oauth(
        &self,
        provider: &ProviderConfig,
        reference: &str,
        credential: OAuthCredential,
    ) -> Result<ResolvedAuth> {
        let refresh_at = credential.expires_at - chrono::Duration::seconds(REFRESH_SKEW_SECONDS);
        let credential = if refresh_at <= chrono::Utc::now() {
            let lock = {
                let mut locks = self.refresh_locks.lock().await;
                locks
                    .entry(reference.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone()
            };
            let _guard = lock.lock().await;
            match self.store.read(reference)? {
                Some(Credential::OAuth(current))
                    if current.expires_at - chrono::Duration::seconds(REFRESH_SKEW_SECONDS)
                        > chrono::Utc::now() =>
                {
                    current
                }
                Some(Credential::OAuth(current)) => {
                    let refreshed = self
                        .refresh(provider_kind(provider), &current)
                        .await
                        .with_context(|| {
                            format!("OAuth refresh failed for {}; login again", provider.name)
                        })?;
                    self.store
                        .write(reference, Credential::OAuth(refreshed.clone()))?;
                    refreshed
                }
                _ => anyhow::bail!("provider `{}` was logged out", provider.name),
            }
        } else {
            credential
        };
        Ok(ResolvedAuth {
            api_key: credential.access,
            source: "oauth",
            account_id: credential.account_id,
            project_id: credential.project_id,
        })
    }

    pub async fn start_flow(
        &self,
        provider: &ProviderConfig,
        method: AuthFlowMethod,
    ) -> Result<AuthFlowSnapshot> {
        match (provider_kind(provider), method) {
            (ProviderKind::KimiCoding | ProviderKind::Xai, AuthFlowMethod::DeviceCode)
            | (ProviderKind::OpenAiCodex, AuthFlowMethod::DeviceCode) => {
                self.start_device_flow(provider).await
            }
            (ProviderKind::OpenAiCodex, AuthFlowMethod::Browser) => {
                self.start_codex_browser_flow(provider).await
            }
            (ProviderKind::GoogleAntigravity, AuthFlowMethod::Browser) => {
                self.start_antigravity_browser_flow(provider).await
            }
            _ => anyhow::bail!("selected authentication method is not supported"),
        }
    }

    pub async fn flow(&self, flow_id: &str) -> Option<AuthFlowSnapshot> {
        self.flows
            .read()
            .await
            .get(flow_id)
            .map(|entry| entry.snapshot.clone())
    }

    pub async fn cancel_flow(&self, flow_id: &str) -> Result<AuthFlowSnapshot> {
        let mut flows = self.flows.write().await;
        let entry = flows
            .get_mut(flow_id)
            .context("authentication flow not found")?;
        entry.snapshot.state = AuthFlowState::Cancelled;
        Ok(entry.snapshot.clone())
    }

    pub async fn complete_flow(&self, flow_id: &str, input: &str) -> Result<AuthFlowSnapshot> {
        let (provider_id, kind, verifier, state, redirect_uri) = {
            let flows = self.flows.read().await;
            let entry = flows
                .get(flow_id)
                .context("authentication flow not found")?;
            let FlowSecret::Browser {
                kind,
                verifier,
                state,
                redirect_uri,
            } = &entry.secret
            else {
                anyhow::bail!("this authentication flow does not accept a manual code");
            };
            (
                entry.snapshot.provider_id.clone(),
                *kind,
                verifier.clone(),
                state.clone(),
                redirect_uri.clone(),
            )
        };
        let (code, received_state) = parse_authorization_input(input)?;
        if let Some(received_state) = received_state {
            if received_state != state {
                anyhow::bail!("OAuth state mismatch");
            }
        }
        let exchange = match kind {
            ProviderKind::OpenAiCodex => {
                self.exchange_codex_code(&code, &verifier, &redirect_uri)
                    .await
            }
            ProviderKind::GoogleAntigravity => {
                self.exchange_antigravity_code(&code, &verifier, &redirect_uri)
                    .await
            }
            _ => anyhow::bail!("provider does not support browser OAuth"),
        };
        match exchange {
            Ok(credential) => {
                self.store
                    .write(&provider_id, Credential::OAuth(credential))?;
                self.finish_flow(flow_id, AuthFlowState::Connected, None)
                    .await
            }
            Err(error) => {
                self.finish_flow(flow_id, AuthFlowState::Failed, Some(error.to_string()))
                    .await
            }
        }
    }

    async fn finish_flow(
        &self,
        flow_id: &str,
        state: AuthFlowState,
        error: Option<String>,
    ) -> Result<AuthFlowSnapshot> {
        let mut flows = self.flows.write().await;
        let entry = flows
            .get_mut(flow_id)
            .context("authentication flow not found")?;
        entry.snapshot.state = state;
        entry.snapshot.error = error;
        Ok(entry.snapshot.clone())
    }

    async fn start_device_flow(&self, provider: &ProviderConfig) -> Result<AuthFlowSnapshot> {
        let (device_code, user_code, verification_uri, interval, expires_in) =
            match provider_kind(provider) {
                ProviderKind::KimiCoding => {
                    let response = self
                        .http
                        .post(format!("{KIMI_AUTH_BASE}/api/oauth/device_authorization"))
                        .form(&[("client_id", KIMI_CLIENT_ID)])
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<Value>()
                        .await?;
                    (
                        required_string(&response, "device_code")?,
                        required_string(&response, "user_code")?,
                        response
                            .get("verification_uri_complete")
                            .and_then(Value::as_str)
                            .or_else(|| response.get("verification_uri").and_then(Value::as_str))
                            .context("Kimi OAuth response missing verification URI")?
                            .to_string(),
                        positive_u64(&response, "interval").unwrap_or(5),
                        positive_u64(&response, "expires_in").unwrap_or(15 * 60),
                    )
                }
                ProviderKind::Xai => {
                    let response = self
                        .http
                        .post("https://auth.x.ai/oauth2/device/code")
                        .form(&[
                            ("client_id", XAI_CLIENT_ID),
                            ("scope", XAI_SCOPE),
                            ("referrer", "pwcli"),
                        ])
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<Value>()
                        .await?;
                    (
                        required_string(&response, "device_code")?,
                        required_string(&response, "user_code")?,
                        response
                            .get("verification_uri_complete")
                            .and_then(Value::as_str)
                            .or_else(|| response.get("verification_uri").and_then(Value::as_str))
                            .context("xAI OAuth response missing verification URI")?
                            .to_string(),
                        positive_u64(&response, "interval").unwrap_or(5),
                        positive_u64(&response, "expires_in").unwrap_or(15 * 60),
                    )
                }
                ProviderKind::OpenAiCodex => {
                    let response = self
                        .http
                        .post(format!(
                            "{OPENAI_AUTH_BASE}/api/accounts/deviceauth/usercode"
                        ))
                        .json(&serde_json::json!({"client_id": OPENAI_CODEX_CLIENT_ID}))
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<Value>()
                        .await?;
                    (
                        required_string(&response, "device_auth_id")?,
                        required_string(&response, "user_code")?,
                        format!("{OPENAI_AUTH_BASE}/codex/device"),
                        positive_u64(&response, "interval").unwrap_or(5),
                        15 * 60,
                    )
                }
                _ => anyhow::bail!("provider does not support device login"),
            };
        validate_http_url(&verification_uri)?;
        let flow_id = format!("auth_{}", uuid::Uuid::now_v7().simple());
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);
        let snapshot = AuthFlowSnapshot {
            flow_id: flow_id.clone(),
            provider_id: provider_id(provider).to_string(),
            method: AuthFlowMethod::DeviceCode,
            state: AuthFlowState::Pending,
            verification_uri: Some(verification_uri),
            user_code: Some(user_code.clone()),
            expires_at: Some(expires_at),
            poll_interval_ms: Some(interval * 1000),
            error: None,
        };
        self.flows.write().await.insert(
            flow_id.clone(),
            FlowEntry {
                snapshot: snapshot.clone(),
                secret: FlowSecret::Device {
                    kind: provider_kind(provider),
                    device_code,
                    interval: Duration::from_secs(interval),
                    expires_at,
                },
            },
        );
        let manager = self.clone();
        tokio::spawn(async move { manager.poll_device_flow(flow_id, user_code).await });
        Ok(snapshot)
    }

    async fn poll_device_flow(&self, flow_id: String, user_code: String) {
        loop {
            let (provider_id, kind, device_code, interval, expires_at, state) = {
                let flows = self.flows.read().await;
                let Some(entry) = flows.get(&flow_id) else {
                    return;
                };
                let FlowSecret::Device {
                    kind,
                    device_code,
                    interval,
                    expires_at,
                } = &entry.secret
                else {
                    return;
                };
                (
                    entry.snapshot.provider_id.clone(),
                    *kind,
                    device_code.clone(),
                    *interval,
                    *expires_at,
                    entry.snapshot.state,
                )
            };
            if state != AuthFlowState::Pending {
                return;
            }
            if chrono::Utc::now() >= expires_at {
                let _ = self
                    .finish_flow(&flow_id, AuthFlowState::Expired, None)
                    .await;
                return;
            }
            tokio::time::sleep(interval).await;
            match self.poll_device_token(kind, &device_code, &user_code).await {
                Ok(Some(credential)) => {
                    let result = self
                        .store
                        .write(&provider_id, Credential::OAuth(credential));
                    let _ = match result {
                        Ok(()) => {
                            self.finish_flow(&flow_id, AuthFlowState::Connected, None)
                                .await
                        }
                        Err(error) => {
                            self.finish_flow(
                                &flow_id,
                                AuthFlowState::Failed,
                                Some(error.to_string()),
                            )
                            .await
                        }
                    };
                    return;
                }
                Ok(None) => {}
                Err(error) if error.to_string().contains("slow_down") => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(error) => {
                    let _ = self
                        .finish_flow(&flow_id, AuthFlowState::Failed, Some(error.to_string()))
                        .await;
                    return;
                }
            }
        }
    }

    async fn poll_device_token(
        &self,
        kind: ProviderKind,
        device_code: &str,
        user_code: &str,
    ) -> Result<Option<OAuthCredential>> {
        let response = match kind {
            ProviderKind::KimiCoding => {
                self.http
                    .post(format!("{KIMI_AUTH_BASE}/api/oauth/token"))
                    .form(&[
                        ("client_id", KIMI_CLIENT_ID),
                        ("device_code", device_code),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ])
                    .send()
                    .await?
            }
            ProviderKind::Xai => {
                self.http
                    .post("https://auth.x.ai/oauth2/token")
                    .form(&[
                        ("client_id", XAI_CLIENT_ID),
                        ("device_code", device_code),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ])
                    .send()
                    .await?
            }
            ProviderKind::OpenAiCodex => {
                let response = self
                    .http
                    .post(format!("{OPENAI_AUTH_BASE}/api/accounts/deviceauth/token"))
                    .json(&serde_json::json!({
                        "device_auth_id": device_code,
                        "user_code": user_code,
                    }))
                    .send()
                    .await?;
                if response.status().as_u16() == 403 || response.status().as_u16() == 404 {
                    return Ok(None);
                }
                let response = response.error_for_status()?.json::<Value>().await?;
                let code = required_string(&response, "authorization_code")?;
                let verifier = required_string(&response, "code_verifier")?;
                return self
                    .exchange_codex_code(&code, &verifier, OPENAI_CODEX_DEVICE_REDIRECT)
                    .await
                    .map(Some);
            }
            _ => anyhow::bail!("provider does not support device OAuth"),
        };
        let status = response.status();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        if status.is_success() {
            return parse_oauth_credential(kind, &value, None).map(Some);
        }
        match value.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => Ok(None),
            Some("slow_down") => anyhow::bail!("slow_down"),
            Some("expired_token") => anyhow::bail!("device code expired"),
            Some("access_denied" | "authorization_denied") => anyhow::bail!("login was denied"),
            Some(error) => anyhow::bail!("OAuth token polling failed: {error}"),
            None => anyhow::bail!("OAuth token polling failed with HTTP {status}"),
        }
    }

    async fn start_codex_browser_flow(
        &self,
        provider: &ProviderConfig,
    ) -> Result<AuthFlowSnapshot> {
        let mut verifier_bytes = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut verifier_bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let mut state_bytes = [0_u8; 16];
        rand::thread_rng().fill_bytes(&mut state_bytes);
        let state = hex::encode(state_bytes);
        let mut url = reqwest::Url::parse(&format!("{OPENAI_AUTH_BASE}/oauth/authorize"))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", OPENAI_CODEX_CLIENT_ID)
            .append_pair("redirect_uri", OPENAI_CODEX_REDIRECT)
            .append_pair("scope", "openid profile email offline_access")
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", "pwcli");
        let flow_id = format!("auth_{}", uuid::Uuid::now_v7().simple());
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(15);
        let snapshot = AuthFlowSnapshot {
            flow_id: flow_id.clone(),
            provider_id: provider_id(provider).to_string(),
            method: AuthFlowMethod::Browser,
            state: AuthFlowState::Pending,
            verification_uri: Some(url.to_string()),
            user_code: None,
            expires_at: Some(expires_at),
            poll_interval_ms: None,
            error: None,
        };
        self.flows.write().await.insert(
            flow_id.clone(),
            FlowEntry {
                snapshot: snapshot.clone(),
                secret: FlowSecret::Browser {
                    kind: ProviderKind::OpenAiCodex,
                    verifier,
                    state: state.clone(),
                    redirect_uri: OPENAI_CODEX_REDIRECT.to_string(),
                },
            },
        );
        self.spawn_browser_callback(flow_id, state, 1455, "OpenAI")
            .await;
        Ok(snapshot)
    }

    async fn start_antigravity_browser_flow(
        &self,
        provider: &ProviderConfig,
    ) -> Result<AuthFlowSnapshot> {
        let (client_id, _) = antigravity_oauth_config()?;
        let mut verifier_bytes = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut verifier_bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let mut state_bytes = [0_u8; 16];
        rand::thread_rng().fill_bytes(&mut state_bytes);
        let state = hex::encode(state_bytes);
        let mut url = reqwest::Url::parse("https://accounts.google.com/o/oauth2/v2/auth")?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", ANTIGRAVITY_REDIRECT)
            .append_pair("scope", ANTIGRAVITY_SCOPES)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent select_account")
            .append_pair("state", &state);
        let flow_id = format!("auth_{}", uuid::Uuid::now_v7().simple());
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(15);
        let snapshot = AuthFlowSnapshot {
            flow_id: flow_id.clone(),
            provider_id: provider_id(provider).to_string(),
            method: AuthFlowMethod::Browser,
            state: AuthFlowState::Pending,
            verification_uri: Some(url.to_string()),
            user_code: None,
            expires_at: Some(expires_at),
            poll_interval_ms: None,
            error: None,
        };
        self.flows.write().await.insert(
            flow_id.clone(),
            FlowEntry {
                snapshot: snapshot.clone(),
                secret: FlowSecret::Browser {
                    kind: ProviderKind::GoogleAntigravity,
                    verifier,
                    state: state.clone(),
                    redirect_uri: ANTIGRAVITY_REDIRECT.to_string(),
                },
            },
        );
        self.spawn_browser_callback(flow_id, state, 51121, "Google Antigravity")
            .await;
        Ok(snapshot)
    }

    async fn spawn_browser_callback(
        &self,
        flow_id: String,
        expected_state: String,
        port: u16,
        provider_name: &'static str,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => listener,
                Err(error) => {
                    tracing::info!(%error, %provider_name, "OAuth callback port unavailable; manual completion remains available");
                    return;
                }
            };
            let accepted =
                tokio::time::timeout(Duration::from_secs(15 * 60), listener.accept()).await;
            let Ok(Ok((mut stream, _))) = accepted else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buffer = vec![0_u8; 8192];
            let count = stream.read(&mut buffer).await.unwrap_or_default();
            let request = String::from_utf8_lossy(&buffer[..count]);
            let target = request.split_whitespace().nth(1).unwrap_or_default();
            let input = format!("http://127.0.0.1:{port}{target}");
            let state_matches = reqwest::Url::parse(&input)
                .ok()
                .and_then(|url| {
                    url.query_pairs()
                        .find(|(key, _)| key == "state")
                        .map(|(_, value)| value.into_owned())
                })
                .as_deref()
                == Some(expected_state.as_str());
            let result = if state_matches {
                manager.complete_flow(&flow_id, &input).await
            } else {
                Err(anyhow::anyhow!("OAuth state mismatch"))
            };
            let (status, body) = if result.is_ok() {
                (
                    "200 OK",
                    "Authentication completed. You can close this window.",
                )
            } else {
                (
                    "400 Bad Request",
                    "Authentication failed. Return to PWCLI for details.",
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }

    async fn exchange_codex_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<OAuthCredential> {
        let value = self
            .http
            .post(format!("{OPENAI_AUTH_BASE}/oauth/token"))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", OPENAI_CODEX_CLIENT_ID),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        parse_oauth_credential(ProviderKind::OpenAiCodex, &value, None)
    }

    async fn exchange_antigravity_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<OAuthCredential> {
        let (client_id, client_secret) = antigravity_oauth_config()?;
        let value = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let mut credential = parse_oauth_credential(ProviderKind::GoogleAntigravity, &value, None)?;
        credential.account_label = extract_google_email(&value);
        credential.project_id = Some(
            self.discover_antigravity_project(&credential.access)
                .await
                .context("Google account has no Antigravity / Cloud Code Assist project")?,
        );
        Ok(credential)
    }

    async fn discover_antigravity_project(&self, access: &str) -> Result<String> {
        let response = self
            .http
            .post("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist")
            .bearer_auth(access)
            .header("User-Agent", "antigravity/cli/1.21.9")
            .json(&serde_json::json!({"metadata": {"ideType": "ANTIGRAVITY"}}))
            .send()
            .await?;
        if response.status().is_success() {
            let value = response.json::<Value>().await?;
            if let Some(project) = extract_antigravity_project(&value) {
                return Ok(project);
            }
        }
        for _ in 0..5 {
            let response = self
                .http
                .post("https://daily-cloudcode-pa.googleapis.com/v1internal:onboardUser")
                .bearer_auth(access)
                .header("User-Agent", "antigravity/cli/1.21.9")
                .header("x-goog-api-client", "gl-rust/1.0 antigravity/1.21.9")
                .json(&serde_json::json!({
                    "tier_id": "free-tier",
                    "metadata": {
                        "ide_type": "ANTIGRAVITY",
                        "ide_name": "antigravity",
                        "ide_version": "1.21.9"
                    }
                }))
                .send()
                .await?;
            if response.status().is_success() {
                let value = response.json::<Value>().await.unwrap_or(Value::Null);
                if let Some(project) = value
                    .get("response")
                    .and_then(extract_antigravity_project)
                    .or_else(|| extract_antigravity_project(&value))
                {
                    return Ok(project);
                }
            } else if response.status().as_u16() != 429 && !response.status().is_server_error() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        anyhow::bail!("Cloud Code Assist project discovery failed")
    }

    async fn refresh(
        &self,
        kind: ProviderKind,
        current: &OAuthCredential,
    ) -> Result<OAuthCredential> {
        if kind == ProviderKind::GoogleAntigravity {
            let (client_id, client_secret) = antigravity_oauth_config()?;
            let value = self
                .http
                .post("https://oauth2.googleapis.com/token")
                .form(&[
                    ("client_id", client_id.as_str()),
                    ("client_secret", client_secret.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", current.refresh.as_str()),
                ])
                .send()
                .await?
                .error_for_status()?
                .json::<Value>()
                .await?;
            let mut credential = parse_oauth_credential(kind, &value, Some(current))?;
            if credential.project_id.is_none() {
                credential.project_id = self
                    .discover_antigravity_project(&credential.access)
                    .await
                    .ok();
            }
            return Ok(credential);
        }
        let (url, fields): (&str, Vec<(&str, &str)>) = match kind {
            ProviderKind::KimiCoding => (
                "https://auth.kimi.com/api/oauth/token",
                vec![
                    ("client_id", KIMI_CLIENT_ID),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", current.refresh.as_str()),
                ],
            ),
            ProviderKind::Xai => (
                "https://auth.x.ai/oauth2/token",
                vec![
                    ("client_id", XAI_CLIENT_ID),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", current.refresh.as_str()),
                ],
            ),
            ProviderKind::OpenAiCodex => (
                "https://auth.openai.com/oauth/token",
                vec![
                    ("client_id", OPENAI_CODEX_CLIENT_ID),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", current.refresh.as_str()),
                ],
            ),
            _ => anyhow::bail!("provider does not support OAuth refresh"),
        };
        let value = self
            .http
            .post(url)
            .form(&fields)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let mut credential = parse_oauth_credential(kind, &value, Some(current))?;
        if kind == ProviderKind::GoogleAntigravity && credential.project_id.is_none() {
            credential.project_id = self
                .discover_antigravity_project(&credential.access)
                .await
                .ok();
        }
        Ok(credential)
    }
}

fn antigravity_oauth_config() -> Result<(String, String)> {
    let client_id = std::env::var("GOOGLE_ANTIGRAVITY_CLIENT_ID")
        .context("Google Antigravity login requires GOOGLE_ANTIGRAVITY_CLIENT_ID")?;
    let client_secret = std::env::var("GOOGLE_ANTIGRAVITY_CLIENT_SECRET")
        .context("Google Antigravity login requires GOOGLE_ANTIGRAVITY_CLIENT_SECRET")?;
    if client_id.trim().is_empty() || client_secret.trim().is_empty() {
        anyhow::bail!("Google Antigravity OAuth client configuration cannot be empty");
    }
    Ok((client_id, client_secret))
}

fn required_string(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| format!("OAuth response missing {field}"))
}

fn positive_u64(value: &Value, field: &str) -> Option<u64> {
    value
        .get(field)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .filter(|value| *value > 0)
}

fn parse_oauth_credential(
    kind: ProviderKind,
    value: &Value,
    previous: Option<&OAuthCredential>,
) -> Result<OAuthCredential> {
    let access = required_string(value, "access_token")?;
    let refresh = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| previous.map(|value| value.refresh.clone()))
        .context("OAuth response missing refresh_token")?;
    let expires_in = positive_u64(value, "expires_in").unwrap_or(3600);
    let account_id = if kind == ProviderKind::OpenAiCodex {
        Some(extract_codex_account_id(&access)?)
    } else {
        previous.and_then(|value| value.account_id.clone())
    };
    Ok(OAuthCredential {
        access,
        refresh,
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64),
        account_id,
        account_label: previous.and_then(|value| value.account_label.clone()),
        project_id: previous.and_then(|value| value.project_id.clone()),
    })
}

fn extract_google_email(value: &Value) -> Option<String> {
    let token = value.get("id_token")?.as_str()?;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("email")?
        .as_str()
        .map(str::to_string)
}

fn extract_antigravity_project(value: &Value) -> Option<String> {
    for key in ["cloudaicompanionProject", "projectId", "project"] {
        if let Some(project) = value.get(key).and_then(Value::as_str) {
            if !project.is_empty() {
                return Some(project.to_string());
            }
        }
        if let Some(project) = value.pointer(&format!("/{key}/id")).and_then(Value::as_str) {
            if !project.is_empty() {
                return Some(project.to_string());
            }
        }
    }
    None
}

pub fn extract_codex_account_id(token: &str) -> Result<String> {
    let payload = token
        .split('.')
        .nth(1)
        .context("invalid Codex access token")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .context("decode Codex access token")?;
    let value: Value = serde_json::from_slice(&bytes)?;
    value
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .context("Codex access token has no ChatGPT account id")
}

fn parse_authorization_input(input: &str) -> Result<(String, Option<String>)> {
    let input = input.trim();
    if let Ok(url) = reqwest::Url::parse(input) {
        let code = url
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned())
            .context("authorization URL has no code")?;
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned());
        return Ok((code, state));
    }
    if let Some((code, state)) = input.split_once('#') {
        return Ok((code.to_string(), Some(state.to_string())));
    }
    if input.is_empty() {
        anyhow::bail!("authorization code must not be empty");
    }
    Ok((input.to_string(), None))
}

fn validate_http_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("untrusted OAuth verification URL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manual_codex_inputs() {
        assert_eq!(
            parse_authorization_input("code#state").unwrap(),
            ("code".into(), Some("state".into()))
        );
        assert_eq!(
            parse_authorization_input("http://localhost:1455/auth/callback?code=c&state=s")
                .unwrap(),
            ("c".into(), Some("s".into()))
        );
    }

    #[test]
    fn rejects_non_http_verification_urls() {
        assert!(validate_http_url("file:///tmp/nope").is_err());
    }

    #[test]
    fn extracts_antigravity_project_shapes() {
        assert_eq!(
            extract_antigravity_project(&serde_json::json!({
                "cloudaicompanionProject": "project-a"
            })),
            Some("project-a".into())
        );
        assert_eq!(
            extract_antigravity_project(&serde_json::json!({
                "project": {"id": "project-b"}
            })),
            Some("project-b".into())
        );
    }
}
