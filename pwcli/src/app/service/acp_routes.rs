use std::convert::Infallible;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::{Stream, StreamExt};

use super::state::AppState;
use crate::runtime::tools::code_agent::acp_runner;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventQuery {
    #[serde(default)]
    after: u64,
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionDecision {
    option_id: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/acp/sessions", get(sessions))
        .route("/acp/sessions/{session_id}", delete(close_session))
        .route("/acp/sessions/{session_id}/cancel", post(cancel_session))
        .route("/acp/events", get(events))
        .route("/acp/permissions", get(permissions))
        .route(
            "/acp/permissions/{permission_id}/resolve",
            post(resolve_permission),
        )
}

async fn sessions() -> Json<Value> {
    Json(json!({ "sessions": acp_runner::list_sessions() }))
}

async fn permissions() -> Json<Value> {
    Json(json!({ "permissions": acp_runner::pending_permissions() }))
}

async fn resolve_permission(
    Path(permission_id): Path<String>,
    Json(decision): Json<PermissionDecision>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    acp_runner::resolve_permission(&permission_id, &decision.option_id).map_err(bad_request)?;
    Ok(Json(json!({ "success": true })))
}

async fn cancel_session(
    Path(session_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    acp_runner::cancel_session(&session_id)
        .await
        .map_err(not_found)?;
    Ok(Json(json!({ "success": true })))
}

async fn close_session(
    Path(session_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    acp_runner::close_session(&session_id)
        .await
        .map_err(not_found)?;
    Ok(Json(json!({ "success": true })))
}

async fn events(
    Query(query): Query<EventQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let history = acp_runner::events_after(query.session_id.as_deref(), query.after);
    let session_id = query.session_id;
    let history = tokio_stream::iter(history.into_iter().map(to_sse));
    let live = tokio_stream::wrappers::BroadcastStream::new(acp_runner::subscribe_events())
        .filter_map(move |event| {
            event
                .ok()
                .filter(|event| {
                    session_id
                        .as_deref()
                        .is_none_or(|session_id| event.session_id == session_id)
                })
                .map(to_sse)
        });
    Sse::new(history.chain(live)).keep_alive(KeepAlive::default())
}

fn to_sse(event: acp_runner::AcpEvent) -> Result<Event, Infallible> {
    Ok(Event::default()
        .id(event.id.to_string())
        .event(&event.kind)
        .json_data(event)
        .expect("serialize ACP event"))
}

fn bad_request(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    api_error(StatusCode::BAD_REQUEST, &error.to_string())
}

fn not_found(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    api_error(StatusCode::NOT_FOUND, &error.to_string())
}

fn api_error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "success": false, "error": message })))
}
