//! TheTVDB compatibility: `/v4/*`, in `api4.thetvdb.com`'s place.
//!
//! What the TMDB relay is for the clients that speak TMDB, this is for those
//! that speak TheTVDB's v4 API and have its name compiled in — Yamtrack,
//! Jellyfin's plugin, Kodi's scraper, Seerr's TheTVDB option. A request is
//! handed on to TheTVDB and its answer handed back, with the fields a person
//! locked here written into it on the way: a series' name, its overview, a
//! season's title, an episode's — in the records, in the translations they
//! carry, and in the translations asked one language at a time.
//!
//! TheTVDB's clients sign in first — `POST /v4/login` with their key — for a
//! token they carry on every call after. A client given a key issued here
//! signs in with it and is answered that same key as its token, so every
//! call after carries a credential this server knows; TheTVDB is then asked
//! with the operator's own token in the client's place, and the client's
//! copy never reaches it. A client that brings its own TheTVDB key is signed
//! in with it and its token handed on as it came, where the surface's policy
//! lets it through by address. An administrator may have the relay hand on
//! each client's own key alone (`tvdb.relayCredential = client`): the
//! operator's is then never used, a key issued here is refused, and the
//! address list guards the surface.
//!
//! Reads only. TheTVDB's one write is a user's favourites, and a relay that
//! forwarded it would let any key issued here edit the operator's.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use axum::{
    Extension, Json,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::any,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::relay::{self, RelayCredential},
    auth::{Identity, middleware as guards, secrets},
    cache,
    config::{Api, Surface, SurfacePolicy},
    domain::{CoverType, CreditType, Episode, ExternalSource, MediaItem, Season},
    error::{AppError, AppResult},
    service::{self, language, orders},
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

/// The order TheTVDB names the aired one by: the one this catalogue numbers
/// its seasons and episodes by.
const AIRED_ORDER: &str = "official";

/// Every season type TheTVDB numbers a series by, `default` — the series'
/// own — among them. Any other in the path is an order this server keeps
/// and TheTVDB does not: a TMDB group's, `tmdb-…`, or one made here, `ams-…`.
const TVDB_TYPES: &[&str] = &[
    "official",
    "default",
    "dvd",
    "absolute",
    "alternate",
    "regional",
    "altdvd",
    "alttwo",
];

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
/// policy nothing else is accepted. With each client's own key relayed
/// (`tvdb.relayCredential = client`), a key issued here is refused with a
/// 401 that says why, and any other is signed in with at TheTVDB.
#[utoipa::path(
    post, path = "/v4/login", tag = TAG,
    operation_id = "thetvdb_login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "A token to carry on every call after: a key issued here is answered as the token itself; another key is signed in with at TheTVDB, and its answer handed back"),
        (status = 400, description = "Not a sign-in"),
        (status = 401, description = "The key is not one issued here, and the surface asks for one; or, with each client's own key relayed, it is one issued here"),
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
    // unjudged under a key policy: a key refused here is a call refused,
    // whether as an error or as the answer that says why.
    let served = outcome
        .as_ref()
        .is_ok_and(|answer| answer.extensions().get::<relay::RefusedHere>().is_none());
    state.calls.note(Api::Tvdb, served);
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

    // Each client's own key alone: one issued here is none of TheTVDB's,
    // and every other is TheTVDB's to judge.
    if relay::credential(state, Surface::Tvdb) == RelayCredential::Client {
        if secrets::is_issued_here(key)
            || relay::carries_key_issued_here(&parts.headers, &parts.uri)
        {
            return Ok(relay::key_issued_here_refused(
                "TheTVDB",
                "tvdb.relayCredential",
            ));
        }
        let answer = send(state, &parts, body, &Credential::None).await?;
        return Ok((answer.status, answer.headers, Body::from(answer.body)).into_response());
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
    if relay::policy(state, Surface::Tvdb) == SurfacePolicy::ApiKey {
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
/// the operator's token, read with a client's own. With each client's own
/// key relayed (`tvdb.relayCredential = client`), the operator's token is
/// never used, and a key issued here is refused with a 401 that says why.
///
/// Every v4 path is accepted; see TheTVDB's own documentation for their
/// shapes. The documents about a work of this catalogue are the ones patched:
/// `series/{id}` and `movies/{id}`, both `/extended` — with the translations,
/// seasons and episodes asked with them — and `/translations/{lang}`; a page
/// of a series' episodes in any of its orders, `[/{lang}]`; `seasons/{id}`
/// and its `/extended`; one episode, `/extended` and `/translations/{lang}`.
#[utoipa::path(
    get, path = "/v4/{path}", tag = TAG,
    operation_id = "thetvdb_relay",
    // Registered against axum as `/v4/{*path}`; see `router` above.
    params(("path" = String, Path, description = "A TheTVDB v4 path, e.g. `series/81189/extended` or `search?query=…`")),
    responses(
        (status = 200, description = "TheTVDB's answer, with the fields locked here written in where it is about a work of this catalogue"),
        (status = 401, description = "No credential this server or TheTVDB takes; or, with each client's own key relayed, a key issued here"),
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
    let credential = match relay::credential(&state, Surface::Tvdb) {
        // Each client's own token alone, as it came; TheTVDB answers one
        // that brings none itself.
        RelayCredential::Client => {
            if relay::carries_key_issued_here(&parts.headers, &parts.uri) {
                return Ok(relay::key_issued_here_refused(
                    "TheTVDB",
                    "tvdb.relayCredential",
                ));
            }
            match parts.headers.get(header::AUTHORIZATION) {
                Some(value) => Credential::Theirs(value.clone()),
                None => Credential::None,
            }
        }
        RelayCredential::Server => match relay::bearer(&parts.headers) {
            Some(token) if secrets::is_issued_here(&token) => {
                // Under a key policy the guard judged this key already. Let
                // through by address, or with the guard off, nobody has:
                // judged here as the sign-in judges the key in its body, or
                // a key revoked, run out or a member's would stand in for
                // the operator all the same.
                if !matches!(identity, Identity::Client(_) | Identity::User(_)) {
                    let holder =
                        guards::resolve_key(&state, &token, address.map(|a| a.0.0)).await?;
                    if holder.is_member() && !state.relay_for_members() {
                        return Err(guards::members_kept_out());
                    }
                }
                Credential::Ours
            }
            // The client's own token travels as it came — never a header
            // that carries a key issued here besides it, in another word or
            // another value: that one stays here, as a key in its place
            // would.
            Some(_) if !relay::authorization_carries_key_issued_here(&parts.headers) => {
                match parts.headers.get(header::AUTHORIZATION) {
                    Some(value) => Credential::Theirs(value.clone()),
                    None => Credential::None,
                }
            }
            _ if matches!(identity, Identity::Client(_) | Identity::User(_)) => Credential::Ours,
            _ => Credential::None,
        },
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

    // A page of episodes in an order this server keeps and TheTVDB does
    // not, or in the aired order while an order is frozen as it: answered
    // here, TheTVDB not asked.
    if let Some(Target::Episodes {
        series,
        order,
        language,
        ..
    }) = &target
        && let Some(page) = answered_here(
            &state,
            *series,
            order,
            language.as_deref(),
            &parts.uri,
            false,
        )
        .await?
    {
        return Ok(Json(page).into_response());
    }

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
                && let Some(ttl) = relay::kept_for(answer.status, &path)
                && answer.body.len() <= RELAY_MAX_BYTES
            {
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

    // An order TheTVDB answers no episode for, where the series has seasons
    // of it — `altdvd` and `alttwo` (thetvdb/v4-api#340) — and this server
    // read it season by season: its page, from what was read.
    if let Some(Target::Episodes {
        series,
        order,
        language,
        ..
    }) = &target
        && answer.status == StatusCode::OK
        && relay::is_json(&answer.headers)
        && serde_json::from_slice::<Value>(&answer.body).is_ok_and(|document| {
            document
                .pointer("/data/episodes")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        })
        && let Some(page) = answered_here(
            &state,
            *series,
            order,
            language.as_deref(),
            &parts.uri,
            true,
        )
        .await?
    {
        return Ok(Json(page).into_response());
    }

    if let Some(target) = target
        && relay::is_json(&answer.headers)
        && let Ok(mut document) = serde_json::from_slice::<Value>(&answer.body)
    {
        let renumbered = match target {
            Target::Series(series) if answer.status == StatusCode::OK => {
                renumber_frozen(&state, series, &mut document).await?
            }
            _ => false,
        };
        if patch(&state, &mut document, target).await? || renumbered {
            let mut headers = answer.headers;
            relay::without_validators(&mut headers);
            return Ok((answer.status, headers, Json(document)).into_response());
        }
    }

    Ok((answer.status, answer.headers, Body::from(answer.body)).into_response())
}

/// A series' record that carries its episodes (`?meta=episodes`), while an
/// order is frozen as its aired numbering: those episodes as that order
/// numbers them, written as a page of its episodes in the aired order is
/// ([`answered_here`]) — the record's are in the series' default order,
/// which is the aired one. `false` where the record carries no episodes, the
/// catalogue does not hold the series, or no order of it is frozen.
async fn renumber_frozen(state: &AppState, series: i64, document: &mut Value) -> AppResult<bool> {
    if !document
        .pointer("/data/episodes")
        .is_some_and(Value::is_array)
    {
        return Ok(false);
    }
    let Some(found) = crate::db::repo::item::find_id_by_external(
        &state.db,
        ExternalSource::TvdbSeries,
        &series.to_string(),
    )
    .await?
    else {
        return Ok(false);
    };
    if crate::db::repo::order::frozen(&state.db, &found)
        .await?
        .is_none()
    {
        return Ok(false);
    }
    let Some(mut item) = service::load(state, &found).await? else {
        return Ok(false);
    };
    state.media.for_clients(&mut item);
    let all = crate::db::repo::order::list(&state.db, &item.id).await?;
    let Some(placed) = orders::as_official(&item, &all) else {
        return Ok(false);
    };
    let episodes: Vec<Value> = placed
        .iter()
        .map(|placed| episode_record(&item, series, placed))
        .collect();
    match document.pointer_mut("/data/episodes") {
        Some(slot) => {
            *slot = Value::Array(episodes);
            Ok(true)
        }
        None => Ok(false),
    }
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
                let token = operators_token(state).await?;
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

/// How long the relay stands back after TheTVDB refused to sign the
/// operator in: a minute, twice as long after each refusal in a row, ten
/// minutes at most.
const SIGN_IN_PAUSE_SECS: u64 = 60;
const SIGN_IN_PAUSE_MOST_SECS: u64 = 10 * 60;

/// The operator's sign-ins that failed in a row, and when the last did, in
/// seconds since the epoch.
static SIGN_IN_FAILURES: AtomicU64 = AtomicU64::new(0);
static SIGN_IN_FAILED_AT: AtomicU64 = AtomicU64::new(0);

/// How long `failures` refused sign-ins in a row keep the relay from
/// signing in again, in seconds.
fn sign_in_pause(failures: u64) -> u64 {
    if failures == 0 {
        return 0;
    }
    let doublings = (failures - 1).min(4);
    (SIGN_IN_PAUSE_SECS << doublings).min(SIGN_IN_PAUSE_MOST_SECS)
}

/// The operator's token for a relayed call — or, while TheTVDB has lately
/// refused to sign the operator in, a refusal at once. Every call a client
/// makes would otherwise be one more sign-in with the operator's key, one
/// after another behind the token's lock, while TheTVDB refuses it,
/// rate-limits it, or locks the key out: a wrong or rotated key, a missing
/// PIN, TheTVDB down.
async fn operators_token(state: &AppState) -> AppResult<String> {
    let failures = SIGN_IN_FAILURES.load(Ordering::Relaxed);
    let failed_at = SIGN_IN_FAILED_AT.load(Ordering::Relaxed);
    if cache::now_secs() < failed_at.saturating_add(sign_in_pause(failures)) {
        // Said once, when the sign-in failed; a 503 is "later" to a client.
        return Err(AppError::Disabled {
            code: "upstream_unavailable",
            message: "TheTVDB did not sign this server in a moment ago; try again later".into(),
        });
    }
    match state.tvdb.bearer().await {
        Ok(token) => {
            SIGN_IN_FAILURES.store(0, Ordering::Relaxed);
            Ok(token)
        }
        Err(e) => {
            SIGN_IN_FAILED_AT.store(cache::now_secs(), Ordering::Relaxed);
            let failures = SIGN_IN_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
            Err(AppError::UpstreamUnavailable(e.context(format!(
                "TheTVDB did not sign the operator in; the relay asks again in {}s",
                sign_in_pause(failures)
            ))))
        }
    }
}

/// A document about a work this catalogue may hold locks on.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `series/{id}`, `series/{id}/extended`: TheTVDB's record of a series,
    /// with what it was asked to carry besides — its translations, its
    /// seasons, its episodes, its people and pictures; `series/{id}/artworks`,
    /// the record with its pictures.
    Series(i64),
    /// `series/{id}/translations/{lang}`: its name and overview in one
    /// language.
    SeriesText(i64, String),
    /// `movies/{id}`, `movies/{id}/extended`.
    Movie(i64),
    /// `movies/{id}/translations/{lang}`.
    MovieText(i64, String),
    /// `series/{id}/episodes/{season-type}[/{lang}]`: a page of its episodes
    /// in one of its orders. Each is told by its TheTVDB id, whatever the
    /// order numbers it; in the aired order, the one this catalogue numbers
    /// by, by its numbers too. The other orders — the DVDs', straight
    /// through, a series' own default — number the same episodes otherwise.
    Episodes {
        series: i64,
        /// The season type, as the path names it.
        order: String,
        aired: bool,
        language: Option<String>,
    },
    /// `seasons/{id}`, `seasons/{id}/extended`: one season of a series,
    /// which names its series, with its episodes and, asked for, its
    /// translations.
    Season(i64),
    /// `episodes/{id}`, `episodes/{id}/extended`.
    Episode(i64),
    /// `episodes/{id}/translations/{lang}`: its title and overview in one
    /// language.
    EpisodeText(i64, String),
}

/// Recognise the paths whose documents are patched.
fn patch_target(path: &str) -> Option<Target> {
    let rest = path.strip_prefix(PREFIX)?;
    let segments: Vec<&str> = rest.trim_matches('/').split('/').collect();
    let id = |text: &str| text.parse::<i64>().ok().filter(|id| *id > 0);
    match segments.as_slice() {
        ["series", series] | ["series", series, "extended"] | ["series", series, "artworks"] => {
            Some(Target::Series(id(series)?))
        }
        ["series", series, "translations", lang] => {
            Some(Target::SeriesText(id(series)?, lang.to_string()))
        }
        ["series", series, "episodes", order] => Some(Target::Episodes {
            series: id(series)?,
            order: order.to_string(),
            aired: *order == AIRED_ORDER,
            language: None,
        }),
        ["series", series, "episodes", order, lang] => Some(Target::Episodes {
            series: id(series)?,
            order: order.to_string(),
            aired: *order == AIRED_ORDER,
            language: Some(lang.to_string()),
        }),
        ["movies", movie] | ["movies", movie, "extended"] => Some(Target::Movie(id(movie)?)),
        ["movies", movie, "translations", lang] => {
            Some(Target::MovieText(id(movie)?, lang.to_string()))
        }
        ["seasons", season] | ["seasons", season, "extended"] => Some(Target::Season(id(season)?)),
        ["episodes", episode] | ["episodes", episode, "extended"] => {
            Some(Target::Episode(id(episode)?))
        }
        ["episodes", episode, "translations", lang] => {
            Some(Target::EpisodeText(id(episode)?, lang.to_string()))
        }
        _ => None,
    }
}

/// Write this server's locks into TheTVDB's document. Whether anything
/// changed.
///
/// A record is written as the catalogue holds the work, in the language it
/// is kept in: TheTVDB's record names a work in its original language, which
/// no client asks for by name, and a lock there is final, as on Sonarr's
/// surface. A document in one language — a translation, a page of episodes
/// asked in one — is written as a relay writes one ([`relay::lay_locks`]):
/// in the catalogue's own language, what was locked in it or in every one;
/// in another, only what was locked in that language itself, TheTVDB's own
/// text standing otherwise. The translations a record carries take, each,
/// what was locked for their language by the same rule. What the sources
/// gave and a person hid — a picture, an alternative title, a credit — is
/// taken out of the lists a record carries ([`take_hidden`]).
async fn patch(state: &AppState, document: &mut Value, target: Target) -> AppResult<bool> {
    let Some(data) = document.get_mut("data") else {
        return Ok(false);
    };
    let stored = language::stored(state);
    let unlocalize = |address: &str| state.media.unlocalize(address);
    Ok(match target {
        Target::Series(id) => {
            match relay::locked_work(state, ExternalSource::TvdbSeries, &id.to_string()).await? {
                Some(item) => {
                    let mut changed = write_record(data, &item);
                    changed |= write_translations(data, &item, &stored);
                    changed |= write_seasons(data, &item);
                    // The episodes a record carries are in the series' own
                    // default order, which may not be the aired one.
                    changed |= write_episodes(data, &item, false);
                    changed |= take_hidden(data, &item, &unlocalize);
                    changed
                }
                None => false,
            }
        }
        Target::Movie(id) => {
            match relay::locked_work(state, ExternalSource::TvdbMovie, &id.to_string()).await? {
                Some(item) => {
                    let changed = write_record(data, &item);
                    let changed = write_translations(data, &item, &stored) || changed;
                    take_hidden(data, &item, &unlocalize) || changed
                }
                None => false,
            }
        }
        Target::SeriesText(id, language) => {
            match relay::locked_work(state, ExternalSource::TvdbSeries, &id.to_string()).await? {
                Some(mut item) => {
                    relay::lay_locks(state, &mut item, &language);
                    write_text(data, &item)
                }
                None => false,
            }
        }
        Target::MovieText(id, language) => {
            match relay::locked_work(state, ExternalSource::TvdbMovie, &id.to_string()).await? {
                Some(mut item) => {
                    relay::lay_locks(state, &mut item, &language);
                    write_text(data, &item)
                }
                None => false,
            }
        }
        Target::Episodes {
            series,
            aired,
            language,
            ..
        } => {
            match relay::locked_work(state, ExternalSource::TvdbSeries, &series.to_string()).await?
            {
                Some(item) => {
                    // The page names its series by its base record, which
                    // is written as a record is.
                    let mut changed = match data.get_mut("series") {
                        Some(series) => write_record(series, &item),
                        None => false,
                    };
                    let mut item = item;
                    if let Some(language) = &language {
                        relay::lay_locks(state, &mut item, language);
                    }
                    changed |= write_episodes(data, &item, aired);
                    changed
                }
                None => false,
            }
        }
        Target::Season(_) => {
            // A season names its series: the work is found by it.
            let Some(series) = data.get("seriesId").and_then(Value::as_i64) else {
                return Ok(false);
            };
            match relay::locked_work(state, ExternalSource::TvdbSeries, &series.to_string()).await?
            {
                Some(item) => {
                    let changed = write_season(data, &item, &stored);
                    // Its pictures: a season's a person may have hid.
                    take_hidden(data, &item, &unlocalize) || changed
                }
                None => false,
            }
        }
        Target::Episode(id) => match episode_work(state, id).await? {
            Some((item, at)) => {
                let episode = &item.episodes[at];
                write_episode(data, &item, episode)
                    | write_episode_translations(data, &item, episode, &stored)
            }
            None => false,
        },
        Target::EpisodeText(id, language) => match episode_work(state, id).await? {
            Some((mut item, at)) => {
                relay::lay_locks(state, &mut item, &language);
                match data {
                    Value::Object(map) => write_episode_texts(map, &item, &item.episodes[at]),
                    _ => false,
                }
            }
            None => false,
        },
    })
}

/// A page of a series' episodes in one of its orders, made here, in the
/// shape TheTVDB answers one: for an order TheTVDB does not keep — a TMDB
/// group's, one made here — when the series has it; for the aired order
/// while an order is frozen as it; and, `filling`, for one of TheTVDB's own
/// it answered with no episode, when this server read it season by season.
/// Every episode on the first page, none on the next; each in the language
/// the path names as far as the catalogue holds it, with what was locked for
/// that language. `None` where TheTVDB answers.
async fn answered_here(
    state: &AppState,
    series: i64,
    order: &str,
    language: Option<&str>,
    uri: &Uri,
    filling: bool,
) -> AppResult<Option<Value>> {
    let aired = order == AIRED_ORDER || order == "default";
    let theirs = TVDB_TYPES.contains(&order);
    if theirs && !aired && !filling {
        return Ok(None);
    }
    if filling && (aired || !theirs) {
        return Ok(None);
    }
    let Some(found) = crate::db::repo::item::find_id_by_external(
        &state.db,
        ExternalSource::TvdbSeries,
        &series.to_string(),
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(mut item) = service::load(state, &found).await? else {
        return Ok(None);
    };
    if let Some(language) = language {
        relay::lay_locks(state, &mut item, language);
    }
    state.media.for_clients(&mut item);
    let all = orders::for_clients(state, &item, (!aired).then_some(order)).await?;
    let placed = if aired {
        match orders::as_official(&item, &all) {
            Some(placed) => placed,
            None => return Ok(None),
        }
    } else {
        match orders::in_use(&item, &all, order) {
            Some(kept) => orders::served(&item, kept),
            None => return Ok(None),
        }
    };

    let first_page = uri
        .query()
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == "page")
        .is_none_or(|(_, page)| page.trim() == "0");
    let episodes: Vec<Value> = if first_page {
        placed
            .iter()
            .map(|placed| episode_record(&item, series, placed))
            .collect()
    } else {
        Vec::new()
    };
    let total = episodes.len();
    Ok(Some(json!({
        "status": "success",
        "data": {
            "series": series_record(&item, series),
            "episodes": episodes,
        },
        "links": {
            "prev": null,
            "self": format!("{PREFIX}{}", uri.path()),
            "next": null,
            "total_items": total,
            "page_size": total,
        },
    })))
}

/// The base record a page of episodes names its series by, as TheTVDB
/// writes one, made of the work.
fn series_record(item: &MediaItem, series: i64) -> Value {
    json!({
        "id": series,
        "name": item.title,
        "slug": item.slug,
        "image": relay::chosen_image(item, CoverType::Poster),
        "firstAired": item.first_aired,
        "lastAired": item.last_aired,
        "year": item.year.map(|year| year.to_string()),
        "overview": item.overview,
        "originalLanguage": item.original_language,
        "originalCountry": item.original_country,
        "averageRuntime": item.runtime,
        "lastUpdated": item.updated_at,
    })
}

/// One episode of a page, as TheTVDB writes one, where the order puts it.
/// One the order carries as a record of its own and TheTVDB gave no id is
/// told by one made of what names it, as Sonarr is
/// ([`orders::own_episode_id`]).
fn episode_record(item: &MediaItem, series: i64, placed: &orders::Placed<'_>) -> Value {
    let episode = placed.episode.as_ref();
    let id = episode
        .tvdb_id
        .unwrap_or_else(|| orders::own_episode_id(episode));
    json!({
        "id": id,
        "seriesId": series,
        "name": episode.title,
        "aired": episode.air_date,
        "runtime": episode.runtime,
        "nameTranslations": [],
        "overview": episode.overview,
        "overviewTranslations": [],
        "image": episode.image,
        "imageType": episode.image.as_ref().map(|_| 11),
        "isMovie": 0,
        "seasons": null,
        "number": placed.episode_number,
        "absoluteNumber": placed.absolute_number.unwrap_or(0),
        "seasonNumber": placed.season_number,
        "lastUpdated": item.updated_at,
        "finaleType": episode.finale_type,
        "year": episode.air_date.as_deref().and_then(|day| day.get(..4)),
    })
}

/// The work one of whose episodes TheTVDB numbers `id`, where a person
/// locked something on it, and which of its episodes that is.
async fn episode_work(state: &AppState, id: i64) -> AppResult<Option<(MediaItem, usize)>> {
    let Some(found) = crate::db::repo::item::find_id_by_episode_tvdb(&state.db, id).await? else {
        return Ok(None);
    };
    let Some(mut item) = service::load(state, &found).await? else {
        return Ok(None);
    };
    if item.locked_fields.is_empty() {
        return Ok(None);
    }
    state.media.for_clients(&mut item);
    let at = item.episodes.iter().position(|e| e.tvdb_id == Some(id));
    Ok(at.map(|at| (item, at)))
}

/// The work's own locks, in TheTVDB's names, into its record — or into the
/// base record a page of its episodes names it by. A field the record does
/// not carry — a film's overview, which TheTVDB keeps in its translations —
/// is not added to it; the name every record has.
fn write_record(data: &mut Value, item: &MediaItem) -> bool {
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
        set("overview", Value::String(overview.clone()), false);
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

/// The work's name and overview into TheTVDB's translation of it in one
/// language, as they are locked for it. A translation always carries both,
/// so both are written whether TheTVDB had them or not; nothing else is.
fn write_text(data: &mut Value, item: &MediaItem) -> bool {
    let Value::Object(map) = data else {
        return false;
    };
    let mut changed = false;
    if relay::is_locked(item, "title") {
        map.insert("name".to_string(), Value::String(item.title.clone()));
        changed = true;
    }
    if relay::is_locked(item, "overview")
        && let Some(overview) = &item.overview
    {
        map.insert("overview".to_string(), Value::String(overview.clone()));
        changed = true;
    }
    changed
}

/// The languages a document listing them one by one is given where TheTVDB
/// has no text of its own in them: the catalogue's own (`stored`), where the
/// field is locked for it, and each one it was locked in alone.
fn locked_languages(
    item: &MediaItem,
    scope: &str,
    field: &str,
    stored: &str,
) -> Vec<(String, String)> {
    let mut texts = Vec::new();
    if !stored.is_empty()
        && let Some(text) = relay::locked_text(item, scope, field, stored, stored)
    {
        texts.push((stored.to_string(), text));
    }
    for (language, text) in relay::locked_languages(item, scope, field) {
        if language != stored {
            texts.push((language, text));
        }
    }
    texts
}

/// An entry of a record's translations, as TheTVDB lists one.
fn translation_entry(language: &str, key: &str, value: &str) -> Value {
    let mut entry = Map::new();
    entry.insert(key.to_string(), Value::String(value.to_string()));
    entry.insert("language".to_string(), Value::String(language.to_string()));
    entry.insert("isAlias".to_string(), Value::Bool(false));
    entry.insert("isPrimary".to_string(), Value::Bool(false));
    Value::Object(entry)
}

/// A locked name or overview — `field`, `title` or `overview` — of the work,
/// a season (`season:1`) or an episode (`episode:1x2`) — `scope` — into the
/// translations its record carries when they were asked for
/// (`meta=translations`). Every entry takes the text locked for its
/// language as a relay writes it ([`relay::locked_text`]) — an alias,
/// another name the work goes by there, is left alone — and every language
/// of [`locked_languages`] TheTVDB has no entry in is put in, as TheTVDB
/// lists one, and named in the record's own list of languages too. A record
/// that carries no translations is not given any.
fn write_translated(
    record: &mut Map<String, Value>,
    item: &MediaItem,
    scope: &str,
    field: &str,
    stored: &str,
) -> bool {
    let (list, key) = match field {
        "title" => ("nameTranslations", "name"),
        _ => ("overviewTranslations", "overview"),
    };
    let mut changed = false;
    let mut put_in: Vec<String> = Vec::new();
    if let Some(Value::Object(translations)) = record.get_mut("translations")
        && let Some(Value::Array(entries)) = translations.get_mut(list)
    {
        let mut present: Vec<String> = Vec::new();
        for entry in entries.iter_mut() {
            let Value::Object(entry) = entry else {
                continue;
            };
            if entry.get("isAlias").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let Some(code) = entry
                .get("language")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            present.push(language::normalize(&code));
            if let Some(text) = relay::locked_text(item, scope, field, &code, stored) {
                entry.insert(key.to_string(), Value::String(text));
                changed = true;
            }
        }
        for (language, text) in locked_languages(item, scope, field, stored) {
            if present.contains(&language) {
                continue;
            }
            entries.push(translation_entry(&language, key, &text));
            put_in.push(language);
            changed = true;
        }
    }
    if let Some(Value::Array(codes)) = record.get_mut(list) {
        for language in put_in {
            if !codes
                .iter()
                .any(|code| code.as_str() == Some(language.as_str()))
            {
                codes.push(Value::String(language));
            }
        }
    }
    changed
}

/// The work's name and overview into the translations its record carries
/// ([`write_translated`]).
fn write_translations(data: &mut Value, item: &MediaItem, stored: &str) -> bool {
    let Value::Object(record) = data else {
        return false;
    };
    let title = write_translated(record, item, "item", "title", stored);
    write_translated(record, item, "item", "overview", stored) || title
}

/// The catalogue's season a TheTVDB season record is: the one holding its
/// TheTVDB id — or, in the aired order, the one this catalogue numbers by,
/// the one of its number, when that one holds no other id. A season of
/// another order is another season.
fn catalogue_season<'a>(item: &'a MediaItem, record: &Map<String, Value>) -> Option<&'a Season> {
    let id = record.get("id").and_then(Value::as_i64);
    if let Some(id) = id
        && let Some(found) = item.seasons.iter().find(|s| s.tvdb_id == Some(id))
    {
        return Some(found);
    }
    if !in_aired_order(record) {
        return None;
    }
    let number = record.get("number").and_then(Value::as_i64)?;
    item.seasons.iter().find(|s| {
        i64::from(s.season_number) == number && s.tvdb_id.is_none_or(|held| Some(held) == id)
    })
}

/// Whether a season record is one of the aired order's.
fn in_aired_order(record: &Map<String, Value>) -> bool {
    record
        .get("type")
        .and_then(|kind| kind.get("type"))
        .and_then(Value::as_str)
        == Some(AIRED_ORDER)
}

/// The titles locked here, into the seasons a series' record lists: each
/// season of the catalogue's ([`catalogue_season`]) is named as the lock
/// names it, the name put in when TheTVDB gives it none.
fn write_seasons(data: &mut Value, item: &MediaItem) -> bool {
    let Some(Value::Array(seasons)) = data.get_mut("seasons") else {
        return false;
    };
    let mut changed = false;
    for record in seasons.iter_mut() {
        let Value::Object(record) = record else {
            continue;
        };
        let Some(season) = catalogue_season(item, record) else {
            continue;
        };
        if relay::season_locked(item, season.season_number, "title")
            && let Some(title) = &season.title
        {
            record.insert("name".to_string(), Value::String(title.clone()));
            changed = true;
        }
    }
    changed
}

/// A season's locks into TheTVDB's record of it, and its episodes': its
/// title as its name, put in when TheTVDB gives it none; its title and
/// overview into the translations asked with it ([`write_translated`]); its
/// chosen poster as its image; and each of its episodes as the catalogue's
/// episode it is.
fn write_season(data: &mut Value, item: &MediaItem, stored: &str) -> bool {
    let Value::Object(record) = data else {
        return false;
    };
    let aired = in_aired_order(record);
    let mut changed = false;
    if let Some(season) = catalogue_season(item, record) {
        let number = season.season_number;
        let scope = format!("season:{number}");
        if relay::season_locked(item, number, "title")
            && let Some(title) = &season.title
        {
            record.insert("name".to_string(), Value::String(title.clone()));
            changed = true;
        }
        changed |= write_translated(record, item, &scope, "title", stored);
        changed |= write_translated(record, item, &scope, "overview", stored);
        if relay::season_locked(item, number, "primaryPoster")
            && record.contains_key("image")
            && let Some(url) = relay::chosen_season_poster(season)
        {
            record.insert("image".to_string(), Value::String(url));
            changed = true;
        }
    }
    changed |= write_episode_list(record.get_mut("episodes"), item, aired);
    changed
}

/// What a person hid of a work, taken out of TheTVDB's record of it — a
/// series', a film's, a season's, with what it was asked to carry: its
/// pictures (`artworks`, a season's `artwork`) by their address, its other
/// names (`aliases`) by their text, its people (`characters`) by their name
/// and the kind of credit TheTVDB gives them ([`tvdb_kind`]). TheTVDB's
/// people and TMDB's share no id, and a credit hidden here is named by
/// TMDB's, so a person is told by name: a namesake in the same role on the
/// same work goes with them. The people of an episode are not the work's
/// credits, which are all a person can hide, and are left as TheTVDB gives
/// them. Whether anything was taken out.
fn take_hidden(data: &mut Value, item: &MediaItem, unlocalize: &dyn Fn(&str) -> String) -> bool {
    let Value::Object(record) = data else {
        return false;
    };
    let hidden = &item.hidden;
    if hidden.is_empty() {
        return false;
    }
    let pictures: Vec<String> = hidden
        .images
        .iter()
        .map(|image| unlocalize(&image.url))
        .collect();
    let titles: Vec<String> = hidden
        .alternative_titles
        .iter()
        .map(|title| folded(&title.title))
        .collect();
    let people: Vec<(CreditType, String)> = hidden
        .credits
        .iter()
        .map(|credit| (credit.credit_type, folded(&credit.person_name)))
        .filter(|(_, name)| !name.is_empty())
        .collect();

    let mut taken = false;
    let mut take = |key: &str, hides: &dyn Fn(&Value) -> bool| {
        if let Some(Value::Array(entries)) = record.get_mut(key) {
            let before = entries.len();
            entries.retain(|entry| !hides(entry));
            taken |= entries.len() != before;
        }
    };
    let text =
        |entry: &Value, key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
    for key in ["artworks", "artwork"] {
        take(key, &|entry| {
            text(entry, "image").is_some_and(|image| pictures.iter().any(|p| *p == image.trim()))
        });
    }
    take("aliases", &|entry| {
        text(entry, "name").is_some_and(|name| titles.contains(&folded(&name)))
    });
    take("characters", &|entry| {
        let Some(kind) = tvdb_kind(entry) else {
            return false;
        };
        text(entry, "personName").is_some_and(|name| {
            let name = folded(&name);
            people.iter().any(|(hid, who)| *hid == kind && *who == name)
        })
    });
    taken
}

/// A name or a title as it is compared: its case and the blanks around it
/// aside.
fn folded(text: &str) -> String {
    text.trim().to_lowercase()
}

/// The kind of credit a person on TheTVDB's list of a work's people is, by
/// the type TheTVDB gives them — named (`peopleType`) or numbered (`type`) —
/// as the catalogue files credits: an actor, a director, a writer (a
/// series' creator is one, as TMDB's is filed), a producer. A guest star, a
/// host, the crew at large are none of them.
fn tvdb_kind(entry: &Value) -> Option<CreditType> {
    let named = entry.get("peopleType").and_then(Value::as_str);
    let numbered = entry.get("type").and_then(Value::as_i64);
    match (named, numbered) {
        (Some("Actor"), _) | (None, Some(3)) => Some(CreditType::Actor),
        (Some("Director"), _) | (None, Some(1)) => Some(CreditType::Director),
        (Some("Writer" | "Creator"), _) | (None, Some(2 | 6)) => Some(CreditType::Writer),
        (Some("Producer"), _) | (None, Some(7)) => Some(CreditType::Producer),
        _ => None,
    }
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

/// The episodes' locks, into a page of a series' episodes in one of its
/// orders, or into the episodes its record carries: each the catalogue's
/// episode it is ([`tvdb_episode`]).
fn write_episodes(data: &mut Value, item: &MediaItem, aired: bool) -> bool {
    write_episode_list(data.get_mut("episodes"), item, aired)
}

/// [`write_episodes`], on the list itself.
fn write_episode_list(episodes: Option<&mut Value>, item: &MediaItem, aired: bool) -> bool {
    let Some(Value::Array(episodes)) = episodes else {
        return false;
    };
    let mut changed = false;
    for record in episodes.iter_mut() {
        if let Some(episode) = tvdb_episode(item, record, aired) {
            changed |= write_episode(record, item, episode);
        }
    }
    changed
}

/// The catalogue's episode a TheTVDB episode record is: the one holding its
/// TheTVDB id, whatever order the record is numbered in — or, in the aired
/// order, the one this catalogue numbers by, the one of its numbers, when
/// that one holds no other id.
fn tvdb_episode<'a>(item: &'a MediaItem, record: &Value, aired: bool) -> Option<&'a Episode> {
    let id = record.get("id").and_then(Value::as_i64);
    if let Some(id) = id
        && let Some(found) = item.episodes.iter().find(|e| e.tvdb_id == Some(id))
    {
        return Some(found);
    }
    if !aired {
        return None;
    }
    let season = record.get("seasonNumber").and_then(Value::as_i64)?;
    let number = record.get("number").and_then(Value::as_i64)?;
    item.episodes.iter().find(|e| {
        i64::from(e.season_number) == season
            && i64::from(e.episode_number) == number
            && e.tvdb_id.is_none_or(|held| Some(held) == id)
    })
}

/// One episode's locks, into TheTVDB's record of it: its title and overview
/// ([`write_episode_texts`]), its date, its runtime and its still.
fn write_episode(record: &mut Value, item: &MediaItem, episode: &Episode) -> bool {
    let Value::Object(map) = record else {
        return false;
    };
    let mut changed = write_episode_texts(map, item, episode);
    let locked = |field: &str| relay::episode_locked(item, episode, field);
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

/// An episode's title and overview, where they are locked, into a record or
/// a translation of it: the work comes laid for the language the document
/// is in, when it is in one.
fn write_episode_texts(map: &mut Map<String, Value>, item: &MediaItem, episode: &Episode) -> bool {
    let mut changed = false;
    if relay::episode_locked(item, episode, "title") {
        map.insert("name".to_string(), Value::String(episode.title.clone()));
        changed = true;
    }
    if relay::episode_locked(item, episode, "overview")
        && let Some(overview) = &episode.overview
    {
        map.insert("overview".to_string(), Value::String(overview.clone()));
        changed = true;
    }
    changed
}

/// An episode's locked title and overview into the translations TheTVDB's
/// record of it carries when asked for ([`write_translated`]).
fn write_episode_translations(
    data: &mut Value,
    item: &MediaItem,
    episode: &Episode,
    stored: &str,
) -> bool {
    let Value::Object(record) = data else {
        return false;
    };
    let scope = format!(
        "episode:{}x{}",
        episode.season_number, episode.episode_number
    );
    let title = write_translated(record, item, &scope, "title", stored);
    write_translated(record, item, &scope, "overview", stored) || title
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::relay::testing::{laid_for, read_with},
        domain::MediaKind,
    };

    /// *Breaking Bad* as its sources gave it, and what a person hid of it:
    /// an actor and a writer, a picture of the work and one of a season, an
    /// alternative title.
    pub(super) fn breaking_bad() -> MediaItem {
        let credit = |kind: CreditType, id: i64, name: &str| crate::domain::Credit {
            id: crate::db::new_id(),
            credit_type: kind,
            person_name: name.into(),
            character_name: None,
            image: None,
            tmdb_person_id: Some(id),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        };
        let picture = |file: &str, season: Option<i32>| crate::domain::Image {
            id: crate::db::new_id(),
            season_number: season,
            cover_type: CoverType::Poster,
            url: format!("https://artworks.thetvdb.com/banners/{file}"),
            language: None,
            sort_order: 0,
            source: Some("tvdb".into()),
            is_manual: false,
        };
        let title = |text: &str, language: &str| crate::domain::AlternativeTitle {
            id: crate::db::new_id(),
            title: text.into(),
            title_type: None,
            language: Some(language.into()),
            is_manual: false,
        };
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.slug = "breaking-bad".into();
        item.external_ids.tvdb = Some(81189);
        item.credits = vec![
            credit(CreditType::Actor, 17419, "Bryan Cranston"),
            credit(CreditType::Actor, 84497, "Aaron Paul"),
            credit(CreditType::Writer, 66633, "Vince Gilligan"),
        ];
        let mut first = Season {
            id: "s1".into(),
            season_number: 1,
            title: None,
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: Some(30272),
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
        };
        first.images = vec![picture("season-hidden.jpg", Some(1))];
        item.seasons = vec![first];
        item.images = vec![picture("hidden.jpg", None), picture("kept.jpg", None)];
        item.alternative_titles = vec![
            title("Во все тяжкие", "rus"),
            title("Breaking Bad – Hidden", "eng"),
        ];
        item
    }

    pub(super) fn hidden_of_breaking_bad() -> Vec<(&'static str, &'static str, &'static str, Value)>
    {
        vec![
            ("credit:actor:tmdb:17419", "hidden", "", json!(true)),
            ("credit:writer:tmdb:66633", "hidden", "", json!(true)),
            (
                "image:https://artworks.thetvdb.com/banners/hidden.jpg",
                "hidden",
                "",
                json!(true),
            ),
            (
                "image:https://artworks.thetvdb.com/banners/season-hidden.jpg",
                "hidden",
                "",
                json!(true),
            ),
            (
                "alternativeTitle:eng:Breaking Bad – Hidden",
                "hidden",
                "",
                json!(true),
            ),
        ]
    }

    #[test]
    fn what_a_person_hid_is_taken_out_of_thetvdbs_record() {
        let item = read_with(breaking_bad(), &hidden_of_breaking_bad());
        let mut data = json!({
            "id": 81189,
            "name": "Breaking Bad",
            "aliases": [
                { "language": "rus", "name": "Во все тяжкие" },
                { "language": "eng", "name": "breaking bad – hidden " },
                // An alias is told by its text, whatever its language.
                { "language": "deu", "name": "Breaking Bad – Hidden" }
            ],
            "artworks": [
                { "image": "https://artworks.thetvdb.com/banners/hidden.jpg", "type": 2 },
                { "image": "https://artworks.thetvdb.com/banners/kept.jpg", "type": 2 }
            ],
            "characters": [
                { "name": "Walter White", "personName": "Bryan Cranston", "peopleType": "Actor" },
                // The same person in another role is another credit.
                { "name": "Director", "personName": "Bryan Cranston", "peopleType": "Director" },
                { "name": "Jesse Pinkman", "personName": "Aaron Paul", "peopleType": "Actor" },
                // A creator is a writer, as the catalogue files them; told by
                // its number where it has no name.
                { "personName": "Vince Gilligan", "peopleType": "Creator" },
                { "personName": " vince gilligan", "type": 2 },
                // A guest star is no credit of the work's.
                { "personName": "Bryan Cranston", "peopleType": "Guest Star" }
            ]
        });
        assert!(take_hidden(&mut data, &item, &|address| address.to_string()));

        let names = |key: &str, field: &str| -> Vec<String> {
            data[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry[field].as_str().unwrap_or_default().to_string())
                .collect()
        };
        assert_eq!(names("aliases", "language"), ["rus"]);
        assert_eq!(
            names("artworks", "image"),
            ["https://artworks.thetvdb.com/banners/kept.jpg"]
        );
        let people: Vec<(String, String)> = data["characters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["personName"].as_str().unwrap().to_string(),
                    c["peopleType"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            people,
            [
                ("Bryan Cranston".to_string(), "Director".to_string()),
                ("Aaron Paul".to_string(), "Actor".to_string()),
                ("Bryan Cranston".to_string(), "Guest Star".to_string()),
            ]
        );

        // A season's record: its pictures.
        let mut season = json!({ "id": 30272, "artwork": [
            { "image": "https://artworks.thetvdb.com/banners/season-hidden.jpg" },
            { "image": "https://artworks.thetvdb.com/banners/season-kept.jpg" }
        ]});
        assert!(take_hidden(&mut season, &item, &|address| address.to_string()));
        assert_eq!(season["artwork"].as_array().unwrap().len(), 1);

        // Nothing hidden: nothing taken.
        let mut record = json!({ "aliases": [ { "language": "eng", "name": "BB" } ] });
        assert!(!take_hidden(&mut record, &breaking_bad(), &|address| {
            address.to_string()
        }));
    }

    #[test]
    fn the_documents_about_a_work_are_recognised() {
        assert_eq!(
            patch_target("/v4/series/81189/artworks"),
            Some(Target::Series(81189))
        );
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
            Some(Target::SeriesText(81189, "fra".into()))
        );
        assert_eq!(
            patch_target("/v4/series/81189/episodes/official/eng"),
            Some(Target::Episodes {
                series: 81189,
                order: "official".into(),
                aired: true,
                language: Some("eng".into())
            })
        );
        assert_eq!(
            patch_target("/v4/series/81189/episodes/official"),
            Some(Target::Episodes {
                series: 81189,
                order: "official".into(),
                aired: true,
                language: None
            })
        );
        assert_eq!(
            patch_target("/v4/movies/12/extended"),
            Some(Target::Movie(12))
        );
        assert_eq!(
            patch_target("/v4/movies/12/translations/eng"),
            Some(Target::MovieText(12, "eng".into()))
        );
        assert_eq!(
            patch_target("/v4/seasons/30272"),
            Some(Target::Season(30272))
        );
        assert_eq!(
            patch_target("/v4/seasons/30272/extended"),
            Some(Target::Season(30272))
        );
        assert_eq!(
            patch_target("/v4/episodes/349232"),
            Some(Target::Episode(349232))
        );
        assert_eq!(
            patch_target("/v4/episodes/349232/extended"),
            Some(Target::Episode(349232))
        );
        assert_eq!(
            patch_target("/v4/episodes/349232/translations/fra"),
            Some(Target::EpisodeText(349232, "fra".into()))
        );
    }

    #[test]
    fn the_other_orders_are_recognised_as_other_orders() {
        // The DVD order numbers episodes another way than this catalogue —
        // and a series' default order may be it: their episodes are told by
        // their ids alone.
        for (path, order, language) in [
            ("/v4/series/81189/episodes/dvd/eng", "dvd", Some("eng")),
            ("/v4/series/81189/episodes/default", "default", None),
            (
                "/v4/series/81189/episodes/default/fra",
                "default",
                Some("fra"),
            ),
            (
                "/v4/series/81189/episodes/absolute/eng",
                "absolute",
                Some("eng"),
            ),
            // An order this server keeps and TheTVDB does not.
            (
                "/v4/series/81189/episodes/ams-mine/eng",
                "ams-mine",
                Some("eng"),
            ),
        ] {
            assert_eq!(
                patch_target(path),
                Some(Target::Episodes {
                    series: 81189,
                    order: order.into(),
                    aired: false,
                    language: language.map(str::to_string)
                }),
                "{path}"
            );
        }
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
        assert_eq!(patch_target("/v4/series/81189/episodes"), None);
        assert_eq!(patch_target("/v4/series/81189/nextAired"), None);
        assert_eq!(patch_target("/v4/series/81189/artworks/extra"), None);
        assert_eq!(patch_target("/v4/series/slug/breaking-bad"), None);
        assert_eq!(patch_target("/v4/series/not-a-number"), None);
        assert_eq!(patch_target("/v4/series/0"), None);
        assert_eq!(patch_target("/v4/seasons/30272/translations/fra"), None);
        assert_eq!(patch_target("/v4/seasons/types"), None);
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

        assert!(write_record(&mut data, &item));

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
        assert!(write_text(&mut data, &item));
        assert_eq!(data["name"], json!("Mon titre"));
        assert_eq!(data["overview"], json!("Mon résumé"));
        assert_eq!(data["year"], json!("2008"));
        assert_eq!(data["language"], json!("fra"));
    }

    #[test]
    fn a_translation_takes_what_was_locked_for_its_language() {
        // The title locked in every language, the overview in German alone.
        let item = read_with(
            MediaItem::empty(MediaKind::Series),
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "overview", "deu", json!("Mein Überblick")),
            ],
        );
        // In German, the overview locked in German. The title locked in
        // every language is the catalogue's own text, written in its
        // language: TheTVDB's German name stands.
        let mut german = json!({ "name": "Breaking Bad", "overview": "…", "language": "deu" });
        assert!(write_text(&mut german, &laid_for(item.clone(), "deu")));
        assert_eq!(german["name"], json!("Breaking Bad"));
        assert_eq!(german["overview"], json!("Mein Überblick"));

        // In Spanish, nothing was locked in it: TheTVDB's own, all of it.
        let mut spanish = json!({ "name": "Breaking Bad", "overview": "…", "language": "spa" });
        assert!(!write_text(&mut spanish, &laid_for(item, "spa")));
        assert_eq!(spanish["name"], json!("Breaking Bad"));
        assert_eq!(spanish["overview"], json!("…"));
    }

    #[test]
    fn a_field_the_record_does_not_carry_is_not_added_to_it() {
        // A film's record has no overview — TheTVDB keeps it in the
        // translations — and no air dates; its name it has.
        let item = locked_series();
        let mut data = json!({ "id": 12, "name": "Upstream Film", "year": "2016", "runtime": 116 });
        assert!(write_record(&mut data, &item));
        assert_eq!(data["name"], json!("Mon titre"));
        assert_eq!(data["runtime"], json!(47));
        assert!(data.get("overview").is_none());
        assert!(data.get("firstAired").is_none());
        assert!(data.get("averageRuntime").is_none());
        // A translation always carries both.
        let mut text = json!({ "name": "Upstream Film", "language": "eng" });
        assert!(write_text(&mut text, &item));
        assert_eq!(text["overview"], json!("Mon résumé"));
    }

    #[test]
    fn a_work_with_nothing_locked_leaves_the_document_as_it_came() {
        let mut item = locked_series();
        item.locked_fields.clear();
        let mut data = json!({ "name": "Breaking Bad" });
        assert!(!write_record(&mut data, &item));
        assert_eq!(data["name"], json!("Breaking Bad"));
        assert!(!write_translations(&mut data, &item, "fra"));
    }

    #[test]
    fn a_records_translations_take_what_was_locked_for_each_language() {
        let item = read_with(
            MediaItem::empty(MediaKind::Series),
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "title", "ita", json!("Il mio titolo")),
                ("item", "overview", "deu", json!("Mein Überblick")),
            ],
        );
        let mut data = json!({
            "id": 81189,
            "name": "Breaking Bad",
            "nameTranslations": ["eng", "deu"],
            "overviewTranslations": ["eng", "deu"],
            "translations": {
                "nameTranslations": [
                    { "language": "eng", "name": "Breaking Bad", "isPrimary": true },
                    { "language": "eng", "name": "BB", "isAlias": true },
                    { "language": "deu", "name": "Breaking Bad (DE)" }
                ],
                "overviewTranslations": [
                    { "language": "eng", "overview": "A chemistry teacher…", "isPrimary": true },
                    { "language": "deu", "overview": "Ein Chemielehrer…" }
                ],
                "alias": ["BB"]
            }
        });

        assert!(write_translations(&mut data, &item, "fra"));

        let names = data["translations"]["nameTranslations"].as_array().unwrap();
        // Locked in every language: the catalogue's own text, written in its
        // language alone — TheTVDB's English and German stand.
        assert_eq!(names[0]["name"], json!("Breaking Bad"));
        assert_eq!(names[0]["isPrimary"], json!(true));
        // An alias is another name the work goes by, and stays.
        assert_eq!(names[1]["name"], json!("BB"));
        assert_eq!(names[2]["name"], json!("Breaking Bad (DE)"));
        // The catalogue's own language, and the one it was locked in alone,
        // put in where TheTVDB had none.
        let added: Vec<(&str, &str)> = names[3..]
            .iter()
            .map(|n| (n["language"].as_str().unwrap(), n["name"].as_str().unwrap()))
            .collect();
        assert_eq!(added, [("fra", "Mon titre"), ("ita", "Il mio titolo")]);
        assert_eq!(
            data["nameTranslations"],
            json!(["eng", "deu", "fra", "ita"])
        );

        // The overview, locked in German alone, in German alone.
        let overviews = data["translations"]["overviewTranslations"]
            .as_array()
            .unwrap();
        assert_eq!(overviews[0]["overview"], json!("A chemistry teacher…"));
        assert_eq!(overviews[1]["overview"], json!("Mein Überblick"));
        assert_eq!(overviews.len(), 2);
        assert_eq!(data["translations"]["alias"], json!(["BB"]));
    }

    fn season(number: i32, tvdb_id: Option<i64>, title: &str) -> Season {
        Season {
            id: format!("s{number}"),
            season_number: number,
            title: Some(title.into()),
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id,
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
        }
    }

    fn episode(season: i32, number: i32, tvdb_id: Option<i64>) -> Episode {
        Episode {
            id: format!("ep-{season}x{number}"),
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: format!("Titre verrouillé {season}x{number}"),
            overview: Some("Résumé".into()),
            air_date: Some("2026-01-03".into()),
            air_date_utc: None,
            runtime: None,
            finale_type: None,
            image: None,
            tvdb_id,
            tmdb_id: None,
            rating: None,
            is_manual: false,
        }
    }

    #[test]
    fn the_episodes_of_the_aired_order_take_their_locks() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes.push(episode(1, 3, Some(3)));
        item.locked_fields = vec!["episode:1x3/title".into(), "episode:1x3/airDate".into()];

        let mut data = json!({ "episodes": [
            { "id": 1, "seasonNumber": 1, "number": 1, "name": "Pilot", "aired": "2026-01-01" },
            { "id": 3, "seasonNumber": 1, "number": 3, "name": "Third", "overview": "TheTVDB's", "aired": "2026-01-02" }
        ]});

        assert!(write_episodes(&mut data, &item, true));

        assert_eq!(data["episodes"][0]["name"], json!("Pilot"));
        assert_eq!(data["episodes"][1]["name"], json!("Titre verrouillé 1x3"));
        assert_eq!(data["episodes"][1]["aired"], json!("2026-01-03"));
        // The overview is not locked: TheTVDB's stays.
        assert_eq!(data["episodes"][1]["overview"], json!("TheTVDB's"));
    }

    #[test]
    fn another_orders_episodes_are_told_by_their_ids_alone() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes = vec![episode(1, 3, Some(3)), episode(1, 4, None)];
        item.locked_fields = vec!["episode:1x3/title".into(), "episode:1x4/title".into()];

        // The DVD order numbers TheTVDB's episode 3 as the fourth of its
        // first disc: told by its id, it is the catalogue's 1x3.
        let mut data = json!({ "episodes": [
            { "id": 3, "seasonNumber": 1, "number": 4, "name": "Third, on the DVD" },
            { "id": 9, "seasonNumber": 1, "number": 3, "name": "Another" }
        ]});
        assert!(write_episodes(&mut data, &item, false));
        assert_eq!(data["episodes"][0]["name"], json!("Titre verrouillé 1x3"));
        assert_eq!(data["episodes"][1]["name"], json!("Another"));

        // In the aired order, by numbers too — but not over an episode the
        // catalogue knows under another id.
        let mut data = json!({ "episodes": [
            { "id": 9, "seasonNumber": 1, "number": 3, "name": "Another" },
            { "id": 10, "seasonNumber": 1, "number": 4, "name": "Fourth" }
        ]});
        assert!(write_episodes(&mut data, &item, true));
        assert_eq!(data["episodes"][0]["name"], json!("Another"));
        assert_eq!(data["episodes"][1]["name"], json!("Titre verrouillé 1x4"));
    }

    #[test]
    fn a_page_in_a_language_takes_what_was_locked_for_it() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes = vec![episode(1, 1, Some(11)), episode(1, 2, Some(12))];
        let item = read_with(
            item,
            &[
                ("episode:1x1", "title", "deu", json!("Der Anfang")),
                ("episode:1x2", "title", "", json!("Le second")),
            ],
        );
        let page = || {
            json!({ "episodes": [
                { "id": 11, "seasonNumber": 1, "number": 1, "name": "Pilot" },
                { "id": 12, "seasonNumber": 1, "number": 2, "name": "Zweite" }
            ]})
        };

        let mut german = page();
        assert!(write_episodes(
            &mut german,
            &laid_for(item.clone(), "deu"),
            true
        ));
        assert_eq!(german["episodes"][0]["name"], json!("Der Anfang"));
        // Locked in every language: the catalogue's own text, written in its
        // own language — TheTVDB's German stands.
        assert_eq!(german["episodes"][1]["name"], json!("Zweite"));

        // In the catalogue's own language, the German title is not one.
        let mut french = page();
        assert!(write_episodes(&mut french, &item, true));
        assert_eq!(french["episodes"][0]["name"], json!("Pilot"));
        assert_eq!(french["episodes"][1]["name"], json!("Le second"));
    }

    #[test]
    fn the_seasons_of_a_record_take_their_locked_titles_in_the_aired_order() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.seasons = vec![
            season(1, Some(30272), "L'éveil"),
            season(2, None, "La chute"),
        ];
        item.locked_fields = vec!["season:1/title".into(), "season:2/title".into()];
        let mut data = json!({ "seasons": [
            { "id": 30272, "number": 1, "type": { "id": 1, "type": "official" } },
            { "id": 30273, "number": 2, "type": { "id": 1, "type": "official" }, "name": "Season 2" },
            { "id": 40000, "number": 2, "type": { "id": 2, "type": "dvd" } }
        ]});
        assert!(write_seasons(&mut data, &item));
        assert_eq!(data["seasons"][0]["name"], json!("L'éveil"));
        assert_eq!(data["seasons"][1]["name"], json!("La chute"));
        // The DVDs' second season is another season.
        assert!(data["seasons"][2].get("name").is_none());
    }

    #[test]
    fn a_season_record_takes_its_title_its_poster_and_its_episodes_locks() {
        let mut item = MediaItem::empty(MediaKind::Series);
        let mut first = season(1, None, "TheTVDB's");
        first.images = vec![crate::domain::Image {
            id: "chosen".into(),
            season_number: Some(1),
            cover_type: CoverType::Poster,
            url: "https://artworks.thetvdb.com/banners/seasons/chosen.jpg".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: true,
        }];
        item.seasons = vec![first];
        item.episodes = vec![episode(1, 1, Some(11))];
        let item = read_with(
            item,
            &[
                ("season:1", "title", "", json!("L'éveil")),
                ("season:1", "title", "deu", json!("Das Erwachen")),
                (
                    "season:1",
                    "primaryPoster",
                    "",
                    json!("https://artworks.thetvdb.com/banners/seasons/chosen.jpg"),
                ),
                ("episode:1x1", "title", "", json!("Titre verrouillé 1x1")),
            ],
        );

        let mut data = json!({
            "id": 30272, "seriesId": 81189, "number": 1,
            "type": { "id": 1, "type": "official", "name": "Aired Order" },
            "image": "https://artworks.thetvdb.com/banners/seasons/upstream.jpg",
            "nameTranslations": ["eng", "deu"],
            "translations": {
                "nameTranslations": [
                    { "language": "eng", "name": "Awakening" },
                    { "language": "deu", "name": "Erwachen" }
                ],
                "overviewTranslations": []
            },
            "episodes": [ { "id": 11, "seasonNumber": 1, "number": 1, "name": "Pilot" } ]
        });

        assert!(write_season(&mut data, &item, "fra"));

        assert_eq!(data["name"], json!("L'éveil"));
        assert_eq!(
            data["image"],
            json!("https://artworks.thetvdb.com/banners/seasons/chosen.jpg")
        );
        let names = data["translations"]["nameTranslations"].as_array().unwrap();
        // The title locked in every language is the French one, the
        // catalogue's: TheTVDB's English stands; German has its own.
        assert_eq!(names[0]["name"], json!("Awakening"));
        assert_eq!(names[1]["name"], json!("Das Erwachen"));
        assert_eq!(names[2]["language"], json!("fra"));
        assert_eq!(names[2]["name"], json!("L'éveil"));
        assert_eq!(data["nameTranslations"], json!(["eng", "deu", "fra"]));
        // Nothing locked: no overview is given it.
        assert_eq!(data["translations"]["overviewTranslations"], json!([]));
        assert_eq!(data["episodes"][0]["name"], json!("Titre verrouillé 1x1"));

        // The same season in another order is another season; its episodes
        // are still told by their ids.
        let mut dvd = json!({
            "id": 50000, "seriesId": 81189, "number": 1,
            "type": { "id": 2, "type": "dvd" },
            "episodes": [ { "id": 11, "seasonNumber": 1, "number": 2, "name": "Pilot" } ]
        });
        assert!(write_season(&mut dvd, &item, "fra"));
        assert!(dvd.get("name").is_none());
        assert_eq!(dvd["episodes"][0]["name"], json!("Titre verrouillé 1x1"));
    }

    #[test]
    fn an_episodes_translations_take_what_was_locked_for_each_language() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.episodes = vec![episode(1, 1, Some(11))];
        let item = read_with(
            item,
            &[
                ("episode:1x1", "title", "", json!("Le début")),
                ("episode:1x1", "title", "deu", json!("Der Anfang")),
            ],
        );
        let mut data = json!({
            "id": 11, "name": "Pilot",
            "nameTranslations": ["eng", "deu"],
            "translations": {
                "nameTranslations": [
                    { "language": "eng", "name": "Pilot" },
                    { "language": "deu", "name": "Pilotfolge" }
                ],
                "overviewTranslations": [ { "language": "deu", "overview": "TheTVDB's" } ]
            }
        });
        let episode = item.episodes[0].clone();

        assert!(write_episode(&mut data, &item, &episode));
        assert!(write_episode_translations(
            &mut data, &item, &episode, "fra"
        ));

        assert_eq!(data["name"], json!("Le début"));
        let names = data["translations"]["nameTranslations"].as_array().unwrap();
        // Locked in every language: in the catalogue's own, French, put in;
        // TheTVDB's English stands.
        assert_eq!(names[0]["name"], json!("Pilot"));
        assert_eq!(names[1]["name"], json!("Der Anfang"));
        assert_eq!(names[2]["language"], json!("fra"));
        assert_eq!(names[2]["name"], json!("Le début"));
        // The overview is not locked: TheTVDB's.
        assert_eq!(
            data["translations"]["overviewTranslations"][0]["overview"],
            json!("TheTVDB's")
        );

        // One episode's translation in one language.
        let german = laid_for(item, "deu");
        let Value::Object(mut text) = json!({ "name": "Pilotfolge", "language": "deu" }) else {
            unreachable!()
        };
        assert!(write_episode_texts(&mut text, &german, &german.episodes[0]));
        assert_eq!(text["name"], json!("Der Anfang"));
    }

    #[test]
    fn a_refused_sign_in_holds_the_relay_back_longer_each_time() {
        assert_eq!(sign_in_pause(0), 0);
        assert_eq!(sign_in_pause(1), 60);
        assert_eq!(sign_in_pause(2), 120);
        assert_eq!(sign_in_pause(3), 240);
        assert_eq!(sign_in_pause(4), 480);
        assert_eq!(sign_in_pause(5), 600);
        assert_eq!(sign_in_pause(u64::MAX), 600);
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

/// The relay as a client meets it, against a stand-in for TheTVDB that
/// keeps what it was asked.
#[cfg(test)]
mod relayed {
    use axum::{
        Extension,
        body::Body,
        extract::{Request, State},
        http::StatusCode,
    };
    use serde_json::{Value, json};

    use super::{login, relay};
    use crate::{
        api::relay::{
            self as relays,
            testing::{self, Asked, OPERATOR_TVDB_KEY, StandIn},
        },
        auth::{Identity, secrets},
        config::{Api, Surface, SurfacePolicy},
        db::repo::client::{self, NewClient},
        domain::{MediaItem, MediaKind},
        state::AppState,
    };

    /// TheTVDB as the stand-in answers it: a sign-in answered a token named
    /// after its key, a series' record and its translations with TheTVDB's
    /// own text, and nothing to a caller with no token of its own.
    fn tvdb(asked: &Asked) -> (StatusCode, Value) {
        if asked.method == "POST" && asked.path == "/v4/login" {
            let key = serde_json::from_str::<Value>(&asked.body)
                .ok()
                .and_then(|body| body["apikey"].as_str().map(str::to_string))
                .unwrap_or_default();
            return (
                StatusCode::OK,
                json!({ "status": "success", "data": { "token": format!("token-for-{key}") } }),
            );
        }
        let signed = asked
            .authorization
            .as_deref()
            .is_some_and(|value| value.starts_with("Bearer token-for-"));
        if !signed {
            return (
                StatusCode::UNAUTHORIZED,
                json!({ "status": "failure", "message": "Unauthorized", "data": null }),
            );
        }
        match asked.path.as_str() {
            "/v4/series/81189/extended" => (
                StatusCode::OK,
                json!({ "status": "success", "data": {
                    "id": 81189, "name": "Breaking Bad", "nameTranslations": ["eng"],
                    "translations": { "nameTranslations": [
                        { "language": "eng", "name": "Breaking Bad", "isPrimary": true }
                    ], "overviewTranslations": [] }
                } }),
            ),
            "/v4/series/81189/translations/deu" => (
                StatusCode::OK,
                json!({ "status": "success", "data": {
                    "name": "Breaking Bad", "overview": "TheTVDB's", "language": "deu"
                } }),
            ),
            _ => (
                StatusCode::NOT_FOUND,
                json!({ "status": "failure", "message": "NotFound" }),
            ),
        }
    }

    /// The server, with a series TheTVDB knows as 81189 locked here: its
    /// title in every language, its overview in German alone.
    async fn server(stand_in: &StandIn) -> AppState {
        let state = testing::server(stand_in).await;
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.slug = "breaking-bad".into();
        item.external_ids.tvdb = Some(81189);
        testing::store(
            &state,
            &item,
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "overview", "deu", json!("Mein Überblick")),
            ],
        )
        .await;
        state
    }

    /// A key issued here, as Yamtrack holds one in place of its TheTVDB key.
    async fn issued(state: &AppState) -> String {
        let key = secrets::generate_api_key().expect("a key");
        client::create(
            &state.db,
            NewClient {
                name: "yamtrack",
                key_prefix: &key.prefix,
                key_hash: &key.hash,
                scopes: &["read".to_string()],
                expires_at: None,
                note: None,
                owner_id: None,
            },
        )
        .await
        .expect("the key is stored");
        key.plaintext
    }

    async fn answer(response: axum::response::Response) -> (StatusCode, Value) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn sign_in(state: &AppState, key: &str) -> (StatusCode, Value) {
        let request = Request::post("/v4/login")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "apikey": key }).to_string()))
            .unwrap();
        answer(
            login(State(state.clone()), None, request)
                .await
                .expect("an answer"),
        )
        .await
    }

    async fn get(state: &AppState, path: &str, token: &str) -> (StatusCode, Value) {
        written_as(state, path, &format!("Bearer {token}")).await
    }

    /// A read with `Authorization` written as a client wrote it.
    async fn written_as(state: &AppState, path: &str, authorization: &str) -> (StatusCode, Value) {
        let request = Request::get(path)
            .header("authorization", authorization)
            .body(Body::empty())
            .unwrap();
        // Let through by address, as under either policy once judged.
        let response = relay(
            State(state.clone()),
            Extension(Identity::Network(None)),
            None,
            request,
        )
        .await
        .expect("an answer");
        answer(response).await
    }

    /// TheTVDB as the stand-in answers for the orders of a series: its aired
    /// list, on its own and in its record, and the Blu-ray order it answers
    /// empty for (thetvdb/v4-api#340).
    fn tvdb_with_orders(asked: &Asked) -> (StatusCode, Value) {
        match asked.path.as_str() {
            "/v4/series/81189/extended" => (
                StatusCode::OK,
                json!({ "status": "success", "data": {
                    "id": 81189, "name": "Breaking Bad",
                    "episodes": [
                        { "id": 11, "seasonNumber": 1, "number": 1, "name": "Pilot" },
                        { "id": 12, "seasonNumber": 1, "number": 2, "name": "Cat's in the Bag..." }
                    ]
                } }),
            ),
            "/v4/series/81189/episodes/official/eng" => (
                StatusCode::OK,
                json!({ "status": "success", "data": {
                    "series": { "id": 81189, "name": "Breaking Bad" },
                    "episodes": [
                        { "id": 11, "seasonNumber": 1, "number": 1, "name": "Pilot" },
                        { "id": 12, "seasonNumber": 1, "number": 2, "name": "Cat's in the Bag..." }
                    ]
                }, "links": { "next": null } }),
            ),
            "/v4/series/81189/episodes/altdvd/eng" => (
                StatusCode::OK,
                json!({ "status": "success", "data": {
                    "series": { "id": 81189, "name": "Breaking Bad" }, "episodes": []
                }, "links": { "next": null } }),
            ),
            _ => tvdb(asked),
        }
    }

    /// An order TheTVDB does not keep, the aired order while one is frozen
    /// as it, and one TheTVDB answers empty for are answered here, as
    /// TheTVDB writes a page; TheTVDB is asked for none of the first two.
    #[tokio::test]
    async fn the_orders_kept_here_are_answered_as_thetvdb_writes_a_page() {
        use crate::{
            db::repo::{self, child::blank_episode},
            domain::{EpisodeOrder, OrderKind, OrderRecord, PlacedEpisode},
        };

        let stand_in = StandIn::start(tvdb_with_orders).await;
        let state = testing::server(&stand_in).await;
        let key = issued(&state).await;
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.slug = "breaking-bad".into();
        item.external_ids.tvdb = Some(81189);
        for (number, id, title) in [(1, 11, "Pilot"), (2, 12, "Cat's in the Bag...")] {
            let mut episode = blank_episode(1, number);
            episode.is_manual = false;
            episode.tvdb_id = Some(id);
            episode.title = title.into();
            item.episodes.push(episode);
        }
        testing::store(&state, &item, &[]).await;
        let order = |key: &str, source: &str, kind: OrderKind, episodes| EpisodeOrder {
            key: key.into(),
            kind,
            source: source.into(),
            source_ref: None,
            label: key.into(),
            is_default: false,
            is_frozen: false,
            announced: None,
            updated_at: crate::db::now(),
            episodes,
        };
        let mut extended = PlacedEpisode::of_tvdb(9001, 1, 1);
        extended.record = Some(OrderRecord {
            title: "Pilot (Extended)".into(),
            overview: None,
            aired: Some("2008-01-20".into()),
            runtime: Some(58),
            image: None,
            finale_type: None,
        });
        repo::order::replace_source(
            &state.db,
            &item.id,
            "tvdb",
            &[order(
                "altdvd",
                "tvdb",
                OrderKind::TvdbAlt,
                vec![extended, PlacedEpisode::of_tvdb(12, 1, 2)],
            )],
        )
        .await
        .expect("TheTVDB's order");
        repo::order::create(
            &state.db,
            &item.id,
            &order(
                "ams-mine",
                "local",
                OrderKind::Custom,
                vec![
                    PlacedEpisode::of_tvdb(12, 1, 1),
                    PlacedEpisode::of_tvdb(11, 1, 2),
                ],
            ),
        )
        .await
        .expect("one made here");

        let (status, page) = get(
            &state,
            "/v4/series/81189/episodes/ams-mine/eng?page=0",
            &key,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let episodes = page["data"]["episodes"].as_array().expect("its episodes");
        assert_eq!(
            episodes
                .iter()
                .map(|e| (
                    e["id"].clone(),
                    e["seasonNumber"].clone(),
                    e["number"].clone()
                ))
                .collect::<Vec<_>>(),
            [
                (json!(12), json!(1), json!(1)),
                (json!(11), json!(1), json!(2))
            ]
        );
        assert_eq!(page["data"]["series"]["id"], json!(81189));
        assert_eq!(page["links"]["next"], Value::Null);
        assert!(
            stand_in
                .asked_at("/v4/series/81189/episodes/ams-mine/eng")
                .is_empty()
        );
        // The next page is empty: everything is on the first.
        let (_, next) = get(
            &state,
            "/v4/series/81189/episodes/ams-mine/eng?page=1",
            &key,
        )
        .await;
        assert_eq!(next["data"]["episodes"], json!([]));

        // TheTVDB's Blu-ray order, empty there: filled with what was read.
        let (status, page) = get(&state, "/v4/series/81189/episodes/altdvd/eng", &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            page["data"]["episodes"][0]["name"],
            json!("Pilot (Extended)")
        );
        assert_eq!(page["data"]["episodes"][0]["id"], json!(9001));
        assert_eq!(page["data"]["episodes"][1]["id"], json!(12));

        // The aired order is TheTVDB's own until an order is frozen as it,
        // on its pages and in the series' record with its episodes.
        let numbered = |document: &Value| {
            document["data"]["episodes"]
                .as_array()
                .expect("its episodes")
                .iter()
                .map(|e| {
                    (
                        e["id"].clone(),
                        e["seasonNumber"].clone(),
                        e["number"].clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let (_, aired) = get(&state, "/v4/series/81189/episodes/official/eng", &key).await;
        assert_eq!(aired["data"]["episodes"][0]["id"], json!(11));
        assert_eq!(
            stand_in
                .asked_at("/v4/series/81189/episodes/official/eng")
                .len(),
            1
        );
        let (status, record) = get(&state, "/v4/series/81189/extended?meta=episodes", &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            numbered(&record),
            [
                (json!(11), json!(1), json!(1)),
                (json!(12), json!(1), json!(2))
            ]
        );
        assert!(
            repo::order::freeze(&state.db, &item.id, Some("ams-mine"))
                .await
                .expect("frozen")
        );
        state.caches.touched(&item.id).await;
        let (_, frozen) = get(&state, "/v4/series/81189/episodes/official/eng", &key).await;
        assert_eq!(frozen["data"]["episodes"][0]["id"], json!(12));
        assert_eq!(
            stand_in
                .asked_at("/v4/series/81189/episodes/official/eng")
                .len(),
            1,
            "TheTVDB not asked again"
        );
        let (status, record) = get(&state, "/v4/series/81189/extended?meta=episodes", &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(record["data"]["name"], json!("Breaking Bad"));
        assert_eq!(
            numbered(&record),
            [
                (json!(12), json!(1), json!(1)),
                (json!(11), json!(1), json!(2))
            ]
        );
    }

    #[tokio::test]
    async fn the_operators_token_stands_in_and_the_record_takes_its_locks() {
        let stand_in = StandIn::start(tvdb).await;
        let state = server(&stand_in).await;
        let key = issued(&state).await;

        let (status, signed) = sign_in(&state, &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(signed["data"]["token"], json!(key));
        assert!(stand_in.asked_at("/v4/login").is_empty());

        let (status, record) =
            get(&state, "/v4/series/81189/extended?meta=translations", &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(record["data"]["name"], json!("Mon titre"));
        let names = record["data"]["translations"]["nameTranslations"]
            .as_array()
            .unwrap();
        // The title locked in every language is the catalogue's French one:
        // TheTVDB's English stands, and French is put in.
        assert_eq!(names[0]["name"], json!("Breaking Bad"));
        assert_eq!(names[1]["language"], json!("fra"));
        assert_eq!(names[1]["name"], json!("Mon titre"));
        let asked = stand_in.asked_at("/v4/series/81189/extended");
        assert_eq!(
            asked[0].authorization.as_deref(),
            Some(format!("Bearer token-for-{OPERATOR_TVDB_KEY}").as_str())
        );

        // A key issued here in capitals stands in for the operator's token
        // too; one behind a token of the client's own is not handed on with
        // it — TheTVDB is asked with no token at all, and says so itself.
        let (status, _) = written_as(
            &state,
            "/v4/series/81189/extended",
            &format!("BEARER {key}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = written_as(
            &state,
            "/v4/series/81189/translations/deu",
            &format!("Bearer token-for-their-key {key}"),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            stand_in.asked_at("/v4/series/81189/translations/deu")[0].authorization,
            None
        );

        // A translation in German takes what was locked for German, and
        // keeps TheTVDB's German name: the title locked in every language is
        // the catalogue's own, in French.
        let (_, german) = get(&state, "/v4/series/81189/translations/deu", &key).await;
        assert_eq!(german["data"]["name"], json!("Breaking Bad"));
        assert_eq!(german["data"]["overview"], json!("Mein Überblick"));
    }

    /// TheTVDB as the stand-in answers it for *Breaking Bad*'s extended
    /// record, with its people, its pictures and its other names.
    fn tvdb_with_lists(asked: &Asked) -> (StatusCode, Value) {
        if asked.method == "POST" && asked.path == "/v4/login" {
            return (
                StatusCode::OK,
                json!({ "status": "success", "data": { "token": "token-for-operator-tvdb-key" } }),
            );
        }
        match asked.path.as_str() {
            "/v4/series/81189/extended" | "/v4/series/81189/artworks" => (
                StatusCode::OK,
                json!({ "status": "success", "data": {
                    "id": 81189, "name": "Breaking Bad",
                    "aliases": [ { "language": "eng", "name": "Breaking Bad – Hidden" },
                                 { "language": "rus", "name": "Во все тяжкие" } ],
                    "artworks": [ { "image": "https://artworks.thetvdb.com/banners/hidden.jpg" },
                                  { "image": "https://artworks.thetvdb.com/banners/kept.jpg" } ],
                    "characters": [ { "personName": "Bryan Cranston", "peopleType": "Actor" },
                                    { "personName": "Aaron Paul", "peopleType": "Actor" } ]
                } }),
            ),
            _ => (
                StatusCode::NOT_FOUND,
                json!({ "status": "failure", "message": "NotFound" }),
            ),
        }
    }

    #[tokio::test]
    async fn what_a_person_hid_is_in_no_record_relayed() {
        let stand_in = StandIn::start(tvdb_with_lists).await;
        let state = testing::server(&stand_in).await;
        testing::store(
            &state,
            &super::tests::breaking_bad(),
            &super::tests::hidden_of_breaking_bad(),
        )
        .await;
        let key = issued(&state).await;
        for path in ["/v4/series/81189/extended", "/v4/series/81189/artworks"] {
            let (status, record) = get(&state, path, &key).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            let data = &record["data"];
            assert_eq!(data["aliases"].as_array().unwrap().len(), 1, "{path}");
            assert_eq!(data["aliases"][0]["language"], json!("rus"));
            assert_eq!(
                data["artworks"],
                json!([{ "image": "https://artworks.thetvdb.com/banners/kept.jpg" }])
            );
            assert_eq!(
                data["characters"],
                json!([{ "personName": "Aaron Paul", "peopleType": "Actor" }])
            );
        }
    }

    #[tokio::test]
    async fn each_clients_own_key_travels_and_one_issued_here_is_refused() {
        let stand_in = StandIn::start(tvdb).await;
        let state = server(&stand_in).await;
        let key = issued(&state).await;
        testing::choose(&state, "tvdb.relayCredential", "client").await;
        assert_eq!(
            relays::policy(&state, Surface::Tvdb),
            SurfacePolicy::Allowlist
        );

        // A key issued here: refused, saying why, counted refused, and
        // TheTVDB asked nothing.
        let (status, refusal) = sign_in(&state, &key).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(refusal["error"], json!("own_key_required"));
        assert_eq!(state.calls.read(Api::Tvdb), (0, 1));
        let (status, _) = get(&state, "/v4/series/81189/extended", &key).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // Its scheme in capitals, or behind a token of their own: refused
        // all the same.
        for (how, written) in [
            ("its scheme in capitals", format!("BEARER {key}")),
            (
                "behind a token of theirs",
                format!("Bearer their-token {key}"),
            ),
        ] {
            let (status, refusal) = written_as(&state, "/v4/series/81189/extended", &written).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{how}");
            assert_eq!(refusal["error"], json!("own_key_required"));
        }
        assert!(stand_in.asked_at("/v4/series/81189/extended").is_empty());

        // Their own key is signed in with at TheTVDB, and their token
        // travels as it came; the locks are written in all the same.
        let (status, signed) = sign_in(&state, "their-tvdb-key").await;
        assert_eq!(status, StatusCode::OK);
        let token = signed["data"]["token"].as_str().unwrap().to_string();
        assert_eq!(token, "token-for-their-tvdb-key");
        let (status, record) = get(&state, "/v4/series/81189/extended", &token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(record["data"]["name"], json!("Mon titre"));
        let asked = stand_in.asked_at("/v4/series/81189/extended");
        assert_eq!(
            asked[0].authorization.as_deref(),
            Some("Bearer token-for-their-tvdb-key")
        );
        // Nothing asked with their token is kept for another.
        get(&state, "/v4/series/81189/extended", &token).await;
        assert_eq!(stand_in.asked_at("/v4/series/81189/extended").len(), 2);
        // The operator was never signed in.
        assert_eq!(stand_in.asked_at("/v4/login").len(), 1);
    }
}
