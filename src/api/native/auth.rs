//! Signing in and out, for every account.

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

/// The longest a username or password may be, in bytes.
///
/// Generous for anything a person would type or a password manager would make,
/// and short enough that the work of hashing it and recording the attempt is
/// bounded. The body limit alone is a megabyte, which is not a bound at all.
pub(crate) const MAX_CREDENTIAL: usize = 256;

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
    pub role: repo::user::Role,
    /// Whether the account may maintain the catalogue, which decides where the
    /// interface takes it next.
    pub can_write: bool,
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
        (status = 403, description = "The account is waiting for approval, or disabled"),
    ),
    security(),
)]
async fn login(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    // Refused before anything is looked up or hashed. Nobody's credentials are
    // this long, so the only thing that sends them is something trying to make
    // this server do expensive work — a megabyte of username is a megabyte of
    // log line and a megabyte of audit row, and neither needs a credential.
    if request.username.len() > MAX_CREDENTIAL || request.password.len() > MAX_CREDENTIAL {
        return Err(AppError::Unauthorized);
    }

    // Passwords switched off: the identity provider signs people in, and only
    // the account the environment names keeps a door — the way back in when
    // the provider is down, and only while it is an active administrator.
    // Every refusal then reads the same, a wrong password on that door
    // included, so nobody learns which name it is.
    let passwords_off = !state.password_login();
    let refused = || AppError::Refused {
        code: "password_login_off",
        message: "this server signs people in through its identity provider".into(),
    };
    if passwords_off && !state.is_break_glass_name(&request.username) {
        return Err(refused());
    }

    let found = repo::user::find_by_username(&state.db, request.username.trim()).await?;

    // Verify even when the user does not exist, so a wrong username and a wrong
    // password take the same time and cannot be told apart.
    let (user, ok) = match found {
        // An account an identity provider made has no password: checked
        // against the dummy all the same, so it takes as long as any other.
        Some(creds) if creds.password_hash == repo::user::NO_PASSWORD => {
            let _ =
                secrets::verify_password_async(request.password.clone(), DUMMY_HASH.to_string())
                    .await;
            (Some(creds.user), false)
        }
        Some(creds) => {
            let ok =
                secrets::verify_password_async(request.password.clone(), creds.password_hash).await;
            (Some(creds.user), ok)
        }
        None => {
            let _ =
                secrets::verify_password_async(request.password.clone(), DUMMY_HASH.to_string())
                    .await;
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

        return Err(if passwords_off {
            refused()
        } else {
            AppError::Unauthorized
        });
    };

    if passwords_off
        && !(user.role == repo::user::Role::Admin && user.status == repo::user::Status::Active)
    {
        return Err(refused());
    }

    // Told only to whoever proved the password: the account exists, and why it
    // may not come in yet.
    match user.status {
        repo::user::Status::Active => {}
        repo::user::Status::Pending => {
            return Err(AppError::Refused {
                code: "account_pending",
                message: "this account is waiting for an administrator to approve it".into(),
            });
        }
        repo::user::Status::Disabled => {
            return Err(AppError::Refused {
                code: "account_disabled",
                message: "this account has been disabled".into(),
            });
        }
    }

    let (cookie, answer) = open_session(&state, user, &headers, &ip).await?;

    Ok((StatusCode::OK, [(header::SET_COOKIE, cookie)], Json(answer)))
}

/// Open a session for someone who has shown who they are — by their password,
/// by signing up, or through the identity provider — and say so in the
/// journal. Returns the cookie to set and what the interface is told.
pub(crate) async fn open_session(
    state: &AppState,
    user: repo::user::User,
    headers: &HeaderMap,
    ip: &ClientIp,
) -> AppResult<(String, LoginResponse)> {
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

    let identity = Identity::User(Box::new(user.clone()));
    audit::record(
        state,
        Event {
            identity: Some(&identity),
            ip,
            action: Action::SignedIn,
            target: Some(&user.username),
            detail: None,
        },
    )
    .await;

    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict{}; Max-Age={}",
        secure_flag(state),
        SESSION_TTL_HOURS * 3600
    );

    Ok((
        cookie,
        LoginResponse {
            can_write: identity.can_write(),
            role: user.role,
            username: user.username,
            expires_at,
        },
    ))
}

/// `; Secure` when this session can only have arrived over TLS.
///
/// Not unconditional: plenty of these run as plain HTTP on a home network, and
/// a `Secure` cookie there is a cookie the browser never sends back — an admin
/// who can sign in and is then immediately signed out again, with nothing to
/// explain it. Set where it can be honoured: this server terminating TLS
/// itself, or a public URL that says `https` because something in front of it
/// does.
pub(crate) fn secure_flag(state: &AppState) -> &'static str {
    let terminates_tls = state.config.server.tls.is_some();
    let published_over_tls = state
        .config
        .server
        .public_url
        .as_deref()
        .is_some_and(|url| url.starts_with("https://"));

    match terminates_tls || published_over_tls {
        true => "; Secure",
        false => "",
    }
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
    /// The signed-in person, when this is a session rather than a key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<MeUser>,
    /// Whether a reader with no credential may browse the catalogue — and so
    /// whether an address handed to a calendar app or a feed reader, which
    /// carries none, will answer.
    pub public_browse: bool,
}

/// The signed-in person, as the interface needs them on every page.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeUser {
    pub id: String,
    pub username: String,
    /// What to call them: the name they gave, or their username.
    pub name: String,
    pub role: repo::user::Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    pub has_password: bool,
}

/// Who this request is authenticated as, and what it may do.
#[utoipa::path(
    get, path = "/auth/me", tag = TAG,
    responses(
        (status = 200, body = MeResponse),
        (status = 401, description = "No valid credential was presented"),
    ),
)]
async fn me(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Json<MeResponse> {
    Json(MeResponse {
        identity: identity.label(),
        can_write: identity.can_write(),
        is_admin: identity.is_admin(),
        user: identity.user().map(|u| MeUser {
            id: u.id.clone(),
            username: u.username.clone(),
            name: u.name().to_string(),
            role: u.role,
            locale: u.locale.clone(),
            has_password: u.has_password,
        }),
        public_browse: state.public_site(),
    })
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Change the signed-in person's password.
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
        (status = 403, description = "An API key, or an account an identity provider made, has no password to change"),
    ),
)]
async fn change_password(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<ChangePasswordRequest>,
) -> AppResult<StatusCode> {
    // An API key cannot change a password: it has no password to change, and
    // letting it set one would be a privilege escalation.
    let user = identity.require_user()?;

    let Some(creds) = repo::user::credentials(&state.db, &user.id).await? else {
        return Err(AppError::NotFound);
    };

    // One the identity provider vouches for has no password to prove.
    if creds.password_hash == repo::user::NO_PASSWORD {
        return Err(AppError::Refused {
            code: "no_password",
            message: "this account signs in through its identity provider and has no password"
                .into(),
        });
    }

    if request.current_password.len() > MAX_CREDENTIAL
        || request.new_password.len() > MAX_CREDENTIAL
    {
        return Err(AppError::BadRequest("that password is too long".into()));
    }

    if !secrets::verify_password_async(request.current_password.clone(), creds.password_hash).await
    {
        return Err(AppError::Unauthorized);
    }

    let hash = secrets::hash_password_async(request.new_password.clone())
        .await
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
