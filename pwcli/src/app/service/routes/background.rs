use axum::routing::{delete, get, post};
use axum::Router;

use super::super::state::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/sessions/{id}/background-events",
            get(super::background_events),
        )
        .route("/background-events", get(super::background_events_global))
        .route("/background-tasks", get(super::list_background_tasks))
        .route(
            "/background-tasks/{id}/retry",
            post(super::retry_background_task),
        )
        .route(
            "/background-tasks/{id}",
            delete(super::cancel_background_task),
        )
        .route(
            "/sessions/{id}/background-tasks",
            delete(super::cancel_session_background_tasks),
        )
        .route(
            "/sessions/{id}/code-agent/decision",
            post(super::resolve_code_agent_decision),
        )
}
