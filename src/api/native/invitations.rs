//! Invitations, as an administrator hands them out.
//!
//! An invitation is a code — shown once, in the link it makes — that opens the
//! sign-up page while `registration.mode` is anything but `closed`, and says
//! which role the account it opens gets: a member's or an editor's. An
//! administrator is made from the Members page, never from a link that could
//! be forwarded.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::{Identity, secrets},
    db::repo::{self, audit::Action, invitation::Invitation, user::Role},
    error::{AppError, AppResult},
    state::AppState,
};

pub const TAG: &str = "Users";

/// How many accounts one invitation may open, at most.
const MAX_USES: i64 = 100;

/// How long an invitation may last, at most, in days.
const MAX_DAYS: i64 = 365;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(revoke))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Invitations {
    pub invitations: Vec<Invitation>,
    /// How many are still usable, and how many accounts they may still open.
    pub usable: i64,
    pub places: i64,
}

#[utoipa::path(
    get, path = "/invitations", tag = TAG,
    responses(
        (status = 200, body = Invitations),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Invitations>> {
    identity.require_admin()?;

    let invitations = repo::invitation::list(&state.db).await?;
    let (usable, places) = repo::invitation::count_usable(&state.db).await?;

    Ok(Json(Invitations {
        invitations,
        usable,
        places,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateInvitationRequest {
    /// `member` or `editor`.
    pub role: Role,
    /// How many accounts it may open; one when left out.
    pub max_uses: Option<i64>,
    /// Days it lasts; seven when left out, and never more than a year.
    pub days: Option<i64>,
    /// Who it is for, for the administrators' memory. Never shown to them.
    pub note: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedInvitation {
    pub invitation: Invitation,
    /// The code itself, `K7QM-2XRP-9DHT-4WCN`. Shown once.
    pub code: String,
    /// The link to send, at this server's public address when it has one —
    /// an administrator on the home network would otherwise hand out the
    /// home network's address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// Make an invitation. Its code is in the answer, and nowhere else ever again.
#[utoipa::path(
    post, path = "/invitations", tag = TAG,
    request_body = CreateInvitationRequest,
    responses(
        (status = 201, body = IssuedInvitation),
        (status = 400, description = "A field was rejected"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<CreateInvitationRequest>,
) -> AppResult<(StatusCode, Json<IssuedInvitation>)> {
    identity.require_admin()?;

    if request.role == Role::Admin {
        return Err(AppError::BadRequest(
            "an invitation opens a member's or an editor's account; promote the account \
             afterwards to make an administrator"
                .into(),
        ));
    }

    let max_uses = request.max_uses.unwrap_or(1);
    if !(1..=MAX_USES).contains(&max_uses) {
        return Err(AppError::BadRequest(format!(
            "an invitation opens 1 to {MAX_USES} accounts"
        )));
    }

    let days = request.days.unwrap_or(7);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(AppError::BadRequest(format!(
            "an invitation lasts 1 to {MAX_DAYS} days"
        )));
    }
    let expires_at = (chrono::Utc::now() + chrono::Duration::days(days))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

    let note = match request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        Some(note) if note.chars().count() > 200 => {
            return Err(AppError::BadRequest(
                "a note is at most 200 characters".into(),
            ));
        }
        other => other.map(String::from),
    };

    let code = secrets::generate_invitation_code()?;

    let invitation = repo::invitation::create(
        &state.db,
        repo::invitation::NewInvitation {
            code_hash: &code.hash,
            code_prefix: &code.prefix,
            role: request.role,
            max_uses,
            expires_at: Some(&expires_at),
            note: note.as_deref(),
            // The person behind the request, or nobody for a server key.
            created_by: identity.person_id(),
        },
    )
    .await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::InvitationCreated,
            target: Some(&invitation.code_prefix),
            detail: Some(&format!(
                "{} · {} use(s) · {days} day(s)",
                invitation.role.as_str(),
                invitation.max_uses
            )),
        },
    )
    .await;

    let link = state
        .config
        .server
        .public_url
        .as_deref()
        .map(|base| format!("{base}/register#invite={}", code.plaintext));

    Ok((
        StatusCode::CREATED,
        Json(IssuedInvitation {
            invitation,
            code: code.plaintext,
            link,
        }),
    ))
}

/// Withdraw an invitation. The accounts it already opened stay.
#[utoipa::path(
    delete, path = "/invitations/{id}", tag = TAG,
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Withdrawn"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404),
    ),
)]
async fn revoke(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;

    let invitation = repo::invitation::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    if !repo::invitation::revoke(&state.db, &id).await? {
        // Already withdrawn: the same outcome, asked twice.
        return Ok(StatusCode::NO_CONTENT);
    }

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::InvitationRevoked,
            target: Some(&invitation.code_prefix),
            detail: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}
