//! Per-surface request guards.
//!
//! Each guard resolves the caller, applies its surface's policy, and inserts an
//! [`Identity`] into the request extensions for handlers to read.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{Request, State},
    http::{HeaderMap, header},
    middleware::Next,
    response::Response,
};

use crate::{
    auth::{Identity, ip, secrets},
    config::{Surface, SurfacePolicy},
    db::repo,
    error::{AppError, AppResult},
    state::AppState,
};

/// Cookie carrying an admin session token.
pub const SESSION_COOKIE: &str = "ams_session";

/// Header an API key may be presented in.
const API_KEY_HEADER: &str = "x-api-key";

/// Query parameters an API key may be presented in.
///
/// `api_key` is what TMDB clients already send: point one at this server and set
/// its "TMDB API key" to a key issued here, and it authenticates unchanged.
pub const API_KEY_PARAMS: &[&str] = &["api_key", "apikey"];

pub async fn guard_native(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    authorize(state, Surface::Native, request, next).await
}

pub async fn guard_tmdb(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    authorize(state, Surface::Tmdb, request, next).await
}

pub async fn guard_arr(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    authorize(state, Surface::Arr, request, next).await
}

async fn authorize(
    state: AppState,
    surface: Surface,
    mut request: Request,
    next: Next,
) -> AppResult<Response> {
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0);

    let client_ip = ip::resolve(
        peer,
        request.headers(),
        &state.config.server.trusted_proxies,
    );

    let identity = match state.config.policy_for(surface) {
        SurfacePolicy::Open => Identity::Anonymous,

        SurfacePolicy::Allowlist => {
            let rules = state.allowlist();
            let matched = ip::matching_rule(client_ip, &rules);
            let allowed = matched.is_some();

            // Recorded either way. A refusal is the only trace a client that
            // cannot reach this server leaves anywhere, and an operator needs
            // its address to do anything about it.
            note_caller(&state, client_ip, &request, surface, allowed);

            if !allowed {
                tracing::warn!(
                    ?client_ip,
                    path = %request.uri().path(),
                    "rejected: peer is not in the allowlist"
                );
                return Err(AppError::Forbidden);
            }

            Identity::Network(matched.map(str::to_string))
        }

        SurfacePolicy::ApiKey => {
            let presented = extract_key(request.headers(), request.uri().query());

            // An admin session is accepted wherever a key is, so the web UI can
            // call the same endpoints without minting a key for itself.
            match resolve_session(&state, request.headers()).await? {
                Some(identity) => identity,
                None => match presented {
                    Some(key) => resolve_key(&state, &key, client_ip).await?,
                    // Nobody said who they are. That is allowed only for the
                    // browse paths, and only when an operator asked for it.
                    None if surface == Surface::Native
                        && state.config.security.public_browse
                        && browsable(request.method(), request.uri().path()) =>
                    {
                        Identity::Visitor
                    }
                    None => return Err(AppError::Unauthorized),
                },
            }
        }
    };

    request.extensions_mut().insert(identity);
    if let Some(addr) = client_ip {
        request.extensions_mut().insert(ClientAddr(addr));
    }

    Ok(next.run(request).await)
}

/// How often the same address is written back to the callers table.
///
/// Sonarr refreshing a library is hundreds of reads a minute, and turning each
/// into a write would make the diagnostic cost more than the diagnosis. A
/// refusal is never throttled: the first one is the one somebody is looking for.
const SIGHTING_INTERVAL: Duration = Duration::from_secs(60);

static LAST_NOTED: LazyLock<Mutex<HashMap<(IpAddr, bool), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Record that somebody called a guarded surface, off the request path.
///
/// Spawned rather than awaited: a caller being served must not wait on a note
/// about them, and a database that is busy must not turn into a refusal.
fn note_caller(
    state: &AppState,
    client_ip: Option<IpAddr>,
    request: &Request,
    surface: Surface,
    allowed: bool,
) {
    let Some(ip) = client_ip else { return };

    if !due(ip, allowed) {
        return;
    }

    let user_agent = request
        .headers()
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(200).collect::<String>());

    let path = request.uri().path().to_string();
    let surface = format!("{surface:?}").to_lowercase();
    let state = state.clone();

    tokio::spawn(async move {
        // `getnameinfo` blocks on a network round trip, so it goes to a thread
        // that is allowed to. Only when the name is stale — which, for a
        // container that keeps its address, is about once an hour.
        let hostname = match state.resolver.cached(ip) {
            Some(name) => Some(name),
            None if state.resolver.is_stale(ip) => {
                let resolver = state.resolver.clone();
                tokio::task::spawn_blocking(move || resolver.resolve(ip))
                    .await
                    .ok()
                    .flatten()
            }
            None => None,
        };

        let sighting = repo::network::Sighting {
            ip,
            hostname: hostname.as_deref(),
            user_agent: user_agent.as_deref(),
            surface: &surface,
            path: &path,
            allowed,
        };

        if let Err(e) = repo::network::saw(&state.db, sighting).await {
            tracing::debug!(%ip, error = %e, "could not record the caller");
        }
    });
}

/// Whether enough time has passed to write this address down again.
///
/// Refusals are throttled separately from acceptances rather than not at all.
/// Not at all was the point — an operator fixing an allowlist wants the refusal
/// to appear the moment it happens — but it made every refused request a
/// database write, and the arr surface it guards is the one reachable without a
/// credential: a host that is not on the list could turn a flood of `GET /v1/…`
/// into a flood of UPSERTs and, on SQLite, hold the single write lock against
/// the clients that *are* allowed. Keyed on the pair, a first refusal is still
/// recorded at once even from an address that was just served.
fn due(ip: IpAddr, allowed: bool) -> bool {
    let Ok(mut last) = LAST_NOTED.lock() else {
        return false;
    };

    let now = Instant::now();
    let key = (ip, allowed);

    match last.get(&key) {
        Some(at) if now.duration_since(*at) < SIGHTING_INTERVAL => false,
        _ => {
            last.insert(key, now);

            // Bounded, because the keys come from whoever can reach the port.
            if last.len() > 4096 {
                last.retain(|_, at| now.duration_since(*at) < SIGHTING_INTERVAL);
            }

            true
        }
    }
}

/// Whether a request with no credential may be served to a visitor.
///
/// An allowlist rather than a denylist, so an endpoint added later is closed
/// until somebody decides it should not be. Everything here is a read of the
/// catalogue itself: what the work is, and what this server holds in total.
/// Operational detail — settings, jobs, the audit trail, raw provider payloads,
/// which fields a person has overridden — stays behind a credential.
fn browsable(method: &axum::http::Method, path: &str) -> bool {
    if method != axum::http::Method::GET {
        return false;
    }

    match path.trim_end_matches('/') {
        "/api/v1/items" | "/api/v1/stats" | "/api/v1/sources" | "/api/v1/facets"
        | "/api/v1/calendar" | "/api/v1/auth/me" => true,
        // The feeds: the schedule again, and the arrivals, in shapes a
        // calendar app or a feed reader takes.
        "/api/v1/calendar.ics" | "/api/v1/feed/added.atom" | "/api/v1/feed/airing.atom" => true,
        // The curated lists, and the shapes the clients import them in. A
        // private list is refused inside, as a hidden work is.
        "/api/v1/lists" | "/api/v1/collections" => true,
        rest if rest
            .strip_prefix("/api/v1/collections/")
            .is_some_and(|tail| {
                !tail.is_empty() && tail.len() <= 18 && tail.bytes().all(|b| b.is_ascii_digit())
            }) =>
        {
            true
        }
        rest if rest.strip_prefix("/api/v1/lists/").is_some_and(|tail| {
            match tail.split('/').collect::<Vec<_>>().as_slice() {
                [key] => !key.is_empty(),
                [key, shape] => {
                    !key.is_empty()
                        && matches!(*shape, "sonarr.json" | "radarr.json" | "stevenlu.json")
                }
                _ => false,
            }
        }) =>
        {
            true
        }
        // Somebody's work, as the catalogue holds it.
        rest if rest
            .strip_prefix("/api/v1/people/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/')) =>
        {
            true
        }
        // A season's chart. What else premieres in it, one segment further,
        // is TMDB's list for whoever maintains the catalogue, and is not.
        rest if rest.strip_prefix("/api/v1/seasons/").is_some_and(|tail| {
            let mut parts = tail.split('/');
            matches!(
                (parts.next(), parts.next(), parts.next()),
                (Some(year), Some(season), None) if !year.is_empty() && !season.is_empty()
            )
        }) =>
        {
            true
        }
        // One work. Its seasons, episodes, artwork and cast come with it, and
        // its calendar is the same dates again; its snapshots and overrides
        // are their own paths and are not listed.
        rest => rest.strip_prefix("/api/v1/items/").is_some_and(|tail| {
            let id = tail
                .strip_suffix("/calendar.ics")
                .or_else(|| tail.strip_suffix("/lists"))
                .or_else(|| tail.strip_suffix("/watch"))
                .or_else(|| tail.strip_suffix("/similar"))
                .unwrap_or(tail);
            !id.is_empty() && !id.contains('/')
        }),
    }
}

/// Refuse a request this server sent to itself.
///
/// `skyhook.sonarr.tv` and `api.radarr.video` are hostnames this server both
/// impersonates *and*, when enrichment is on, calls. Redirecting them per
/// container is fine; doing it at the resolver means this server resolves the
/// same name and calls itself, and nothing about the resulting request looks
/// unusual. Outbound calls carry this process's id, so the loop is visible here.
pub async fn reject_self_calls(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    let caller = request
        .headers()
        .get(crate::providers::radarr::LOOP_HEADER)
        .and_then(|v| v.to_str().ok());

    if caller == Some(state.instance.as_str()) {
        tracing::error!(
            path = %request.uri().path(),
            "this server resolved an upstream provider to itself; set the upstream to \
             somewhere else, or turn enrichment off"
        );
        return Err(AppError::LoopDetected);
    }

    Ok(next.run(request).await)
}

/// The resolved caller address, for handlers that log or audit.
#[derive(Clone, Copy, Debug)]
pub struct ClientAddr(pub std::net::IpAddr);

/// Find an API key in the places clients are able to put one.
fn extract_key(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(value) = headers.get(API_KEY_HEADER).and_then(|v| v.to_str().ok()) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        && let Some(token) = value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
    {
        let token = token.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }

    let query = query?;
    for pair in query.split('&') {
        // `continue`, not `?`: returning here would abandon the whole query on
        // the first bare flag, so `?adult&api_key=…` answered 401 for a request
        // that carried a perfectly good key.
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if API_KEY_PARAMS.contains(&name) {
            let decoded = urlencoding::decode(value).ok()?;
            let trimmed = decoded.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

async fn resolve_key(
    state: &AppState,
    presented: &str,
    client_ip: Option<std::net::IpAddr>,
) -> AppResult<Identity> {
    let hash = secrets::hash_api_key(presented);

    let Some(found) = repo::client::find_by_key_hash(&state.db, &hash).await? else {
        return Err(AppError::Unauthorized);
    };

    // The lookup already matched on hash; this guards against a timing signal
    // leaking through any future change to how the row is selected.
    if !secrets::api_key_hash_matches(&hash, &found.key_hash) {
        return Err(AppError::Unauthorized);
    }

    if !found.client.is_enabled {
        tracing::warn!(client = %found.client.name, "rejected: client is disabled");
        return Err(AppError::Forbidden);
    }

    if let Some(expires_at) = &found.client.expires_at
        && expires_at.as_str() <= crate::db::now().as_str()
    {
        tracing::warn!(client = %found.client.name, "rejected: key has expired");
        return Err(AppError::Forbidden);
    }

    let ip_text = client_ip.map(|ip| ip.to_string());
    if let Err(e) = repo::client::touch(&state.db, &found.client.id, ip_text.as_deref()).await {
        // Bookkeeping must never fail a request that was otherwise authorised.
        tracing::warn!(error = %e, "could not record key usage");
    }

    Ok(Identity::Client(Box::new(found.client)))
}

async fn resolve_session(state: &AppState, headers: &HeaderMap) -> AppResult<Option<Identity>> {
    let Some(token) = session_token(headers) else {
        return Ok(None);
    };

    let hash = secrets::hash_api_key(&token);

    Ok(repo::user::find_session_user(&state.db, &hash)
        .await?
        .map(|user| Identity::Admin(Box::new(user))))
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod browse_tests {
    use super::browsable;
    use axum::http::Method;

    fn allowed(path: &str) -> bool {
        browsable(&Method::GET, path)
    }

    #[test]
    fn a_visitor_reads_the_catalogue() {
        assert!(allowed("/api/v1/items"));
        assert!(allowed(
            "/api/v1/items/01a0c0e3-4c5d-7738-a2cf-4972b0bc2dcc"
        ));
        assert!(allowed("/api/v1/stats"));
        assert!(allowed("/api/v1/auth/me"));
        // The credits every page carries, which some sources' licences require.
        assert!(allowed("/api/v1/sources"));
        // The catalogue from other sides: what it can be narrowed by, what
        // airs when, and who is in it.
        assert!(allowed("/api/v1/facets"));
        assert!(allowed("/api/v1/calendar"));
        assert!(allowed("/api/v1/people/17419"));
        assert!(!allowed("/api/v1/people/17419/secret"));
        assert!(allowed("/api/v1/seasons/2026/autumn"));
        assert!(!allowed("/api/v1/seasons/2026/autumn/candidates"));
        assert!(!allowed("/api/v1/seasons/2026"));
        assert!(!allowed("/api/v1/seasons//autumn"));
    }

    #[test]
    fn a_visitor_does_not_read_how_the_server_is_run() {
        // Every one of these would tell a stranger something about the
        // deployment rather than about the films.
        assert!(!allowed("/api/v1/settings"));
        assert!(!allowed("/api/v1/jobs"));
        assert!(!allowed("/api/v1/audit"));
        assert!(!allowed("/api/v1/clients"));
        assert!(!allowed("/api/v1/fields"));
    }

    #[test]
    fn a_works_own_sub_paths_stay_closed() {
        // Snapshots are raw provider payloads and overrides say who edited
        // what. Both hang off an item id, so the rule has to be narrower than
        // "anything under /items/".
        let id = "01a0c0e3-4c5d-7738-a2cf-4972b0bc2dcc";

        assert!(!allowed(&format!("/api/v1/items/{id}/snapshots")));
        assert!(!allowed(&format!("/api/v1/items/{id}/overrides")));
        assert!(!allowed(&format!("/api/v1/items/{id}/nfo")));
        assert!(allowed(&format!("/api/v1/items/{id}/calendar.ics")));
        assert!(!allowed(&format!("/api/v1/items/{id}/calendar.ics/x")));
        assert!(!allowed("/api/v1/items//calendar.ics"));
        assert!(allowed("/api/v1/calendar.ics"));
        assert!(allowed("/api/v1/feed/added.atom"));
        assert!(allowed("/api/v1/feed/airing.atom"));
        assert!(!allowed("/api/v1/feed/other.atom"));
        assert!(allowed("/api/v1/lists"));
        assert!(allowed("/api/v1/lists/autumn-2026"));
        assert!(allowed("/api/v1/lists/autumn-2026/sonarr.json"));
        assert!(allowed("/api/v1/lists/autumn-2026/radarr.json"));
        assert!(!allowed("/api/v1/lists/autumn-2026/items"));
        assert!(!allowed("/api/v1/lists//sonarr.json"));
        assert!(allowed(&format!("/api/v1/items/{id}/lists")));
        assert!(allowed(&format!("/api/v1/items/{id}/watch")));
        assert!(allowed(&format!("/api/v1/items/{id}/similar")));
        assert!(!allowed(&format!("/api/v1/items/{id}/suggestions")));
        assert!(allowed("/api/v1/collections"));
        assert!(allowed("/api/v1/collections/10"));
        assert!(!allowed("/api/v1/collections/ten"));
        assert!(!allowed("/api/v1/collections/10/x"));
        assert!(!allowed("/api/v1/collections/99999999999999999999"));
        assert!(!allowed(&format!("/api/v1/items/{id}/watch/x")));
    }

    #[test]
    fn nothing_that_changes_anything_is_browsable() {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(!browsable(&method, "/api/v1/items"), "{method} /items");
        }
    }

    #[test]
    fn a_trailing_slash_does_not_open_a_door() {
        assert!(allowed("/api/v1/items/"));
        assert!(!allowed("/api/v1/settings/"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn a_key_is_found_in_the_dedicated_header() {
        let h = headers(&[("x-api-key", "ams_abc")]);
        assert_eq!(extract_key(&h, None).as_deref(), Some("ams_abc"));
    }

    #[test]
    fn a_key_is_found_in_a_bearer_token() {
        let h = headers(&[("authorization", "Bearer ams_abc")]);
        assert_eq!(extract_key(&h, None).as_deref(), Some("ams_abc"));
    }

    #[test]
    fn a_key_is_found_in_the_query_string() {
        // This is the form a TMDB client sends without any modification.
        let h = HeaderMap::new();
        assert_eq!(
            extract_key(&h, Some("language=en&api_key=ams_abc")).as_deref(),
            Some("ams_abc")
        );
        assert_eq!(
            extract_key(&h, Some("apikey=ams_abc")).as_deref(),
            Some("ams_abc")
        );
    }

    #[test]
    fn a_url_encoded_key_is_decoded() {
        let h = HeaderMap::new();
        assert_eq!(
            extract_key(&h, Some("api_key=ams%5Fabc")).as_deref(),
            Some("ams_abc")
        );
    }

    #[test]
    fn an_empty_or_absent_key_is_not_accepted() {
        assert_eq!(extract_key(&HeaderMap::new(), None), None);
        assert_eq!(extract_key(&headers(&[("x-api-key", "  ")]), None), None);
        assert_eq!(extract_key(&HeaderMap::new(), Some("api_key=")), None);
        assert_eq!(extract_key(&HeaderMap::new(), Some("other=x")), None);
    }

    #[test]
    fn the_header_wins_over_the_query_string() {
        let h = headers(&[("x-api-key", "from-header")]);
        assert_eq!(
            extract_key(&h, Some("api_key=from-query")).as_deref(),
            Some("from-header")
        );
    }

    #[test]
    fn the_session_cookie_is_picked_out_of_a_crowd() {
        let h = headers(&[("cookie", "theme=dark; ams_session=tok123; other=x")]);
        assert_eq!(session_token(&h).as_deref(), Some("tok123"));
    }

    #[test]
    fn a_missing_session_cookie_yields_nothing() {
        assert_eq!(session_token(&HeaderMap::new()), None);
        assert_eq!(session_token(&headers(&[("cookie", "theme=dark")])), None);
        assert_eq!(session_token(&headers(&[("cookie", "ams_session=")])), None);
    }

    #[test]
    fn session_cookies_split_across_headers_are_still_found() {
        let h = headers(&[("cookie", "theme=dark"), ("cookie", "ams_session=tok123")]);
        assert_eq!(session_token(&h).as_deref(), Some("tok123"));
    }
}
