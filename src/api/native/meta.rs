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
        .routes(routes!(import_dataset))
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
    /// The rest is how the server is run, for whoever maintains the catalogue
    /// — the administration's dashboard reads them — and absent for anybody
    /// else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overrides: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clients: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_entries: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_searches: Option<u64>,
}

/// How much this server is holding.
///
/// Counted as the reader is shown the catalogue: for whoever maintains it,
/// every work; for anybody else — a visitor among them, under public
/// browsing — the works switched on and, as the adult policy has it for
/// them, not for adults, as the lists and `/figures` count them. How the
/// server is run — keys, locks, the journal, the jobs, the caches — is told
/// only to whoever maintains it, whose dashboard this is.
#[utoipa::path(get, path = "/stats", tag = TAG, responses((status = 200, body = Stats)))]
async fn stats(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Stats>> {
    let (series, movies) = if identity.can_write() {
        (
            repo::item::count(&state.db, Some(MediaKind::Series)).await?,
            repo::item::count(&state.db, Some(MediaKind::Movie)).await?,
        )
    } else {
        let shown = |kind: MediaKind| repo::item::Query {
            kind: Some(kind),
            include_adult: state.adult_for(identity.client_id(), identity.peer_id(), None),
            ..Default::default()
        };
        (
            repo::item::count_matching(&state.db, &shown(MediaKind::Series)).await?,
            repo::item::count_matching(&state.db, &shown(MediaKind::Movie)).await?,
        )
    };

    let mut stats = Stats {
        series,
        movies,
        total: series + movies,
        overrides: None,
        clients: None,
        audit_entries: None,
        jobs: None,
        cached_items: None,
        cached_searches: None,
    };
    if identity.can_write() {
        stats.overrides = Some(repo::override_field::count(&state.db).await?);
        stats.clients = Some(repo::client::count(&state.db).await?);
        stats.audit_entries = Some(repo::audit::count(&state.db).await?);
        stats.jobs = Some(repo::job::count(&state.db).await?);
        stats.cached_items = Some(state.caches.items.l1_entries());
        stats.cached_searches = Some(state.caches.searches.l1_entries());
    }
    Ok(Json(stats))
}

#[derive(Serialize, ToSchema)]
pub struct Sources {
    /// `tmdb`, `tvdb`, `fanart`, `tvmaze`, `anilist`, `mal`, `imdb`, `fankai`, `fankaiwiki`: those
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
        (names::FANKAI, state.flag("fankai.enabled", false)),
        // Asked only in the course of a Fankai fetch.
        (
            names::FANKAI_WIKI,
            state.flag("fankai.enabled", false) && state.flag("fankai.wiki", false),
        ),
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
    /// One instance, or several coordinating through the cache server.
    pub mode: crate::config::Mode,
    pub tmdb_configured: bool,
    pub tmdb_language: String,
    pub skyhook_fallback: bool,
    pub refresh_enabled: bool,
    pub auth_disabled: bool,
    pub public_browse: bool,
    pub native_policy: &'static str,
    pub tmdb_policy: &'static str,
    pub tvdb_policy: &'static str,
    pub anilist_policy: &'static str,
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
        mode: state.coord.mode(),
        tmdb_configured: state.tmdb.is_configured(),
        tmdb_language: state.config.tmdb.language.clone(),
        skyhook_fallback: state.flag("skyhook.fallback", true),
        refresh_enabled: state.flag("refresh.enabled", true),
        auth_disabled: state.config.security.auth_disabled,
        public_browse: state.public_site(),
        native_policy: policy_name(state.config.policy_for(Surface::Native)),
        tmdb_policy: policy_name(state.config.policy_for(Surface::Tmdb)),
        tvdb_policy: policy_name(state.config.policy_for(Surface::Tvdb)),
        anilist_policy: policy_name(state.config.policy_for(Surface::Anilist)),
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
    /// A task's name: `refresh.sweep`, `refresh.item`, `refresh.all`,
    /// `import.anime`, `import.imdb`, `export.nfo`.
    pub kind: Option<String>,
    /// `running`, `succeeded` or `failed`.
    pub status: Option<String>,
    /// `schedule`, or `person` for the runs somebody started.
    pub by: Option<String>,
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

    let filter = repo::job::Query {
        kind: query.kind,
        status: query.status,
        by: query.by,
        limit: query.limit.unwrap_or(50),
        offset: query.offset.unwrap_or(0),
    };
    let mut jobs = repo::job::list(&state.db, &filter).await?;

    // A refresh names its work by id; the page names it by its title.
    let ids: Vec<String> = jobs
        .iter()
        .filter_map(|j| j.target.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let works = repo::item::titles(&state.db, &ids).await?;
    for job in &mut jobs {
        job.work = job.target.as_deref().and_then(|id| works.get(id)).cloned();
    }

    Ok(Json(JobsResponse {
        jobs,
        // As many as the filters match, so the pages end where the runs do.
        total: repo::job::count_matching(&state.db, &filter).await?,
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

#[derive(Serialize, ToSchema)]
pub struct Imported {
    /// What the import kept: `8442 entries`, `3 of 3 works rated`. Absent
    /// while it is still going.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Download one of the lists the further sources work from, now.
///
/// `anime-lists` or `imdb-ratings`. Refused while the source that needs the
/// list is off, and while another import is running. An import that takes
/// longer than a request is given carries on after the answer, 202, and is
/// followed under Jobs like any other.
#[utoipa::path(
    post, path = "/datasets/{name}/import", tag = TAG,
    params(("name" = String, Path, description = "`anime-lists` or `imdb-ratings`")),
    responses(
        (status = 200, body = Imported),
        (status = 202, description = "Still downloading; it carries on, and its result is under Jobs", body = Imported),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No list by that name"),
        (status = 409, description = "The source is off, or an import is already running"),
        (status = 502, description = "The list could not be downloaded or read"),
    ),
)]
async fn import_dataset(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> AppResult<(StatusCode, Json<Imported>)> {
    require_admin(&identity)?;

    use crate::jobs::datasets::{Asked, import_now};

    // On its own task, recording itself, whatever becomes of this request:
    // run inside it, an import that outlasted the request's time limit was
    // dropped halfway, its job left "running" until a restart and nothing
    // written down. The IMDb list alone is allowed five minutes to arrive.
    let (done, answer) = tokio::sync::oneshot::channel();
    let task = state.clone();
    let actor = identity.clone();
    let list = name.clone();
    tokio::spawn(async move {
        let by = actor.label();
        let asked = import_now(&task, &list, Some(&by)).await;
        if let Asked::Imported(summary) = &asked {
            audit::record(
                &task,
                Event {
                    identity: Some(&actor),
                    ip: &ip,
                    action: Action::DatasetImported,
                    target: Some(&list),
                    detail: Some(summary),
                },
            )
            .await;
        }
        // Whoever asked may have been answered already; that is fine.
        let _ = done.send(asked);
    });

    // Answered before the request's own limit would cut it off.
    let wait = state.config.server.request_timeout.mul_f64(0.75);
    let asked = match tokio::time::timeout(wait, answer).await {
        Ok(Ok(asked)) => asked,
        Ok(Err(_)) => {
            return Err(crate::error::AppError::Internal(anyhow::anyhow!(
                "the import stopped without saying how it went"
            )));
        }
        Err(_) => return Ok((StatusCode::ACCEPTED, Json(Imported { summary: None }))),
    };

    match asked {
        Asked::Imported(summary) => Ok((
            StatusCode::OK,
            Json(Imported {
                summary: Some(summary),
            }),
        )),
        Asked::Unknown => Err(crate::error::AppError::NotFound),
        Asked::NotWanted => Err(crate::error::AppError::Conflict(
            "no source that uses this list is switched on".into(),
        )),
        Asked::Busy => Err(crate::error::AppError::Conflict(
            "an import is already running; it will be done in a moment".into(),
        )),
        // Why is in the job run, where an operator reads it; the answer does
        // not repeat it, since it can name hosts.
        Asked::Failed(e) => Err(crate::error::AppError::UpstreamUnavailable(e)),
    }
}

pub(crate) fn policy_name(policy: SurfacePolicy) -> &'static str {
    match policy {
        SurfacePolicy::ApiKey => "apikey",
        SurfacePolicy::Allowlist => "allowlist",
        SurfacePolicy::Open => "open",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MediaItem;

    #[tokio::test]
    async fn a_reader_is_counted_what_a_reader_is_shown() {
        let state = super::super::testing::server().await;
        for (title, enabled, adult) in [
            ("Shown", true, false),
            ("Switched off", false, false),
            ("For adults", true, true),
        ] {
            let mut work = MediaItem::empty(MediaKind::Series);
            work.title = title.into();
            work.slug = crate::domain::make_slug(title, None);
            work.is_enabled = enabled;
            work.is_adult = adult;
            repo::item::upsert(
                &state.db,
                repo::item::ItemWrite {
                    item: &work,
                    replace_children: true,
                },
            )
            .await
            .expect("stored");
        }

        // A visitor: what the lists show them, and nothing of how the server
        // is run.
        let Json(seen) = stats(State(state.clone()), Extension(Identity::Visitor))
            .await
            .expect("answered");
        assert_eq!((seen.series, seen.movies, seen.total), (1, 0, 1));
        assert!(seen.overrides.is_none() && seen.clients.is_none());
        assert!(seen.audit_entries.is_none() && seen.jobs.is_none());
        assert!(seen.cached_items.is_none() && seen.cached_searches.is_none());
        let shape = serde_json::to_value(&seen).unwrap();
        assert!(shape.get("auditEntries").is_none(), "{shape}");

        // Whoever maintains the catalogue: every work, and the counters.
        let Json(kept) = stats(State(state), Extension(Identity::Anonymous))
            .await
            .expect("answered");
        assert_eq!(kept.series, 3);
        assert!(kept.clients.is_some() && kept.audit_entries.is_some());
    }
}
