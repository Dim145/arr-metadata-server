//! TMDB compatibility: `/3/*`.
//!
//! This grew out of an earlier TMDB relay, with one addition that is the whole
//! point of this server: responses are **patched with local edits** before
//! they are returned.
//!
//! A request is relayed upstream with this server's own TMDB credentials
//! substituted for whatever the client sent — or, where an administrator
//! chose so (`tmdb.relayCredential = client`), with the client's own, as it
//! came, and a key issued here refused. If the path addresses a title a
//! person locked fields on — the title's own document, one of its seasons or
//! episodes, or a page of search, trending or discover results it is among —
//! those fields are written into the upstream document on the way back, a
//! text as it is locked for the language the client asked: in the
//! catalogue's own language, the text locked in it or in every one; in
//! another, only one locked in that language itself, TMDB's own text standing
//! otherwise ([`relay::lay_locks`]). What the sources gave and a person hid
//! here — a credit, a picture, an alternative title — is taken out of the
//! lists TMDB answers with. The client sees TMDB's full response, with the
//! operator's corrections applied.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Map, Value, json};
use utoipa_axum::router::OpenApiRouter;

use crate::{
    api::relay::{self, RelayCredential, climbs},
    cache,
    config::Surface,
    db::repo,
    domain::{CoverType, CreditType, Episode, ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    providers::lang::iso_3166_2_to_3,
    service::{self, language},
    state::AppState,
};

/// Query parameters this server controls and the client does not.
///
/// Every spelling of a key the guard accepts has to be here too. A client may
/// authenticate with `?apikey=`, and for a while only `?api_key=` was stripped —
/// so its credential for *this* server was relayed to TMDB, and landed in
/// TMDB's access logs on every request.
const OVERRIDDEN_PARAMS: &[&str] = &["api_key", "apikey", "include_adult"];

/// The parameters this server controls when the client's own key travels:
/// its word on adult titles, and `apikey`, which is a key for this server and
/// never TMDB's. `api_key` is then the client's own TMDB key, handed on as it
/// came — one issued here was refused before anything was asked.
const OURS_BESIDE_THEIR_KEY: &[&str] = &["apikey", "include_adult"];

/// What a relayed request keeps of the caller's headers: what describes it,
/// and nothing else.
///
/// Not the caller's credential for *this* server — its `Authorization`, its
/// `X-Api-Key`, and its session cookie above all: the interface is
/// same-origin with `/3/*` and the cookie is `Path=/`, so an `<img
/// src="/3/...">` or a bookmark carries a live admin session, which TMDB's
/// logs would have. Nor a proxy's `X-Forwarded-For`, `Forwarded` or
/// `Referer`, which describe the caller's network. Nor a `Range`, an
/// `If-Range` or an `If-Match`: TMDB's edge answers a range with a fragment,
/// and a fragment kept would be every caller's answer for hours.
const ASKED_WITH: &[&str] = &["user-agent", "accept", "accept-language"];

/// The caller's validators, handed on only for an answer that is neither
/// kept nor patched: TMDB would answer "unchanged" for a body it never saw
/// the final shape of, and an answer kept has to be the whole document.
/// This server's own layer answers them on the body it serves.
const VALIDATORS: &[&str] = &["if-none-match", "if-modified-since"];

/// The caller's headers that travel and may change TMDB's answer: what a
/// kept document is told apart by, besides its path and its query.
const VARIES_BY: &[&str] = &["accept", "accept-language"];

/// The paths that mint or use a TMDB session — a request token, a guest
/// session, an account's own pages — whose answers are one caller's.
const SESSION_PATHS: &[&str] = &["authentication", "guest_session", "account"];

/// The parameters a TMDB session travels in.
const SESSION_PARAMS: &[&str] = &["session_id", "guest_session_id"];

/// The most an answer may weigh, as the other relays read theirs: TMDB's
/// largest documents — a series with its images and credits appended — run
/// to a few megabytes.
const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;

/// The largest document the relay keeps; TMDB's biggest run to a few
/// hundred kilobytes.
const RELAY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The language TMDB answers in when none is asked.
const TMDB_DEFAULT_LANGUAGE: &str = "en-US";

/// How many results of one page are looked up here, in the order the page
/// lists them. TMDB's pages hold twenty.
const MOST_RESULTS: usize = 100;

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "TMDB compatibility";

pub fn router() -> OpenApiRouter<AppState> {
    // Registered by hand rather than through `routes!`: this needs axum's
    // wildcard (`{*path}`) to match every TMDB path, and OpenAPI has no
    // wildcard syntax to express that. The documented path is attached to the
    // spec separately, in `ApiDoc`.
    OpenApiRouter::new()
        .route("/3/{*path}", any(proxy))
        .route("/4/{*path}", any(proxy_v4))
}

fn loop_refused() -> AppError {
    // A warning, not an error: anybody past the guard can send the header.
    tracing::warn!(
        "api.themoviedb.org was asked by an instance of this server: it resolves back here; set \
         AMS_TMDB_UPSTREAM to TMDB's real address"
    );
    AppError::LoopDetected
}

/// Whose key TMDB is asked with.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asking {
    /// The operator's own: a v3 key in the query, a v4 token in a header.
    Operator(String),
    /// The client's own, as it came: its v3 key in the query, its v4 token
    /// in `Authorization`.
    Client,
}

/// Relay any TMDB v3 request, with this server's locks written in on the way
/// back.
///
/// The request goes upstream with this server's own credentials substituted
/// for whatever the client sent — or, when `tmdb.relayCredential` is
/// `client`, with the client's own TMDB key or token as it came, a key issued
/// here being refused with a 401 that says why. If the path addresses a
/// title a person locked fields on, those fields are written into the
/// upstream document before it is returned, so the client gets TMDB's full
/// response with the operator's corrections in it. A text — a title, an
/// overview — is written as it is locked for the language the client asked
/// (`language=`, TMDB's `en-US` when none is): locked in that language, or in
/// every one. One locked in another language alone leaves TMDB's own text in
/// the one asked.
///
/// Every TMDB v3 path is accepted; see TMDB's own documentation for their
/// shapes. Patched: `/3/movie/{id}`, `/3/tv/{id}`, `/3/tv/{id}/season/{n}`,
/// `/3/tv/{id}/season/{n}/episode/{e}`, and the results of
/// `/3/search/{movie,tv,multi}`, `/3/trending/{movie,tv,all}/{window}` and
/// `/3/discover/{movie,tv}`.
#[utoipa::path(
    get, path = "/3/{path}", tag = TAG,
    // Registered against axum as `/3/{*path}`; see `router` above.
    params(("path" = String, Path, description = "A TMDB v3 path, e.g. `tv/1396` or `search/movie`")),
    responses(
        (status = 200, description = "TMDB's response, with the fields locked here written in where any are"),
        (status = 401, description = "No valid credential was presented; or, with each client's own key relayed, a key issued here was presented"),
        (status = 503, description = "This server has no TMDB API key configured to stand in with"),
    ),
)]
pub async fn proxy(State(state): State<AppState>, request: Request) -> AppResult<Response> {
    if relay::looped(request.headers()) {
        return Err(loop_refused());
    }
    if !state.config.tmdb.passthrough {
        return Err(AppError::ProviderNotConfigured);
    }

    // TMDB's v4 API, for the lists Radarr's import lists read from it when
    // TMDB is redirected here too. Only the public lists: an account's own
    // lists, ratings and watchlist are read with that account's token, which
    // this relay must not carry. Forwarding a caller's would relay a
    // credential; substituting the operator's would serve the operator's
    // account to every caller. Refused before any key is looked for, so the
    // answer is the same whatever is configured.
    let v4 = request.uri().path().starts_with("/4/");
    if v4 && !v4_allowed(request.uri().path()) {
        return Err(AppError::Forbidden);
    }

    // Whose key TMDB is asked with: this server's, in place of whatever the
    // client sent — or, where an administrator chose so, the client's own,
    // as it came. A key issued here is then refused before TMDB is asked: it
    // is no key of TMDB's, and nothing of this server's leaves it.
    let asking = match relay::credential(&state, Surface::Tmdb) {
        RelayCredential::Server => {
            let Some(api_key) = state.config.tmdb.api_key.clone() else {
                return Err(AppError::ProviderNotConfigured);
            };
            // A v4 path is answered only with a v4 read token; a v3 key is
            // refused there, and the relay says so rather than passing the
            // refusal on.
            if v4 && !api_key.starts_with("eyJ") {
                return Err(AppError::ProviderNotConfigured);
            }
            Asking::Operator(api_key)
        }
        RelayCredential::Client => {
            if relay::carries_key_issued_here(request.headers(), request.uri()) {
                return Ok(relay::key_issued_here_refused(
                    "TMDB",
                    "tmdb.relayCredential",
                ));
            }
            Asking::Client
        }
    };

    // A read carries nothing in its body TMDB looks at: whatever a caller
    // put there stays here, so what is kept is the answer to the path and
    // the query alone.
    let (parts, _) = request.into_parts();

    // Reading only. The clients this relay exists for — Jellyseerr, Overseerr,
    // Plex — never write, and a relay that forwards writes is one that lets any
    // key issued here rate a film, or empty a list, as the operator.
    if !matches!(parts.method, Method::GET | Method::HEAD) {
        return Err(AppError::Forbidden);
    }

    // `..` in any spelling. The path is interpolated after `/3`, and a URL
    // parser resolves dot segments before the request goes out: `/3/%2e%2e/4/x`
    // leaves TMDB's v3 API for its v4 one, carrying the operator's credentials.
    if climbs(parts.uri.path()) {
        return Err(AppError::BadRequest("that is not a TMDB path".into()));
    }

    let target = upstream_url(
        &state.config.tmdb.upstream,
        &parts.uri,
        &asking,
        state.config.tmdb.include_adult,
    );

    // The parser that builds the outgoing request has the last word on where
    // it goes — a backslash is a slash to it, and there may be spellings
    // `climbs` has not met — so the path it reads must be the path checked.
    if !url::Url::parse(&target).is_ok_and(|url| url.path().ends_with(parts.uri.path())) {
        return Err(AppError::BadRequest("that is not a TMDB path".into()));
    }

    let patchable = patch_target(parts.uri.path());

    // What TMDB answered last time, while it is still good: a read that is
    // the same for every caller — never one that mints or uses a TMDB
    // session — filed under the path, the query and the headers that may
    // change the answer, without the credential, under the generation the
    // settings are on; and patched with the locks on each serve, so an edit
    // shows at once whatever the cache holds. Only what the operator's key
    // was answered: what a client's own key was answered is that client's,
    // and a caller whose key TMDB would refuse must not be served what
    // another's was told.
    let filed_as = (matches!(asking, Asking::Operator(_)) && may_keep(&parts.method, &parts.uri))
        .then(|| relay_key(&state, &parts.uri, &parts.headers));
    let remembered = match &filed_as {
        Some(key) => state
            .caches
            .relay
            .get(key)
            .await
            .filter(cache::Relayed::is_fresh)
            .filter(|hit| hit.status == StatusCode::OK.as_u16()),
        None => None,
    };

    let (status, headers, bytes) = match remembered {
        Some(hit) => {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&hit.content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            (StatusCode::OK, headers, hit.body)
        }
        None => {
            let validators = filed_as.is_none() && patchable.is_none();
            let (status, mut headers, bytes) =
                send(&state, &parts, &target, &asking, validators).await?;
            if let Some(key) = filed_as {
                // Asked without the caller's validators, the answer's own
                // could never be answered "unchanged" by TMDB: this server's
                // layer tags the body it serves instead, kept or not, the
                // same for both.
                headers.remove(header::ETAG);
                headers.remove(header::LAST_MODIFIED);
                if let Some(ttl) = kept_for(status, parts.uri.path())
                    && bytes.len() <= RELAY_MAX_BYTES
                {
                    let content_type = headers
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
                                status: status.as_u16(),
                                content_type,
                                expires_at: cache::now_secs() + ttl.as_secs(),
                                body: bytes.clone(),
                            },
                            ttl,
                        )
                        .await;
                }
            }
            (status, headers, bytes)
        }
    };

    // Only TMDB's own documents about the works it lists are worth
    // inspecting; images, configuration, refusals and errors pass through
    // untouched.
    if status == StatusCode::OK
        && let Some(target) = patchable
        && relay::is_json(&headers)
        && let Ok(mut document) = serde_json::from_slice::<Value>(&bytes)
    {
        let asked = asked_language(&parts.uri);
        if patch(&state, &mut document, target, &asked).await? {
            // TMDB's validators described TMDB's bytes; the patched document
            // is tagged by this server's own layer, and kept as long as it
            // says.
            let mut headers = headers;
            relay::without_validators(&mut headers);
            return Ok((status, headers, Json(document)).into_response());
        }
    }

    Ok((status, headers, Body::from(bytes)).into_response())
}

/// Ask TMDB, with the operator's credentials or the client's own, and read
/// what it answered.
async fn send(
    state: &AppState,
    parts: &Parts,
    target: &str,
    asking: &Asking,
    validators: bool,
) -> AppResult<(StatusCode, HeaderMap, Bytes)> {
    let mut upstream = state
        .http
        .request(parts.method.clone(), target)
        .headers(forwarded_headers(&parts.headers, validators))
        // This name is one this server answers on: a resolver sending it
        // back here would otherwise have it call itself.
        .header(crate::providers::radarr::LOOP_HEADER, &state.instance);

    match asking {
        // A v4 token authenticates by header; the query parameter is
        // ignored then.
        Asking::Operator(key) if key.starts_with("eyJ") => {
            upstream = upstream.bearer_auth(key);
        }
        Asking::Operator(_) => {}
        // The client's own v4 token, as it came; its v3 key is in the
        // address already. One issued here was refused before.
        Asking::Client => {
            if let Some(value) = parts.headers.get(header::AUTHORIZATION) {
                upstream = upstream.header(header::AUTHORIZATION, value.clone());
            }
        }
    }

    let started = Instant::now();
    // The error would name the address it was sent to, a key in its query —
    // the operator's, or the client's own; it is logged and answered, so
    // what is kept of it is its kind and what lay under it.
    let response = upstream.send().await.map_err(|e| {
        anyhow::anyhow!(
            "TMDB request failed: {}",
            crate::providers::describe_request_error(&e)
        )
    });
    crate::metrics::upstream("tmdb", started, response.as_ref().ok().map(|r| r.status()));
    let response = response.map_err(AppError::UpstreamUnavailable)?;

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::LOOP_DETECTED {
        tracing::warn!(
            "api.themoviedb.org resolves to this server; set AMS_TMDB_UPSTREAM to TMDB's real \
             address"
        );
    }
    let headers = answer_headers(response.headers());

    // A HEAD's answer has no body, whatever its length says of the GET's.
    // Read as a stream, bounded: the error of a body read whole would name
    // the address asked, a key in its query.
    let body = if parts.method == Method::HEAD {
        Vec::new()
    } else {
        crate::providers::read_body(response, ANSWER_LIMIT)
            .await
            .map_err(|e| {
                AppError::UpstreamUnavailable(anyhow::anyhow!(
                    "TMDB's answer could not be read: {}",
                    crate::telemetry::redact(&format!("{e:#}"))
                ))
            })?
    };
    Ok((status, headers, Bytes::from(body)))
}

/// Whether TMDB's answer to a request may be kept, and served to anybody
/// else who asks the same: a read that neither mints nor uses a TMDB
/// session. A request token or a guest session made for one caller, an
/// account's pages, anything asked with a session's id, is that caller's.
fn may_keep(method: &Method, uri: &Uri) -> bool {
    *method == Method::GET && !in_a_session(uri)
}

/// Whether a request mints or uses a TMDB session: one of the paths that
/// do — judged on the decoded segment, `%61ccount` being `account` to
/// TMDB — or a session's id in its query, by any spelling of its name.
fn in_a_session(uri: &Uri) -> bool {
    let mut segments = uri.path().split('/').filter(|segment| !segment.is_empty());
    // The version, `3` or `4`; the next segment names what is asked.
    let _version = segments.next();
    let session_path = segments
        .next()
        .is_some_and(|segment| match urlencoding::decode(segment) {
            Ok(segment) => SESSION_PATHS
                .iter()
                .any(|path| segment.eq_ignore_ascii_case(path)),
            // Not a path of TMDB's, and kept for nobody.
            Err(_) => true,
        });
    let session_param = uri.query().is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes()).any(|(name, _)| {
            SESSION_PARAMS
                .iter()
                .any(|param| name.eq_ignore_ascii_case(param))
        })
    });
    session_path || session_param
}

/// How long an answer of this status is kept, when it is: a whole document,
/// for as long as its kind stays good, and nothing else. Another success —
/// a fragment, "no content" — a redirect, "unchanged", a refusal, an error,
/// and a "no such thing" too, are asked again.
fn kept_for(status: StatusCode, path: &str) -> Option<Duration> {
    (status == StatusCode::OK).then(|| cache::relay_ttl(path))
}

/// What a relayed document is filed under: what tells one answer of TMDB's
/// from another ([`filed_under`]) — and the upstream, the word on adult
/// titles this server adds, and the generation, so a settings change files
/// what follows elsewhere.
fn relay_key(state: &AppState, uri: &Uri, headers: &HeaderMap) -> String {
    format!(
        "tmdb:{}:{}:{}:{}",
        state.caches.generation(),
        state.config.tmdb.upstream,
        state.config.tmdb.include_adult,
        filed_under(uri, headers)
    )
}

/// The path, the query asked — less the parameters this server sets, in a
/// fixed order ([`relay::sorted_query`]) — and the caller's headers that
/// travel and may change TMDB's answer, each value escaped so that no value
/// can pass for another header's.
fn filed_under(uri: &Uri, headers: &HeaderMap) -> String {
    let varies = VARIES_BY
        .iter()
        .map(|name| {
            let values: Vec<String> = headers
                .get_all(*name)
                .iter()
                .map(|value| url::form_urlencoded::byte_serialize(value.as_bytes()).collect())
                .collect();
            format!("{name}={}", values.join(","))
        })
        .collect::<Vec<_>>()
        .join("&");
    format!(
        "{}?{}#{varies}",
        uri.path(),
        relay::sorted_query(uri, OVERRIDDEN_PARAMS)
    )
}

/// Where TMDB is asked: its address, the path, the query asked less the
/// parameters this server sets — told by their name as TMDB reads it,
/// decoded and in any case, so that `include%5Fadult` is one of them as it
/// is for the key the answer is kept under — and this server's own: the
/// operator's v3 key, when it is the operator's key TMDB is asked with and
/// the query is where it goes, and its word on adult titles. Asked with the
/// client's own key, the client's `api_key` stays as it came.
fn upstream_url(base: &str, uri: &Uri, asking: &Asking, include_adult: bool) -> String {
    let ours = match asking {
        Asking::Operator(_) => OVERRIDDEN_PARAMS,
        Asking::Client => OURS_BESIDE_THEIR_KEY,
    };
    let mut pairs: Vec<String> = uri
        .query()
        .map(|query| {
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                // The client's own TMDB key, where the operator's stands in —
                // or the key it used to authenticate *here* — must never
                // reach upstream.
                .filter(|pair| {
                    let name = url::form_urlencoded::parse(pair.as_bytes())
                        .next()
                        .map(|(name, _)| name.into_owned())
                        .unwrap_or_default();
                    !ours.iter().any(|ours| name.eq_ignore_ascii_case(ours))
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // A v4 token travels in a header, not in the address.
    if let Asking::Operator(key) = asking
        && !key.starts_with("eyJ")
    {
        pairs.push(format!("api_key={key}"));
    }
    pairs.push(format!("include_adult={include_adult}"));

    format!("{base}{}?{}", uri.path(), pairs.join("&"))
}

/// The v4 route, registered by hand as `/4/{*path}` like the v3 one: the same
/// relay, with the checks above that hold it to the public lists.
#[utoipa::path(
    get, path = "/4/{path}", tag = TAG,
    params(("path" = String, Path, description = "A TMDB v4 list path: `list/{id}`")),
    responses(
        (status = 200, description = "TMDB's own response"),
        (status = 401, description = "With each client's own token relayed, a key issued here was presented"),
        (status = 403, description = "Not a public list, or not a read"),
        (status = 503, description = "No TMDB v4 read token is configured to stand in with"),
    ),
)]
pub async fn proxy_v4(state: State<AppState>, request: Request) -> AppResult<Response> {
    proxy(state, request).await
}

/// The v4 paths relayed: the public lists, and nothing of an account.
fn v4_allowed(path: &str) -> bool {
    path.starts_with("/4/list/") && path.len() > "/4/list/".len()
}

/// The caller's headers as TMDB is asked with them: those that describe the
/// request ([`ASKED_WITH`]), and the caller's validators where TMDB may
/// answer them ([`VALIDATORS`]).
fn forwarded_headers(headers: &HeaderMap, validators: bool) -> HeaderMap {
    let mut asked = relay::asked_with(headers, ASKED_WITH);
    if validators {
        asked.extend(relay::asked_with(headers, VALIDATORS));
    }
    asked
}

/// The headers TMDB's answer keeps: those every relay keeps
/// ([`relay::kept`]: not the hops, a cookie, or TMDB's word on cross-origin
/// reads, which is this server's to give), less an address of TMDB's — a
/// redirect the client was not followed past, a `Content-Location` — which
/// would name the request this server made, a key in its query.
fn answer_headers(headers: &HeaderMap) -> HeaderMap {
    let mut kept = relay::kept(headers);
    kept.remove(header::LOCATION);
    kept.remove(header::CONTENT_LOCATION);
    kept
}

/// The language a client asked TMDB in: the `language` asked, the last when
/// there are several, as TMDB reads them, and TMDB's own `en-US` when none
/// is.
fn asked_language(uri: &Uri) -> String {
    let asked = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .filter(|(name, _)| name == "language")
            .last()
            .map(|(_, value)| value.into_owned())
    });
    asked
        .as_deref()
        .map(str::trim)
        .filter(|asked| !asked.is_empty())
        .unwrap_or(TMDB_DEFAULT_LANGUAGE)
        .to_string()
}

/// Whether a client asked TMDB in the language the catalogue's own text is
/// held in — the one a homepage and the genres' names were locked in. A
/// language is one whatever its region: `fr`, `fr-FR` and `fr-CA` are
/// French.
fn asks_in(asked: &str, catalogue: &str) -> bool {
    let asked = language::normalize(asked);
    !asked.is_empty() && asked == language::normalize(catalogue)
}

/// A document of TMDB's this server may hold edits for.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `/3/tv/{id}`, `/3/movie/{id}`: a title's own document.
    Title { kind: MediaKind, tmdb_id: i64 },
    /// `/3/tv/{id}/season/{n}`: one of a series' seasons, with its episodes.
    Season { tmdb_id: i64, season: i32 },
    /// `/3/tv/{id}/season/{n}/episode/{e}`: one episode, which says its own
    /// numbers and id.
    Episode { tmdb_id: i64 },
    /// `/3/search/{movie,tv,multi}`, `/3/trending/{movie,tv,all}/{window}`,
    /// `/3/discover/{movie,tv}`: a page of results, each of the kind the
    /// path names, or of the kind it says it is (`media_type`).
    Results { kind: Option<MediaKind> },
    /// What hangs off a title, listing what its sources gave it: its people
    /// (`/credits`, a series' `/aggregate_credits`), its pictures
    /// (`/images`, a season's too) or its other titles
    /// (`/alternative_titles`). What a person hid here is taken out.
    Part {
        kind: MediaKind,
        tmdb_id: i64,
        part: Part,
    },
}

/// Which of a title's lists a document of it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    People,
    Pictures,
    Titles,
}

/// Recognise the documents patched, exactly: a title's, a season's, an
/// episode's, a page of results, and the lists a title's sources gave it —
/// its people, its pictures, its other titles, which a person may have hid
/// some of. Any other sub-resource is left alone: its document carries
/// nothing a lock addresses.
fn patch_target(path: &str) -> Option<Target> {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let id = |text: &str| text.parse::<i64>().ok();
    let number = |text: &str| text.parse::<i32>().ok().filter(|n| *n >= 0);

    match segments.as_slice() {
        ["3", "tv", series] => Some(Target::Title {
            kind: MediaKind::Series,
            tmdb_id: id(series)?,
        }),
        ["3", "movie", movie] => Some(Target::Title {
            kind: MediaKind::Movie,
            tmdb_id: id(movie)?,
        }),
        ["3", "tv", series, "season", season] => Some(Target::Season {
            tmdb_id: id(series)?,
            season: number(season)?,
        }),
        ["3", "tv", series, "season", season, "episode", episode] => {
            number(season)?;
            number(episode)?;
            Some(Target::Episode {
                tmdb_id: id(series)?,
            })
        }
        ["3", "search", "movie"] | ["3", "discover", "movie"] | ["3", "trending", "movie", _] => {
            Some(Target::Results {
                kind: Some(MediaKind::Movie),
            })
        }
        ["3", "search", "tv"] | ["3", "discover", "tv"] | ["3", "trending", "tv", _] => {
            Some(Target::Results {
                kind: Some(MediaKind::Series),
            })
        }
        ["3", "search", "multi"] | ["3", "trending", "all", _] => {
            Some(Target::Results { kind: None })
        }
        ["3", kind @ ("movie" | "tv"), title, part] => {
            let kind = match *kind {
                "movie" => MediaKind::Movie,
                _ => MediaKind::Series,
            };
            let part = match (*part, kind) {
                ("credits", _) | ("aggregate_credits", MediaKind::Series) => Part::People,
                ("images", _) => Part::Pictures,
                ("alternative_titles", _) => Part::Titles,
                _ => return None,
            };
            Some(Target::Part {
                kind,
                tmdb_id: id(title)?,
                part,
            })
        }
        // A season's posters: a person may have hid one of them too.
        ["3", "tv", series, "season", season, "images"] => {
            number(season)?;
            Some(Target::Part {
                kind: MediaKind::Series,
                tmdb_id: id(series)?,
                part: Part::Pictures,
            })
        }
        _ => None,
    }
}

/// Write this server's locks into TMDB's document. Whether anything changed.
///
/// A work is written in as it is locked for the language the document is in
/// (`asked`), as a relay writes one ([`relay::lay_locks`]): in the
/// catalogue's own language, the texts locked in it or in every language; in
/// another, those locked in that language alone. What a person hid of what
/// the sources gave is taken out of the lists the document carries
/// ([`take_hidden`]).
async fn patch(
    state: &AppState,
    document: &mut Value,
    target: Target,
    asked: &str,
) -> AppResult<bool> {
    let unlocalize = |address: &str| state.media.unlocalize(address);
    let in_its_language = asks_in(asked, &state.language(None, None));
    match target {
        Target::Title { kind, tmdb_id } => {
            let source = ExternalSource::tmdb_for(kind);
            let Some(mut item) = relay::locked_work(state, source, &tmdb_id.to_string()).await?
            else {
                return Ok(false);
            };
            relay::lay_locks(state, &mut item, asked);
            let Value::Object(map) = document else {
                return Ok(false);
            };
            let written = write_locked(map, &item, kind, in_its_language, &unlocalize);
            Ok(take_hidden(map, &item, &unlocalize) | written)
        }
        Target::Season { tmdb_id, season } => {
            let source = ExternalSource::tmdb_for(MediaKind::Series);
            let Some(mut item) = relay::locked_work(state, source, &tmdb_id.to_string()).await?
            else {
                return Ok(false);
            };
            relay::lay_locks(state, &mut item, asked);
            let Value::Object(map) = document else {
                return Ok(false);
            };
            let written = write_season(map, &item, season, &unlocalize);
            // Its posters, where they were asked with it.
            let withheld = Withheld::of(&item, &unlocalize);
            let taken = match map.get_mut("images") {
                Some(Value::Object(images)) => take_pictures(images, &withheld),
                _ => false,
            };
            Ok(written | taken)
        }
        Target::Episode { tmdb_id } => {
            let source = ExternalSource::tmdb_for(MediaKind::Series);
            let Some(mut item) = relay::locked_work(state, source, &tmdb_id.to_string()).await?
            else {
                return Ok(false);
            };
            relay::lay_locks(state, &mut item, asked);
            let Value::Object(map) = document else {
                return Ok(false);
            };
            Ok(write_episode(map, &item, &unlocalize))
        }
        Target::Results { kind } => {
            patch_results(state, document, kind, asked, in_its_language).await
        }
        Target::Part {
            kind,
            tmdb_id,
            part,
        } => {
            let source = ExternalSource::tmdb_for(kind);
            let Some(item) = relay::locked_work(state, source, &tmdb_id.to_string()).await? else {
                return Ok(false);
            };
            let Value::Object(map) = document else {
                return Ok(false);
            };
            let withheld = Withheld::of(&item, &unlocalize);
            Ok(match part {
                Part::People => take_people(map, &withheld),
                Part::Pictures => take_pictures(map, &withheld),
                Part::Titles => take_titles(map, &withheld),
            })
        }
    }
}

/// The locks of every work of a page of results that this catalogue holds
/// and a person locked something on, each written in as its own document
/// takes them, in the fields a result carries: its title, its overview, its
/// date, its poster and its background. Looked up together — which of them
/// the catalogue holds, one query a kind; which of those carry a lock, one
/// more — and only those read whole.
async fn patch_results(
    state: &AppState,
    document: &mut Value,
    kind: Option<MediaKind>,
    asked: &str,
    in_its_language: bool,
) -> AppResult<bool> {
    let entries: Vec<(usize, MediaKind, i64)> = match document.get("results") {
        Some(Value::Array(results)) => results
            .iter()
            .enumerate()
            .take(MOST_RESULTS)
            .filter_map(|(at, result)| {
                let kind = kind.or_else(|| result_kind(result))?;
                let id = result.get("id")?.as_i64()?;
                Some((at, kind, id))
            })
            .collect(),
        _ => return Ok(false),
    };

    let mut held: HashMap<(MediaKind, i64), String> = HashMap::new();
    for kind in [MediaKind::Movie, MediaKind::Series] {
        let ids: Vec<String> = entries
            .iter()
            .filter(|(_, of, _)| *of == kind)
            .map(|(_, _, id)| id.to_string())
            .collect();
        if ids.is_empty() {
            continue;
        }
        let found =
            repo::item::held_external_ids(&state.db, ExternalSource::tmdb_for(kind), &ids).await?;
        for (value, media_id) in found {
            if let Ok(id) = value.parse() {
                held.insert((kind, id), media_id);
            }
        }
    }
    if held.is_empty() {
        return Ok(false);
    }

    // Which of them a person locked something on, of the work itself: the
    // others are TMDB's as they came, and not read at all.
    let media_ids: Vec<String> = held.values().cloned().collect();
    let overrides = repo::override_field::list_for_many(&state.db, &media_ids).await?;
    let mut works: HashMap<String, MediaItem> = HashMap::new();
    for media_id in media_ids {
        let locked = overrides
            .get(&media_id)
            .is_some_and(|found| found.iter().any(|o| o.scope == "item"));
        if !locked || works.contains_key(&media_id) {
            continue;
        }
        if let Some(mut item) = service::load(state, &media_id).await?
            && !item.locked_fields.is_empty()
        {
            state.media.for_clients(&mut item);
            relay::lay_locks(state, &mut item, asked);
            works.insert(media_id, item);
        }
    }
    if works.is_empty() {
        return Ok(false);
    }

    let Some(Value::Array(results)) = document.get_mut("results") else {
        return Ok(false);
    };
    let unlocalize = |address: &str| state.media.unlocalize(address);
    let mut changed = false;
    for (at, kind, id) in entries {
        let Some(item) = held
            .get(&(kind, id))
            .and_then(|media_id| works.get(media_id))
        else {
            continue;
        };
        if let Some(Value::Object(result)) = results.get_mut(at) {
            changed |= write_locked(result, item, kind, in_its_language, &unlocalize);
        }
    }
    Ok(changed)
}

/// What a result of a page of several kinds says it is: a film, a series,
/// or neither — a person.
fn result_kind(result: &Value) -> Option<MediaKind> {
    match result.get("media_type")?.as_str()? {
        "movie" => Some(MediaKind::Movie),
        "tv" => Some(MediaKind::Series),
        _ => None,
    }
}

/// The work's locks, in TMDB's names: only the fields a person locked, only
/// into those TMDB's document carries — a title's own, or a result of a
/// list, which carries fewer. The work comes laid for the language the
/// document is in, so a text is written as it is locked for that language
/// ([`relay::lay_locks`]). The homepage and the genres' names only for a
/// client that asked in the catalogue's own language (`in_its_language`),
/// the one they were locked in: TMDB names the genres in the language asked,
/// and a German reader is answered TMDB's German homepage.
///
/// `unlocalize` gives the address a provider gave a picture this server
/// keeps a copy of: a TMDB client builds an image's address from TMDB's
/// path, so only one of TMDB's own can be named.
fn write_locked(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    kind: MediaKind,
    in_its_language: bool,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let mut changed = false;
    let (name, original_name) = match kind {
        MediaKind::Series => ("name", "original_name"),
        MediaKind::Movie => ("title", "original_title"),
    };
    let text = |value: &Option<String>| value.clone().map(Value::String);

    if relay::is_locked(item, "title") {
        changed |= put(map, name, Some(Value::String(item.title.clone())));
    }
    if relay::is_locked(item, "overview") {
        changed |= put(map, "overview", text(&item.overview));
    }
    if in_its_language {
        if relay::is_locked(item, "homepage") {
            changed |= put(map, "homepage", text(&item.homepage));
        }
        if relay::is_locked(item, "genres")
            && let Some(Value::Array(theirs)) = map.get("genres")
        {
            let genres = tmdb_genres(theirs, &item.genres).map(Value::Array);
            changed |= put(map, "genres", genres);
        }
    }

    if relay::is_locked(item, "originalTitle") {
        changed |= put(map, original_name, text(&item.original_title));
    }

    match kind {
        MediaKind::Series => {
            if relay::is_locked(item, "firstAired") {
                changed |= put(map, "first_air_date", text(&item.first_aired));
            }
            if relay::is_locked(item, "lastAired") {
                changed |= put(map, "last_air_date", text(&item.last_aired));
            }
            if relay::is_locked(item, "status") {
                let status = item.status.as_deref().and_then(tmdb_series_status);
                changed |= put(map, "status", status.map(|s| Value::String(s.to_string())));
            }
            if relay::is_locked(item, "runtime") {
                changed |= put(map, "episode_run_time", item.runtime.map(|r| json!([r])));
            }
        }
        MediaKind::Movie => {
            if relay::is_locked(item, "inCinemas") {
                changed |= put(map, "release_date", text(&item.in_cinemas));
            }
            if relay::is_locked(item, "runtime") {
                changed |= put(map, "runtime", item.runtime.map(|r| json!(r)));
            }
        }
    }

    // The poster and the background a person chose, when TMDB has them: a
    // kept copy is named by the address it came from.
    for (key, field, cover) in [
        ("poster_path", "primaryPoster", CoverType::Poster),
        ("backdrop_path", "primaryFanart", CoverType::Fanart),
    ] {
        if relay::is_locked(item, field) {
            let path = relay::chosen_image(item, cover)
                .and_then(|address| tmdb_image_path(&unlocalize(&address)));
            changed |= put(map, key, path.map(Value::String));
        }
    }

    changed
}

/// A season's locks, and its episodes', into TMDB's document of it. The
/// season is the catalogue's of the same number, as the merge pairs them —
/// unless the catalogue knows it under another TMDB id, when it is another
/// season and only its episodes are looked at, each on its own. The work
/// comes laid for the language the document is in: a name and an overview
/// are written as they are locked for it.
fn write_season(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    number: i32,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let mut changed = false;
    let theirs = map.get("id").and_then(Value::as_i64);
    let season = item
        .seasons
        .iter()
        .find(|s| s.season_number == number && s.tmdb_id.is_none_or(|id| Some(id) == theirs));
    if let Some(season) = season {
        let locked = |field: &str| relay::season_locked(item, number, field);
        if locked("title") {
            changed |= put(map, "name", season.title.clone().map(Value::String));
        }
        if locked("overview") {
            changed |= put(map, "overview", season.overview.clone().map(Value::String));
        }
        if locked("airDate") {
            changed |= put(map, "air_date", season.air_date.clone().map(Value::String));
        }
        if locked("primaryPoster") {
            let path = relay::chosen_season_poster(season)
                .and_then(|address| tmdb_image_path(&unlocalize(&address)));
            changed |= put(map, "poster_path", path.map(Value::String));
        }
    }

    if let Some(Value::Array(episodes)) = map.get_mut("episodes") {
        for episode in episodes.iter_mut() {
            if let Value::Object(episode) = episode {
                changed |= write_episode(episode, item, unlocalize);
            }
        }
    }
    changed
}

/// One episode's locks into TMDB's record of it, as the catalogue's episode
/// it is ([`catalogue_episode`]): its title and overview as they are locked
/// for the language the document is in, its date, runtime and still in
/// every language.
fn write_episode(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let (Some(season), Some(number)) = (
        map.get("season_number").and_then(Value::as_i64),
        map.get("episode_number").and_then(Value::as_i64),
    ) else {
        return false;
    };
    let theirs = map.get("id").and_then(Value::as_i64);
    let Some(episode) = catalogue_episode(item, season, number, theirs) else {
        return false;
    };
    let locked = |field: &str| relay::episode_locked(item, episode, field);

    let mut changed = false;
    if locked("title") {
        changed |= put(map, "name", Some(Value::String(episode.title.clone())));
    }
    if locked("overview") {
        changed |= put(map, "overview", episode.overview.clone().map(Value::String));
    }
    if locked("airDate") {
        changed |= put(map, "air_date", episode.air_date.clone().map(Value::String));
    }
    if locked("runtime") {
        changed |= put(map, "runtime", episode.runtime.map(|r| json!(r)));
    }
    if locked("image") {
        let path = episode
            .image
            .as_deref()
            .and_then(|address| tmdb_image_path(&unlocalize(address)));
        changed |= put(map, "still_path", path.map(Value::String));
    }
    changed
}

/// The catalogue's episode a TMDB episode is: a special by the TMDB id the
/// merge gave it, since TMDB numbers its specials its own way; any other by
/// its numbers, as the merge pairs them — unless the catalogue knows that
/// one under another TMDB id, when it is another episode.
fn catalogue_episode(
    item: &MediaItem,
    season: i64,
    number: i64,
    theirs: Option<i64>,
) -> Option<&Episode> {
    if season == 0 {
        let theirs = theirs?;
        return item.episodes.iter().find(|e| e.tmdb_id == Some(theirs));
    }
    item.episodes.iter().find(|e| {
        i64::from(e.season_number) == season
            && i64::from(e.episode_number) == number
            && e.tmdb_id.is_none_or(|id| Some(id) == theirs)
    })
}

/// What the sources gave a work and a person hid here, as TMDB names each:
/// a credit by the person's TMDB id and its kind, a picture by the path TMDB
/// files it under, an alternative title by its text and the country TMDB
/// files it for. A credit a source named by the person's name alone — one of
/// Skyhook's, Fan-Kai's — is none TMDB's lists can be told by: TMDB names
/// every person by an id, and a name could be somebody else's.
struct Withheld {
    credits: Vec<(CreditType, i64)>,
    pictures: HashSet<String>,
    titles: Vec<(String, Option<String>)>,
}

impl Withheld {
    fn of(item: &MediaItem, unlocalize: &dyn Fn(&str) -> String) -> Self {
        Self {
            credits: item
                .hidden
                .credits
                .iter()
                .filter_map(|credit| Some((credit.credit_type, credit.tmdb_person_id?)))
                .collect(),
            pictures: item
                .hidden
                .images
                .iter()
                .filter_map(|image| tmdb_image_path(&unlocalize(&image.url)))
                .collect(),
            titles: item
                .hidden
                .alternative_titles
                .iter()
                .map(|title| {
                    (
                        folded(&title.title),
                        title.language.clone().filter(|code| !code.is_empty()),
                    )
                })
                .collect(),
        }
    }

    fn hides(&self, kind: CreditType, person: i64) -> bool {
        self.credits.contains(&(kind, person))
    }

    /// Whether TMDB's alternative title is one hidden here: its text, its
    /// case and the blanks around it aside, and its country, as the
    /// catalogue files it ([`iso_3166_2_to_3`]).
    fn hides_title(&self, entry: &Value) -> bool {
        let Some(title) = entry.get("title").and_then(Value::as_str) else {
            return false;
        };
        let country = entry
            .get("iso_3166_1")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .map(iso_3166_2_to_3);
        let title = folded(title);
        self.titles
            .iter()
            .any(|(text, filed)| *text == title && *filed == country)
    }
}

/// A title as it is compared: its case and the blanks around it aside.
fn folded(text: &str) -> String {
    text.trim().to_lowercase()
}

/// What a person hid of a work, taken out of TMDB's document of it: of its
/// people — a series' creators, its `credits` and `aggregate_credits` when
/// they were asked with it — of its pictures and of its other titles. The
/// people of a season or an episode — its guest stars, its crew — are not
/// the work's credits, which are all a person can hide, and stay as TMDB
/// gives them. Whether anything was taken out.
fn take_hidden(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let withheld = Withheld::of(item, unlocalize);
    let mut taken = take_people(map, &withheld);
    for key in ["credits", "aggregate_credits"] {
        if let Some(Value::Object(people)) = map.get_mut(key) {
            taken |= take_people(people, &withheld);
        }
    }
    if let Some(Value::Object(pictures)) = map.get_mut("images") {
        taken |= take_pictures(pictures, &withheld);
    }
    if let Some(Value::Object(titles)) = map.get_mut("alternative_titles") {
        taken |= take_titles(titles, &withheld);
    }
    taken
}

/// TMDB's lists of a work's people, less those hidden here: its cast by the
/// person's id among the actors hidden, its crew by the person's id and the
/// kind of credit their job makes ([`crew_kind`]), a series' creators among
/// its writers — a creator is one, as the catalogue files them.
fn take_people(map: &mut Map<String, Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    for (key, kind) in [
        ("cast", CreditType::Actor),
        ("created_by", CreditType::Writer),
    ] {
        if let Some(Value::Array(people)) = map.get_mut(key) {
            let before = people.len();
            people.retain(|person| {
                !person
                    .get("id")
                    .and_then(Value::as_i64)
                    .is_some_and(|id| withheld.hides(kind, id))
            });
            taken |= people.len() != before;
        }
    }
    if let Some(Value::Array(crew)) = map.get_mut("crew") {
        taken |= take_crew(crew, withheld);
    }
    taken
}

/// The kind of credit a job of TMDB's crew is, as the catalogue reads it
/// ([`crate::providers::tmdb::map`]): a director by the job, a producer by
/// the job — the producers proper, not the executive ones — and a writer by
/// the department, whatever the job.
fn crew_kind(job: &str, department: Option<&str>) -> Option<CreditType> {
    match job {
        "Director" => Some(CreditType::Director),
        "Producer" => Some(CreditType::Producer),
        _ if department == Some("Writing") => Some(CreditType::Writer),
        _ => None,
    }
}

/// TMDB's crew, less the jobs hidden here: an entry a job, or in a series'
/// aggregate a person with every job they had, of which the hidden ones are
/// taken out — and the person with them when none is left.
fn take_crew(crew: &mut Vec<Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    crew.retain_mut(|member| {
        let Value::Object(member) = member else {
            return true;
        };
        let Some(id) = member.get("id").and_then(Value::as_i64) else {
            return true;
        };
        let department = member
            .get("department")
            .and_then(Value::as_str)
            .map(str::to_string);
        let hidden = |job: &Value| {
            job.as_str()
                .and_then(|job| crew_kind(job, department.as_deref()))
                .is_some_and(|kind| withheld.hides(kind, id))
        };
        if let Some(Value::Array(jobs)) = member.get_mut("jobs") {
            let before = jobs.len();
            jobs.retain(|job| !job.get("job").is_some_and(hidden));
            if jobs.len() == before {
                return true;
            }
            taken = true;
            return !jobs.is_empty();
        }
        if member.get("job").is_some_and(hidden) {
            taken = true;
            return false;
        }
        true
    });
    taken
}

/// TMDB's lists of a work's or a season's pictures, less those hidden here,
/// told by the path TMDB files each under.
fn take_pictures(map: &mut Map<String, Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    for key in ["posters", "backdrops", "logos"] {
        if let Some(Value::Array(pictures)) = map.get_mut(key) {
            let before = pictures.len();
            pictures.retain(|picture| {
                !picture
                    .get("file_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| withheld.pictures.contains(path))
            });
            taken |= pictures.len() != before;
        }
    }
    taken
}

/// TMDB's list of a work's other titles — a film's `titles`, a series'
/// `results` — less those hidden here.
fn take_titles(map: &mut Map<String, Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    for key in ["titles", "results"] {
        if let Some(Value::Array(titles)) = map.get_mut(key) {
            let before = titles.len();
            titles.retain(|title| !withheld.hides_title(title));
            taken |= titles.len() != before;
        }
    }
    taken
}

/// Write `value` over a field TMDB's document carries; a field it does not
/// carry — an error's, a document of another shape — is not added, and no
/// value leaves the field as TMDB gave it. Whether it was written.
fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) -> bool {
    match (map.get_mut(key), value) {
        (Some(slot), Some(value)) => {
            *slot = value;
            true
        }
        _ => false,
    }
}

/// TMDB's own genre objects for the genres locked here, in the order they
/// were locked: each found among those TMDB's document carries by its name,
/// so that its id is TMDB's. None when one of them is not there — an id
/// cannot be made up, and a genre without one breaks the clients that link
/// or key by it — and TMDB's genres then stay as they came.
fn tmdb_genres(theirs: &[Value], locked: &[String]) -> Option<Vec<Value>> {
    let mut genres: Vec<Value> = Vec::with_capacity(locked.len());
    for name in locked {
        let wanted = name.trim().to_lowercase();
        let genre = theirs.iter().find(|genre| {
            genre.get("id").is_some_and(Value::is_number)
                && genre
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|theirs| theirs.trim().to_lowercase() == wanted)
        })?;
        if !genres.contains(genre) {
            genres.push(genre.clone());
        }
    }
    Some(genres)
}

/// The path TMDB files an image under, from its address: `/abc.jpg` from
/// `https://image.tmdb.org/t/p/original/abc.jpg`, whatever the size asked.
fn tmdb_image_path(address: &str) -> Option<String> {
    let rest = address.strip_prefix("https://image.tmdb.org/t/p/")?;
    let (_size, file) = rest.split_once('/')?;
    (!file.is_empty() && !file.contains('/')).then(|| format!("/{file}"))
}

/// Canonical status back to TMDB's vocabulary; anything else is left as
/// TMDB said it.
fn tmdb_series_status(status: &str) -> Option<&'static str> {
    match status {
        "ended" => Some("Ended"),
        "upcoming" => Some("Planned"),
        "continuing" => Some("Returning Series"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_path_that_climbs_out_of_v3_is_not_a_tmdb_path() {
        use super::climbs;
        // The outgoing URL is built by interpolation and parsed by reqwest,
        // which resolves dot segments — so this one would have left the v3 API
        // for the v4 one, carrying the operator's credentials with it.
        assert!(climbs("/3/%2e%2e/4/account"));
        // A backslash is a slash to that parser: this one would have left the
        // public lists for an account's, carrying the operator's token.
        assert!(climbs("/4/list/..\\account/1/lists"));
        assert!(climbs("/4/list/8136%2F..%2F..%2Faccount"));
        assert!(climbs("/3/movie/238%5C..%5C4"));
        // The parser's own reading of it — why the outgoing URL is checked
        // against the path too, whatever `climbs` misses.
        let url = url::Url::parse("https://api.themoviedb.org/4/list/..\\account/1/lists").unwrap();
        assert_eq!(url.path(), "/4/account/1/lists");
        assert!(!url.path().ends_with("/4/list/..\\account/1/lists"));
        let url = url::Url::parse("https://api.themoviedb.org/3/movie/238?api_key=x").unwrap();
        assert!(url.path().ends_with("/3/movie/238"));
    }

    #[test]
    fn of_tmdbs_v4_only_the_public_lists_are_relayed() {
        assert!(v4_allowed("/4/list/8136"));
        assert!(v4_allowed("/4/list/8136/item_status"));
        for refused in [
            "/4/list/",
            "/4/list",
            "/4/account/123/movie/watchlist",
            "/4/auth/request_token",
            "/4/",
        ] {
            assert!(!v4_allowed(refused), "{refused}");
        }
        assert!(climbs("/3/../4/account"));
        assert!(climbs("/3/tv/%2E%2E/list"));
        assert!(climbs("/3/./tv/1396"));

        assert!(!climbs("/3/tv/1396"));
        assert!(!climbs("/3/search/movie"));
        assert!(!climbs("/3/movie/550/credits"));
        // A dot inside a segment is just a character.
        assert!(!climbs("/3/configuration/countries.json"));
    }

    use super::*;
    use crate::{
        api::relay::testing::{laid_for, read_with},
        domain::{Image, Season},
    };

    fn uri(text: &str) -> Uri {
        text.parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn the_relay_strips_every_credential_the_caller_presented_to_us() {
        // A key accepted here must never be forwarded upstream, in any of the
        // spellings the guard accepts — nor the session cookie, which reaches
        // this route because the interface is same-origin with it.
        for name in crate::auth::middleware::API_KEY_PARAMS {
            assert!(
                OVERRIDDEN_PARAMS.contains(name),
                "{name} authenticates here but is relayed to TMDB"
            );
        }

        let asked = headers(&[
            ("user-agent", "Jellyseerr"),
            ("accept", "application/json"),
            ("accept-language", "fr-FR"),
            ("cookie", "ams_session=secret"),
            ("authorization", "Bearer ams_key"),
            ("x-api-key", "ams_key"),
            ("x-forwarded-for", "192.168.1.20"),
            ("forwarded", "for=192.168.1.20"),
            ("referer", "https://ams.lan/work/x"),
            ("origin", "https://ams.lan"),
            (crate::providers::radarr::LOOP_HEADER, "another-instance"),
            ("host", "api.themoviedb.org"),
            ("connection", "keep-alive"),
            ("accept-encoding", "gzip"),
            ("range", "bytes=0-0"),
            ("if-range", "\"tag\""),
            ("if-match", "\"tag\""),
            ("if-none-match", "\"tag\""),
            ("if-modified-since", "Wed, 21 Oct 2015 07:28:00 GMT"),
        ]);

        for validators in [false, true] {
            let forwarded = forwarded_headers(&asked, validators);
            assert_eq!(forwarded.get(header::USER_AGENT).unwrap(), "Jellyseerr");
            assert_eq!(forwarded.get(header::ACCEPT).unwrap(), "application/json");
            assert_eq!(forwarded.get(header::ACCEPT_LANGUAGE).unwrap(), "fr-FR");
            for never in [
                "cookie",
                "authorization",
                "x-api-key",
                "x-forwarded-for",
                "forwarded",
                "referer",
                "origin",
                crate::providers::radarr::LOOP_HEADER,
                "host",
                "connection",
                "accept-encoding",
                "range",
                "if-range",
                "if-match",
            ] {
                assert!(forwarded.get(never).is_none(), "{never} was forwarded");
            }
            // The validators, only where TMDB may answer them.
            assert_eq!(forwarded.get(header::IF_NONE_MATCH).is_some(), validators);
            assert_eq!(
                forwarded.get(header::IF_MODIFIED_SINCE).is_some(),
                validators
            );
        }
    }

    #[test]
    fn an_answer_names_no_address_of_tmdbs_and_takes_no_cookie() {
        let answer = headers(&[
            ("content-type", "application/json;charset=utf-8"),
            (
                "location",
                "https://api.themoviedb.org/3/movie/1?api_key=0123456789abcdef",
            ),
            ("content-location", "/3/movie/1?api_key=0123456789abcdef"),
            ("set-cookie", "a=b"),
            ("access-control-allow-origin", "*"),
            ("etag", "W/\"x\""),
        ]);
        let kept = answer_headers(&answer);
        assert!(kept.get(header::CONTENT_TYPE).is_some());
        assert!(kept.get(header::ETAG).is_some());
        for gone in [
            "location",
            "content-location",
            "set-cookie",
            "access-control-allow-origin",
        ] {
            assert!(kept.get(gone).is_none(), "{gone} was kept");
        }
    }

    #[test]
    fn only_a_read_nobodys_session_is_in_is_kept() {
        for kept in [
            "/3/movie/550",
            "/3/movie/550?language=fr-FR&append_to_response=credits",
            "/3/search/movie?query=account",
            "/3/configuration",
            "/3/tv/1396/season/1",
            "/4/list/8136",
        ] {
            assert!(may_keep(&Method::GET, &uri(kept)), "{kept}");
        }
        for personal in [
            // Minted for one caller.
            "/3/authentication/token/new",
            "/3/authentication/guest_session/new",
            "/3/authentication",
            // Read with one caller's session.
            "/3/guest_session/abc/rated/movies",
            "/3/account",
            "/3/account/1/favorite/movies?session_id=s",
            "/3/movie/550/account_states?session_id=s",
            "/3/movie/550?guest_session_id=g",
            "/3/movie/550?Session_ID=s",
            "/3/movie/550?session%5Fid=s",
            // Spelt so that a plain comparison misses it.
            "/3/%61ccount/1/lists",
            "/3/Account/1",
            "/3//account/1",
        ] {
            assert!(!may_keep(&Method::GET, &uri(personal)), "{personal}");
        }
        // A HEAD is never kept, nor answered from what is.
        assert!(!may_keep(&Method::HEAD, &uri("/3/movie/550")));
    }

    #[test]
    fn only_a_whole_document_is_kept() {
        assert_eq!(
            kept_for(StatusCode::OK, "/3/configuration"),
            Some(Duration::from_secs(24 * 60 * 60))
        );
        assert_eq!(
            kept_for(StatusCode::OK, "/3/movie/550"),
            Some(cache::relay_ttl("/3/movie/550"))
        );
        for status in [
            StatusCode::NON_AUTHORITATIVE_INFORMATION,
            StatusCode::NO_CONTENT,
            StatusCode::PARTIAL_CONTENT,
            StatusCode::MOVED_PERMANENTLY,
            StatusCode::FOUND,
            StatusCode::NOT_MODIFIED,
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::NOT_FOUND,
            StatusCode::RANGE_NOT_SATISFIABLE,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert_eq!(kept_for(status, "/3/configuration"), None, "{status}");
        }
    }

    #[test]
    fn a_kept_document_is_told_apart_by_everything_that_changes_it() {
        let none = HeaderMap::new();

        // One request, however its query is spelt, and whatever key it
        // carried for this server.
        assert_eq!(
            filed_under(&uri("/3/movie/550?language=fr&page=1"), &none),
            filed_under(
                &uri("/3/movie/550?page=1&api_key=ams_x&language=fr&include_adult=true"),
                &none
            )
        );
        assert_eq!(
            filed_under(&uri("/3/movie/550?language=fr"), &none),
            filed_under(&uri("/3/movie/550?language=fr&include%5Fadult=true"), &none)
        );

        // Another language, another order of a parameter given twice,
        // another path: another answer.
        let french = filed_under(&uri("/3/movie/550?language=fr"), &none);
        assert_ne!(french, filed_under(&uri("/3/movie/550?language=de"), &none));
        assert_ne!(
            filed_under(&uri("/3/movie/550?language=de&language=fr"), &none),
            filed_under(&uri("/3/movie/550?language=fr&language=de"), &none)
        );
        assert_ne!(french, filed_under(&uri("/3/tv/550?language=fr"), &none));

        // The headers that travel and may change the answer.
        let path = uri("/3/movie/550");
        let plain = filed_under(&path, &none);
        assert_ne!(
            plain,
            filed_under(&path, &headers(&[("accept-language", "de-DE")]))
        );
        assert_ne!(
            plain,
            filed_under(&path, &headers(&[("accept", "application/xml")]))
        );
        // What does not travel, or does not change TMDB's answer, files
        // nothing apart.
        assert_eq!(
            plain,
            filed_under(
                &path,
                &headers(&[
                    ("range", "bytes=0-0"),
                    ("user-agent", "x"),
                    ("cookie", "a=b")
                ])
            )
        );
        // A value cannot pass for another header's.
        assert_ne!(
            filed_under(
                &path,
                &headers(&[("accept", "x&accept-language=y"), ("accept-language", "z")])
            ),
            filed_under(
                &path,
                &headers(&[("accept", "x"), ("accept-language", "y&accept-language=z")])
            )
        );
    }

    #[test]
    fn upstream_is_asked_without_the_parameters_this_server_sets() {
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri(
                "/3/movie/550?language=fr&include%5Fadult=true&API_KEY=theirs&apikey=ams_x&adult&page=2",
            ),
            &Asking::Operator("0123456789abcdef".into()),
            false,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/3/movie/550?language=fr&adult&page=2\
             &api_key=0123456789abcdef&include_adult=false"
        );

        // A v4 token travels in a header, not in the address.
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri("/4/list/8136?page=1"),
            &Asking::Operator("eyJtoken".into()),
            true,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/4/list/8136?page=1&include_adult=true"
        );
    }

    #[test]
    fn the_clients_own_key_travels_as_it_came_and_nothing_of_ours_does() {
        // Their `api_key` is TMDB's own parameter, and theirs: handed on.
        // `apikey` is this server's, and the word on adult titles too.
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri("/3/movie/550?api_key=their-tmdb-key&language=fr&apikey=x&include_adult=true"),
            &Asking::Client,
            false,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/3/movie/550?api_key=their-tmdb-key&language=fr\
             &include_adult=false"
        );
        // No key at all: none is added, and TMDB says so itself.
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri("/3/movie/550"),
            &Asking::Client,
            false,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/3/movie/550?include_adult=false"
        );
    }

    #[test]
    fn the_language_asked_is_tmdbs_reading_of_the_query() {
        assert_eq!(asked_language(&uri("/3/movie/1?language=de-DE")), "de-DE");
        assert_eq!(
            asked_language(&uri("/3/movie/1?language=en&language=fr")),
            "fr"
        );
        // TMDB answers `en-US` when no language is asked.
        assert_eq!(asked_language(&uri("/3/movie/1")), "en-US");
        assert_eq!(asked_language(&uri("/3/movie/1?language=")), "en-US");
    }

    #[test]
    fn a_homepage_and_the_genres_are_locked_in_the_language_of_the_catalogue() {
        let asks = |text: &str, catalogue: &str| asks_in(&asked_language(&uri(text)), catalogue);
        assert!(asks("/3/movie/1?language=fr-FR", "fr-FR"));
        assert!(asks("/3/movie/1?language=fr", "fr-FR"));
        assert!(asks("/3/movie/1?language=fr-CA", "fr-FR"));
        assert!(asks("/3/movie/1?language=en&language=fr", "fr-FR"));
        assert!(asks("/3/movie/1", "en-US"));
        assert!(asks("/3/movie/1?language=", "en-US"));
        assert!(!asks("/3/movie/1", "fr-FR"));

        assert!(!asks("/3/movie/1?language=de-DE", "fr-FR"));
        assert!(!asks("/3/movie/1?language=fr&language=de", "fr-FR"));
        assert!(!asks("/3/movie/1?language=../../x", "fr-FR"));
    }

    #[test]
    fn the_documents_patched_are_recognised() {
        assert_eq!(
            patch_target("/3/tv/1396"),
            Some(Target::Title {
                kind: MediaKind::Series,
                tmdb_id: 1396
            })
        );
        assert_eq!(
            patch_target("/3/movie/329865"),
            Some(Target::Title {
                kind: MediaKind::Movie,
                tmdb_id: 329865
            })
        );
        assert_eq!(
            patch_target("/3/tv/1396/"),
            Some(Target::Title {
                kind: MediaKind::Series,
                tmdb_id: 1396
            })
        );
        assert_eq!(
            patch_target("/3/tv/1396/season/1"),
            Some(Target::Season {
                tmdb_id: 1396,
                season: 1
            })
        );
        assert_eq!(
            patch_target("/3/tv/1396/season/0/episode/3"),
            Some(Target::Episode { tmdb_id: 1396 })
        );
        for (path, kind) in [
            ("/3/search/movie", Some(MediaKind::Movie)),
            ("/3/search/tv", Some(MediaKind::Series)),
            ("/3/search/multi", None),
            ("/3/trending/movie/week", Some(MediaKind::Movie)),
            ("/3/trending/tv/day", Some(MediaKind::Series)),
            ("/3/trending/all/day", None),
            ("/3/discover/movie", Some(MediaKind::Movie)),
            ("/3/discover/tv", Some(MediaKind::Series)),
        ] {
            assert_eq!(patch_target(path), Some(Target::Results { kind }), "{path}");
        }
    }

    #[test]
    fn other_documents_are_left_alone() {
        // A season's people are not the work's credits; a film has no
        // aggregate; a list TMDB keeps besides is none a person hides from.
        assert_eq!(patch_target("/3/tv/1396/season/1/credits"), None);
        assert_eq!(patch_target("/3/tv/1396/season/1/aggregate_credits"), None);
        assert_eq!(patch_target("/3/tv/1396/season/1/episode/1/credits"), None);
        assert_eq!(patch_target("/3/movie/329865/aggregate_credits"), None);
        assert_eq!(patch_target("/3/movie/329865/keywords"), None);
        assert_eq!(patch_target("/3/movie/329865/credits/extra"), None);
        assert_eq!(patch_target("/3/tv/1396/season/x"), None);
        assert_eq!(patch_target("/3/tv/1396/season/1/episode/-1"), None);
        assert_eq!(patch_target("/3/search/person"), None);
        assert_eq!(patch_target("/3/search/collection"), None);
        assert_eq!(patch_target("/3/trending/person/day"), None);
        assert_eq!(patch_target("/3/trending/all"), None);
        assert_eq!(patch_target("/3/configuration"), None);
        assert_eq!(patch_target("/3/tv/not-a-number"), None);
        assert_eq!(patch_target("/3/tv"), None);
        assert_eq!(patch_target("/other/tv/1"), None);
        assert_eq!(patch_target("/4/list/8136"), None);
    }

    fn as_is(address: &str) -> String {
        address.to_string()
    }

    /// *Fight Club* as its sources gave it: two actors, a director, a
    /// writer, two posters and three other titles.
    pub(super) fn fight_club() -> MediaItem {
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
        let poster = |path: &str| Image {
            id: crate::db::new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: format!("https://image.tmdb.org/t/p/original{path}"),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        };
        let title = |text: &str, country: &str| crate::domain::AlternativeTitle {
            id: crate::db::new_id(),
            title: text.into(),
            title_type: None,
            language: Some(country.into()),
            is_manual: false,
        };
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Fight Club".into();
        item.slug = "fight-club-1999".into();
        item.external_ids.tmdb = Some(550);
        item.credits = vec![
            credit(CreditType::Actor, 819, "Edward Norton"),
            credit(CreditType::Actor, 287, "Brad Pitt"),
            credit(CreditType::Director, 7467, "David Fincher"),
            credit(CreditType::Writer, 7468, "Jim Uhls"),
        ];
        item.images = vec![poster("/kept.jpg"), poster("/hidden.jpg")];
        item.alternative_titles = vec![
            title("El club de la pelea", "mex"),
            title("Fight Club – Hidden", "usa"),
        ];
        item
    }

    /// What a person hid of it: an actor, the director, a poster, a title.
    pub(super) fn hidden_of_fight_club() -> Vec<(&'static str, &'static str, &'static str, Value)> {
        vec![
            ("credit:actor:tmdb:819", "hidden", "", json!(true)),
            ("credit:director:tmdb:7467", "hidden", "", json!(true)),
            (
                "image:https://image.tmdb.org/t/p/original/hidden.jpg",
                "hidden",
                "",
                json!(true),
            ),
            (
                "alternativeTitle:usa:Fight Club – Hidden",
                "hidden",
                "",
                json!(true),
            ),
        ]
    }

    /// The ids of a list of TMDB's people, in order.
    fn ids(list: &Value) -> Vec<i64> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_i64().unwrap())
            .collect()
    }

    #[test]
    fn the_lists_a_titles_sources_gave_are_documents_patched_too() {
        let part = |kind, tmdb_id, part| {
            Some(Target::Part {
                kind,
                tmdb_id,
                part,
            })
        };
        assert_eq!(
            patch_target("/3/movie/550/credits"),
            part(MediaKind::Movie, 550, Part::People)
        );
        assert_eq!(
            patch_target("/3/tv/1396/aggregate_credits"),
            part(MediaKind::Series, 1396, Part::People)
        );
        assert_eq!(
            patch_target("/3/movie/550/images"),
            part(MediaKind::Movie, 550, Part::Pictures)
        );
        assert_eq!(
            patch_target("/3/tv/1396/season/2/images"),
            part(MediaKind::Series, 1396, Part::Pictures)
        );
        assert_eq!(
            patch_target("/3/tv/1396/alternative_titles"),
            part(MediaKind::Series, 1396, Part::Titles)
        );
        assert_eq!(patch_target("/3/movie/x/credits"), None);
    }

    #[test]
    fn what_a_person_hid_is_taken_out_of_tmdbs_lists() {
        let item = read_with(fight_club(), &hidden_of_fight_club());
        assert_eq!(item.hidden.credits.len(), 2);

        let Value::Object(mut document) = json!({
            "id": 550,
            "title": "Fight Club",
            "credits": {
                "cast": [
                    { "id": 819, "name": "Edward Norton", "character": "Narrator" },
                    { "id": 287, "name": "Brad Pitt", "character": "Tyler Durden" }
                ],
                "crew": [
                    { "id": 7467, "name": "David Fincher", "job": "Director", "department": "Directing" },
                    // The same person in another job is another credit.
                    { "id": 7467, "name": "David Fincher", "job": "Producer", "department": "Production" },
                    { "id": 7468, "name": "Jim Uhls", "job": "Screenplay", "department": "Writing" }
                ]
            },
            "images": {
                "posters": [ { "file_path": "/kept.jpg" }, { "file_path": "/hidden.jpg" } ],
                "backdrops": [ { "file_path": "/backdrop.jpg" } ],
                "logos": []
            },
            "alternative_titles": { "titles": [
                { "iso_3166_1": "MX", "title": "El club de la pelea" },
                { "iso_3166_1": "US", "title": " fight club – hidden " },
                // The same text, filed for another country: another title.
                { "iso_3166_1": "GB", "title": "Fight Club – Hidden" }
            ] }
        }) else {
            unreachable!()
        };
        assert!(take_hidden(&mut document, &item, &as_is));

        assert_eq!(ids(&document["credits"]["cast"]), [287]);
        let crew: Vec<(i64, &str)> = document["credits"]["crew"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["id"].as_i64().unwrap(), c["job"].as_str().unwrap()))
            .collect();
        assert_eq!(crew, [(7467, "Producer"), (7468, "Screenplay")]);
        assert_eq!(
            document["images"]["posters"],
            json!([{ "file_path": "/kept.jpg" }])
        );
        assert_eq!(document["images"]["backdrops"].as_array().unwrap().len(), 1);
        let countries: Vec<&str> = document["alternative_titles"]["titles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["iso_3166_1"].as_str().unwrap())
            .collect();
        assert_eq!(countries, ["MX", "GB"]);
        assert_eq!(document["title"], json!("Fight Club"));

        // Taken out once: nothing more to take.
        assert!(!take_hidden(&mut document, &item, &as_is));
    }

    #[test]
    fn a_series_aggregate_and_its_creators_lose_what_was_hidden_alone() {
        let withheld = Withheld {
            credits: vec![
                (CreditType::Actor, 17419),
                (CreditType::Director, 66633),
                (CreditType::Writer, 66633),
            ],
            pictures: HashSet::new(),
            titles: Vec::new(),
        };
        let Value::Object(mut aggregate) = json!({
            "cast": [
                { "id": 17419, "roles": [ { "character": "Walter White" } ] },
                { "id": 84497, "roles": [ { "character": "Jesse Pinkman" } ] }
            ],
            "crew": [
                // Of a person's jobs, the hidden ones alone go.
                { "id": 66633, "department": "Directing",
                  "jobs": [ { "job": "Director" }, { "job": "Second Unit Director" } ] },
                // Every job a writer's, whatever it is called: the person goes.
                { "id": 66633, "department": "Writing", "jobs": [ { "job": "Writer" } ] },
                { "id": 66633, "department": "Production", "jobs": [ { "job": "Executive Producer" } ] },
                { "id": 29924, "department": "Directing", "jobs": [ { "job": "Director" } ] }
            ]
        }) else {
            unreachable!()
        };
        assert!(take_people(&mut aggregate, &withheld));
        assert_eq!(ids(&aggregate["cast"]), [84497]);
        assert_eq!(ids(&aggregate["crew"]), [66633, 66633, 29924]);
        assert_eq!(
            aggregate["crew"][0]["jobs"],
            json!([{ "job": "Second Unit Director" }])
        );
        assert_eq!(aggregate["crew"][1]["department"], json!("Production"));

        // A series' creator is one of its writers.
        let Value::Object(mut series) = json!({
            "created_by": [ { "id": 66633, "name": "Vince Gilligan" } ],
            "name": "Breaking Bad"
        }) else {
            unreachable!()
        };
        assert!(take_people(&mut series, &withheld));
        assert_eq!(series["created_by"], json!([]));
    }

    fn series_document() -> Map<String, Value> {
        let Value::Object(map) = json!({
            "id": 1396,
            "name": "Breaking Bad",
            "original_name": "Breaking Bad",
            "overview": "A chemistry teacher…",
            "homepage": "https://www.sonypictures.com/tv/breakingbad",
            "first_air_date": "2008-01-20",
            "last_air_date": "2013-09-29",
            "status": "Ended",
            "episode_run_time": [45, 47],
            "genres": [
                { "id": 18, "name": "Drame" },
                { "id": 80, "name": "Crime" },
                { "id": 9648, "name": "Mystère" }
            ],
            "poster_path": "/ggFHVNu6YYI5L9pCfOacjizRGt.jpg",
            "backdrop_path": "/tsRy63Mu5cu8etL1X7ZLyf7UP1M.jpg"
        }) else {
            unreachable!()
        };
        map
    }

    fn locked_series(fields: &[&str]) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.original_title = Some("Mon titre original".into());
        item.overview = Some("Mon résumé".into());
        item.homepage = Some("https://example.fr".into());
        item.first_aired = Some("2008-01-21".into());
        item.last_aired = Some("2013-09-30".into());
        item.status = Some("continuing".into());
        item.runtime = Some(50);
        item.genres = vec!["Crime".into(), "drame".into()];
        item.locked_fields = fields.iter().map(|f| format!("item/{f}")).collect();
        item
    }

    #[test]
    fn only_the_fields_locked_are_written() {
        let mut document = series_document();
        let item = locked_series(&["title"]);

        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));

        assert_eq!(document["name"], json!("Mon titre"));
        // Not locked: as TMDB gave it, whatever the catalogue holds.
        let upstream = series_document();
        for field in [
            "overview",
            "homepage",
            "original_name",
            "first_air_date",
            "last_air_date",
            "status",
            "episode_run_time",
            "genres",
            "poster_path",
        ] {
            assert_eq!(document[field], upstream[field], "{field}");
        }
    }

    #[test]
    fn every_lock_is_written_in_tmdbs_names() {
        let mut document = series_document();
        let item = locked_series(&[
            "title",
            "originalTitle",
            "overview",
            "homepage",
            "firstAired",
            "lastAired",
            "status",
            "runtime",
            "genres",
        ]);

        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));

        assert_eq!(document["name"], json!("Mon titre"));
        assert_eq!(document["original_name"], json!("Mon titre original"));
        assert_eq!(document["overview"], json!("Mon résumé"));
        assert_eq!(document["homepage"], json!("https://example.fr"));
        assert_eq!(document["first_air_date"], json!("2008-01-21"));
        assert_eq!(document["last_air_date"], json!("2013-09-30"));
        assert_eq!(document["status"], json!("Returning Series"));
        assert_eq!(document["episode_run_time"], json!([50]));
        // TMDB's own genres, its ids kept, in the order locked.
        assert_eq!(
            document["genres"],
            json!([{ "id": 80, "name": "Crime" }, { "id": 18, "name": "Drame" }])
        );
        assert!(!document.contains_key("title"));
    }

    /// A series locked in a catalogue kept in French: its title in every
    /// language, its overview in French alone and in German alone, its
    /// homepage and its first date.
    fn locked_in_languages() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.overview = Some("A chemistry teacher…".into());
        read_with(
            item,
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "overview", "fra", json!("Mon résumé")),
                ("item", "overview", "deu", json!("Mein Überblick")),
                ("item", "homepage", "", json!("https://example.fr")),
                ("item", "firstAired", "", json!("2008-01-21")),
            ],
        )
    }

    #[test]
    fn a_text_is_written_as_it_is_locked_for_the_language_asked() {
        // In the catalogue's own language, every lock.
        let item = locked_in_languages();
        let mut document = series_document();
        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(document["name"], json!("Mon titre"));
        assert_eq!(document["overview"], json!("Mon résumé"));
        assert_eq!(document["homepage"], json!("https://example.fr"));

        // In German: the overview locked in German. The title locked in
        // every language is the catalogue's own, written in its language:
        // TMDB's German one stands, as its German homepage does.
        let german = laid_for(locked_in_languages(), "deu");
        let mut document = series_document();
        assert!(write_locked(
            &mut document,
            &german,
            MediaKind::Series,
            false,
            &as_is
        ));
        assert_eq!(document["name"], series_document()["name"]);
        assert_eq!(document["overview"], json!("Mein Überblick"));
        assert_eq!(document["homepage"], series_document()["homepage"]);
        // A date is one in every language.
        assert_eq!(document["first_air_date"], json!("2008-01-21"));

        // In Spanish: nothing was locked in it, and its texts are TMDB's
        // Spanish ones; the date locked in every language is written all
        // the same.
        let spanish = laid_for(locked_in_languages(), "spa");
        let mut document = series_document();
        assert!(write_locked(
            &mut document,
            &spanish,
            MediaKind::Series,
            false,
            &as_is
        ));
        assert_eq!(document["name"], series_document()["name"]);
        assert_eq!(document["overview"], series_document()["overview"]);
        assert_eq!(document["first_air_date"], json!("2008-01-21"));
    }

    #[test]
    fn a_genre_tmdb_has_no_id_for_leaves_tmdbs_genres_alone() {
        let mut document = series_document();
        let mut item = locked_series(&["genres"]);
        item.genres = vec!["Drame".into(), "Inventé ici".into()];

        assert!(!write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(document["genres"], series_document()["genres"]);

        // Never a genre without its id, whatever is locked.
        assert_eq!(
            tmdb_genres(
                &[json!({ "id": null, "name": "Drame" })],
                &["Drame".to_string()]
            ),
            None
        );
        assert_eq!(
            tmdb_genres(&[json!({ "id": 18, "name": "Drame" })], &[]),
            Some(vec![])
        );
    }

    #[test]
    fn a_movie_takes_its_locks_under_its_own_names() {
        let Value::Object(mut document) = json!({
            "id": 329865, "title": "Arrival", "original_title": "Arrival",
            "overview": "…", "release_date": "2016-11-10", "runtime": 116,
            "status": "Released"
        }) else {
            unreachable!()
        };
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Premier Contact".into();
        item.in_cinemas = Some("2016-12-07".into());
        item.runtime = Some(118);
        item.status = Some("released".into());
        item.locked_fields = vec![
            "item/title".into(),
            "item/inCinemas".into(),
            "item/runtime".into(),
            "item/status".into(),
        ];

        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Movie,
            true,
            &as_is
        ));

        assert_eq!(document["title"], json!("Premier Contact"));
        assert_eq!(document["release_date"], json!("2016-12-07"));
        assert_eq!(document["runtime"], json!(118));
        // A film's statuses are another set, and left alone.
        assert_eq!(document["status"], json!("Released"));
        assert!(!document.contains_key("name"));
    }

    #[test]
    fn an_answer_that_is_not_a_title_takes_nothing() {
        let Value::Object(mut error) = json!({
            "success": false, "status_code": 34,
            "status_message": "The resource you requested could not be found."
        }) else {
            unreachable!()
        };
        let item = locked_series(&["title", "overview", "firstAired", "runtime"]);
        assert!(!write_locked(
            &mut error,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(error.len(), 3);
    }

    #[test]
    fn a_result_takes_the_locks_it_carries_a_field_for() {
        // A result of a search: fewer fields than the title's own document.
        let Value::Object(mut result) = json!({
            "id": 1396, "name": "Breaking Bad", "original_name": "Breaking Bad",
            "overview": "…", "first_air_date": "2008-01-20",
            "poster_path": "/upstream.jpg", "genre_ids": [18], "popularity": 9.5
        }) else {
            unreachable!()
        };
        let item = locked_series(&["title", "overview", "firstAired", "homepage", "status"]);
        assert!(write_locked(
            &mut result,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(result["name"], json!("Mon titre"));
        assert_eq!(result["overview"], json!("Mon résumé"));
        assert_eq!(result["first_air_date"], json!("2008-01-21"));
        // What a result does not carry is not added to it.
        assert!(!result.contains_key("homepage"));
        assert!(!result.contains_key("status"));
        assert_eq!(result["genre_ids"], json!([18]));
    }

    #[test]
    fn a_result_says_its_kind_on_a_page_of_several() {
        assert_eq!(
            result_kind(&json!({ "media_type": "movie" })),
            Some(MediaKind::Movie)
        );
        assert_eq!(
            result_kind(&json!({ "media_type": "tv" })),
            Some(MediaKind::Series)
        );
        assert_eq!(result_kind(&json!({ "media_type": "person" })), None);
        assert_eq!(result_kind(&json!({ "id": 1 })), None);
    }

    #[test]
    fn a_chosen_picture_is_written_only_when_locked() {
        let mut item = locked_series(&[]);
        item.images.push(Image {
            id: "img1".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://ams.example/media/abc".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: true,
        });
        item.primary_images.poster = Some("img1".into());
        let origin = |_: &str| "https://image.tmdb.org/t/p/original/chosen.jpg".to_string();

        // Chosen by the sources' order, not by a person: TMDB's stays.
        let mut document = series_document();
        assert!(!write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &origin
        ));
        assert_eq!(document["poster_path"], series_document()["poster_path"]);

        // Chosen by a person: written, in any language.
        item.locked_fields = vec!["item/primaryPoster".into()];
        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            false,
            &origin
        ));
        assert_eq!(document["poster_path"], json!("/chosen.jpg"));
        assert_eq!(
            document["backdrop_path"],
            series_document()["backdrop_path"]
        );

        // A picture TMDB has no path for is not one to name.
        let mut document = series_document();
        assert!(!write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(document["poster_path"], series_document()["poster_path"]);
    }

    fn episode(season: i32, number: i32, tmdb_id: Option<i64>) -> Episode {
        Episode {
            id: format!("ep-{season}x{number}"),
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: format!("Épisode verrouillé {season}x{number}"),
            overview: Some(format!("Résumé verrouillé {season}x{number}")),
            air_date: Some("2008-02-01".into()),
            air_date_utc: None,
            runtime: Some(48),
            finale_type: None,
            image: Some("https://image.tmdb.org/t/p/original/still.jpg".into()),
            tvdb_id: None,
            tmdb_id,
            rating: None,
            is_manual: false,
        }
    }

    /// A series with a first season and a special, its season's and its
    /// episodes' fields to lock.
    fn series_with_a_season() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.seasons = vec![Season {
            id: "s1".into(),
            season_number: 1,
            title: Some("Première saison".into()),
            overview: Some("Le début.".into()),
            air_date: Some("2008-01-21".into()),
            tmdb_id: Some(3572),
            tvdb_id: None,
            is_manual: false,
            images: vec![Image {
                id: "season-poster".into(),
                season_number: Some(1),
                cover_type: CoverType::Poster,
                url: "https://image.tmdb.org/t/p/w500/season.jpg".into(),
                language: None,
                sort_order: 0,
                source: None,
                is_manual: true,
            }],
            primary_images: crate::domain::PrimaryImages {
                poster: Some("season-poster".into()),
                fanart: None,
            },
        }];
        item.episodes = vec![
            episode(1, 1, Some(62085)),
            episode(1, 2, None),
            episode(0, 1, Some(1_000_001)),
        ];
        item
    }

    /// [`series_with_a_season`], with its season's every field and its
    /// episodes' locked as they are held.
    fn locked_season() -> MediaItem {
        let mut item = series_with_a_season();
        item.locked_fields = [
            "season:1/title",
            "season:1/overview",
            "season:1/airDate",
            "season:1/primaryPoster",
            "episode:1x1/title",
            "episode:1x1/overview",
            "episode:1x1/airDate",
            "episode:1x1/runtime",
            "episode:1x1/image",
            "episode:1x2/title",
            "episode:0x1/title",
        ]
        .iter()
        .map(|key| key.to_string())
        .collect();
        item
    }

    fn season_document() -> Map<String, Value> {
        let Value::Object(map) = json!({
            "_id": "52542282760ee313280017f9",
            "id": 3572,
            "name": "Saison 1",
            "overview": "TMDB's overview",
            "air_date": "2008-01-20",
            "poster_path": "/upstream-season.jpg",
            "season_number": 1,
            "episodes": [
                { "id": 62085, "season_number": 1, "episode_number": 1, "name": "Pilot",
                  "overview": "TMDB's", "air_date": "2008-01-20", "runtime": 59,
                  "still_path": "/upstream-still.jpg" },
                { "id": 62086, "season_number": 1, "episode_number": 2, "name": "Cat's in the Bag...",
                  "overview": "TMDB's", "air_date": "2008-01-27", "runtime": 49,
                  "still_path": "/upstream-still-2.jpg" },
                { "id": 62087, "season_number": 1, "episode_number": 3, "name": "...And the Bag's in the River",
                  "overview": "TMDB's", "air_date": "2008-02-10", "runtime": 48,
                  "still_path": null }
            ]
        }) else {
            unreachable!()
        };
        map
    }

    #[test]
    fn a_season_takes_its_locks_and_its_episodes_theirs() {
        let item = locked_season();
        let mut document = season_document();

        assert!(write_season(&mut document, &item, 1, &as_is));

        assert_eq!(document["name"], json!("Première saison"));
        assert_eq!(document["overview"], json!("Le début."));
        assert_eq!(document["air_date"], json!("2008-01-21"));
        assert_eq!(document["poster_path"], json!("/season.jpg"));

        let episodes = document["episodes"].as_array().unwrap();
        assert_eq!(episodes[0]["name"], json!("Épisode verrouillé 1x1"));
        assert_eq!(episodes[0]["overview"], json!("Résumé verrouillé 1x1"));
        assert_eq!(episodes[0]["air_date"], json!("2008-02-01"));
        assert_eq!(episodes[0]["runtime"], json!(48));
        assert_eq!(episodes[0]["still_path"], json!("/still.jpg"));
        // Its title alone is locked: the rest is TMDB's.
        assert_eq!(episodes[1]["name"], json!("Épisode verrouillé 1x2"));
        assert_eq!(episodes[1]["overview"], json!("TMDB's"));
        assert_eq!(episodes[1]["runtime"], json!(49));
        // Not in the catalogue: as it came.
        assert_eq!(episodes[2]["name"], json!("...And the Bag's in the River"));
    }

    #[test]
    fn a_season_in_another_language_takes_what_was_locked_in_it() {
        // An episode's title locked in German alone, a date in every
        // language, the season's name in every language.
        let item = read_with(
            series_with_a_season(),
            &[
                ("season:1", "title", "", json!("Première saison")),
                ("episode:1x1", "title", "deu", json!("Der Anfang")),
                ("episode:1x1", "airDate", "", json!("2008-02-03")),
            ],
        );

        // A French reader: the German title is not French.
        let mut document = season_document();
        assert!(write_season(&mut document, &item, 1, &as_is));
        assert_eq!(document["name"], json!("Première saison"));
        assert_eq!(document["episodes"][0]["name"], json!("Pilot"));
        assert_eq!(document["episodes"][0]["air_date"], json!("2008-02-03"));

        // A German reader: the title locked in German, the date locked in
        // every language — and TMDB's German name of the season, the one
        // locked in every language being the catalogue's own text.
        let german = laid_for(item, "deu");
        let mut document = season_document();
        assert!(write_season(&mut document, &german, 1, &as_is));
        assert_eq!(document["name"], season_document()["name"]);
        assert_eq!(document["episodes"][0]["name"], json!("Der Anfang"));
        assert_eq!(document["episodes"][0]["air_date"], json!("2008-02-03"));
        // Nothing locked on it: TMDB's German stays.
        assert_eq!(
            document["episodes"][1]["name"],
            json!("Cat's in the Bag...")
        );
    }

    #[test]
    fn a_season_tmdb_numbers_otherwise_is_left_to_its_episodes() {
        // The catalogue's first season is another of TMDB's: its locks are
        // not this one's, though its number is.
        let item = locked_season();
        let mut document = season_document();
        document.insert("id".into(), json!(9999));
        assert!(write_season(&mut document, &item, 1, &as_is));
        assert_eq!(document["name"], json!("Saison 1"));
        assert_eq!(document["poster_path"], json!("/upstream-season.jpg"));
        // The episodes are judged each on its own.
        assert_eq!(
            document["episodes"][0]["name"],
            json!("Épisode verrouillé 1x1")
        );
    }

    #[test]
    fn an_episode_is_the_catalogues_by_its_numbers_and_a_special_by_its_id() {
        let item = locked_season();
        assert_eq!(
            catalogue_episode(&item, 1, 1, Some(62085)).map(|e| e.id.as_str()),
            Some("ep-1x1")
        );
        // Known under another TMDB id: another episode.
        assert!(catalogue_episode(&item, 1, 1, Some(1)).is_none());
        // Held with no TMDB id: its numbers say it.
        assert_eq!(
            catalogue_episode(&item, 1, 2, Some(62086)).map(|e| e.id.as_str()),
            Some("ep-1x2")
        );
        // A special by its TMDB id alone, whatever TMDB numbers it.
        assert_eq!(
            catalogue_episode(&item, 0, 7, Some(1_000_001)).map(|e| e.id.as_str()),
            Some("ep-0x1")
        );
        assert!(catalogue_episode(&item, 0, 1, Some(5)).is_none());
        assert!(catalogue_episode(&item, 0, 1, None).is_none());

        // One episode's own document.
        let Value::Object(mut special) = json!({
            "id": 1_000_001, "season_number": 0, "episode_number": 7,
            "name": "TMDB's special", "overview": "", "air_date": null, "runtime": null,
            "still_path": null
        }) else {
            unreachable!()
        };
        assert!(write_episode(&mut special, &item, &as_is));
        assert_eq!(special["name"], json!("Épisode verrouillé 0x1"));
    }

    #[test]
    fn statuses_round_trip_back_to_tmdbs_vocabulary() {
        assert_eq!(tmdb_series_status("ended"), Some("Ended"));
        assert_eq!(tmdb_series_status("continuing"), Some("Returning Series"));
        assert_eq!(tmdb_series_status("upcoming"), Some("Planned"));
        assert_eq!(tmdb_series_status("released"), None);
    }

    #[test]
    fn a_chosen_image_is_named_by_the_path_tmdb_files_it_under() {
        assert_eq!(
            tmdb_image_path("https://image.tmdb.org/t/p/original/abc.jpg").as_deref(),
            Some("/abc.jpg")
        );
        assert_eq!(
            tmdb_image_path("https://image.tmdb.org/t/p/w500/x_Y-z.png").as_deref(),
            Some("/x_Y-z.png")
        );
        assert_eq!(
            tmdb_image_path("https://assets.fanart.tv/fanart/tv/1/poster.jpg"),
            None
        );
        assert_eq!(tmdb_image_path("upload:0123"), None);
        assert_eq!(
            tmdb_image_path("https://image.tmdb.org/t/p/original/"),
            None
        );
    }
}

/// The relay as a client meets it, against a stand-in for TMDB that keeps
/// what it was asked.
#[cfg(test)]
mod relayed {
    use axum::{
        body::Body,
        extract::{Request, State},
        http::StatusCode,
    };
    use serde_json::{Value, json};

    use super::proxy;
    use crate::{
        api::relay::{
            self,
            testing::{self, Asked, OPERATOR_TMDB_KEY, StandIn},
        },
        config::{Surface, SurfacePolicy},
        domain::{Episode, MediaItem, MediaKind, Season},
        state::AppState,
    };

    /// TMDB as the stand-in answers it: every text its own, and a request
    /// that carries no key refused as TMDB refuses one.
    fn tmdb(asked: &Asked) -> (StatusCode, Value) {
        let keyed = asked
            .query
            .split('&')
            .any(|pair| pair.starts_with("api_key="))
            || asked.authorization.is_some();
        if !keyed {
            return (
                StatusCode::UNAUTHORIZED,
                json!({ "success": false, "status_code": 7, "status_message": "Invalid API key" }),
            );
        }
        let results = json!({ "page": 1, "total_pages": 1, "total_results": 2, "results": [
            { "id": 1396, "media_type": "tv", "name": "Breaking Bad", "overview": "TMDB's" },
            { "id": 17419, "media_type": "person", "name": "Bryan Cranston" }
        ]});
        let episode = json!({ "id": 62085, "season_number": 1, "episode_number": 1,
                              "name": "Pilot", "overview": "TMDB's" });
        match asked.path.as_str() {
            "/3/tv/1396" => (
                StatusCode::OK,
                json!({ "id": 1396, "name": "Breaking Bad", "overview": "TMDB's",
                        "first_air_date": "2008-01-20" }),
            ),
            "/3/tv/1396/season/1" => (
                StatusCode::OK,
                json!({ "id": 3572, "season_number": 1, "name": "Season 1", "overview": "",
                        "episodes": [episode] }),
            ),
            "/3/tv/1396/season/1/episode/1" => (StatusCode::OK, episode),
            "/3/search/tv" | "/3/trending/all/day" | "/3/discover/tv" => (StatusCode::OK, results),
            _ => (
                StatusCode::NOT_FOUND,
                json!({ "success": false, "status_code": 34 }),
            ),
        }
    }

    /// The server, with a series TMDB knows as 1396 locked here: its title
    /// in every language, its overview in German alone, its first season's
    /// name and its first episode's title in every language.
    async fn server(stand_in: &StandIn) -> AppState {
        let state = testing::server(stand_in).await;
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.slug = "breaking-bad".into();
        item.external_ids.tmdb = Some(1396);
        item.seasons = vec![Season {
            id: "s1".into(),
            season_number: 1,
            title: Some("Season 1".into()),
            overview: None,
            air_date: None,
            tmdb_id: Some(3572),
            tvdb_id: None,
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
        }];
        item.episodes = vec![Episode {
            id: "e1".into(),
            season_number: 1,
            episode_number: 1,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: "Pilot".into(),
            overview: None,
            air_date: None,
            air_date_utc: None,
            runtime: None,
            finale_type: None,
            image: None,
            tvdb_id: None,
            tmdb_id: Some(62085),
            rating: None,
            is_manual: false,
        }];
        testing::store(
            &state,
            &item,
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "overview", "deu", json!("Mein Überblick")),
                ("season:1", "title", "", json!("Première saison")),
                ("episode:1x1", "title", "", json!("Le pilote")),
            ],
        )
        .await;
        state
    }

    async fn get(state: &AppState, target: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
        let mut request = Request::builder().uri(target);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let answer = proxy(State(state.clone()), request.body(Body::empty()).unwrap())
            .await
            .expect("an answer");
        let status = answer.status();
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_operators_key_stands_in_and_every_document_takes_its_locks() {
        let stand_in = StandIn::start(tmdb).await;
        let state = server(&stand_in).await;
        assert_eq!(relay::policy(&state, Surface::Tmdb), SurfacePolicy::ApiKey);

        let (status, title) = get(&state, "/3/tv/1396?api_key=ams_issued&language=fr", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(title["name"], json!("Mon titre"));
        let asked = stand_in.asked_at("/3/tv/1396");
        assert!(
            asked[0]
                .query
                .contains(&format!("api_key={OPERATOR_TMDB_KEY}"))
        );
        assert!(!asked[0].query.contains("ams_issued"));

        // Kept, and patched again on each serve.
        let (_, again) = get(&state, "/3/tv/1396?language=fr&api_key=ams_issued", &[]).await;
        assert_eq!(again["name"], json!("Mon titre"));
        assert_eq!(stand_in.asked_at("/3/tv/1396").len(), 1);

        let (_, season) = get(&state, "/3/tv/1396/season/1?language=fr-FR", &[]).await;
        assert_eq!(season["name"], json!("Première saison"));
        assert_eq!(season["episodes"][0]["name"], json!("Le pilote"));
        let (_, episode) = get(&state, "/3/tv/1396/season/1/episode/1?language=fr", &[]).await;
        assert_eq!(episode["name"], json!("Le pilote"));
        for list in [
            "/3/search/tv?query=breaking&language=fr",
            "/3/trending/all/day?language=fr",
            "/3/discover/tv?language=fr",
        ] {
            let (status, page) = get(&state, list, &[]).await;
            assert_eq!(status, StatusCode::OK, "{list}");
            assert_eq!(page["results"][0]["name"], json!("Mon titre"), "{list}");
            assert_eq!(
                page["results"][1]["name"],
                json!("Bryan Cranston"),
                "{list}"
            );
        }

        // Asked in no language, TMDB answers in its own English: a text
        // locked in every language is the catalogue's French, and TMDB's
        // English stands.
        let (_, episode) = get(&state, "/3/tv/1396/season/1/episode/1", &[]).await;
        assert_eq!(episode["name"], json!("Pilot"));
        let (_, page) = get(&state, "/3/discover/tv", &[]).await;
        assert_eq!(page["results"][0]["name"], json!("Breaking Bad"));
    }

    #[tokio::test]
    async fn the_language_asked_takes_what_was_locked_for_it() {
        let stand_in = StandIn::start(tmdb).await;
        let state = server(&stand_in).await;

        // In German: the overview locked in German alone. The title locked
        // in every language is the catalogue's own text, written in its
        // language, French: TMDB's German one stands, as it did before a
        // lock had a language.
        let (_, german) = get(&state, "/3/tv/1396?language=de-DE", &[]).await;
        assert_eq!(german["name"], json!("Breaking Bad"));
        assert_eq!(german["overview"], json!("Mein Überblick"));
        let (_, page) = get(&state, "/3/search/tv?query=x&language=de", &[]).await;
        assert_eq!(page["results"][0]["name"], json!("Breaking Bad"));
        assert_eq!(page["results"][0]["overview"], json!("Mein Überblick"));

        // Nothing locked in Spanish: TMDB's own text, all of it.
        let (_, spanish) = get(&state, "/3/tv/1396?language=es-ES", &[]).await;
        assert_eq!(spanish["name"], json!("Breaking Bad"));
        assert_eq!(spanish["overview"], json!("TMDB's"));

        // In French, the catalogue's own: the title locked in every
        // language; the overview, locked in German alone, is TMDB's.
        let (_, french) = get(&state, "/3/tv/1396?language=fr-FR", &[]).await;
        assert_eq!(french["name"], json!("Mon titre"));
        assert_eq!(french["overview"], json!("TMDB's"));
    }

    /// TMDB's lists of *Fight Club*, as the stand-in answers them: each on
    /// its own, and appended to the film's document.
    fn fight_clubs_lists(asked: &Asked) -> (StatusCode, Value) {
        let credits = json!({ "id": 550,
            "cast": [ { "id": 819, "name": "Edward Norton" }, { "id": 287, "name": "Brad Pitt" } ],
            "crew": [ { "id": 7467, "name": "David Fincher", "job": "Director", "department": "Directing" } ] });
        let images = json!({ "id": 550,
            "posters": [ { "file_path": "/kept.jpg" }, { "file_path": "/hidden.jpg" } ],
            "backdrops": [], "logos": [] });
        let titles = json!({ "id": 550, "titles": [
            { "iso_3166_1": "MX", "title": "El club de la pelea" },
            { "iso_3166_1": "US", "title": "Fight Club – Hidden" } ] });
        match asked.path.as_str() {
            "/3/movie/550/credits" => (StatusCode::OK, credits),
            "/3/movie/550/images" => (StatusCode::OK, images),
            "/3/movie/550/alternative_titles" => (StatusCode::OK, titles),
            "/3/movie/550" => (
                StatusCode::OK,
                json!({ "id": 550, "title": "Fight Club",
                        "credits": credits, "images": images, "alternative_titles": titles }),
            ),
            _ => (StatusCode::NOT_FOUND, json!({ "status_code": 34 })),
        }
    }

    #[tokio::test]
    async fn what_a_person_hid_is_in_no_list_relayed() {
        let stand_in = StandIn::start(fight_clubs_lists).await;
        let state = testing::server(&stand_in).await;
        testing::store(
            &state,
            &super::tests::fight_club(),
            &super::tests::hidden_of_fight_club(),
        )
        .await;
        let ids = |list: &Value| -> Vec<i64> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["id"].as_i64().unwrap())
                .collect()
        };
        let paths = |list: &Value| -> Vec<String> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["file_path"].as_str().unwrap().to_string())
                .collect()
        };

        let (status, credits) = get(&state, "/3/movie/550/credits", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids(&credits["cast"]), [287]);
        assert_eq!(credits["crew"], json!([]));
        let (_, images) = get(&state, "/3/movie/550/images", &[]).await;
        assert_eq!(paths(&images["posters"]), ["/kept.jpg"]);
        let (_, titles) = get(&state, "/3/movie/550/alternative_titles", &[]).await;
        assert_eq!(titles["titles"].as_array().unwrap().len(), 1);
        assert_eq!(titles["titles"][0]["iso_3166_1"], json!("MX"));

        // Appended to the film's own document, in any language: the same.
        let (_, film) = get(
            &state,
            "/3/movie/550?append_to_response=credits,images,alternative_titles&language=de",
            &[],
        )
        .await;
        assert_eq!(ids(&film["credits"]["cast"]), [287]);
        assert_eq!(paths(&film["images"]["posters"]), ["/kept.jpg"]);
        assert_eq!(
            film["alternative_titles"]["titles"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn each_clients_own_key_travels_and_one_issued_here_is_refused() {
        let stand_in = StandIn::start(tmdb).await;
        let state = server(&stand_in).await;
        testing::choose(&state, "tmdb.relayCredential", "client").await;
        // A key is no use here any more: the address list guards the relay.
        assert_eq!(
            relay::policy(&state, Surface::Tmdb),
            SurfacePolicy::Allowlist
        );

        // A key issued here, wherever it is: refused, saying why, and TMDB
        // asked nothing.
        for (target, headers) in [
            ("/3/tv/1396?api_key=ams_issued", vec![]),
            ("/3/tv/1396", vec![("authorization", "Bearer ams_issued")]),
            ("/3/tv/1396", vec![("x-api-key", "ams_issued")]),
            // Its scheme in capitals, another scheme, or behind a token of
            // their own: found all the same, and kept here.
            ("/3/tv/1396", vec![("authorization", "BEARER ams_issued")]),
            ("/3/tv/1396", vec![("authorization", "Token ams_issued")]),
            (
                "/3/tv/1396",
                vec![("authorization", "Bearer their-token ams_issued")],
            ),
        ] {
            let (status, refusal) = get(&state, target, &headers).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{target}");
            assert_eq!(refusal["error"], json!("own_key_required"));
        }
        assert!(stand_in.asked_at("/3/tv/1396").is_empty());

        // Their own key travels as it came, the operator's never; the locks
        // are written in all the same.
        let (status, title) =
            get(&state, "/3/tv/1396?api_key=their-tmdb-key&language=fr", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(title["name"], json!("Mon titre"));
        let asked = stand_in.asked_at("/3/tv/1396");
        assert!(asked[0].query.contains("api_key=their-tmdb-key"));
        assert!(!asked[0].query.contains(OPERATOR_TMDB_KEY));

        // Their v4 token, in its header; and what it was answered is kept
        // for nobody.
        let (status, _) = get(
            &state,
            "/3/tv/1396?language=fr",
            &[("authorization", "Bearer their-token")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let asked = stand_in.asked_at("/3/tv/1396");
        assert_eq!(asked.len(), 2);
        assert_eq!(
            asked[1].authorization.as_deref(),
            Some("Bearer their-token")
        );

        // No key at all: TMDB's own refusal, handed back.
        let (status, refusal) = get(&state, "/3/tv/1396", &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(refusal["status_code"], json!(7));
    }
}
