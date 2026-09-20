//! Sonarr compatibility: the Skyhook surface.
//!
//! Sonarr builds these URLs from
//! `https://skyhook.sonarr.tv/v1/tvdb/{route}/{language}/` and cannot be pointed
//! elsewhere, so reaching this server means overriding DNS for that host. See
//! `docs/integration.md`.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;

use crate::{
    error::{AppError, AppResult},
    service::series,
    state::AppState,
    wire::sonarr::{ShowResource, from_item},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/tvdb/shows/{language}/{tvdb_id}", get(show))
        .route("/v1/tvdb/search/{language}", get(search))
}

#[derive(Deserialize)]
struct ShowPath {
    language: String,
    tvdb_id: i64,
}

#[derive(Deserialize)]
struct SearchPath {
    language: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    term: Option<String>,
}

async fn show(
    State(state): State<AppState>,
    Path(ShowPath { language, tvdb_id }): Path<ShowPath>,
) -> AppResult<Json<ShowResource>> {
    let item = series::by_client_id(&state, tvdb_id, &language)
        .await?
        .ok_or(AppError::NotFound)?;

    // Echo back the id the client asked for: Sonarr has already stored it, and
    // a different one in the response would orphan the series.
    Ok(Json(from_item(&item, tvdb_id, &language)))
}

async fn search(
    State(state): State<AppState>,
    Path(SearchPath { language }): Path<SearchPath>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<ShowResource>>> {
    let term = query.term.unwrap_or_default();

    let items = series::search(&state, &term, &language).await?;

    // A result Sonarr cannot address is worse than no result: it would show in
    // the list and then fail on add.
    let shows = items
        .iter()
        .filter_map(|item| series::client_id(item).map(|id| from_item(item, id, &language)))
        .collect();

    Ok(Json(shows))
}
