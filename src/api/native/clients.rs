//! API client management.

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
    db::repo::{self, audit::Action, client::ApiClient},
    error::{AppError, AppResult},
    state::AppState,
};

/// Scopes a key may carry. `read` is implied by existing at all.
const KNOWN_SCOPES: &[&str] = &["read", "write", "admin"];

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Clients";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(detail, update, remove))
}

/// Every issued key. The keys themselves are not recoverable.
#[utoipa::path(
    get, path = "/clients", tag = TAG,
    responses(
        (status = 200, body = Vec<ApiClient>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<ApiClient>>> {
    require_admin(&identity)?;

    Ok(Json(
        repo::client::list(&state.db, repo::client::Owner::Any).await?,
    ))
}

/// One client.
#[utoipa::path(
    get, path = "/clients/{id}", tag = TAG,
    params(("id" = String, Path, description = "The client's identifier")),
    responses(
        (status = 200, body = ApiClient),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such client"),
    ),
)]
async fn detail(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<ApiClient>> {
    require_admin(&identity)?;

    repo::client::get(&state.db, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = CreateClientRequest)]
pub struct CreateRequest {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub note: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = CreateClientResponse)]
pub struct CreateResponse {
    #[serde(flatten)]
    pub client: ApiClient,
    /// Shown once. It is not recoverable afterwards — only its hash is stored.
    pub key: String,
}

/// Issue a key.
///
/// The response carries the key in the clear. It is the only time it is ever
/// shown: only a SHA-256 of it is stored.
#[utoipa::path(
    post, path = "/clients", tag = TAG,
    request_body = CreateRequest,
    responses(
        (status = 201, description = "The key, shown once", body = CreateResponse),
        (status = 400, description = "Empty name or unknown scope"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "A client already has that name"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<CreateRequest>,
) -> AppResult<(StatusCode, Json<CreateResponse>)> {
    require_admin(&identity)?;

    let name = request.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name must not be empty".into()));
    }

    for scope in &request.scopes {
        if !KNOWN_SCOPES.contains(&scope.as_str()) {
            return Err(AppError::BadRequest(format!(
                "unknown scope {scope:?}; expected one of {}",
                KNOWN_SCOPES.join(", ")
            )));
        }
    }

    // Kept as a time this server reads, in UTC: what is stored is what the
    // guard compares.
    let expires_at = request
        .expires_at
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(expiry)
        .transpose()?;

    // The server's keys share one namespace; a person's are theirs alone.
    let _held = state.accounts_lock().await;
    if repo::client::name_taken(&state.db, None, name, None).await? {
        return Err(AppError::Conflict(format!(
            "a client named {name:?} already exists"
        )));
    }

    let generated = secrets::generate_api_key()?;

    let client = repo::client::create(
        &state.db,
        repo::client::NewClient {
            name,
            key_prefix: &generated.prefix,
            key_hash: &generated.hash,
            scopes: &request.scopes,
            expires_at: expires_at.as_deref(),
            note: request.note.as_deref(),
            owner_id: None,
        },
    )
    .await
    .map_err(|e| {
        // The unique index on name is the only constraint a caller can trip here.
        if e.to_string().contains("UNIQUE") || e.to_string().contains("duplicate key") {
            AppError::Conflict(format!("a client named {name:?} already exists"))
        } else {
            AppError::Internal(e)
        }
    })?;

    tracing::info!(client = %client.name, actor = %identity.label(), "issued an API key");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ClientCreated,
            // The prefix, never the key.
            target: Some(&client.name),
            detail: Some(&format!(
                "{} [{}]",
                client.key_prefix,
                client.scopes.join(", ")
            )),
        },
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(CreateResponse {
            client,
            key: generated.plaintext,
        }),
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = UpdateClientRequest)]
pub struct UpdateRequest {
    pub is_enabled: Option<bool>,
}

/// Enable or disable a key without revoking it.
#[utoipa::path(
    patch, path = "/clients/{id}", tag = TAG,
    params(("id" = String, Path, description = "The client's identifier")),
    request_body = UpdateRequest,
    responses(
        (status = 200, body = ApiClient),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such client"),
    ),
)]
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> AppResult<Json<ApiClient>> {
    require_admin(&identity)?;

    if let Some(enabled) = request.is_enabled {
        // Read first: the trail is read by humans, who know the name, and a
        // person's key paused from here is one its owner cannot resume.
        let key = repo::client::get(&state.db, &id).await?;
        let name = key.as_ref().map(|c| c.name.clone());

        if !repo::client::set_enabled(&state.db, &id, enabled).await? {
            return Err(AppError::NotFound);
        }

        // A server key has no owner to keep it from; an administrator's own
        // key, paused from here, is theirs to resume from their account.
        if let Some(owner) = key.as_ref().and_then(|k| k.owner_id.as_deref()) {
            if !enabled && identity.person_id() != Some(owner) {
                mark_paused(&state, &id, Some(&identity.label())).await?;
            } else if enabled {
                mark_paused(&state, &id, None).await?;
            }
        }

        audit::record(
            &state,
            Event {
                identity: Some(&identity),
                ip: &ip,
                action: Action::ClientUpdated,
                target: name.as_deref(),
                detail: Some(if enabled { "enabled" } else { "disabled" }),
            },
        )
        .await;
    }

    repo::client::get(&state.db, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// Revoke a key permanently.
#[utoipa::path(
    delete, path = "/clients/{id}", tag = TAG,
    params(("id" = String, Path, description = "The client's identifier")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such client"),
        (status = 409, description = "That is the key this request authenticated with"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_admin(&identity)?;

    let name = repo::client::get(&state.db, &id).await?.map(|c| c.name);

    // Refuse to delete the key currently being used: it would lock the caller
    // out mid-session with no way back in.
    if let Identity::Client(current) = &identity
        && current.id == id
    {
        return Err(AppError::Conflict(
            "this is the key you are authenticated with; disable it from another session".into(),
        ));
    }

    if !repo::client::delete(&state.db, &id).await? {
        return Err(AppError::NotFound);
    }

    tracing::info!(%id, actor = %identity.label(), "revoked an API key");

    // A revoked key's settings would otherwise linger under its id.
    state
        .forget_settings(crate::settings::Scope::Client, &id)
        .await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ClientRevoked,
            // Named, not identified: the row is gone, and the name is what the
            // trail is read for. Matches Action::ClientCreated.
            target: name.as_deref(),
            detail: Some(&id),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

fn require_admin(identity: &Identity) -> AppResult<()> {
    identity.is_admin().then_some(()).ok_or(AppError::Forbidden)
}

/// An expiry a caller gave, or why it cannot be one: a time — RFC 3339, or a
/// bare date, read as its first moment in UTC — still to come. Returned as
/// stored, in RFC 3339 and UTC.
fn expiry(text: &str) -> AppResult<String> {
    let at = crate::auth::middleware::key_expiry(text).ok_or_else(|| {
        AppError::BadRequest("expiresAt is a time, as 2026-12-31T23:59:00Z or 2026-12-31".into())
    })?;
    if at <= chrono::Utc::now() {
        return Err(AppError::BadRequest("expiresAt has already passed".into()));
    }
    Ok(crate::db::to_rfc3339(at))
}

/// Where the administrator who paused a person's key is written down.
///
/// In the store every instance shares rather than on the key's row: what
/// matters is only that the owner's own "resume" is refused while it is
/// there, and that is the store that needs no change of schema to hold it.
fn pause_entry(id: &str) -> String {
    format!("client.pausedBy:{id}")
}

/// Write down who paused a person's key, or that nobody holds it paused.
async fn mark_paused(state: &AppState, id: &str, by: Option<&str>) -> AppResult<()> {
    repo::keystore::put(&state.db, &pause_entry(id), by.unwrap_or_default()).await?;
    Ok(())
}

/// Whether an administrator paused this key, which its owner then may not
/// resume.
pub(crate) async fn paused_by_admin(state: &AppState, id: &str) -> AppResult<bool> {
    Ok(repo::keystore::get(&state.db, &pause_entry(id))
        .await?
        .is_some_and(|by| !by.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expiry_is_a_time_still_to_come() {
        assert_eq!(
            expiry("2999-12-31T00:00:00+02:00").unwrap(),
            "2999-12-30T22:00:00.000Z"
        );
        assert_eq!(expiry("2999-12-31").unwrap(), "2999-12-31T00:00:00.000Z");
        for refused in ["31/12/2999", "Dec 31 2999", "2001-01-01", "soon"] {
            assert!(
                matches!(expiry(refused), Err(AppError::BadRequest(_))),
                "{refused}"
            );
        }
    }
}
