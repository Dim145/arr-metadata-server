//! The catalogue in numbers, for everyone; the server's health, for whoever
//! maintains it.

use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use axum::{Extension, Json, extract::State, http::header};
use serde::{Deserialize, Serialize};
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

/// How long this process has been answering, in seconds.
pub fn uptime_seconds() -> u64 {
    STARTED.elapsed().max(Duration::ZERO).as_secs()
}

pub fn router() -> OpenApiRouter<AppState> {
    LazyLock::force(&STARTED);
    OpenApiRouter::new()
        .routes(routes!(figures))
        .routes(routes!(health))
        .routes(routes!(metrics))
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    pub name: String,
    pub count: i64,
}

#[derive(Serialize, Deserialize, ToSchema)]
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

    // Read across the whole catalogue, by anyone the site is open to, so
    // kept a while: under a mark of the catalogue, so a work stored, changed
    // or listed again is counted at once, and under the settings generation
    // and the adult decision, which say what is counted. An empty catalogue's
    // figures are not kept, as the facets' are not.
    let key = format!(
        "figures:{adult}:{}:{}",
        state.caches.stamp(),
        repo::item::catalogue_stamp(&state.db).await?
    );
    if let Some(cached) = state.caches.searches.get(&key).await
        && let Ok(figures) = serde_json::from_str::<Figures>(&cached)
    {
        return Ok(Json(figures));
    }

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

    let figures = Figures {
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
    };
    if figures.total > 0
        && let Ok(encoded) = serde_json::to_string(&figures)
    {
        state.caches.searches.insert(key, encoded).await;
    }
    Ok(Json(figures))
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
    /// One instance, or several.
    pub mode: crate::config::Mode,
    /// The instance that answered, by name.
    pub instance: String,
    /// Whether it runs the schedules.
    pub leads: bool,
    /// Which instance does, by name; none while no lease is held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// How many instances were heard of lately, this one included.
    pub instances: usize,
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
    let cache = |c: &crate::cache::Space<String>| CacheFigures {
        entries: c.l1_entries(),
        bytes: c.l1_bytes(),
    };
    Ok(Json(Health {
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: STARTED.elapsed().max(Duration::ZERO).as_secs(),
        database: match state.db.dialect() {
            crate::db::Dialect::Sqlite => "sqlite",
            crate::db::Dialect::Postgres => "postgres",
        },
        mode: state.coord.mode(),
        instance: state.coord.instance.name.clone(),
        leads: state.coord.leads(),
        leader: state.coord.leader(),
        instances: state.coord.instances().len(),
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
            entries: state.caches.lists.l1_entries(),
            bytes: state.caches.lists.l1_bytes(),
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
                name: "fankai",
                on: state.flag("fankai.enabled", false),
            },
            Source {
                name: "fankaiwiki",
                on: state.flag("fankai.enabled", false) && state.flag("fankai.wiki", false),
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
                limit: 8,
                ..Default::default()
            },
        )
        .await?,
    }))
}

/// What the server counts about itself, in the text Prometheus reads:
/// requests by surface, calls upstream by provider, what the caches keep,
/// what the catalogue holds, what the jobs did, and how long it has been up.
/// For an administrator, or a key with the `admin` scope — which a scraper
/// sends as `Authorization: Bearer`. What the database holds is left out of
/// a scrape it cannot answer, rather than the whole scrape failing when it
/// matters most.
#[utoipa::path(
    get, path = "/admin/metrics", tag = TAG,
    responses(
        (status = 200, description = "The Prometheus text exposition", content_type = "text/plain"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn metrics(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<([(header::HeaderName, &'static str); 1], String)> {
    if !identity.is_admin() {
        return Err(AppError::Forbidden);
    }
    use crate::metrics::{Metric, label};

    let mut gauges = match held(&state).await {
        Ok(gauges) => gauges,
        Err(e) => {
            tracing::warn!(error = %e, "the scrape goes without what the database holds");
            Vec::new()
        }
    };
    let c = &state.caches;
    let caches: [(&str, u64, u64, crate::cache::Tally, crate::cache::Tally); 4] = [
        (
            "items",
            c.items.l1_entries(),
            c.items.l1_bytes(),
            c.items.l1_tally(),
            c.items.l2_tally(),
        ),
        (
            "searches",
            c.searches.l1_entries(),
            c.searches.l1_bytes(),
            c.searches.l1_tally(),
            c.searches.l2_tally(),
        ),
        (
            "lists",
            c.lists.l1_entries(),
            c.lists.l1_bytes(),
            c.lists.l1_tally(),
            c.lists.l2_tally(),
        ),
        (
            "relay",
            c.relay.l1_entries(),
            c.relay.l1_bytes(),
            c.relay.l1_tally(),
            c.relay.l2_tally(),
        ),
    ];
    gauges.push(Metric {
        name: "ams_cache_entries",
        help: "Entries kept in each cache, in this process's memory.",
        samples: caches
            .iter()
            .map(|(name, entries, ..)| (format!("cache=\"{name}\""), *entries as f64))
            .chain([("cache=\"sessions\"".to_string(), c.session_entries() as f64)])
            .collect(),
    });
    gauges.push(Metric {
        name: "ams_cache_bytes",
        help: "Bytes kept in each cache, in this process's memory.",
        samples: caches
            .iter()
            .map(|(name, _, bytes, ..)| (format!("cache=\"{name}\""), *bytes as f64))
            .collect(),
    });
    let mut hits = Vec::new();
    let mut misses = Vec::new();
    for (name, _, _, l1, l2) in &caches {
        hits.push((format!("cache=\"{name}\",tier=\"memory\""), l1.hits as f64));
        misses.push((
            format!("cache=\"{name}\",tier=\"memory\""),
            l1.misses as f64,
        ));
        hits.push((format!("cache=\"{name}\",tier=\"server\""), l2.hits as f64));
        misses.push((
            format!("cache=\"{name}\",tier=\"server\""),
            l2.misses as f64,
        ));
    }
    let sessions = c.session_tally();
    hits.push((
        "cache=\"sessions\",tier=\"memory\"".to_string(),
        sessions.hits as f64,
    ));
    misses.push((
        "cache=\"sessions\",tier=\"memory\"".to_string(),
        sessions.misses as f64,
    ));
    gauges.push(Metric {
        name: "ams_cache_hits_total",
        help: "Reads each cache answered, by tier, since the start.",
        samples: hits,
    });
    gauges.push(Metric {
        name: "ams_cache_misses_total",
        help: "Reads each cache could not answer, by tier, since the start.",
        samples: misses,
    });
    if c.wants_redis() {
        let redis = c.redis();
        let up = redis.as_ref().is_some_and(|r| r.is_up());
        gauges.push(Metric {
            name: "ams_redis_up",
            help: "Whether the cache server answers: 1 when it does.",
            samples: vec![(String::new(), if up { 1.0 } else { 0.0 })],
        });
        if let Some(redis) = redis {
            gauges.push(Metric {
                name: "ams_redis_latency_seconds",
                help: "The cache server's last round trip.",
                samples: vec![(String::new(), redis.latency().as_secs_f64())],
            });
            gauges.push(Metric {
                name: "ams_redis_errors_total",
                help: "Commands the cache server did not answer, since the start.",
                samples: vec![(
                    String::new(),
                    redis.errors.load(std::sync::atomic::Ordering::Relaxed) as f64,
                )],
            });
            if let Some(info) = redis.info().await {
                for (name, help, value) in [
                    (
                        "ams_redis_used_memory_bytes",
                        "Memory the cache server uses.",
                        info.used_memory,
                    ),
                    (
                        "ams_redis_maxmemory_bytes",
                        "The cache server's memory ceiling; 0 when none.",
                        info.maxmemory,
                    ),
                    (
                        "ams_redis_evicted_keys_total",
                        "Keys the cache server evicted for room.",
                        info.evicted_keys,
                    ),
                    (
                        "ams_redis_connected_clients",
                        "Clients the cache server has.",
                        info.connected_clients,
                    ),
                ] {
                    gauges.push(Metric {
                        name,
                        help,
                        samples: vec![(String::new(), value as f64)],
                    });
                }
            }
        }
    }
    gauges.push(Metric {
        name: "ams_uptime_seconds",
        help: "Seconds since the server started.",
        samples: vec![(String::new(), STARTED.elapsed().as_secs_f64())],
    });
    gauges.push(Metric {
        name: "ams_leader",
        help: "Whether this instance runs the schedules: 1 when it leads.",
        samples: vec![(
            format!("ams_instance=\"{}\"", label(&state.coord.instance.name)),
            if state.coord.leads() { 1.0 } else { 0.0 },
        )],
    });
    gauges.push(Metric {
        name: "ams_instances",
        help: "Instances of this server heard of lately, this one included.",
        samples: vec![(String::new(), state.coord.instances().len() as f64)],
    });
    gauges.push(Metric {
        name: "ams_build_info",
        help: "The version running; always 1.",
        samples: vec![(
            format!("version=\"{}\"", label(env!("CARGO_PKG_VERSION"))),
            1.0,
        )],
    });

    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        crate::metrics::render(&gauges),
    ))
}

/// What the database holds, as gauges: works, episodes, locks, job runs.
async fn held(state: &AppState) -> anyhow::Result<Vec<crate::metrics::Metric>> {
    use crate::metrics::{Metric, label};

    let everything = repo::item::Query {
        include_adult: true,
        include_disabled: true,
        ..Default::default()
    };
    let of = |kind: MediaKind| repo::item::Query {
        kind: Some(kind),
        ..everything.clone()
    };
    let series = repo::item::count_matching(&state.db, &of(MediaKind::Series)).await?;
    let movies = repo::item::count_matching(&state.db, &of(MediaKind::Movie)).await?;
    let episodes = repo::item::episode_count(&state.db, true, true).await?;
    let locks = repo::override_field::count(&state.db).await?;
    let runs = repo::job::counts(&state.db).await?;

    Ok(vec![
        Metric {
            name: "ams_works",
            help: "Works held, by kind.",
            samples: vec![
                ("kind=\"series\"".into(), series as f64),
                ("kind=\"movie\"".into(), movies as f64),
            ],
        },
        Metric {
            name: "ams_episodes",
            help: "Episodes held.",
            samples: vec![(String::new(), episodes as f64)],
        },
        Metric {
            name: "ams_locks",
            help: "Fields locked by a manual edit.",
            samples: vec![(String::new(), locks as f64)],
        },
        Metric {
            name: "ams_job_runs",
            help: "Job runs kept in the log, by kind and status.",
            samples: runs
                .into_iter()
                .map(|(kind, status, n)| {
                    (
                        format!("kind=\"{}\",status=\"{}\"", label(&kind), label(&status)),
                        n as f64,
                    )
                })
                .collect(),
        },
    ])
}
