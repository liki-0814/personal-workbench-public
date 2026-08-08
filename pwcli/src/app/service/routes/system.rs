use axum::routing::{get, post};
use axum::Router;

use super::super::state::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(super::health))
        .route("/daemon/status", get(super::daemon_status))
        .route("/daemon/shutdown", post(super::daemon_shutdown))
}
