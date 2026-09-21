//! Finding a work at a provider, and pulling it in.
//!
//! The catalogue otherwise fills itself: a client asks for something, this
//! server fetches it, and it is stored. That is the right default and a poor way
//! to add one particular film — you would have to make Radarr ask for it.
//!
//! So: search the providers directly, see what they have, and take the one you
//! meant. The fetch is the same path a client's request takes, so an imported
//! work is indistinguishable from one that arrived on its own, refresh
//! schedule and all.

use axum::{
    Extension, Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::audit::Action,
    domain::{MediaItem, MediaKind},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

pub const TAG: &str = "Discover";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(search))
        .routes(routes!(import))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub term: String,
    /// `series` or `movie`. Both are searched when this is absent.
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub year: Option<i32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub kind: MediaKind,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    /// Whether this server already holds it, so the interface can say "stored"
    /// rather than offering to import it twice.
    pub stored: bool,
    pub is_adult: bool,
}

/// Ask the providers what they have.
///
/// This reaches out; it is not a search of what is already stored. The results
/// are shallow on purpose — enough to recognise the right work and no more,
/// because fetching twenty in full to show a list would be twenty times the
/// work for nineteen wasted.
#[utoipa::path(
    get, path = "/discover", tag = TAG,
    params(SearchQuery),
    responses(
        (status = 200, body = Vec<Found>),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn search(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<Found>>> {
    require_write(&identity)?;

    let term = query.term.trim();
    if term.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let wanted = query
        .kind
        .as_deref()
        .map(str::parse::<MediaKind>)
        .transpose()
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let language = state.language(None, None);
    let mut found = Vec::new();

    if wanted != Some(MediaKind::Movie) {
        match service::series::search(&state, term, &language).await {
            Ok(hits) => found.extend(hits.iter().map(describe)),
            Err(e) => tracing::warn!(term, error = %e, "series search failed"),
        }
    }

    if wanted != Some(MediaKind::Series) {
        match service::movie::search(&state, term, query.year).await {
            Ok(hits) => found.extend(hits.iter().map(describe)),
            Err(e) => tracing::warn!(term, error = %e, "movie search failed"),
        }
    }

    // Silence from a client means no — one that never mentions adult titles is
    // not asking for them. It cannot mean that here: this is the operator's own
    // screen, and they said what they wanted when they set `adult.mode`. Asking
    // for what the server allows is what makes the setting visible from the
    // place it is set.
    let adult = state.adult_for(
        identity.client_id(),
        identity.peer_id(),
        Some(state.adult_visible()),
    );
    found.retain(|hit| adult || !hit.is_adult);

    // Whether each is already held, asked of the store rather than inferred
    // from the result: a freshly mapped hit carries an id and a timestamp
    // because every entity does, which says nothing about it being stored.
    for hit in &mut found {
        hit.stored = held(&state, hit).await;
    }

    Ok(Json(found))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub kind: String,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
}

/// Fetch a work from its providers and store it.
///
/// Idempotent: importing something already held refreshes it rather than
/// duplicating it, because the fetch path keys on the external id.
#[utoipa::path(
    post, path = "/discover/import", tag = TAG,
    request_body = ImportRequest,
    responses(
        (status = 200, body = MediaItem),
        (status = 400, description = "No usable identifier"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No provider had it"),
    ),
)]
async fn import(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<ImportRequest>,
) -> AppResult<Json<MediaItem>> {
    require_write(&identity)?;

    let kind: MediaKind = request
        .kind
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;

    let language = state.language(None, None);

    let found = match kind {
        MediaKind::Series => match (request.tvdb_id, request.tmdb_id, request.imdb_id.as_deref()) {
            (Some(tvdb), _, _) => service::series::by_tvdb_id(&state, tvdb, &language).await?,
            (_, Some(tmdb), _) => service::series::by_tmdb_id(&state, tmdb).await?,
            (_, _, Some(imdb)) => service::series::by_imdb_id(&state, imdb).await?,
            _ => return Err(AppError::BadRequest("no identifier to fetch by".into())),
        },
        MediaKind::Movie => match (request.tmdb_id, request.imdb_id.as_deref()) {
            (Some(tmdb), _) => service::movie::by_tmdb_id(&state, tmdb).await?,
            (_, Some(imdb)) => service::movie::by_imdb_id(&state, imdb).await?,
            _ => return Err(AppError::BadRequest("no identifier to fetch by".into())),
        },
    };

    let item = found.ok_or(AppError::NotFound)?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ItemImported,
            target: Some(&item.title),
            detail: Some(&format!("{} {}", kind.as_str(), item.id)),
        },
    )
    .await;

    Ok(Json(item))
}

fn describe(item: &MediaItem) -> Found {
    Found {
        kind: item.kind,
        title: item.title.clone(),
        year: item.year,
        overview: item.overview.clone(),
        poster: item
            .images
            .iter()
            .find(|image| image.cover_type == crate::domain::CoverType::Poster)
            .map(|image| image.url.clone()),
        tmdb_id: item.external_ids.tmdb,
        tvdb_id: item.external_ids.tvdb,
        imdb_id: item.external_ids.imdb.clone(),
        // Filled in afterwards, against the store.
        stored: false,
        is_adult: item.is_adult,
    }
}

/// Whether this server already holds the work a hit stands for.
async fn held(state: &AppState, hit: &Found) -> bool {
    use crate::domain::ExternalSource::{Imdb, TmdbMovie, TmdbTv, TvdbSeries};

    let lookups: [(crate::domain::ExternalSource, Option<String>); 3] = [
        (TvdbSeries, hit.tvdb_id.map(|id| id.to_string())),
        (
            if hit.kind == MediaKind::Series {
                TmdbTv
            } else {
                TmdbMovie
            },
            hit.tmdb_id.map(|id| id.to_string()),
        ),
        (Imdb, hit.imdb_id.clone()),
    ];

    for (source, value) in lookups {
        let Some(value) = value else { continue };

        if matches!(
            crate::db::repo::item::find_id_by_external(&state.db, source, &value).await,
            Ok(Some(_))
        ) {
            return true;
        }
    }

    false
}

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}
