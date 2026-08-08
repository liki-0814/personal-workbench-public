use std::net::IpAddr;
use std::path::Path;

use anyhow::Context;
use axum::body::Body;
use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use super::super::state::AppState;
use super::WebState;
use crate::runtime::documents::{CreateDocument, RevisionConflict, UpdateDocument};

const MAX_ASSET_BYTES: usize = 20 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
struct UrlAssetRequest {
    url: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/documents", get(list).post(create))
        .route("/api/documents/import", post(import_html))
        .route("/api/documents/{id}", get(read).put(update).delete(remove))
        .route("/api/documents/{id}/evidence", get(read_evidence))
        .route("/api/documents/{id}/assets", post(upload_asset))
        .route("/api/documents/{id}/assets/url", post(fetch_asset))
        .route("/api/documents/{id}/export.html", get(export_html))
        .route("/documents/{id}/preview", get(preview_html))
        .route("/api/document-assets/{id}/{asset}", get(serve_asset))
}

fn web(state: &AppState) -> Result<&std::sync::Arc<WebState>, Box<Response<Body>>> {
    state
        .web
        .as_ref()
        .ok_or_else(|| Box::new(failure(StatusCode::NOT_FOUND, "Web service is disabled")))
}

fn data_dir(web: &WebState) -> &Path {
    web.data.path().parent().unwrap_or_else(|| Path::new("."))
}

async fn list(State(state): State<AppState>) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::list(data_dir(web)) {
        Ok(documents) => json_response(
            StatusCode::OK,
            json!({ "success": true, "documents": documents }),
        ),
        Err(error) => internal(error),
    }
}

async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateDocument>,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::create(data_dir(web), request) {
        Ok(document) => json_response(
            StatusCode::CREATED,
            json!({ "success": true, "document": document }),
        ),
        Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn read(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::read(data_dir(web), &id) {
        Ok(document) => json_response(
            StatusCode::OK,
            json!({ "success": true, "document": document }),
        ),
        Err(error) => failure(StatusCode::NOT_FOUND, &error.to_string()),
    }
}

async fn read_evidence(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::read_evidence_ledger(data_dir(web), &id) {
        Ok(evidence) => json_response(
            StatusCode::OK,
            json!({ "success": true, "evidence": evidence }),
        ),
        Err(error) => failure(StatusCode::NOT_FOUND, &error.to_string()),
    }
}

async fn update(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<UpdateDocument>,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::update(data_dir(web), &id, request) {
        Ok(document) => json_response(
            StatusCode::OK,
            json!({ "success": true, "document": document }),
        ),
        Err(error) if error.downcast_ref::<RevisionConflict>().is_some() => {
            failure(StatusCode::CONFLICT, &error.to_string())
        }
        Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn remove(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::delete(data_dir(web), &id) {
        Ok(()) => json_response(StatusCode::OK, json!({ "success": true })),
        Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn upload_asset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    mut multipart: Multipart,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    let field = match multipart.next_field().await {
        Ok(Some(field)) => field,
        Ok(None) => return failure(StatusCode::BAD_REQUEST, "asset file is required"),
        Err(error) => return failure(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let filename = field.file_name().unwrap_or("asset").to_string();
    let bytes = match field.bytes().await {
        Ok(bytes) if bytes.len() <= MAX_ASSET_BYTES => bytes,
        Ok(_) => return failure(StatusCode::PAYLOAD_TOO_LARGE, "asset exceeds 20 MiB"),
        Err(error) => return failure(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    match crate::runtime::documents::store_asset(data_dir(web), &id, &filename, &bytes) {
        Ok(path) => json_response(
            StatusCode::CREATED,
            json!({
                "success": true,
                "path": path,
                "url": format!("/api/document-assets/{id}/{}", path.trim_start_matches("assets/")),
            }),
        ),
        Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn fetch_asset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<UrlAssetRequest>,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match download_public_image(&request.url).await {
        Ok((filename, bytes)) => {
            match crate::runtime::documents::store_asset(data_dir(web), &id, &filename, &bytes) {
                Ok(path) => json_response(
                    StatusCode::CREATED,
                    json!({
                        "success": true,
                        "path": path,
                        "url": format!("/api/document-assets/{id}/{}", path.trim_start_matches("assets/")),
                    }),
                ),
                Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
            }
        }
        Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn serve_asset(
    State(state): State<AppState>,
    AxumPath((id, asset)): AxumPath<(String, String)>,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    let path = match crate::runtime::documents::resolve_asset(data_dir(web), &id, &asset) {
        Ok(path) => path,
        Err(error) => return failure(StatusCode::NOT_FOUND, &error.to_string()),
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) => return internal(error),
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(image_mime(&path)),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    response
}

async fn export_html(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::export_html(data_dir(web), &id) {
        Ok((manifest, html)) => html_response(
            html,
            Some(&manifest.title),
            manifest.runtime == crate::runtime::documents::DocumentRuntime::Archify,
        ),
        Err(error) => failure(StatusCode::NOT_FOUND, &error.to_string()),
    }
}

async fn preview_html(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    match crate::runtime::documents::export_html(data_dir(web), &id) {
        Ok((manifest, html)) => html_response(
            html,
            None,
            manifest.runtime == crate::runtime::documents::DocumentRuntime::Archify,
        ),
        Err(error) => failure(StatusCode::NOT_FOUND, &error.to_string()),
    }
}

fn html_response(
    html: String,
    download_title: Option<&str>,
    allow_archify_runtime: bool,
) -> Response<Body> {
    let mut response = Response::new(Body::from(html));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        if allow_archify_runtime {
            HeaderValue::from_static(
                "default-src 'none'; img-src data:; style-src 'unsafe-inline' https://fonts.googleapis.com; font-src data: https://fonts.gstatic.com; script-src 'unsafe-inline'; connect-src 'none'; frame-ancestors 'self'; base-uri 'none'; form-action 'none'",
            )
        } else {
            HeaderValue::from_static(
                "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:",
            )
        },
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(title) = download_title {
        let filename = format!("{}.html", safe_download_name(title));
        if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
    }
    response
}

async fn import_html(State(state): State<AppState>, mut multipart: Multipart) -> Response<Body> {
    let web = match web(&state) {
        Ok(web) => web,
        Err(response) => return *response,
    };
    let field = match multipart.next_field().await {
        Ok(Some(field)) => field,
        Ok(None) => return failure(StatusCode::BAD_REQUEST, "HTML file is required"),
        Err(error) => return failure(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let title = field
        .file_name()
        .unwrap_or("Imported HTML")
        .trim_end_matches(".html")
        .trim_end_matches(".htm")
        .trim();
    let title = if title.is_empty() {
        "Imported HTML".to_owned()
    } else {
        title.to_owned()
    };
    let bytes = match field.bytes().await {
        Ok(bytes) if bytes.len() <= MAX_HTML_BYTES => bytes,
        Ok(_) => return failure(StatusCode::PAYLOAD_TOO_LARGE, "HTML file exceeds 64 MiB"),
        Err(error) => return failure(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    match crate::runtime::documents::import_html(data_dir(web), &title, &bytes) {
        Ok(document) => json_response(
            StatusCode::CREATED,
            json!({ "success": true, "document": document }),
        ),
        Err(error) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn download_public_image(input: &str) -> anyhow::Result<(String, Vec<u8>)> {
    let mut url = reqwest::Url::parse(input)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    for _ in 0..5 {
        ensure_public_url(&url).await?;
        let response = client.get(url.clone()).send().await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .context("image redirect is missing Location")?;
            url = url.join(location)?;
            continue;
        }
        if !response.status().is_success() {
            anyhow::bail!("image request failed with {}", response.status());
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(
            content_type.split(';').next(),
            Some("image/png" | "image/jpeg" | "image/webp")
        ) {
            anyhow::bail!("URL must return a PNG, JPEG or WebP image");
        }
        if response.content_length().unwrap_or_default() > MAX_ASSET_BYTES as u64 {
            anyhow::bail!("remote image exceeds 20 MiB");
        }
        let extension = match content_type.split(';').next() {
            Some("image/png") => "png",
            Some("image/webp") => "webp",
            _ => "jpg",
        };
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if bytes.len().saturating_add(chunk.len()) > MAX_ASSET_BYTES {
                anyhow::bail!("remote image exceeds 20 MiB");
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((format!("remote.{extension}"), bytes));
    }
    anyhow::bail!("too many image redirects")
}

async fn ensure_public_url(url: &reqwest::Url) -> anyhow::Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("only HTTP and HTTPS image URLs are supported");
    }
    let host = url.host_str().context("image URL has no host")?;
    let port = url
        .port_or_known_default()
        .context("image URL has no port")?;
    for address in tokio::net::lookup_host((host, port)).await? {
        if !is_public_ip(address.ip()) {
            anyhow::bail!("image URL resolves to a private or local address");
        }
    }
    Ok(())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified())
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local())
        }
    }
}

fn safe_download_name(title: &str) -> String {
    let filtered = title
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = filtered.trim_matches('-');
    if trimmed.is_empty() {
        "document".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn image_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
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

fn failure(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, json!({ "success": false, "error": message }))
}

fn internal(error: impl std::fmt::Display) -> Response<Body> {
    failure(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses_are_rejected() {
        assert!(!is_public_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("10.0.0.1".parse().unwrap()));
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
    }
}
