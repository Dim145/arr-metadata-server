//! The catalogue, for agents: a Model Context Protocol server at `/mcp`.
//!
//! An agent — Claude Code, Open WebUI, anything that speaks MCP — asks a
//! server which tools it has and calls them. These read the catalogue and
//! nothing else: search it, read a work and its episodes, the schedule of a
//! week, the curated lists, where a work's values came from, how the server
//! is doing and what there is to do. None changes anything: tools that write
//! would come under a switch and a scope of their own.
//!
//! - **Off** until `mcp.enabled` is on: switched off, `/mcp` answers `503`
//!   with the code `mcp_disabled`, as a surface switched off does.
//! - **A key with the `mcp` scope** is the one credential taken, in
//!   `Authorization: Bearer` or `X-Api-Key`: never the session's cookie, which
//!   a page elsewhere could ride on, nor a key in the address, which a proxy's
//!   log keeps. A key given that scope alone opens this door and no other
//!   (`Identity::only_mcp`). What the tools read, they read as an
//!   administrator would — and only read.
//! - **Under the names it is given**: the public address's host, this
//!   machine's own, and those in `AMS_ALLOWED_HOSTS` — a page that rebinds a
//!   name of its own to this server is refused for its `Host` — and from no
//!   browser page but this server's own, by its `Origin`. Both are checked by
//!   the SDK, from 1.4 on, where they once were not (CVE-2026-42559).
//! - **No session is kept**: every request is answered on its own, so any of
//!   several instances answers it, and a restart loses nothing.
//! - Outside the timeout and the compression every other answer goes
//!   through, since an answer may be a stream; inside the native API's rate
//!   limit, by address; and in the trail, each call, as `mcp.called`.

mod facade;
mod server;
mod tools;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::{Next, from_fn, from_fn_with_state},
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use tokio_util::sync::CancellationToken;
use tower_http::{
    catch_panic::CatchPanicLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer,
};

use crate::{
    auth::{Identity, ip, middleware::ClientAddr},
    error::AppError,
    state::AppState,
};

/// The setting that lets agents in.
pub const SETTING: &str = "mcp.enabled";

/// The scope a key needs to call `/mcp`.
pub const SCOPE: &str = "mcp";

/// The largest request taken: a call is a tool's name and a few arguments,
/// a kilobyte at the most.
const LARGEST_REQUEST: usize = 64 * 1024;

/// The door's router, `rest`, with `/mcp` beside it rather than inside it —
/// beside its timeout and its compression — and routed rather than nested,
/// so that its trace and its count read `/mcp` whole.
pub fn mounted(rest: Router, state: &AppState, stop: CancellationToken) -> Router {
    let mcp = router(state, stop).into_service::<axum::body::Body>();
    Router::new()
        .route_service("/mcp", mcp.clone())
        .route_service("/mcp/", mcp)
        .fallback_service(rest)
}

/// The MCP server, as the door mounts it at `/mcp`: the SDK's service behind
/// the guard, the rate limit and the maintenance's, with the trace, the
/// request's id and the count every answer has — and neither the timeout nor
/// the compression. `stop` ends what it is answering when the server stops.
pub fn router(state: &AppState, stop: CancellationToken) -> Router {
    let catalogue = server::Catalogue::new(state.clone());
    let service = StreamableHttpService::new(
        move || Ok(catalogue.clone()),
        Arc::new(NeverSessionManager::default()),
        config(&state.config.server, stop),
    );

    let router = Router::new()
        .fallback_service(service)
        .layer(from_fn_with_state(state.clone(), guard))
        .layer(from_fn_with_state(
            state.clone(),
            crate::auth::ratelimit::limit,
        ))
        .layer(from_fn_with_state(
            state.clone(),
            crate::backup::maintenance::guard,
        ))
        .layer(CatchPanicLayer::new())
        // Nothing here is a page: an answer runs nothing, is framed by
        // nothing, and is the key's alone, so nothing between keeps it.
        .layer(fixed(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(fixed(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'; sandbox",
        ))
        .layer(fixed(header::REFERRER_POLICY, "no-referrer"))
        .layer(fixed(header::CACHE_CONTROL, "no-store"))
        .layer(TraceLayer::new_for_http().make_span_with(crate::telemetry::RequestSpan))
        .layer(from_fn(crate::telemetry::trace::serve));
    crate::telemetry::with_request_id(router, state.config.server.trusted_proxies.clone())
        .layer(from_fn(crate::metrics::observe))
}

/// One header, on every answer.
fn fixed(name: header::HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

/// How the SDK serves: statelessly — no session to keep, so several
/// instances answer alike — with plain JSON where an answer is one message,
/// under the names [`allowed_hosts`] gives, from the origins
/// [`allowed_origins`] gives, and stopped by `stop`.
fn config(server: &crate::config::Server, stop: CancellationToken) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts(server))
        .with_allowed_origins(allowed_origins(server))
        .enforce_origin_validation()
        .with_max_request_body_bytes(LARGEST_REQUEST)
        .with_cancellation_token(stop)
}

/// The names `/mcp` answers under: this machine's own, the public
/// address's, and the operator's (`AMS_ALLOWED_HOSTS`). A name a page
/// elsewhere rebinds to this server's address is none of these.
fn allowed_hosts(server: &crate::config::Server) -> Vec<String> {
    let mut hosts: Vec<String> = ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_string)
        .collect();
    if let Some(host) = server
        .public_url
        .as_deref()
        .and_then(|url| url::Url::parse(url).ok())
        .and_then(|url| url.host_str().map(str::to_string))
    {
        hosts.push(host);
    }
    hosts.extend(server.allowed_hosts.iter().cloned());
    let mut seen = std::collections::HashSet::new();
    hosts
        .into_iter()
        .map(|host| host.trim().trim_matches(['[', ']']).to_ascii_lowercase())
        .filter(|host| !host.is_empty() && seen.insert(host.clone()))
        .collect()
}

/// The browser pages that may call `/mcp`: this server's own, at its public
/// address — written with its port, the scheme's when it has none. A request
/// from any other page is refused; one from no page at all, as an agent's
/// is, carries no origin and is not asked for one.
fn allowed_origins(server: &crate::config::Server) -> Vec<String> {
    server
        .public_url
        .as_deref()
        .and_then(|url| url::Url::parse(url).ok())
        .and_then(|url| {
            let host = url.host_str()?;
            let port = url.port_or_known_default()?;
            Some(format!("{}://{host}:{port}", url.scheme()))
        })
        .into_iter()
        .collect()
}

/// Who may call: refused while MCP is off; else a key with the `mcp` scope,
/// in a header — and the key, its holder and their address are left on the
/// request for the tools.
async fn guard(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    if !state.flag(SETTING, false) {
        return AppError::Disabled {
            code: "mcp_disabled",
            message: "MCP is switched off on this server: an administrator turns it on under \
                      Keys & network"
                .into(),
        }
        .into_response();
    }

    let peer = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0);
    let client_ip = ip::caller(
        peer,
        request.headers(),
        &state.config.server.trusted_proxies,
    )
    .address();

    let Some(presented) = key_of(request.headers()) else {
        return AppError::Unauthorized.into_response();
    };
    let identity = match crate::auth::middleware::resolve_key(&state, &presented, client_ip).await {
        Ok(identity) => identity,
        Err(refusal) => return refusal.into_response(),
    };
    if !identity.can_use_mcp() {
        return AppError::Refused {
            code: "mcp_scope_missing",
            message: "this key does not open MCP: issue one with the mcp scope on Keys & network"
                .into(),
        }
        .into_response();
    }

    request.extensions_mut().insert::<Identity>(identity);
    if let Some(addr) = client_ip {
        request.extensions_mut().insert(ClientAddr(addr));
    }
    next.run(request).await
}

/// The key a request carries where an agent puts one: `Authorization:
/// Bearer`, or `X-Api-Key`. Not the address's query, nor a cookie.
fn key_of(headers: &HeaderMap) -> Option<String> {
    crate::api::relay::bearer(headers).or_else(|| {
        headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
    })
}

/// What a client looking for OAuth's metadata is told: there is none. The
/// MCP server takes keys; without this, the interface's pages answered the
/// address, and a client read a page where it expected JSON.
pub async fn no_oauth() -> Response {
    AppError::NotFound.into_response()
}
