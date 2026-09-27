//! The caches, as an administrator reads them: what each space holds in
//! each tier and how often it answered, the server behind the second tier
//! and how it is doing — and the way to forget a space, or all of them.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    cache::{self, Tally},
    db::repo::audit::Action,
    error::{AppError, AppResult},
    state::AppState,
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(report))
        .routes(routes!(flush_all))
        .routes(routes!(flush_space))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub server: Server,
    pub spaces: Vec<SpaceReport>,
    /// The generation every search is filed under.
    pub generation: u64,
    /// The epoch every list is filed under: moved on by every write.
    pub epoch: u64,
    /// How long a public page may be kept by a browser or a proxy; 0 when
    /// the interface's `no-cache` stands.
    pub public_seconds: u64,
    /// This instance among the others, and who leads.
    pub instances: Instances,
}

/// The instances of this server: one, or several coordinating through
/// the cache server.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Instances {
    pub mode: crate::config::Mode,
    /// The instance that answered this request.
    pub this: crate::coord::Instance,
    /// Whether it is the one running the schedules.
    pub leads: bool,
    /// Which instance leads, by name; none while no lease is held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// Every instance heard of lately, this one included.
    pub all: Vec<crate::coord::Announced>,
}

/// The server behind the second tier.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    /// Whether AMS_REDIS_URL names one at all.
    pub configured: bool,
    /// Whether it is attached: reached at least once since the start.
    pub attached: bool,
    /// Whether it answered the last command.
    pub up: bool,
    /// Its address, without the password.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub errors: u64,
    /// Keys under this server's prefix, all spaces together.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<cache::redis::Info>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceReport {
    pub id: &'static str,
    pub enabled: bool,
    pub ttl_seconds: u64,
    pub memory: Memory,
    /// None for a space kept in memory alone, or without a server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<Shared>,
}

/// The first tier: this process's memory.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub entries: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(flatten)]
    pub tally: Tally,
}

/// The second tier: the server, shared.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Shared {
    /// None when the server did not answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<u64>,
    #[serde(flatten)]
    pub tally: Tally,
}

#[utoipa::path(
    get, path = "/admin/cache", tag = TAG,
    responses(
        (status = 200, body = Report),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn report(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Report>> {
    identity.require_admin()?;
    let c = &state.caches;

    let redis = c.redis();
    // One walk of the keyspace for every count the page shows.
    let prefixes: Vec<String> = cache::SPACES
        .iter()
        .map(|space| redis.as_ref().map(|r| r.key(space, "")).unwrap_or_default())
        .collect();
    let counted = match &redis {
        Some(redis) => redis.count_by_prefixes(&redis.prefix, &prefixes).await,
        None => None,
    };
    let keys_of = |space: &str| -> Option<u64> {
        let i = cache::SPACES.iter().position(|s| *s == space)?;
        counted.as_ref().map(|(_, counts)| counts[i])
    };
    let server = match &redis {
        Some(redis) => Server {
            configured: true,
            attached: true,
            up: redis.is_up(),
            address: state
                .config
                .cache
                .redis_url
                .as_ref()
                .map(|url| format!("{url:?}")),
            prefix: redis.prefix.clone(),
            latency_ms: Some(redis.latency().as_secs_f64() * 1000.0),
            last_error: redis.last_error(),
            errors: redis.errors.load(std::sync::atomic::Ordering::Relaxed),
            keys: counted.as_ref().map(|(total, _)| *total),
            info: redis.info().await,
        },
        None => Server {
            configured: c.wants_redis(),
            attached: false,
            up: false,
            address: state
                .config
                .cache
                .redis_url
                .as_ref()
                .map(|url| format!("{url:?}")),
            prefix: state.config.cache.redis_prefix.clone(),
            latency_ms: None,
            last_error: None,
            errors: 0,
            keys: None,
            info: None,
        },
    };

    macro_rules! space {
        ($space:expr) => {{
            let space = &$space;
            space.settle().await;
            SpaceReport {
                id: space.id,
                enabled: space.enabled(),
                ttl_seconds: space.ttl().as_secs(),
                memory: Memory {
                    entries: space.l1_entries(),
                    bytes: Some(space.l1_bytes()),
                    tally: space.l1_tally(),
                },
                server: if space.has_l2() {
                    Some(Shared {
                        keys: keys_of(space.id),
                        tally: space.l2_tally(),
                    })
                } else {
                    None
                },
            }
        }};
    }
    let spaces = vec![
        space!(c.items),
        space!(c.searches),
        space!(c.lists),
        space!(c.relay),
        SpaceReport {
            id: "sessions",
            enabled: c.sessions_enabled(),
            ttl_seconds: c.session_ttl.as_secs(),
            memory: Memory {
                entries: c.session_entries(),
                bytes: None,
                tally: c.session_tally(),
            },
            server: None,
        },
    ];

    Ok(Json(Report {
        server,
        spaces,
        generation: c.generation(),
        epoch: c.epoch(),
        public_seconds: state.config.cache.public_seconds,
        instances: Instances {
            mode: state.coord.mode(),
            this: state.coord.instance.clone(),
            leads: state.coord.leads(),
            leader: state.coord.leader(),
            all: state.coord.instances(),
        },
    }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Flushed {
    /// Which spaces were forgotten.
    pub spaces: Vec<&'static str>,
    /// Keys the server let go of, all spaces together.
    pub keys: u64,
}

/// Forget every space, in every tier: this server's keys on the shared
/// server — never anybody else's — and every instance's memory.
#[utoipa::path(
    post, path = "/admin/cache/flush", tag = TAG,
    responses(
        (status = 200, body = Flushed),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn flush_all(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
) -> AppResult<Json<Flushed>> {
    identity.require_admin()?;
    let mut keys = 0;
    for space in cache::SPACES {
        keys += state.caches.flush(space).await.unwrap_or(0);
    }
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::CacheFlushed,
            target: Some("*"),
            detail: Some(&format!("{keys} keys on the server")),
        },
    )
    .await;
    Ok(Json(Flushed {
        spaces: cache::SPACES.to_vec(),
        keys,
    }))
}

/// Forget one space, in every tier.
#[utoipa::path(
    post, path = "/admin/cache/{space}/flush", tag = TAG,
    params(("space" = String, Path, description = "`items`, `searches`, `lists`, `relay` or `sessions`")),
    responses(
        (status = 200, body = Flushed),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such space"),
    ),
)]
async fn flush_space(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(space): Path<String>,
) -> AppResult<(StatusCode, Json<Flushed>)> {
    identity.require_admin()?;
    let Some(id) = cache::SPACES.iter().find(|s| **s == space) else {
        return Err(AppError::NotFound);
    };
    let keys = state.caches.flush(id).await.unwrap_or(0);
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::CacheFlushed,
            target: Some(id),
            detail: Some(&format!("{keys} keys on the server")),
        },
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(Flushed {
            spaces: vec![id],
            keys,
        }),
    ))
}
