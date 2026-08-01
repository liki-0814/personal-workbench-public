use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;

use super::super::state::AppState;

type ApiError = (StatusCode, String);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttentionQuery {
    #[serde(default)]
    include_resolved: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAttentionRequest {
    status: String,
    #[serde(default)]
    expected_revision: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyResolveRequest {
    #[serde(default)]
    expected_version: Option<u32>,
    #[serde(default)]
    resolution: serde_json::Value,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/attention", get(list))
        .route("/attention/{id}", patch(update).delete(dismiss))
        .route("/attention/{id}/resolve", post(resolve_legacy))
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<AttentionQuery>,
) -> Result<Json<Vec<crate::task::RuntimeAttention>>, ApiError> {
    let mut items = state.task_broker.list_attention().map_err(internal_error)?;
    if !query.include_resolved {
        items.retain(|item| item.status != "resolved");
    }
    Ok(Json(items))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateAttentionRequest>,
) -> Result<Json<crate::task::RuntimeAttention>, ApiError> {
    state
        .task_broker
        .update_attention_status(&id, &request.status, request.expected_revision)
        .map(Json)
        .map_err(mutation_error)
}

async fn dismiss(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::task::RuntimeAttention>, ApiError> {
    state
        .task_broker
        .dismiss_attention(&id)
        .map(Json)
        .map_err(mutation_error)
}

async fn resolve_legacy(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<LegacyResolveRequest>,
) -> Result<Json<crate::task::RuntimeAttention>, ApiError> {
    let _resolution = request.resolution;
    state
        .task_broker
        .update_attention_status(&id, "resolved", request.expected_version)
        .map(Json)
        .map_err(mutation_error)
}

fn mutation_error(error: anyhow::Error) -> ApiError {
    if is_database_busy(&error) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Runtime task database is busy; please retry".to_string(),
        );
    }
    let message = error.to_string();
    let status = if message.contains("not found") {
        StatusCode::NOT_FOUND
    } else if message.contains("revision changed") {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, message)
}

fn is_database_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|sqlite| {
                matches!(
                    sqlite,
                    rusqlite::Error::SqliteFailure(details, _)
                        if matches!(
                            details.code,
                            rusqlite::ErrorCode::DatabaseBusy
                                | rusqlite::ErrorCode::DatabaseLocked
                        )
                )
            })
    })
}

fn internal_error(error: anyhow::Error) -> ApiError {
    tracing::error!(%error, "failed to read RuntimeTask attention");
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
