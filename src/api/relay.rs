//! What the relays share: a request handed on to a service in a client's
//! place, and its answer handed back.
//!
//! Four services are relayed this way — TMDB under `/3`, TheTVDB under `/v4`,
//! AniList by its name, and Sonarr's own services for whatever this server
//! does not answer itself. Each keeps its own rules for the credentials it
//! stands in for and for what it writes into an answer; what they have in
//! common is here: which headers travel, which never do, what a path may not
//! say, whose credential the service is asked with, the work a document is
//! patched for, and the text it is patched with in the language asked — and,
//! for TMDB's, TheTVDB's and AniList's, what is kept of their answers: one
//! exchange for every caller asking the same at once, and what was kept
//! standing in when the service fails ([`answer`]).

use std::{sync::LazyLock, time::Duration};

use axum::{
    Json,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::{
    auth::secrets,
    cache,
    config::{Surface, SurfacePolicy},
    domain::{CoverType, Episode, ExternalSource, MediaItem, Season, fields},
    error::AppResult,
    service::{self, language},
    state::AppState,
};

/// The client the relays ask with: following no redirect — a redirect is
/// handed back to the caller as it came, since the address it names was
/// chosen by somebody else.
pub static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

/// Hop-by-hop headers (RFC 9110 §7.6.1), and those the HTTP stack sets
/// itself: the body is read decoded, so its encoding and length are the
/// stack's to say. This server's own mark is one too: it names the instance
/// that sent a request, not the one the answer comes from.
pub const HOP_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "content-encoding",
    "accept-encoding",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    crate::providers::radarr::LOOP_HEADER,
];

/// Query parameters that carry a key for this server, never the service's.
///
/// Every spelling of a key the guard accepts has to be here too. A client may
/// authenticate with `?apikey=`, and for a while only `?api_key=` was
/// stripped — so its credential for *this* server was relayed to TMDB, and
/// landed in TMDB's access logs on every request.
pub const OUR_PARAMS: &[&str] = &["apikey", "api_key"];

/// Whether a request was addressed to one of `hosts`.
///
/// HTTP/2 names the host in the address, HTTP/1.1 in `Host`, with its port.
/// The names are hostnames, so everything from a colon on is the port — and
/// an address in brackets is never one of them.
pub fn addressed_to(headers: &HeaderMap, uri: &Uri, hosts: &[&str]) -> bool {
    // `Host` first, then the address, as the doors' own host check reads a
    // request: a request that names one host in each must not pass the
    // check under one name and be dispatched under the other.
    let named = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|host| host.split(':').next())
        .map(str::to_string)
        .or_else(|| uri.host().map(str::to_string));
    named.is_some_and(|name| {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        hosts.contains(&name.as_str())
    })
}

/// Whether a request was sent by an instance of this server: no client sends
/// the header, so one that carries it — whosever — has come round a loop,
/// through a resolver or a door several instances share.
pub fn looped(headers: &HeaderMap) -> bool {
    headers.contains_key(crate::providers::radarr::LOOP_HEADER)
}

/// The query asked, less the parameters named, as it was written; none when
/// nothing is left of it.
pub fn query_without(uri: &Uri, dropped: &[&str]) -> Option<String> {
    let kept: Vec<&str> = uri
        .query()?
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or_default();
            !dropped.iter().any(|ours| name.eq_ignore_ascii_case(ours))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join("&"))
}

/// The path and query asked, less a key for this server.
pub fn without_our_params(uri: &Uri) -> String {
    match query_without(uri, OUR_PARAMS) {
        Some(query) => format!("{}?{query}", uri.path()),
        None => uri.path().to_string(),
    }
}

/// The query asked, less the parameters named, in a fixed order: what a
/// cached document is filed under, so that two spellings of one request are
/// one entry.
///
/// Ordered by name alone: a parameter given twice keeps the order it was
/// given in, which is the order a service reads it in — the last
/// `language` wins — so `language=fr&language=de` and the other way round,
/// two answers, are two entries.
pub fn sorted_query(uri: &Uri, dropped: &[&str]) -> String {
    let mut pairs: Vec<(String, String)> = uri
        .query()
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| !dropped.iter().any(|d| name.eq_ignore_ascii_case(d)))
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    // Stable: equal names stay as they came.
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

/// How long a relay keeps an answer of this status, when it keeps it at
/// all: a document for as long as its kind stays good — `path` as
/// [`cache::relay_ttl`] reads it — and a "no such thing" a few minutes.
/// Any other answer is asked again: another success than `200` — a
/// fragment of a document, "no content" — a redirect, "unchanged", a
/// refusal or an error was one caller's answer, or is gone the next minute.
pub fn kept_for(status: StatusCode, path: &str) -> Option<Duration> {
    match status {
        StatusCode::OK => Some(cache::relay_ttl(path)),
        StatusCode::NOT_FOUND => Some(Duration::from_secs(5 * 60)),
        _ => None,
    }
}

/// The caller's headers the service is asked with: those named, and nothing
/// else. Not a credential, a cookie, or a proxy's `X-Forwarded-For`.
pub fn asked_with(headers: &HeaderMap, names: &[&str]) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if names.contains(&name.as_str()) {
            out.append(name.clone(), value.clone());
        }
    }
    out
}

/// The headers an answer keeps: all but the hop-by-hop ones, a cookie,
/// which would be set against this server — where the session cookie lives
/// — and the service's own word on cross-origin reads, which is this
/// server's to give for its own origin.
pub fn kept(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if HOP_HEADERS.contains(&name.as_str())
            || name == header::SET_COOKIE
            || name == header::COOKIE
            || name.as_str().starts_with("access-control-")
        {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            out.append(name, value);
        }
    }
    out
}

/// The validators a patched answer may not carry: the service's described
/// the service's bytes, and a client holding them would be told "unchanged"
/// for a body it never saw the final shape of.
pub fn without_validators(headers: &mut HeaderMap) {
    for name in [
        header::ETAG,
        header::LAST_MODIFIED,
        header::CACHE_CONTROL,
        header::EXPIRES,
    ] {
        headers.remove(name);
    }
}

/// Whether any segment of `path` could be read as leaving its place: a dot
/// segment, encoded or not, or a separator hidden inside a segment.
///
/// Percent-decoded first, because `%2e%2e` and `..` mean the same thing to
/// the URL parser that builds the outgoing request and different things to a
/// naive comparison. To that parser a backslash is a slash, so
/// `list/..\account` is `account`; and `%2F` survives it, but what a
/// service's own edge makes of `..%2F` is not this server's to find out. An
/// un-decodable escape is treated as suspicious rather than harmless —
/// nothing these services address needs one.
pub fn climbs(path: &str) -> bool {
    path.split('/').any(|segment| {
        match urlencoding::decode(segment) {
            Ok(decoded) => matches!(decoded.as_ref(), "." | "..") || decoded.contains(['/', '\\']),
            // Not valid UTF-8 once decoded: not a path of theirs either.
            Err(_) => true,
        }
    })
}

pub fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}

/// The bearer token a request carries, when it carries one. The scheme is
/// read whatever its case, as HTTP reads every scheme (RFC 9110 §11.1): a key
/// sent as `BEARER ams_…` is a bearer token all the same, and a relay that
/// took it for none would hand it on.
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(|c: char| c.is_ascii_whitespace())?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_string())
}

/// Every word a credential is written with — split at spaces, commas,
/// equals signs, semicolons and quotes, its bytes read as they came — where
/// a key may sit whatever the shape around it: `Token ams_…`,
/// `Bearer theirs ams_…`, `token="ams_…"`.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '=' | ';' | '"'))
        .filter(|word| !word.is_empty())
}

/// Every word each `Authorization` value carries, whatever its scheme — and,
/// of a `Basic` one, the name and the password it encodes.
fn authorization_words(headers: &HeaderMap) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for value in headers.get_all(header::AUTHORIZATION) {
        let text = String::from_utf8_lossy(value.as_bytes());
        let mut parts = text.split_whitespace();
        if parts
            .next()
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic"))
            && let Some(encoded) = parts.next()
            && let Ok(decoded) = STANDARD.decode(encoded)
        {
            let decoded = String::from_utf8_lossy(&decoded);
            found.extend(decoded.split(':').map(str::to_string));
        }
        found.extend(words(&text).map(str::to_string));
    }
    found
}

/// Whether `Authorization` carries a key issued here anywhere in it, in any
/// of its values: what a relay that hands the header on to its service
/// never hands on.
pub fn authorization_carries_key_issued_here(headers: &HeaderMap) -> bool {
    authorization_words(headers)
        .iter()
        .any(|word| secrets::is_issued_here(word))
}

/// A work of this catalogue a service's document is about, when a person
/// locked something on it: what a relay writes into the answer. The
/// pictures it names are addressed as a client can fetch them.
pub async fn locked_work(
    state: &AppState,
    source: ExternalSource,
    id: &str,
) -> AppResult<Option<MediaItem>> {
    let Some(found) = crate::db::repo::item::find_id_by_external(&state.db, source, id).await?
    else {
        return Ok(None);
    };
    let Some(mut item) = service::load(state, &found).await? else {
        return Ok(None);
    };
    if item.locked_fields.is_empty() {
        return Ok(None);
    }
    state.media.for_clients(&mut item);
    Ok(Some(item))
}

/// Whether a field of the work itself is locked: `title`, `overview`,
/// `primaryPoster`, as the field registry names them.
pub fn is_locked(item: &MediaItem, field: &str) -> bool {
    let key = format!("item/{field}");
    item.locked_fields.contains(&key)
}

/// The address of the picture a person chose for a work, when they chose
/// one: the poster or the backdrop.
pub fn chosen_image(item: &MediaItem, kind: CoverType) -> Option<String> {
    let chosen = match kind {
        CoverType::Poster => item.primary_images.poster.as_deref(),
        CoverType::Fanart => item.primary_images.fanart.as_deref(),
        _ => None,
    }?;
    item.images
        .iter()
        .find(|image| image.id == chosen)
        .map(|image| image.url.clone())
}

/// The address of the poster a person chose for a season, when they chose
/// one.
pub fn chosen_season_poster(season: &Season) -> Option<String> {
    let chosen = season.primary_images.poster.as_deref()?;
    season
        .images
        .iter()
        .find(|image| image.id == chosen)
        .map(|image| image.url.clone())
}

/// Whether a field of one of the work's seasons is locked — `title`,
/// `overview`, `airDate`, `primaryPoster` — by the catalogue's own number.
pub fn season_locked(item: &MediaItem, season: i32, field: &str) -> bool {
    let key = format!("season:{season}/{field}");
    item.locked_fields.contains(&key)
}

/// Whether a field of one of the work's episodes is locked, by the
/// catalogue's own numbers.
pub fn episode_locked(item: &MediaItem, episode: &Episode, field: &str) -> bool {
    let key = format!(
        "episode:{}x{}/{field}",
        episode.season_number, episode.episode_number
    );
    item.locked_fields.contains(&key)
}

// ─── Whose credential a relay asks with ─────────────────────────────────────

/// Whose credential a relay asks its service with: an administrator's
/// choice on the access page, `tmdb.relayCredential` for TMDB's relay and
/// `tvdb.relayCredential` for TheTVDB's. AniList has no key to choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RelayCredential {
    /// This server's own key, in place of whatever the client sent: the
    /// operator's key serves every client, and the operator answers for that
    /// use to the service.
    Server,
    /// The client's own, handed on as it came. This server's key is never
    /// used, and a key issued here is refused — it is none of the service's.
    /// Nothing asked with a client's own credential is kept for another.
    Client,
}

impl RelayCredential {
    /// The setting that says it for a surface: none for one whose service
    /// takes no key, or that relays nothing.
    pub fn setting(surface: Surface) -> Option<&'static str> {
        match surface {
            Surface::Tmdb => Some("tmdb.relayCredential"),
            Surface::Tvdb => Some("tvdb.relayCredential"),
            _ => None,
        }
    }
}

/// Whose credential the relay on `surface` asks with: this server's own,
/// unless an administrator chose each client's.
pub fn credential(state: &AppState, surface: Surface) -> RelayCredential {
    let chosen =
        RelayCredential::setting(surface).and_then(|key| state.settings.resolve(key, None, None));
    match chosen.as_deref() {
        Some("client") => RelayCredential::Client,
        _ => RelayCredential::Server,
    }
}

/// How a surface is guarded, as its guard applies it: the deployment's
/// policy (`AMS_*_AUTH`, after `AMS_AUTH_DISABLED`) — except on a relay
/// asked with each client's own credential. A key issued here is refused
/// there, so a key policy would turn away every caller the relay can serve:
/// the address list guards it instead, as it guards Sonarr's surface. Open
/// stays open.
pub fn policy(state: &AppState, surface: Surface) -> SurfacePolicy {
    let configured = state.config.policy_for(surface);
    guarded(configured, credential(state, surface))
}

/// [`policy`], once its two answers are known.
fn guarded(configured: SurfacePolicy, credential: RelayCredential) -> SurfacePolicy {
    if configured == SurfacePolicy::ApiKey && credential == RelayCredential::Client {
        SurfacePolicy::Allowlist
    } else {
        configured
    }
}

/// Whether a request carries a key issued here where a key may be sent: in
/// `Authorization`, as a bearer token or under any other scheme, in any of
/// its values and anywhere in each; in `X-Api-Key`; or in the query under
/// any name a key goes by here — `api_key` among them, which is TMDB's own.
/// Every word of each is looked at, not only the first, the scheme read
/// whatever its case. What a relay asked with each client's own credential
/// refuses before its service is asked anything, since it is this server's
/// and not the service's.
pub fn carries_key_issued_here(headers: &HeaderMap, uri: &Uri) -> bool {
    let in_header: Vec<String> = headers
        .get_all("x-api-key")
        .iter()
        .flat_map(|value| {
            words(&String::from_utf8_lossy(value.as_bytes()))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    let in_query: Vec<String> = uri
        .query()
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| {
                    OUR_PARAMS
                        .iter()
                        .any(|ours| name.eq_ignore_ascii_case(ours))
                })
                .flat_map(|(_, value)| words(&value).map(str::to_string).collect::<Vec<_>>())
                .collect()
        })
        .unwrap_or_default();
    authorization_words(headers)
        .into_iter()
        .chain(in_header)
        .chain(in_query)
        .any(|key| secrets::is_issued_here(&key))
}

/// A refusal this server made itself, marked on the answer, so that a route
/// counting its calls tells it from a service's own refusal handed back.
#[derive(Clone, Copy, Debug)]
pub struct RefusedHere;

/// What a relay asked with each client's own credential answers a key
/// issued here: a 401 that says why, before the service is asked anything.
/// Neither the key nor what it is for leaves this server.
pub fn key_issued_here_refused(service: &str, setting: &str) -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "own_key_required",
            "message": format!(
                "this server hands each client's own {service} key on to {service} ({setting} \
                 is client), and a key issued here is not one: use your own {service} key, or \
                 have an administrator let the relay stand in with the server's"
            ),
        })),
    )
        .into_response();
    response.extensions_mut().insert(RefusedHere);
    response
}

// ─── What is kept of an answer, and what stands in when the service fails ───

pub use kept::{
    Ask, Fetched, STALE_IF_ERROR, Service, answer, changed_here, is_reference, local_first, unsent,
};

/// What is kept of an answer, and what stands in when its service fails:
/// shared by TMDB's, TheTVDB's and AniList's relays ([`answer`]).
mod kept {
    use std::{
        collections::HashMap,
        future::Future,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    use bytes::Bytes;
    use chrono::{DateTime, Utc};
    use futures::future::{BoxFuture, FutureExt as _, Shared, WeakShared};

    use super::*;
    use crate::{
        db::repo::{change::ChangedHere, relay_document},
        error::{AppError, Throttle},
    };

    /// A service whose answers its relay keeps: what it keeps, and what stands
    /// in for it, depend on which.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Service {
        Tmdb,
        Tvdb,
        Anilist,
    }

    impl Service {
        /// Its name in the metrics, the log and the database: `tmdb`, `tvdb`,
        /// `anilist`.
        pub const fn name(self) -> &'static str {
            match self {
                Self::Tmdb => "tmdb",
                Self::Tvdb => "tvdb",
                Self::Anilist => "anilist",
            }
        }

        /// Its name in a message.
        pub const fn label(self) -> &'static str {
            match self {
                Self::Tmdb => "TMDB",
                Self::Tvdb => "TheTVDB",
                Self::Anilist => "AniList",
            }
        }

        /// Whether its relay keeps a "no such thing" a few minutes, as it keeps
        /// a document: TheTVDB's and AniList's do; TMDB's asks again.
        pub const fn keeps_not_found(self) -> bool {
            !matches!(self, Self::Tmdb)
        }

        /// Whether its reference lists are kept in the database too
        /// ([`is_reference`]): TMDB's and TheTVDB's; never AniList's, whose
        /// terms want nothing stored.
        pub const fn keeps_references(self) -> bool {
            !matches!(self, Self::Anilist)
        }

        /// Whether a 403 is the service saying it is off — AniList's "API
        /// disabled" — rather than refusing the request.
        pub const fn forbids_when_down(self) -> bool {
            matches!(self, Self::Anilist)
        }

        /// The setting that says whether its relay answers the works held
        /// here itself ([`local_first`]): TMDB's and TheTVDB's; AniList's
        /// relay always asks AniList.
        pub const fn mode_setting(self) -> Option<&'static str> {
            match self {
                Self::Tmdb => Some("tmdb.relayMode"),
                Self::Tvdb => Some("tvdb.relayMode"),
                Self::Anilist => None,
            }
        }
    }

    /// Whether the relay of `service` answers a work this server holds
    /// itself, in the service's shape, before asking the service
    /// (`*.relayMode = local-first`); `relay`, the default, asks the service
    /// for everything. What it answers so is counted apart
    /// ([`crate::metrics::relay_local`]).
    pub fn local_first(state: &AppState, service: Service) -> bool {
        service
            .mode_setting()
            .and_then(|key| state.settings.resolve(key, None, None))
            .is_some_and(|mode| mode == "local-first")
    }

    /// The setting that lets what a relay kept stand in for a service that
    /// fails: on unless an administrator turns it off.
    pub const STALE_IF_ERROR: &str = "relays.staleIfError";

    /// The most of a document a relay keeps: anything larger is asked again.
    pub const KEEP_MAX_BYTES: usize = 4 * 1024 * 1024;

    /// Whether what the relays kept may stand in for a service that fails
    /// ([`STALE_IF_ERROR`]).
    pub fn stale_if_error(state: &AppState) -> bool {
        state
            .settings
            .bool_at(STALE_IF_ERROR, None, None)
            .unwrap_or(true)
    }

    /// Whether a service's status says it failed, and what was kept may stand
    /// in: too many requests, an error of its own, a gateway in front of it that
    /// got nothing — and AniList's 403, its API switched off. Not a refusal of
    /// the request itself — 400, 401 — nor a "no such thing".
    pub fn status_calls_for_stale(service: Service, status: StatusCode) -> bool {
        matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
            || (status == StatusCode::FORBIDDEN && service.forbids_when_down())
    }

    /// Whether a relay's exchange that came to an error left the service
    /// unheard, and what was kept may stand in: it could not be reached, said
    /// nothing in time or nothing readable ([`AppError::UpstreamUnavailable`]),
    /// its door was closed or full ([`AppError::RateLimited`], see [`unsent`]),
    /// or it would not sign this server in a moment ago.
    pub fn error_calls_for_stale(error: &AppError) -> bool {
        matches!(
            error,
            AppError::UpstreamUnavailable(_)
                | AppError::RateLimited(_)
                | AppError::Disabled {
                    code: "upstream_unavailable",
                    ..
                }
        )
    }

    /// What a relay answers a request its provider's door did not let through
    /// ([`crate::providers::Gate::send_now`]): a door closed by the service's
    /// own 429 or 503 is a 429 that says when it opens; a door whose every
    /// place stayed taken, a 429 that says a second; any other failure, the
    /// service unheard — a 502.
    pub fn unsent(service: Service, error: anyhow::Error) -> AppError {
        if let Some(left) = crate::providers::closed_for(&error) {
            return AppError::RateLimited(Throttle::after(left));
        }
        if crate::providers::is_busy(&error) {
            return AppError::RateLimited(Throttle::after(Duration::from_secs(1)));
        }
        AppError::UpstreamUnavailable(error.context(format!("{} request failed", service.label())))
    }

    /// Where an answer that stands in for its service came from, and how old it
    /// is, in seconds: what its `Age` says.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum StoodIn {
        /// The relay space's last answer to the same request.
        Memory { age: u64 },
        /// A reference list kept in the database ([`is_reference`]).
        Database { age: u64 },
    }

    impl StoodIn {
        pub const fn age(self) -> u64 {
            match self {
                Self::Memory { age } | Self::Database { age } => age,
            }
        }
    }

    /// What a relay's service answered — or what stood in for it: its status,
    /// the headers kept of it ([`kept`](fn@super::kept)), and its body, before any lock is
    /// written in.
    #[derive(Clone, Debug)]
    pub struct Fetched {
        pub status: StatusCode,
        pub headers: HeaderMap,
        pub body: Bytes,
        stood_in: Option<StoodIn>,
    }

    impl Fetched {
        /// What the service answered.
        pub fn new(status: StatusCode, headers: HeaderMap, body: Bytes) -> Self {
            Self {
                status,
                headers,
                body,
                stood_in: None,
            }
        }

        /// Where it came from and how old it is, when it stood in for the
        /// service's own answer.
        #[cfg(test)]
        pub fn stood_in(&self) -> Option<StoodIn> {
            self.stood_in
        }

        /// What the relay space held, still good.
        fn kept(hit: &cache::Relayed) -> Self {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&hit.content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            Self::new(
                StatusCode::from_u16(hit.status).unwrap_or(StatusCode::OK),
                headers,
                hit.body.clone(),
            )
        }

        /// A whole document kept, standing in for the service: its type, and
        /// its age as `Age` says it (RFC 9111 §5.1).
        fn stale(content_type: &str, body: Bytes, stood_in: StoodIn) -> Self {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            headers.insert(header::AGE, HeaderValue::from(stood_in.age()));
            Self {
                status: StatusCode::OK,
                headers,
                body,
                stood_in: Some(stood_in),
            }
        }
    }

    /// The type an answer was written in, as it is kept: JSON when it says
    /// none.
    fn content_type_of(headers: &HeaderMap) -> String {
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string()
    }

    /// What a relay asks of [`answer`]: whose service, what the answer is
    /// filed under, and the path its durations are read by.
    #[derive(Clone, Debug)]
    pub struct Ask {
        pub service: Service,
        /// What the answer is kept under: only a read asked with this server's
        /// own credential, the same for every caller, has one. None: nothing is
        /// kept, nothing shared, and nothing stands in.
        pub key: Option<String>,
        /// The path the durations are read by ([`cache::relay_ttl`],
        /// [`cache::relay_stale_for`]): the request's own — `/3/movie/550`,
        /// `/v4/series/81189` — or, for AniList, [`cache::ANILIST_QUERY`] or
        /// [`cache::ANILIST_COLLECTIONS`].
        pub path: String,
        /// What the database keeps a reference list under ([`is_reference`]):
        /// the key without the generation the memory's keys carry, which starts
        /// again at every start. None for AniList, which keeps none.
        pub reference: Option<String>,
    }

    /// Answer a request with what the service answers, as every relay with a
    /// key to keep it under does:
    ///
    /// 1. a document the relay space holds, still good — a whole one, or a "no
    ///    such thing" where the service's relay keeps those — and nothing else;
    /// 2. otherwise one exchange, `fetch`, for every caller asking the same at
    ///    once ([`together`]), its answer kept once ([`keep`]);
    /// 3. and where the service failed ([`status_calls_for_stale`],
    ///    [`error_calls_for_stale`]), what was kept stands in, unless
    ///    `relays.staleIfError` is off ([`stand_in`]): the service's own answer,
    ///    or its error, otherwise.
    ///
    /// Without a key, `fetch` alone. What the answer is patched with — the
    /// locks — is the relay's own to write, on a stale answer as on any other.
    /// `fetch` runs on its own, whoever asked first: it owns what it reads.
    pub async fn answer<F>(state: &AppState, ask: Ask, fetch: F) -> AppResult<Fetched>
    where
        F: Future<Output = AppResult<Fetched>> + Send + 'static,
    {
        let Some(key) = ask.key.clone() else {
            return fetch.await;
        };
        let held = state.caches.relay.get(&key).await;
        if let Some(hit) = &held
            && hit.is_fresh()
            && (hit.status == 200 || (hit.status == 404 && ask.service.keeps_not_found()))
        {
            return Ok(Fetched::kept(hit));
        }

        let flight = {
            let (state, ask) = (state.clone(), ask.clone());
            async move {
                let fetched = fetch.await?;
                keep(&state, &ask, &fetched).await;
                Ok(fetched)
            }
        };
        match together(key, flight).await {
            Ok(fetched) if status_calls_for_stale(ask.service, fetched.status) => {
                let why = fetched.status.as_u16().to_string();
                Ok(stand_in(state, &ask, held.as_ref(), &why)
                    .await
                    .unwrap_or(fetched))
            }
            Err(error) if error_calls_for_stale(&error) => {
                match stand_in(state, &ask, held.as_ref(), &error.to_string()).await {
                    Some(stale) => Ok(stale),
                    None => Err(error),
                }
            }
            other => other,
        }
    }

    /// Keep what a relay's service answered, under the ask's key: a whole
    /// document good for its path's time ([`cache::relay_ttl`]) and held for as
    /// long as it may stand in after ([`cache::relay_stale_for`]); a "no such
    /// thing", where the service's relay keeps those, five minutes. A reference
    /// list goes to the database too ([`keep_reference`]). Nothing else is kept:
    /// another success, a refusal, an error, what stood in, or anything past
    /// [`KEEP_MAX_BYTES`].
    pub async fn keep(state: &AppState, ask: &Ask, fetched: &Fetched) {
        let Some(key) = &ask.key else { return };
        let whole = fetched.status == StatusCode::OK;
        if fetched.stood_in.is_some()
            || fetched.body.len() > KEEP_MAX_BYTES
            || !(whole
                || (fetched.status == StatusCode::NOT_FOUND && ask.service.keeps_not_found()))
        {
            return;
        }
        let Some(fresh_for) = kept_for(fetched.status, &ask.path) else {
            return;
        };
        let held_for = if whole {
            fresh_for.max(cache::relay_stale_for(&ask.path))
        } else {
            fresh_for
        };
        state
            .caches
            .relay
            .insert_for(
                key.clone(),
                cache::Relayed {
                    status: fetched.status.as_u16(),
                    content_type: content_type_of(&fetched.headers),
                    expires_at: cache::now_secs() + fresh_for.as_secs(),
                    body: fetched.body.clone(),
                },
                held_for,
            )
            .await;
        if whole && let Some(filed_as) = &ask.reference {
            keep_reference(state, ask.service, filed_as, &ask.path, fetched).await;
        }
    }

    /// Let what was kept stand in for a service that failed, `why` saying how:
    /// the relay space's last whole answer to the same request while it is
    /// young enough ([`cache::Relayed::may_stand_in`]), otherwise a reference
    /// list kept in the database ([`reread_reference`]). With its `Age`,
    /// counted (`ams_relay_stale_total`) and said in the log. None when
    /// `relays.staleIfError` is off, or nothing was kept.
    pub async fn stand_in(
        state: &AppState,
        ask: &Ask,
        held: Option<&cache::Relayed>,
        why: &str,
    ) -> Option<Fetched> {
        if !stale_if_error(state) {
            return None;
        }
        let stale = match held.filter(|hit| hit.may_stand_in(&ask.path)) {
            Some(hit) => Fetched::stale(
                &hit.content_type,
                hit.body.clone(),
                StoodIn::Memory {
                    age: hit.age(&ask.path),
                },
            ),
            None => {
                let filed_as = ask.reference.as_deref()?;
                reread_reference(state, ask.service, filed_as, &ask.path).await?
            }
        };
        let stood_in = stale.stood_in?;
        let from_database = matches!(stood_in, StoodIn::Database { .. });
        crate::metrics::relay_stale(ask.service.name(), from_database);
        tracing::info!(
            relay = ask.service.name(),
            path = %ask.path,
            failure = why,
            age_s = stood_in.age(),
            from = if from_database { "database" } else { "memory" },
            "the service failed; answered stale, with what was kept"
        );
        Some(stale)
    }

    /// Whether a path is a reference list a client cannot start without, which
    /// TMDB's and TheTVDB's relays keep in the database too: TMDB's
    /// configuration and what is under it, its genres, its certifications and
    /// the countries it lists watch providers for; TheTVDB's languages,
    /// countries, genres, ratings, and the types and statuses its records are
    /// filed under. The path as the relay was asked it: `/3/…`, `/v4/…`.
    pub fn is_reference(path: &str) -> bool {
        const TMDB: &[&str] = &[
            "/3/genre/movie/list",
            "/3/genre/tv/list",
            "/3/certification/movie/list",
            "/3/certification/tv/list",
            "/3/watch/providers/regions",
        ];
        const TVDB: &[&str] = &[
            "languages",
            "countries",
            "genres",
            "content/ratings",
            "artwork/types",
            "artwork/statuses",
            "companies/types",
            "entities",
            "inspiration/types",
            "sources/types",
            "seasons/types",
            "series/statuses",
            "movies/statuses",
            "people/types",
            "genders",
        ];
        let path = path.trim_end_matches('/');
        path == "/3/configuration"
            || path.starts_with("/3/configuration/")
            || TMDB.contains(&path)
            || path
                .strip_prefix("/v4/")
                .is_some_and(|rest| TVDB.contains(&rest))
    }

    /// Keep a reference list a relay's service answered in the database, under
    /// `filed_as` ([`Ask::reference`]): a whole document of a path
    /// [`is_reference`] names, written as the service wrote it — TMDB's and
    /// TheTVDB's alone. What is past keeping goes as it is written. A failure
    /// is said in the log and costs the answer nothing.
    pub async fn keep_reference(
        state: &AppState,
        service: Service,
        filed_as: &str,
        path: &str,
        fetched: &Fetched,
    ) {
        if !service.keeps_references()
            || fetched.status != StatusCode::OK
            || fetched.stood_in.is_some()
            || fetched.body.len() > KEEP_MAX_BYTES
            || !is_reference(path)
        {
            return;
        }
        let Ok(body) = std::str::from_utf8(&fetched.body) else {
            return;
        };
        let document = relay_document::Document {
            content_type: content_type_of(&fetched.headers),
            body: body.to_string(),
            fetched_at: crate::db::now(),
        };
        if let Err(e) = relay_document::write(&state.db, service.name(), filed_as, &document).await
        {
            tracing::warn!(
                relay = service.name(),
                path,
                error = format_args!("{e:#}"),
                "a reference list could not be kept"
            );
            return;
        }
        if let Err(e) = relay_document::purge(&state.db, &relay_document::cutoff()).await {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "the reference lists past keeping could not be deleted"
            );
        }
    }

    /// The reference list a relay's service answered last under `filed_as`,
    /// kept in the database, while it is under [`relay_document::KEEP_DAYS`]
    /// old: what stands in when the memory holds nothing.
    pub async fn reread_reference(
        state: &AppState,
        service: Service,
        filed_as: &str,
        path: &str,
    ) -> Option<Fetched> {
        if !service.keeps_references() || !is_reference(path) {
            return None;
        }
        let document = match relay_document::read(
            &state.db,
            service.name(),
            filed_as,
            &relay_document::cutoff(),
        )
        .await
        {
            Ok(found) => found?,
            Err(e) => {
                tracing::warn!(
                    relay = service.name(),
                    path,
                    error = format_args!("{e:#}"),
                    "a reference list could not be read"
                );
                return None;
            }
        };
        let age = crate::db::parse_rfc3339(&document.fetched_at)
            .and_then(|at| u64::try_from((Utc::now() - at).num_seconds()).ok())
            .unwrap_or(0);
        Some(Fetched::stale(
            &document.content_type,
            Bytes::from(document.body),
            StoodIn::Database { age },
        ))
    }

    /// An exchange under way: what it comes to, the answer or the error.
    type Exchange = BoxFuture<'static, Result<Fetched, Arc<AppError>>>;

    /// What a flight comes to, shared by everyone who waited for it.
    type Flight = Shared<Exchange>;

    /// The exchanges under way, by what their answer is kept under — each with
    /// its number, so that one ending removes itself and never one that came
    /// after. Held weakly: a flight every caller gave up on ends there.
    type Flights = parking_lot::Mutex<HashMap<String, (u64, WeakShared<Exchange>)>>;

    static IN_FLIGHT: LazyLock<Flights> = LazyLock::new(Default::default);

    /// The number the next flight takes.
    static FLIGHTS: AtomicU64 = AtomicU64::new(0);

    /// A flight's way out of [`IN_FLIGHT`]: dropped when its exchange ends, or
    /// is given up on by every caller, it takes the flight's entry with it.
    struct Landing {
        key: String,
        id: u64,
    }

    impl Drop for Landing {
        fn drop(&mut self) {
            let mut flights = IN_FLIGHT.lock();
            if flights.get(&self.key).is_some_and(|(id, _)| *id == self.id) {
                flights.remove(&self.key);
            }
        }
    }

    /// One exchange for every caller asking for `key` at once: the first runs
    /// `fetch`, those that come while it runs wait for its answer — or its
    /// error — rather than asking the service again. Once it has ended, the
    /// next caller asks anew. A caller that gives up costs the others nothing;
    /// the exchange stops only when every one of them has.
    pub async fn together<F>(key: String, fetch: F) -> AppResult<Fetched>
    where
        F: Future<Output = AppResult<Fetched>> + Send + 'static,
    {
        let flight: Flight = {
            let mut flights = IN_FLIGHT.lock();
            match flights.get(&key).and_then(|(_, flight)| flight.upgrade()) {
                Some(under_way) => under_way,
                None => {
                    let id = FLIGHTS.fetch_add(1, Ordering::Relaxed);
                    let landing = Landing {
                        key: key.clone(),
                        id,
                    };
                    let flight = async move {
                        let _landing = landing;
                        fetch.await.map_err(Arc::new)
                    }
                    .boxed()
                    .shared();
                    if let Some(weak) = flight.downgrade() {
                        flights.insert(key, (id, weak));
                    }
                    flight
                }
            }
        };
        flight
            .await
            .map_err(|shared| Arc::try_unwrap(shared).unwrap_or_else(|shared| again(&shared)))
    }

    /// The same error once more, for another caller that waited for it: what it
    /// wraps carried over as text, which is what the log and the answer read of
    /// it.
    fn again(error: &AppError) -> AppError {
        match error {
            AppError::NotFound => AppError::NotFound,
            AppError::BadRequest(message) => AppError::BadRequest(message.clone()),
            AppError::Unauthorized => AppError::Unauthorized,
            AppError::Forbidden => AppError::Forbidden,
            AppError::Conflict(message) => AppError::Conflict(message.clone()),
            AppError::Refused { code, message } => AppError::Refused {
                code,
                message: message.clone(),
            },
            AppError::Disabled { code, message } => AppError::Disabled {
                code,
                message: message.clone(),
            },
            AppError::UpstreamUnavailable(cause) => {
                AppError::UpstreamUnavailable(anyhow::anyhow!("{cause:#}"))
            }
            AppError::ProviderNotConfigured => AppError::ProviderNotConfigured,
            AppError::RateLimited(throttle) => AppError::RateLimited(*throttle),
            AppError::LoopDetected => AppError::LoopDetected,
            AppError::Internal(cause) => AppError::Internal(anyhow::anyhow!("{cause:#}")),
            AppError::PayloadTooLarge(message) => AppError::PayloadTooLarge(message.clone()),
            AppError::UnsupportedMediaType(message) => {
                AppError::UnsupportedMediaType(message.clone())
            }
        }
    }

    // ─── What changed here, for the relays' feeds of changes ────────────────────

    /// The works changed here at `from` or after and up to `to` that have an
    /// id at `source`, one entry an id, the latest change first: what TMDB's
    /// `/3/movie/changes` and `/3/tv/changes`, and TheTVDB's `/v4/updates`, add
    /// to the service's own ([`crate::db::repo::change::changed_between`]).
    ///
    /// `to` is counted in, to the millisecond the journal keeps: a window
    /// that ends now — the one asked without an end — holds what was
    /// changed in this very millisecond, which an answer served from memory
    /// can follow that closely.
    pub async fn changed_here(
        state: &AppState,
        source: ExternalSource,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> AppResult<Vec<ChangedHere>> {
        Ok(crate::db::repo::change::changed_between(
            &state.db,
            source,
            &crate::db::to_rfc3339(from),
            &crate::db::to_rfc3339(to + chrono::TimeDelta::milliseconds(1)),
        )
        .await?)
    }

    /// What is kept of an answer, what is shared in flight, and what stands in
    /// for a service that fails.
    #[cfg(test)]
    mod tests {
        use std::sync::atomic::AtomicUsize;

        use super::*;

        fn document(body: &'static str) -> Fetched {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json;charset=utf-8"),
            );
            Fetched::new(StatusCode::OK, headers, Bytes::from_static(body.as_bytes()))
        }

        fn status(status: StatusCode) -> Fetched {
            Fetched::new(status, HeaderMap::new(), Bytes::new())
        }

        fn unheard() -> AppError {
            AppError::UpstreamUnavailable(anyhow::anyhow!("could not connect"))
        }

        fn ask(service: Service, key: &str, path: &str) -> Ask {
            Ask {
                service,
                key: Some(key.into()),
                path: path.into(),
                reference: None,
            }
        }

        /// An exchange that counts itself, and answers `with`.
        fn counted(
            calls: &Arc<AtomicUsize>,
            with: fn() -> AppResult<Fetched>,
        ) -> impl Future<Output = AppResult<Fetched>> + Send + 'static {
            let calls = Arc::clone(calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                with()
            }
        }

        /// A document the relay space held, fetched `age` seconds ago at `path`.
        async fn held(state: &AppState, key: &str, path: &str, age: u64, body: &'static str) {
            let fetched = cache::now_secs() - age;
            state
                .caches
                .relay
                .insert(
                    key.into(),
                    cache::Relayed {
                        status: 200,
                        content_type: "application/json".into(),
                        expires_at: fetched + cache::relay_ttl(path).as_secs(),
                        body: Bytes::from_static(body.as_bytes()),
                    },
                )
                .await;
        }

        #[test]
        fn the_reference_lists_are_those_a_client_starts_with() {
            for path in [
                "/3/configuration",
                "/3/configuration/languages",
                "/3/configuration/countries/",
                "/3/genre/movie/list",
                "/3/genre/tv/list",
                "/3/certification/movie/list",
                "/3/certification/tv/list",
                "/3/watch/providers/regions",
                "/v4/languages",
                "/v4/countries",
                "/v4/genres",
                "/v4/content/ratings",
                "/v4/artwork/types",
                "/v4/artwork/statuses",
                "/v4/companies/types",
                "/v4/entities",
                "/v4/inspiration/types",
                "/v4/sources/types",
                "/v4/seasons/types",
                "/v4/series/statuses",
                "/v4/movies/statuses",
                "/v4/people/types",
                "/v4/genders",
            ] {
                assert!(is_reference(path), "{path}");
            }
            for path in [
                "/3/movie/550",
                "/3/genre/movie",
                "/3/watch/providers/movie",
                "/3/configurations",
                "/4/list/1",
                "/v4/genres/12",
                "/v4/series/81189",
                "/v4/search",
                cache::ANILIST_QUERY,
                cache::ANILIST_COLLECTIONS,
            ] {
                assert!(!is_reference(path), "{path}");
            }
        }

        #[test]
        fn only_a_service_that_failed_calls_for_what_was_kept() {
            for code in [429, 500, 502, 503, 504] {
                let code = StatusCode::from_u16(code).unwrap();
                for service in [Service::Tmdb, Service::Tvdb, Service::Anilist] {
                    assert!(status_calls_for_stale(service, code), "{code}");
                }
            }
            for code in [200, 304, 400, 401, 404, 501] {
                let code = StatusCode::from_u16(code).unwrap();
                assert!(!status_calls_for_stale(Service::Tmdb, code), "{code}");
            }
            // AniList's API switched off answers 403; TMDB's 403 is a refusal.
            assert!(status_calls_for_stale(
                Service::Anilist,
                StatusCode::FORBIDDEN
            ));
            assert!(!status_calls_for_stale(
                Service::Tmdb,
                StatusCode::FORBIDDEN
            ));
            assert!(!status_calls_for_stale(
                Service::Tvdb,
                StatusCode::FORBIDDEN
            ));

            assert!(error_calls_for_stale(&unheard()));
            assert!(error_calls_for_stale(&AppError::RateLimited(
                Throttle::after(Duration::from_secs(3))
            )));
            assert!(error_calls_for_stale(&AppError::Disabled {
                code: "upstream_unavailable",
                message: "TheTVDB did not sign this server in".into(),
            }));
            for error in [
                AppError::Disabled {
                    code: "surface_disabled",
                    message: "off".into(),
                },
                AppError::BadRequest("that is not a TMDB path".into()),
                AppError::ProviderNotConfigured,
                AppError::Forbidden,
                AppError::Internal(anyhow::anyhow!("a bug")),
            ] {
                assert!(!error_calls_for_stale(&error), "{error}");
            }
        }

        #[test]
        fn a_door_that_did_not_let_a_relay_through_is_a_429_that_says_when() {
            let closed = unsent(
                Service::Tmdb,
                anyhow::Error::new(crate::providers::Closed {
                    label: "TMDB",
                    left: Duration::from_secs(30),
                }),
            );
            assert!(
                matches!(closed, AppError::RateLimited(t) if t.retry_after == Duration::from_secs(30))
            );
            let busy = unsent(
                Service::Anilist,
                anyhow::Error::new(crate::providers::Busy("AniList")),
            );
            assert!(
                matches!(busy, AppError::RateLimited(t) if t.retry_after == Duration::from_secs(1))
            );
            let failed = unsent(Service::Tvdb, anyhow::anyhow!("timed out"));
            assert!(matches!(failed, AppError::UpstreamUnavailable(_)));
            assert!(error_calls_for_stale(&failed));
            let response = closed.into_response();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "30");
        }

        #[test]
        fn an_error_shared_in_flight_is_the_same_error_for_each() {
            let shared = AppError::UpstreamUnavailable(
                anyhow::anyhow!("could not connect").context("TMDB request failed"),
            );
            let again = again(&shared);
            assert!(matches!(&again, AppError::UpstreamUnavailable(cause)
            if format!("{cause:#}") == "TMDB request failed: could not connect"));
            assert!(matches!(
                again_of(AppError::RateLimited(Throttle::after(Duration::from_secs(2)))),
                AppError::RateLimited(t) if t.retry_after == Duration::from_secs(2)
            ));
            assert!(matches!(
                again_of(AppError::Disabled {
                    code: "upstream_unavailable",
                    message: "later".into()
                }),
                AppError::Disabled {
                    code: "upstream_unavailable",
                    ..
                }
            ));
        }

        fn again_of(error: AppError) -> AppError {
            again(&error)
        }

        #[tokio::test]
        async fn callers_asking_the_same_at_once_share_one_exchange() {
            let calls = Arc::new(AtomicUsize::new(0));
            let gate = Arc::new(tokio::sync::Notify::new());
            let key = "test:together:one exchange".to_string();
            let fetch = || {
                let (calls, gate) = (Arc::clone(&calls), Arc::clone(&gate));
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    gate.notified().await;
                    Ok(document(r#"{"once":true}"#))
                }
            };

            let waiting: Vec<_> = (0..8)
                .map(|_| tokio::spawn(together(key.clone(), fetch())))
                .collect();
            // Every caller is waiting before the exchange is let go.
            while !IN_FLIGHT.lock().contains_key(&key) || calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            gate.notify_waiters();
            for answer in futures::future::join_all(waiting).await {
                let answer = answer.unwrap().unwrap();
                assert_eq!(answer.status, StatusCode::OK);
                assert_eq!(answer.body, Bytes::from_static(br#"{"once":true}"#));
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1, "one exchange for all");
            assert!(!IN_FLIGHT.lock().contains_key(&key), "cleared at the end");

            // Once it has ended, the next caller asks anew.
            let next = together(key.clone(), async { Ok(document("{}")) })
                .await
                .unwrap();
            assert_eq!(next.body, Bytes::from_static(b"{}"));
        }

        #[tokio::test]
        async fn an_error_in_flight_is_every_callers_and_a_flight_given_up_ends() {
            let calls = Arc::new(AtomicUsize::new(0));
            let key = "test:together:error".to_string();
            let (first, second) = tokio::join!(
                together(key.clone(), {
                    let calls = Arc::clone(&calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        Err(unheard())
                    }
                }),
                together(key.clone(), counted(&calls, || Ok(document("{}")))),
            );
            assert!(matches!(first, Err(AppError::UpstreamUnavailable(_))));
            assert!(matches!(second, Err(AppError::UpstreamUnavailable(_))));
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // Everybody gave up: the exchange is dropped, and its entry with it.
            let key = "test:together:given up".to_string();
            let abandoned = tokio::time::timeout(
                Duration::from_millis(20),
                together(key.clone(), async {
                    std::future::pending::<()>().await;
                    Ok(document("{}"))
                }),
            )
            .await;
            assert!(abandoned.is_err());
            assert!(!IN_FLIGHT.lock().contains_key(&key));
            let fresh = together(key, async { Ok(document(r#"{"anew":1}"#)) })
                .await
                .unwrap();
            assert_eq!(fresh.body, Bytes::from_static(br#"{"anew":1}"#));
        }

        #[tokio::test]
        async fn a_document_kept_is_served_while_good_and_stands_in_when_the_service_fails() {
            let state = crate::api::native::testing::server().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let path = "/3/movie/9200001";
            let key = "tmdb:test:/3/movie/9200001";

            // Asked, kept, and served from memory the next time.
            let first = answer(
                &state,
                ask(Service::Tmdb, key, path),
                counted(&calls, || Ok(document(r#"{"id":9200001}"#))),
            )
            .await
            .unwrap();
            assert_eq!(first.stood_in(), None);
            let again = answer(
                &state,
                ask(Service::Tmdb, key, path),
                counted(&calls, || Ok(status(StatusCode::BAD_GATEWAY))),
            )
            .await
            .unwrap();
            assert_eq!(again.status, StatusCode::OK);
            assert_eq!(again.body, first.body);
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // Past its end: TMDB is asked, fails, and what was kept stands in,
            // with its age.
            held(&state, key, path, 7 * 3600, r#"{"id":9200001,"old":true}"#).await;
            let failures: [fn() -> AppResult<Fetched>; 4] = [
                || Ok(status(StatusCode::SERVICE_UNAVAILABLE)),
                || Ok(status(StatusCode::TOO_MANY_REQUESTS)),
                || Err(unheard()),
                || {
                    Err(AppError::RateLimited(Throttle::after(Duration::from_secs(
                        20,
                    ))))
                },
            ];
            for failure in failures {
                let stale = answer(
                    &state,
                    ask(Service::Tmdb, key, path),
                    counted(&calls, failure),
                )
                .await
                .unwrap();
                assert_eq!(stale.status, StatusCode::OK);
                assert_eq!(
                    stale.body,
                    Bytes::from_static(br#"{"id":9200001,"old":true}"#)
                );
                let age: u64 = stale.headers[header::AGE]
                    .to_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!((7 * 3600..7 * 3600 + 60).contains(&age), "{age}");
                assert!(matches!(stale.stood_in(), Some(StoodIn::Memory { .. })));
                assert_eq!(
                    stale.headers[header::CONTENT_TYPE],
                    "application/json",
                    "as it was kept"
                );
            }

            // A refusal of the request itself is the service's answer.
            let refused = answer(
                &state,
                ask(Service::Tmdb, key, path),
                counted(&calls, || Ok(status(StatusCode::UNAUTHORIZED))),
            )
            .await
            .unwrap();
            assert_eq!(refused.status, StatusCode::UNAUTHORIZED);

            // A good answer takes the old one's place.
            let renewed = answer(
                &state,
                ask(Service::Tmdb, key, path),
                counted(&calls, || Ok(document(r#"{"id":9200001,"new":true}"#))),
            )
            .await
            .unwrap();
            assert_eq!(renewed.stood_in(), None);
            let kept = state.caches.relay.get(key).await.unwrap();
            assert!(kept.is_fresh());
            assert_eq!(kept.body, renewed.body);
        }

        #[tokio::test]
        async fn nothing_stands_in_past_its_window_nor_when_switched_off() {
            let state = crate::api::native::testing::server().await;
            let calls = Arc::new(AtomicUsize::new(0));

            // A week and an hour: too old for TMDB.
            let (key, path) = ("tmdb:test:/3/tv/9200002", "/3/tv/9200002");
            held(&state, key, path, 7 * 86_400 + 3600, "{}").await;
            let failed = answer(
                &state,
                ask(Service::Tmdb, key, path),
                counted(&calls, || Ok(status(StatusCode::BAD_GATEWAY))),
            )
            .await
            .unwrap();
            assert_eq!(failed.status, StatusCode::BAD_GATEWAY);
            assert_eq!(failed.stood_in(), None);

            // A day and an hour: too old for AniList, young enough for TheTVDB.
            let day_and_hour = 86_400 + 3600;
            let (anilist, query) = ("anilist:test:q", cache::ANILIST_QUERY);
            held(&state, anilist, query, day_and_hour, "{}").await;
            let failed = answer(
                &state,
                ask(Service::Anilist, anilist, query),
                counted(&calls, || Err(unheard())),
            )
            .await;
            assert!(matches!(failed, Err(AppError::UpstreamUnavailable(_))));
            let (tvdb, series) = ("tvdb:test:/v4/series/9200003", "/v4/series/9200003");
            held(&state, tvdb, series, day_and_hour, r#"{"data":{}}"#).await;
            let stale = answer(
                &state,
                ask(Service::Tvdb, tvdb, series),
                counted(&calls, || Err(unheard())),
            )
            .await
            .unwrap();
            assert!(matches!(stale.stood_in(), Some(StoodIn::Memory { .. })));

            // AniList's API switched off is a failure of the service's: two
            // hours, past the hour a query is good for, inside the day.
            held(
                &state,
                anilist,
                query,
                2 * 3600,
                r#"{"data":{"Media":null}}"#,
            )
            .await;
            let stale = answer(
                &state,
                ask(Service::Anilist, anilist, query),
                counted(&calls, || Ok(status(StatusCode::FORBIDDEN))),
            )
            .await
            .unwrap();
            assert_eq!(stale.status, StatusCode::OK);
            assert!(matches!(stale.stood_in(), Some(StoodIn::Memory { .. })));

            // Off: the failure is handed back as it came.
            state
                .settings
                .set(
                    crate::settings::Scope::Server,
                    "",
                    STALE_IF_ERROR,
                    "false",
                    None,
                )
                .await
                .unwrap();
            assert!(!stale_if_error(&state));
            let failed = answer(
                &state,
                ask(Service::Tvdb, tvdb, series),
                counted(&calls, || Ok(status(StatusCode::SERVICE_UNAVAILABLE))),
            )
            .await
            .unwrap();
            assert_eq!(failed.status, StatusCode::SERVICE_UNAVAILABLE);
        }

        #[tokio::test]
        async fn a_feed_of_changes_hands_its_failure_on() {
            let state = crate::api::native::testing::server().await;
            let calls = Arc::new(AtomicUsize::new(0));
            for (service, key, path) in [
                (
                    Service::Tmdb,
                    "tmdb:test:/3/movie/changes",
                    "/3/movie/changes",
                ),
                (Service::Tvdb, "tvdb:test:/v4/updates", "/v4/updates"),
            ] {
                // Twenty minutes old: past its quarter of an hour.
                held(&state, key, path, 20 * 60, r#"{"results":[]}"#).await;
                let failed = answer(
                    &state,
                    ask(service, key, path),
                    counted(&calls, || Ok(status(StatusCode::BAD_GATEWAY))),
                )
                .await
                .unwrap();
                assert_eq!(failed.status, StatusCode::BAD_GATEWAY, "{path}");
                assert_eq!(failed.stood_in(), None);
            }
        }

        #[tokio::test]
        async fn a_closed_door_with_nothing_kept_is_a_429_and_a_clients_own_request_stands_alone() {
            let state = crate::api::native::testing::server().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let closed = answer(
                &state,
                ask(
                    Service::Tmdb,
                    "tmdb:test:/3/movie/9200004",
                    "/3/movie/9200004",
                ),
                counted(&calls, || {
                    Err(unsent(
                        Service::Tmdb,
                        anyhow::Error::new(crate::providers::Closed {
                            label: "TMDB",
                            left: Duration::from_secs(12),
                        }),
                    ))
                }),
            )
            .await;
            assert!(
                matches!(closed, Err(AppError::RateLimited(t)) if t.retry_after == Duration::from_secs(12))
            );

            // Without a key: asked every time, kept for nobody, and nothing
            // stands in for it.
            let theirs = Ask {
                key: None,
                ..ask(Service::Tmdb, "", "/3/movie/9200005")
            };
            for _ in 0..2 {
                answer(
                    &state,
                    theirs.clone(),
                    counted(&calls, || Ok(document("{}"))),
                )
                .await
                .unwrap();
            }
            let failed = answer(&state, theirs, counted(&calls, || Err(unheard()))).await;
            assert!(failed.is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 4);
        }

        #[tokio::test]
        async fn a_no_such_thing_is_kept_by_the_relays_that_keep_one() {
            let state = crate::api::native::testing::server().await;
            let calls = Arc::new(AtomicUsize::new(0));
            for (service, key, path, asked) in [
                (
                    Service::Tmdb,
                    "tmdb:test:/3/movie/9200006",
                    "/3/movie/9200006",
                    2,
                ),
                (
                    Service::Tvdb,
                    "tvdb:test:/v4/movies/9200006",
                    "/v4/movies/9200006",
                    1,
                ),
            ] {
                let before = calls.load(Ordering::SeqCst);
                for _ in 0..2 {
                    let missing = answer(
                        &state,
                        ask(service, key, path),
                        counted(&calls, || Ok(status(StatusCode::NOT_FOUND))),
                    )
                    .await
                    .unwrap();
                    assert_eq!(missing.status, StatusCode::NOT_FOUND);
                }
                assert_eq!(calls.load(Ordering::SeqCst) - before, asked, "{service:?}");
            }
            // A "no such thing" kept leaves the memory at its end.
            let kept = state
                .caches
                .relay
                .get("tvdb:test:/v4/movies/9200006")
                .await
                .unwrap();
            assert!(cache::Cached::keep_for(&kept).unwrap() <= Duration::from_secs(300));
        }

        #[tokio::test]
        async fn a_reference_list_is_kept_in_the_database_and_stands_in_when_memory_has_nothing() {
            let state = crate::api::native::testing::server().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let path = "/3/configuration";
            let filed_as = "https://tmdb.test:/3/configuration?#accept=";
            let configuration = || Ask {
                reference: Some(filed_as.into()),
                ..ask(Service::Tmdb, "tmdb:7:test:/3/configuration", path)
            };

            answer(
                &state,
                configuration(),
                counted(&calls, || Ok(document(r#"{"images":{"base_url":"x"}}"#))),
            )
            .await
            .unwrap();
            let kept = relay_document::read(&state.db, "tmdb", filed_as, &relay_document::cutoff())
                .await
                .unwrap()
                .expect("kept in the database");
            assert_eq!(kept.body, r#"{"images":{"base_url":"x"}}"#);
            assert_eq!(kept.content_type, "application/json;charset=utf-8");

            // A start later: the memory is empty, and TMDB is down.
            state.caches.relay.invalidate_all().await;
            let stale = answer(&state, configuration(), counted(&calls, || Err(unheard())))
                .await
                .unwrap();
            assert_eq!(stale.status, StatusCode::OK);
            assert_eq!(
                stale.body,
                Bytes::from_static(br#"{"images":{"base_url":"x"}}"#)
            );
            assert!(matches!(stale.stood_in(), Some(StoodIn::Database { .. })));
            assert!(stale.headers.contains_key(header::AGE));

            // A path that is no reference list is never written there, and
            // AniList's never are.
            for (service, key, path) in [
                (
                    Service::Tmdb,
                    "tmdb:test:/3/movie/9200007",
                    "/3/movie/9200007",
                ),
                (
                    Service::Anilist,
                    "anilist:test:genres",
                    cache::ANILIST_COLLECTIONS,
                ),
            ] {
                answer(
                    &state,
                    Ask {
                        reference: Some(format!("{key}#ref")),
                        ..ask(service, key, path)
                    },
                    counted(&calls, || Ok(document("{}"))),
                )
                .await
                .unwrap();
            }
            let rows = sqlx::query(state.db.sql("SELECT relay, cache_key FROM relay_document"))
                .fetch_all(state.db.pool())
                .await
                .unwrap();
            assert_eq!(rows.len(), 1, "the configuration alone");
        }

        #[tokio::test]
        async fn neither_a_stand_in_nor_a_document_past_its_size_is_kept() {
            let state = crate::api::native::testing::server().await;
            let path = "/3/movie/9200008";
            let key = "tmdb:test:/3/movie/9200008";
            let big = Fetched::new(
                StatusCode::OK,
                HeaderMap::new(),
                Bytes::from(vec![b' '; KEEP_MAX_BYTES + 1]),
            );
            keep(&state, &ask(Service::Tmdb, key, path), &big).await;
            assert!(state.caches.relay.get(key).await.is_none());

            let stale = Fetched::stale(
                "application/json",
                Bytes::from_static(b"{}"),
                StoodIn::Memory { age: 9 },
            );
            keep(&state, &ask(Service::Tmdb, key, path), &stale).await;
            assert!(state.caches.relay.get(key).await.is_none());
        }
    }
}

// ─── The text a work is answered with, by language ──────────────────────────

/// Lay over a work the texts a relayed document in `asked` is written with,
/// and say them locked — a language as a client wrote it, `fr-FR` or
/// TheTVDB's `fra`. In the language the catalogue keeps its works in, the
/// work as it is read: a text locked in that language, else in every one.
/// In another, only a text locked in that language itself: a text locked in
/// every language is the catalogue's own, and a service's document in
/// another language keeps the service's text in it — a German Jellyseerr is
/// answered TMDB's German, as it was before a lock had a language. What is
/// not text, a date or a picture, is written in every language.
///
/// The relays' rule alone. The native API, Sonarr, Radarr, Plex and the
/// `.nfo` serve their own documents, and a text locked in every language is
/// theirs in any language ([`language::lay_locks`]).
pub fn lay_locks(state: &AppState, item: &mut MediaItem, asked: &str) {
    let asked = language::normalize(asked);
    if !asked.is_empty() && asked == language::stored(state) {
        return;
    }
    fields::lay_only_in(item, &asked);
}

/// The text a person locked for a field of a work — the work's own, `item`;
/// a season's, `season:1`; an episode's, `episode:1x2` — as a relayed
/// document in `language` carries it ([`lay_locks`]): in the catalogue's
/// own language, `stored` (`fra`), the one locked in it, else the one locked
/// in every language ([`fields::text_lock`]); in another, only one locked in
/// that language itself. `language` as a client wrote it: `fr-FR`, or
/// TheTVDB's `fra`. None where nothing was locked for it, or the text was
/// locked cleared.
pub fn locked_text(
    item: &MediaItem,
    scope: &str,
    field: &str,
    language: &str,
    stored: &str,
) -> Option<String> {
    let language = language::normalize(language);
    let lock = if !language.is_empty() && language == stored {
        fields::text_lock(item, scope, field, &language)
    } else {
        item.locked_texts.iter().find(|lock| {
            lock.scope == scope
                && lock.field == field
                && !language.is_empty()
                && lock.language == language
        })
    };
    lock.and_then(|lock| lock.value.clone())
        .filter(|text| !text.trim().is_empty())
}

/// The languages a person locked a field of a work in alone, with the text
/// they locked: what a document that lists its languages one by one is
/// given where it has none of its own in them.
pub fn locked_languages(item: &MediaItem, scope: &str, field: &str) -> Vec<(String, String)> {
    item.locked_texts
        .iter()
        .filter(|lock| lock.scope == scope && lock.field == field && !lock.language.is_empty())
        .filter_map(|lock| {
            let text = lock.value.clone().filter(|text| !text.trim().is_empty())?;
            Some((lock.language.clone(), text))
        })
        .collect()
}

/// What the relays' tests share: a work as it is read with its locks, as a
/// document in another language is written from it, and the whole server
/// with the services it relays at a stand-in that answers what a test says
/// and keeps what it was asked.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use axum::{Router, extract::Request, response::IntoResponse};
    use serde_json::Value;

    use super::*;
    use crate::{
        config,
        domain::fields::{self, Override},
    };

    /// The operator's own keys, as the stand-in sees them.
    pub const OPERATOR_TMDB_KEY: &str = "operator-tmdb-key";
    pub const OPERATOR_TVDB_KEY: &str = "operator-tvdb-key";

    /// A work as it is read in a catalogue kept in French, these locks
    /// applied: `(scope, field, language, value)`, the language empty for a
    /// lock in every one.
    pub fn read_with(mut item: MediaItem, locks: &[(&str, &str, &str, Value)]) -> MediaItem {
        let overrides: Vec<Override> = locks
            .iter()
            .map(|(scope, field, language, value)| Override {
                scope: scope.to_string(),
                field: field.to_string(),
                language: (!language.is_empty()).then(|| language.to_string()),
                value: Some(value.clone()),
                updated_at: crate::db::now(),
                updated_by: None,
            })
            .collect();
        fields::apply_in(&mut item, &overrides, "fra").expect("the locks apply");
        item
    }

    /// The work as a relayed document in `language`, another than the
    /// catalogue's, is written from: what [`super::lay_locks`] makes of it.
    pub fn laid_for(mut item: MediaItem, language: &str) -> MediaItem {
        fields::lay_only_in(&mut item, language);
        item
    }

    /// One request the stand-in was asked.
    #[derive(Clone, Debug)]
    pub struct Asked {
        pub method: String,
        pub path: String,
        pub query: String,
        pub authorization: Option<String>,
        pub body: String,
    }

    /// What the stand-in answers a request with: a status and a document.
    pub type Answers = fn(&Asked) -> (StatusCode, Value);

    /// A stand-in for the services relayed: it answers each request as
    /// `answers` says, and keeps what it was asked.
    pub struct StandIn {
        pub at: String,
        asked: Arc<Mutex<Vec<Asked>>>,
    }

    impl StandIn {
        pub async fn start(answers: Answers) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a port");
            let at = format!("http://{}", listener.local_addr().expect("its address"));
            let asked = Arc::new(Mutex::new(Vec::new()));
            let kept = Arc::clone(&asked);
            let app = Router::new().fallback(move |request: Request| {
                let kept = Arc::clone(&kept);
                async move {
                    let (parts, body) = request.into_parts();
                    let body = axum::body::to_bytes(body, 1 << 20)
                        .await
                        .unwrap_or_default();
                    let one = Asked {
                        method: parts.method.to_string(),
                        path: parts.uri.path().to_string(),
                        query: parts.uri.query().unwrap_or_default().to_string(),
                        authorization: parts
                            .headers
                            .get(header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string),
                        body: String::from_utf8_lossy(&body).into_owned(),
                    };
                    let (status, document) = answers(&one);
                    kept.lock().expect("the list").push(one);
                    (status, Json(document)).into_response()
                }
            });
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self { at, asked }
        }

        /// What it was asked at a path, in order.
        pub fn asked_at(&self, path: &str) -> Vec<Asked> {
            self.asked
                .lock()
                .expect("the list")
                .iter()
                .filter(|asked| asked.path == path)
                .cloned()
                .collect()
        }
    }

    /// The whole server, its catalogue kept in French, TMDB and TheTVDB
    /// keyed as an operator keys them, every service at `stand_in`, and its
    /// relays guarded by a key.
    pub async fn server(stand_in: &StandIn) -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.security.auth_disabled = false;
        config.security.tmdb_policy = SurfacePolicy::ApiKey;
        config.security.tvdb_policy = SurfacePolicy::ApiKey;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = Some(OPERATOR_TMDB_KEY.into());
        config.tmdb.language = "fr-FR".into();
        config.tmdb.passthrough = true;
        config.tvdb.api_key = Some(OPERATOR_TVDB_KEY.into());
        config.tvdb.pin = None;
        config.tvdb.passthrough = true;
        let at = stand_in.at.clone();
        for upstream in [
            &mut config.tmdb.upstream,
            &mut config.fanart.upstream,
            &mut config.skyhook.upstream,
            &mut config.sonarr_services.upstream,
            &mut config.sonarr_services.xem_upstream,
            &mut config.radarr_metadata.upstream,
            &mut config.tvmaze.upstream,
            &mut config.anilist.upstream,
            &mut config.mal.upstream,
            &mut config.mal.jikan_upstream,
            &mut config.mal.tenrai_upstream,
            &mut config.fankai.upstream,
            &mut config.fankai_wiki.upstream,
            &mut config.imdb.datasets,
            &mut config.anime_mapping.url,
        ] {
            upstream.clone_from(&at);
        }
        config.tvdb.upstream = format!("{at}/v4");
        AppState::bootstrap(config).await.expect("a server")
    }

    /// Store a work, and lock these fields of it: `(scope, field, language,
    /// value)`, as [`read_with`] takes them.
    pub async fn store(state: &AppState, item: &MediaItem, locks: &[(&str, &str, &str, Value)]) {
        crate::db::repo::item::upsert(
            &state.db,
            crate::db::repo::item::ItemWrite {
                item,
                replace_children: true,
            },
        )
        .await
        .expect("the work is stored");
        for (scope, field, language, value) in locks {
            crate::db::repo::override_field::set_in(
                &state.db,
                &item.id,
                scope,
                field,
                language,
                Some(value),
                None,
            )
            .await
            .expect("the lock is stored");
        }
    }

    /// Choose whose key a relay asks its service with.
    pub async fn choose(state: &AppState, key: &str, value: &str) {
        state
            .settings
            .set(crate::settings::Scope::Server, "", key, value, None)
            .await
            .expect("the setting is written");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn a_request_names_its_service_by_its_host() {
        let path: Uri = "/v1/time".parse().unwrap();
        let hosts = &["services.sonarr.tv", "graphql.anilist.co"];
        assert!(addressed_to(&host("services.sonarr.tv"), &path, hosts));
        assert!(addressed_to(&host("GraphQL.AniList.co:443"), &path, hosts));
        assert!(addressed_to(&host("graphql.anilist.co."), &path, hosts));
        assert!(!addressed_to(&host("skyhook.sonarr.tv"), &path, hosts));
        assert!(!addressed_to(
            &host("graphql.anilist.co.evil"),
            &path,
            hosts
        ));
        assert!(!addressed_to(&HeaderMap::new(), &path, hosts));

        // HTTP/2 carries it in the address.
        let full: Uri = "https://graphql.anilist.co/".parse().unwrap();
        assert!(addressed_to(&HeaderMap::new(), &full, hosts));
        // Named in both, `Host` is the one read, as the doors read it.
        assert!(!addressed_to(&host("skyhook.sonarr.tv"), &full, hosts));
    }

    #[test]
    fn a_key_for_this_server_is_not_relayed_in_the_address() {
        let uri: Uri = "/v1/update/main/changes?version=4.0&apikey=ams_x&os=linux&API_KEY=y"
            .parse()
            .unwrap();
        assert_eq!(
            without_our_params(&uri),
            "/v1/update/main/changes?version=4.0&os=linux"
        );
        let bare: Uri = "/v1/time?apikey=ams_x".parse().unwrap();
        assert_eq!(without_our_params(&bare), "/v1/time");
        assert_eq!(query_without(&bare, OUR_PARAMS), None);
    }

    #[test]
    fn a_cached_document_is_filed_under_one_spelling_of_its_query() {
        let a: Uri = "/v4/series/1?page=1&meta=episodes&apikey=k"
            .parse()
            .unwrap();
        let b: Uri = "/v4/series/1?meta=episodes&page=1".parse().unwrap();
        assert_eq!(sorted_query(&a, OUR_PARAMS), sorted_query(&b, OUR_PARAMS));
        assert_eq!(sorted_query(&a, OUR_PARAMS), "meta=episodes&page=1");
        let none: Uri = "/v4/series/1".parse().unwrap();
        assert_eq!(sorted_query(&none, OUR_PARAMS), "");

        // A parameter given twice is read last-wins upstream: two orders,
        // two answers, two entries.
        let fr_last: Uri = "/3/movie/1?language=de&page=1&language=fr".parse().unwrap();
        let de_last: Uri = "/3/movie/1?language=fr&page=1&language=de".parse().unwrap();
        assert_ne!(
            sorted_query(&fr_last, OUR_PARAMS),
            sorted_query(&de_last, OUR_PARAMS)
        );
        assert_eq!(
            sorted_query(&fr_last, OUR_PARAMS),
            "language=de&language=fr&page=1"
        );
    }

    #[test]
    fn only_a_whole_document_or_a_no_such_thing_is_kept() {
        assert_eq!(
            kept_for(StatusCode::OK, "/v4/series/1"),
            Some(cache::relay_ttl("/v4/series/1"))
        );
        assert_eq!(
            kept_for(StatusCode::NOT_FOUND, "/v4/series/1"),
            Some(Duration::from_secs(5 * 60))
        );
        for status in [
            StatusCode::CREATED,
            StatusCode::NON_AUTHORITATIVE_INFORMATION,
            StatusCode::NO_CONTENT,
            StatusCode::PARTIAL_CONTENT,
            StatusCode::MOVED_PERMANENTLY,
            StatusCode::FOUND,
            StatusCode::NOT_MODIFIED,
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
        ] {
            assert_eq!(kept_for(status, "/v4/series/1"), None, "{status}");
        }
    }

    #[test]
    fn a_path_that_climbs_out_of_its_place_is_refused() {
        // The outgoing URL is built by interpolation and parsed by reqwest,
        // which resolves dot segments — so this one would have left the v3
        // API for the v4 one, carrying the operator's credentials with it.
        assert!(climbs("/3/%2e%2e/4/account"));
        // A backslash is a slash to that parser.
        assert!(climbs("/4/list/..\\account/1/lists"));
        assert!(climbs("/4/list/8136%2F..%2F..%2Faccount"));
        assert!(climbs("/3/movie/238%5C..%5C4"));
        assert!(climbs("/3/../4/account"));
        assert!(climbs("/v4/series/%2E%2E/user"));
        assert!(climbs("/v4/./series/1"));

        assert!(!climbs("/3/tv/1396"));
        assert!(!climbs("/v4/series/81189/extended"));
        assert!(!climbs("/v4/search"));
        // A dot inside a segment is just a character.
        assert!(!climbs("/3/configuration/countries.json"));
    }

    #[test]
    fn an_answer_keeps_its_headers_but_not_the_hops_or_a_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(header::SET_COOKIE, HeaderValue::from_static("a=b"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("29"));
        headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
        headers.insert(
            crate::providers::radarr::LOOP_HEADER,
            HeaderValue::from_static("another-instance"),
        );

        let kept = kept(&headers);

        assert_eq!(kept.get(header::CONTENT_TYPE).unwrap(), "application/json");
        assert_eq!(kept.get("x-ratelimit-remaining").unwrap(), "29");
        assert!(kept.get(header::CONNECTION).is_none());
        assert!(kept.get(header::SET_COOKIE).is_none());
        assert!(kept.get(crate::providers::radarr::LOOP_HEADER).is_none());
        assert!(kept.get("access-control-allow-origin").is_none());
    }

    #[test]
    fn a_request_one_of_our_instances_sent_is_a_loop() {
        let mut headers = HeaderMap::new();
        assert!(!looped(&headers));
        headers.insert(
            crate::providers::radarr::LOOP_HEADER,
            HeaderValue::from_static("another-instance"),
        );
        assert!(looped(&headers));
    }

    #[test]
    fn only_the_headers_named_travel() {
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, HeaderValue::from_static("Yamtrack"));
        headers.insert(header::COOKIE, HeaderValue::from_static("ams_session=x"));
        headers.insert("x-api-key", HeaderValue::from_static("ams_k"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.0.0.2"));
        let asked = asked_with(&headers, &["user-agent", "accept"]);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked.get(header::USER_AGENT).unwrap(), "Yamtrack");
    }

    #[test]
    fn a_bearer_token_is_read_whatever_its_case() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer  their.jwt.token "),
        );
        assert_eq!(bearer(&headers).as_deref(), Some("their.jwt.token"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer ams_k"),
        );
        assert_eq!(bearer(&headers).as_deref(), Some("ams_k"));
        // Any case, and any blank after the scheme.
        for written in ["BEARER ams_k", "BeArEr\tams_k", "  Bearer   ams_k  "] {
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(written).unwrap(),
            );
            assert_eq!(bearer(&headers).as_deref(), Some("ams_k"), "{written:?}");
        }
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer(&headers), None);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearerams_k"),
        );
        assert_eq!(bearer(&headers), None);
    }

    #[test]
    fn every_spelling_of_a_key_the_guard_takes_is_stripped() {
        for name in crate::auth::middleware::API_KEY_PARAMS {
            assert!(
                OUR_PARAMS.contains(name),
                "{name} authenticates here but would be relayed"
            );
        }
    }
}

#[cfg(test)]
mod credential_tests {
    use serde_json::json;

    use super::{testing::read_with, *};
    use crate::domain::MediaKind;

    #[test]
    fn a_relay_asked_with_each_clients_key_is_guarded_by_address() {
        // A key issued here is refused there: asking for one would refuse
        // everybody.
        assert_eq!(
            guarded(SurfacePolicy::ApiKey, RelayCredential::Client),
            SurfacePolicy::Allowlist
        );
        assert_eq!(
            guarded(SurfacePolicy::ApiKey, RelayCredential::Server),
            SurfacePolicy::ApiKey
        );
        for policy in [SurfacePolicy::Allowlist, SurfacePolicy::Open] {
            for credential in [RelayCredential::Server, RelayCredential::Client] {
                assert_eq!(guarded(policy, credential), policy);
            }
        }
        assert_eq!(
            RelayCredential::setting(Surface::Tmdb),
            Some("tmdb.relayCredential")
        );
        assert_eq!(
            RelayCredential::setting(Surface::Tvdb),
            Some("tvdb.relayCredential")
        );
        assert_eq!(RelayCredential::setting(Surface::Anilist), None);
    }

    #[test]
    fn a_key_issued_here_is_found_wherever_a_key_is_read() {
        let none = HeaderMap::new();
        let at = |text: &str| -> Uri { text.parse().unwrap() };
        assert!(carries_key_issued_here(
            &none,
            &at("/3/movie/1?api_key=ams_x")
        ));
        assert!(carries_key_issued_here(
            &none,
            &at("/3/movie/1?APIKEY=ams_x")
        ));
        assert!(carries_key_issued_here(
            &none,
            &at("/3/movie/1?api%5Fkey=ams_x")
        ));
        assert!(!carries_key_issued_here(
            &none,
            &at("/3/movie/1?api_key=their-tmdb-key")
        ));
        assert!(!carries_key_issued_here(
            &none,
            &at("/3/movie/1?query=ams_x")
        ));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer ams_x"),
        );
        assert!(carries_key_issued_here(&headers, &at("/v4/series/1")));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer their.jwt.token"),
        );
        assert!(!carries_key_issued_here(&headers, &at("/v4/series/1")));
        headers.insert("x-api-key", HeaderValue::from_static("ams_x"));
        assert!(carries_key_issued_here(&headers, &at("/v4/series/1")));
    }

    #[test]
    fn a_key_issued_here_is_found_whatever_the_header_wraps_it_in() {
        let at: Uri = "/3/movie/1".parse().unwrap();
        let with = |values: &[&str]| {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            }
            headers
        };
        // The scheme in capitals, or another scheme altogether.
        for carried in [
            "BEARER ams_x",
            "bEaReR ams_x",
            "Token ams_x",
            "ams_x",
            "Token token=\"ams_x\"",
            // After the client's own token, in the same value.
            "Bearer their.jwt.token ams_x",
            "Bearer their.jwt.token, ams_x",
            // A name and a password: `user:ams_x`, and `ams_x:` .
            "Basic dXNlcjphbXNfeA==",
            "Basic YW1zX3g6",
            // Its prefix in capitals is still none to hand on.
            "Bearer AMS_x",
        ] {
            assert!(
                carries_key_issued_here(&with(&[carried]), &at),
                "{carried:?}"
            );
            assert!(authorization_carries_key_issued_here(&with(&[carried])));
        }
        // In a second value of the header, behind the client's own.
        assert!(carries_key_issued_here(
            &with(&["Bearer their.jwt.token", "Bearer ams_x"]),
            &at
        ));
        // In X-Api-Key, behind a scheme.
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("Bearer ams_x"));
        assert!(carries_key_issued_here(&headers, &at));

        // The client's own credential alone is theirs.
        for theirs in ["Bearer their.jwt.token", "Basic dXNlcjpwYXNz", "Token abc"] {
            assert!(
                !carries_key_issued_here(&with(&[theirs]), &at),
                "{theirs:?}"
            );
            assert!(!authorization_carries_key_issued_here(&with(&[theirs])));
        }
    }

    #[test]
    fn a_text_is_locked_for_a_language_in_it_or_in_every_one() {
        let item = read_with(
            MediaItem::empty(MediaKind::Series),
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "overview", "deu", json!("Mein Überblick")),
                ("season:1", "title", "ita", json!("La prima")),
            ],
        );
        // In the catalogue's own language, a text locked in every one.
        assert_eq!(
            locked_text(&item, "item", "title", "fr-FR", "fra").as_deref(),
            Some("Mon titre")
        );
        // In another, only what was locked in it: the service's own text
        // stands where the lock is every language's.
        assert_eq!(locked_text(&item, "item", "title", "es-ES", "fra"), None);
        assert_eq!(
            locked_text(&item, "item", "overview", "de", "fra").as_deref(),
            Some("Mein Überblick")
        );
        assert_eq!(locked_text(&item, "item", "overview", "fra", "fra"), None);
        // A language that names none is none of them.
        assert_eq!(locked_text(&item, "item", "title", "", "fra"), None);
        assert_eq!(
            locked_languages(&item, "season:1", "title"),
            [("ita".to_string(), "La prima".to_string())]
        );
        // A lock in every language is the work's own text, and no language
        // of its own.
        assert!(locked_languages(&item, "item", "title").is_empty());
    }
}
