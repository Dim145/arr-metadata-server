//! A person's own account: what they are called, where they are signed in,
//! and their API keys.
//!
//! Only a session reaches these — a key cannot manage the account it acts
//! for, or it could mint keys that outlive its own revocation.

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
        native::users::{clean_display_name, clean_email, clean_locale},
    },
    auth::{Identity, middleware::CurrentSession, secrets},
    db::repo::{
        self,
        audit::Action,
        client::ApiClient,
        user::{Role, Session, User},
    },
    error::{AppError, AppResult},
    state::AppState,
};

pub const TAG: &str = "Account";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(profile, update_profile))
        .routes(routes!(sessions, sign_out_elsewhere))
        .routes(routes!(close_session))
        .routes(routes!(keys, create_key))
        .routes(routes!(update_key, delete_key))
        .routes(routes!(rotate_key))
}

// ─── profile ─────────────────────────────────────────────────────────────────

#[utoipa::path(
    get, path = "/account", tag = TAG,
    responses(
        (status = 200, body = User),
        (status = 403, description = "A key cannot manage an account"),
    ),
)]
async fn profile(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<User>> {
    let me = identity.require_user()?;

    repo::user::get(&state.db, &me.id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    /// Blank clears it.
    pub display_name: Option<String>,
    /// Blank clears it.
    pub email: Option<String>,
    /// `fr`, `en`; blank follows the browser.
    pub locale: Option<String>,
}

/// Change what you are called, your address and your language. The username
/// and the role are not yours to change.
#[utoipa::path(
    patch, path = "/account", tag = TAG,
    request_body = ProfileUpdate,
    responses(
        (status = 200, body = User),
        (status = 400, description = "A field was rejected"),
        (status = 403, description = "A key cannot manage an account"),
    ),
)]
async fn update_profile(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(update): Json<ProfileUpdate>,
) -> AppResult<Json<User>> {
    let me = identity.require_user()?;

    let display_name = match &update.display_name {
        Some(raw) => clean_display_name(Some(raw))?,
        None => me.display_name.clone(),
    };
    let email = match &update.email {
        Some(raw) => clean_email(Some(raw))?,
        None => me.email.clone(),
    };
    let locale = match &update.locale {
        Some(raw) => clean_locale(Some(raw))?,
        None => me.locale.clone(),
    };

    repo::user::update_profile(
        &state.db,
        &me.id,
        display_name.as_deref(),
        email.as_deref(),
        locale.as_deref(),
    )
    .await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ProfileUpdated,
            target: Some(&me.username),
            detail: None,
        },
    )
    .await;

    repo::user::get(&state.db, &me.id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

// ─── sessions ────────────────────────────────────────────────────────────────

/// Where you are signed in, the device asking first among equals.
#[utoipa::path(
    get, path = "/account/sessions", tag = TAG,
    responses(
        (status = 200, body = Vec<Session>),
        (status = 403, description = "A key cannot manage an account"),
    ),
)]
async fn sessions(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    current: Option<Extension<CurrentSession>>,
) -> AppResult<Json<Vec<Session>>> {
    let me = identity.require_user()?;
    let current = current.map(|Extension(CurrentSession(id))| id);

    Ok(Json(
        repo::user::list_sessions(&state.db, &me.id, current.as_deref()).await?,
    ))
}

/// Sign out everywhere but here.
#[utoipa::path(
    delete, path = "/account/sessions", tag = TAG,
    responses(
        (status = 204),
        (status = 403, description = "A key cannot manage an account"),
    ),
)]
async fn sign_out_elsewhere(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    current: Option<Extension<CurrentSession>>,
    ip: ClientIp,
) -> AppResult<StatusCode> {
    let me = identity.require_user()?;
    let keep = current
        .map(|Extension(CurrentSession(id))| id)
        .unwrap_or_default();

    let closed = repo::user::delete_other_sessions(&state.db, &me.id, &keep).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::SessionsRevoked,
            target: Some(&me.username),
            detail: Some(&format!("{closed} closed, this one kept")),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Close one of your sessions.
#[utoipa::path(
    delete, path = "/account/sessions/{id}", tag = TAG,
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 403, description = "A key cannot manage an account"),
        (status = 404, description = "Not one of your sessions"),
    ),
)]
async fn close_session(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let me = identity.require_user()?;

    repo::user::delete_user_session(&state.db, &me.id, &id)
        .await?
        .then_some(StatusCode::NO_CONTENT)
        .ok_or(AppError::NotFound)
}

// ─── keys ────────────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountKeys {
    pub keys: Vec<ApiClient>,
    /// How many you may hold; absent for an administrator, who is not limited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// The scopes your role lets a key carry.
    pub scopes: Vec<String>,
    /// Whether these keys also open the TMDB relay: an editor's and an
    /// administrator's do, a member's only when an administrator allows it.
    pub relay: bool,
}

/// How many keys this person may hold: none for an administrator's limit.
fn limit_for(state: &AppState, me: &User) -> Option<i64> {
    (me.role != Role::Admin).then(|| state.keys_per_user())
}

#[utoipa::path(
    get, path = "/account/keys", tag = TAG,
    responses(
        (status = 200, body = AccountKeys),
        (status = 403, description = "A key cannot manage an account"),
    ),
)]
async fn keys(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<AccountKeys>> {
    let me = identity.require_user()?;

    Ok(Json(AccountKeys {
        keys: repo::client::list(&state.db, repo::client::Owner::User(&me.id)).await?,
        limit: limit_for(&state, me),
        scopes: me.role.scopes().iter().map(|s| s.to_string()).collect(),
        relay: state.api_on(crate::config::Api::Tmdb)
            && (me.role != Role::Member || state.relay_for_members()),
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewKey {
    pub name: String,
    /// Within what your role grants; `read` when left out.
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedKey {
    pub key: ApiClient,
    /// Shown once: only its hash is kept.
    pub secret: String,
}

/// The scopes a new key asks for, within what the role grants.
fn granted(me: &User, asked: &[String]) -> AppResult<Vec<String>> {
    let allowed = me.role.scopes();
    let mut scopes: Vec<String> = vec!["read".into()];

    for scope in asked {
        if !allowed.contains(&scope.as_str()) {
            return Err(AppError::BadRequest(format!(
                "your role cannot grant a key the {scope:?} scope"
            )));
        }
        if !scopes.contains(scope) {
            scopes.push(scope.clone());
        }
    }

    Ok(scopes)
}

fn clean_key_name(raw: &str) -> AppResult<String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(AppError::BadRequest(
            "a key's name is 1 to 60 characters".into(),
        ));
    }
    Ok(name.to_string())
}

/// Make a key that acts in your name. Its secret is shown in this answer and
/// never again.
#[utoipa::path(
    post, path = "/account/keys", tag = TAG,
    request_body = NewKey,
    responses(
        (status = 201, body = IssuedKey),
        (status = 400, description = "A field was rejected"),
        (status = 403, description = "The key limit is reached, or a key asked"),
        (status = 409, description = "You already have a key by that name"),
    ),
)]
async fn create_key(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<NewKey>,
) -> AppResult<(StatusCode, Json<IssuedKey>)> {
    let me = identity.require_user()?;
    let name = clean_key_name(&request.name)?;
    let scopes = granted(me, &request.scopes)?;

    // The count and the insert under one hold, or two requests sent together
    // would both find room for one more.
    let _held = state.accounts_lock().await;

    if let Some(limit) = limit_for(&state, me)
        && repo::client::count_owned(&state.db, &me.id).await? >= limit
    {
        return Err(AppError::Refused {
            code: "key_limit",
            message: match limit {
                0 => "keys are made by an administrator on this server".into(),
                1 => "you already have your key; regenerate it instead".into(),
                n => format!("you already hold {n} keys, the most this server allows"),
            },
        });
    }

    if repo::client::name_taken(&state.db, Some(&me.id), &name, None).await? {
        return Err(AppError::Conflict(format!(
            "you already have a key named {name:?}"
        )));
    }

    let generated = secrets::generate_api_key()?;
    let key = repo::client::create(
        &state.db,
        repo::client::NewClient {
            name: &name,
            key_prefix: &generated.prefix,
            key_hash: &generated.hash,
            scopes: &scopes,
            expires_at: None,
            note: None,
            owner_id: Some(&me.id),
        },
    )
    .await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ClientCreated,
            target: Some(&key.name),
            detail: Some(&format!("{}'s, {}", me.username, scopes.join("+"))),
        },
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(IssuedKey {
            key,
            secret: generated.plaintext,
        }),
    ))
}

/// One of your keys, or nobody's business.
async fn own_key(state: &AppState, me: &User, id: &str) -> AppResult<ApiClient> {
    repo::client::get(&state.db, id)
        .await?
        .filter(|key| key.owner_id.as_deref() == Some(me.id.as_str()))
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyUpdate {
    pub name: Option<String>,
    pub is_enabled: Option<bool>,
    pub scopes: Option<Vec<String>>,
}

/// Rename a key of yours, pause it, or change what it may do.
#[utoipa::path(
    patch, path = "/account/keys/{id}", tag = TAG,
    params(("id" = String, Path)),
    request_body = KeyUpdate,
    responses(
        (status = 200, body = ApiClient),
        (status = 400, description = "A field was rejected"),
        (status = 404, description = "Not one of your keys"),
        (status = 409, description = "You already have a key by that name"),
    ),
)]
async fn update_key(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(update): Json<KeyUpdate>,
) -> AppResult<Json<ApiClient>> {
    let me = identity.require_user()?;

    // Everything checked before anything is written, so a refused scope has
    // not already renamed the key.
    let name = update.name.as_deref().map(clean_key_name).transpose()?;
    let scopes = update
        .scopes
        .as_deref()
        .map(|asked| granted(me, asked))
        .transpose()?;

    let _held = state.accounts_lock().await;
    let key = own_key(&state, me, &id).await?;

    if let Some(name) = &name
        && repo::client::name_taken(&state.db, Some(&me.id), name, Some(&id)).await?
    {
        return Err(AppError::Conflict(format!(
            "you already have a key named {name:?}"
        )));
    }

    if let Some(name) = &name {
        repo::client::rename(&state.db, &id, name).await?;
    }
    if let Some(enabled) = update.is_enabled {
        repo::client::set_enabled(&state.db, &id, enabled).await?;
    }
    if let Some(scopes) = &scopes {
        repo::client::set_scopes(&state.db, &id, scopes).await?;
    }

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ClientUpdated,
            target: Some(&key.name),
            detail: Some(&format!("{}'s", me.username)),
        },
    )
    .await;

    repo::client::get(&state.db, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// A new secret for one of your keys. The old one stops working at once;
/// whatever used it must be given the new one.
#[utoipa::path(
    post, path = "/account/keys/{id}/rotate", tag = TAG,
    params(("id" = String, Path)),
    responses(
        (status = 200, body = IssuedKey),
        (status = 404, description = "Not one of your keys"),
    ),
)]
async fn rotate_key(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Json<IssuedKey>> {
    let me = identity.require_user()?;
    let key = own_key(&state, me, &id).await?;

    let generated = secrets::generate_api_key()?;
    repo::client::rotate(&state.db, &id, &generated.prefix, &generated.hash).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ClientRotated,
            target: Some(&key.name),
            detail: Some(&format!("{}'s", me.username)),
        },
    )
    .await;

    let key = repo::client::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(IssuedKey {
        key,
        secret: generated.plaintext,
    }))
}

/// Revoke one of your keys.
#[utoipa::path(
    delete, path = "/account/keys/{id}", tag = TAG,
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 404, description = "Not one of your keys"),
    ),
)]
async fn delete_key(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let me = identity.require_user()?;
    let key = own_key(&state, me, &id).await?;

    repo::client::delete(&state.db, &id).await?;
    if let Err(e) = state
        .forget_settings(crate::settings::Scope::Client, &id)
        .await
    {
        tracing::warn!(key = %id, error = %e, "a revoked key's settings were left behind");
    }

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ClientRevoked,
            target: Some(&key.name),
            detail: Some(&format!("{}'s", me.username)),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::user::Status;

    fn person(role: Role) -> User {
        User {
            id: "u1".into(),
            username: "margaux".into(),
            display_name: None,
            email: None,
            role,
            status: Status::Active,
            locale: None,
            oidc_linked: false,
            has_password: true,
            invited_by: None,
            created_at: "2026-09-26T00:00:00Z".into(),
            updated_at: None,
            last_login_at: None,
        }
    }

    #[test]
    fn a_key_carries_no_more_than_its_owner_s_role() {
        let member = person(Role::Member);
        assert_eq!(granted(&member, &[]).unwrap(), vec!["read"]);
        assert!(granted(&member, &["write".into()]).is_err());

        let editor = person(Role::Editor);
        assert_eq!(
            granted(&editor, &["write".into(), "read".into()]).unwrap(),
            vec!["read", "write"]
        );
        assert!(granted(&editor, &["admin".into()]).is_err());

        let admin = person(Role::Admin);
        assert!(granted(&admin, &["admin".into()]).is_ok());
        assert!(granted(&admin, &["root".into()]).is_err());
    }

    #[test]
    fn a_key_name_is_short_and_not_blank() {
        assert_eq!(
            clean_key_name("  Sonarr du salon ").unwrap(),
            "Sonarr du salon"
        );
        assert!(clean_key_name("   ").is_err());
        assert!(clean_key_name(&"k".repeat(61)).is_err());
    }
}
