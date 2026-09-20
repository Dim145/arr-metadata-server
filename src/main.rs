mod api;
mod auth;
mod cache;
mod config;
mod db;
mod domain;
mod error;
mod jobs;
mod providers;
mod service;
mod state;
mod telemetry;
mod ui;
mod wire;
mod web;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    // The runtime image has no shell and no curl, so the binary answers its own
    // container health check.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck().await;
    }

    // A local .env is a convenience for development; real deployments pass the
    // environment directly and this call is a no-op.
    let _ = dotenvy::dotenv();

    telemetry::init();

    // reqwest and axum-server both link rustls; without an explicit choice the
    // process has two candidate providers and every handshake fails at runtime.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install the rustls crypto provider"))?;

    let config = config::Config::from_env().context("invalid configuration")?;

    let state = state::AppState::bootstrap(config).await?;

    web::serve(state).await
}

/// Probe the running server over loopback and exit non-zero if it is unwell.
///
/// `AMS_BIND_ADDRESS` gives the port; the host is always loopback, because this
/// runs inside the same container.
async fn healthcheck() -> Result<()> {
    let bind = std::env::var("AMS_BIND_ADDRESS")
        .or_else(|_| std::env::var("BIND_ADDRESS"))
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let port = bind.rsplit(':').next().unwrap_or("8080");

    // TLS terminates here when configured, so probe the same scheme.
    let scheme = if std::env::var("AMS_TLS_CERT").is_ok() { "https" } else { "http" };
    let url = format!("{scheme}://127.0.0.1:{port}/health");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        // The certificate is issued for a provider hostname, not for loopback.
        .danger_accept_invalid_certs(scheme == "https")
        .build()?;

    let status = client.get(&url).send().await?.status();

    if status.is_success() {
        Ok(())
    } else {
        anyhow::bail!("health check returned {status}")
    }
}
