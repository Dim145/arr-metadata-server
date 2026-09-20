//! HTTP server: router assembly, middleware stack, graceful shutdown.

use std::time::Duration;

use anyhow::{Context, Result};
use axum::{Router, http::StatusCode, routing::get};
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer,
    normalize_path::NormalizePathLayer, timeout::TimeoutLayer, trace::TraceLayer,
};

use crate::state::AppState;

pub async fn serve(state: AppState) -> Result<()> {
    let bind = state.config.server.bind;
    let timeout = state.config.server.request_timeout;

    let router = build_router(state.clone(), timeout);

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

    state.db.close().await;
    Ok(())
}

fn build_router(state: AppState, timeout: Duration) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
        .layer(NormalizePathLayer::trim_trailing_slash())
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            timeout,
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
}

async fn health() -> StatusCode {
    StatusCode::OK
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
