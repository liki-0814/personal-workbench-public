use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, Response, StatusCode};
use axum::routing::get;
use axum::Router;

use super::state::AppState;

pub struct EmbeddedAsset {
    pub bytes: &'static [u8],
    pub content_type: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/web_assets_generated.rs"));

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/{*path}", get(asset_or_spa))
}

async fn index() -> Response<Body> {
    asset_response("index.html")
}

async fn asset_or_spa(Path(path): Path<String>) -> Response<Body> {
    if path == "api" || path.starts_with("api/") {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"success":false,"error":"Not found"}"#))
            .expect("static 404 response");
    }
    let path = path.trim_start_matches('/');
    if generated_asset(path).is_some() {
        asset_response(path)
    } else if path.starts_with("assets/") {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from("Asset not found; reload the application."))
            .expect("missing asset response")
    } else {
        asset_response("index.html")
    }
}

fn asset_response(path: &str) -> Response<Body> {
    let Some(asset) = generated_asset(path) else {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from(
                "Web assets are not embedded. Run npm run build before building pwcli.",
            ))
            .expect("missing asset response");
    };
    let cache = if path == "index.html" {
        "no-cache"
    } else if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(asset.bytes))
        .expect("embedded asset response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_frontend_contains_index() {
        let asset = generated_asset("index.html").expect("run npm run build before cargo test");
        assert_eq!(asset.content_type, "text/html; charset=utf-8");
        assert!(asset.bytes.starts_with(b"<!doctype html>"));
    }

    #[tokio::test]
    async fn missing_hashed_asset_returns_not_found() {
        let response = asset_or_spa(Path("assets/old-build.js".into())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
