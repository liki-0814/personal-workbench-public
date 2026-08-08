use std::path::{Path as FilePath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use super::super::state::AppState;

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Deserialize)]
struct UploadRequest {
    data: String,
    filename: Option<String>,
}

#[derive(Deserialize)]
struct FetchRequest {
    url: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/images/upload", post(upload))
        .route("/api/images/fetch", post(fetch_remote))
        .route("/api/images/{filename}", get(serve))
        .route(
            "/api/image-artifacts/{session_id}/{artifact_id}",
            get(serve_artifact),
        )
        .route(
            "/api/image-artifacts/sessions/{session_id}",
            delete(delete_session_artifacts),
        )
}

async fn delete_session_artifacts(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response<Body> {
    let Some(web) = state.web.as_ref() else {
        return error(StatusCode::NOT_FOUND, "Web service is disabled");
    };
    let data_dir = web
        .data
        .path()
        .parent()
        .unwrap_or_else(|| FilePath::new("."));
    match crate::runtime::image_generation::delete_session_artifacts(data_dir, &session_id).await {
        Ok(()) => json_response(StatusCode::OK, json!({ "success": true })),
        Err(cause) => error(StatusCode::BAD_REQUEST, &cause.to_string()),
    }
}

async fn serve_artifact(
    State(state): State<AppState>,
    Path((session_id, artifact_id)): Path<(String, String)>,
) -> Response<Body> {
    let Some(web) = state.web.as_ref() else {
        return error(StatusCode::NOT_FOUND, "Web service is disabled");
    };
    let data_dir = web
        .data
        .path()
        .parent()
        .unwrap_or_else(|| FilePath::new("."));
    let path = match crate::runtime::image_generation::artifact_path(
        data_dir,
        &session_id,
        &artifact_id,
    ) {
        Ok(path) => path,
        Err(_) => return error(StatusCode::NOT_FOUND, "Image artifact not found"),
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return error(StatusCode::NOT_FOUND, "Image artifact not found")
        }
        Err(cause) => return internal(cause),
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(mime_for(&path)),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

async fn fetch_remote(Json(request): Json<FetchRequest>) -> Response<Body> {
    match crate::runtime::media::fetch_remote_image(&request.url).await {
        Ok(image) => {
            let mut response = Response::new(Body::from(image.bytes));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(image.mime));
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(cause) => error(cause.status, &cause.message),
    }
}

async fn upload(
    State(state): State<AppState>,
    Json(request): Json<UploadRequest>,
) -> Response<Body> {
    if request.data.trim().is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "Missing or invalid \"data\" field (expected base64 string)",
        );
    }
    let extension = extension_from_data_uri(&request.data);
    let raw = request
        .data
        .split_once(";base64,")
        .map(|(_, value)| value)
        .unwrap_or(&request.data);
    let bytes = match base64::engine::general_purpose::STANDARD.decode(raw) {
        Ok(bytes) if bytes.len() <= MAX_IMAGE_BYTES => bytes,
        Ok(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "Image exceeds 20MB limit"),
        Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid base64 image data"),
    };
    let suffix = request
        .filename
        .as_deref()
        .map(sanitize_filename)
        .filter(|value| !value.is_empty())
        .map(|value| format!("_{value}"))
        .unwrap_or_default();
    let filename = format!("{}{}{}", uuid::Uuid::new_v4(), suffix, extension);
    let directory = match images_dir(&state) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    if let Err(cause) = tokio::fs::create_dir_all(&directory).await {
        return internal(cause);
    }
    if let Err(cause) = tokio::fs::write(directory.join(&filename), bytes).await {
        return internal(cause);
    }
    json_response(
        StatusCode::OK,
        json!({ "url": format!("/api/images/{filename}") }),
    )
}

async fn serve(State(state): State<AppState>, Path(filename): Path<String>) -> Response<Body> {
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return error(StatusCode::BAD_REQUEST, "Invalid filename");
    }
    let directory = match images_dir(&state) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    let path = directory.join(&filename);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return error(StatusCode::NOT_FOUND, "Image not found");
        }
        Err(cause) => return internal(cause),
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(mime_for(&path)),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

fn images_dir(state: &AppState) -> Result<PathBuf, Box<Response<Body>>> {
    state
        .web
        .as_ref()
        .map(|web| {
            web.data
                .path()
                .parent()
                .unwrap_or_else(|| FilePath::new("."))
                .join("images")
        })
        .ok_or_else(|| Box::new(error(StatusCode::NOT_FOUND, "Web service is disabled")))
}

fn extension_from_data_uri(value: &str) -> &'static str {
    let mime = value
        .strip_prefix("data:image/")
        .and_then(|value| value.split_once(';').map(|(mime, _)| mime))
        .unwrap_or_default();
    match mime.to_ascii_lowercase().as_str() {
        "jpeg" => ".jpg",
        "gif" => ".gif",
        "webp" => ".webp",
        "svg+xml" => ".svg",
        _ => ".png",
    }
}

fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "._-".contains(*character))
        .take(128)
        .collect()
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
        _ => "application/octet-stream",
    }
}

fn error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, json!({ "error": message }))
}

fn internal(cause: impl std::fmt::Display) -> Response<Body> {
    error(StatusCode::INTERNAL_SERVER_ERROR, &cause.to_string())
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response<Body> {
    let mut response = Response::new(Body::from(value.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names_and_detects_data_uri_types() {
        assert_eq!(sanitize_filename("../my image?.png"), "..myimage.png");
        assert_eq!(
            extension_from_data_uri("data:image/jpeg;base64,AA=="),
            ".jpg"
        );
    }
}
