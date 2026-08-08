use std::path::{Path as FilePath, PathBuf};

use axum::body::Body;
use axum::extract::Query;
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::state::AppState;
use crate::runtime::tools::fs_local::{list_directory, search_files, FsSandbox};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
    raw: Option<String>,
}

#[derive(Deserialize)]
struct SearchQuery {
    path: Option<String>,
    q: Option<String>,
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedDirectory {
    canonical_path: String,
    display_path: String,
    fs_base: String,
    readable: bool,
}

#[derive(Deserialize)]
struct WriteBody {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct RenameBody {
    from: String,
    to: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/fs/list", get(list))
        .route("/api/fs/read", get(read))
        .route("/api/fs/search", get(search))
        .route("/api/fs/info", get(info))
        .route("/api/fs/resolve-directory", post(resolve_directory))
        .route("/api/fs/write", post(write))
        .route("/api/fs/mkdir", post(mkdir))
        .route("/api/fs/rename", post(rename))
        .route("/api/fs/delete", delete(remove))
}

async fn resolve_directory(Json(body): Json<PathBody>) -> Response<Body> {
    if body.path.trim().is_empty() {
        return failure(StatusCode::BAD_REQUEST, "path is required");
    }
    let sandbox = match FsSandbox::from_config() {
        Ok(sandbox) => sandbox,
        Err(error) => return failure(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    match sandbox.resolve_existing_directory(&body.path) {
        Ok(path) => fs_json(
            StatusCode::OK,
            serde_json::to_value(ResolvedDirectory {
                canonical_path: path.to_string_lossy().into_owned(),
                display_path: body.path,
                fs_base: sandbox.root().to_string_lossy().into_owned(),
                readable: std::fs::read_dir(&path).is_ok(),
            })
            .unwrap_or_else(|_| json!({ "success": false })),
        ),
        Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn list(Query(query): Query<PathQuery>) -> Response<Body> {
    let target = match resolve(query.path.as_deref().unwrap_or(".")) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    match list_directory(&target).await {
        Ok(entries) => fs_json(
            StatusCode::OK,
            json!({
                "success": true,
                "path": target,
                "entries": entries.into_iter().map(|entry| json!({
                    "name": entry.name,
                    "type": if entry.is_dir { "directory" } else { "file" },
                    "size": entry.size,
                })).collect::<Vec<_>>()
            }),
        ),
        Err(error) => internal(error),
    }
}

async fn read(Query(query): Query<PathQuery>) -> Response<Body> {
    let target = match resolve(query.path.as_deref().unwrap_or(".")) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    let metadata = match tokio::fs::metadata(&target).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return failure(StatusCode::BAD_REQUEST, "Path is not a file"),
        Err(error) => return internal(error),
    };
    if metadata.len() > MAX_FILE_BYTES {
        return failure(StatusCode::BAD_REQUEST, "File exceeds 10MB limit");
    }
    let bytes = match tokio::fs::read(&target).await {
        Ok(bytes) => bytes,
        Err(error) => return internal(error),
    };
    if query.raw.as_deref() == Some("1") {
        let mut response = Response::new(Body::from(bytes));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(mime_for(&target)),
        );
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        );
        set_fs_header(&mut response);
        return response;
    }
    if bytes.contains(&0) {
        return fs_json(
            StatusCode::OK,
            json!({
                "success": true,
                "isBinary": true,
                "meta": {
                    "size": metadata.len(),
                    "type": target.extension().and_then(|value| value.to_str()).unwrap_or("unknown")
                }
            }),
        );
    }
    match String::from_utf8(bytes) {
        Ok(content) => fs_json(
            StatusCode::OK,
            json!({ "success": true, "content": content, "isBinary": false }),
        ),
        Err(_) => fs_json(
            StatusCode::OK,
            json!({
                "success": true,
                "isBinary": true,
                "meta": { "size": metadata.len(), "type": "unknown" }
            }),
        ),
    }
}

async fn search(Query(query): Query<SearchQuery>) -> Response<Body> {
    let target = match resolve(query.path.as_deref().unwrap_or(".")) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    let needle = query.q.as_deref().unwrap_or("*").trim_matches('*');
    match search_files(&target, needle).await {
        Ok(files) => fs_json(
            StatusCode::OK,
            json!({ "success": true, "files": files.into_iter().take(100).collect::<Vec<_>>() }),
        ),
        Err(error) => internal(error),
    }
}

async fn info(Query(query): Query<PathQuery>) -> Response<Body> {
    let target = match resolve(query.path.as_deref().unwrap_or(".")) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    match tokio::fs::metadata(&target).await {
        Ok(metadata) => {
            let modified = metadata.modified().ok().map(DateTime::<Utc>::from);
            let created = metadata.created().ok().map(DateTime::<Utc>::from);
            fs_json(
                StatusCode::OK,
                json!({
                    "success": true,
                    "name": target.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
                    "type": if metadata.is_dir() { "directory" } else { "file" },
                    "size": metadata.len(),
                    "modified": modified,
                    "created": created,
                }),
            )
        }
        Err(error) => internal(error),
    }
}

async fn write(Json(body): Json<WriteBody>) -> Response<Body> {
    if body.path.trim().is_empty() {
        return failure(StatusCode::BAD_REQUEST, "path and content are required");
    }
    if body.content.len() as u64 > MAX_FILE_BYTES {
        return failure(StatusCode::BAD_REQUEST, "Content exceeds 10MB limit");
    }
    let target = match resolve(&body.path) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    if let Some(parent) = target.parent() {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return internal(error);
        }
    }
    match tokio::fs::write(&target, body.content).await {
        Ok(()) => fs_json(StatusCode::OK, json!({ "success": true, "path": target })),
        Err(error) => internal(error),
    }
}

async fn mkdir(Json(body): Json<PathBody>) -> Response<Body> {
    let target = match resolve_required(&body.path) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    match tokio::fs::create_dir_all(&target).await {
        Ok(()) => fs_json(StatusCode::OK, json!({ "success": true, "path": target })),
        Err(error) => internal(error),
    }
}

async fn rename(Json(body): Json<RenameBody>) -> Response<Body> {
    let from = match resolve_required(&body.from) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    let to = match resolve_required(&body.to) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    if let Some(parent) = to.parent() {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return internal(error);
        }
    }
    match tokio::fs::rename(&from, &to).await {
        Ok(()) => fs_json(
            StatusCode::OK,
            json!({ "success": true, "from": from, "to": to }),
        ),
        Err(error) => internal(error),
    }
}

async fn remove(Json(body): Json<PathBody>) -> Response<Body> {
    let target = match resolve_required(&body.path) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    let result = match tokio::fs::metadata(&target).await {
        Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir(&target).await,
        Ok(_) => tokio::fs::remove_file(&target).await,
        Err(error) => return internal(error),
    };
    match result {
        Ok(()) => fs_json(StatusCode::OK, json!({ "success": true, "path": target })),
        Err(error) => internal(error),
    }
}

fn resolve_required(input: &str) -> Result<PathBuf, Box<Response<Body>>> {
    if input.trim().is_empty() {
        return Err(Box::new(failure(
            StatusCode::BAD_REQUEST,
            "path is required",
        )));
    }
    resolve(input)
}

fn resolve(input: &str) -> Result<PathBuf, Box<Response<Body>>> {
    FsSandbox::from_config()
        .and_then(|sandbox| sandbox.resolve(input))
        .map_err(|_| {
            Box::new(failure(
                StatusCode::FORBIDDEN,
                "Access denied: path outside allowed directory",
            ))
        })
}

fn fs_json(status: StatusCode, value: Value) -> Response<Body> {
    let mut response = json_response(status, value);
    set_fs_header(&mut response);
    response
}

fn failure(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, json!({ "success": false, "error": message }))
}

fn internal(error: impl std::fmt::Display) -> Response<Body> {
    failure(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
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

fn set_fs_header(response: &mut Response<Body>) {
    response
        .headers_mut()
        .insert("x-fs-access", HeaderValue::from_static("true"));
}

fn mime_for(path: &FilePath) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_types_cover_markdown_assets() {
        assert_eq!(mime_for(FilePath::new("image.png")), "image/png");
        assert_eq!(
            mime_for(FilePath::new("file.bin")),
            "application/octet-stream"
        );
    }
}
