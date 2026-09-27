//! TMDB compatibility: `/3/*`.
//!
//! This grew out of an earlier TMDB relay, with one addition that is the whole
//! point of this server: responses are **patched with local edits** before
//! they are returned.
//!
//! A request is relayed upstream with this server's own TMDB credentials
//! substituted for whatever the client sent. If the path addresses a title that
//! has manual overrides stored here, those fields are rewritten in the upstream
//! document on the way back. The client sees TMDB's full response, with the
//! operator's corrections applied.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use utoipa_axum::router::OpenApiRouter;

use crate::{
    cache,
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus the ones the HTTP stack recomputes.
const HOP_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "content-encoding",
    // What the client would accept is not what the relay's client accepts:
    // reqwest asks for what it can decode, and what is kept is decoded.
    "accept-encoding",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Query parameters this server controls and the client does not.
///
/// Every spelling of a key the guard accepts has to be here too. A client may
/// authenticate with `?apikey=`, and for a while only `?api_key=` was stripped —
/// so its credential for *this* server was relayed to TMDB, and landed in
/// TMDB's access logs on every request.
const OVERRIDDEN_PARAMS: &[&str] = &["api_key", "apikey", "include_adult"];

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

/// Relay any TMDB v3 request, with this server's edits applied on the way back.
///
/// The request goes upstream with this server's own credentials substituted for
/// whatever the client sent. If the path addresses a title that has manual
/// overrides stored here, those fields are rewritten in the upstream document
/// before it is returned — so the client gets TMDB's full response with the
/// operator's corrections in it.
///
/// Every TMDB v3 path is accepted; see TMDB's own documentation for their
/// shapes. Only `/3/tv/{id}` and `/3/movie/{id}` are patched.
#[utoipa::path(
    get, path = "/3/{path}", tag = TAG,
    // Registered against axum as `/3/{*path}`; see `router` above.
    params(("path" = String, Path, description = "A TMDB v3 path, e.g. `tv/1396` or `search/movie`")),
    responses(
        (status = 200, description = "TMDB's response, with local edits applied where any exist"),
        (status = 401, description = "No valid credential was presented"),
        (status = 503, description = "This server has no TMDB API key configured"),
    ),
)]
pub async fn proxy(State(state): State<AppState>, request: Request) -> AppResult<Response> {
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

    let (parts, body) = request.into_parts();

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

    let target = upstream_url(&state, &parts.uri, &api_key);

    // The parser that builds the outgoing request has the last word on where
    // it goes — a backslash is a slash to it, and there may be spellings
    // `climbs` has not met — so the path it reads must be the path checked.
    if !url::Url::parse(&target).is_ok_and(|url| url.path().ends_with(parts.uri.path())) {
        return Err(AppError::BadRequest("that is not a TMDB path".into()));
    }

    // What TMDB answered last time, while it is still good: a document is
    // keyed by the path and query asked, without the credential, under the
    // generation the settings are on — and patched with the local overrides
    // on each serve, so an edit shows at once whatever the cache holds.
    let relay_key = (parts.method == Method::GET).then(|| relay_key(&state, &parts.uri));
    let remembered = match &relay_key {
        Some(key) => state
            .caches
            .relay
            .get(key)
            .await
            .filter(cache::Relayed::is_fresh),
        None => None,
    };

    let (status, headers, bytes) = if let Some(hit) = remembered {
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(&hit.content_type) {
            headers.insert(header::CONTENT_TYPE, value);
        }
        (
            StatusCode::from_u16(hit.status).unwrap_or(StatusCode::OK),
            headers,
            hit.body,
        )
    } else {
        let body_bytes = axum::body::to_bytes(body, 2 * 1024 * 1024)
            .await
            .map_err(|e| AppError::BadRequest(format!("could not read the request body: {e}")))?;

        let mut upstream =
            state
                .http
                .request(parts.method.clone(), &target)
                .headers(forwarded_headers(
                    &parts.headers,
                    patch_target(parts.uri.path()).is_some(),
                ));

        // A v4 token authenticates by header; the query parameter is ignored then.
        if api_key.starts_with("eyJ") {
            upstream = upstream.bearer_auth(&api_key);
        }

        if !body_bytes.is_empty() {
            upstream = upstream.body(body_bytes.to_vec());
        }

        let started = std::time::Instant::now();
        // The error would name the address it was sent to, this server's own
        // key in its query; it is logged and answered, so the address is
        // taken off it.
        let response = upstream.send().await.map_err(reqwest::Error::without_url);
        crate::metrics::upstream("tmdb", started, response.as_ref().ok().map(|r| r.status()));
        let response = response.map_err(|e| AppError::UpstreamUnavailable(e.into()))?;

        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let headers = response.headers().clone();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| AppError::UpstreamUnavailable(e.into()))?;

        // Kept as TMDB gave it: a document for as long as its kind stays
        // good, a "no such thing" a few minutes, an error not at all.
        if let Some(key) = relay_key
            && (status.is_success() || status == StatusCode::NOT_FOUND)
            && bytes.len() <= RELAY_MAX_BYTES
        {
            let ttl = if status == StatusCode::NOT_FOUND {
                std::time::Duration::from_secs(5 * 60)
            } else {
                cache::relay_ttl(parts.uri.path())
            };
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
        (status, headers, bytes)
    };

    // Only JSON documents for a known title are worth inspecting; images,
    // configuration and errors pass through untouched.
    if let Some(target) = patch_target(parts.uri.path())
        && is_json(&headers)
        && let Ok(mut document) = serde_json::from_slice::<Value>(&bytes)
        && patch(&state, &mut document, target).await?
    {
        // TMDB's validators described TMDB's bytes; the patched document is
        // tagged by this server's own layer, and kept as long as it says.
        let mut patched = response_headers(&headers);
        for name in [
            header::ETAG,
            header::LAST_MODIFIED,
            header::CACHE_CONTROL,
            header::EXPIRES,
        ] {
            patched.remove(name);
        }
        return Ok((status, patched, axum::Json(document)).into_response());
    }

    Ok((status, response_headers(&headers), Body::from(bytes)).into_response())
}

/// The largest document the relay keeps; TMDB's biggest run to a few
/// hundred kilobytes.
const RELAY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// What a relayed document is filed under: the path and the query asked,
/// the parameters this server sets left out, in a fixed order — and the
/// generation, so a settings change files what follows elsewhere.
fn relay_key(state: &AppState, uri: &Uri) -> String {
    let mut pairs: Vec<(String, String)> = uri
        .query()
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| !OVERRIDDEN_PARAMS.contains(&name.as_ref()))
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    format!(
        "{}:{}:{}:{}?{query}",
        state.caches.generation(),
        state.config.tmdb.upstream,
        state.config.tmdb.include_adult,
        uri.path()
    )
}

/// Rebuild the upstream URL, replacing the parameters this server controls.
fn upstream_url(state: &AppState, uri: &Uri, api_key: &str) -> String {
    let mut pairs: Vec<(String, String)> = uri
        .query()
        .map(|q| {
            q.split('&')
                .filter(|p| !p.is_empty())
                .map(|pair| match pair.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => (pair.to_string(), String::new()),
                })
                // The client's own TMDB key — or the key it used to authenticate
                // *here* — must never reach upstream.
                .filter(|(k, _)| !OVERRIDDEN_PARAMS.contains(&k.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if !api_key.starts_with("eyJ") {
        pairs.push(("api_key".to_string(), api_key.to_string()));
    }

    pairs.push((
        "include_adult".to_string(),
        state.config.tmdb.include_adult.to_string(),
    ));

    let query = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    format!("{}{}?{}", state.config.tmdb.upstream, uri.path(), query)
}

/// Headers that are a credential for *this* server and must not be relayed.
///
/// The cookie is the one that matters most: the interface is same-origin with
/// `/3/*` and the session cookie is `Path=/`, so anything a browser sends here —
/// an `<img src="/3/...">`, a bookmark — carries a live admin session, and
/// forwarding it would hand that session to TMDB's logs. `SameSite` does not
/// help; the request is same-site.
const OUR_CREDENTIALS: &[&str] = &["x-api-key", "cookie"];

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

/// Whether any segment of `path` could be read as leaving its place: a dot
/// segment, encoded or not, or a separator hidden inside a segment.
///
/// Percent-decoded first, because `%2e%2e` and `..` mean the same thing to the
/// URL parser that builds the outgoing request and different things to a naive
/// comparison. To that parser a backslash is a slash, so `list/..\account`
/// is `account`; and `%2F` survives it, but what TMDB's own edge makes of
/// `..%2F` is not this server's to find out. An un-decodable escape is
/// treated as suspicious rather than harmless — nothing TMDB addresses needs
/// one.
fn climbs(path: &str) -> bool {
    path.split('/').any(|segment| {
        match urlencoding::decode(segment) {
            Ok(decoded) => matches!(decoded.as_ref(), "." | "..") || decoded.contains(['/', '\\']),
            // Not valid UTF-8 once decoded: not a TMDB path either.
            Err(_) => true,
        }
    })
}

/// The caller's headers as TMDB is asked with them. For a document this
/// server patches, the caller's validators stay here: TMDB would answer
/// "unchanged" for a body it never saw the final shape of.
fn forwarded_headers(headers: &HeaderMap, patchable: bool) -> HeaderMap {
    headers
        .iter()
        .filter(|(name, _)| !HOP_HEADERS.contains(&name.as_str()))
        .filter(|(name, _)| {
            !(patchable && matches!(*name, &header::IF_NONE_MATCH | &header::IF_MODIFIED_SINCE))
        })
        // The client's Authorization is its credential for *this* server, not
        // for TMDB; forwarding it would leak it upstream.
        .filter(|(name, _)| *name != header::AUTHORIZATION)
        .filter(|(name, _)| !OUR_CREDENTIALS.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();

    for (name, value) in headers {
        if HOP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        // A `Set-Cookie` from upstream would be written against *this* origin,
        // where the session cookie lives. Nothing TMDB sets belongs here.
        if name == header::SET_COOKIE {
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

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
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

/// Apply this server's overrides to an upstream document.
///
/// Returns whether anything changed.
async fn patch(state: &AppState, document: &mut Value, target: PatchTarget) -> AppResult<bool> {
    let source = ExternalSource::tmdb_for(target.kind);

    let Some(id) =
        repo::item::find_id_by_external(&state.db, source, &target.tmdb_id.to_string()).await?
    else {
        return Ok(false);
    };

    // No overrides means the upstream document is already what we would serve.
    if repo::override_field::list(&state.db, &id).await?.is_empty() {
        return Ok(false);
    }

    let Some(item) = service::load(state, &id).await? else {
        return Ok(false);
    };

    let Value::Object(map) = document else {
        return Ok(false);
    };

    let mut changed = false;
    for (key, value) in tmdb_fields(&item, target.kind) {
        map.insert(key.to_string(), value);
        changed = true;
    }

    Ok(changed)
}

/// The canonical item's fields, named and shaped as TMDB names and shapes them.
fn tmdb_fields(item: &MediaItem, kind: MediaKind) -> Vec<(&'static str, Value)> {
    let mut out: Vec<(&'static str, Value)> = Vec::new();

    push_text(&mut out, "overview", &item.overview);
    push_text(&mut out, "homepage", &item.homepage);

    match kind {
        MediaKind::Series => {
            out.push(("name", Value::String(item.title.clone())));
            push_text(&mut out, "original_name", &item.original_title);
            push_text(&mut out, "first_air_date", &item.first_aired);
            push_text(&mut out, "last_air_date", &item.last_aired);

            if let Some(status) = item.status.as_deref() {
                out.push((
                    "status",
                    Value::String(tmdb_series_status(status).to_string()),
                ));
            }
            if let Some(runtime) = item.runtime {
                out.push(("episode_run_time", json!([runtime])));
            }
        }
        MediaKind::Movie => {
            out.push(("title", Value::String(item.title.clone())));
            push_text(&mut out, "original_title", &item.original_title);
            push_text(&mut out, "release_date", &item.in_cinemas);

            if let Some(runtime) = item.runtime {
                out.push(("runtime", json!(runtime)));
            }
        }
    }

    // TMDB's genres are objects; ours are names. The id is not something we can
    // invent, and no client keys on it here.
    if !item.genres.is_empty() {
        let genres: Vec<Value> = item
            .genres
            .iter()
            .map(|name| json!({ "id": Value::Null, "name": name }))
            .collect();
        out.push(("genres", Value::Array(genres)));
    }

    out
}

fn push_text(out: &mut Vec<(&'static str, Value)>, key: &'static str, value: &Option<String>) {
    if let Some(v) = value {
        out.push((key, Value::String(v.clone())));
    }
}

/// Canonical status back to TMDB's vocabulary.
fn tmdb_series_status(status: &str) -> &'static str {
    match status {
        "ended" => "Ended",
        "upcoming" => "Planned",
        _ => "Returning Series",
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

        assert!(OUR_CREDENTIALS.contains(&"cookie"));
        assert!(OUR_CREDENTIALS.contains(&"x-api-key"));
    }

    use super::*;

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

    #[test]
    fn series_fields_use_tmdbs_names() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.overview = Some("Mon résumé".into());
        item.first_aired = Some("2008-01-20".into());
        item.status = Some("ended".into());
        item.runtime = Some(47);

        let fields: std::collections::HashMap<_, _> =
            tmdb_fields(&item, MediaKind::Series).into_iter().collect();

        assert_eq!(fields["name"], json!("Mon titre"));
        assert_eq!(fields["overview"], json!("Mon résumé"));
        assert_eq!(fields["first_air_date"], json!("2008-01-20"));
        assert_eq!(fields["status"], json!("Ended"));
        assert_eq!(fields["episode_run_time"], json!([47]));
        assert!(!fields.contains_key("title"));
    }

    #[test]
    fn movie_fields_use_tmdbs_names() {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Premier Contact".into();
        item.in_cinemas = Some("2016-11-11".into());
        item.runtime = Some(116);

        let fields: std::collections::HashMap<_, _> =
            tmdb_fields(&item, MediaKind::Movie).into_iter().collect();

        assert_eq!(fields["title"], json!("Premier Contact"));
        assert_eq!(fields["release_date"], json!("2016-11-11"));
        assert_eq!(fields["runtime"], json!(116));
        assert!(!fields.contains_key("name"));
    }

    #[test]
    fn genres_are_reshaped_into_tmdbs_objects() {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.genres = vec!["Drame".into()];

        let fields: std::collections::HashMap<_, _> =
            tmdb_fields(&item, MediaKind::Movie).into_iter().collect();

        assert_eq!(fields["genres"], json!([{ "id": null, "name": "Drame" }]));
    }

    #[test]
    fn statuses_round_trip_back_to_tmdbs_vocabulary() {
        assert_eq!(tmdb_series_status("ended"), "Ended");
        assert_eq!(tmdb_series_status("continuing"), "Returning Series");
        assert_eq!(tmdb_series_status("upcoming"), "Planned");
    }
}
