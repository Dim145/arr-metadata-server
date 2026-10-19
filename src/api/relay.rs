//! What the relays share: a request handed on to a service in a client's
//! place, and its answer handed back.
//!
//! Four services are relayed this way — TMDB under `/3`, TheTVDB under `/v4`,
//! AniList by its name, and Sonarr's own services for whatever this server
//! does not answer itself. Each keeps its own rules for the credentials it
//! stands in for and for what it writes into an answer; what they have in
//! common is here: which headers travel, which never do, what a path may not
//! say, whose credential the service is asked with, the work a document is
//! patched for, and the text it is patched with in the language asked.

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
