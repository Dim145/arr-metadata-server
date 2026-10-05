//! TMDB compatibility: `/3/*`.
//!
//! This grew out of an earlier TMDB relay, with one addition that is the whole
//! point of this server: responses are **patched with local edits** before
//! they are returned.
//!
//! A request is relayed upstream with this server's own TMDB credentials
//! substituted for whatever the client sent. If the path addresses a title a
//! person locked fields on, those fields are written into the upstream
//! document on the way back — its text only for a client that asked in the
//! language the text was written in. The client sees TMDB's full response,
//! with the operator's corrections applied.

use std::time::{Duration, Instant};

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
    api::relay::{self, climbs},
    cache,
    domain::{CoverType, ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service::language,
    state::AppState,
};

/// Query parameters this server controls and the client does not.
///
/// Every spelling of a key the guard accepts has to be here too. A client may
/// authenticate with `?apikey=`, and for a while only `?api_key=` was stripped —
/// so its credential for *this* server was relayed to TMDB, and landed in
/// TMDB's access logs on every request.
const OVERRIDDEN_PARAMS: &[&str] = &["api_key", "apikey", "include_adult"];

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

/// Relay any TMDB v3 request, with this server's locks written in on the way
/// back.
///
/// The request goes upstream with this server's own credentials substituted for
/// whatever the client sent. If the path addresses a title a person locked
/// fields on, those fields are written into the upstream document before it
/// is returned — the text only when the client asked in the language it was
/// written in — so the client gets TMDB's full response with the operator's
/// corrections in it.
///
/// Every TMDB v3 path is accepted; see TMDB's own documentation for their
/// shapes. Only `/3/tv/{id}` and `/3/movie/{id}` are patched.
#[utoipa::path(
    get, path = "/3/{path}", tag = TAG,
    // Registered against axum as `/3/{*path}`; see `router` above.
    params(("path" = String, Path, description = "A TMDB v3 path, e.g. `tv/1396` or `search/movie`")),
    responses(
        (status = 200, description = "TMDB's response, with the fields locked here written in where any are"),
        (status = 401, description = "No valid credential was presented"),
        (status = 503, description = "This server has no TMDB API key configured"),
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

    let Some(api_key) = state.config.tmdb.api_key.clone() else {
        return Err(AppError::ProviderNotConfigured);
    };

    // A v4 path is answered only with a v4 read token; a v3 key is refused
    // there, and the relay says so rather than passing the refusal on.
    if v4 && !api_key.starts_with("eyJ") {
        return Err(AppError::ProviderNotConfigured);
    }

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

    // A v4 token authenticates by header; a v3 key travels in the query.
    let query_key = (!api_key.starts_with("eyJ")).then_some(api_key.as_str());
    let target = upstream_url(
        &state.config.tmdb.upstream,
        &parts.uri,
        query_key,
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
    // shows at once whatever the cache holds.
    let filed_as =
        may_keep(&parts.method, &parts.uri).then(|| relay_key(&state, &parts.uri, &parts.headers));
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
                send(&state, &parts, &target, &api_key, validators).await?;
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

    // Only TMDB's own document for a known title is worth inspecting;
    // images, configuration, refusals and errors pass through untouched.
    if status == StatusCode::OK
        && let Some(target) = patchable
        && relay::is_json(&headers)
        && let Ok(mut document) = serde_json::from_slice::<Value>(&bytes)
    {
        let in_its_language = asks_in(&parts.uri, &state.language(None, None));
        if patch(&state, &mut document, target, in_its_language).await? {
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

/// Ask TMDB with this server's credentials, and read what it answered.
async fn send(
    state: &AppState,
    parts: &Parts,
    target: &str,
    api_key: &str,
    validators: bool,
) -> AppResult<(StatusCode, HeaderMap, Bytes)> {
    let mut upstream = state
        .http
        .request(parts.method.clone(), target)
        .headers(forwarded_headers(&parts.headers, validators))
        // This name is one this server answers on: a resolver sending it
        // back here would otherwise have it call itself.
        .header(crate::providers::radarr::LOOP_HEADER, &state.instance);

    // A v4 token authenticates by header; the query parameter is ignored then.
    if api_key.starts_with("eyJ") {
        upstream = upstream.bearer_auth(api_key);
    }

    let started = Instant::now();
    // The error would name the address it was sent to, this server's own
    // key in its query; it is logged and answered, so what is kept of it
    // is its kind and what lay under it.
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
    // the address asked, the operator's key in its query.
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
/// is for the key the answer is kept under — and this server's own: its v3
/// key, when the query is where its key goes, and its word on adult titles.
fn upstream_url(base: &str, uri: &Uri, query_key: Option<&str>, include_adult: bool) -> String {
    let mut pairs: Vec<String> = uri
        .query()
        .map(|query| {
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                // The client's own TMDB key — or the key it used to
                // authenticate *here* — must never reach upstream.
                .filter(|pair| {
                    let name = url::form_urlencoded::parse(pair.as_bytes())
                        .next()
                        .map(|(name, _)| name.into_owned())
                        .unwrap_or_default();
                    !OVERRIDDEN_PARAMS
                        .iter()
                        .any(|ours| name.eq_ignore_ascii_case(ours))
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if let Some(key) = query_key {
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
        (status = 403, description = "Not a public list, or not a read"),
        (status = 503, description = "No TMDB v4 read token is configured"),
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
/// would name the request this server made, the operator's key in its
/// query.
fn answer_headers(headers: &HeaderMap) -> HeaderMap {
    let mut kept = relay::kept(headers);
    kept.remove(header::LOCATION);
    kept.remove(header::CONTENT_LOCATION);
    kept
}

/// Whether a client asked TMDB in the language the catalogue's text is held
/// in — the text a person locked was written in it: the `language` asked,
/// the last when there are several, as TMDB reads them, and TMDB's own
/// `en-US` when none is. A language is one whatever its region: `fr`,
/// `fr-FR` and `fr-CA` are French.
fn asks_in(uri: &Uri, catalogue: &str) -> bool {
    let asked = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .filter(|(name, _)| name == "language")
            .last()
            .map(|(_, value)| value.into_owned())
    });
    let asked = asked
        .as_deref()
        .map(str::trim)
        .filter(|asked| !asked.is_empty())
        .unwrap_or(TMDB_DEFAULT_LANGUAGE);
    let asked = language::normalize(asked);
    !asked.is_empty() && asked == language::normalize(catalogue)
}

/// A title this server may hold edits for.
#[derive(Debug, PartialEq, Eq)]
struct PatchTarget {
    kind: MediaKind,
    tmdb_id: i64,
}

/// Recognise `/3/tv/{id}` and `/3/movie/{id}` exactly.
///
/// Sub-resources (`/credits`, `/season/1`, …) are left alone: their documents do
/// not carry the fields an override addresses.
fn patch_target(path: &str) -> Option<PatchTarget> {
    let mut segments = path.trim_matches('/').split('/');

    if segments.next()? != "3" {
        return None;
    }

    let kind = match segments.next()? {
        "tv" => MediaKind::Series,
        "movie" => MediaKind::Movie,
        _ => return None,
    };

    let tmdb_id = segments.next()?.parse().ok()?;

    // Anything further means a sub-resource.
    if segments.next().is_some() {
        return None;
    }

    Some(PatchTarget { kind, tmdb_id })
}

/// Write this server's locks into TMDB's document. Whether anything changed.
async fn patch(
    state: &AppState,
    document: &mut Value,
    target: PatchTarget,
    in_its_language: bool,
) -> AppResult<bool> {
    let source = ExternalSource::tmdb_for(target.kind);
    let Some(item) = relay::locked_work(state, source, &target.tmdb_id.to_string()).await? else {
        return Ok(false);
    };
    let Value::Object(map) = document else {
        return Ok(false);
    };
    Ok(write_locked(
        map,
        &item,
        target.kind,
        in_its_language,
        |address| state.media.unlocalize(address),
    ))
}

/// The work's locks, in TMDB's names: only the fields a person locked, only
/// into those TMDB's document carries, and the text — the title, the
/// overview, the homepage, the genres' names — only for a client that asked
/// in the language it was written in (`in_its_language`): a German
/// reader is answered TMDB's German, not a lock written in French.
///
/// `unlocalize` gives the address a provider gave a picture this server
/// keeps a copy of: a TMDB client builds an image's address from TMDB's
/// path, so only one of TMDB's own can be named.
fn write_locked(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    kind: MediaKind,
    in_its_language: bool,
    unlocalize: impl Fn(&str) -> String,
) -> bool {
    let mut changed = false;
    let (name, original_name) = match kind {
        MediaKind::Series => ("name", "original_name"),
        MediaKind::Movie => ("title", "original_title"),
    };
    let text = |value: &Option<String>| value.clone().map(Value::String);

    if in_its_language {
        if relay::is_locked(item, "title") {
            changed |= put(map, name, Some(Value::String(item.title.clone())));
        }
        if relay::is_locked(item, "overview") {
            changed |= put(map, "overview", text(&item.overview));
        }
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
            Some("0123456789abcdef"),
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
            None,
            true,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/4/list/8136?page=1&include_adult=true"
        );
    }

    #[test]
    fn the_text_is_locked_in_the_language_of_the_catalogue() {
        assert!(asks_in(&uri("/3/movie/1?language=fr-FR"), "fr-FR"));
        assert!(asks_in(&uri("/3/movie/1?language=fr"), "fr-FR"));
        assert!(asks_in(&uri("/3/movie/1?language=fr-CA"), "fr-FR"));
        assert!(asks_in(&uri("/3/movie/1?language=en&language=fr"), "fr-FR"));
        // TMDB answers `en-US` when no language is asked.
        assert!(asks_in(&uri("/3/movie/1"), "en-US"));
        assert!(asks_in(&uri("/3/movie/1?language="), "en-US"));
        assert!(!asks_in(&uri("/3/movie/1"), "fr-FR"));

        assert!(!asks_in(&uri("/3/movie/1?language=de-DE"), "fr-FR"));
        assert!(!asks_in(
            &uri("/3/movie/1?language=fr&language=de"),
            "fr-FR"
        ));
        assert!(!asks_in(&uri("/3/movie/1?language=../../x"), "fr-FR"));
    }

    #[test]
    fn detail_paths_are_recognised() {
        assert_eq!(
            patch_target("/3/tv/1396"),
            Some(PatchTarget {
                kind: MediaKind::Series,
                tmdb_id: 1396
            })
        );
        assert_eq!(
            patch_target("/3/movie/329865"),
            Some(PatchTarget {
                kind: MediaKind::Movie,
                tmdb_id: 329865
            })
        );
        assert_eq!(
            patch_target("/3/tv/1396/"),
            Some(PatchTarget {
                kind: MediaKind::Series,
                tmdb_id: 1396
            })
        );
    }

    #[test]
    fn sub_resources_and_other_paths_are_left_alone() {
        assert_eq!(patch_target("/3/tv/1396/season/1"), None);
        assert_eq!(patch_target("/3/movie/329865/credits"), None);
        assert_eq!(patch_target("/3/search/tv"), None);
        assert_eq!(patch_target("/3/configuration"), None);
        assert_eq!(patch_target("/3/tv/not-a-number"), None);
        assert_eq!(patch_target("/3/tv"), None);
        assert_eq!(patch_target("/other/tv/1"), None);
    }

    fn as_is(address: &str) -> String {
        address.to_string()
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
            as_is
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
            as_is
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

    #[test]
    fn another_language_is_answered_tmdbs_text() {
        let mut document = series_document();
        let item = locked_series(&["title", "overview", "homepage", "genres", "firstAired"]);

        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            false,
            as_is
        ));

        let upstream = series_document();
        for field in ["name", "overview", "homepage", "genres"] {
            assert_eq!(document[field], upstream[field], "{field}");
        }
        // A date is one in every language.
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
            as_is
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
            as_is
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
            as_is
        ));
        assert_eq!(error.len(), 3);
    }

    #[test]
    fn a_chosen_picture_is_written_only_when_locked() {
        let mut item = locked_series(&[]);
        item.images.push(crate::domain::Image {
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
            origin
        ));
        assert_eq!(document["poster_path"], series_document()["poster_path"]);

        // Chosen by a person: written, in any language.
        item.locked_fields = vec!["item/primaryPoster".into()];
        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            false,
            origin
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
            as_is
        ));
        assert_eq!(document["poster_path"], series_document()["poster_path"]);
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
