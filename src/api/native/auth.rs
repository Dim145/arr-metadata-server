//! Administrator sign-in.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::{Identity, middleware::SESSION_COOKIE, secrets},
    db::repo::{self, audit::Action},
    error::{AppError, AppResult},
    state::AppState,
};

/// How long a session lasts. Long enough not to interrupt a curation session,
/// short enough that a forgotten browser tab stops working.
const SESSION_TTL_HOURS: i64 = 12;

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Session";

/// Reachable without a credential — nothing else could ever obtain one.
pub fn public_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(login))
}

pub fn authenticated_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(me))
        .routes(routes!(logout))
        .routes(routes!(change_password))
}

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub username: String,
    pub expires_at: String,
}

/// Sign in and receive a session cookie.
///
/// A wrong username and a wrong password are indistinguishable, by design: the
/// password is verified either way so the two take the same time.
#[utoipa::path(
    post, path = "/auth/login", tag = TAG,
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Signed in; a session cookie is set", body = LoginResponse),
        (status = 401, description = "Those credentials were not accepted"),
    ),
    security(),
)]
async fn login(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    let found = repo::user::find_by_username(&state.db, request.username.trim()).await?;

    // Verify even when the user does not exist, so a wrong username and a wrong
    // password take the same time and cannot be told apart.
    let (user, ok) = match found {
        Some(creds) => {
            let ok = secrets::verify_password(&request.password, &creds.password_hash);
            (Some(creds.user), ok)
        }
        None => {
            let _ = secrets::verify_password(&request.password, DUMMY_HASH);
            (None, false)
        }
    };

    let (Some(user), true) = (user, ok) else {
        tracing::warn!(username = %request.username, "failed sign-in attempt");

        // Recorded deliberately: a run of these is the one thing in this log
        // worth alerting on. The attempted username is kept; the password is not.
        audit::record(
            &state,
            Event {
                identity: None,
                ip: &ip,
                action: Action::SignInFailed,
                target: Some(request.username.trim()),
                detail: None,
            },
        )
        .await;

        return Err(AppError::Unauthorized);
    };

    let (token, token_hash) = secrets::generate_session_token()?;

    let expires_at = repo::user::create_session(
        &state.db,
        &token_hash,
        &user.id,
        chrono::Duration::hours(SESSION_TTL_HOURS),
        headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok()),
        ip.as_text().as_deref(),
    )
    .await?;

    repo::user::mark_login(&state.db, &user.id).await?;

    let identity = Identity::Admin(Box::new(user.clone()));
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::SignedIn,
            target: Some(&user.username),
            detail: None,
        },
    )
    .await;

    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        SESSION_TTL_HOURS * 3600
    );

    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(LoginResponse {
            username: user.username,
            expires_at,
        }),
    ))
}

/// An argon2id hash of a value nobody knows, used to equalise timing on the
/// unknown-user path. Verifying against it costs the same as a real check.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$\
                          YQqCqZ1bQZ3vLQ4mJ0Xz0xKZ8p1n3sVQ1kJ2m9Y7bWc";

/// Sign out, invalidating this session server-side.
#[utoipa::path(
    post, path = "/auth/logout", tag = TAG,
    responses((status = 204, description = "Signed out")),
)]
async fn logout(
    State(state): State<AppState>,
    identity: Option<Extension<Identity>>,
    ip: ClientIp,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    if let Some(token) = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.trim().to_string())
    {
        repo::user::delete_session(&state.db, &secrets::hash_api_key(&token)).await?;
    }

    audit::record(
        &state,
        Event {
            identity: identity.as_ref().map(|Extension(i)| i),
            ip: &ip,
            action: Action::SignedOut,
            target: None,
            detail: None,
        },
    )
    .await;

    let cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");

    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub identity: String,
    pub can_write: bool,
    pub is_admin: bool,
}

/// Who this request is authenticated as, and what it may do.
#[utoipa::path(
    get, path = "/auth/me", tag = TAG,
    responses(
        (status = 200, body = MeResponse),
        (status = 401, description = "No valid credential was presented"),
    ),
)]
async fn me(Extension(identity): Extension<Identity>) -> Json<MeResponse> {
    Json(MeResponse {
        identity: identity.label(),
        can_write: identity.can_write(),
        is_admin: identity.is_admin(),
    })
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Change the signed-in administrator's password.
///
/// Every existing session is invalidated: they were all authorised under the
/// old password.
#[utoipa::path(
    post, path = "/auth/password", tag = TAG,
    request_body = ChangePasswordRequest,
    responses(
        (status = 204, description = "Changed; sign in again"),
        (status = 400, description = "The new password was rejected"),
        (status = 401, description = "The current password was wrong"),
        (status = 403, description = "An API key has no password to change"),
    ),
)]
async fn change_password(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<ChangePasswordRequest>,
) -> AppResult<StatusCode> {
    let Identity::Admin(user) = &identity else {
        // An API key cannot change a password: it has no password to change,
        // and letting it set one would be a privilege escalation.
        return Err(AppError::Forbidden);
    };

    let Some(creds) = repo::user::find_by_username(&state.db, &user.username).await? else {
        return Err(AppError::NotFound);
    };

    if !secrets::verify_password(&request.current_password, &creds.password_hash) {
        return Err(AppError::Unauthorized);
    }

    let hash = secrets::hash_password(&request.new_password)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    repo::user::set_password(&state.db, &user.id, &hash).await?;

    // Every existing session was authorised under the old password.
    repo::user::delete_sessions_for_user(&state.db, &user.id).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::PasswordChanged,
            target: Some(&user.username),
            detail: Some("all sessions invalidated"),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}
