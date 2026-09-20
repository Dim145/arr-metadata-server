//! Per-surface request guards.
//!
//! Each guard resolves the caller, applies its surface's policy, and inserts an
//! [`Identity`] into the request extensions for handlers to read.

use std::net::SocketAddr;

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
const API_KEY_PARAMS: &[&str] = &["api_key", "apikey"];

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

    let client_ip = ip::resolve(peer, request.headers(), &state.config.server.trusted_proxies);

    let identity = match state.config.policy_for(surface) {
        SurfacePolicy::Open => Identity::Anonymous,

        SurfacePolicy::Allowlist => {
            if !ip::is_allowed(client_ip, &state.config.security.arr_allowlist) {
                tracing::warn!(
                    ?client_ip,
                    path = %request.uri().path(),
                    "rejected: peer is not in the allowlist"
                );
                return Err(AppError::Forbidden);
            }
            Identity::Network
        }

        SurfacePolicy::ApiKey => {
            let presented = extract_key(request.headers(), request.uri().query());

            // An admin session is accepted wherever a key is, so the web UI can
            // call the same endpoints without minting a key for itself.
            match resolve_session(&state, request.headers()).await? {
                Some(identity) => identity,
                None => match presented {
                    Some(key) => resolve_key(&state, &key, client_ip).await?,
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

    if let Some(value) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(token) = value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer ")) {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }

    let query = query?;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
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

    if let Some(expires_at) = &found.client.expires_at {
        if expires_at.as_str() <= crate::db::now().as_str() {
            tracing::warn!(client = %found.client.name, "rejected: key has expired");
            return Err(AppError::Forbidden);
        }
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
        assert_eq!(extract_key(&h, Some("apikey=ams_abc")).as_deref(), Some("ams_abc"));
    }

    #[test]
    fn a_url_encoded_key_is_decoded() {
        let h = HeaderMap::new();
        assert_eq!(extract_key(&h, Some("api_key=ams%5Fabc")).as_deref(), Some("ams_abc"));
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
