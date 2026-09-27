//! The doors, as an administrator reads them: which one the interface uses,
//! which one the clients use, the names that one answers to, the certificate
//! it shows and the authority behind it — and the way to renew it now.

use axum::{Extension, Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::audit::Action,
    error::AppResult,
    state::AppState,
    tls,
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(renew))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Listeners {
    pub web: Web,
    /// None when no clients' listener is configured.
    pub clients: Option<Door>,
}

/// The interface's and the native API's door.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Web {
    pub bind: String,
    /// Whether it is in TLS, from the operator's files.
    pub tls: bool,
}

/// The clients' door.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Door {
    pub bind: String,
    /// `authority`: a certificate this server issues itself; `own`: the
    /// operator's files.
    pub mode: &'static str,
    /// Every name it answers to.
    pub names: Vec<String>,
    /// Names asked for that the authority may not certify: its constraints
    /// were set when it was made.
    pub refused: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate: Option<Certificate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<Authority>,
    pub renewing: bool,
    /// A certificate with fewer days left than this is renewed.
    pub renew_before_days: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Certificate {
    pub names: Vec<String>,
    pub not_after: String,
    /// SHA-256, as a browser shows it.
    pub fingerprint: String,
    pub due: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Authority {
    pub subject: String,
    pub fingerprint: String,
    pub not_after: String,
}

#[utoipa::path(
    get, path = "/admin/tls", tag = TAG,
    responses(
        (status = 200, body = Listeners),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn status(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Listeners>> {
    identity.require_admin()?;

    let clients = state.tls.clients.as_ref().map(|clients| {
        let issued = clients.issued();
        Door {
            bind: clients.bind.to_string(),
            mode: if clients.authority.is_some() {
                "authority"
            } else {
                "own"
            },
            names: clients.names.iter().map(ToString::to_string).collect(),
            refused: clients.refused().iter().map(ToString::to_string).collect(),
            certificate: issued.map(|issued| Certificate {
                names: issued.names.iter().map(ToString::to_string).collect(),
                not_after: tls::rfc3339(issued.not_after),
                fingerprint: issued.fingerprint.clone(),
                due: issued.due(),
            }),
            authority: clients.authority.as_ref().map(|authority| Authority {
                subject: authority.subject.clone(),
                fingerprint: authority.fingerprint.clone(),
                not_after: tls::rfc3339(authority.not_after),
            }),
            renewing: tls::is_renewing(),
            renew_before_days: tls::ca::RENEW_BEFORE_DAYS,
        }
    });

    Ok(Json(Listeners {
        web: Web {
            bind: state.config.server.bind.to_string(),
            tls: state.config.server.tls.is_some(),
        },
        clients,
    }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Renewal {
    /// The run's id in the history.
    pub job_id: Option<String>,
}

/// Issue the clients' certificate anew, now, whatever time it has left.
#[utoipa::path(
    post, path = "/admin/tls/renew", tag = TAG,
    responses(
        (status = 202, body = Renewal),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "A renewal is running already"),
        (status = 503, description = "No certificate of this server's own to renew"),
    ),
)]
async fn renew(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
) -> AppResult<(StatusCode, Json<Renewal>)> {
    identity.require_admin()?;
    let job_id = tls::renew_now(&state, &identity.label()).await?;
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::CertificateRenewed,
            target: None,
            detail: job_id.as_deref(),
        },
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(Renewal { job_id })))
}
