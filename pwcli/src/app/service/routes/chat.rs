use axum::routing::{get, patch, post};
use axum::Router;

use super::super::state::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/sessions",
            post(super::create_session).get(super::list_sessions),
        )
        .route(
            "/sessions/{id}",
            get(super::get_session).delete(super::delete_session),
        )
        .route("/sessions/{id}/snapshot", get(super::get_session_snapshot))
        .route("/sessions/{id}/chat", post(super::chat))
        .route("/sessions/{id}/stream", post(super::stream_chat))
        .route(
            "/sessions/{id}/compact",
            post(super::compact_session_context),
        )
        .route("/sessions/{id}/branches", get(super::list_session_branches))
        .route("/sessions/{id}/branch", post(super::switch_session_branch))
        .route("/sessions/{id}/control", post(super::control_harness))
        .route("/sessions/{id}/inputs", post(super::submit_session_input))
        .route("/sessions/{id}/runtime", get(super::get_session_runtime))
        .route("/sessions/{id}/events", get(super::session_runtime_events))
        .route(
            "/sessions/{id}/relocate-workspace",
            post(super::relocate_session_workspace),
        )
        .route(
            "/sessions/{id}/queue/{item_id}",
            patch(super::patch_queue_item).delete(super::delete_queue_item),
        )
        .route(
            "/sessions/{id}/harness-events",
            get(super::replay_harness_events),
        )
        .route("/tools", get(super::list_tools))
        .route("/skills", get(super::list_skills))
        .route("/resources/reload", post(super::reload_resources))
        .route("/backends", get(super::list_backends))
}
