//! TheTVDB compatibility: `/v4/*`, in `api4.thetvdb.com`'s place.
//!
//! What the TMDB relay is for the clients that speak TMDB, this is for those
//! that speak TheTVDB's v4 API and have its name compiled in — Yamtrack,
//! Jellyfin's plugin, Kodi's scraper. A request is handed on to TheTVDB and
//! its answer handed back, with the fields a person locked here written into
//! it on the way: a series' name, its overview, an episode's title.
//!
//! TheTVDB's clients sign in first — `POST /v4/login` with their key — for a
//! token they carry on every call after. A client given a key issued here
//! signs in with it and is answered that same key as its token, so every
//! call after carries a credential this server knows; TheTVDB is then asked
//! with the operator's own token in the client's place, and the client's
//! copy never reaches it. A client that brings its own TheTVDB key is signed
//! in with it and its token handed on as it came, where the surface's policy
//! lets it through by address.
//!
//! Reads only. TheTVDB's one write is a user's favourites, and a relay that
//! forwarded it would let any key issued here edit the operator's.

use std::time::Instant;

use axum::{
    Extension, Json,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::any,
};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::relay,
    auth::{Identity, middleware as guards, secrets},
    cache,
    config::{Api, Surface, SurfacePolicy},
    domain::{CoverType, Episode, ExternalSource, MediaItem},
    error::{AppError, AppResult},
    state::AppState,
};

/// The name TheTVDB's clients call it by.
pub const HOSTS: &[&str] = &["api4.thetvdb.com"];

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "TheTVDB compatibility";

/// Where a client signs in.
pub const LOGIN_PATH: &str = "/v4/login";

/// Where TheTVDB's API lives, as a client spells it: the upstream carries the
/// same segment (`AMS_TVDB_UPSTREAM` ends in `/v4`), so a path is handed on
/// without it.
const PREFIX: &str = "/v4";

/// A sign-in is a key and perhaps a PIN; anything longer is not one.
const LOGIN_LIMIT: usize = 16 * 1024;

/// The most an answer may weigh: a page of five hundred episodes, a series'
/// hundreds of artworks, run to a few megabytes.
const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;

/// The largest document the relay keeps.
const RELAY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// What a relayed request keeps of the caller's headers: what describes it.
const ASKED_WITH: &[&str] = &["user-agent", "accept", "accept-language", "content-type"];

/// The caller's validators, handed on only for a document this server does
/// not patch: TheTVDB would answer "unchanged" for a body it never saw the
/// final shape of, and a client that cached the document before a lock
/// would keep the unlocked one for as long as TheTVDB's record stood still.
const VALIDATORS: &[&str] = &["if-none-match", "if-modified-since"];

pub fn router() -> OpenApiRouter<AppState> {
    // The sign-in is a route of its own, so it is documented as what it is;
    // everything else is the wildcard, which `routes!` cannot collect and
    // `ApiDoc` names separately, as it does the TMDB relay's.
    OpenApiRouter::new()
        .routes(routes!(login))
        .route("/v4/{*path}", any(relay))
}

/// What a TheTVDB client signs in with.
#[derive(Deserialize, ToSchema)]
// The interface's own sign-in is a `LoginRequest` too; utoipa files schemas
// by name, and the second would overwrite the first.
#[schema(as = TvdbLoginRequest)]
pub struct LoginRequest {
    /// A key issued here, or the client's own TheTVDB key.
    pub apikey: String,
    /// A subscriber's PIN, with the client's own key.
    #[serde(default)]
    pub pin: Option<String>,
}

/// Whose credential TheTVDB is asked with.
enum Credential {
    /// The operator's token, in a client's place.
    Ours,
    /// The client's own TheTVDB token, as it came.
    Theirs(HeaderValue),
    /// None: TheTVDB says so itself.
    None,
}

/// What TheTVDB answered.
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

fn loop_refused() -> AppError {
    // A warning, not an error: anybody past the guard can send the header.
    tracing::warn!(
        "{} was asked by an instance of this server: it resolves back here; set \
         AMS_TVDB_UPSTREAM to TheTVDB's real address",
        HOSTS[0]
    );
    AppError::LoopDetected
}

/// Whether a path is a user's own — `user`, `user/favorites`. Asked with
/// the operator's token it would be the operator's account, served to
/// whoever holds a key, as the TMDB relay refuses to serve an account's
/// paths; a client's own token reads that client's own. Judged on the
/// decoded segment: `%75ser` is `user` to TheTVDB.
fn users_own(path: &str) -> bool {
    path.strip_prefix(PREFIX)
        .and_then(|rest| rest.trim_start_matches('/').split('/').next())
        .is_some_and(|segment| {
            urlencoding::decode(segment).is_ok_and(|segment| segment.eq_ignore_ascii_case("user"))
        })
}

/// Sign a TheTVDB client in.
///
/// A key issued here is taken as the client's token: it is answered as it
/// came, and every call after carries it. Any other key is TheTVDB's to
/// judge, where the surface lets a caller through by address; under a key
/// policy nothing else is accepted.
#[utoipa::path(
    post, path = "/v4/login", tag = TAG,
    request_body = LoginRequest,
    responses(
        (status = 200, description = "A token to carry on every call after: a key issued here is answered as the token itself; another key is signed in with at TheTVDB, and its answer handed back"),
        (status = 400, description = "Not a sign-in"),
        (status = 401, description = "The key is not one issued here, and the surface asks for one"),
        (status = 403, description = "The key's holder may not use the relays"),
        (status = 503, description = "The relay is off, or this server has no TheTVDB key to stand in with"),
    ),
    security(),
)]
async fn login(
    State(state): State<AppState>,
    address: Option<Extension<guards::ClientAddr>>,
    request: Request,
) -> AppResult<Response> {
    let outcome = sign_in(&state, address.map(|a| a.0.0), request).await;
    // Counted here rather than by the guard, which let the sign-in through
    // unjudged under a key policy: a key refused here is a call refused.
    state.calls.note(Api::Tvdb, outcome.is_ok());
    outcome
}

async fn sign_in(
    state: &AppState,
    address: Option<std::net::IpAddr>,
    request: Request,
) -> AppResult<Response> {
    if relay::looped(request.headers()) {
        return Err(loop_refused());
    }
    if !state.config.tvdb.passthrough {
        return Err(AppError::ProviderNotConfigured);
    }

    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, LOGIN_LIMIT)
        .await
        .map_err(|_| AppError::PayloadTooLarge("a sign-in is a key and a PIN".into()))?;
    let asked: LoginRequest = serde_json::from_slice(&body)
        .map_err(|e| AppError::BadRequest(format!("not a TheTVDB sign-in: {e}")))?;
    let key = asked.apikey.trim();
    if key.is_empty() {
        return Err(AppError::BadRequest("the sign-in names no key".into()));
    }

    if secrets::is_issued_here(key) {
        // Judged as the guard judges a key presented anywhere else: unknown,
        // disabled, run out, or its owner's account closed.
        let holder = guards::resolve_key(state, key, address).await?;
        if holder.is_member() && !state.relay_for_members() {
            return Err(guards::members_kept_out());
        }
        if !state.tvdb.is_configured() {
            return Err(AppError::ProviderNotConfigured);
        }
        // A PIN goes with a subscriber's own key; with a key issued here, the
        // operator's PIN is the one TheTVDB is signed in with.
        tracing::debug!(
            client = %holder.label(),
            pin_sent = asked.pin.is_some(),
            "a TheTVDB client signed in with a key issued here"
        );
        return Ok(Json(json!({ "status": "success", "data": { "token": key } })).into_response());
    }

    // Somebody else's key: TheTVDB's own to judge — where the policy lets the
    // caller through by address. Under a key policy, nothing but a key issued
    // here is one.
    if state.config.policy_for(Surface::Tvdb) == SurfacePolicy::ApiKey {
        return Err(AppError::Unauthorized);
    }
    let answer = send(state, &parts, body, &Credential::None).await?;
    Ok((answer.status, answer.headers, Body::from(answer.body)).into_response())
}

/// Relay any TheTVDB v4 read, with this server's locks written in on the way
/// back.
///
/// A bearer token that is a key issued here is this server's business: the
/// operator's own TheTVDB token takes its place, and the client's copy never
/// leaves. Any other token is the client's own, and travels as it came. A
/// caller with no token who authenticated with a key — in the header or the
/// query — is asked for with the operator's token too. The operator's own
/// account, `user` and what hangs off it, is nobody else's: refused with
/// the operator's token, read with a client's own.
///
/// Every v4 path is accepted; see TheTVDB's own documentation for their
/// shapes. The documents about a work of this catalogue are the ones patched:
/// `series/{id}`, `movies/{id}`, both `/extended` and `/translations/{lang}`,
/// a series' episodes in the aired order, and one episode.
#[utoipa::path(
    get, path = "/v4/{path}", tag = TAG,
    // Registered against axum as `/v4/{*path}`; see `router` above.
    params(("path" = String, Path, description = "A TheTVDB v4 path, e.g. `series/81189/extended` or `search?query=…`")),
    responses(
        (status = 200, description = "TheTVDB's answer, with the fields locked here written in where it is about a work of this catalogue"),
        (status = 401, description = "No credential this server or TheTVDB takes"),
        (status = 403, description = "Not a read, or the operator's own account asked for with a key issued here"),
        (status = 503, description = "The relay is off, or this server has no TheTVDB key to stand in with"),
    ),
    security(("apiKey" = []), ("bearer" = [])),
)]
pub async fn relay(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    address: Option<Extension<guards::ClientAddr>>,
    request: Request,
) -> AppResult<Response> {
    if relay::looped(request.headers()) {
        return Err(loop_refused());
    }
    if !state.config.tvdb.passthrough {
        return Err(AppError::ProviderNotConfigured);
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Err(AppError::Forbidden);
    }

    let (parts, _) = request.into_parts();
    let credential = match relay::bearer(&parts.headers) {
        Some(token) if secrets::is_issued_here(&token) => {
            // Under a key policy the guard judged this key already. Let
            // through by address, or with the guard off, nobody has: judged
            // here as the sign-in judges the key in its body, or a key
            // revoked, run out or a member's would stand in for the
            // operator all the same.
            if !matches!(identity, Identity::Client(_) | Identity::User(_)) {
                let holder = guards::resolve_key(&state, &token, address.map(|a| a.0.0)).await?;
                if holder.is_member() && !state.relay_for_members() {
                    return Err(guards::members_kept_out());
                }
            }
            Credential::Ours
        }
        Some(_) => match parts.headers.get(header::AUTHORIZATION) {
            Some(value) => Credential::Theirs(value.clone()),
            None => Credential::None,
        },
        None if matches!(identity, Identity::Client(_) | Identity::User(_)) => Credential::Ours,
        None => Credential::None,
    };
    let path = parts.uri.path().to_string();
    if matches!(credential, Credential::Ours) {
        if !state.tvdb.is_configured() {
            return Err(AppError::ProviderNotConfigured);
        }
        if users_own(&path) {
            return Err(AppError::Forbidden);
        }
    }

    let target = patch_target(&path);

    // What TheTVDB answered last time, while it is still good: a document
    // asked with the operator's token is the same for every caller, and is
    // kept under the path and query asked; one asked with a client's own is
    // not.
    let cache_key = (parts.method == Method::GET && matches!(credential, Credential::Ours))
        .then(|| relay_key(&state, &parts.uri));
    let remembered = match &cache_key {
        Some(key) => state
            .caches
            .relay
            .get(key)
            .await
            .filter(cache::Relayed::is_fresh),
        None => None,
    };

    let answer = match remembered {
        Some(hit) => {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&hit.content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            Answer {
                status: StatusCode::from_u16(hit.status).unwrap_or(StatusCode::OK),
                headers,
                body: hit.body,
            }
        }
        None => {
            let answer = send(&state, &parts, Bytes::new(), &credential).await?;
            if let Some(key) = cache_key
                && (answer.status.is_success() || answer.status == StatusCode::NOT_FOUND)
                && answer.body.len() <= RELAY_MAX_BYTES
            {
                let ttl = if answer.status == StatusCode::NOT_FOUND {
                    std::time::Duration::from_secs(5 * 60)
                } else {
                    cache::relay_ttl(&path)
                };
                let content_type = answer
                    .headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("application/json")
                    .to_string();
                state
                    .caches
                    .relay
                    .insert_for(
                        key,
                        cache::Relayed {
                            status: answer.status.as_u16(),
                            content_type,
                            expires_at: cache::now_secs() + ttl.as_secs(),
                            body: answer.body.clone(),
                        },
                        ttl,
                    )
                    .await;
            }
            answer
        }
    };

    if let Some(target) = target
        && relay::is_json(&answer.headers)
        && let Ok(mut document) = serde_json::from_slice::<Value>(&answer.body)
        && patch(&state, &mut document, target).await?
    {
        let mut headers = answer.headers;
        relay::without_validators(&mut headers);
        return Ok((answer.status, headers, Json(document)).into_response());
    }

    Ok((answer.status, answer.headers, Body::from(answer.body)).into_response())
}

/// What a relayed document is filed under: the path and the query asked,
/// the parameters this server takes left out, in a fixed order — and the
/// generation, so a settings change files what follows elsewhere.
fn relay_key(state: &AppState, uri: &Uri) -> String {
    format!(
        "tvdb:{}:{}:{}?{}",
        state.caches.generation(),
        state.config.tvdb.upstream,
        uri.path(),
        relay::sorted_query(uri, relay::OUR_PARAMS)
    )
}

/// Where TheTVDB is asked: its address, the path without the segment the
/// upstream already carries, the query less a key for this server.
fn target_of(state: &AppState, uri: &Uri) -> AppResult<String> {
    let path = uri.path();
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return Err(AppError::NotFound);
    };
    // `..` in any spelling: the path is interpolated after the upstream's,
    // and a URL parser resolves dot segments before the request goes out.
    if relay::climbs(path) {
        return Err(AppError::BadRequest("that is not a TheTVDB path".into()));
    }
    let base = state.config.tvdb.upstream.trim_end_matches('/');
    let target = match relay::query_without(uri, relay::OUR_PARAMS) {
        Some(query) => format!("{base}{rest}?{query}"),
        None => format!("{base}{rest}"),
    };
    // The parser that builds the outgoing request has the last word on where
    // it goes, so the path it reads must be the path checked.
    if !url::Url::parse(&target).is_ok_and(|url| url.path().ends_with(rest)) {
        return Err(AppError::BadRequest("that is not a TheTVDB path".into()));
    }
    Ok(target)
}

/// Ask TheTVDB, and read what it answered. With the operator's token, once
/// more after signing in anew when TheTVDB refused it: a token outlives its
/// welcome after a month.
async fn send(
    state: &AppState,
    parts: &Parts,
    body: Bytes,
    credential: &Credential,
) -> AppResult<Answer> {
    let target = target_of(state, &parts.uri)?;
    let mut asked = relay::asked_with(&parts.headers, ASKED_WITH);
    if patch_target(parts.uri.path()).is_none() {
        asked.extend(relay::asked_with(&parts.headers, VALIDATORS));
    }
    let mut asked_again = false;
    loop {
        let mut outbound = relay::CLIENT
            .request(parts.method.clone(), &target)
            .headers(asked.clone())
            // This name is one this server answers on: a resolver sending it
            // back here would otherwise have it call itself.
            .header(crate::providers::radarr::LOOP_HEADER, &state.instance)
            .timeout(std::time::Duration::from_secs(60));
        let mut ours = None;
        match credential {
            Credential::Ours => {
                let token = state
                    .tvdb
                    .bearer()
                    .await
                    .map_err(AppError::UpstreamUnavailable)?;
                outbound = outbound.bearer_auth(&token);
                ours = Some(token);
            }
            Credential::Theirs(value) => {
                outbound = outbound.header(header::AUTHORIZATION, value.clone());
            }
            Credential::None => {}
        }
        if !body.is_empty() {
            outbound = outbound.body(body.clone());
        }

        let started = Instant::now();
        let response = outbound.send().await;
        crate::metrics::upstream("tvdb", started, response.as_ref().ok().map(|r| r.status()));
        let response = response.map_err(|e| {
            AppError::UpstreamUnavailable(anyhow::anyhow!(
                "TheTVDB request failed: {}",
                crate::providers::describe_request_error(&e)
            ))
        })?;

        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        // The operator's token refused: asked once more with a fresh one,
        // when the one refused has had the time to run out — a refusal
        // within a minute of signing in is for another reason, and a
        // caller repeating it must not turn every call into a sign-in.
        if status == StatusCode::UNAUTHORIZED
            && !asked_again
            && let Some(token) = &ours
            && state.tvdb.refused(token).await
        {
            tracing::debug!("TheTVDB refused the operator's token; asking again with a fresh one");
            asked_again = true;
            continue;
        }
        if status == StatusCode::LOOP_DETECTED {
            tracing::warn!(
                "api4.thetvdb.com resolves to this server; set AMS_TVDB_UPSTREAM to TheTVDB's \
                 real address"
            );
        }
        let headers = relay::kept(response.headers());
        // A HEAD's answer has no body, whatever its length says of the GET's.
        let body = if parts.method == Method::HEAD {
            Vec::new()
        } else {
            crate::providers::read_body(response, ANSWER_LIMIT)
                .await
                .map_err(|e| AppError::UpstreamUnavailable(e.context("TheTVDB's answer")))?
        };
        return Ok(Answer {
            status,
            headers,
            body: Bytes::from(body),
        });
    }
}

/// A document about a work this catalogue may hold locks on.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `series/{id}`, `series/{id}/extended`: TheTVDB's record of a series.
    Series(i64),
    /// `series/{id}/translations/{lang}`: its name and overview in one
    /// language.
    SeriesText(i64),
    /// `movies/{id}`, `movies/{id}/extended`.
    Movie(i64),
    /// `movies/{id}/translations/{lang}`.
    MovieText(i64),
    /// `series/{id}/episodes/official[/{lang}]`: the episodes in the aired
    /// order, the one this catalogue numbers by. The other orders — the
    /// DVDs', straight through, a series' own default when it is one of
    /// those — number the same episodes otherwise, and are left alone.
    Episodes(i64),
    /// `episodes/{id}`, `episodes/{id}/extended`.
    Episode(i64),
    /// `episodes/{id}/translations/{lang}`: its title and overview in one
    /// language.
    EpisodeText(i64),
}

/// Recognise the paths whose documents are patched.
fn patch_target(path: &str) -> Option<Target> {
    let rest = path.strip_prefix(PREFIX)?;
    let segments: Vec<&str> = rest.trim_matches('/').split('/').collect();
    let id = |text: &str| text.parse::<i64>().ok().filter(|id| *id > 0);
    match segments.as_slice() {
        ["series", series] | ["series", series, "extended"] => Some(Target::Series(id(series)?)),
        ["series", series, "translations", _] => Some(Target::SeriesText(id(series)?)),
        ["series", series, "episodes", "official"]
        | ["series", series, "episodes", "official", _] => Some(Target::Episodes(id(series)?)),
        ["movies", movie] | ["movies", movie, "extended"] => Some(Target::Movie(id(movie)?)),
        ["movies", movie, "translations", _] => Some(Target::MovieText(id(movie)?)),
        ["episodes", episode] | ["episodes", episode, "extended"] => {
            Some(Target::Episode(id(episode)?))
        }
        ["episodes", episode, "translations", _] => Some(Target::EpisodeText(id(episode)?)),
        _ => None,
    }
}

/// Write this server's locks into TheTVDB's document. Whether anything
/// changed.
async fn patch(state: &AppState, document: &mut Value, target: Target) -> AppResult<bool> {
    let Some(data) = document.get_mut("data") else {
        return Ok(false);
    };
    Ok(match target {
        Target::Series(id) | Target::SeriesText(id) => {
            let text_only = matches!(target, Target::SeriesText(_));
            match relay::locked_work(state, ExternalSource::TvdbSeries, &id.to_string()).await? {
                Some(item) => write_record(data, &item, text_only),
                None => false,
            }
        }
        Target::Movie(id) | Target::MovieText(id) => {
            let text_only = matches!(target, Target::MovieText(_));
            match relay::locked_work(state, ExternalSource::TvdbMovie, &id.to_string()).await? {
                Some(item) => write_record(data, &item, text_only),
                None => false,
            }
        }
        Target::Episodes(id) => {
            match relay::locked_work(state, ExternalSource::TvdbSeries, &id.to_string()).await? {
                Some(item) => write_episodes(data, &item),
                None => false,
            }
        }
        Target::Episode(id) | Target::EpisodeText(id) => {
            let text_only = matches!(target, Target::EpisodeText(_));
            let Some(found) = crate::db::repo::item::find_id_by_episode_tvdb(&state.db, id).await?
            else {
                return Ok(false);
            };
            let Some(mut item) = crate::service::load(state, &found).await? else {
                return Ok(false);
            };
            if item.locked_fields.is_empty() {
                return Ok(false);
            }
            state.media.for_clients(&mut item);
            match item.episodes.iter().find(|e| e.tvdb_id == Some(id)) {
                Some(episode) => write_episode(data, &item, episode, text_only),
                None => false,
            }
        }
    })
}

/// The work's own locks, in TheTVDB's names. On a translation, only the
/// text: TheTVDB's record alone carries the rest. A field the record does
/// not carry — a film's overview, which TheTVDB keeps in its translations —
/// is not added to it; the name every record has.
fn write_record(data: &mut Value, item: &MediaItem, text_only: bool) -> bool {
    let Value::Object(map) = data else {
        return false;
    };
    let mut changed = false;
    let mut set = |key: &str, value: Value, always: bool| {
        if always || map.contains_key(key) {
            map.insert(key.to_string(), value);
            changed = true;
        }
    };

    if relay::is_locked(item, "title") {
        set("name", Value::String(item.title.clone()), true);
    }
    if relay::is_locked(item, "overview")
        && let Some(overview) = &item.overview
    {
        set("overview", Value::String(overview.clone()), text_only);
    }
    if text_only {
        return changed;
    }
    if relay::is_locked(item, "year")
        && let Some(year) = item.year
    {
        // TheTVDB writes the year as text.
        set("year", Value::String(year.to_string()), false);
    }
    if relay::is_locked(item, "firstAired")
        && let Some(date) = &item.first_aired
    {
        set("firstAired", Value::String(date.clone()), false);
    }
    if relay::is_locked(item, "lastAired")
        && let Some(date) = &item.last_aired
    {
        set("lastAired", Value::String(date.clone()), false);
    }
    if relay::is_locked(item, "runtime")
        && let Some(runtime) = item.runtime
    {
        // A series' is its average, a film's its own; whichever the record
        // carries is the one rewritten.
        set("averageRuntime", Value::from(runtime), false);
        set("runtime", Value::from(runtime), false);
    }
    if relay::is_locked(item, "primaryPoster")
        && let Some(url) = relay::chosen_image(item, CoverType::Poster)
    {
        set("image", Value::String(url), false);
    }
    if relay::is_locked(item, "status")
        && let Some(status) = item.status.as_deref()
        && let Some(name) = tvdb_series_status(status)
        && let Some(Value::Object(record)) = map.get_mut("status")
    {
        record.insert("name".to_string(), Value::String(name.to_string()));
        changed = true;
    }
    changed
}

/// Canonical status back to TheTVDB's vocabulary; a film's statuses are
/// another set, and left alone.
fn tvdb_series_status(status: &str) -> Option<&'static str> {
    match status {
        "ended" => Some("Ended"),
        "continuing" => Some("Continuing"),
        "upcoming" => Some("Upcoming"),
        _ => None,
    }
}

/// The episodes' locks, into a page of TheTVDB's aired order.
fn write_episodes(data: &mut Value, item: &MediaItem) -> bool {
    let Some(Value::Array(episodes)) = data.get_mut("episodes") else {
        return false;
    };
    let mut changed = false;
    for record in episodes.iter_mut() {
        let (Some(season), Some(number)) = (
            record.get("seasonNumber").and_then(Value::as_i64),
            record.get("number").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let Some(episode) = item.episodes.iter().find(|e| {
            i64::from(e.season_number) == season && i64::from(e.episode_number) == number
        }) else {
            continue;
        };
        changed |= write_episode(record, item, episode, false);
    }
    changed
}

/// One episode's locks, into TheTVDB's record of it. On a translation, only
/// the text.
fn write_episode(record: &mut Value, item: &MediaItem, episode: &Episode, text_only: bool) -> bool {
    let Value::Object(map) = record else {
        return false;
    };
    let scope = format!(
        "episode:{}x{}/",
        episode.season_number, episode.episode_number
    );
    let locked = |field: &str| {
        item.locked_fields
            .iter()
            .any(|key| key.strip_prefix(&scope) == Some(field))
    };
    let mut changed = false;
    if locked("title") {
        map.insert("name".to_string(), Value::String(episode.title.clone()));
        changed = true;
    }
    if locked("overview")
        && let Some(overview) = &episode.overview
    {
        map.insert("overview".to_string(), Value::String(overview.clone()));
        changed = true;
    }
    if text_only {
        return changed;
    }
    if locked("airDate")
        && let Some(date) = &episode.air_date
    {
        map.insert("aired".to_string(), Value::String(date.clone()));
        changed = true;
    }
    if locked("runtime")
        && let Some(runtime) = episode.runtime
    {
        map.insert("runtime".to_string(), Value::from(runtime));
        changed = true;
    }
    if locked("image")
        && let Some(image) = &episode.image
    {
        map.insert("image".to_string(), Value::String(image.clone()));
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MediaKind;

    #[test]
    fn the_documents_about_a_work_are_recognised() {
        assert_eq!(
            patch_target("/v4/series/81189"),
            Some(Target::Series(81189))
        );
        assert_eq!(
            patch_target("/v4/series/81189/extended"),
            Some(Target::Series(81189))
        );
        assert_eq!(
            patch_target("/v4/series/81189/translations/fra"),
            Some(Target::SeriesText(81189))
        );
        assert_eq!(
            patch_target("/v4/series/81189/episodes/official/eng"),
            Some(Target::Episodes(81189))
        );
        assert_eq!(
            patch_target("/v4/series/81189/episodes/official"),
            Some(Target::Episodes(81189))
        );
        assert_eq!(
            patch_target("/v4/movies/12/extended"),
            Some(Target::Movie(12))
        );
        assert_eq!(
            patch_target("/v4/movies/12/translations/eng"),
            Some(Target::MovieText(12))
        );
        assert_eq!(
            patch_target("/v4/episodes/349232"),
            Some(Target::Episode(349232))
        );
        assert_eq!(
            patch_target("/v4/episodes/349232/translations/fra"),
            Some(Target::EpisodeText(349232))
        );
    }

    #[test]
    fn the_operators_own_account_is_told_from_the_rest() {
        assert!(users_own("/v4/user"));
        assert!(users_own("/v4/user/favorites"));
        assert!(users_own("/v4/user/"));
        assert!(users_own("/v4/User"));
        // Spelt so that a naive prefix test misses it, and TheTVDB reads it
        // as `user` all the same.
        assert!(users_own("/v4/%75ser/favorites"));
        assert!(users_own("/v4//user"));

        assert!(!users_own("/v4/users"));
        assert!(!users_own("/v4/series/81189"));
        assert!(!users_own("/v4/search?query=user"));
        assert!(!users_own("/3/user"));
    }

    #[test]
    fn the_other_documents_are_left_alone() {
        // The DVD order numbers episodes another way than this catalogue —
        // and a series' default order may be it.
        assert_eq!(patch_target("/v4/series/81189/episodes/dvd/eng"), None);
        assert_eq!(patch_target("/v4/series/81189/episodes/default"), None);
        assert_eq!(patch_target("/v4/series/81189/episodes/absolute/eng"), None);
        assert_eq!(patch_target("/v4/series/81189/artworks"), None);
        assert_eq!(patch_target("/v4/series/81189/nextAired"), None);
        assert_eq!(patch_target("/v4/series/slug/breaking-bad"), None);
        assert_eq!(patch_target("/v4/series/not-a-number"), None);
        assert_eq!(patch_target("/v4/series/0"), None);
        assert_eq!(patch_target("/v4/search"), None);
        assert_eq!(patch_target("/v4/login"), None);
        assert_eq!(patch_target("/v4/user/favorites"), None);
        assert_eq!(patch_target("/3/tv/81189"), None);
    }

    fn locked_series() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.overview = Some("Mon résumé".into());
        item.year = Some(2008);
        item.first_aired = Some("2008-01-20".into());
        item.status = Some("ended".into());
        item.runtime = Some(47);
        item.locked_fields = vec![
            "item/overview".into(),
            "item/runtime".into(),
            "item/status".into(),
            "item/title".into(),
            "item/year".into(),
        ];
        item
    }

    #[test]
    fn a_record_takes_the_locks_in_thetvdbs_names() {
        let item = locked_series();
        let mut data = json!({
            "id": 81189, "name": "Breaking Bad", "overview": "A chemistry teacher…",
            "year": "2008", "firstAired": "2008-01-20", "averageRuntime": 45,
            "status": { "id": 2, "name": "Continuing", "recordType": "series" },
            "image": "https://artworks.thetvdb.com/banners/posters/81189-10.jpg"
        });

        assert!(write_record(&mut data, &item, false));

        assert_eq!(data["name"], json!("Mon titre"));
        assert_eq!(data["overview"], json!("Mon résumé"));
        assert_eq!(data["year"], json!("2008"));
        assert_eq!(data["averageRuntime"], json!(47));
        assert_eq!(data["status"]["name"], json!("Ended"));
        assert_eq!(data["status"]["id"], json!(2));
        // Not locked: left as TheTVDB said it.
        assert_eq!(data["firstAired"], json!("2008-01-20"));
        assert!(
            data["image"]
                .as_str()
                .unwrap()
                .contains("artworks.thetvdb.com")
        );
    }

    #[test]
    fn a_translation_takes_the_text_and_nothing_else() {
        let item = locked_series();
        let mut data =
            json!({ "name": "Breaking Bad", "overview": "…", "language": "fra", "year": "2008" });
        assert!(write_record(&mut data, &item, true));
        assert_eq!(data["name"], json!("Mon titre"));
        assert_eq!(data["overview"], json!("Mon résumé"));
        assert_eq!(data["year"], json!("2008"));
    }

    #[test]
    fn a_field_the_record_does_not_carry_is_not_added_to_it() {
        // A film's record has no overview — TheTVDB keeps it in the
        // translations — and no air dates; its name it has.
        let item = locked_series();
        let mut data = json!({ "id": 12, "name": "Upstream Film", "year": "2016", "runtime": 116 });
        assert!(write_record(&mut data, &item, false));
        assert_eq!(data["name"], json!("Mon titre"));
        assert_eq!(data["runtime"], json!(47));
        assert!(data.get("overview").is_none());
        assert!(data.get("firstAired").is_none());
        assert!(data.get("averageRuntime").is_none());
        // A translation always carries both.
        let mut text = json!({ "name": "Upstream Film", "language": "eng" });
        assert!(write_record(&mut text, &item, true));
        assert_eq!(text["overview"], json!("Mon résumé"));
    }

    #[test]
    fn a_work_with_nothing_locked_leaves_the_document_as_it_came() {
        let mut item = locked_series();
        item.locked_fields.clear();
        let mut data = json!({ "name": "Breaking Bad" });
        assert!(!write_record(&mut data, &item, false));
        assert_eq!(data["name"], json!("Breaking Bad"));
    }

    #[test]
    fn the_episodes_of_the_aired_order_take_their_locks() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes.push(Episode {
            id: "ep".into(),
            season_number: 1,
            episode_number: 3,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: "Titre verrouillé".into(),
            overview: Some("Résumé".into()),
            air_date: Some("2026-01-03".into()),
            air_date_utc: None,
            runtime: None,
            finale_type: None,
            image: None,
            tvdb_id: Some(3),
            tmdb_id: None,
            rating: None,
            is_manual: false,
        });
        item.locked_fields = vec!["episode:1x3/title".into(), "episode:1x3/airDate".into()];

        let mut data = json!({ "episodes": [
            { "id": 1, "seasonNumber": 1, "number": 1, "name": "Pilot", "aired": "2026-01-01" },
            { "id": 3, "seasonNumber": 1, "number": 3, "name": "Third", "overview": "TheTVDB's", "aired": "2026-01-02" }
        ]});

        assert!(write_episodes(&mut data, &item));

        assert_eq!(data["episodes"][0]["name"], json!("Pilot"));
        assert_eq!(data["episodes"][1]["name"], json!("Titre verrouillé"));
        assert_eq!(data["episodes"][1]["aired"], json!("2026-01-03"));
        // The overview is not locked: TheTVDB's stays.
        assert_eq!(data["episodes"][1]["overview"], json!("TheTVDB's"));
    }

    #[test]
    fn statuses_go_back_to_thetvdbs_vocabulary() {
        assert_eq!(tvdb_series_status("ended"), Some("Ended"));
        assert_eq!(tvdb_series_status("continuing"), Some("Continuing"));
        assert_eq!(tvdb_series_status("upcoming"), Some("Upcoming"));
        assert_eq!(tvdb_series_status("released"), None);
    }

    #[test]
    fn a_sign_in_is_read_with_or_without_a_pin() {
        let asked: LoginRequest = serde_json::from_str(r#"{"apikey":"ams_k"}"#).unwrap();
        assert_eq!(asked.apikey, "ams_k");
        assert_eq!(asked.pin, None);
        let asked: LoginRequest =
            serde_json::from_str(r#"{"apikey":"theirs","pin":"ABCD"}"#).unwrap();
        assert_eq!(asked.pin.as_deref(), Some("ABCD"));
        assert!(serde_json::from_str::<LoginRequest>(r#"{"pin":"ABCD"}"#).is_err());
    }
}
