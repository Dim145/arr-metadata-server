//! The catalogue in numbers, for everyone; the server's health, for whoever
//! maintains it.

use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use axum::{Extension, Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo::{self, job::Job},
    domain::MediaKind,
    error::{AppError, AppResult},
    state::AppState,
};

const TAG: &str = super::meta::TAG;

/// When this process began: touched when the routes are built, which is at
/// the start.
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn router() -> OpenApiRouter<AppState> {
    LazyLock::force(&STARTED);
    OpenApiRouter::new()
        .routes(routes!(figures))
        .routes(routes!(health))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    pub name: String,
    pub count: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Figures {
    pub total: i64,
    pub series: i64,
    pub movies: i64,
    pub episodes: i64,
    /// Works added in the last thirty days.
    pub added_recently: i64,
    /// Works by decade of release, earliest first.
    pub decades: Vec<Count>,
    /// The commonest genres, most common first.
    pub genres: Vec<Count>,
    /// The commonest networks and studios, most common first.
    pub networks: Vec<Count>,
    /// The languages works were made in, commonest first.
    pub languages: Vec<Count>,
    /// Works by the whole part of their score, lowest first: `7` is seven
    /// point something.
    pub scores: Vec<Count>,
    pub statuses: Vec<Count>,
}

/// The catalogue in numbers: what it holds, when it was made, what it is
/// about, where it came from and how it is rated — counted as a reader is
/// shown it.
#[utoipa::path(
    get, path = "/figures", tag = TAG,
    responses((status = 200, body = Figures)),
)]
async fn figures(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Figures>> {
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let query = repo::item::Query {
        include_adult: adult,
        ..Default::default()
    };
    let facets = repo::item::facets(&state.db, &query).await?;
    let series = repo::item::count_matching(
        &state.db,
        &repo::item::Query {
            kind: Some(MediaKind::Series),
            ..query.clone()
        },
    )
    .await?;
    let movies = repo::item::count_matching(
        &state.db,
        &repo::item::Query {
            kind: Some(MediaKind::Movie),
            ..query.clone()
        },
    )
    .await?;
    // Written as `created_at` is, so the two compare as text the way they
    // compare as instants.
    let since = crate::db::to_rfc3339(chrono::Utc::now() - chrono::Duration::days(30));
    let counts = |facets: Vec<repo::item::Facet>, most: usize| -> Vec<Count> {
        facets
            .into_iter()
            .take(most)
            .map(|f| Count {
                name: f.value,
                count: f.count,
            })
            .collect()
    };

    Ok(Json(Figures {
        total: series + movies,
        series,
        movies,
        episodes: repo::item::episode_count(&state.db, adult, false).await?,
        added_recently: repo::item::added_since(&state.db, &since, adult).await?,
        decades: repo::item::decades(&state.db, adult)
            .await?
            .into_iter()
            .map(|(decade, count)| Count {
                name: format!("{decade}s"),
                count,
            })
            .collect(),
        genres: counts(facets.genres, 12),
        networks: counts(facets.networks, 10),
        languages: counts(facets.languages, 8),
        scores: repo::item::score_buckets(&state.db, adult)
            .await?
            .into_iter()
            .map(|(bucket, count)| Count {
                name: bucket.to_string(),
                count,
            })
            .collect(),
        statuses: counts(facets.statuses, 8),
    }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CacheFigures {
    pub entries: u64,
    pub bytes: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub name: &'static str,
    pub on: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub version: &'static str,
    pub uptime_seconds: u64,
    pub database: &'static str,
    pub works: i64,
    pub episodes: i64,
    /// Works whose last refresh failed.
    pub refresh_failed: i64,
    pub rules: i64,
    pub items_cache: CacheFigures,
    pub searches_cache: CacheFigures,
    pub lists_cache: CacheFigures,
    /// The sources, and whether each is switched on and able to answer.
    pub sources: Vec<Source>,
    /// The latest runs of the jobs, newest first.
    pub jobs: Vec<Job>,
}

/// How the server is: its version and uptime, what it holds, what failed,
/// what it keeps in memory, which sources are on and what its jobs did last.
#[utoipa::path(
    get, path = "/admin/health", tag = TAG,
    responses(
        (status = 200, body = Health),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn health(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Health>> {
    if !identity.is_admin() {
        return Err(AppError::Forbidden);
    }
    let everything = repo::item::Query {
        include_adult: true,
        include_disabled: true,
        ..Default::default()
    };
    let cache = |c: &moka::future::Cache<String, String>| CacheFigures {
        entries: c.entry_count(),
        bytes: c.weighted_size(),
    };
    Ok(Json(Health {
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: STARTED.elapsed().max(Duration::ZERO).as_secs(),
        database: match state.db.dialect() {
            crate::db::Dialect::Sqlite => "sqlite",
            crate::db::Dialect::Postgres => "postgres",
        },
        works: repo::item::count_matching(&state.db, &everything).await?,
        episodes: repo::item::episode_count(&state.db, true, true).await?,
        refresh_failed: repo::item::count_matching(
            &state.db,
            &repo::item::Query {
                refresh_failed: true,
                ..everything.clone()
            },
        )
        .await?,
        rules: repo::network::count_rules(&state.db).await?,
        items_cache: cache(&state.caches.items),
        searches_cache: cache(&state.caches.searches),
        lists_cache: CacheFigures {
            entries: state.caches.lists.entry_count(),
            bytes: state.caches.lists.weighted_size(),
        },
        sources: vec![
            Source {
                name: "tmdb",
                on: state.tmdb.is_configured(),
            },
            Source {
                name: "tvmaze",
                on: state.flag("tvmaze.enabled", false),
            },
            Source {
                name: "anilist",
                on: state.flag("anilist.enabled", false),
            },
            Source {
                name: "mal",
                on: state.flag("mal.enabled", false),
            },
            Source {
                name: "imdb",
                on: state.flag("imdb.enabled", false),
            },
            Source {
                name: "skyhook",
                on: state.flag("skyhook.fallback", false) || state.flag("skyhook.enrich", false),
            },
            Source {
                name: "radarr",
                on: state.flag("radarr.fallback", false) || state.flag("radarr.enrich", false),
            },
        ],
        jobs: repo::job::list(
            &state.db,
            &repo::job::Query {
                kind: None,
                status: None,
                limit: 8,
                offset: 0,
            },
        )
        .await?,
    }))
}
