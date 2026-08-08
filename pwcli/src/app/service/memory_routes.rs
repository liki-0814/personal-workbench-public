use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::runtime::memory::{
    collect_stats_with_preread, resolve_slug, sync_index, validate_slug, MemoryEntry, MemoryError,
    MemoryIndexLine, MemoryStats, MemoryStore,
};

use super::state::AppState;

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Serialize)]
struct ContentResponse {
    content: String,
}

#[derive(Deserialize)]
struct ContentBody {
    content: String,
}

#[derive(Serialize)]
struct IndexEntrySummary {
    slug: String,
    summary: String,
    updated_at: i64,
}

#[derive(Serialize)]
struct StatsJson {
    schema_version: u32,
    active_entries: usize,
    soft_deleted_entries: usize,
    archived_entries: usize,
    index_bytes: usize,
    profile_bytes: usize,
    index_lines: usize,
    vector_indexed: Option<usize>,
    has_profile: bool,
}

impl From<MemoryStats> for StatsJson {
    fn from(s: MemoryStats) -> Self {
        Self {
            schema_version: s.schema_version,
            active_entries: s.active_entries,
            soft_deleted_entries: s.soft_deleted_entries,
            archived_entries: s.archived_entries,
            index_bytes: s.index_bytes,
            profile_bytes: s.profile_bytes,
            index_lines: s.index_lines,
            vector_indexed: s.vector_indexed,
            has_profile: s.has_profile,
        }
    }
}

#[derive(Serialize)]
struct OverviewResponse {
    user_slug: String,
    profile: String,
    index_raw: String,
    entries: Vec<IndexEntrySummary>,
    stats: StatsJson,
}

#[derive(Deserialize)]
struct UpsertEntryBody {
    summary: String,
    content: String,
}

#[derive(Deserialize)]
struct CreateEntryBody {
    slug: String,
    summary: String,
    content: String,
}

#[derive(Serialize)]
struct CreateEntryResponse {
    slug: String,
    requested_slug: String,
    entry: MemoryEntry,
}

fn user_slug(state: &AppState) -> String {
    state
        .config
        .user
        .as_ref()
        .and_then(|u| u.slug.clone())
        .unwrap_or_else(|| "local".to_string())
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ErrorBody>) {
    (status, Json(ErrorBody { error: msg.into() }))
}

fn map_memory_error(e: MemoryError) -> (StatusCode, Json<ErrorBody>) {
    match &e {
        MemoryError::SummaryTooLong { .. }
        | MemoryError::EntryTooLarge { .. }
        | MemoryError::IndexFull { .. }
        | MemoryError::ProfileTooLarge { .. } => err(StatusCode::BAD_REQUEST, e.to_string()),
        MemoryError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            err(StatusCode::NOT_FOUND, io.to_string())
        }
        MemoryError::Io(_) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn open_store(slug: &str) -> Result<MemoryStore, (StatusCode, Json<ErrorBody>)> {
    MemoryStore::new(slug).map_err(map_memory_error)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/memory/overview", get(get_overview))
        .route("/memory/profile", get(get_profile).put(put_profile))
        .route("/memory/index", get(get_index).put(put_index))
        .route("/memory/entries", post(create_entry))
        .route(
            "/memory/entries/{slug}",
            get(get_entry).put(put_entry).delete(delete_entry),
        )
}

async fn get_overview(
    State(state): State<AppState>,
) -> Result<Json<OverviewResponse>, (StatusCode, Json<ErrorBody>)> {
    let slug = user_slug(&state);
    let store = open_store(&slug)?;
    let profile = store.read_profile().map_err(map_memory_error)?;
    let index_raw = store.read_index_raw().map_err(map_memory_error)?;
    let idx = store.read_index().map_err(map_memory_error)?;
    let entries: Vec<IndexEntrySummary> = idx
        .entries
        .into_iter()
        .map(|line| IndexEntrySummary {
            slug: line.slug,
            summary: line.summary,
            updated_at: line.updated_at,
        })
        .collect();
    let stats =
        collect_stats_with_preread(&store, &index_raw, &profile).map_err(map_memory_error)?;
    Ok(Json(OverviewResponse {
        user_slug: slug,
        profile,
        index_raw,
        entries,
        stats: stats.into(),
    }))
}

async fn get_profile(
    State(state): State<AppState>,
) -> Result<Json<ContentResponse>, (StatusCode, Json<ErrorBody>)> {
    let store = open_store(&user_slug(&state))?;
    let content = store.read_profile().map_err(map_memory_error)?;
    Ok(Json(ContentResponse { content }))
}

async fn put_profile(
    State(state): State<AppState>,
    Json(body): Json<ContentBody>,
) -> Result<Json<ContentResponse>, (StatusCode, Json<ErrorBody>)> {
    let store = open_store(&user_slug(&state))?;
    store
        .write_profile_atomic(&body.content)
        .map_err(map_memory_error)?;
    Ok(Json(ContentResponse {
        content: body.content,
    }))
}

async fn get_index(
    State(state): State<AppState>,
) -> Result<Json<ContentResponse>, (StatusCode, Json<ErrorBody>)> {
    let store = open_store(&user_slug(&state))?;
    let content = store.read_index_raw().map_err(map_memory_error)?;
    Ok(Json(ContentResponse { content }))
}

async fn put_index(
    State(state): State<AppState>,
    Json(body): Json<ContentBody>,
) -> Result<Json<ContentResponse>, (StatusCode, Json<ErrorBody>)> {
    let store = open_store(&user_slug(&state))?;
    store
        .write_index_raw(&body.content)
        .map_err(map_memory_error)?;
    Ok(Json(ContentResponse {
        content: body.content,
    }))
}

async fn get_entry(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<MemoryEntry>, (StatusCode, Json<ErrorBody>)> {
    if !validate_slug(&slug) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid slug"));
    }
    let store = open_store(&user_slug(&state))?;
    let entry = store.read_entry(&slug).map_err(map_memory_error)?;
    Ok(Json(entry))
}

async fn put_entry(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<UpsertEntryBody>,
) -> Result<Json<MemoryEntry>, (StatusCode, Json<ErrorBody>)> {
    if !validate_slug(&slug) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid slug"));
    }
    let store = open_store(&user_slug(&state))?;
    let now = now_ts();
    let entry = match store.read_entry_raw(&slug) {
        Ok(mut existing) => {
            existing.summary = body.summary;
            existing.content = body.content;
            existing.updated_at = now;
            existing.deleted_at = None;
            existing.ensure_id();
            existing
        }
        Err(MemoryError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            MemoryEntry::new_active(slug.clone(), body.summary, body.content, now, None)
        }
        Err(e) => return Err(map_memory_error(e)),
    };
    store.write_entry_atomic(&entry).map_err(map_memory_error)?;
    store
        .upsert_index_line(&MemoryIndexLine {
            slug: slug.clone(),
            summary: entry.summary.clone(),
            updated_at: now,
        })
        .map_err(map_memory_error)?;
    sync_index::best_effort_upsert(&store, &entry);
    Ok(Json(entry))
}

async fn delete_entry(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    if !validate_slug(&slug) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid slug"));
    }
    let store = open_store(&user_slug(&state))?;
    store
        .soft_delete_entry(&slug, now_ts())
        .map_err(map_memory_error)?;
    sync_index::best_effort_delete(&store, &slug);
    Ok(Json(serde_json::json!({ "deleted": slug })))
}

async fn create_entry(
    State(state): State<AppState>,
    Json(body): Json<CreateEntryBody>,
) -> Result<Json<CreateEntryResponse>, (StatusCode, Json<ErrorBody>)> {
    if !validate_slug(&body.slug) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid slug"));
    }
    let store = open_store(&user_slug(&state))?;
    let resolved = resolve_slug(&store, &body.slug);
    let now = now_ts();
    let supersedes = if resolved != body.slug {
        Some(body.slug.clone())
    } else {
        None
    };
    let entry = MemoryEntry::new_active(
        resolved.clone(),
        body.summary,
        body.content,
        now,
        supersedes,
    );
    store.write_entry_atomic(&entry).map_err(map_memory_error)?;
    store
        .upsert_index_line(&MemoryIndexLine {
            slug: resolved.clone(),
            summary: entry.summary.clone(),
            updated_at: now,
        })
        .map_err(map_memory_error)?;
    sync_index::best_effort_upsert(&store, &entry);
    Ok(Json(CreateEntryResponse {
        slug: resolved,
        requested_slug: body.slug,
        entry,
    }))
}
