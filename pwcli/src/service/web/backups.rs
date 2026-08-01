use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::state::AppState;
use super::WebState;

#[derive(Deserialize)]
struct SnapshotRequest {
    #[serde(default)]
    label: String,
}

#[derive(Deserialize)]
struct RestoreRequest {
    filename: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/backups", get(list))
        .route("/api/backups/snapshot", post(snapshot))
        .route("/api/backups/restore", post(restore))
}

async fn list(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let web = web(&state)?;
    let data = list_backups(&directory(web)).map_err(internal)?;
    Ok(Json(json!({ "success": true, "data": data })))
}

async fn snapshot(
    State(state): State<AppState>,
    Json(request): Json<SnapshotRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let web = web(&state)?;
    let label = request
        .label
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "_-".contains(*character))
        .take(32)
        .collect::<String>();
    let filename = format!(
        "manual-{}{}.db",
        Utc::now().format("%Y-%m-%dT%H-%M-%S"),
        if label.is_empty() {
            String::new()
        } else {
            format!("-{label}")
        }
    );
    let target = directory(web).join(&filename);
    fs::create_dir_all(target.parent().expect("backup parent")).map_err(internal)?;
    web.data.backup_to(&target).map_err(internal)?;
    Ok(Json(json!({ "success": true, "file": target })))
}

async fn restore(
    State(state): State<AppState>,
    Json(request): Json<RestoreRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let web = web(&state)?;
    if !valid_filename(&request.filename) {
        return Err(error(StatusCode::BAD_REQUEST, "Invalid filename"));
    }
    let source = directory(web).join(&request.filename);
    if !source.is_file() {
        return Err(error(
            StatusCode::NOT_FOUND,
            &format!("Backup not found: {}", request.filename),
        ));
    }
    fs::create_dir_all(directory(web)).map_err(internal)?;
    let safety = format!("pre-restore-{}.db", Utc::now().format("%Y-%m-%dT%H-%M-%S"));
    web.data
        .backup_to(&directory(web).join(&safety))
        .map_err(internal)?;
    web.data.restore_from(&source).map_err(internal)?;
    web.events.publish(vec!["*".to_string()]);
    Ok(Json(json!({
        "success": true,
        "restored": request.filename,
        "safetySnapshot": safety,
        "note": "Database restored in place"
    })))
}

fn list_backups(directory: &Path) -> Result<Vec<Value>> {
    fs::create_dir_all(directory)?;
    let mut backups = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("db"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let modified = metadata.modified().ok()?;
            let filename = entry.file_name().to_string_lossy().into_owned();
            let kind = if filename == "latest.db" {
                "periodic"
            } else if filename.starts_with("pre-write-") || filename.starts_with("pre-restore-") {
                "pre-write"
            } else {
                "manual"
            };
            Some((
                modified,
                json!({
                    "filename": filename,
                    "kind": kind,
                    "size": metadata.len(),
                    "mtime": DateTime::<Utc>::from(modified),
                }),
            ))
        })
        .collect::<Vec<_>>();
    backups.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(backups.into_iter().map(|(_, value)| value).collect())
}

fn directory(web: &WebState) -> PathBuf {
    web.data
        .path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups")
}

fn valid_filename(filename: &str) -> bool {
    !filename.is_empty()
        && filename.ends_with(".db")
        && !filename.contains("..")
        && !filename.contains('/')
        && !filename.contains('\\')
}

fn web(state: &AppState) -> Result<&std::sync::Arc<WebState>, (StatusCode, Json<Value>)> {
    state
        .web
        .as_ref()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Web service is disabled"))
}

fn internal(cause: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    error(StatusCode::INTERNAL_SERVER_ERROR, &cause.to_string())
}

fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "success": false, "error": message })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_backup_path_traversal() {
        assert!(valid_filename("manual-one.db"));
        assert!(!valid_filename("../data.db"));
    }

    #[test]
    fn snapshot_round_trip_restores_data() {
        let root = tempfile::tempdir().unwrap();
        let store = super::super::DataStore::open(root.path().join("data.db")).unwrap();
        store.set_raw("key", "\"before\"").unwrap();
        let snapshot = root.path().join("backup.db");
        store.backup_to(&snapshot).unwrap();
        store.set_raw("key", "\"after\"").unwrap();
        store.restore_from(&snapshot).unwrap();
        assert_eq!(store.get_raw("key").unwrap().as_deref(), Some("\"before\""));
    }
}
