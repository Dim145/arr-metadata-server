//! Server metadata: what is editable, what is stored, how it is configured.

use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
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
    config::{Surface, SurfacePolicy},
    db::repo::{self, audit::Action},
    domain::{MediaKind, fields},
    error::AppResult,
    state::AppState,
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Server";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(field_registry))
        .routes(routes!(stats))
        .routes(routes!(sources))
        .routes(routes!(settings))
        .routes(routes!(clear_cache))
        .routes(routes!(jobs))
}

#[derive(Serialize, ToSchema)]
pub struct FieldRegistry {
    pub item: &'static [fields::FieldDef],
    pub season: &'static [fields::FieldDef],
    pub episode: &'static [fields::FieldDef],
}

/// Which fields are editable, and what type each holds.
///
/// This is the authority: an override naming a field absent from here is
/// refused. The web UI builds its edit form from it.
#[utoipa::path(
    get, path = "/fields", tag = TAG,
    responses((status = 200, body = FieldRegistry)),
)]
async fn field_registry() -> Json<FieldRegistry> {
    Json(FieldRegistry {
        item: fields::ITEM_FIELDS,
        season: fields::SEASON_FIELDS,
        episode: fields::EPISODE_FIELDS,
    })
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub series: i64,
    pub movies: i64,
    pub total: i64,
    pub overrides: i64,
    pub clients: i64,
    pub audit_entries: i64,
    pub jobs: i64,
    pub cached_items: u64,
    pub cached_searches: u64,
}

/// How much this server is holding.
#[utoipa::path(get, path = "/stats", tag = TAG, responses((status = 200, body = Stats)))]
async fn stats(State(state): State<AppState>) -> AppResult<Json<Stats>> {
    let series = repo::item::count(&state.db, Some(MediaKind::Series)).await?;
    let movies = repo::item::count(&state.db, Some(MediaKind::Movie)).await?;

    Ok(Json(Stats {
        series,
        movies,
        total: series + movies,
        overrides: repo::override_field::count(&state.db).await?,
        clients: repo::client::count(&state.db).await?,
        audit_entries: repo::audit::count(&state.db).await?,
        jobs: repo::job::count(&state.db).await?,
        cached_items: state.caches.items.entry_count(),
        cached_searches: state.caches.searches.entry_count(),
    }))
}

#[derive(Serialize, ToSchema)]
pub struct Sources {
    /// `tmdb`, `tvdb`, `fanart`, `tvmaze`, `anilist`, `mal`, `imdb`: those
    /// switched on and able to answer, in that order.
    pub sources: Vec<&'static str>,
}

/// Where the data this server serves comes from.
///
/// For the credits every page carries. Several of these sources make their
/// data free to use on the condition that it is credited where it is shown —
/// TVmaze's licence asks for a link, IMDb's for a line — so a visitor has to be
/// able to read this too.
#[utoipa::path(get, path = "/sources", tag = TAG, responses((status = 200, body = Sources)))]
async fn sources(State(state): State<AppState>) -> Json<Sources> {
    use crate::providers::names;

    let candidates = [
        (names::TMDB, state.tmdb.is_configured()),
        (names::TVDB, state.tvdb.is_enabled()),
        (names::FANART, state.fanart.is_enabled()),
        (names::TVMAZE, state.flag("tvmaze.enabled", false)),
        (names::ANILIST, state.flag("anilist.enabled", false)),
        (names::MAL, state.flag("mal.enabled", false)),
        (names::IMDB, state.flag("imdb.enabled", false)),
    ];

    Json(Sources {
        sources: candidates
            .into_iter()
            .filter_map(|(name, on)| on.then_some(name))
            .collect(),
    })
}

/// A list downloaded whole: when it last landed, and how much of it was kept.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListImport {
    pub imported_at: String,
    pub rows: i64,
}

/// What the further sources are working from.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FurtherSources {
    /// `official` when `AMS_MAL_CLIENT_ID` is set, `jikan` otherwise.
    pub mal_via: &'static str,
    /// The anime identifier list; absent until it has been downloaded once.
    pub anime_list: Option<ListImport>,
    /// IMDb's ratings; absent until they have been downloaded once.
    pub imdb_ratings: Option<ListImport>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub version: &'static str,
    pub public_url: Option<String>,
    pub database: &'static str,
    pub tmdb_configured: bool,
    pub tmdb_language: String,
    pub skyhook_fallback: bool,
    pub refresh_enabled: bool,
    pub auth_disabled: bool,
    pub public_browse: bool,
    pub native_policy: &'static str,
    pub tmdb_policy: &'static str,
    pub arr_policy: &'static str,
    pub further_sources: FurtherSources,
}

/// Effective configuration.
///
/// Deliberately carries no secrets: the TMDB key is reported as a boolean,
/// never echoed. Behind the administrator check all the same — which surface
/// takes which credential, whether authentication is off and whether anyone may
/// browse is a map of the way in, and only the interface asks for it.
#[utoipa::path(
    get, path = "/settings", tag = TAG,
    responses(
        (status = 200, body = Settings),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn settings(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Settings>> {
    require_admin(&identity)?;

    let imported = |import: Option<repo::import::Import>| {
        import.map(|i| ListImport {
            imported_at: i.imported_at,
            rows: i.row_count,
        })
    };

    let further_sources = FurtherSources {
        mal_via: if state.mal.uses_official_api() {
            "official"
        } else {
            "jikan"
        },
        anime_list: imported(repo::import::get(&state.db, crate::jobs::datasets::ANIME).await?),
        imdb_ratings: imported(repo::import::get(&state.db, crate::jobs::datasets::IMDB).await?),
    };

    Ok(Json(Settings {
        version: env!("CARGO_PKG_VERSION"),
        public_url: state.config.server.public_url.clone(),
        database: match state.db.dialect() {
            crate::db::Dialect::Sqlite => "sqlite",
            crate::db::Dialect::Postgres => "postgres",
        },
        tmdb_configured: state.tmdb.is_configured(),
        tmdb_language: state.config.tmdb.language.clone(),
        skyhook_fallback: state.flag("skyhook.fallback", true),
        refresh_enabled: state.flag("refresh.enabled", true),
        auth_disabled: state.config.security.auth_disabled,
        public_browse: state.config.security.public_browse,
        native_policy: policy_name(state.config.policy_for(Surface::Native)),
        tmdb_policy: policy_name(state.config.policy_for(Surface::Tmdb)),
        arr_policy: policy_name(state.config.policy_for(Surface::Arr)),
        further_sources,
    }))
}

fn require_admin(identity: &Identity) -> AppResult<()> {
    identity
        .is_admin()
        .then_some(())
        .ok_or(crate::error::AppError::Forbidden)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct JobQuery {
    /// `refresh.sweep` or `refresh.item`.
    pub kind: Option<String>,
    /// `running`, `succeeded` or `failed`.
    pub status: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub offset: Option<i64>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobsResponse {
    pub jobs: Vec<repo::job::Job>,
    pub total: i64,
}

/// What the scheduler has been doing, newest first.
///
/// One row per run: a sweep over twenty-five entries is one job with a summary,
/// because twenty-five rows every fifteen minutes would bury the one that
/// failed. A refresh someone asked for by hand gets its own row.
#[utoipa::path(
    get, path = "/jobs", tag = TAG,
    params(JobQuery),
    responses(
        (status = 200, body = JobsResponse),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn jobs(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<JobQuery>,
) -> AppResult<Json<JobsResponse>> {
    require_admin(&identity)?;

    let jobs = repo::job::list(
        &state.db,
        &repo::job::Query {
            kind: query.kind,
            status: query.status,
            limit: query.limit.unwrap_or(50),
            offset: query.offset.unwrap_or(0),
        },
    )
    .await?;

    Ok(Json(JobsResponse {
        jobs,
        total: repo::job::count(&state.db).await?,
    }))
}

/// Drop every cached entity and search result.
///
/// The database is untouched — this only discards the in-process layer in front
/// of it, so the next request re-reads and re-applies overrides.
#[utoipa::path(
    post, path = "/cache/clear", tag = TAG,
    responses(
        (status = 204, description = "Cleared"),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn clear_cache(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
) -> AppResult<StatusCode> {
    if !identity.can_write() {
        return Err(crate::error::AppError::Forbidden);
    }

    state.caches.invalidate_all().await;
    tracing::info!(actor = %identity.label(), "cleared the in-process cache");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::CacheCleared,
            target: None,
            detail: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

fn policy_name(policy: SurfacePolicy) -> &'static str {
    match policy {
        SurfacePolicy::ApiKey => "apikey",
        SurfacePolicy::Allowlist => "allowlist",
        SurfacePolicy::Open => "open",
    }
}
