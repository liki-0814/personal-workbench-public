use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use futures::TryStreamExt;
use serde_json::{json, Value};

use super::super::state::AppState;
use super::config::ProviderEndpoint;
use crate::ai::config::ProviderConfig;
use crate::ai::provider::AuthManager;
use crate::runtime::image_generation::{ImageGenerationRequest, ImageGenerationService};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/proxy/openai", post(openai))
        .route("/api/proxy/responses", post(openai_responses))
        .route("/api/proxy/anthropic", post(anthropic))
        .route("/api/proxy/google", post(google))
        .route("/api/proxy/generate-image", post(generate_image))
}

#[derive(Clone, Copy)]
enum UpstreamAuth {
    Bearer,
    Anthropic,
    Google,
}

async fn openai(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    forward(
        &state,
        &headers,
        ProxyEndpoint::Relative("chat/completions"),
        body,
        UpstreamAuth::Bearer,
    )
    .await
}

async fn openai_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    forward(
        &state,
        &headers,
        ProxyEndpoint::Relative("responses"),
        body,
        UpstreamAuth::Bearer,
    )
    .await
}

async fn anthropic(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    forward(
        &state,
        &headers,
        ProxyEndpoint::Relative("v1/messages"),
        body,
        UpstreamAuth::Anthropic,
    )
    .await
}

async fn google(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    forward(
        &state,
        &headers,
        ProxyEndpoint::Google,
        body,
        UpstreamAuth::Google,
    )
    .await
}

#[derive(Clone, Copy)]
enum ProxyEndpoint {
    Relative(&'static str),
    Google,
}

async fn forward(
    state: &AppState,
    headers: &HeaderMap,
    endpoint: ProxyEndpoint,
    body: Value,
    auth: UpstreamAuth,
) -> Response<Body> {
    let provider = match selected_provider(state, headers).await {
        Ok(provider) if !provider.base_url.is_empty() => provider,
        Ok(_) => return error(StatusCode::BAD_REQUEST, "Provider URL is empty"),
        Err(response) => return *response,
    };
    let upstream_url = match upstream_url(&provider, headers, endpoint) {
        Ok(url) => url,
        Err(cause) => return error(StatusCode::BAD_REQUEST, &cause.to_string()),
    };
    let request = reqwest::Client::new()
        .post(upstream_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body);
    let request = match auth {
        UpstreamAuth::Anthropic => request
            .header("x-api-key", provider.api_key)
            .header("anthropic-version", "2023-06-01"),
        UpstreamAuth::Bearer => request.bearer_auth(provider.api_key),
        UpstreamAuth::Google => request.header("x-goog-api-key", provider.api_key),
    };
    let request = if let Some(user_agent) = provider.user_agent.as_deref() {
        request.header(header::USER_AGENT, user_agent)
    } else {
        request
    };
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(cause) => return error(StatusCode::BAD_GATEWAY, &cause.to_string()),
    };
    let status = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let cache_control = upstream.headers().get(header::CACHE_CONTROL).cloned();
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    if let Some(value) = content_type {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Some(value) = cache_control {
        response.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    response
}

fn upstream_url(
    provider: &ProviderEndpoint,
    headers: &HeaderMap,
    endpoint: ProxyEndpoint,
) -> anyhow::Result<String> {
    match endpoint {
        ProxyEndpoint::Relative(path) => Ok(format!("{}/{path}", provider.base_url)),
        ProxyEndpoint::Google => {
            let model = required_header(headers, "x-model-id")?;
            if !provider.models.iter().any(|entry| {
                entry.get("id").and_then(Value::as_str) == Some(model)
                    || entry.get("name").and_then(Value::as_str) == Some(model)
            }) {
                anyhow::bail!("Model is not configured for the selected provider");
            }
            let action = required_header(headers, "x-google-action")?;
            if !matches!(action, "generateContent" | "streamGenerateContent") {
                anyhow::bail!("Invalid Google Generative action");
            }
            let base = provider.base_url.trim_end_matches('/');
            let mut url = if base.contains("{model}") {
                base.replace("{model}", model).replace("{action}", action)
            } else if base.contains(":generateContent") || base.contains(":streamGenerateContent") {
                base.to_string()
            } else if base.ends_with(&format!("/models/{model}")) {
                format!("{base}:{action}")
            } else {
                format!("{base}/models/{model}:{action}")
            };
            if action == "streamGenerateContent" && !url.contains("alt=sse") {
                url.push(if url.contains('?') { '&' } else { '?' });
                url.push_str("alt=sse");
            }
            Ok(url)
        }
    }
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> anyhow::Result<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing {name} header"))
}

async fn generate_image(
    State(state): State<AppState>,
    Json(request): Json<ImageGenerationRequest>,
) -> Response<Body> {
    let Some(web) = state.web.as_ref() else {
        return error(StatusCode::NOT_FOUND, "Web service is disabled");
    };
    let data_dir = web
        .data
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    match ImageGenerationService::new(data_dir)
        .generate(&request, None)
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => json_response(StatusCode::OK, value),
            Err(cause) => error(StatusCode::INTERNAL_SERVER_ERROR, &cause.to_string()),
        },
        Err(cause) => {
            let message = cause.to_string();
            let status = if message.contains("upstream")
                || message.contains("provider response")
                || message.contains("Unable to")
            {
                StatusCode::BAD_GATEWAY
            } else {
                StatusCode::BAD_REQUEST
            };
            error(status, &message)
        }
    }
}

async fn selected_provider(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ProviderEndpoint, Box<Response<Body>>> {
    let config = &state
        .web
        .as_ref()
        .ok_or_else(|| Box::new(error(StatusCode::NOT_FOUND, "Web service is disabled")))?
        .config;
    let provider = if let Some(id) = headers
        .get("x-provider-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.provider_by_id(id)
    } else if let Some(index) = headers
        .get("x-provider-index")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
    {
        // Backward compatibility for an already-open browser tab from an older release.
        config.provider(index)
    } else {
        Err(anyhow::anyhow!("Missing x-provider-id header"))
    }
    .map_err(|cause| Box::new(error(StatusCode::BAD_REQUEST, &cause.to_string())))?;
    resolve_provider_auth(state.auth_manager.as_ref(), provider)
        .await
        .map_err(|cause| Box::new(error(StatusCode::UNAUTHORIZED, &cause.to_string())))
}

async fn resolve_provider_auth(
    auth_manager: &AuthManager,
    mut provider: ProviderEndpoint,
) -> anyhow::Result<ProviderEndpoint> {
    // Provider API keys and OAuth tokens live in CredentialStore. config.json
    // deliberately keeps api_key empty after migration, so the proxy must use
    // the same authentication resolver as the normal LLM runtime.
    let credential_profile = provider.compat_profile.clone().or_else(|| {
        (!provider.id.trim().is_empty()).then(|| format!("credential:{}", provider.id))
    });
    let configured = ProviderConfig {
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
        protocol: provider.protocol.clone(),
        model: String::new(),
        models: Vec::new(),
        use_proxy: provider.use_proxy,
        compat_profile: credential_profile,
    };
    provider.api_key = auth_manager.resolve(&configured).await?.api_key;
    Ok(provider)
}

fn error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, json!({ "error": message }))
}

fn json_response(status: StatusCode, value: Value) -> Response<Body> {
    let mut response = Response::new(Body::from(value.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::provider::{Credential, CredentialStore};

    fn endpoint(api_key: &str) -> ProviderEndpoint {
        ProviderEndpoint {
            id: "provider-test".into(),
            name: "Test".into(),
            base_url: "https://api.example.test".into(),
            api_key: api_key.into(),
            protocol: "anthropic_messages".into(),
            models: Vec::new(),
            use_proxy: Some(true),
            user_agent: Some("claude-cli/test".into()),
            compat_profile: Some("credential:provider-test".into()),
        }
    }

    #[tokio::test]
    async fn proxy_resolves_api_key_from_credential_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        store
            .write(
                "provider-test",
                Credential::ApiKey {
                    key: "stored-secret".into(),
                },
            )
            .unwrap();
        let manager = AuthManager::new(store);

        let mut provider = endpoint("");
        // The stable provider id is sufficient even if an older record has no
        // explicit credential marker yet.
        provider.compat_profile = None;
        let resolved = resolve_provider_auth(&manager, provider).await.unwrap();

        assert_eq!(resolved.api_key, "stored-secret");
    }

    #[tokio::test]
    async fn proxy_keeps_legacy_config_key_as_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let manager = AuthManager::new(CredentialStore::new(dir.path().join("credentials.json")));

        let resolved = resolve_provider_auth(&manager, endpoint("legacy-secret"))
            .await
            .unwrap();

        assert_eq!(resolved.api_key, "legacy-secret");
    }

    #[tokio::test]
    async fn proxy_reports_missing_authentication() {
        let dir = tempfile::tempdir().unwrap();
        let manager = AuthManager::new(CredentialStore::new(dir.path().join("credentials.json")));

        let error = match resolve_provider_auth(&manager, endpoint("")).await {
            Ok(_) => panic!("expected missing authentication to fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("is not authenticated"));
    }
}
