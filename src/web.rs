//! HTTP server: router assembly, middleware stack, TLS, graceful shutdown.

use anyhow::{Context, Result};
use axum::{
    Json, Router, ServiceExt,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tower::Layer;
use utoipa_swagger_ui::SwaggerUi;

use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, cors::CorsLayer,
    limit::RequestBodyLimitLayer, normalize_path::NormalizePathLayer,
    set_header::SetResponseHeaderLayer, timeout::TimeoutLayer, trace::TraceLayer,
};

use crate::{api, state::AppState};

/// Largest request body accepted anywhere. The only sizeable one is a bulk
/// movie lookup, which is a list of integers.
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub async fn serve(state: AppState) -> Result<()> {
    let bind = state.config.server.bind;

    // Always started: whether it sweeps is a setting it re-reads, so turning
    // refresh on no longer needs a restart.
    tokio::spawn(crate::jobs::refresh::run(state.clone()));

    // Sonarr builds its URLs from `.../v1/tvdb/{route}/{language}/` — with a
    // trailing slash — and its hostname is compiled in, so there is no way to
    // ask it not to. `NormalizePathLayer` applied with `Router::layer` runs
    // *after* routing and so cannot help; it has to wrap the whole router as a
    // service, before a path is ever matched.
    let router = NormalizePathLayer::trim_trailing_slash().layer(build_router(state.clone()));

    match state.config.server.tls.clone() {
        Some(tls) => {
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
                .await
                .with_context(|| {
                    format!(
                        "could not load the TLS certificate {} / key {}",
                        tls.cert.display(),
                        tls.key.display()
                    )
                })?;

            tracing::info!(%bind, "listening (TLS)");

            axum_server::bind_rustls(bind, config)
                .serve(
                    ServiceExt::<Request>::into_make_service_with_connect_info::<
                        std::net::SocketAddr,
                    >(router),
                )
                .await
                .context("server error")?;
        }
        None => {
            tracing::info!(%bind, "listening");

            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .with_context(|| format!("cannot bind {bind}"))?;

            axum::serve(
                listener,
                ServiceExt::<Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(
                    router,
                ),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("server error")?;
        }
    }

    state.db.close().await;
    Ok(())
}

fn build_router(state: AppState) -> Router {
    let timeout = state.config.server.request_timeout;

    // Each surface carries its own authentication policy, applied inside; the
    // spec is collected from the same handlers, so it cannot drift from them.
    let (surfaces, openapi) = api::build(state.clone());

    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .merge(surfaces)
        // Documentation sits behind the native guard: it is an administrative
        // view of the server, and the UI reaches it with its session cookie.
        .merge(
            Router::<AppState>::from(SwaggerUi::new("/api/docs").url("/api/openapi.json", openapi))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    crate::auth::middleware::guard_native,
                )),
        )
        // The UI's fallback must be last: it answers every path the API did not.
        .merge(crate::ui::router())
        .with_state(state.clone())
        // Outside everything, because a request for a name this server does not
        // answer to should not reach a guard, let alone a handler.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            check_host,
        ))
        .layer(cors(&state))
        .layer(header_layer(
            header::CONTENT_SECURITY_POLICY,
            CONTENT_SECURITY_POLICY,
        ))
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            timeout,
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
}

/// Refuse a request addressed to a name this server does not answer to.
///
/// The defence against DNS rebinding. A name with a one-second TTL that
/// resolves first to the attacker's host and then to this server's address
/// makes the victim's own browser treat the attacker's page as *same-origin*
/// with this server — CORS does not apply to a same-origin request, so it never
/// gets a say. What the script can then do depends on the surface policy: with
/// the default `apikey` it still has no credential, but with `allowlist` the
/// browser is calling from an address that is on the list.
///
/// Off unless `AMS_ALLOWED_HOSTS` names something, because there is no safe
/// guess — this is reached by container name, LAN address, and whatever the
/// router calls it.
async fn check_host(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let allowed = &state.config.server.allowed_hosts;

    if allowed.is_empty() {
        return next.run(request).await;
    }

    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(strip_port)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if allowed.iter().any(|a| a == &host) {
        return next.run(request).await;
    }

    tracing::warn!(%host, "refused a request addressed to a name this server does not answer to");
    (StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response()
}

/// `example.com:8080` is `example.com`; `[::1]:8080` is `[::1]`.
fn strip_port(host: &str) -> &str {
    let host = host.trim();

    if let Some(end) = host.strip_prefix('[').and_then(|_| host.find(']')) {
        return &host[..=end];
    }

    match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    }
}

/// What a page here may load, and who may embed it.
///
/// The interface is served from this same origin and pulls in no third-party
/// code, so everything is `'self'`. Two exceptions, both forced:
///
/// * **images** come from wherever a provider filed them — TMDB, TheTVDB,
///   Fanart.tv and whatever host a manual entry names;
/// * **inline styles** are how React writes a `style` attribute, and the
///   interface uses them for per-card animation delays and accent colours.
///
/// `frame-ancestors 'none'` is the one that earns its place on a server on a
/// home network: it is what stops a page elsewhere framing this one and
/// borrowing an administrator's clicks. `base-uri` and `form-action` close the
/// two ways a stray tag could redirect a relative URL or a form off-origin.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     img-src 'self' data: https:; \
     style-src 'self' 'unsafe-inline'; \
     script-src 'self'; \
     connect-src 'self'; \
     font-src 'self' data:; \
     object-src 'none'; \
     base-uri 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'";

/// One fixed response header, on every answer this server gives.
fn header_layer(
    name: header::HeaderName,
    value: &'static str,
) -> tower_http::set_header::SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

fn cors(state: &AppState) -> CorsLayer {
    let origins = &state.config.server.cors_origins;

    if origins.is_empty() {
        // Same-origin only: the UI is served from this server.
        return CorsLayer::new();
    }

    let parsed: Vec<HeaderValue> = origins
        .iter()
        // `*` cannot be combined with credentials, and tower-http answers that
        // by panicking rather than refusing — so a plausible thing to write in
        // a compose file took the process down at startup instead of being
        // reported. There is no wildcard to have here: the cookie rides along.
        .filter(|o| {
            if o.trim() == "*" {
                tracing::warn!(
                    "ignoring `*` in AMS_CORS_ORIGINS: this server sends credentials, \
                     so every allowed origin has to be named"
                );
                return false;
            }
            true
        })
        .filter_map(|o| match o.parse() {
            Ok(value) => Some(value),
            Err(_) => {
                tracing::warn!(origin = %o, "ignoring an invalid CORS origin");
                None
            }
        })
        .collect();

    if parsed.is_empty() {
        return CorsLayer::new();
    }

    CorsLayer::new()
        .allow_origin(parsed)
        .allow_credentials(true)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            "x-api-key".parse().unwrap(),
        ])
}

async fn health() -> StatusCode {
    StatusCode::OK
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Ready {
    database: bool,
    tmdb_configured: bool,
}

/// Liveness is `/health`; this is readiness, and it touches the database.
async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Ready>) {
    let database = state.db.health().await.is_ok();

    let status = if database {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(Ready {
            database,
            tmdb_configured: state.tmdb.is_configured(),
        }),
    )
}

/// Resolve on SIGINT or SIGTERM so container stops drain in-flight requests.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => tracing::warn!(error = %e, "cannot install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_host_header_is_compared_without_its_port() {
        assert_eq!(strip_port("metadata.example.com"), "metadata.example.com");
        assert_eq!(
            strip_port("metadata.example.com:8080"),
            "metadata.example.com"
        );
        assert_eq!(strip_port(" 192.168.1.50:8080 "), "192.168.1.50");
        assert_eq!(strip_port("[::1]:8080"), "[::1]");
        assert_eq!(strip_port("[2001:db8::1]"), "[2001:db8::1]");
        // Not a port, so not stripped.
        assert_eq!(strip_port("host:notaport"), "host:notaport");
    }

    use super::*;
}
