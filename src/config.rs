//! Typed configuration, built once at startup from the process environment.
//!
//! Every knob is prefixed `AMS_`. Legacy names from the two projects this server
//! replaces (`TMDB_API_KEY`, `BIND_ADDRESS`, …) are still accepted as fallbacks so
//! an existing `.env` keeps working.

use std::{net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{Context, Result, bail};
use ipnet::IpNet;

/// Which authentication a given API surface enforces.
///
/// Some clients cannot present a credential at all: Sonarr and Radarr hard-code
/// their metadata URLs, and most TMDB clients compile their key in. Those are
/// guarded by network policy instead — see [`Security::allowlist`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfacePolicy {
    /// Requires a valid API key (header, or `api_key`/`apikey` query parameter).
    ApiKey,
    /// Requires the peer address to match the allowlist.
    Allowlist,
    /// Open. Only ever selected explicitly.
    Open,
}

impl FromStr for SurfacePolicy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "apikey" | "api_key" | "key" => Ok(Self::ApiKey),
            "allowlist" | "ip" | "cidr" => Ok(Self::Allowlist),
            "open" | "none" | "disabled" => Ok(Self::Open),
            other => bail!("invalid surface policy {other:?} (expected apikey|allowlist|open)"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub server: Server,
    pub database: Database,
    pub security: Security,
    pub tmdb: Tmdb,
    pub skyhook: Skyhook,
    pub radarr_metadata: RadarrMetadata,
    pub fanart: Fanart,
    pub tvdb: Tvdb,
    /// Which provider wins when two disagree, most trusted first.
    pub provider_priority: Vec<String>,
    pub cache: Cache,
    pub refresh: Refresh,
    pub export: Export,
}

#[derive(Clone, Debug)]
pub struct Server {
    pub bind: SocketAddr,
    /// Absolute URL the server is reachable at, used to rewrite image URLs it proxies.
    pub public_url: Option<String>,
    pub tls: Option<Tls>,
    pub request_timeout: Duration,
    /// Origins allowed by CORS on the native API. Empty means same-origin only.
    pub cors_origins: Vec<String>,
    /// Networks whose `X-Forwarded-For` is honoured when resolving the peer address.
    pub trusted_proxies: Vec<IpNet>,
}

#[derive(Clone, Debug)]
pub struct Tls {
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Database {
    pub url: String,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct Security {
    /// Master switch. When true every surface becomes [`SurfacePolicy::Open`].
    pub auth_disabled: bool,
    /// Let anyone read the catalogue, with no credential.
    ///
    /// Off by default: turning it on publishes what this server knows to
    /// whoever can reach the port. It grants a fixed list of read-only paths
    /// and nothing else — see `crate::auth::middleware::browsable`.
    pub public_browse: bool,
    pub native_policy: SurfacePolicy,
    pub tmdb_policy: SurfacePolicy,
    pub arr_policy: SurfacePolicy,
    /// Peers allowed to reach any surface whose policy is
    /// [`SurfacePolicy::Allowlist`].
    ///
    /// Not only the arr surfaces: a TMDB client whose key is compiled in — which
    /// is most of them — can only be let through by address either.
    pub allowlist: Vec<IpNet>,
    /// Bootstrap administrator, created on first start when no admin exists.
    pub bootstrap_admin: Option<(String, String)>,
    /// Requests per minute per peer on the native API. `0` disables the limiter.
    pub rate_limit_per_minute: u32,
    /// Days of audit history to keep. `0` keeps everything.
    pub audit_retention_days: u32,
}

#[derive(Clone, Debug)]
pub struct Tmdb {
    pub api_key: Option<String>,
    pub upstream: String,
    pub language: String,
    pub include_adult: bool,
    /// How many search hits get enriched with external ids before being returned.
    pub search_limit: usize,
    /// Whether unmatched `/3/*` requests are forwarded upstream.
    pub passthrough: bool,
}

#[derive(Clone, Debug)]
pub struct Skyhook {
    pub upstream: String,
    /// Answer from the real skyhook.sonarr.tv when nothing else can.
    pub fallback: bool,
    /// Also merge its data into every series, not only the ones nothing else
    /// could answer. Costs one call per refresh and fills gaps TMDB leaves —
    /// air time, TVMaze and AniList ids, and episode ordering hints.
    pub enrich: bool,
}

#[derive(Clone, Debug)]
pub struct Tvdb {
    pub upstream: String,
    /// Without a key the provider is skipped.
    pub api_key: Option<String>,
    /// Only a subscriber key needs one; a project key must not send it.
    pub pin: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct Fanart {
    pub upstream: String,
    /// Without a key the provider is simply skipped.
    pub api_key: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct RadarrMetadata {
    pub upstream: String,
    /// Answer from the real api.radarr.video when nothing else can.
    pub fallback: bool,
    /// Also merge its data into every movie. It is a curated view of TMDB with
    /// certifications and extra ratings already resolved.
    pub enrich: bool,
}

#[derive(Clone, Debug)]
pub struct Cache {
    pub max_entries: u64,
    pub item_ttl: Duration,
    pub search_ttl: Duration,
}

#[derive(Clone, Debug)]
pub struct Export {
    /// Where `POST /api/v1/export/nfo` writes. Unset disables that endpoint.
    pub nfo_path: Option<PathBuf>,
    /// Also download the artwork the documents point at, beside them.
    ///
    /// On, because a `.nfo` whose pictures are remote URLs is half an export:
    /// Kodi fetches them, Plex's Personal Media agent often does not. Off is for
    /// someone who only wants the text, or has no room for a library's artwork.
    pub artwork: bool,
}

#[derive(Clone, Debug)]
pub struct Refresh {
    pub enabled: bool,
    /// How often the scheduler looks for stale entries.
    pub interval: Duration,
    /// Age past which a still-running series is refetched.
    pub continuing_ttl: Duration,
    /// Age past which an ended series or released movie is refetched.
    pub ended_ttl: Duration,
    /// Entries refreshed per scheduler tick.
    pub batch_size: u32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            server: Server {
                bind: var_or(
                    &["AMS_BIND_ADDRESS", "BIND_ADDRESS", "LISTEN_ADDR"],
                    "0.0.0.0:8080",
                )
                .parse()
                .context("AMS_BIND_ADDRESS is not a valid socket address")?,
                public_url: opt(&["AMS_PUBLIC_URL"]).map(|u| u.trim_end_matches('/').to_string()),
                tls: match (
                    opt(&["AMS_TLS_CERT", "LEGACY_TLS_CERT"]),
                    opt(&["AMS_TLS_KEY", "LEGACY_TLS_KEY"]),
                ) {
                    (Some(cert), Some(key)) => Some(Tls {
                        cert: cert.into(),
                        key: key.into(),
                    }),
                    (None, None) => None,
                    _ => bail!("AMS_TLS_CERT and AMS_TLS_KEY must be set together"),
                },
                request_timeout: secs(&["AMS_REQUEST_TIMEOUT"], 60)?,
                cors_origins: list(&["AMS_CORS_ORIGINS"]),
                trusted_proxies: nets(&["AMS_TRUSTED_PROXIES"], &[])?,
            },
            database: Database {
                url: var_or(
                    &["AMS_DATABASE_URL", "DATABASE_URL"],
                    "sqlite://data/ams.db?mode=rwc",
                ),
                max_connections: num(&["AMS_DATABASE_MAX_CONNECTIONS"], 10)?,
                acquire_timeout: secs(&["AMS_DATABASE_ACQUIRE_TIMEOUT"], 30)?,
            },
            security: Security {
                auth_disabled: flag(&["AMS_AUTH_DISABLED"], false)?,
                public_browse: flag(&["AMS_PUBLIC_BROWSE"], false)?,
                native_policy: policy(&["AMS_NATIVE_AUTH"], SurfacePolicy::ApiKey)?,
                tmdb_policy: policy(&["AMS_TMDB_AUTH"], SurfacePolicy::ApiKey)?,
                arr_policy: policy(&["AMS_ARR_AUTH"], SurfacePolicy::Allowlist)?,
                allowlist: nets(
                    &["AMS_ALLOWLIST", "AMS_ARR_ALLOWLIST"],
                    // RFC1918 + loopback + the usual container ranges.
                    &[
                        "127.0.0.0/8",
                        "::1/128",
                        "10.0.0.0/8",
                        "172.16.0.0/12",
                        "192.168.0.0/16",
                        "fc00::/7",
                    ],
                )?,
                bootstrap_admin: match (opt(&["AMS_ADMIN_USERNAME"]), opt(&["AMS_ADMIN_PASSWORD"]))
                {
                    (Some(u), Some(p)) => Some((u, p)),
                    (None, None) => None,
                    _ => bail!("AMS_ADMIN_USERNAME and AMS_ADMIN_PASSWORD must be set together"),
                },
                rate_limit_per_minute: num(&["AMS_RATE_LIMIT_PER_MINUTE"], 600)?,
                audit_retention_days: num(&["AMS_AUDIT_RETENTION_DAYS"], 90)?,
            },
            tmdb: Tmdb {
                api_key: opt(&["AMS_TMDB_API_KEY", "TMDB_API_KEY"]),
                upstream: var_or(
                    &["AMS_TMDB_UPSTREAM", "TMDB_UPSTREAM"],
                    "https://api.themoviedb.org",
                )
                .trim_end_matches('/')
                .to_string(),
                language: var_or(&["AMS_TMDB_LANGUAGE", "TMDB_LANGUAGE"], "en-US"),
                include_adult: flag(&["AMS_TMDB_INCLUDE_ADULT", "TMDB_INCLUDE_ADULT"], false)?,
                search_limit: num::<usize>(&["AMS_TMDB_SEARCH_LIMIT", "TMDB_SEARCH_LIMIT"], 10)?,
                passthrough: flag(&["AMS_TMDB_PASSTHROUGH"], true)?,
            },
            skyhook: Skyhook {
                upstream: var_or(
                    &["AMS_SKYHOOK_UPSTREAM", "SKYHOOK_BASE_URL"],
                    "https://skyhook.sonarr.tv",
                )
                .trim_end_matches('/')
                .to_string(),
                fallback: flag(&["AMS_SKYHOOK_FALLBACK"], true)?,
                enrich: flag(&["AMS_SKYHOOK_ENRICH"], true)?,
            },
            radarr_metadata: RadarrMetadata {
                upstream: var_or(
                    &["AMS_RADARR_METADATA_UPSTREAM"],
                    "https://api.radarr.video",
                )
                .trim_end_matches('/')
                .to_string(),
                fallback: flag(&["AMS_RADARR_METADATA_FALLBACK"], true)?,
                enrich: flag(&["AMS_RADARR_METADATA_ENRICH"], true)?,
            },
            fanart: Fanart {
                upstream: var_or(&["AMS_FANART_UPSTREAM"], "https://webservice.fanart.tv/v3")
                    .trim_end_matches('/')
                    .to_string(),
                api_key: opt(&["AMS_FANART_API_KEY", "FANARTTV_API_KEY"]),
                enabled: flag(&["AMS_FANART_ENABLED"], true)?,
            },
            tvdb: Tvdb {
                upstream: var_or(&["AMS_TVDB_UPSTREAM"], "https://api4.thetvdb.com/v4")
                    .trim_end_matches('/')
                    .to_string(),
                api_key: opt(&["AMS_TVDB_API_KEY", "TVDB_API_KEY"]),
                pin: opt(&["AMS_TVDB_PIN"]),
                enabled: flag(&["AMS_TVDB_ENABLED"], true)?,
            },
            provider_priority: {
                let configured = list(&["AMS_PROVIDER_PRIORITY"]);
                if configured.is_empty() {
                    // TMDB first because it is the broadest and the one holding a
                    // key; the arr services then fill what it leaves; artwork
                    // providers last, since they only ever add images.
                    ["tmdb", "tvdb", "skyhook", "radarr", "fanart"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                } else {
                    configured
                }
            },
            cache: Cache {
                max_entries: num(&["AMS_CACHE_MAX_ENTRIES"], 10_000)?,
                item_ttl: secs(&["AMS_CACHE_ITEM_TTL", "REDIS_TTL"], 3_600)?,
                search_ttl: secs(&["AMS_CACHE_SEARCH_TTL", "REDIS_SEARCH_TTL"], 1_800)?,
            },
            export: Export {
                nfo_path: opt(&["AMS_NFO_EXPORT_PATH"]).map(PathBuf::from),
                artwork: flag(&["AMS_NFO_EXPORT_ARTWORK"], true)?,
            },
            refresh: Refresh {
                enabled: flag(&["AMS_REFRESH_ENABLED"], true)?,
                interval: secs(&["AMS_REFRESH_INTERVAL"], 900)?,
                continuing_ttl: secs(&["AMS_REFRESH_CONTINUING_TTL"], 21_600)?,
                ended_ttl: secs(&["AMS_REFRESH_ENDED_TTL"], 604_800)?,
                batch_size: num(&["AMS_REFRESH_BATCH_SIZE"], 25)?,
            },
        })
    }

    /// Effective policy for a surface, after the global kill switch.
    pub fn policy_for(&self, surface: Surface) -> SurfacePolicy {
        if self.security.auth_disabled {
            return SurfacePolicy::Open;
        }
        match surface {
            Surface::Native => self.security.native_policy,
            Surface::Tmdb => self.security.tmdb_policy,
            Surface::Arr => self.security.arr_policy,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// `/api/v1/*` — the server's own API and the web UI's backend.
    Native,
    /// `/3/*` — TMDB-compatible surface.
    Tmdb,
    /// `/v1/tvdb/*` and `/v1/movie/*` — Sonarr and Radarr compatibility.
    Arr,
}

// ─── env helpers ─────────────────────────────────────────────────────────────

/// First non-empty value among `keys`.
fn opt(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn var_or(keys: &[&str], default: &str) -> String {
    opt(keys).unwrap_or_else(|| default.to_string())
}

fn num<T: FromStr>(keys: &[&str], default: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match opt(keys) {
        None => Ok(default),
        Some(v) => v
            .parse()
            .map_err(|e| anyhow::anyhow!("{} is not a valid number: {e}", keys[0])),
    }
}

fn flag(keys: &[&str], default: bool) -> Result<bool> {
    match opt(keys) {
        None => Ok(default),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => bail!("{} is not a valid boolean: {other:?}", keys[0]),
        },
    }
}

fn secs(keys: &[&str], default: u64) -> Result<Duration> {
    Ok(Duration::from_secs(num(keys, default)?))
}

/// Comma-separated list, empty entries dropped.
fn list(keys: &[&str]) -> Vec<String> {
    opt(keys)
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Comma-separated CIDRs. A bare address is accepted and treated as a /32 or /128.
fn nets(keys: &[&str], defaults: &[&str]) -> Result<Vec<IpNet>> {
    let raw = list(keys);
    let source: Vec<String> = if raw.is_empty() {
        defaults.iter().map(|s| s.to_string()).collect()
    } else {
        raw
    };

    source
        .iter()
        .map(|s| {
            s.parse::<IpNet>()
                .or_else(|_| s.parse::<std::net::IpAddr>().map(IpNet::from))
                .with_context(|| format!("{} contains an invalid network: {s:?}", keys[0]))
        })
        .collect()
}

fn policy(keys: &[&str], default: SurfacePolicy) -> Result<SurfacePolicy> {
    match opt(keys) {
        None => Ok(default),
        Some(v) => v.parse(),
    }
}
