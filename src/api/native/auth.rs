//! Administrator sign-in.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    auth::{Identity, middleware::SESSION_COOKIE, secrets},
    db::repo,
    error::{AppError, AppResult},
    state::AppState,
};

/// How long a session lasts. Long enough not to interrupt a curation session,
/// short enough that a forgotten browser tab stops working.
const SESSION_TTL_HOURS: i64 = 12;

pub fn public_router() -> Router<AppState> {
    Router::new().route("/auth/login", post(login))
}

pub fn authenticated_router() -> Router<AppState> {
    Router::new()
        .route("/auth/me", get(me))
        .route("/auth/logout", post(logout))
        .route("/auth/password", post(change_password))
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub username: String,
    pub expires_at: String,
}

async fn login(
    State(state): State<AppState>,
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
        return Err(AppError::Unauthorized);
    };

    let (token, token_hash) = secrets::generate_session_token()?;

    let expires_at = repo::user::create_session(
        &state.db,
        &token_hash,
        &user.id,
        chrono::Duration::hours(SESSION_TTL_HOURS),
        headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()),
        None,
    )
    .await?;

    repo::user::mark_login(&state.db, &user.id).await?;

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

async fn logout(
    State(state): State<AppState>,
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

    let cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");

    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub identity: String,
    pub can_write: bool,
    pub is_admin: bool,
}

async fn me(Extension(identity): Extension<Identity>) -> Json<MeResponse> {
    Json(MeResponse {
        identity: identity.label(),
        can_write: identity.can_write(),
        is_admin: identity.is_admin(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
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

    Ok(StatusCode::NO_CONTENT)
}
