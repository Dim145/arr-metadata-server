//! Accounts, as an administrator manages them.
//!
//! Every account has a role — member, editor, administrator — and a status:
//! active, pending (signed up where sign-ups wait for approval), or disabled.
//! Two things are never allowed, whoever asks: leaving the server without an
//! active administrator, and an administrator demoting, disabling or deleting
//! themselves — both would lock the door with nobody inside.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::{Identity, secrets},
    db::repo::{
        self,
        audit::Action,
        client::ApiClient,
        user::{Role, Session, Status, User},
    },
    error::{AppError, AppResult},
    state::AppState,
};

pub const TAG: &str = "Users";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(detail, update, remove))
        .routes(routes!(reset_password))
        .routes(routes!(revoke_sessions))
        .routes(routes!(bulk))
}

// ─── shared rules ────────────────────────────────────────────────────────────

/// A username as typed, or why it cannot be one: two to forty letters, digits,
/// dots, dashes or underscores. Compared without regard to case.
pub(crate) fn clean_username(raw: &str) -> AppResult<String> {
    let name = raw.trim();
    let valid = (2..=40).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));

    valid.then(|| name.to_string()).ok_or_else(|| {
        AppError::BadRequest(
            "a username is 2 to 40 letters, digits, dots, dashes or underscores".into(),
        )
    })
}

/// A display name, or nothing when left blank.
pub(crate) fn clean_display_name(raw: Option<&str>) -> AppResult<Option<String>> {
    match raw.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) if name.chars().count() > 80 => Err(AppError::BadRequest(
            "a display name is at most 80 characters".into(),
        )),
        other => Ok(other.map(String::from)),
    }
}

/// An e-mail address, or nothing when left blank. Checked for shape only:
/// nothing is sent to it.
pub(crate) fn clean_email(raw: Option<&str>) -> AppResult<Option<String>> {
    let Some(email) = raw.map(str::trim).filter(|e| !e.is_empty()) else {
        return Ok(None);
    };

    let shaped = email.len() <= 254
        && !email.chars().any(char::is_whitespace)
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));

    shaped
        .then(|| Some(email.to_string()))
        .ok_or_else(|| AppError::BadRequest(format!("{email:?} is not an e-mail address")))
}

/// The interface's language, when chosen: a short tag such as `fr`.
pub(crate) fn clean_locale(raw: Option<&str>) -> AppResult<Option<String>> {
    match raw.map(str::trim).filter(|l| !l.is_empty()) {
        None => Ok(None),
        Some(tag)
            if tag.len() <= 10 && tag.chars().all(|c| c.is_ascii_alphabetic() || c == '-') =>
        {
            Ok(Some(tag.to_ascii_lowercase()))
        }
        Some(tag) => Err(AppError::BadRequest(format!(
            "{tag:?} is not a language tag"
        ))),
    }
}

/// A password for someone to type once and change: twenty characters from
/// the same generator as the session tokens.
pub(crate) fn generated_password() -> AppResult<String> {
    let (token, _) = secrets::generate_session_token()?;
    Ok(token.chars().take(20).collect())
}

/// The password an administrator typed, or one made for them — and in the
/// second case, the copy to show them once. Bounded as signing in is: a
/// password longer than sign-in accepts is an account nobody can enter.
fn given_or_generated(given: Option<String>) -> AppResult<(String, Option<String>)> {
    match given.filter(|p| !p.is_empty()) {
        Some(given) if given.len() > crate::api::native::auth::MAX_CREDENTIAL => {
            Err(AppError::BadRequest("that password is too long".into()))
        }
        Some(given) => Ok((given, None)),
        None => {
            let made = generated_password()?;
            Ok((made.clone(), Some(made)))
        }
    }
}

/// Whether taking `target` out of the administrators — by a new role, a
/// status, or deletion — would leave nobody to administer the server.
async fn would_orphan(state: &AppState, target: &User) -> AppResult<bool> {
    Ok(target.role == Role::Admin
        && target.status == Status::Active
        && repo::user::count_active_admins(&state.db, Some(&target.id)).await? == 0)
}

fn last_admin() -> AppError {
    AppError::Conflict("the server needs at least one active administrator".into())
}

fn yourself() -> AppError {
    AppError::Conflict("you cannot take your own rights away; another administrator can".into())
}

// ─── listing ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub term: Option<String>,
    /// `admin`, `editor` or `member`.
    pub role: Option<String>,
    /// `active`, `pending` or `disabled`.
    pub status: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub offset: Option<i64>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsersPage {
    pub users: Vec<repo::user::Listed>,
    pub total: i64,
    pub counts: repo::user::Counts,
}

/// Every account, pending ones first.
#[utoipa::path(
    get, path = "/users", tag = TAG,
    params(ListQuery),
    responses(
        (status = 200, body = UsersPage),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<UsersPage>> {
    identity.require_admin()?;

    fn parse(raw: Option<&str>) -> Option<&str> {
        raw.map(str::trim).filter(|v| !v.is_empty() && *v != "all")
    }
    let q = repo::user::Query {
        term: query.term.clone(),
        role: parse(query.role.as_deref())
            .map(str::parse::<Role>)
            .transpose()
            .map_err(|e| AppError::BadRequest(e.to_string()))?,
        status: parse(query.status.as_deref())
            .map(str::parse::<Status>)
            .transpose()
            .map_err(|e| AppError::BadRequest(e.to_string()))?,
        limit: query.limit.unwrap_or(100),
        offset: query.offset.unwrap_or(0),
    };

    let (users, total) = repo::user::list(&state.db, &q).await?;
    let counts = repo::user::counts(&state.db).await?;

    Ok(Json(UsersPage {
        users,
        total,
        counts,
    }))
}

// ─── one account ─────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserDetail {
    pub user: User,
    pub keys: Vec<ApiClient>,
    pub sessions: Vec<Session>,
}

#[utoipa::path(
    get, path = "/users/{id}", tag = TAG,
    params(("id" = String, Path)),
    responses(
        (status = 200, body = UserDetail),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404),
    ),
)]
async fn detail(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<UserDetail>> {
    identity.require_admin()?;

    let user = repo::user::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let keys = repo::client::list(&state.db, repo::client::Owner::User(&id)).await?;
    let sessions = repo::user::list_sessions(&state.db, &id, None).await?;

    Ok(Json(UserDetail {
        user,
        keys,
        sessions,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub username: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub role: Role,
    /// Twelve characters at least. Left out, one is generated and returned —
    /// once.
    pub password: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedUser {
    pub user: User,
    /// The generated password, when none was given. Shown once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Open an account for someone.
#[utoipa::path(
    post, path = "/users", tag = TAG,
    request_body = CreateUserRequest,
    responses(
        (status = 201, body = CreatedUser),
        (status = 400, description = "A field was rejected"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "That username is taken"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<CreateUserRequest>,
) -> AppResult<(StatusCode, Json<CreatedUser>)> {
    identity.require_admin()?;

    let username = clean_username(&request.username)?;
    let display_name = clean_display_name(request.display_name.as_deref())?;
    let email = clean_email(request.email.as_deref())?;

    if repo::user::find_by_username(&state.db, &username)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(format!(
            "the username {username:?} is taken"
        )));
    }

    let (password, generated) = given_or_generated(request.password)?;
    let hash = secrets::hash_password_async(password)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let user = repo::user::create(
        &state.db,
        repo::user::NewUser {
            username: &username,
            password_hash: &hash,
            role: request.role,
            status: Status::Active,
            display_name: display_name.as_deref(),
            email: email.as_deref(),
            invited_by: identity.user().map(|u| u.id.as_str()),
            oidc: None,
        },
    )
    .await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::UserCreated,
            target: Some(&user.username),
            detail: Some(user.role.as_str()),
        },
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(CreatedUser {
            user,
            password: generated,
        }),
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    /// Blank clears it.
    pub display_name: Option<String>,
    /// Blank clears it.
    pub email: Option<String>,
    pub role: Option<Role>,
    pub status: Option<Status>,
}

/// Change an account: what it is called, its role, its status.
///
/// Disabling an account, or sending it back to pending, closes its sessions
/// at once; its keys stop working with it.
#[utoipa::path(
    patch, path = "/users/{id}", tag = TAG,
    params(("id" = String, Path)),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, body = User),
        (status = 400, description = "A field was rejected"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404),
        (status = 409, description = "It would leave no administrator, or it is your own account"),
    ),
)]
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> AppResult<Json<User>> {
    identity.require_admin()?;

    // Every field is checked before anything is written, so a request refused
    // for its e-mail has not already changed a role on the way.
    let display_name = request
        .display_name
        .as_deref()
        .map(|raw| clean_display_name(Some(raw)))
        .transpose()?;
    let email = request
        .email
        .as_deref()
        .map(|raw| clean_email(Some(raw)))
        .transpose()?;

    let _held = state.accounts_lock().await;

    let target = repo::user::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    let changes = change(&state, &identity, &target, request.role, request.status).await?;

    if display_name.is_some() || email.is_some() {
        let display_name = display_name.unwrap_or_else(|| target.display_name.clone());
        let email = email.unwrap_or_else(|| target.email.clone());
        repo::user::update_profile(
            &state.db,
            &id,
            display_name.as_deref(),
            email.as_deref(),
            target.locale.as_deref(),
        )
        .await?;
    }

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::UserUpdated,
            target: Some(&target.username),
            detail: (!changes.is_empty()).then_some(changes.as_str()),
        },
    )
    .await;

    repo::user::get(&state.db, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// Apply a new role or status to `target`, within the two rules; returns what
/// changed, for the journal. The caller holds the accounts lock.
async fn change(
    state: &AppState,
    identity: &Identity,
    target: &User,
    role: Option<Role>,
    status: Option<Status>,
) -> AppResult<String> {
    let myself = identity.person_id() == Some(target.id.as_str());
    let demoted = role.is_some_and(|r| r != Role::Admin) && target.role == Role::Admin;
    let stopped = status.is_some_and(|s| s != Status::Active) && target.status == Status::Active;

    if myself && (demoted || stopped) {
        return Err(yourself());
    }
    if (demoted || stopped) && would_orphan(state, target).await? {
        return Err(last_admin());
    }

    let mut changes = Vec::new();

    if let Some(role) = role.filter(|r| *r != target.role) {
        repo::user::set_role(&state.db, &target.id, role).await?;
        changes.push(format!("role {} → {}", target.role, role.as_str()));
    }

    if let Some(status) = status.filter(|s| *s != target.status) {
        repo::user::set_status(&state.db, &target.id, status).await?;
        if status != Status::Active {
            repo::user::delete_sessions_for_user(&state.db, &target.id).await?;
        }
        changes.push(format!(
            "status {} → {}",
            target.status.as_str(),
            status.as_str()
        ));
    }

    Ok(changes.join(", "))
}

/// Close an account for good: its sessions and keys go with it. What it did
/// stays in the journal, under its name.
#[utoipa::path(
    delete, path = "/users/{id}", tag = TAG,
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404),
        (status = 409, description = "It is the last administrator, or your own account"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;

    let _held = state.accounts_lock().await;

    let target = repo::user::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    delete_one(&state, &identity, &ip, &target).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// The caller holds the accounts lock.
async fn delete_one(
    state: &AppState,
    identity: &Identity,
    ip: &ClientIp,
    target: &User,
) -> AppResult<()> {
    if identity.person_id() == Some(target.id.as_str()) {
        return Err(yourself());
    }
    if would_orphan(state, target).await? {
        return Err(last_admin());
    }

    // The keys go by cascade; the settings hung off them would be left behind.
    for key in repo::client::list(&state.db, repo::client::Owner::User(&target.id)).await? {
        if let Err(e) = state
            .forget_settings(crate::settings::Scope::Client, &key.id)
            .await
        {
            tracing::warn!(key = %key.id, error = %e, "a deleted key's settings were left behind");
        }
    }

    // An invitation made by someone who is no longer an administrator is void
    // already; one made by someone deleted would lose its maker, and with it
    // the check, so it is withdrawn first.
    repo::invitation::revoke_by_creator(&state.db, &target.id).await?;
    repo::user::delete(&state.db, &target.id).await?;

    audit::record(
        state,
        Event {
            identity: Some(identity),
            ip,
            action: Action::UserDeleted,
            target: Some(&target.username),
            detail: Some(target.role.as_str()),
        },
    )
    .await;

    Ok(())
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordRequest {
    /// Left out, one is generated and returned — once.
    pub password: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetPassword {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Give an account a new password; its sessions are closed.
#[utoipa::path(
    post, path = "/users/{id}/password", tag = TAG,
    params(("id" = String, Path)),
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, body = ResetPassword),
        (status = 400, description = "The password was rejected"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404),
    ),
)]
async fn reset_password(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<ResetPasswordRequest>,
) -> AppResult<Json<ResetPassword>> {
    identity.require_admin()?;

    let target = repo::user::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Your own is changed from your account, which asks for the current one:
    // a session left open must not be enough to lock its owner out.
    if identity.person_id() == Some(target.id.as_str()) {
        return Err(AppError::Conflict(
            "change your own password from your account page".into(),
        ));
    }

    let (password, generated) = given_or_generated(request.password)?;
    let hash = secrets::hash_password_async(password)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    repo::user::set_password(&state.db, &id, &hash).await?;
    repo::user::delete_sessions_for_user(&state.db, &id).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::UserPasswordReset,
            target: Some(&target.username),
            detail: Some("sessions closed"),
        },
    )
    .await;

    Ok(Json(ResetPassword {
        password: generated,
    }))
}

/// Close every session of an account, wherever it is signed in.
#[utoipa::path(
    delete, path = "/users/{id}/sessions", tag = TAG,
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404),
    ),
)]
async fn revoke_sessions(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;

    let target = repo::user::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let closed = repo::user::delete_sessions_for_user(&state.db, &id).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::SessionsRevoked,
            target: Some(&target.username),
            detail: Some(&format!("{closed} closed")),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkRequest {
    pub ids: Vec<String>,
    /// `role`, `status` or `delete`.
    pub action: String,
    /// The role or status, for those two.
    pub value: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Refusal {
    pub id: String,
    pub reason: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkOutcome {
    pub done: Vec<String>,
    pub refused: Vec<Refusal>,
}

/// One change to several accounts at once. Each is checked on its own, so
/// your own account or the last administrator in a selection is left out and
/// said, rather than failing the rest.
#[utoipa::path(
    post, path = "/users/bulk", tag = TAG,
    request_body = BulkRequest,
    responses(
        (status = 200, body = BulkOutcome),
        (status = 400, description = "An unknown action or value"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn bulk(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<BulkRequest>,
) -> AppResult<Json<BulkOutcome>> {
    identity.require_admin()?;

    if request.ids.len() > 500 {
        return Err(AppError::BadRequest("at most 500 accounts at once".into()));
    }

    let value = request.value.as_deref().unwrap_or_default();
    let (role, status) = match request.action.as_str() {
        "role" => (
            Some(
                value
                    .parse::<Role>()
                    .map_err(|e| AppError::BadRequest(e.to_string()))?,
            ),
            None,
        ),
        "status" => (
            None,
            Some(
                value
                    .parse::<Status>()
                    .map_err(|e| AppError::BadRequest(e.to_string()))?,
            ),
        ),
        "delete" => (None, None),
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown action {other:?}: role, status or delete"
            )));
        }
    };

    let mut outcome = BulkOutcome {
        done: Vec::new(),
        refused: Vec::new(),
    };

    let _held = state.accounts_lock().await;

    for id in &request.ids {
        let Some(target) = repo::user::get(&state.db, id).await? else {
            outcome.refused.push(Refusal {
                id: id.clone(),
                reason: "no such account".into(),
            });
            continue;
        };

        let result = if request.action == "delete" {
            delete_one(&state, &identity, &ip, &target).await
        } else {
            match change(&state, &identity, &target, role, status).await {
                Ok(changes) => {
                    if !changes.is_empty() {
                        audit::record(
                            &state,
                            Event {
                                identity: Some(&identity),
                                ip: &ip,
                                action: Action::UserUpdated,
                                target: Some(&target.username),
                                detail: Some(&changes),
                            },
                        )
                        .await;
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        };

        match result {
            Ok(()) => outcome.done.push(id.clone()),
            Err(AppError::Conflict(reason)) => outcome.refused.push(Refusal {
                id: id.clone(),
                reason,
            }),
            Err(other) => return Err(other),
        }
    }

    Ok(Json(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_short_and_plain() {
        assert_eq!(clean_username("  margaux ").unwrap(), "margaux");
        assert_eq!(clean_username("dim.145-x_y").unwrap(), "dim.145-x_y");
        assert!(clean_username("a").is_err());
        assert!(clean_username("with space").is_err());
        assert!(clean_username("émile").is_err());
        assert!(clean_username(&"x".repeat(41)).is_err());
    }

    #[test]
    fn an_email_is_checked_for_shape_and_a_blank_one_is_none() {
        assert_eq!(
            clean_email(Some(" leo@example.org ")).unwrap().as_deref(),
            Some("leo@example.org")
        );
        assert_eq!(clean_email(Some("   ")).unwrap(), None);
        assert_eq!(clean_email(None).unwrap(), None);
        assert!(clean_email(Some("leo@localhost")).is_err());
        assert!(clean_email(Some("no-at.example.org")).is_err());
        assert!(clean_email(Some("two words@example.org")).is_err());
    }

    #[test]
    fn a_display_name_and_a_locale_are_bounded() {
        assert_eq!(clean_display_name(Some("  ")).unwrap(), None);
        assert!(clean_display_name(Some(&"é".repeat(81))).is_err());
        assert_eq!(clean_locale(Some("FR")).unwrap().as_deref(), Some("fr"));
        assert_eq!(
            clean_locale(Some("pt-BR")).unwrap().as_deref(),
            Some("pt-br")
        );
        assert!(clean_locale(Some("fr;drop")).is_err());
    }

    #[test]
    fn a_generated_password_is_long_enough_to_be_accepted() {
        let password = generated_password().unwrap();
        assert_eq!(password.chars().count(), 20);
        assert!(secrets::hash_password(&password).is_ok());
    }
}
