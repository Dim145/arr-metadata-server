//! Radarr compatibility.
//!
//! Radarr builds these URLs from `https://api.radarr.video/v1/{route}`. As with
//! Sonarr, the host is compiled in, so reaching this server means overriding DNS.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    error::{AppError, AppResult},
    service::movie,
    state::AppState,
    wire::radarr::{CollectionResource, MovieResource, from_item},
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Radarr compatibility";

pub fn router() -> OpenApiRouter<AppState> {
    // Static segments are matched before the `{tmdb_id}` route, so these do not
    // need to be ordered by hand.
    OpenApiRouter::new()
        .routes(routes!(bulk))
        .routes(routes!(changed))
        .routes(routes!(by_imdb))
        .routes(routes!(collection))
        .routes(routes!(by_tmdb))
        .routes(routes!(search))
        .routes(routes!(popular))
        .routes(routes!(trending))
}

/// A movie by TMDB id, in Radarr's format.
#[utoipa::path(
    get, path = "/v1/movie/{tmdb_id}", tag = TAG,
    params(("tmdb_id" = i64, Path, description = "TMDB movie id")),
    responses(
        (status = 200, body = MovieResource),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No provider could resolve this id"),
    ),
    security(),
)]
async fn by_tmdb(
    State(state): State<AppState>,
    Path(tmdb_id): Path<i64>,
) -> AppResult<Json<MovieResource>> {
    let item = movie::by_tmdb_id(&state, tmdb_id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(from_item(&item)))
}

/// Radarr expects an array here even though it only ever uses the first entry.
/// A movie by IMDb id. Radarr expects an array even though it reads only the first.
#[utoipa::path(
    get, path = "/v1/movie/imdb/{imdb_id}", tag = TAG,
    params(("imdb_id" = String, Path, description = "IMDb id, with or without the `tt` prefix")),
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn by_imdb(
    State(state): State<AppState>,
    Path(imdb_id): Path<String>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let found = movie::by_imdb_id(&state, &imdb_id).await?;

    Ok(Json(found.iter().map(from_item).collect()))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    year: Option<i32>,
}

/// Search movies, in Radarr's format.
#[utoipa::path(
    get, path = "/v1/search", tag = TAG,
    params(
        ("q" = Option<String>, Query, description = "Free text, or `tmdb:`/`imdb:` followed by an id"),
        ("year" = Option<i32>, Query, description = "Narrow to a release year"),
    ),
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let term = query.q.unwrap_or_default();

    let items = movie::search(&state, &term, query.year).await?;

    Ok(Json(items.iter().map(from_item).collect()))
}

/// Several movies at once.
///
/// A title that cannot be resolved is skipped rather than failing the batch,
/// which is what Radarr expects when one entry has gone away upstream. Capped
/// at 100 ids per request.
#[utoipa::path(
    post, path = "/v1/movie/bulk", tag = TAG,
    request_body = Vec<i64>,
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn bulk(
    State(state): State<AppState>,
    Json(tmdb_ids): Json<Vec<i64>>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let items = movie::bulk(&state, &tmdb_ids).await?;

    Ok(Json(items.iter().map(from_item).collect()))
}

#[derive(Deserialize)]
struct ChangedQuery {
    since: Option<String>,
}

/// TMDB ids changed since a date, which Radarr polls to know what to refetch.
#[utoipa::path(
    get, path = "/v1/movie/changed", tag = TAG,
    params(("since" = String, Query, description = "A date or timestamp; only the date part is used")),
    responses(
        (status = 200, body = Vec<i64>),
        (status = 400, description = "`since` was not supplied"),
    ),
    security(),
)]
async fn changed(
    State(state): State<AppState>,
    Query(query): Query<ChangedQuery>,
) -> AppResult<Json<Vec<i64>>> {
    let Some(since) = query.since else {
        return Err(AppError::BadRequest(
            "the `since` parameter is required".into(),
        ));
    };

    Ok(Json(movie::changed_since(&state, &since).await?))
}

/// A collection and the movies in it.
///
/// The parts carry search-level detail only: fetching each in full would be
/// dozens of upstream calls for a list Radarr uses to offer suggestions.
#[utoipa::path(
    get, path = "/v1/movie/collection/{tmdb_id}", tag = TAG,
    params(("tmdb_id" = i64, Path, description = "TMDB collection id")),
    responses(
        (status = 200, body = CollectionResource),
        (status = 404, description = "No such collection, or no TMDB key is configured"),
    ),
    security(),
)]
async fn collection(
    State(state): State<AppState>,
    Path(tmdb_id): Path<i64>,
) -> AppResult<Json<CollectionResource>> {
    let (name, overview, parts) = movie::collection(&state, tmdb_id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(CollectionResource {
        name,
        overview,
        tmdb_id,
        images: Vec::new(),
        parts: parts.iter().map(from_item).collect(),
    }))
}

/// TMDB's popular movies.
#[utoipa::path(
    get, path = "/v1/list/tmdb/popular", tag = TAG,
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn popular(State(state): State<AppState>) -> AppResult<Json<Vec<MovieResource>>> {
    Ok(Json(
        movie::popular(&state)
            .await?
            .iter()
            .map(from_item)
            .collect(),
    ))
}

/// TMDB's trending movies for the week.
#[utoipa::path(
    get, path = "/v1/list/tmdb/trending", tag = TAG,
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn trending(State(state): State<AppState>) -> AppResult<Json<Vec<MovieResource>>> {
    Ok(Json(
        movie::trending(&state)
            .await?
            .iter()
            .map(from_item)
            .collect(),
    ))
}
