use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use super::super::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/illustration/runs/{id}", get(run))
        .route("/api/illustration/references/status", get(reference_status))
        .route("/api/illustration/references", get(personal_references))
        .route(
            "/api/illustration/references/starter/reset",
            post(reset_starter),
        )
        .route("/api/illustration/references/{id}", delete(remove_personal))
}

async fn run(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let data_dir = data_dir(&state)?;
    let run = crate::runtime::illustration::IllustrationRunStore::new(&data_dir)
        .load(&id)
        .await
        .map_err(not_found)?;
    Ok(Json(json!({ "success": true, "data": run })))
}

async fn reference_status(State(state): State<AppState>) -> ApiResult {
    let store = crate::runtime::illustration::ReferenceStore::new(&data_dir(&state)?);
    Ok(Json(json!({
        "success": true,
        "data": {
            "references": store.status().map_err(internal)?,
            "python": crate::runtime::illustration::python_env::status()
        }
    })))
}

async fn personal_references(State(state): State<AppState>) -> ApiResult {
    let store = crate::runtime::illustration::ReferenceStore::new(&data_dir(&state)?);
    Ok(Json(
        json!({ "success": true, "data": store.list_personal().map_err(internal)? }),
    ))
}

async fn reset_starter(State(state): State<AppState>) -> ApiResult {
    let store = crate::runtime::illustration::ReferenceStore::new(&data_dir(&state)?);
    store.reset_starter().map_err(internal)?;
    Ok(Json(json!({ "success": true })))
}

async fn remove_personal(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let store = crate::runtime::illustration::ReferenceStore::new(&data_dir(&state)?);
    store.remove_personal(&id).map_err(not_found)?;
    Ok(Json(json!({ "success": true })))
}

fn data_dir(state: &AppState) -> Result<std::path::PathBuf, (StatusCode, Json<Value>)> {
    state
        .web
        .as_ref()
        .and_then(|web| web.data.path().parent().map(std::path::Path::to_path_buf))
        .ok_or_else(|| internal(anyhow::anyhow!("Web data store is unavailable")))
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn internal(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "success": false, "error": error.to_string() })),
    )
}
fn not_found(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "success": false, "error": error.to_string() })),
    )
}
