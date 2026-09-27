//! The clients' listener: a second door, in TLS, under the names Sonarr,
//! Radarr and the TMDB clients have compiled in.
//!
//! The interface and the native API keep their own door, `AMS_BIND_ADDRESS`,
//! plain or in TLS as the operator decides — behind a reverse proxy, or on a
//! home network by address. The clients cannot be told where to go: they call
//! `https://skyhook.sonarr.tv/…` and its like, so their door has to be on
//! 443, in TLS, with a certificate for those names. This module keeps that
//! door's certificate: issued by the authority in [`ca`] unless the operator
//! brings their own, renewed before it runs out, and loaded into the
//! listener without a restart.

pub mod ca;

use std::{
    net::SocketAddr,
    sync::{Arc, LazyLock, RwLock},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use axum::{
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_server::tls_rustls::RustlsConfig;
use time::OffsetDateTime;

use crate::{
    config::Config,
    db::repo::job,
    error::{AppError, AppResult},
    state::AppState,
};

/// The names the clients have compiled in, and so the names every clients'
/// certificate carries.
pub const IMPERSONATED: [&str; 3] = [
    "skyhook.sonarr.tv",
    "api.radarr.video",
    "api.themoviedb.org",
];

/// The script that makes a container trust the authority, served so a
/// deployment from the image alone has it.
pub const TRUST_SCRIPT: &str = include_str!("../../docker/trust-ca.sh");

/// How often the certificate is looked at, to be renewed in time.
pub const RENEW_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// What this process knows of its doors.
pub struct Tls {
    pub clients: Option<Door>,
}

/// The clients' door.
pub struct Door {
    pub bind: SocketAddr,
    /// Every name it answers to: the compiled-in ones, then the operator's.
    pub names: Vec<ca::Name>,
    /// The authority that issues its certificate — none when the operator
    /// brought their own.
    pub authority: Option<ca::Authority>,
    /// What the listener serves with; reloaded on renewal.
    pub rustls: RustlsConfig,
    issued: RwLock<Option<ca::Info>>,
}

impl Door {
    /// What is known of the certificate the authority issued and the
    /// listener shows: its names, its end, its fingerprint — not its key.
    pub fn issued(&self) -> Option<ca::Info> {
        self.issued
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The names asked for that the authority may not certify.
    pub fn refused(&self) -> Vec<ca::Name> {
        match &self.authority {
            Some(authority) => authority.permitted(&self.names).1,
            None => Vec::new(),
        }
    }
}

impl Tls {
    /// Open the doors' certificates: the authority read or made, the
    /// certificate read or issued. Nothing listens yet.
    pub async fn open(config: &Config) -> Result<Self> {
        let Some(cfg) = &config.clients else {
            return Ok(Self { clients: None });
        };

        let mut names: Vec<ca::Name> = IMPERSONATED
            .iter()
            .map(|name| ca::Name::Dns((*name).to_string()))
            .collect();
        for text in &cfg.names {
            match ca::Name::parse(text) {
                Some(name) => {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                None => bail!("AMS_CLIENTS_NAMES: {text:?} is neither a hostname nor an address"),
            }
        }

        let clients = match &cfg.tls {
            Some(own) => Door {
                bind: cfg.bind,
                names,
                authority: None,
                rustls: RustlsConfig::from_pem_file(&own.cert, &own.key)
                    .await
                    .with_context(|| {
                        format!(
                            "could not load the clients' certificate {} / key {}",
                            own.cert.display(),
                            own.key.display()
                        )
                    })?,
                issued: RwLock::new(None),
            },
            None => {
                let authority = ca::Authority::open(&cfg.dir, &names)?;
                let wanted = authority.permitted(&names).0;
                let issued = match authority.issued()? {
                    Some(issued) if issued.info.covers(&wanted) && !issued.info.due() => issued,
                    _ => authority.issue(&names)?,
                };
                let rustls = RustlsConfig::from_pem(
                    issued.chain_pem.clone().into_bytes(),
                    issued.key_pem.clone().into_bytes(),
                )
                .await
                .context("the clients' certificate could not be loaded")?;
                Door {
                    bind: cfg.bind,
                    names,
                    authority: Some(authority),
                    rustls,
                    issued: RwLock::new(Some(issued.info)),
                }
            }
        };
        Ok(Self {
            clients: Some(clients),
        })
    }
}

/// A moment, as the API writes every other.
pub fn rfc3339(at: OffsetDateTime) -> String {
    crate::db::to_rfc3339(
        chrono::DateTime::<chrono::Utc>::from_timestamp(at.unix_timestamp(), 0).unwrap_or_default(),
    )
}

// ── Renewal ─────────────────────────────────────────────────────────────

static RENEWING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether the certificate is being renewed right now.
pub fn is_renewing() -> bool {
    RENEWING.try_lock().is_err()
}

/// Whether there is a certificate of this server's own to renew.
pub fn renews(state: &AppState) -> bool {
    state
        .tls
        .clients
        .as_ref()
        .is_some_and(|clients| clients.authority.is_some())
}

/// Issue the certificate anew when it is time — or now, when forced — and
/// load it into the listener. What was done, in a line for the run.
pub async fn renew(state: &AppState, force: bool) -> Result<String> {
    let Some(clients) = &state.tls.clients else {
        bail!("no clients' listener is configured");
    };
    let Some(authority) = &clients.authority else {
        bail!("the certificate is the operator's own: nothing to renew here");
    };

    let wanted = authority.permitted(&clients.names).0;
    let current = clients.issued();
    let stale = current
        .as_ref()
        .is_none_or(|issued| issued.due() || !issued.covers(&wanted));
    if !force && !stale {
        let until = current.map(|issued| issued.not_after.date().to_string());
        return Ok(format!(
            "valid until {}; nothing to do",
            until.unwrap_or_default()
        ));
    }

    let issued = authority.issue(&clients.names)?;
    clients
        .rustls
        .reload_from_pem(
            issued.chain_pem.clone().into_bytes(),
            issued.key_pem.clone().into_bytes(),
        )
        .await
        .context("the new certificate could not be loaded")?;
    let until = issued.info.not_after.date().to_string();
    *clients.issued.write().unwrap_or_else(|e| e.into_inner()) = Some(issued.info);
    Ok(format!("renewed; valid until {until}"))
}

/// Renew now, as a task somebody started: the run's id.
pub async fn renew_now(state: &AppState, by: &str) -> AppResult<Option<String>> {
    if !renews(state) {
        return Err(AppError::Disabled {
            code: "not_configured",
            message: "no certificate of this server's own to renew".into(),
        });
    }
    let held = RENEWING
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::Conflict("the certificate is being renewed already".into()))?;
    let record = job::start_by(&state.db, job::kinds::TLS_RENEW, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let outcome = renew(&state, true).await;
        close(&state, &record, outcome).await;
    });
    Ok(Some(id))
}

/// The certificate looked at daily, and renewed when it is time.
pub async fn run_renewals(state: AppState) {
    if !renews(&state) {
        return;
    }
    loop {
        tokio::time::sleep(RENEW_EVERY).await;
        let Ok(held) = RENEWING.clone().try_lock_owned() else {
            continue;
        };
        let record = job::start_by(&state.db, job::kinds::TLS_RENEW, None, None)
            .await
            .inspect_err(|e| tracing::warn!(error = %e, "could not open the renewal's run"))
            .ok();
        let outcome = renew(&state, false).await;
        match record {
            Some(record) => close(&state, &record, outcome).await,
            None => {
                if let Err(e) = outcome {
                    tracing::warn!(error = format_args!("{e:#}"), "the renewal failed");
                }
            }
        }
        drop(held);
    }
}

async fn close(state: &AppState, record: &str, outcome: Result<String>) {
    let closed = match outcome {
        Ok(summary) => job::finish(&state.db, record, Some(&summary), None).await,
        Err(e) => {
            tracing::warn!(error = format_args!("{e:#}"), "the renewal failed");
            job::finish(&state.db, record, None, Some(&format!("{e:#}"))).await
        }
    };
    if let Err(e) = closed {
        tracing::warn!(error = %e, "could not close the renewal's run");
    }
}

// ── Served ──────────────────────────────────────────────────────────────

/// The authority's certificate, for a client to trust. Public: it holds no
/// secret, and a client fetches it before it can trust anything.
pub async fn ca_certificate(State(state): State<AppState>) -> Response {
    match state
        .tls
        .clients
        .as_ref()
        .and_then(|clients| clients.authority.as_ref())
    {
        Some(authority) => (
            [
                (header::CONTENT_TYPE, "application/x-x509-ca-cert"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"arr-metadata-ca.crt\"",
                ),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            authority.cert_pem().to_string(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The script a container runs at start to trust the authority.
pub async fn trust_script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"trust-ca.sh\"",
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        TRUST_SCRIPT,
    )
        .into_response()
}
