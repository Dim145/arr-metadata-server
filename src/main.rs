mod api;
mod auth;
mod cache;
mod config;
mod coord;
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
    // nosemgrep: rust.lang.security.args.args — the binary's own subcommand, matched against a fixed list below
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

    // Whatever an error or a trace repeats, none of these reaches the log.
    for secret in config.secrets() {
        telemetry::redact_also(&secret);
    }

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

/// Probe the running server and exit non-zero if it is unwell.
///
/// The address and the scheme are the server's own, read from the same
/// configuration it was started with — every name `AMS_BIND_ADDRESS` goes
/// by, an empty variable read as unset — and probed over loopback when it
/// listens on every interface, at the address it is bound to otherwise.
async fn healthcheck() -> Result<()> {
    let _ = dotenvy::dotenv();
    let config = config::Config::from_env().context("invalid configuration")?;
    let url = health_url(config.server.bind, config.server.tls.is_some());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        // The certificate is issued for a provider hostname, not for loopback.
        .danger_accept_invalid_certs(url.starts_with("https://"))
        .build()?;

    let status = client.get(&url).send().await?.status();

    if status.is_success() {
        Ok(())
    } else {
        anyhow::bail!("health check returned {status}")
    }
}

/// Where the health check asks: loopback for a server on every interface —
/// IPv4's, which a dual-stack `[::]` answers too — the bound address for one
/// bound to a single interface.
fn health_url(bind: std::net::SocketAddr, tls: bool) -> String {
    let scheme = if tls { "https" } else { "http" };
    let host = match bind.ip() {
        ip if ip.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        ip => ip.to_string(),
    };
    format!("{scheme}://{host}:{}/health", bind.port())
}

#[cfg(test)]
mod tests {
    use super::health_url;

    #[test]
    fn the_health_check_asks_where_the_server_listens() {
        assert_eq!(
            health_url("0.0.0.0:8080".parse().unwrap(), false),
            "http://127.0.0.1:8080/health"
        );
        assert_eq!(
            health_url("[::]:9000".parse().unwrap(), true),
            "https://127.0.0.1:9000/health"
        );
        assert_eq!(
            health_url("192.168.1.5:8080".parse().unwrap(), false),
            "http://192.168.1.5:8080/health"
        );
        assert_eq!(
            health_url("[::1]:8080".parse().unwrap(), false),
            "http://[::1]:8080/health"
        );
    }
}
