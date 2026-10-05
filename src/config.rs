//! Typed configuration, built once at startup from the process environment.
//!
//! Every knob is prefixed `AMS_`. A few unprefixed names from the tools this
//! server grew out of (`TMDB_API_KEY`, `BIND_ADDRESS`, …) are still accepted as
//! fallbacks so an existing `.env` keeps working.

use std::{
    fmt::Display, net::SocketAddr, ops::RangeInclusive, path::PathBuf, str::FromStr, time::Duration,
};

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

/// How many of this server there are.
///
/// One, the default: SQLite or PostgreSQL, the media on disk or in a bucket,
/// every job scheduled by the process itself. Several: every instance reads
/// and writes the same PostgreSQL, keeps the media in the same bucket, and
/// coordinates through the cache server — which one schedules, which one is
/// fetching what, what the others must forget — so that a request may land
/// on any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Single,
    Multi,
}

impl FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "single" | "one" | "" => Ok(Self::Single),
            "multi" | "multiple" | "cluster" => Ok(Self::Multi),
            other => bail!("AMS_MODE must be single or multi, not {other:?}"),
        }
    }
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Multi => "multi",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub mode: Mode,
    /// What this instance is called among the others: `AMS_INSTANCE_NAME`,
    /// or the machine's hostname — the container's name, in a container.
    pub instance_name: String,
    pub server: Server,
    pub database: Database,
    pub security: Security,
    pub tmdb: Tmdb,
    pub skyhook: Skyhook,
    pub sonarr_services: SonarrServices,
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
    /// Connections each door holds at once. Past it, a new one waits in the
    /// system's queue until another closes, rather than taking a descriptor
    /// the database and the providers need.
    pub max_connections: usize,
    /// How long a connection has to say what it wants — its first bytes, a
    /// request's head — and how long it may sit idle between two requests.
    pub header_read_timeout: Duration,
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
    /// The fingerprint of an authority to replace with one made for the names
    /// asked, once: an authority's name constraints are set when it is made,
    /// so a name added later is covered only by a new one.
    pub replace_authority: Option<String>,
}

#[derive(Clone)]
pub struct Database {
    pub url: String,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
}

/// Written by hand: the URL may carry the database's password.
impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("url", &masked_url(&self.url))
            .field("max_connections", &self.max_connections)
            .field("acquire_timeout", &self.acquire_timeout)
            .finish()
    }
}

/// An address as a log may show it: its password, if any, masked, and its
/// query — which can carry one too (`?password=`) — left out.
fn masked_url(text: &str) -> String {
    match url::Url::parse(text) {
        Ok(mut url) => {
            if url.password().is_some() {
                let _ = url.set_password(Some("***"));
            }
            url.set_query(None);
            url.to_string()
        }
        // `sqlite:data/ams.db` and the like: nothing a password hides in.
        Err(_) if !text.contains('@') => text.to_string(),
        Err(_) => "<unreadable>".to_string(),
    }
}

/// A secret, as a log may show it: whether there is one.
fn masked(secret: &Option<String>) -> Option<&'static str> {
    secret.as_ref().map(|_| "***")
}

#[derive(Clone)]
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
    /// The TheTVDB relay's. A TheTVDB client signs in with a key, so a key
    /// issued here can stand in that field, as with TMDB.
    pub tvdb_policy: SurfacePolicy,
    /// The AniList relay's. AniList has no key at all, so nothing a client
    /// sends can be one of this server's: by address, like Sonarr's.
    pub anilist_policy: SurfacePolicy,
    pub arr_policy: SurfacePolicy,
    /// Peers allowed to reach any surface whose policy is
    /// [`SurfacePolicy::Allowlist`].
    ///
    /// Not only the arr surfaces: a TMDB client whose key is compiled in — which
    /// is most of them — can only be let through by address either.
    ///
    /// The list's first value, in an empty table, and nothing after: the
    /// Access page edits it from then on.
    pub allowlist: Vec<IpNet>,
    /// Whether the environment named an allowlist at all, rather than the
    /// default being taken: a start then says so when the two differ.
    pub allowlist_from_env: bool,
    /// Bootstrap administrator, created on first start when no admin exists.
    pub bootstrap_admin: Option<(String, String)>,
    /// Requests per minute per peer on the native API. `0` disables the limiter.
    pub rate_limit_per_minute: u32,
    /// Failed sign-ins an account takes in fifteen minutes before it is
    /// refused without being checked. `0` never refuses.
    pub sign_in_failures_per_account: u32,
    /// Days of audit history to keep. `0` keeps everything.
    pub audit_retention_days: u32,
}

/// Written by hand so that no log line, however it came to print the
/// configuration, can carry the client secret or the administrator's
/// password.
impl std::fmt::Debug for Security {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Security")
            .field("auth_disabled", &self.auth_disabled)
            .field("public_browse", &self.public_browse)
            .field("public_browse_env", &self.public_browse_env)
            .field("signups_per_hour", &self.signups_per_hour)
            .field("signups_per_hour_total", &self.signups_per_hour_total)
            .field("oidc_client_secret", &masked(&self.oidc_client_secret))
            .field("force_password_login", &self.force_password_login)
            .field("native_policy", &self.native_policy)
            .field("tmdb_policy", &self.tmdb_policy)
            .field("tvdb_policy", &self.tvdb_policy)
            .field("anilist_policy", &self.anilist_policy)
            .field("arr_policy", &self.arr_policy)
            .field("allowlist", &self.allowlist)
            .field("allowlist_from_env", &self.allowlist_from_env)
            .field(
                "bootstrap_admin",
                &self
                    .bootstrap_admin
                    .as_ref()
                    .map(|(username, _)| (username, "***")),
            )
            .field("rate_limit_per_minute", &self.rate_limit_per_minute)
            .field(
                "sign_in_failures_per_account",
                &self.sign_in_failures_per_account,
            )
            .field("audit_retention_days", &self.audit_retention_days)
            .finish()
    }
}

#[derive(Clone)]
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

impl std::fmt::Debug for Tmdb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tmdb")
            .field("api_key", &masked(&self.api_key))
            .field("upstream", &self.upstream)
            .field("language", &self.language)
            .field("include_adult", &self.include_adult)
            .field("search_limit", &self.search_limit)
            .field("passthrough", &self.passthrough)
            .finish()
    }
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

/// `services.sonarr.tv`, which this server answers for when it is resolved
/// here: its scene-mapping list, with this catalogue's titles added, and
/// everything else passed on.
#[derive(Clone, Debug)]
pub struct SonarrServices {
    pub upstream: String,
    /// TheXEM, whose names Sonarr downloads beside that list: read so that a
    /// title added is never one of them for another series.
    pub xem_upstream: String,
    /// Whether the list is given this catalogue's titles; off, it is passed
    /// on as it is.
    pub scene_mappings: bool,
    /// Whether a series' title in the caller's language is searched with too.
    pub scene_mapping_search: bool,
}

#[derive(Clone)]
pub struct Tvdb {
    pub upstream: String,
    /// Without a key the provider is skipped.
    pub api_key: Option<String>,
    /// Only a subscriber key needs one; a project key must not send it.
    pub pin: Option<String>,
    pub enabled: bool,
    /// Whether `/v4/*` is relayed to TheTVDB for the clients that call it
    /// there — Yamtrack, Jellyfin's plugin, Kodi's scraper — with this
    /// catalogue's locked fields written into the answers.
    pub passthrough: bool,
}

impl std::fmt::Debug for Tvdb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tvdb")
            .field("upstream", &self.upstream)
            .field("api_key", &masked(&self.api_key))
            .field("pin", &masked(&self.pin))
            .field("enabled", &self.enabled)
            .field("passthrough", &self.passthrough)
            .finish()
    }
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
    /// Whether `graphql.anilist.co` is relayed for the clients that call it
    /// there, with this catalogue's locked fields written into the answers.
    pub passthrough: bool,
}

/// MyAnimeList: its official API when a client id is set, Jikan otherwise.
#[derive(Clone)]
pub struct Mal {
    pub upstream: String,
    pub jikan_upstream: String,
    /// Free from myanimelist.net's API settings. Without one, Jikan is used —
    /// an unofficial mirror that serves MAL from its own cache, which can be
    /// weeks old and fails outright when MyAnimeList refuses it.
    pub client_id: Option<String>,
    pub enabled: bool,
}

impl std::fmt::Debug for Mal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mal")
            .field("upstream", &self.upstream)
            .field("jikan_upstream", &self.jikan_upstream)
            .field("client_id", &masked(&self.client_id))
            .field("enabled", &self.enabled)
            .finish()
    }
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

#[derive(Clone)]
pub struct Fanart {
    pub upstream: String,
    /// Without a key the provider is simply skipped.
    pub api_key: Option<String>,
    pub enabled: bool,
}

impl std::fmt::Debug for Fanart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fanart")
            .field("upstream", &self.upstream)
            .field("api_key", &masked(&self.api_key))
            .field("enabled", &self.enabled)
            .finish()
    }
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
        let config = Self::read_env()?;
        config.check_mode()?;
        Ok(config)
    }

    /// What several instances need of the rest of the configuration, checked
    /// at start rather than found out in production: a database they can all
    /// write, a store they all reach, and the bus they coordinate on.
    fn check_mode(&self) -> Result<()> {
        if self.mode == Mode::Single {
            return Ok(());
        }
        let scheme = self.database.url.split(':').next().unwrap_or_default();
        if scheme == "sqlite" {
            bail!(
                "AMS_MODE=multi needs PostgreSQL: several instances cannot share an SQLite file. \
                 Point AMS_DATABASE_URL at a postgres:// database, or run one instance"
            );
        }
        if self.media.storage == MediaStorage::Filesystem {
            bail!(
                "AMS_MODE=multi needs the media in a bucket every instance reaches: set \
                 AMS_MEDIA_STORAGE=s3 (or off), not filesystem"
            );
        }
        if self.cache.redis_url.is_none() {
            bail!(
                "AMS_MODE=multi needs a cache server: the instances coordinate through it. \
                 Set AMS_REDIS_URL to a Valkey or Redis address"
            );
        }
        Ok(())
    }

    fn read_env() -> Result<Self> {
        Ok(Self {
            mode: var_or(&["AMS_MODE"], "single").parse()?,
            instance_name: opt(&["AMS_INSTANCE_NAME"])
                .unwrap_or_else(|| whoami::hostname().unwrap_or_else(|_| "instance".to_string())),
            server: Server {
                bind: var_or(
                    &["AMS_BIND_ADDRESS", "BIND_ADDRESS", "LISTEN_ADDR"],
                    "0.0.0.0:8080",
                )
                .parse()
                .context("AMS_BIND_ADDRESS is not a valid socket address")?,
                public_url: opt(&["AMS_PUBLIC_URL"]).map(|u| u.trim_end_matches('/').to_string()),
                tls: match (opt(&["AMS_TLS_CERT"]), opt(&["AMS_TLS_KEY"])) {
                    (Some(cert), Some(key)) => Some(Tls {
                        cert: cert.into(),
                        key: key.into(),
                    }),
                    (None, None) => None,
                    _ => bail!("AMS_TLS_CERT and AMS_TLS_KEY must be set together"),
                },
                // Zero would answer every request that waits on anything 504.
                request_timeout: secs_in(&["AMS_REQUEST_TIMEOUT"], 60, 1..=86_400)?,
                cors_origins: list(&["AMS_CORS_ORIGINS"]),
                allowed_hosts: list(&["AMS_ALLOWED_HOSTS"])
                    .into_iter()
                    .map(|h| h.trim().to_ascii_lowercase())
                    .filter(|h| !h.is_empty())
                    .collect(),
                trusted_proxies: nets(&["AMS_TRUSTED_PROXIES"], &[])?,
                max_connections: num_in(&["AMS_MAX_CONNECTIONS"], 1024, 1..=1_000_000)?,
                header_read_timeout: secs_in(&["AMS_HEADER_READ_TIMEOUT"], 30, 1..=600)?,
            },
            database: Database {
                url: var_or(
                    &["AMS_DATABASE_URL", "DATABASE_URL"],
                    "sqlite://data/ams.db?mode=rwc",
                ),
                max_connections: num_in(&["AMS_DATABASE_MAX_CONNECTIONS"], 10, 1..=10_000)?,
                acquire_timeout: secs_in(&["AMS_DATABASE_ACQUIRE_TIMEOUT"], 30, 1..=3_600)?,
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
                tvdb_policy: policy(&["AMS_TVDB_AUTH"], SurfacePolicy::ApiKey)?,
                anilist_policy: policy(&["AMS_ANILIST_AUTH"], SurfacePolicy::Allowlist)?,
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
                allowlist_from_env: opt(&["AMS_ALLOWLIST", "AMS_ARR_ALLOWLIST"]).is_some(),
                bootstrap_admin: match (opt(&["AMS_ADMIN_USERNAME"]), opt(&["AMS_ADMIN_PASSWORD"]))
                {
                    (Some(u), Some(p)) => Some((u, p)),
                    (None, None) => None,
                    _ => bail!("AMS_ADMIN_USERNAME and AMS_ADMIN_PASSWORD must be set together"),
                },
                rate_limit_per_minute: num(&["AMS_RATE_LIMIT_PER_MINUTE"], 600)?,
                sign_in_failures_per_account: num(&["AMS_SIGNIN_FAILURES_PER_ACCOUNT"], 10)?,
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
                // The range `tmdb.searchLimit` holds, which this seeds.
                search_limit: num_in::<usize>(
                    &["AMS_TMDB_SEARCH_LIMIT", "TMDB_SEARCH_LIMIT"],
                    10,
                    1..=50,
                )?,
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
            sonarr_services: SonarrServices {
                upstream: var_or(
                    &["AMS_SONARR_SERVICES_UPSTREAM"],
                    "https://services.sonarr.tv",
                )
                .trim_end_matches('/')
                .to_string(),
                xem_upstream: var_or(&["AMS_THEXEM_UPSTREAM"], "https://thexem.info")
                    .trim_end_matches('/')
                    .to_string(),
                scene_mappings: flag(&["AMS_SONARR_SCENE_MAPPINGS"], false)?,
                scene_mapping_search: flag(&["AMS_SONARR_SCENE_MAPPING_SEARCH"], true)?,
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
                passthrough: flag(&["AMS_TVDB_PASSTHROUGH"], true)?,
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
                passthrough: flag(&["AMS_ANILIST_PASSTHROUGH"], true)?,
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
                // Zero would time every call to the cache server out.
                redis_timeout: Duration::from_millis(num_in(
                    &["AMS_REDIS_TIMEOUT_MS"],
                    150,
                    1..=60_000,
                )?),
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
                    replace_authority: opt(&["AMS_TLS_REPLACE_AUTHORITY"]),
                }),
                None => None,
            },
            // The two the settings take over are held to the ranges the
            // settings hold, here, rather than found out by the seeding.
            refresh: Refresh {
                enabled: flag(&["AMS_REFRESH_ENABLED"], true)?,
                interval: secs_in(&["AMS_REFRESH_INTERVAL"], 900, 60..=604_800)?,
                continuing_ttl: secs(&["AMS_REFRESH_CONTINUING_TTL"], 21_600)?,
                ended_ttl: secs(&["AMS_REFRESH_ENDED_TTL"], 604_800)?,
                batch_size: num_in(&["AMS_REFRESH_BATCH_SIZE"], 25, 1..=500)?,
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
            Surface::Tvdb => self.security.tvdb_policy,
            Surface::Anilist => self.security.anilist_policy,
            Surface::Arr => self.security.arr_policy,
        }
    }

    /// Every secret the configuration holds — provider keys, the identity
    /// provider's client secret, the administrator's password, the bucket's
    /// secret, the passwords in the database's and the cache server's
    /// addresses — for the log to mask wherever one turns up.
    pub fn secrets(&self) -> Vec<String> {
        let password_of = |address: &str| {
            url::Url::parse(address)
                .ok()
                .and_then(|url| url.password().map(str::to_string))
        };
        [
            self.tmdb.api_key.clone(),
            self.tvdb.api_key.clone(),
            self.tvdb.pin.clone(),
            self.fanart.api_key.clone(),
            self.mal.client_id.clone(),
            self.security.oidc_client_secret.clone(),
            self.security
                .bootstrap_admin
                .as_ref()
                .map(|(_, password)| password.clone()),
            self.media.s3.as_ref().map(|s3| s3.secret_key.clone()),
            password_of(&self.database.url),
            self.cache
                .redis_url
                .as_ref()
                .and_then(|redis| password_of(&redis.0)),
        ]
        .into_iter()
        .flatten()
        .collect()
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
    /// `/v4/*`, the TheTVDB relay, in api4.thetvdb.com's place.
    Tvdb,
    /// `graphql.anilist.co`, the AniList relay.
    Anilist,
}

impl Api {
    /// Every API, in the order the access page lists them: the stand-ins,
    /// the relays, this server's own. The counts are filed by discriminant
    /// (`index`), not by this order.
    pub const ALL: [Api; 6] = [
        Api::Sonarr,
        Api::Radarr,
        Api::Tmdb,
        Api::Tvdb,
        Api::Anilist,
        Api::Native,
    ];

    /// The setting that switches it.
    pub fn setting(self) -> &'static str {
        match self {
            Api::Sonarr => "api.sonarr",
            Api::Radarr => "api.radarr",
            Api::Tmdb => "api.tmdb",
            Api::Native => "api.native",
            Api::Tvdb => "api.tvdb",
            Api::Anilist => "api.anilist",
        }
    }

    /// Which API a request to `surface` at `path` is a call to.
    pub fn of(surface: Surface, path: &str) -> Api {
        match surface {
            Surface::Arr if path.starts_with("/v1/tvdb") || path == "/v1/scenemapping" => {
                Api::Sonarr
            }
            Surface::Arr => Api::Radarr,
            Surface::Tmdb => Api::Tmdb,
            Surface::Tvdb => Api::Tvdb,
            Surface::Anilist => Api::Anilist,
            Surface::Native => Api::Native,
        }
    }

    /// Whether it relays a service's own API, with this catalogue's edits
    /// written in: what a member's key may be kept from, since a relay
    /// spends the operator's quota there.
    pub fn is_relay(self) -> bool {
        matches!(self, Api::Tmdb | Api::Tvdb | Api::Anilist)
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
    /// `/v4/*` — the TheTVDB relay.
    Tvdb,
    /// `graphql.anilist.co`, on the clients' door — the AniList relay.
    Anilist,
    /// `/v1/tvdb/*` and `/v1/movie/*` — Sonarr and Radarr compatibility.
    Arr,
}

impl Surface {
    /// Whether the surface relays a service's own API.
    pub fn is_relay(self) -> bool {
        matches!(self, Surface::Tmdb | Surface::Tvdb | Surface::Anilist)
    }
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

/// A number within `range`, or why not, naming the variable: a value that
/// would stop the server working is refused at start, not found out later.
fn num_in<T>(keys: &[&str], default: T, range: RangeInclusive<T>) -> Result<T>
where
    T: FromStr + PartialOrd + Display,
    T::Err: Display,
{
    let value = num(keys, default)?;
    if !range.contains(&value) {
        bail!(
            "{} must be between {} and {}, not {value}",
            keys[0],
            range.start(),
            range.end()
        );
    }
    Ok(value)
}

/// [`num_in`], in seconds.
fn secs_in(keys: &[&str], default: u64, range: RangeInclusive<u64>) -> Result<Duration> {
    Ok(Duration::from_secs(num_in(keys, default, range)?))
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
        Some(v) => v
            .parse()
            .with_context(|| format!("{} is not a surface policy", keys[0])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_out_of_range_is_refused_naming_its_variable() {
        // Unset variables give their defaults, which are within range.
        assert_eq!(num_in(&["AMS_TEST_UNSET_A7Q"], 25u32, 1..=500).unwrap(), 25);
        let refused = num_in(&["AMS_TEST_UNSET_A7Q"], 0u32, 1..=500).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "AMS_TEST_UNSET_A7Q must be between 1 and 500, not 0"
        );
        assert!(secs_in(&["AMS_TEST_UNSET_A7Q"], 30, 60..=604_800).is_err());
    }

    #[test]
    fn printing_the_configuration_prints_no_secret() {
        let security = format!(
            "{:?}",
            Security {
                auth_disabled: false,
                public_browse: false,
                public_browse_env: None,
                signups_per_hour: 5,
                signups_per_hour_total: 100,
                oidc_client_secret: Some("oidc-secret-value".into()),
                force_password_login: false,
                native_policy: SurfacePolicy::ApiKey,
                tmdb_policy: SurfacePolicy::ApiKey,
                tvdb_policy: SurfacePolicy::ApiKey,
                anilist_policy: SurfacePolicy::Allowlist,
                arr_policy: SurfacePolicy::Allowlist,
                allowlist: Vec::new(),
                allowlist_from_env: false,
                bootstrap_admin: Some(("admin".into(), "admin-password-value".into())),
                rate_limit_per_minute: 600,
                sign_in_failures_per_account: 10,
                audit_retention_days: 90,
            }
        );
        assert!(!security.contains("oidc-secret-value"), "{security}");
        assert!(!security.contains("admin-password-value"), "{security}");
        assert!(security.contains("\"admin\""), "{security}");

        let database = format!(
            "{:?}",
            Database {
                url: "postgres://ams:db-password-value@db:5432/ams?password=other".into(),
                max_connections: 10,
                acquire_timeout: Duration::from_secs(30),
            }
        );
        assert!(!database.contains("db-password-value"), "{database}");
        assert!(!database.contains("other"), "{database}");
        assert!(database.contains("db:5432"), "{database}");
        assert_eq!(
            masked_url("sqlite://data/ams.db?mode=rwc"),
            "sqlite://data/ams.db"
        );
    }
}
