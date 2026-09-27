//! Sonarr compatibility: the Skyhook surface.
//!
//! Sonarr builds these URLs from
//! `https://skyhook.sonarr.tv/v1/tvdb/{route}/{language}/` and cannot be pointed
//! elsewhere, so reaching this server means overriding DNS for that host. See
//! `docs/integration.md`.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    error::{AppError, AppResult},
    service::{language, series},
    state::AppState,
    wire::sonarr::{ShowResource, from_item},
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Sonarr compatibility";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show))
        .routes(routes!(search))
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

/// A series by TVDB id, in Skyhook's format.
///
/// The id may also be one this server synthesised for a work TMDB indexes and
/// TVDB does not; the response echoes back whichever id was asked for, because
/// Sonarr has already stored it.
#[utoipa::path(
    get, path = "/v1/tvdb/shows/{language}/{tvdb_id}", tag = TAG,
    params(
        ("language" = String, Path, description = "Two-letter language code, e.g. `en`"),
        ("tvdb_id" = i64, Path, description = "TVDB series id, or a synthesised one"),
    ),
    responses(
        (status = 200, body = ShowResource),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No provider could resolve this id"),
    ),
    security(),
)]
async fn show(
    State(state): State<AppState>,
    Path(ShowPath { language, tvdb_id }): Path<ShowPath>,
) -> AppResult<Json<ShowResource>> {
    let mut item = series::by_client_id(&state, tvdb_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Sonarr puts the language in the URL, which is the only place in this
    // protocol it appears. Episode text for a language nobody has asked for yet
    // is fetched here, once, and stored.
    language::apply(&state, &mut item, &language).await?;

    // Echo back the id the client asked for: Sonarr has already stored it, and
    // a different one in the response would orphan the series.
    state.media.for_clients(&mut item);
    Ok(Json(from_item(&item, tvdb_id, &language)))
}

/// Search series, in Skyhook's format.
///
/// `term` may be free text or a provider lookup: `tvdb:`, `tmdb:`, `imdb:`,
/// `mal:` or `anilist:`. Results Sonarr could not address are omitted — showing
/// one that fails on add is worse than showing nothing.
#[utoipa::path(
    get, path = "/v1/tvdb/search/{language}", tag = TAG,
    params(
        ("language" = String, Path, description = "Two-letter language code, e.g. `en`"),
        ("term" = Option<String>, Query, description = "Free text, or `prefix:id`"),
    ),
    responses(
        (status = 200, body = Vec<ShowResource>),
        (status = 403, description = "The caller's address is not in the allowlist"),
    ),
    security(),
)]
async fn search(
    State(state): State<AppState>,
    Path(SearchPath { language }): Path<SearchPath>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<ShowResource>>> {
    let term = query.term.unwrap_or_default();

    let mut items = series::search(&state, &term).await?;

    // Only the work's own title here: fetching a season of episode text for
    // each of ten search results would turn one search into dozens of calls.
    for item in &mut items {
        language::apply_shallow(&state, item, &language);
        state.media.for_clients(item);
    }

    // A result Sonarr cannot address is worse than no result: it would show in
    // the list and then fail on add.
    let shows = items
        .iter()
        .filter_map(|item| series::client_id(item).map(|id| from_item(item, id, &language)))
        .collect();

    Ok(Json(shows))
}
