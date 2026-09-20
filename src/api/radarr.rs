//! Radarr compatibility.
//!
//! Radarr builds these URLs from `https://api.radarr.video/v1/{route}`. As with
//! Sonarr, the host is compiled in, so reaching this server means overriding DNS.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    error::{AppError, AppResult},
    service::movie,
    state::AppState,
    wire::radarr::{CollectionResource, MovieResource, from_item},
};

pub fn router() -> Router<AppState> {
    Router::new()
        // Static segments are matched before the `{tmdb_id}` route, so these
        // three do not need to be ordered by hand.
        .route("/v1/movie/bulk", post(bulk))
        .route("/v1/movie/changed", get(changed))
        .route("/v1/movie/imdb/{imdb_id}", get(by_imdb))
        .route("/v1/movie/collection/{tmdb_id}", get(collection))
        .route("/v1/movie/{tmdb_id}", get(by_tmdb))
        .route("/v1/search", get(search))
        .route("/v1/list/tmdb/popular", get(popular))
        .route("/v1/list/tmdb/trending", get(trending))
}

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

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let term = query.q.unwrap_or_default();

    let items = movie::search(&state, &term, query.year).await?;

    Ok(Json(items.iter().map(from_item).collect()))
}

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

async fn popular(State(state): State<AppState>) -> AppResult<Json<Vec<MovieResource>>> {
    Ok(Json(
        movie::popular(&state)
            .await?
            .iter()
            .map(from_item)
            .collect(),
    ))
}

async fn trending(State(state): State<AppState>) -> AppResult<Json<Vec<MovieResource>>> {
    Ok(Json(
        movie::trending(&state)
            .await?
            .iter()
            .map(from_item)
            .collect(),
    ))
}
