use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::config::ProviderConfig;
use crate::ai::provider::{
    provider_kind, AuthFlowMethod, Credential, ProviderKind, ProviderRegistry,
};

use super::super::state::AppState;
use super::WebState;

type ApiError = (StatusCode, Json<Value>);
type ApiResult = Result<Json<Value>, ApiError>;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvidersQuery {
    /// Full diff: include models the user previously dismissed, used only by
    /// the explicit "check for model updates" action.
    full_diff: Option<bool>,
}

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
    models: Vec<crate::ai::config::ModelEntry>,
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
    models: Option<Vec<crate::ai::config::ModelEntry>>,
    /// Image-generation half of the model catalog.  Stored alongside chat
    /// models but managed independently; omitted means "keep existing".
    image_models: Option<Vec<crate::ai::config::ModelEntry>>,
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DismissModelChangesRequest {
    added: Option<bool>,
    removed: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdoptModelChangesRequest {
    model_ids: Option<Vec<String>>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/providers", get(list).post(create))
        .route("/api/providers/catalog", get(catalog))
        .route("/api/providers/priorities", put(priorities))
        .route("/api/providers/{id}", patch(update).delete(remove))
        .route("/api/providers/{id}/test", post(test_connection))
        .route(
            "/api/providers/{id}/model-changes/adopt",
            post(adopt_model_changes),
        )
        .route(
            "/api/providers/{id}/model-changes/prune",
            post(prune_model_changes),
        )
        .route(
            "/api/providers/{id}/model-changes/dismiss",
            post(dismiss_model_changes),
        )
        .route("/api/providers/{id}/auth/start", post(start_auth))
        .route("/api/providers/{id}/auth/status", get(auth_status))
        .route("/api/providers/{id}/auth/complete", post(complete_auth))
        .route("/api/providers/{id}/auth/cancel", post(cancel_auth))
        .route("/api/providers/{id}/auth/logout", post(logout))
}

async fn catalog(State(state): State<AppState>) -> ApiResult {
    let registry = ProviderRegistry::global();
    let mut configs_by_kind = std::collections::HashMap::new();
    if let Some(web) = &state.web {
        if let Ok(values) = web.config.read().and_then(provider_values) {
            for value in values {
                if let Ok(configured) = config_from_value(&value) {
                    configs_by_kind.insert(provider_kind(&configured), value);
                    let _ = registry.materialize(&configured);
                }
            }
        }
    }
    let mut providers = Vec::new();
    for provider in registry.all() {
        let available = !matches!(provider.kind(), ProviderKind::GoogleAntigravity)
            || crate::ai::provider::antigravity_oauth_configured();
        let (image_models, chat_models) = partition_image_models(provider.get_models());
        // Catalog reads are part of the settings and login UI's critical path.
        // Never make them wait for an external model-discovery service.  The
        // next catalog read observes the refreshed in-memory registry.
        if !matches!(
            provider.kind(),
            ProviderKind::GoogleAntigravity | ProviderKind::OpenAiCodex
        ) {
            let refresh_provider = std::sync::Arc::clone(&provider);
            let auth_manager = std::sync::Arc::clone(&state.auth_manager);
            // Native /models discovery needs credentials; resolve them
            // best-effort so an unauthenticated provider still refreshes
            // through the models.dev fallback.
            let config_value = configs_by_kind.get(&provider.kind()).cloned();
            tokio::spawn(async move {
                let auth = match config_value
                    .and_then(|value| config_from_value(&value).ok())
                    .map(|configured| ProviderRegistry::global().materialize(&configured))
                {
                    Some(materialized) => auth_manager.resolve(&materialized).await.ok(),
                    None => None,
                };
                if let Err(error) = refresh_provider.refresh_models(auth.as_ref()).await {
                    tracing::warn!(provider = refresh_provider.id(), %error, "provider model refresh failed");
                }
            });
        }
        providers.push(
            json!({
                "kind": provider.kind(),
                "name": provider.name(),
                "authMethod": if matches!(provider.kind(), ProviderKind::QwenTokenPlanCn) { "api_key" } else { "oauth" },
                "available": available,
                "unavailableReason": if !available {
                    Some("当前 PWCLI 版本未配置 Google Antigravity OAuth 客户端")
                } else {
                    None
                },
                "defaultModel": provider.default_model(),
                "models": chat_models,
                "imageModels": image_models,
            }),
        );
    }
    Ok(success(Value::Array(providers)))
}

async fn list(State(state): State<AppState>, Query(query): Query<ProvidersQuery>) -> ApiResult {
    migrate_legacy(&state).map_err(internal)?;
    let mut values =
        provider_values(web(&state)?.config.read().map_err(internal)?).map_err(bad_request)?;
    // Repair providers created by older builds whose browser callback stored
    // OAuth credentials but never ran dedicated model discovery.
    for value in &values {
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Ok(provider) = config_from_value(value) else {
            continue;
        };
        if provider.models.is_empty()
            && matches!(
                provider_kind(&provider),
                ProviderKind::OpenAiCodex | ProviderKind::GoogleAntigravity
            )
        {
            if let Err(error) = refresh_and_seed_provider_models(&state, id).await {
                tracing::warn!(provider_id = id, %error, "failed to repair empty provider model catalog");
            }
        }
    }
    values = provider_values(web(&state)?.config.read().map_err(internal)?).map_err(bad_request)?;
    let mut views = Vec::with_capacity(values.len());
    for value in values {
        views.push(
            provider_view(&state, &value, query.full_diff.unwrap_or(false))
                .await
                .map_err(internal)?,
        );
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
        let builtin = ProviderRegistry::global()
            .get(request.kind)
            .context("unknown built-in provider")
            .map_err(bad_request)?;
        (
            builtin.name().to_string(),
            builtin.base_url().to_string(),
            builtin.default_protocol().to_string(),
            if request.default_model.trim().is_empty() {
                builtin.default_model().to_string()
            } else {
                request.default_model.trim().to_string()
            },
            builtin.get_models(),
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
        provider_view(&state, &value, false)
            .await
            .map_err(internal)?,
    ))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PatchProviderRequest>,
) -> ApiResult {
    migrate_legacy(&state).map_err(internal)?;
    let validate_models = request.models.is_some()
        || request.image_models.is_some()
        || request.default_model.is_some();
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
            if request.models.is_some() || request.image_models.is_some() {
                // Chat and image-generation models are managed independently:
                // patching one half must never drop the other.
                let existing: Vec<Value> = provider
                    .get("models")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let existing_ids: Vec<String> = existing
                    .iter()
                    .filter_map(|value| value.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect();
                let (existing_image, existing_chat): (Vec<Value>, Vec<Value>) = existing
                    .into_iter()
                    .partition(|value| crate::runtime::image_generation::is_image_model(value));
                let mut merged = match request.models {
                    Some(models) => serde_json::to_value(&models)?
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                    None => existing_chat,
                };
                let image_models = match request.image_models {
                    Some(models) => serde_json::to_value(&models)?
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                    None => existing_image,
                };
                // Deleting a discoverable model records it as ignored, so the
                // update dialog does not immediately re-suggest it.
                let kept_ids: std::collections::HashSet<&str> = merged
                    .iter()
                    .chain(image_models.iter())
                    .filter_map(|value| value.get("id").and_then(Value::as_str))
                    .collect();
                let removed_ids: Vec<String> = existing_ids
                    .iter()
                    .filter(|id| !kept_ids.contains(id.as_str()))
                    .cloned()
                    .collect();
                if !removed_ids.is_empty() {
                    let mut ignored: Vec<String> = provider
                        .get("ignoredModelIds")
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    for id in removed_ids {
                        if !ignored.contains(&id) {
                            ignored.push(id);
                        }
                    }
                    provider["ignoredModelIds"] = json!(ignored);
                }
                merged.extend(image_models);
                provider["models"] = Value::Array(merged);
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
            if validate_models {
                validate_model_selection(provider)?;
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
        provider_view(&state, &value, false)
            .await
            .map_err(internal)?,
    ))
}

fn validate_model_selection(provider: &Value) -> Result<()> {
    let models = provider
        .get("models")
        .and_then(Value::as_array)
        .context("at least one model is required")?;
    if models.is_empty() {
        anyhow::bail!("at least one model is required");
    }
    let mut ids = std::collections::HashSet::new();
    let mut enabled_ids = std::collections::HashSet::new();
    for model in models {
        let id = model.get("id").and_then(Value::as_str).unwrap_or("").trim();
        if id.is_empty() {
            anyhow::bail!("model id cannot be empty");
        }
        if !ids.insert(id) {
            anyhow::bail!("duplicate model id: {id}");
        }
        if model.get("enabled").and_then(Value::as_bool) != Some(false) {
            enabled_ids.insert(id);
        }
    }
    if enabled_ids.is_empty() {
        anyhow::bail!("at least one model must be enabled");
    }
    let default_model = provider
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !enabled_ids.contains(default_model) {
        anyhow::bail!("default model must exist and be enabled");
    }
    Ok(())
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
    let values =
        provider_values(web(&state)?.config.read().map_err(internal)?).map_err(bad_request)?;
    let mut views = Vec::with_capacity(values.len());
    for value in values {
        views.push(
            provider_view(&state, &value, false)
                .await
                .map_err(internal)?,
        );
    }
    Ok(success(Value::Array(views)))
}

async fn test_connection(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let provider = provider_for_auth(web(&state)?, &id).map_err(bad_request)?;
    let provider = ProviderRegistry::global().materialize(&provider);
    state
        .auth_manager
        .resolve(&provider)
        .await
        .map_err(bad_request)?;
    let started = std::time::Instant::now();
    let client = crate::ai::llm::LlmClient::with_provider(
        provider.clone(),
        state.config.backend_url.clone(),
    )
    .with_auth_manager(std::sync::Arc::clone(&state.auth_manager));
    client
        .chat_with_max_tokens(
            &[crate::ai::llm::ChatMessage {
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
        if flow.state == crate::ai::provider::AuthFlowState::Connected {
            if let Err(error) = refresh_and_seed_provider_models(&state, &id).await {
                tracing::warn!(provider_id = id, %error, "provider login succeeded but model discovery failed");
            }
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
    if flow.state == crate::ai::provider::AuthFlowState::Connected {
        if let Err(error) = refresh_and_seed_provider_models(&state, &id).await {
            tracing::warn!(provider_id = id, %error, "provider login succeeded but model discovery failed");
        }
    }
    Ok(success(serde_json::to_value(flow).map_err(internal)?))
}

async fn refresh_and_seed_provider_models(state: &AppState, id: &str) -> Result<()> {
    let web_state = state.web.as_ref().context("Web service is disabled")?;
    let provider = provider_for_auth(web_state, id)?;
    let kind = provider_kind(&provider);
    let service = ProviderRegistry::global()
        .get(kind)
        .context("provider service not found")?;
    let auth = state.auth_manager.resolve(&provider).await?;
    let discovered = service.refresh_models(Some(&auth)).await?;
    if discovered.is_empty() {
        anyhow::bail!("provider model discovery returned no models");
    }
    if !provider.models.is_empty() {
        return Ok(());
    }

    let provider_id = id.to_string();
    let mut changed = false;
    web_state.config.update(|root| {
        let providers = ensure_provider_array(root)?;
        let raw = providers
            .iter_mut()
            .find(|value| value.get("id").and_then(Value::as_str) == Some(provider_id.as_str()))
            .context("provider not found")?;
        changed = seed_discovered_models(raw, &discovered)?;
        Ok(())
    })?;
    if changed {
        publish(state);
    }
    Ok(())
}

fn seed_discovered_models(
    raw: &mut Value,
    discovered: &[crate::ai::config::ModelEntry],
) -> Result<bool> {
    let configured_is_empty = raw
        .get("models")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    if !configured_is_empty || discovered.is_empty() {
        return Ok(false);
    }
    raw["models"] = serde_json::to_value(discovered)?;
    let selected = raw.get("model").and_then(Value::as_str).unwrap_or_default();
    if !discovered.iter().any(|model| model.id == selected) {
        raw["model"] = Value::String(discovered[0].id.clone());
    }
    Ok(true)
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

/// Whether a model missing from the latest discovery can be trusted as
/// "retired by the provider".  A degraded fallback source (e.g. models.dev
/// after a failed native call) may simply lack the model, so removal is
/// only reported for the provider's authoritative source.
fn removal_is_trustworthy(kind: ProviderKind, source: Option<&'static str>) -> bool {
    match kind {
        ProviderKind::KimiCoding => source == Some("models-dev"),
        ProviderKind::Xai | ProviderKind::QwenTokenPlanCn => source == Some("native"),
        ProviderKind::OpenAiCodex | ProviderKind::GoogleAntigravity => source == Some("dedicated"),
        ProviderKind::Custom => false,
    }
}

/// Image-generation models are kept out of chat pickers: the Web API exposes
/// them in a separate `imageModels` list and the gen-image tool consumes them
/// automatically.
fn is_image_entry(model: &crate::ai::config::ModelEntry) -> bool {
    model
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.image)
        == Some(true)
        || crate::ai::config::is_image_generation_model_id(&model.id)
}

/// Splits a model list into `(image_generation, chat)` halves.
fn partition_image_models(
    models: Vec<crate::ai::config::ModelEntry>,
) -> (
    Vec<crate::ai::config::ModelEntry>,
    Vec<crate::ai::config::ModelEntry>,
) {
    models.into_iter().partition(is_image_entry)
}

/// Computes the user-facing model delta for one provider:
/// `added` = discovered models absent from the user's configuration (and not
/// previously dismissed, unless `include_ignored`), `removed` = configured
/// models the provider no longer serves.  This is derived from live discovery
/// on every read, so it survives restarts and needs no separate pending-state
/// bookkeeping.
fn compute_model_changes(
    kind: ProviderKind,
    raw: &Value,
    config_models: &[crate::ai::config::ModelEntry],
    include_ignored: bool,
) -> Option<(
    Vec<crate::ai::config::ModelEntry>,
    Vec<crate::ai::config::ModelEntry>,
)> {
    let service = ProviderRegistry::global().get(kind)?;
    let discovered = service.get_models();
    if discovered.is_empty() {
        return None;
    }
    let ignored: std::collections::HashSet<&str> = if include_ignored {
        Default::default()
    } else {
        raw.get("ignoredModelIds")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    };
    let config_ids: std::collections::HashSet<&str> = config_models
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    let added = discovered
        .iter()
        .filter(|model| {
            !config_ids.contains(model.id.as_str()) && !ignored.contains(model.id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let removed = if removal_is_trustworthy(kind, service.discovery_source()) {
        let discovered_ids: std::collections::HashSet<&str> =
            discovered.iter().map(|model| model.id.as_str()).collect();
        config_models
            .iter()
            .filter(|model| !discovered_ids.contains(model.id.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    Some((added, removed))
}

fn configured_model_changes(
    state: &AppState,
    id: &str,
) -> Result<(
    Vec<crate::ai::config::ModelEntry>,
    Vec<crate::ai::config::ModelEntry>,
)> {
    let web = state.web.as_ref().context("Web service is disabled")?;
    let raw = find_provider(web, id)?;
    let kind = provider_kind_from_value(&raw);
    if kind == ProviderKind::Custom {
        anyhow::bail!("custom providers have no dynamic model catalog");
    }
    let config = config_from_value(&raw)?;
    Ok(compute_model_changes(kind, &raw, &config.models, false).unwrap_or_default())
}

/// Adopts newly discovered models into the user's configuration, enabled by
/// default.  `modelIds` selects a subset; omitting it adopts everything.
async fn adopt_model_changes(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AdoptModelChangesRequest>,
) -> ApiResult {
    let (added, _) = configured_model_changes(&state, &id).map_err(bad_request)?;
    let mut selected = match request.model_ids {
        Some(ids) => {
            let wanted: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
            added
                .into_iter()
                .filter(|model| wanted.contains(model.id.as_str()))
                .collect::<Vec<_>>()
        }
        None => added,
    };
    if selected.is_empty() {
        return Ok(success(json!({ "adopted": 0 })));
    }
    for entry in &mut selected {
        entry.enabled = Some(true);
    }
    let adopted = selected.len();
    let target_id = id.clone();
    web(&state)?
        .config
        .update(move |root| {
            let providers = ensure_provider_array(root)?;
            let provider = providers
                .iter_mut()
                .find(|value| value.get("id").and_then(Value::as_str) == Some(target_id.as_str()))
                .context("provider not found")?;
            let models = provider
                .get_mut("models")
                .and_then(Value::as_array_mut)
                .context("provider has no model list")?;
            for entry in &selected {
                if let Some(existing) = models.iter_mut().find(|value| {
                    value.get("id").and_then(Value::as_str) == Some(entry.id.as_str())
                }) {
                    existing["enabled"] = json!(true);
                } else {
                    models.push(serde_json::to_value(entry)?);
                }
            }
            // Adopting a previously dismissed model un-ignores it.
            if let Some(ignored) = provider
                .get_mut("ignoredModelIds")
                .and_then(Value::as_array_mut)
            {
                let adopted_ids: std::collections::HashSet<&str> =
                    selected.iter().map(|entry| entry.id.as_str()).collect();
                ignored.retain(|value| {
                    !value
                        .as_str()
                        .is_some_and(|model_id| adopted_ids.contains(model_id))
                });
            }
            validate_model_selection(provider)?;
            Ok(())
        })
        .map_err(bad_request)?;
    publish(&state);
    Ok(success(json!({ "adopted": adopted })))
}

/// Removes models the provider no longer serves from the user's
/// configuration.  Models are kept until the user explicitly prunes them.
async fn prune_model_changes(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let (_, removed) = configured_model_changes(&state, &id).map_err(bad_request)?;
    if removed.is_empty() {
        return Ok(success(json!({ "pruned": 0 })));
    }
    let removed_ids: std::collections::HashSet<String> =
        removed.iter().map(|entry| entry.id.clone()).collect();
    let pruned = removed_ids.len();
    let target_id = id.clone();
    web(&state)?
        .config
        .update(move |root| {
            let providers = ensure_provider_array(root)?;
            let provider = providers
                .iter_mut()
                .find(|value| value.get("id").and_then(Value::as_str) == Some(target_id.as_str()))
                .context("provider not found")?;
            let models = provider
                .get_mut("models")
                .and_then(Value::as_array_mut)
                .context("provider has no model list")?;
            models.retain(|value| {
                !value
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|model_id| removed_ids.contains(model_id))
            });
            if let Some(ignored) = provider
                .get_mut("ignoredModelIds")
                .and_then(Value::as_array_mut)
            {
                ignored.retain(|value| {
                    !value
                        .as_str()
                        .is_some_and(|model_id| removed_ids.contains(model_id))
                });
            }
            validate_model_selection(provider)?;
            Ok(())
        })
        .map_err(bad_request)?;
    publish(&state);
    Ok(success(json!({ "pruned": pruned })))
}

/// Dismisses discovery notifications by recording the affected model ids in
/// `ignoredModelIds`, so they stop appearing until discovery changes again or
/// the user adds them manually.  An empty body dismisses both halves;
/// `{"added": true}` dismisses only the newly discovered models.
async fn dismiss_model_changes(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<DismissModelChangesRequest>,
) -> ApiResult {
    let scoped = request.added.is_some() || request.removed.is_some();
    let clear_added = if scoped {
        request.added.unwrap_or(false)
    } else {
        true
    };
    let clear_removed = if scoped {
        request.removed.unwrap_or(false)
    } else {
        true
    };
    let (added, removed) = configured_model_changes(&state, &id).map_err(bad_request)?;
    let mut ignore_ids = Vec::new();
    if clear_added {
        ignore_ids.extend(added.iter().map(|entry| entry.id.clone()));
    }
    if clear_removed {
        ignore_ids.extend(removed.iter().map(|entry| entry.id.clone()));
    }
    if ignore_ids.is_empty() {
        return Ok(success(json!({ "dismissed": true })));
    }
    let target_id = id.clone();
    web(&state)?
        .config
        .update(move |root| {
            let providers = ensure_provider_array(root)?;
            let provider = providers
                .iter_mut()
                .find(|value| value.get("id").and_then(Value::as_str) == Some(target_id.as_str()))
                .context("provider not found")?;
            let existing: std::collections::HashSet<String> = provider
                .get("ignoredModelIds")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let merged = existing
                .union(
                    &ignore_ids
                        .iter()
                        .cloned()
                        .collect::<std::collections::HashSet<_>>(),
                )
                .cloned()
                .collect::<Vec<_>>();
            provider["ignoredModelIds"] = json!(merged);
            Ok(())
        })
        .map_err(bad_request)?;
    publish(&state);
    Ok(success(json!({ "dismissed": true })))
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

async fn provider_view(state: &AppState, raw: &Value, full_diff: bool) -> Result<Value> {
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .context("provider is missing id")?;
    let provider = config_from_value(raw)?;
    let kind = provider_kind(&provider);
    let mut materialized = ProviderRegistry::global().materialize(&provider);
    let auth = state.auth_manager.status(&provider).await?;
    let custom = kind == ProviderKind::Custom;
    let (image_models, chat_models) =
        partition_image_models(std::mem::take(&mut materialized.models));
    let model_changes =
        compute_model_changes(kind, raw, &provider.models, full_diff).map(|(added, removed)| {
            json!({
                "added": added,
                "removed": removed,
            })
        });
    Ok(json!({
        "id": id,
        "kind": kind,
        "name": materialized.name,
        "auth": auth,
        "models": chat_models,
        "imageModels": image_models,
        "modelChanges": model_changes,
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
            "google-antigravity" => ProviderKind::GoogleAntigravity,
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

    fn entry(id: &str, image: Option<bool>) -> crate::ai::config::ModelEntry {
        let mut entry = crate::ai::config::ModelEntry::default();
        entry.id = id.into();
        entry.name = id.into();
        if image.is_some() {
            entry.capabilities = Some(crate::ai::config::ModelCapabilities {
                image,
                ..Default::default()
            });
        }
        entry
    }

    #[test]
    fn partitions_image_models_by_capability_and_id_heuristic() {
        let models = vec![
            entry("grok-4", None),
            entry("grok-imagine-1", None),
            entry("qwen-image-2.0-pro", None),
            entry("custom-renderer", Some(true)),
        ];
        let (image, chat) = partition_image_models(models);
        let image_ids: Vec<&str> = image.iter().map(|m| m.id.as_str()).collect();
        let chat_ids: Vec<&str> = chat.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            image_ids,
            vec!["grok-imagine-1", "qwen-image-2.0-pro", "custom-renderer"]
        );
        assert_eq!(chat_ids, vec!["grok-4"]);
    }

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

    #[test]
    fn model_selection_requires_an_enabled_default() {
        assert!(validate_model_selection(&json!({
            "model": "model-a",
            "models": [
                { "id": "model-a", "name": "A", "enabled": true },
                { "id": "model-b", "name": "B", "enabled": false }
            ]
        }))
        .is_ok());
        assert!(validate_model_selection(&json!({
            "model": "model-b",
            "models": [
                { "id": "model-a", "name": "A", "enabled": true },
                { "id": "model-b", "name": "B", "enabled": false }
            ]
        }))
        .is_err());
    }

    #[test]
    fn empty_provider_is_seeded_from_authenticated_discovery() {
        let mut raw = json!({ "model": "missing-default", "models": [] });
        let discovered = vec![entry("gpt-5.5", None), entry("gpt-5.4", None)];
        assert!(seed_discovered_models(&mut raw, &discovered).unwrap());
        assert_eq!(raw["model"], "gpt-5.5");
        assert_eq!(raw["models"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn discovery_does_not_overwrite_user_managed_models() {
        let mut raw = json!({
            "model": "kept",
            "models": [{ "id": "kept", "name": "Kept", "enabled": true }]
        });
        let discovered = vec![entry("new-model", None)];
        assert!(!seed_discovered_models(&mut raw, &discovered).unwrap());
        assert_eq!(raw["model"], "kept");
        assert_eq!(raw["models"].as_array().unwrap().len(), 1);
    }
}
