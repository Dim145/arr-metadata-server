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
    pub tvmaze: Tvmaze,
    pub anilist: Anilist,
    pub mal: Mal,
    pub imdb: Imdb,
    pub fankai: Fankai,
    pub fankai_wiki: FankaiWiki,
    pub anime_mapping: AnimeMapping,
    /// Which provider wins when two disagree, most trusted first.
    pub provider_priority: Vec<String>,
    pub cache: Cache,
    pub refresh: Refresh,
    pub export: Export,
    pub media: Media,
    /// The clients' door: a second listener, in TLS, under the names Sonarr,
    /// Radarr and the TMDB clients have compiled in. None when unset.
    pub clients: Option<ClientsDoor>,
}

#[derive(Clone, Debug)]
pub struct Server {
    pub bind: SocketAddr,
    /// Absolute URL the server is reachable at: what the media it keeps are
    /// addressed by in what it serves, and where a sign-in or an invitation
    /// leads back to.
    pub public_url: Option<String>,
    pub tls: Option<Tls>,
    pub request_timeout: Duration,
    /// Origins allowed by CORS on the native API. Empty means same-origin only.
    pub cors_origins: Vec<String>,
    /// Networks whose `X-Forwarded-For` is honoured when resolving the peer address.
    pub trusted_proxies: Vec<IpNet>,
    /// Hostnames this server answers to. Empty means it answers to any.
    ///
    /// The defence against DNS rebinding, and the only one there is: a name
    /// with a one-second TTL that resolves to `evil.com` and then to this
    /// server's address makes the victim's own browser treat `http://evil.com/`
    /// as *same-origin* with it, so CORS never applies. With the default
    /// `apikey` policy the attacker's script still has no credential; with
    /// `allowlist` — a natural choice on a home network — the browser is
    /// calling from an allowed address, and every read is theirs.
    ///
    /// Empty by default because there is no safe guess: this is reached by
    /// container name, by LAN address, by whatever the router calls it. Naming
    /// them is the operator's to do.
    pub allowed_hosts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Tls {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// The clients' listener. Independent of the interface's: that one may stay
/// plain, behind a reverse proxy or on a home network, while this one is
/// always in TLS, since the clients call `https://…` and nothing else.
#[derive(Clone, Debug)]
pub struct ClientsDoor {
    pub bind: SocketAddr,
    /// The operator's own certificate and key, instead of the authority this
    /// server keeps.
    pub tls: Option<Tls>,
    /// Names the listener answers to besides the compiled-in ones — and,
    /// with the authority, names its certificate carries.
    pub names: Vec<String>,
    /// Where the authority and the certificate it issues are kept.
    pub dir: PathBuf,
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
    /// `AMS_PUBLIC_BROWSE` when the environment names it at all. It gives the
    /// `site.access` setting its first value, and the setting decides after
    /// that — except that an explicit `false` keeps the site private whatever
    /// the setting says: closing the catalogue from the environment must not
    /// be undone by a value stored before.
    pub public_browse_env: Option<bool>,
    /// Accounts one address, and the whole server, may open in an hour
    /// without an invitation.
    pub signups_per_hour: usize,
    pub signups_per_hour_total: usize,
    /// The identity provider's client secret, when the environment holds it
    /// rather than the settings: it wins over a stored one, and keeps the
    /// secret out of the database and its backups.
    pub oidc_client_secret: Option<String>,
    /// AMS_FORCE_PASSWORD_LOGIN: passwords stay on whatever the settings say —
    /// the way back in when the identity provider fails and nobody kept a
    /// door.
    pub force_password_login: bool,
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

/// TVmaze: exact broadcast times, for series. No key.
#[derive(Clone, Debug)]
pub struct Tvmaze {
    pub upstream: String,
    /// Seeds the `tvmaze.enabled` setting once; the interface decides after.
    pub enabled: bool,
}

/// Fankai: the Fan-Kai productions, from their own metadata service. No key.
#[derive(Clone, Debug)]
pub struct Fankai {
    pub upstream: String,
    /// Seeds the `fankai.enabled` setting once; the interface decides after.
    pub enabled: bool,
}

/// The Fankai wiki: which anime each Fan-Kai was cut from. No key.
#[derive(Clone, Debug)]
pub struct FankaiWiki {
    /// MediaWiki's API for the wiki, `…/api.php`.
    pub upstream: String,
    /// Seeds the `fankai.wiki` setting once; the interface decides after.
    pub enabled: bool,
}

/// AniList: scores, titles and flags for anime. No key.
#[derive(Clone, Debug)]
pub struct Anilist {
    pub upstream: String,
    pub enabled: bool,
}

/// MyAnimeList: its official API when a client id is set, Jikan otherwise.
#[derive(Clone, Debug)]
pub struct Mal {
    pub upstream: String,
    pub jikan_upstream: String,
    /// Free from myanimelist.net's API settings. Without one, Jikan is used —
    /// an unofficial mirror that serves MAL from its own cache, which can be
    /// weeks old and fails outright when MyAnimeList refuses it.
    pub client_id: Option<String>,
    pub enabled: bool,
}

/// IMDb's own non-commercial datasets, for ratings. No key, no API.
#[derive(Clone, Debug)]
pub struct Imdb {
    pub datasets: String,
    pub enabled: bool,
}

/// Which AniList and MyAnimeList entries a TheTVDB or TMDB id is, so anime is
/// looked up by identifier rather than guessed at by title.
#[derive(Clone, Debug)]
pub struct AnimeMapping {
    pub url: String,
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
    /// How long a session's token is remembered before the database is
    /// asked again: a sign-out or a change of role is felt at once anyway.
    pub session_ttl: Duration,
    /// The server behind the second tier — Valkey, Redis or compatible —
    /// as `redis://[user:password@]host:port[/db]` or `rediss://`. None
    /// keeps the caches to memory.
    pub redis_url: Option<RedisUrl>,
    /// What every key this server writes there begins with.
    pub redis_prefix: String,
    /// How long one command may take before the answer is "nothing".
    pub redis_timeout: Duration,
    /// How long a public page of the catalogue may be kept by a browser or
    /// a proxy, in seconds; zero keeps the interface's `no-cache`.
    pub public_seconds: u64,
}

/// A cache server's address, which may carry a password: printed without it.
#[derive(Clone)]
pub struct RedisUrl(pub String);

impl std::fmt::Debug for RedisUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match url::Url::parse(&self.0) {
            Ok(mut url) => {
                if url.password().is_some() {
                    let _ = url.set_password(Some("***"));
                }
                // A query can carry a password too (`?pass=`): none is shown.
                url.set_query(None);
                write!(f, "{url}")
            }
            Err(_) => f.write_str("<invalid>"),
        }
    }
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

/// Where the media a work points at — its pictures, a Fan-Kai's theme — are
/// kept once fetched, so the catalogue reads without its providers.
#[derive(Clone, Debug)]
pub struct Media {
    pub storage: MediaStorage,
    /// The directory, when the storage is the filesystem.
    pub dir: PathBuf,
    /// The bucket, when it is S3 or something that speaks it.
    pub s3: Option<S3>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaStorage {
    /// Nothing is kept: every picture stays a link to its provider.
    Off,
    /// Files under `AMS_MEDIA_DIR`, served by this server.
    Filesystem,
    /// An S3 bucket, served through this server or by the bucket itself.
    S3,
}

impl FromStr for MediaStorage {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "" => Ok(Self::Off),
            "filesystem" | "fs" | "file" | "local" => Ok(Self::Filesystem),
            "s3" => Ok(Self::S3),
            other => bail!("AMS_MEDIA_STORAGE must be off, filesystem or s3, not {other:?}"),
        }
    }
}

#[derive(Clone)]
pub struct S3 {
    /// Unset for Amazon's own; set for anything else that speaks S3 —
    /// Garage, MinIO, RustFS, Ceph — as `http(s)://host:port`.
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    /// Under which the keys are filed, when the bucket holds other things.
    pub prefix: Option<String>,
    /// `bucket` in the path rather than as a subdomain: what the compatible
    /// servers speak, and what a bucket name with a dot needs.
    pub path_style: bool,
}

/// Written by hand so that no log line, however it came to print the
/// configuration, can carry the secret.
impl std::fmt::Debug for S3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key", &self.access_key)
            .field("secret_key", &"…")
            .field("prefix", &self.prefix)
            .field("path_style", &self.path_style)
            .finish()
    }
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
                allowed_hosts: list(&["AMS_ALLOWED_HOSTS"])
                    .into_iter()
                    .map(|h| h.trim().to_ascii_lowercase())
                    .filter(|h| !h.is_empty())
                    .collect(),
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
                public_browse_env: match opt(&["AMS_PUBLIC_BROWSE"]) {
                    Some(_) => Some(flag(&["AMS_PUBLIC_BROWSE"], false)?),
                    None => None,
                },
                signups_per_hour: num(&["AMS_SIGNUPS_PER_HOUR"], 5)?,
                signups_per_hour_total: num(&["AMS_SIGNUPS_PER_HOUR_TOTAL"], 100)?,
                oidc_client_secret: opt(&["AMS_OIDC_CLIENT_SECRET"]),
                force_password_login: flag(&["AMS_FORCE_PASSWORD_LOGIN"], false)?,
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
            // The sources below are off until somebody turns them on: each is a
            // new party this server talks to, and that is the operator's call.
            tvmaze: Tvmaze {
                upstream: var_or(&["AMS_TVMAZE_UPSTREAM"], "https://api.tvmaze.com")
                    .trim_end_matches('/')
                    .to_string(),
                enabled: flag(&["AMS_TVMAZE_ENABLED"], false)?,
            },
            fankai: Fankai {
                upstream: var_or(&["AMS_FANKAI_UPSTREAM"], "https://metadata.fankai.fr")
                    .trim_end_matches('/')
                    .to_string(),
                enabled: flag(&["AMS_FANKAI_ENABLED"], false)?,
            },
            fankai_wiki: FankaiWiki {
                upstream: var_or(
                    &["AMS_FANKAI_WIKI_UPSTREAM"],
                    "https://fan-kai.fandom.com/fr/api.php",
                ),
                enabled: flag(&["AMS_FANKAI_WIKI_ENABLED"], false)?,
            },
            anilist: Anilist {
                upstream: var_or(&["AMS_ANILIST_UPSTREAM"], "https://graphql.anilist.co")
                    .trim_end_matches('/')
                    .to_string(),
                enabled: flag(&["AMS_ANILIST_ENABLED"], false)?,
            },
            mal: Mal {
                upstream: var_or(&["AMS_MAL_UPSTREAM"], "https://api.myanimelist.net/v2")
                    .trim_end_matches('/')
                    .to_string(),
                jikan_upstream: var_or(&["AMS_JIKAN_UPSTREAM"], "https://api.jikan.moe/v4")
                    .trim_end_matches('/')
                    .to_string(),
                client_id: opt(&["AMS_MAL_CLIENT_ID", "MAL_CLIENT_ID"]),
                enabled: flag(&["AMS_MAL_ENABLED"], false)?,
            },
            imdb: Imdb {
                datasets: var_or(&["AMS_IMDB_DATASETS"], "https://datasets.imdbws.com")
                    .trim_end_matches('/')
                    .to_string(),
                enabled: flag(&["AMS_IMDB_ENABLED"], false)?,
            },
            anime_mapping: AnimeMapping {
                url: var_or(
                    &["AMS_ANIME_MAPPING_URL"],
                    "https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-full.json",
                ),
            },
            provider_priority: {
                let configured = list(&["AMS_PROVIDER_PRIORITY"]);
                if configured.is_empty() {
                    // TMDB first because it is the broadest and the one holding a
                    // key; the arr services then fill what it leaves; artwork
                    // providers last, since they only ever add images.
                    [
                        "tmdb",
                        "tvdb",
                        "skyhook",
                        "radarr",
                        "fanart",
                        "tvmaze",
                        "anilist",
                        "mal",
                        "fankai",
                        "fankaiwiki",
                    ]
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
                session_ttl: secs(&["AMS_CACHE_SESSION_TTL"], 30)?,
                redis_url: opt(&["AMS_REDIS_URL", "REDIS_URL"])
                    .map(|u| u.trim().to_string())
                    .filter(|u| !u.is_empty())
                    .map(RedisUrl),
                redis_prefix: var_or(&["AMS_REDIS_PREFIX"], "ams:"),
                redis_timeout: Duration::from_millis(num(&["AMS_REDIS_TIMEOUT_MS"], 150)?),
                public_seconds: num(&["AMS_PUBLIC_CACHE_SECONDS"], 60)?,
            },
            export: Export {
                nfo_path: opt(&["AMS_NFO_EXPORT_PATH"]).map(PathBuf::from),
                artwork: flag(&["AMS_NFO_EXPORT_ARTWORK"], true)?,
            },
            media: {
                let storage: MediaStorage = var_or(&["AMS_MEDIA_STORAGE"], "off").parse()?;
                let s3 = if storage == MediaStorage::S3 {
                    let endpoint =
                        opt(&["AMS_S3_ENDPOINT"]).map(|e| e.trim_end_matches('/').to_string());
                    if let Some(endpoint) = &endpoint
                        && !endpoint.starts_with("http://")
                        && !endpoint.starts_with("https://")
                    {
                        bail!("AMS_S3_ENDPOINT must start with http:// or https://");
                    }
                    let need = |key: &'static str| {
                        opt(&[key]).ok_or_else(|| {
                            anyhow::anyhow!("{key} is required when AMS_MEDIA_STORAGE=s3")
                        })
                    };
                    Some(S3 {
                        // A compatible server speaks the path style; Amazon
                        // takes either, and the subdomain is its default.
                        path_style: flag(&["AMS_S3_PATH_STYLE"], endpoint.is_some())?,
                        endpoint,
                        region: var_or(&["AMS_S3_REGION"], "us-east-1"),
                        bucket: need("AMS_S3_BUCKET")?,
                        access_key: need("AMS_S3_ACCESS_KEY")?,
                        secret_key: need("AMS_S3_SECRET_KEY")?,
                        prefix: opt(&["AMS_S3_PREFIX"])
                            .map(|p| p.trim_matches('/').to_string())
                            .filter(|p| !p.is_empty()),
                    })
                } else {
                    None
                };
                Media {
                    storage,
                    dir: PathBuf::from(var_or(&["AMS_MEDIA_DIR"], "data/media")),
                    s3,
                }
            },
            clients: match opt(&["AMS_CLIENTS_BIND"]) {
                Some(bind) => Some(ClientsDoor {
                    bind: bind
                        .parse()
                        .context("AMS_CLIENTS_BIND is not a valid socket address")?,
                    tls: match (
                        opt(&["AMS_CLIENTS_TLS_CERT"]),
                        opt(&["AMS_CLIENTS_TLS_KEY"]),
                    ) {
                        (Some(cert), Some(key)) => Some(Tls {
                            cert: cert.into(),
                            key: key.into(),
                        }),
                        (None, None) => None,
                        _ => bail!(
                            "AMS_CLIENTS_TLS_CERT and AMS_CLIENTS_TLS_KEY must be set together"
                        ),
                    },
                    names: list(&["AMS_CLIENTS_NAMES"]),
                    dir: PathBuf::from(var_or(&["AMS_TLS_DIR"], "data/tls")),
                }),
                None => None,
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

/// The APIs an administrator can switch off one by one. Finer than a
/// [`Surface`]: Sonarr and Radarr share a guard, but not a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Api {
    /// `/v1/tvdb/*`, in Skyhook's place.
    Sonarr,
    /// `/v1/movie/*`, `/v1/search`, `/v1/list/*`, in api.radarr.video's place.
    Radarr,
    /// `/3/*` and `/4/*`, the TMDB relay.
    Tmdb,
    /// `/api/v1/*` called with a key. The interface's session is not an API
    /// call, and is never switched off.
    Native,
}

impl Api {
    pub const ALL: [Api; 4] = [Api::Sonarr, Api::Radarr, Api::Tmdb, Api::Native];

    /// The setting that switches it.
    pub fn setting(self) -> &'static str {
        match self {
            Api::Sonarr => "api.sonarr",
            Api::Radarr => "api.radarr",
            Api::Tmdb => "api.tmdb",
            Api::Native => "api.native",
        }
    }

    /// Which API a request to `surface` at `path` is a call to.
    pub fn of(surface: Surface, path: &str) -> Api {
        match surface {
            Surface::Arr if path.starts_with("/v1/tvdb") => Api::Sonarr,
            Surface::Arr => Api::Radarr,
            Surface::Tmdb => Api::Tmdb,
            Surface::Native => Api::Native,
        }
    }

    pub fn index(self) -> usize {
        self as usize
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
/// The first of `keys` set to something. A variable set to nothing is passed
/// over rather than taken: `.env.example` ships `AMS_MAL_CLIENT_ID=` empty, and
/// a copy of it used to hide a `MAL_CLIENT_ID` given anywhere else.
fn opt(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
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
