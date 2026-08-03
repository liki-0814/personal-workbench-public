use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::ProviderConfig;
use crate::provider_ai::{
    provider_kind, AuthFlowMethod, Credential, ProviderCatalog, ProviderKind,
};

use super::super::state::AppState;
use super::WebState;

type ApiError = (StatusCode, Json<Value>);
type ApiResult = Result<Json<Value>, ApiError>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProviderRequest {
    #[serde(default)]
    kind: ProviderKind,
    #[serde(default)]
    name: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    default_model: String,
    #[serde(default)]
    models: Vec<crate::config::provider::ModelEntry>,
    #[serde(default)]
    use_proxy: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchProviderRequest {
    name: Option<String>,
    base_url: Option<String>,
    protocol: Option<String>,
    api_key: Option<String>,
    default_model: Option<String>,
    models: Option<Vec<crate::config::provider::ModelEntry>>,
    enabled: Option<bool>,
    use_proxy: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PriorityRequest {
    provider_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartAuthRequest {
    method: AuthFlowMethod,
}

#[derive(Debug, Deserialize)]
struct CompleteAuthRequest {
    #[serde(rename = "flowId")]
    flow_id: String,
    input: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowRequest {
    flow_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatusQuery {
    flow_id: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/providers", get(list).post(create))
        .route("/api/providers/catalog", get(catalog))
        .route("/api/providers/priorities", put(priorities))
        .route("/api/providers/{id}", patch(update).delete(remove))
        .route("/api/providers/{id}/test", post(test_connection))
        .route("/api/providers/{id}/auth/start", post(start_auth))
        .route("/api/providers/{id}/auth/status", get(auth_status))
        .route("/api/providers/{id}/auth/complete", post(complete_auth))
        .route("/api/providers/{id}/auth/cancel", post(cancel_auth))
        .route("/api/providers/{id}/auth/logout", post(logout))
}

async fn catalog() -> ApiResult {
    let providers = ProviderCatalog
        .all()
        .into_iter()
        .map(|provider| {
            json!({
                "kind": provider.kind,
                "name": provider.name,
                "authMethod": if matches!(provider.kind, ProviderKind::QwenTokenPlanCn) { "api_key" } else { "oauth" },
                "defaultModel": provider.default_model,
                "models": provider.models,
            })
        })
        .collect::<Vec<_>>();
    Ok(success(Value::Array(providers)))
}

async fn list(State(state): State<AppState>) -> ApiResult {
    migrate_legacy(&state).map_err(internal)?;
    let values =
        provider_values(web(&state)?.config.read().map_err(internal)?).map_err(bad_request)?;
    let mut views = Vec::with_capacity(values.len());
    for value in values {
        views.push(provider_view(&state, &value).await.map_err(internal)?);
    }
    Ok(success(Value::Array(views)))
}

async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateProviderRequest>,
) -> ApiResult {
    migrate_legacy(&state).map_err(internal)?;
    let id = format!("provider_{}", uuid::Uuid::now_v7().simple());
    let (name, base_url, protocol, model, models, compat_profile) = if request.kind.is_builtin() {
        let builtin = ProviderCatalog
            .get(request.kind)
            .context("unknown built-in provider")
            .map_err(bad_request)?;
        (
            builtin.name.to_string(),
            builtin.base_url.to_string(),
            builtin.protocol.to_string(),
            if request.default_model.trim().is_empty() {
                builtin.default_model.to_string()
            } else {
                request.default_model.trim().to_string()
            },
            builtin.models,
            Some(format!("builtin:{};credential:{id}", request.kind.as_str())),
        )
    } else {
        let name = request.name.trim();
        let base_url = request.base_url.trim();
        if name.is_empty() || base_url.is_empty() {
            return Err(bad_request("custom provider requires name and baseUrl"));
        }
        validate_custom_protocol(&request.protocol).map_err(bad_request)?;
        let model = if request.default_model.trim().is_empty() {
            request
                .models
                .first()
                .map(|entry| entry.id.clone())
                .unwrap_or_default()
        } else {
            request.default_model.trim().to_string()
        };
        (
            name.to_string(),
            base_url.to_string(),
            normalize_protocol(&request.protocol),
            model,
            request.models,
            Some(format!("credential:{id}")),
        )
    };
    let stored = json!({
        "id": id,
        "name": name,
        "base_url": base_url,
        "api_key": "",
        "protocol": protocol,
        "model": model,
        "models": models,
        "enabled": true,
        "useProxy": request.use_proxy,
        "compatProfile": compat_profile,
    });
    let stored_id = id.clone();
    let kind = request.kind;
    let new_credential = request
        .api_key
        .filter(|key| !key.trim().is_empty())
        .map(|key| Credential::ApiKey { key });
    if let Some(credential) = new_credential.clone() {
        state
            .auth_manager
            .store()
            .write(&id, credential)
            .map_err(internal)?;
    }
    let config_result = web(&state)?.config.update(move |root| {
        let providers = ensure_provider_array(root)?;
        if kind.is_builtin()
            && providers
                .iter()
                .any(|value| provider_kind_from_value(value) == kind)
        {
            anyhow::bail!("this built-in provider is already configured");
        }
        providers.push(stored);
        if root.get("active_provider").is_none_or(Value::is_null) {
            root["active_provider"] = Value::String(stored_id);
        }
        Ok(())
    });
    if let Err(error) = config_result {
        if new_credential.is_some() {
            let _ = state.auth_manager.store().remove(&id);
        }
        return Err(bad_request(error));
    }
    publish(&state);
    let value = find_provider(web(&state)?, &id).map_err(internal)?;
    Ok(success(
        provider_view(&state, &value).await.map_err(internal)?,
    ))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PatchProviderRequest>,
) -> ApiResult {
    migrate_legacy(&state).map_err(internal)?;
    let key = request
        .api_key
        .clone()
        .filter(|value| !value.trim().is_empty());
    let target_id = id.clone();
    web(&state)?
        .config
        .update(move |root| {
            let providers = ensure_provider_array(root)?;
            let provider = providers
                .iter_mut()
                .find(|value| value.get("id").and_then(Value::as_str) == Some(target_id.as_str()))
                .context("provider not found")?;
            let kind = provider_kind_from_value(provider);
            if let Some(enabled) = request.enabled {
                provider["enabled"] = json!(enabled);
            }
            if let Some(model) = request.default_model {
                provider["model"] = json!(model.trim());
            }
            if let Some(models) = request.models {
                provider["models"] = serde_json::to_value(models)?;
            }
            if let Some(use_proxy) = request.use_proxy {
                provider["useProxy"] = json!(use_proxy);
            }
            if kind == ProviderKind::Custom {
                if let Some(name) = request.name.filter(|value| !value.trim().is_empty()) {
                    provider["name"] = json!(name.trim());
                }
                if let Some(base_url) = request.base_url.filter(|value| !value.trim().is_empty()) {
                    provider["base_url"] = json!(base_url.trim());
                }
                if let Some(protocol) = request.protocol {
                    validate_custom_protocol(&protocol)?;
                    provider["protocol"] = json!(normalize_protocol(&protocol));
                }
            }
            Ok(())
        })
        .map_err(bad_request)?;
    if let Some(key) = key {
        state
            .auth_manager
            .store()
            .write(&id, Credential::ApiKey { key })
            .map_err(internal)?;
    }
    publish(&state);
    let value = find_provider(web(&state)?, &id).map_err(internal)?;
    Ok(success(
        provider_view(&state, &value).await.map_err(internal)?,
    ))
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let target_id = id.clone();
    web(&state)?
        .config
        .update(move |root| {
            let removing_active =
                root.get("active_provider").and_then(Value::as_str) == Some(target_id.as_str());
            let next_active = {
                let providers = ensure_provider_array(root)?;
                let before = providers.len();
                providers.retain(|value| {
                    value.get("id").and_then(Value::as_str) != Some(target_id.as_str())
                });
                if providers.len() == before {
                    anyhow::bail!("provider not found");
                }
                providers
                    .first()
                    .and_then(|value| value.get("id"))
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            if removing_active {
                root["active_provider"] = next_active;
            }
            Ok(())
        })
        .map_err(bad_request)?;
    state.auth_manager.store().remove(&id).map_err(internal)?;
    publish(&state);
    Ok(success(json!({ "deleted": id })))
}

async fn priorities(
    State(state): State<AppState>,
    Json(request): Json<PriorityRequest>,
) -> ApiResult {
    let ids = request.provider_ids;
    web(&state)?
        .config
        .update(move |root| {
            let providers = ensure_provider_array(root)?;
            if ids.len() != providers.len() {
                anyhow::bail!("providerIds must contain every provider exactly once");
            }
            let mut by_id = std::collections::HashMap::new();
            for provider in std::mem::take(providers) {
                let id = provider
                    .get("id")
                    .and_then(Value::as_str)
                    .context("provider is missing id")?
                    .to_string();
                if by_id.insert(id, provider).is_some() {
                    anyhow::bail!("provider ids must be unique");
                }
            }
            for id in ids {
                providers.push(
                    by_id
                        .remove(&id)
                        .with_context(|| format!("unknown provider id `{id}`"))?,
                );
            }
            if !by_id.is_empty() {
                anyhow::bail!("providerIds must contain every provider exactly once");
            }
            Ok(())
        })
        .map_err(bad_request)?;
    publish(&state);
    list(State(state)).await
}

async fn test_connection(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let provider = provider_for_auth(web(&state)?, &id).map_err(bad_request)?;
    let provider = ProviderCatalog.materialize(&provider);
    state
        .auth_manager
        .resolve(&provider)
        .await
        .map_err(bad_request)?;
    let started = std::time::Instant::now();
    let client =
        crate::llm::LlmClient::with_provider(provider.clone(), state.config.backend_url.clone())
            .with_auth_manager(std::sync::Arc::clone(&state.auth_manager));
    client
        .chat_with_max_tokens(
            &[crate::llm::ChatMessage {
                role: "user".into(),
                content: "Reply with OK.".into(),
                images: Vec::new(),
                generated_images: Vec::new(),
                tool_calls: None,
                tool_call_id: None,
            }],
            Some("This is a connection test. Reply with OK only."),
            Some(16),
        )
        .await
        .map_err(bad_request)?;
    Ok(success(json!({
        "status": "ok",
        "model": provider.model,
        "latencyMs": started.elapsed().as_millis(),
    })))
}

async fn start_auth(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<StartAuthRequest>,
) -> ApiResult {
    let provider = provider_for_auth(web(&state)?, &id).map_err(bad_request)?;
    let flow = state
        .auth_manager
        .start_flow(&provider, request.method)
        .await
        .map_err(bad_request)?;
    Ok(success(serde_json::to_value(flow).map_err(internal)?))
}

async fn auth_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AuthStatusQuery>,
) -> ApiResult {
    if let Some(flow_id) = query.flow_id {
        let flow = state
            .auth_manager
            .flow(&flow_id)
            .await
            .context("authentication flow not found")
            .map_err(bad_request)?;
        if flow.provider_id != id {
            return Err(bad_request(
                "authentication flow belongs to another provider",
            ));
        }
        return Ok(success(serde_json::to_value(flow).map_err(internal)?));
    }
    let provider = provider_for_auth(web(&state)?, &id).map_err(bad_request)?;
    let status = state
        .auth_manager
        .status(&provider)
        .await
        .map_err(internal)?;
    Ok(success(serde_json::to_value(status).map_err(internal)?))
}

async fn complete_auth(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CompleteAuthRequest>,
) -> ApiResult {
    let existing = state
        .auth_manager
        .flow(&request.flow_id)
        .await
        .context("authentication flow not found")
        .map_err(bad_request)?;
    if existing.provider_id != id {
        return Err(bad_request(
            "authentication flow belongs to another provider",
        ));
    }
    let flow = state
        .auth_manager
        .complete_flow(&request.flow_id, &request.input)
        .await
        .map_err(bad_request)?;
    Ok(success(serde_json::to_value(flow).map_err(internal)?))
}

async fn cancel_auth(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<FlowRequest>,
) -> ApiResult {
    let existing = state
        .auth_manager
        .flow(&request.flow_id)
        .await
        .context("authentication flow not found")
        .map_err(bad_request)?;
    if existing.provider_id != id {
        return Err(bad_request(
            "authentication flow belongs to another provider",
        ));
    }
    let flow = state
        .auth_manager
        .cancel_flow(&request.flow_id)
        .await
        .map_err(bad_request)?;
    Ok(success(serde_json::to_value(flow).map_err(internal)?))
}

async fn logout(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    state.auth_manager.store().remove(&id).map_err(internal)?;
    Ok(success(json!({ "status": "disconnected" })))
}

fn migrate_legacy(state: &AppState) -> Result<()> {
    let web = state.web.as_ref().context("Web service is disabled")?;
    let root = web.config.read()?;
    let legacy_active = root
        .get("active_provider")
        .and_then(Value::as_str)
        .map(str::to_string);
    let providers = provider_values(root)?;
    let mut replacements = std::collections::HashMap::new();
    let mut identity_map = std::collections::HashMap::new();
    let mut first_provider_id = None;
    for (index, provider) in providers.iter().enumerate() {
        let id = provider
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("provider_{}", uuid::Uuid::now_v7().simple()));
        if first_provider_id.is_none() {
            first_provider_id = Some(id.clone());
        }
        identity_map.insert(id.clone(), id.clone());
        if let Some(name) = provider.get("name").and_then(Value::as_str) {
            identity_map
                .entry(name.to_string())
                .or_insert_with(|| id.clone());
        }
        let key = provider
            .get("api_key")
            .or_else(|| provider.get("apiKey"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !key.is_empty()
            && !key.contains("****")
            && state.auth_manager.store().read(&id)?.is_none()
        {
            state.auth_manager.store().write(
                &id,
                Credential::ApiKey {
                    key: key.to_string(),
                },
            )?;
        }
        let has_credential_marker = provider
            .get("compatProfile")
            .or_else(|| provider.get("compat_profile"))
            .and_then(Value::as_str)
            .is_some_and(|value| {
                value
                    .split(';')
                    .any(|part| part == format!("credential:{id}"))
            });
        if provider.get("id").and_then(Value::as_str) != Some(id.as_str())
            || !key.is_empty()
            || !has_credential_marker
        {
            let mut next = provider.clone();
            next["id"] = json!(id);
            next["api_key"] = json!("");
            if let Some(object) = next.as_object_mut() {
                object.remove("apiKey");
            }
            next["enabled"] = next.get("enabled").cloned().unwrap_or(json!(true));
            let kind = provider_kind_from_value(&next);
            next["compatProfile"] = json!(if kind.is_builtin() {
                format!("builtin:{};credential:{id}", kind.as_str())
            } else {
                format!("credential:{id}")
            });
            if let Some(object) = next.as_object_mut() {
                object.remove("compat_profile");
            }
            replacements.insert(index, next);
        }
    }
    let migrated_active = legacy_active
        .as_deref()
        .and_then(|active| identity_map.get(active))
        .cloned()
        .or(first_provider_id);
    let active_changed = legacy_active != migrated_active;
    if replacements.is_empty() && !active_changed {
        return Ok(());
    }
    web.config.update(move |root| {
        {
            let stored = ensure_provider_array(root)?;
            for (index, value) in replacements {
                if index < stored.len() {
                    stored[index] = value;
                }
            }
        }
        root["active_provider"] = migrated_active.map(Value::String).unwrap_or(Value::Null);
        Ok(())
    })?;
    publish(state);
    Ok(())
}

async fn provider_view(state: &AppState, raw: &Value) -> Result<Value> {
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .context("provider is missing id")?;
    let provider = config_from_value(raw)?;
    let kind = provider_kind(&provider);
    let materialized = ProviderCatalog.materialize(&provider);
    let auth = state.auth_manager.status(&provider).await?;
    let custom = kind == ProviderKind::Custom;
    Ok(json!({
        "id": id,
        "kind": kind,
        "name": materialized.name,
        "auth": auth,
        "models": materialized.models,
        "defaultModel": materialized.model,
        "enabled": raw.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        "priority": provider_index(state.web.as_ref().context("Web service is disabled")?, id)?,
        "builtin": kind.is_builtin(),
        "baseUrl": custom.then_some(provider.base_url.clone()),
        "protocol": custom.then_some(provider.protocol.clone()),
        "useProxy": custom.then_some(provider.use_proxy.unwrap_or(false)),
        "customEndpoint": if custom { Some(json!({
            "baseUrl": provider.base_url,
            "protocol": provider.protocol,
            "useProxy": provider.use_proxy,
        })) } else { None },
    }))
}

fn provider_for_auth(web: &WebState, id: &str) -> Result<ProviderConfig> {
    let raw = find_provider(web, id)?;
    config_from_value(&raw)
}

fn config_from_value(raw: &Value) -> Result<ProviderConfig> {
    let mut value = raw.clone();
    if value.get("base_url").is_none() {
        value["base_url"] = value.get("baseUrl").cloned().unwrap_or(json!(""));
    }
    if value.get("api_key").is_none() {
        value["api_key"] = value.get("apiKey").cloned().unwrap_or(json!(""));
    }
    if value.get("model").is_none() {
        value["model"] = value.get("defaultModel").cloned().unwrap_or(json!(""));
    }
    serde_json::from_value(value).context("invalid provider configuration")
}

fn find_provider(web: &WebState, id: &str) -> Result<Value> {
    provider_values(web.config.read()?)?
        .into_iter()
        .find(|value| value.get("id").and_then(Value::as_str) == Some(id))
        .context("provider not found")
}

fn provider_index(web: &WebState, id: &str) -> Result<usize> {
    provider_values(web.config.read()?)?
        .iter()
        .position(|value| value.get("id").and_then(Value::as_str) == Some(id))
        .context("provider not found")
}

fn provider_values(root: Value) -> Result<Vec<Value>> {
    Ok(root
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn ensure_provider_array(root: &mut Value) -> Result<&mut Vec<Value>> {
    if root.get("providers").is_none() {
        root["providers"] = json!([]);
    }
    root.get_mut("providers")
        .and_then(Value::as_array_mut)
        .context("providers must be an array")
}

fn provider_kind_from_value(value: &Value) -> ProviderKind {
    value
        .get("compatProfile")
        .or_else(|| value.get("compat_profile"))
        .and_then(Value::as_str)
        .and_then(|value| {
            value
                .split(';')
                .find_map(|part| part.strip_prefix("builtin:"))
        })
        .map(|kind| match kind {
            "kimi-coding" => ProviderKind::KimiCoding,
            "xai" => ProviderKind::Xai,
            "openai-codex" => ProviderKind::OpenAiCodex,
            "qwen-token-plan-cn" => ProviderKind::QwenTokenPlanCn,
            _ => ProviderKind::Custom,
        })
        .unwrap_or_default()
}

fn validate_custom_protocol(value: &str) -> Result<()> {
    match normalize_protocol(value).as_str() {
        "openai_chat" | "openai_responses" | "anthropic_messages" | "google_generative" => Ok(()),
        _ => anyhow::bail!("unsupported custom provider protocol"),
    }
}

fn normalize_protocol(value: &str) -> String {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "openai" | "openai_chat" | "openai_compatible" => "openai_chat".into(),
        "responses" | "openai_responses" => "openai_responses".into(),
        "anthropic" | "anthropic_messages" => "anthropic_messages".into(),
        "google" | "gemini" | "google_generative" | "generative_language" => {
            "google_generative".into()
        }
        other => other.to_string(),
    }
}

fn publish(state: &AppState) {
    if let Some(web) = &state.web {
        if let Ok(value) = web.config.frontend_providers(true) {
            let _ = web.data.set_raw("ai_providers", &value.to_string());
        }
        web.events
            .publish(vec!["ai_providers".to_string(), "providers".to_string()]);
    }
}

fn web(state: &AppState) -> Result<&std::sync::Arc<WebState>, ApiError> {
    state
        .web
        .as_ref()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Web service is disabled"))
}

fn success(data: Value) -> Json<Value> {
    Json(json!({ "success": true, "data": data }))
}
fn bad_request(cause: impl std::fmt::Display) -> ApiError {
    error(StatusCode::BAD_REQUEST, &cause.to_string())
}
fn internal(cause: impl std::fmt::Display) -> ApiError {
    error(StatusCode::INTERNAL_SERVER_ERROR, &cause.to_string())
}
fn error(status: StatusCode, message: &str) -> ApiError {
    (status, Json(json!({ "success": false, "error": message })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_builtin_kind_with_credential_marker() {
        assert_eq!(
            provider_kind_from_value(&json!({
                "compatProfile": "builtin:xai;credential:provider-1"
            })),
            ProviderKind::Xai
        );
    }

    #[test]
    fn custom_protocols_remain_distinct() {
        assert_eq!(normalize_protocol("openai-responses"), "openai_responses");
        assert_eq!(normalize_protocol("anthropic"), "anthropic_messages");
        assert_eq!(normalize_protocol("google_generative"), "google_generative");
    }
}
