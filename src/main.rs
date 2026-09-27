mod api;
mod auth;
mod cache;
mod config;
mod db;
mod domain;
mod error;
mod export;
mod jobs;
mod media;
mod merge;
mod metrics;
mod outbound;
mod providers;
mod service;
mod settings;
mod state;
mod telemetry;
mod tls;
mod ui;
mod web;
mod wire;

use anyhow::{Context, Result};

/// The allocator: a server of many small, short-lived allocations across
/// many threads, which this one serves faster and with less fragmentation
/// than the system's.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

const USAGE: &str = "\
arr-metadata-server — a metadata server for the *arr stack

    arr-metadata-server                      run the server
    arr-metadata-server healthcheck          probe a running server over loopback
    arr-metadata-server transfer FROM TO     copy a database to another engine

  transfer takes two connection URLs and copies every row from the first into
  the second, which must already be empty. Migrations are applied to the target
  first. Add --force to add to a target that already holds rows.

    arr-metadata-server transfer \\
        'sqlite://data/ams.db' \\
        'postgres://ams:secret@localhost/ams'
";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        // The runtime image has no shell and no curl, so the binary answers its
        // own container health check.
        Some("healthcheck") => return healthcheck().await,
        Some("transfer") => return transfer(&args[1..]).await,
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("arr-metadata-server {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown command {other:?}\n\n{USAGE}"),
        None => {}
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

/// Copy one database into another.
///
/// Deliberately a separate command rather than something the server does at
/// startup: moving data is a decision, and doing it implicitly on a config
/// change would be the kind of surprise nobody wants from a database.
async fn transfer(args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    let urls: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();

    let [from, to] = urls.as_slice() else {
        anyhow::bail!("transfer needs a source and a target URL\n\n{USAGE}");
    };

    let open = |url: &str| crate::config::Database {
        url: url.to_string(),
        max_connections: 4,
        acquire_timeout: std::time::Duration::from_secs(30),
    };

    let source = db::Db::connect(&open(from))
        .await
        .context("cannot open the source")?;
    let target = db::Db::connect(&open(to))
        .await
        .context("cannot open the target")?;

    // The source is read only; migrating it would be an unasked-for change.
    target
        .migrate()
        .await
        .context("cannot migrate the target")?;

    let report = db::transfer::run(&source, &target, force).await?;

    for (table, rows) in &report.copied {
        if *rows > 0 {
            println!("  {rows:>8}  {table}");
        }
    }
    println!("\n{} rows copied", report.total());

    source.close().await;
    target.close().await;

    Ok(())
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
    let scheme = if std::env::var("AMS_TLS_CERT").is_ok() {
        "https"
    } else {
        "http"
    };
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
