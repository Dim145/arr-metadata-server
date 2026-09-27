//! Signing in through the identity provider, and what an administrator sets
//! and checks of it.
//!
//! `/auth/oidc/start` sends the browser to the provider, which sends it back
//! to `/auth/oidc/callback`. There the account is found by the provider's own
//! name for the person — or tied, when a signed-in person asked for it from
//! their account page, or opened, where allowed. Nothing is ever tied by an
//! e-mail address: an address a person typed into their own profile proves
//! nothing, and tying on it would hand one person's account to another.
//!
//! The answer is a page rather than a redirect: the session cookie is
//! SameSite=Strict, and a cookie set at the end of a chain of redirects that
//! began at another site is not sent with the next request of that chain. A
//! page that moves on by itself makes a request of its own, from this site.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
        native::{auth, users},
    },
    auth::{
        Identity, middleware,
        oidc::{self, Flow, Refusal, Vouched},
    },
    db::repo::{
        self,
        audit::Action,
        user::{NO_PASSWORD, Role, Status, User},
    },
    error::{AppError, AppResult},
    settings::Scope,
    state::{AppState, Registration},
};

/// Where a flow's cookie is sent when the site is not served over HTTPS: the
/// two routes of the flow, and nowhere else. (Over HTTPS it is a `__Host-`
/// cookie, which must be sent everywhere.)
const FLOW_PATH: &str = "/api/v1/auth/oidc";

/// Reachable without a credential: they are how one is obtained.
pub fn public_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(start))
        .routes(routes!(callback))
}

/// An administrator's.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(configuration, configure))
        .routes(routes!(test))
        .routes(routes!(unlink))
}

fn secure(state: &AppState) -> bool {
    !auth::secure_flag(state).is_empty()
}

// ─── the flow ────────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StartQuery {
    /// The page of this site to come back to.
    pub next: Option<String>,
    /// Set by a signed-in person tying their account to the provider.
    pub link: Option<String>,
}

/// Go to the identity provider to sign in — or, with `link`, to tie the
/// signed-in account to the person the provider knows.
#[utoipa::path(
    get, path = "/auth/oidc/start", tag = auth::TAG,
    params(StartQuery),
    responses(
        (status = 303, description = "To the identity provider, or back to the sign-in page saying why not"),
    ),
    security(),
)]
async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StartQuery>,
) -> Response {
    match begin(&state, &headers, query).await {
        Ok((jar, url)) => (StatusCode::SEE_OTHER, jar, [(header::LOCATION, url)]).into_response(),
        Err(failure) => {
            tracing::info!(reason = failure.code(), detail = %failure, "a sign-in through the identity provider could not start");
            to_login(failure.code(), None)
        }
    }
}

async fn begin(
    state: &AppState,
    headers: &HeaderMap,
    query: StartQuery,
) -> Result<(PrivateCookieJar, String), Failure> {
    let provider = state.oidc_provider().ok_or(Failure::Unavailable)?;

    // Tying needs the person to be signed in already, here: the session
    // cookie is Strict, so another site cannot start this in their name.
    let link = match query.link.as_deref().filter(|l| !l.is_empty()) {
        None => None,
        Some(_) => Some(
            middleware::session_user(state, headers)
                .await
                .map_err(internal)?
                .ok_or(Failure::SignInFirst)?
                .id,
        ),
    };

    let started = oidc::start(&provider, safe_next(query.next.as_deref()), link)
        .await
        .map_err(|e| Failure::Provider(format!("{e:#}")))?;

    let secure = secure(state);
    let value = serde_json::to_string(&started.flow).map_err(internal)?;
    let cookie = Cookie::build((oidc::cookie_name(&started.state, secure), value))
        .path(if secure { "/" } else { FLOW_PATH })
        .http_only(true)
        // Lax, not Strict: the browser comes back from the provider's site,
        // and must bring this with it.
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(oidc::FLOW_TTL.as_secs() as i64));
    // Secure wherever the site is reached in TLS; on a home network reached
    // in plain HTTP the browser would never send a Secure cookie back, and
    // the sign-in could not complete at all.
    let cookie = if secure { cookie.secure(true) } else { cookie }.build();

    Ok((
        PrivateCookieJar::new(state.cookie_key.clone()).add(cookie),
        started.url,
    ))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    /// Set by the provider when it did not sign the person in.
    pub error: Option<String>,
}

/// Where the identity provider sends the browser back.
///
/// Signed in, a page that moves on to where the person was going; refused,
/// the sign-in page, told why in a word (`?sso=`).
#[utoipa::path(
    get, path = "/auth/oidc/callback", tag = auth::TAG,
    params(CallbackQuery),
    responses(
        (status = 200, description = "Signed in; a session cookie is set, and the page moves on"),
        (status = 303, description = "Refused: back to the sign-in page"),
    ),
    security(),
)]
async fn callback(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let secure = secure(&state);
    let jar = PrivateCookieJar::from_headers(&headers, state.cookie_key.clone());

    // The flow this return belongs to, by the state it carries; spent
    // whatever happens next.
    let name = query
        .state
        .as_deref()
        .map(|returned| oidc::cookie_name(returned, secure));
    let flow = name
        .as_deref()
        .and_then(|name| jar.get(name))
        .and_then(|cookie| serde_json::from_str::<Flow>(cookie.value()).ok());
    let jar = match name {
        Some(name) => jar.remove(Cookie::build(name).path(if secure { "/" } else { FLOW_PATH })),
        None => jar,
    };

    match complete(&state, &ip, &headers, query, flow).await {
        Ok((session, next)) => (
            jar,
            [
                (header::SET_COOKIE, session),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            onward(&next),
        )
            .into_response(),
        Err(failure) => {
            tracing::info!(reason = failure.code(), detail = %failure, "a sign-in through the identity provider was refused");
            audit::record(
                &state,
                Event {
                    identity: None,
                    ip: &ip,
                    action: Action::SignInFailed,
                    target: Some("identity provider"),
                    // The word, and what the provider or the check said:
                    // what an administrator reading the journal needs.
                    detail: Some(&format!(
                        "{}: {}",
                        failure.code(),
                        clean(&failure.to_string(), 200)
                    )),
                },
            )
            .await;

            to_login(failure.code(), Some(jar))
        }
    }
}

/// Back to the sign-in page, told why in a word.
fn to_login(code: &str, jar: Option<PrivateCookieJar>) -> Response {
    let location = [(header::LOCATION, format!("/login?sso={code}"))];
    match jar {
        Some(jar) => (StatusCode::SEE_OTHER, jar, location).into_response(),
        None => (StatusCode::SEE_OTHER, location).into_response(),
    }
}

/// Why a return from the provider did not end in a session.
#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("signing in through a provider is not set up")]
    Unavailable,
    #[error("tying an account to the provider needs its owner signed in")]
    SignInFirst,
    #[error("the provider said {0}")]
    Denied(String),
    #[error("this sign-in was not started here, or took too long")]
    Expired,
    #[error("{0}")]
    Provider(String),
    #[error("nobody here is tied to that account at the provider")]
    NoAccount,
    #[error("the account is waiting for approval")]
    Pending,
    #[error("the account is disabled")]
    Disabled,
    #[error(
        "that account at the provider is tied to someone else here, or this account to someone \
         else there"
    )]
    LinkedElsewhere,
    #[error("{0}")]
    Internal(String),
}

impl Failure {
    /// The word the sign-in page is told.
    fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::SignInFirst => "sign_in_first",
            Self::Denied(_) => "denied",
            Self::Expired => "expired",
            Self::Provider(_) | Self::Internal(_) => "failed",
            Self::NoAccount => "no_account",
            Self::Pending => "pending",
            Self::Disabled => "disabled",
            Self::LinkedElsewhere => "linked_elsewhere",
        }
    }
}

fn internal(e: impl std::fmt::Display) -> Failure {
    Failure::Internal(e.to_string())
}

/// Text from outside, fit for a log line or the journal: printable, bounded.
fn clean(text: &str, most: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(most)
        .collect()
}

async fn complete(
    state: &AppState,
    ip: &ClientIp,
    headers: &HeaderMap,
    query: CallbackQuery,
    flow: Option<Flow>,
) -> Result<(String, String), Failure> {
    let provider = state.oidc_provider().ok_or(Failure::Unavailable)?;

    if let Some(error) = query.error {
        return Err(Failure::Denied(clean(&error, 80)));
    }

    let flow = flow.ok_or(Failure::Expired)?;
    let (Some(code), Some(returned)) = (query.code, query.state) else {
        return Err(Failure::Expired);
    };

    let vouched = oidc::finish(&provider, &state.oidc_mapping(), flow, &returned, &code)
        .await
        .map_err(|e| match e {
            Refusal::UnknownFlow => Failure::Expired,
            Refusal::Provider(why) | Refusal::Invalid(why) => Failure::Provider(why),
        })?;

    let user = account_for(state, ip, &vouched).await?;

    match user.status {
        Status::Active => {}
        Status::Pending => return Err(Failure::Pending),
        Status::Disabled => return Err(Failure::Disabled),
    }

    // Where each belongs when nothing else was asked for, as after a
    // password: the catalogue's maintainers to the administration.
    let next = if vouched.next == "/" && user.role >= Role::Editor {
        "/admin".to_string()
    } else {
        vouched.next.clone()
    };

    let (session, _) = auth::open_session(state, user, headers, ip)
        .await
        .map_err(internal)?;

    Ok((session, next))
}

/// The account the provider's person has here: found, tied, or opened.
async fn account_for(state: &AppState, ip: &ClientIp, vouched: &Vouched) -> Result<User, Failure> {
    // Under the accounts lock, like every other change of role or account:
    // a role the provider takes away must not race an administrator's.
    let _held = state.accounts_lock().await;
    let db = &state.db;

    let known = repo::user::find_by_oidc(db, &vouched.issuer, &vouched.subject)
        .await
        .map_err(internal)?;

    // Tied, at the request of the account's owner, signed in when they asked.
    if let Some(link) = vouched.link.as_deref() {
        let owner = repo::user::get(db, link)
            .await
            .map_err(internal)?
            .ok_or(Failure::NoAccount)?;
        match &known {
            Some(other) if other.id != owner.id => return Err(Failure::LinkedElsewhere),
            Some(_) => {}
            None if owner.oidc_linked => return Err(Failure::LinkedElsewhere),
            None => {
                // The account changes shape: whoever holds its session reads it anew.
                state.caches.forget_sessions();
                repo::user::link_oidc(db, &owner.id, &vouched.issuer, &vouched.subject)
                    .await
                    .map_err(internal)?;
                state.caches.forget_sessions();
                record(
                    state,
                    ip,
                    None,
                    Action::UserLinked,
                    &owner.username,
                    &format!(
                        "at its owner's request · {} · {}",
                        clean(&vouched.issuer, 120),
                        clean(&vouched.subject, 80)
                    ),
                )
                .await;
            }
        }
        let owner = repo::user::get(db, &owner.id)
            .await
            .map_err(internal)?
            .ok_or(Failure::NoAccount)?;
        return sync_role(state, ip, owner, vouched.role).await;
    }

    // Known already, by the provider's own name for them.
    if let Some(user) = known {
        return sync_role(state, ip, user, vouched.role).await;
    }

    if !state.flag("oidc.autoRegister", false) {
        return Err(Failure::NoAccount);
    }

    // Opened. Its role is the provider's to say when a claim is named, a
    // member's otherwise; it waits for approval when sign-ups do.
    let username = free_username(state, vouched).await?;
    let role = vouched.role.unwrap_or(Role::Member);
    let status = if state.registration() == Registration::Approval {
        Status::Pending
    } else {
        Status::Active
    };
    let display_name = vouched.name.as_deref().map(|n| clean(n, 80));
    // Kept only when verified — and even then only as a way to reach them,
    // never as proof of who they are.
    let email = vouched
        .email
        .as_deref()
        .filter(|_| vouched.email_verified)
        .and_then(|e| users::clean_email(Some(e)).ok().flatten());

    let user = repo::user::create(
        db,
        repo::user::NewUser {
            username: &username,
            password_hash: NO_PASSWORD,
            role,
            status,
            display_name: display_name.as_deref(),
            email: email.as_deref(),
            invited_by: None,
            oidc: Some((&vouched.issuer, &vouched.subject)),
        },
    )
    .await
    .map_err(internal)?;

    record(
        state,
        ip,
        None,
        Action::UserRegistered,
        &user.username,
        &format!(
            "{} · identity provider · {}",
            user.role.as_str(),
            user.status.as_str()
        ),
    )
    .await;

    Ok(user)
}

/// The role the provider's claims give, applied — unless it would leave the
/// server without an active administrator.
async fn sync_role(
    state: &AppState,
    ip: &ClientIp,
    user: User,
    mapped: Option<Role>,
) -> Result<User, Failure> {
    // Nothing said: no claim named, or the claims did not carry it. Roles are
    // this server's to give then.
    let Some(role) = mapped.filter(|role| *role != user.role) else {
        return Ok(user);
    };

    if user.role == Role::Admin
        && user.status == Status::Active
        && repo::user::count_active_admins(&state.db, Some(&user.id))
            .await
            .map_err(internal)?
            == 0
    {
        tracing::warn!(
            username = %user.username,
            "the identity provider's claims would demote the last administrator; left as they are"
        );
        return Ok(user);
    }

    state.caches.forget_sessions();
    repo::user::set_role(&state.db, &user.id, role)
        .await
        .map_err(internal)?;
    state.caches.forget_sessions();
    record(
        state,
        ip,
        None,
        Action::UserUpdated,
        &user.username,
        &format!(
            "role {} → {} (identity provider)",
            user.role.as_str(),
            role.as_str()
        ),
    )
    .await;

    repo::user::get(&state.db, &user.id)
        .await
        .map_err(internal)?
        .ok_or(Failure::NoAccount)
}

/// A username nobody has: the provider's preferred one, the e-mail's local
/// part or the name, cleaned to what a username may hold, numbered if taken —
/// and never the one the environment names, which keeps a password door.
async fn free_username(state: &AppState, vouched: &Vouched) -> Result<String, Failure> {
    let wanted = vouched
        .preferred_username
        .as_deref()
        .or_else(|| vouched.email.as_deref().and_then(|e| e.split('@').next()))
        .or(vouched.name.as_deref())
        .unwrap_or_default();

    let cleaned: String = wanted
        .chars()
        .filter_map(|c| match c {
            c if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') => Some(c),
            ' ' => Some('.'),
            _ => None,
        })
        .take(34)
        .collect();
    let base = if cleaned.chars().count() >= 2 {
        cleaned
    } else {
        "member".to_string()
    };

    for n in 1..=50 {
        let candidate = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let candidate = users::clean_username(&candidate).map_err(internal)?;
        if !state.is_break_glass_name(&candidate)
            && repo::user::find_by_username(&state.db, &candidate)
                .await
                .map_err(internal)?
                .is_none()
        {
            return Ok(candidate);
        }
    }

    Err(Failure::Internal("no free username was found".into()))
}

/// The page that moves on, from this site, to where the person was going.
fn onward(next: &str) -> Html<String> {
    let href = escape(next);
    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"color-scheme\" content=\"dark light\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <meta http-equiv=\"refresh\" content=\"0;url={href}\">\
         <title>Cinémathèque</title></head>\
         <body><p><a href=\"{href}\">Continue</a></p></body></html>"
    ))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A page of this site to come back to: a path, never an address elsewhere.
fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(path)
            if path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.chars().any(char::is_control)
                && path.len() <= 500 =>
        {
            path.to_string()
        }
        _ => "/".to_string(),
    }
}

async fn record(
    state: &AppState,
    ip: &ClientIp,
    identity: Option<&Identity>,
    action: Action,
    target: &str,
    detail: &str,
) {
    audit::record(
        state,
        Event {
            identity,
            ip,
            action,
            target: Some(target),
            detail: Some(detail),
        },
    )
    .await;
}

// ─── what an administrator sets ──────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcConfiguration {
    pub enabled: bool,
    pub issuer: String,
    pub client_id: String,
    /// Whether a secret is stored. It is never sent back.
    pub secret_set: bool,
    /// Whether AMS_OIDC_CLIENT_SECRET holds it, which wins over a stored one.
    pub secret_from_env: bool,
    pub scopes: String,
    pub button_label: String,
    pub auto_register: bool,
    pub role_claim: String,
    pub admin_values: String,
    pub editor_values: String,
    pub password_login: bool,
    /// AMS_FORCE_PASSWORD_LOGIN keeps passwords on whatever is set here.
    pub password_forced: bool,
    /// What to register at the provider as the return address; none without
    /// AMS_PUBLIC_URL, which the flow needs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// Everything is set and it is on: the sign-in page offers it.
    pub ready: bool,
    /// The account AMS_ADMIN_USERNAME names, which keeps its password
    /// whatever `passwordLogin` says, while it is an active administrator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub break_glass: Option<String>,
}

fn read(state: &AppState) -> OidcConfiguration {
    let text = |key: &str| state.settings.resolve(key, None, None).unwrap_or_default();

    OidcConfiguration {
        enabled: state.flag("oidc.enabled", false),
        issuer: text("oidc.issuer"),
        client_id: text("oidc.clientId"),
        secret_set: state
            .settings
            .at(Scope::Server, "", "oidc.clientSecret")
            .is_some(),
        secret_from_env: state.config.security.oidc_client_secret.is_some(),
        scopes: text("oidc.scopes"),
        button_label: text("oidc.buttonLabel"),
        auto_register: state.flag("oidc.autoRegister", false),
        role_claim: text("oidc.roleClaim"),
        admin_values: text("oidc.adminValues"),
        editor_values: text("oidc.editorValues"),
        password_login: state.flag("auth.passwordLogin", true),
        password_forced: state.config.security.force_password_login,
        redirect_uri: state.oidc_redirect_uri(),
        ready: state.oidc_provider().is_some(),
        break_glass: state
            .config
            .security
            .bootstrap_admin
            .as_ref()
            .map(|(name, _)| name.clone()),
    }
}

#[utoipa::path(
    get, path = "/admin/oidc", tag = super::meta::TAG,
    responses(
        (status = 200, body = OidcConfiguration),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn configuration(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<OidcConfiguration>> {
    identity.require_admin()?;
    Ok(Json(read(&state)))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcUpdate {
    pub enabled: Option<bool>,
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    /// Left out, the stored secret stays; empty, it is removed.
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
    pub button_label: Option<String>,
    pub auto_register: Option<bool>,
    pub role_claim: Option<String>,
    pub admin_values: Option<String>,
    pub editor_values: Option<String>,
    pub password_login: Option<bool>,
}

/// Change how people sign in through the provider. Each field left out stays.
/// All of it is checked before any of it is written.
#[utoipa::path(
    put, path = "/admin/oidc", tag = super::meta::TAG,
    request_body = OidcUpdate,
    responses(
        (status = 200, body = OidcConfiguration),
        (status = 400, description = "A field was rejected"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "Passwords cannot be switched off yet; the message says why"),
    ),
)]
async fn configure(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(update): Json<OidcUpdate>,
) -> AppResult<Json<OidcConfiguration>> {
    identity.require_admin()?;

    // What would be stored, key by key: `None` removes it.
    let mut changes: Vec<(&'static str, Option<String>)> = Vec::new();
    let mut text = |key: &'static str, value: &Option<String>| {
        if let Some(value) = value {
            let value = value.trim();
            changes.push((key, (!value.is_empty()).then(|| value.to_string())));
        }
    };
    text("oidc.issuer", &update.issuer);
    text("oidc.clientId", &update.client_id);
    text("oidc.clientSecret", &update.client_secret);
    text("oidc.scopes", &update.scopes);
    text("oidc.buttonLabel", &update.button_label);
    text("oidc.roleClaim", &update.role_claim);
    text("oidc.adminValues", &update.admin_values);
    text("oidc.editorValues", &update.editor_values);
    for (key, value) in [
        ("oidc.enabled", update.enabled),
        ("oidc.autoRegister", update.auto_register),
        ("auth.passwordLogin", update.password_login),
    ] {
        if let Some(value) = value {
            changes.push((key, Some(value.to_string())));
        }
    }

    // Every value checked before a single one is written.
    for (key, value) in &changes {
        if let Some(value) = value {
            let def = crate::settings::registry::find(key)
                .ok_or_else(|| AppError::BadRequest(format!("no setting called {key}")))?;
            crate::settings::registry::validate(def, value)
                .map_err(|why| AppError::BadRequest(format!("{key} {why}")))?;
        }
    }
    if let Some((_, Some(issuer))) = changes.iter().find(|(key, _)| *key == "oidc.issuer") {
        let parsed = url::Url::parse(issuer)
            .map_err(|_| AppError::BadRequest(format!("{issuer:?} is not a URL")))?;
        match parsed.scheme() {
            "https" => {}
            "http" if oidc::http_allowed(&parsed) => {}
            "http" => {
                return Err(AppError::BadRequest(
                    "an issuer on the internet is reached over https: over plain http, anyone on \
                     the way could hand this server keys of their own"
                        .into(),
                ));
            }
            _ => return Err(AppError::BadRequest("the issuer is an https URL".into())),
        }
    }

    // Passwords off: only with a provider that is set up and answers, and
    // with somebody who could still be let back in if it failed.
    let after = |key: &str| -> Option<String> {
        match changes.iter().find(|(k, _)| *k == key) {
            Some((_, value)) => value.clone(),
            None => state.settings.resolve(key, None, None),
        }
    };
    let passwords_off = after("auth.passwordLogin").as_deref() == Some("false");
    let touches_provider = changes.iter().any(|(k, _)| k.starts_with("oidc."));
    let was_off = !state.flag("auth.passwordLogin", true);
    if passwords_off && (!was_off || touches_provider) {
        let provider = state.oidc_provider_from(&after).ok_or_else(|| {
            AppError::Conflict(
                "passwords stay on until the identity provider is set up and switched on".into(),
            )
        })?;
        if let Err(e) = oidc::discover(&provider).await {
            return Err(AppError::Conflict(format!(
                "passwords stay on: the identity provider does not answer as it should ({})",
                clean(&format!("{e:#}"), 300)
            )));
        }
        let me_linked = identity.user().is_some_and(|me| me.oidc_linked);
        if !me_linked && !state.break_glass_ready().await? {
            return Err(AppError::Conflict(
                "passwords stay on: nobody could be let back in if the provider failed. Tie your \
                 own account to it first, from your account page, or name an active \
                 administrator in AMS_ADMIN_USERNAME"
                    .into(),
            ));
        }
    }

    for (key, value) in &changes {
        match value {
            Some(value) => state
                .settings
                .set(Scope::Server, "", key, value, Some(&identity.label()))
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
            None => {
                state
                    .settings
                    .clear(Scope::Server, "", key)
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
            }
        }
    }

    oidc::forget().await;
    // The other instances read the settings again, and forget the provider
    // they discovered too.
    state.coord.tell(crate::coord::Message::Settings);

    if !changes.is_empty() {
        let keys: Vec<&str> = changes.iter().map(|(key, _)| *key).collect();
        record(
            &state,
            &ip,
            Some(&identity),
            Action::OidcConfigured,
            "identity provider",
            &keys.join(", "),
        )
        .await;
    }

    Ok(Json(read(&state)))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    /// The issuer to try; the stored one when left out.
    pub issuer: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestOutcome {
    pub ok: bool,
    /// What went wrong, in the words of the check that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery: Option<oidc::Discovery>,
}

/// Read the provider's discovery document, to check an issuer before relying
/// on it. Answers what it found, or why it found nothing.
#[utoipa::path(
    post, path = "/admin/oidc/test", tag = super::meta::TAG,
    request_body = TestRequest,
    responses(
        (status = 200, body = TestOutcome),
        (status = 400, description = "No issuer to try"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn test(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<TestRequest>,
) -> AppResult<Json<TestOutcome>> {
    identity.require_admin()?;

    let issuer = request
        .issuer
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty())
        .or_else(|| state.settings.resolve("oidc.issuer", None, None))
        .ok_or_else(|| AppError::BadRequest("there is no issuer to try".into()))?;

    let provider = oidc::Provider {
        issuer,
        client_id: String::new(),
        client_secret: None,
        scopes: Vec::new(),
        redirect_uri: state
            .oidc_redirect_uri()
            .unwrap_or_else(|| "http://localhost/".into()),
    };

    Ok(Json(match oidc::discover(&provider).await {
        Ok(discovery) => TestOutcome {
            ok: true,
            error: None,
            discovery: Some(discovery),
        },
        Err(e) => TestOutcome {
            ok: false,
            error: Some(clean(&format!("{e:#}"), 400)),
            discovery: None,
        },
    }))
}

/// Untie an account from the identity provider, and close its sessions. Not
/// an account without a password of its own: untied, it could never sign in
/// again — give it one first.
#[utoipa::path(
    delete, path = "/users/{id}/oidc", tag = users::TAG,
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404),
        (status = 409, description = "The account has no password of its own"),
    ),
)]
async fn unlink(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;

    let _held = state.accounts_lock().await;
    let user = repo::user::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !user.has_password {
        return Err(AppError::Conflict(
            "this account has no password of its own: untied, it could not sign in again. Give \
             it one first"
                .into(),
        ));
    }

    repo::user::unlink_oidc(&state.db, &id).await?;
    // Signed in through the tie being undone: not any more.
    state.caches.forget_sessions();
    repo::user::delete_sessions_for_user(&state.db, &id).await?;
    state.caches.forget_sessions();
    record(
        &state,
        &ip,
        Some(&identity),
        Action::UserUnlinked,
        &user.username,
        "identity provider · sessions closed",
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_of_this_site_is_gone_back_to() {
        assert_eq!(safe_next(Some("/admin/users")), "/admin/users");
        assert_eq!(
            safe_next(Some("/browse?genre=drama")),
            "/browse?genre=drama"
        );
        assert_eq!(safe_next(Some("//evil.example")), "/");
        assert_eq!(safe_next(Some("/\\evil.example")), "/");
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(Some("/\tx")), "/");
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn the_onward_page_cannot_be_broken_out_of() {
        let page = onward("/a\"><script>x</script>").0;
        assert!(!page.contains("<script>"));
        assert!(page.contains("url=/a&quot;&gt;&lt;script&gt;"));
    }

    #[test]
    fn outside_text_is_printable_and_bounded() {
        assert_eq!(
            clean("access\u{7}_denied\nforged line", 100),
            "access_deniedforged line"
        );
        assert_eq!(clean(&"x".repeat(500), 80).len(), 80);
    }
}
