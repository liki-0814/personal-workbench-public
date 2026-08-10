use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use futures::TryStreamExt;
use serde_json::{json, Value};

use super::super::state::AppState;
use super::config::ProviderEndpoint;
use crate::runtime::image_generation::{ImageGenerationRequest, ImageGenerationService};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/proxy/openai", post(openai))
        .route("/api/proxy/anthropic", post(anthropic))
        .route("/api/proxy/generate-image", post(generate_image))
}

async fn openai(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    forward(&state, &headers, "chat/completions", body, false).await
}

async fn anthropic(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    forward(&state, &headers, "v1/messages", body, true).await
}

async fn forward(
    state: &AppState,
    headers: &HeaderMap,
    endpoint: &str,
    body: Value,
    anthropic: bool,
) -> Response<Body> {
    let provider = match selected_provider(state, headers) {
        Ok(provider) if !provider.base_url.is_empty() && !provider.api_key.is_empty() => provider,
        Ok(_) => return error(StatusCode::BAD_REQUEST, "Provider URL or API key is empty"),
        Err(response) => return *response,
    };
    let request = reqwest::Client::new()
        .post(format!("{}/{endpoint}", provider.base_url))
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body);
    let request = if anthropic {
        request
            .header("x-api-key", provider.api_key)
            .header("anthropic-version", "2023-06-01")
    } else {
        request.bearer_auth(provider.api_key)
    };
    let request = if let Some(user_agent) = provider.user_agent.as_deref() {
        request.header(header::USER_AGENT, user_agent)
    } else {
        request
    };
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(cause) => return error(StatusCode::BAD_GATEWAY, &cause.to_string()),
    };
    let status = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let cache_control = upstream.headers().get(header::CACHE_CONTROL).cloned();
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    if let Some(value) = content_type {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Some(value) = cache_control {
        response.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    response
}

async fn generate_image(
    State(state): State<AppState>,
    Json(request): Json<ImageGenerationRequest>,
) -> Response<Body> {
    let Some(web) = state.web.as_ref() else {
        return error(StatusCode::NOT_FOUND, "Web service is disabled");
    };
    let data_dir = web
        .data
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    match ImageGenerationService::new(data_dir)
        .generate(&request, None)
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => json_response(StatusCode::OK, value),
            Err(cause) => error(StatusCode::INTERNAL_SERVER_ERROR, &cause.to_string()),
        },
        Err(cause) => {
            let message = cause.to_string();
            let status = if message.contains("upstream")
                || message.contains("provider response")
                || message.contains("Unable to")
            {
                StatusCode::BAD_GATEWAY
            } else {
                StatusCode::BAD_REQUEST
            };
            error(status, &message)
        }
    }
}

fn selected_provider(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ProviderEndpoint, Box<Response<Body>>> {
    let index = headers
        .get("x-provider-index")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    state
        .web
        .as_ref()
        .ok_or_else(|| Box::new(error(StatusCode::NOT_FOUND, "Web service is disabled")))?
        .config
        .provider(index)
        .map_err(|cause| Box::new(error(StatusCode::BAD_REQUEST, &cause.to_string())))
}

fn error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, json!({ "error": message }))
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
