//! Browsing, creating and refreshing works.

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    auth::Identity,
    db::repo,
    domain::{ExternalIds, MediaItem, MediaKind, make_slug},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/items", get(list).post(create))
        .route("/items/{id}", get(detail).patch(update).delete(remove))
        .route("/items/{id}/refresh", post(refresh))
        .route("/items/{id}/snapshots", get(snapshots))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub term: Option<String>,
    pub kind: Option<String>,
    pub year: Option<i32>,
    #[serde(default)]
    pub manual_only: bool,
    #[serde(default)]
    pub include_disabled: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse {
    pub items: Vec<MediaItem>,
    pub total: i64,
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    let kind = query
        .kind
        .as_deref()
        .map(str::parse::<MediaKind>)
        .transpose()
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let mut items = repo::item::search(
        &state.db,
        &repo::item::Query {
            term: query.term,
            kind,
            year: query.year,
            manual_only: query.manual_only,
            include_disabled: query.include_disabled,
            limit: query.limit.unwrap_or(50),
            offset: query.offset.unwrap_or(0),
        },
    )
    .await?;

    // Without this the list would show provider values while the detail view
    // showed edited ones, and a lock would look like it had not taken.
    service::apply_overrides(&state, &mut items).await?;

    let total = repo::item::count(&state.db, kind).await?;

    Ok(Json(ListResponse { items, total }))
}

/// One work, with children and manual overrides applied.
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<MediaItem>> {
    service::load(&state, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub kind: String,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub status: Option<String>,
    pub runtime: Option<i32>,
    pub first_aired: Option<String>,
    pub in_cinemas: Option<String>,
    pub network: Option<String>,
    pub studio: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub external_ids: ExternalIds,
}

/// Create an entry by hand.
///
/// The result is marked manual, which exempts it from the refresh scheduler and
/// protects its children from being replaced. If external ids are supplied it
/// still becomes refreshable — a manual entry can be a stub that later fills in
/// from a provider without losing what was typed.
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<CreateRequest>,
) -> AppResult<(StatusCode, Json<MediaItem>)> {
    require_write(&identity)?;

    let kind: MediaKind = request
        .kind
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;

    let title = request.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("title must not be empty".into()));
    }

    let mut item = MediaItem::empty(kind);
    item.title = title.to_string();
    item.year = request.year;
    item.overview = request.overview;
    item.status = request.status;
    item.runtime = request.runtime;
    item.first_aired = request.first_aired;
    item.in_cinemas = request.in_cinemas;
    item.network = request.network;
    item.studio = request.studio;
    item.genres = request.genres;
    item.external_ids = request.external_ids;
    item.is_manual = true;
    item.slug = unique_slug(&state, kind, title, request.year).await?;

    for (source, value) in item.external_ids.rows(kind) {
        if repo::item::find_id_by_external(&state.db, source, &value)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict(format!(
                "another entry already claims {source}={value}"
            )));
        }
    }

    repo::item::upsert(
        &state.db,
        repo::item::ItemWrite {
            item: &item,
            replace_children: false,
        },
    )
    .await?;

    tracing::info!(id = %item.id, actor = %identity.label(), "created a manual entry");

    let stored = service::load(&state, &item.id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("entry vanished after being created")))?;

    Ok((StatusCode::CREATED, Json(stored)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequest {
    pub is_enabled: Option<bool>,
}

/// Change an entry's own state. Field edits go through the override endpoints.
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> AppResult<Json<MediaItem>> {
    require_write(&identity)?;

    if let Some(enabled) = request.is_enabled {
        if !repo::item::set_enabled(&state.db, &id, enabled).await? {
            return Err(AppError::NotFound);
        }
        state.caches.items.invalidate(&format!("item:{id}")).await;
    }

    service::load(&state, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_write(&identity)?;

    if !repo::item::delete(&state.db, &id).await? {
        return Err(AppError::NotFound);
    }

    state.caches.items.invalidate(&format!("item:{id}")).await;
    tracing::info!(%id, actor = %identity.label(), "deleted an entry");

    Ok(StatusCode::NO_CONTENT)
}

/// Refetch from providers now, ignoring the refresh schedule.
async fn refresh(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<MediaItem>> {
    require_write(&identity)?;

    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    let refreshed = crate::jobs::refresh::refresh_one(&state, &item)
        .await
        .map_err(AppError::UpstreamUnavailable)?;

    Ok(Json(refreshed.unwrap_or(item)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub provider: String,
    pub fetched_at: String,
    pub etag: Option<String>,
    pub payload: serde_json::Value,
}

/// The raw provider documents behind an entry, for diagnosing a bad mapping.
async fn snapshots(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<SnapshotSummary>>> {
    let snapshots = repo::snapshot::list(&state.db, &id).await?;

    Ok(Json(
        snapshots
            .into_iter()
            .map(|s| SnapshotSummary {
                provider: s.provider,
                fetched_at: s.fetched_at,
                etag: s.etag,
                payload: s.payload,
            })
            .collect(),
    ))
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}

/// A slug not already taken by another work of the same kind.
///
/// Two films can share a title and a year; the slug is a primary key here, so
/// the second one gets a suffix rather than failing the create.
async fn unique_slug(
    state: &AppState,
    kind: MediaKind,
    title: &str,
    year: Option<i32>,
) -> AppResult<String> {
    let base = make_slug(title, year);

    if repo::item::find_id_by_slug(&state.db, kind, &base)
        .await?
        .is_none()
    {
        return Ok(base);
    }

    for suffix in 2..=50 {
        let candidate = format!("{base}-{suffix}");
        if repo::item::find_id_by_slug(&state.db, kind, &candidate)
            .await?
            .is_none()
        {
            return Ok(candidate);
        }
    }

    Err(AppError::Conflict(format!(
        "too many entries already share the slug {base:?}"
    )))
}
