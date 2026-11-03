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

/// Scopes a key may carry. `read` is implied by existing at all; `metrics`
/// alone makes a scraper's key, which reads the figures and nothing else;
/// `mcp` opens the MCP server, `/mcp`, and alone makes an agent's key, which
/// opens nothing else.
const KNOWN_SCOPES: &[&str] = &["read", "write", "admin", "metrics", crate::mcp::SCOPE];

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
    operation_id = "clients_list",
    responses(
        (status = 200, body = Vec<ApiClient>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<ApiClient>>> {
    identity.require_admin()?;

    Ok(Json(
        repo::client::list(&state.db, repo::client::Owner::Any).await?,
    ))
}

/// One client.
#[utoipa::path(
    get, path = "/clients/{id}", tag = TAG,
    operation_id = "clients_detail",
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
    identity.require_admin()?;

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
    operation_id = "clients_create",
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
    identity.require_admin()?;

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
///
/// A person's key paused from here pauses its owner's keys: its owner can
/// neither resume it nor make another, or resume any other of theirs, until an
/// administrator enables a key of theirs from here, or lifts the pause
/// (`DELETE /users/{id}/keys-pause`).
#[utoipa::path(
    patch, path = "/clients/{id}", tag = TAG,
    operation_id = "clients_update",
    params(("id" = String, Path, description = "The client's identifier")),
    request_body = UpdateRequest,
    responses(
        (status = 200, body = ApiClient),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such client"),
    ),
)]
pub(crate) async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> AppResult<Json<ApiClient>> {
    identity.require_admin()?;

    if let Some(enabled) = request.is_enabled {
        // Under the accounts lock, as its owner's own changes to their keys
        // are: a key enabled between the pause being read and being written
        // would be enabled still.
        let _held = state.accounts_lock().await;

        // Read first: the trail is read by humans, who know the name, and a
        // person's key paused from here is one its owner cannot resume.
        let key = repo::client::get(&state.db, &id).await?;
        let name = key.as_ref().map(|c| c.name.clone());

        if !repo::client::set_enabled(&state.db, &id, enabled).await? {
            return Err(AppError::NotFound);
        }

        // A server key has no owner to keep it from; an administrator's own
        // key, paused from here, is theirs to resume from their account.
        let mut said = String::from(if enabled { "enabled" } else { "disabled" });
        if let Some(owner) = key.as_ref().and_then(|k| k.owner_id.as_deref()) {
            if !enabled && identity.person_id() != Some(owner) {
                mark_paused(&state, owner, Some(&identity.label())).await?;
                said.push_str(" · its owner can neither make nor resume keys until it is lifted");
            } else if enabled && lift_pause(&state, owner).await? {
                said.push_str(" · the pause on its owner's keys lifted");
            }
        }

        audit::record(
            &state,
            Event {
                identity: Some(&identity),
                ip: &ip,
                action: Action::ClientUpdated,
                target: name.as_deref(),
                detail: Some(&said),
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
    operation_id = "clients_remove",
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
    identity.require_admin()?;

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

/// Where the administrator who paused a person's keys is written down.
///
/// On the person, not on a key: a key its owner deletes and makes again is
/// another key, and a pause that stood on the first went with it. In the store
/// every instance shares rather than on the account's row, which is the one
/// that needs no change of schema to hold it.
fn pause_entry(owner_id: &str) -> String {
    format!("user.keysPausedBy:{owner_id}")
}

/// What an older version wrote: the pause on one key, and nothing on its
/// owner. Still honoured, until an administrator lifts it.
fn key_pause_entry(key_id: &str) -> String {
    format!("client.pausedBy:{key_id}")
}

/// Write down who paused a person's keys, or that nobody holds them paused.
async fn mark_paused(state: &AppState, owner_id: &str, by: Option<&str>) -> AppResult<()> {
    repo::keystore::put(&state.db, &pause_entry(owner_id), by.unwrap_or_default()).await?;
    Ok(())
}

/// Who an entry says paused, when it says anybody.
async fn standing(state: &AppState, entry: &str) -> AppResult<Option<String>> {
    Ok(repo::keystore::get(&state.db, entry)
        .await?
        .filter(|by| !by.is_empty()))
}

/// Who paused this person's keys, when an administrator did and none has
/// lifted it: until then their owner can neither make a key nor resume one.
pub(crate) async fn paused_by_admin(state: &AppState, owner_id: &str) -> AppResult<Option<String>> {
    if let Some(by) = standing(state, &pause_entry(owner_id)).await? {
        return Ok(Some(by));
    }
    // A key paused before the pause stood on the person: the person's, too.
    for key in repo::client::list(&state.db, repo::client::Owner::User(owner_id)).await? {
        if let Some(by) = standing(state, &key_pause_entry(&key.id)).await? {
            return Ok(Some(by));
        }
    }
    Ok(None)
}

/// Lift the pause on a person's keys, the one on the person and any that an
/// older version left on a key: whether one stood. What an administrator does
/// by enabling one of their keys, or by saying so.
pub(crate) async fn lift_pause(state: &AppState, owner_id: &str) -> AppResult<bool> {
    let mut lifted = false;
    if standing(state, &pause_entry(owner_id)).await?.is_some() {
        mark_paused(state, owner_id, None).await?;
        lifted = true;
    }
    for key in repo::client::list(&state.db, repo::client::Owner::User(owner_id)).await? {
        let entry = key_pause_entry(&key.id);
        if standing(state, &entry).await?.is_some() {
            repo::keystore::put(&state.db, &entry, "").await?;
            lifted = true;
        }
    }
    Ok(lifted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::user::{NO_PASSWORD, NewUser, Role, Status, User};

    async fn person(state: &AppState, name: &str, role: Role) -> User {
        repo::user::create(
            &state.db,
            NewUser {
                username: name,
                password_hash: NO_PASSWORD,
                role,
                status: Status::Active,
                display_name: None,
                email: None,
                invited_by: None,
                oidc: None,
            },
        )
        .await
        .expect("an account")
    }

    async fn key_of(state: &AppState, owner: &User, name: &str) -> ApiClient {
        repo::client::create(
            &state.db,
            repo::client::NewClient {
                name,
                key_prefix: "ams_test",
                key_hash: name,
                scopes: &["read".to_string()],
                expires_at: None,
                note: None,
                owner_id: Some(&owner.id),
            },
        )
        .await
        .expect("a key")
    }

    /// What the version before this one wrote was a pause on a key, and
    /// nothing on its owner: still a pause on the person, and lifted with it.
    #[tokio::test]
    async fn a_pause_put_on_a_key_by_an_older_version_is_still_the_persons() {
        let state = crate::api::native::testing::server().await;
        let alice = person(&state, "alice", Role::Member).await;
        let key = key_of(&state, &alice, "Sonarr").await;

        assert_eq!(paused_by_admin(&state, &alice.id).await.unwrap(), None);
        repo::keystore::put(
            &state.db,
            &format!("client.pausedBy:{}", key.id),
            "admin:root",
        )
        .await
        .unwrap();
        assert_eq!(
            paused_by_admin(&state, &alice.id).await.unwrap().as_deref(),
            Some("admin:root")
        );

        assert!(lift_pause(&state, &alice.id).await.unwrap());
        assert_eq!(paused_by_admin(&state, &alice.id).await.unwrap(), None);
        assert!(!lift_pause(&state, &alice.id).await.unwrap(), "none stood");
    }

    /// Pausing a person's key writes the pause on the person, naming who; an
    /// administrator's own key, paused from here, is theirs to resume.
    #[tokio::test]
    async fn pausing_a_persons_key_pauses_the_person_and_naming_who() {
        let state = crate::api::native::testing::server().await;
        let admin = person(&state, "root", Role::Admin).await;
        let alice = person(&state, "alice", Role::Member).await;
        let hers = key_of(&state, &alice, "Sonarr").await;
        let own = key_of(&state, &admin, "Mine").await;
        let as_admin = || Extension(Identity::User(Box::new(admin.clone())));
        let pause = |id: &str, enabled: bool| {
            update(
                State(state.clone()),
                as_admin(),
                ClientIp::NONE,
                Path(id.to_string()),
                Json(UpdateRequest {
                    is_enabled: Some(enabled),
                }),
            )
        };

        let _ = pause(&own.id, false).await.expect("an administrator's own");
        assert_eq!(paused_by_admin(&state, &admin.id).await.unwrap(), None);

        let _ = pause(&hers.id, false).await.expect("a member's");
        assert_eq!(
            paused_by_admin(&state, &alice.id).await.unwrap().as_deref(),
            Some("admin:root")
        );
        // Enabling it from here lifts the pause on its owner.
        let _ = pause(&hers.id, true).await.expect("resumed");
        assert_eq!(paused_by_admin(&state, &alice.id).await.unwrap(), None);
    }

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
