mod api;
mod auth;
mod cache;
mod config;
mod db;
mod domain;
mod error;
mod jobs;
mod merge;
mod providers;
mod state;
mod telemetry;
mod web;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
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
