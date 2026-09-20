//! HTTP server: router assembly, middleware stack, TLS, graceful shutdown.

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    routing::get,
};
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

    if state.config.refresh.enabled {
        tokio::spawn(crate::jobs::refresh::run(state.clone()));
    }

    let router = build_router(state.clone());

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
                .serve(router.into_make_service_with_connect_info::<std::net::SocketAddr>())
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
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
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
        .layer(cors(&state))
        .layer(security_headers())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            timeout,
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        // Sonarr sends some of these paths with a trailing slash and some
        // without; normalising before routing avoids registering each twice.
        .layer(NormalizePathLayer::trim_trailing_slash())
}

/// Headers applied to every response.
///
/// The UI is served from this same origin and loads no third-party code, so the
/// policy can be strict. Images are the exception: posters come from TMDB.
fn security_headers() -> impl tower::Layer<
    axum::routing::Route,
    Service = tower_http::set_header::SetResponseHeader<axum::routing::Route, HeaderValue>,
> + Clone {
    SetResponseHeaderLayer::overriding(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    )
}

fn cors(state: &AppState) -> CorsLayer {
    let origins = &state.config.server.cors_origins;

    if origins.is_empty() {
        // Same-origin only: the UI is served from this server.
        return CorsLayer::new();
    }

    let parsed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| match o.parse() {
            Ok(value) => Some(value),
            Err(_) => {
                tracing::warn!(origin = %o, "ignoring an invalid CORS origin");
                None
            }
        })
        .collect();

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
