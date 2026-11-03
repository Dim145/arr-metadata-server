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
//! Nor by a session alone. A session left open, or taken, is not the person:
//! a tie it made would outlive the password that is then reset, and sign
//! somebody else in to the account for good. So a tie is asked for with
//! `POST /account/provider`, which wants the account's password — or, for an
//! account with none, has the provider sign the person in again — and the
//! journal says who tied and who untied.
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
        native::{account, auth, users},
    },
    auth::{
        Identity,
        middleware::CurrentSession,
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

/// An administrator's — and what a person does with their own tie.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(configuration, configure))
        .routes(routes!(test))
        .routes(routes!(unlink))
        .routes(routes!(tie, untie))
}

/// Whether the flow's cookies may be `Secure` (and its own, `__Host-`): the
/// rule the session cookie follows. The start and the return reach this
/// server the same way, so both name the flow's cookie alike.
fn secure(state: &AppState, headers: &HeaderMap, peer: &auth::Peer) -> bool {
    auth::served_securely(state, headers, peer)
}

// ─── the flow ────────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StartQuery {
    /// The page of this site to come back to.
    pub next: Option<String>,
    /// Refused: a tie is asked for from the account page, with
    /// `POST /account/provider`, which proves it by the account's password.
    pub link: Option<String>,
}

/// Go to the identity provider to sign in. Tying an account to the person the
/// provider knows is asked for with `POST /account/provider`.
#[utoipa::path(
    get, path = "/auth/oidc/start", tag = auth::TAG,
    operation_id = "oidc_start",
    params(StartQuery),
    responses(
        (status = 303, description = "To the identity provider, or back to the sign-in page saying why not"),
    ),
    security(),
)]
async fn start(
    State(state): State<AppState>,
    peer: auth::Peer,
    headers: HeaderMap,
    Query(query): Query<StartQuery>,
) -> Response {
    match begin(&state, &headers, &peer, query).await {
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
    peer: &auth::Peer,
    query: StartQuery,
) -> Result<(PrivateCookieJar, String), Failure> {
    let provider = state.oidc_provider().ok_or(Failure::Unavailable)?;

    // Tying is not asked for here: a link that carries a session and no proof
    // — the password, or the provider's own word that the person signed in
    // again — would tie whoever holds the session, and the tie would outlive
    // the password. The account page asks for it with the proof.
    if query.link.as_deref().is_some_and(|l| !l.is_empty()) {
        return Err(Failure::SignInFirst);
    }

    let started = oidc::start(&provider, safe_next(query.next.as_deref()), None, false)
        .await
        .map_err(|e| Failure::Provider(format!("{e:#}")))?;

    let jar = sealed(state, headers, peer, &started)?;
    Ok((jar, started.url))
}

/// The flow, sealed into the cookie that carries it to the provider and back.
fn sealed(
    state: &AppState,
    headers: &HeaderMap,
    peer: &auth::Peer,
    started: &oidc::Started,
) -> Result<PrivateCookieJar, Failure> {
    let secure = secure(state, headers, peer);
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

    Ok(PrivateCookieJar::new(state.cookie_key.clone()).add(cookie))
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
    operation_id = "oidc_callback",
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
    peer: auth::Peer,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let secure = secure(&state, &headers, &peer);
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

    // Worth a row in the journal only when a sign-in was started here — the
    // return carries the flow this server sealed — and a provider is set up
    // to answer it. Anybody can send this request with nothing at all, and a
    // row for each would be the journal's to hold for ninety days.
    let started_here = flow.is_some() && state.oidc_provider().is_some();

    match complete(&state, &ip, &headers, secure, query, flow).await {
        Ok((session, next)) => (
            jar,
            [
                (header::SET_COOKIE, session),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            onward(&next),
        )
            .into_response(),
        Err(failure) if !started_here => {
            tracing::debug!(
                reason = failure.code(),
                "a return from the identity provider that was not started here was refused"
            );
            to_login(failure.code(), Some(jar))
        }
        Err(failure) => {
            tracing::info!(reason = failure.code(), detail = %failure, "a sign-in through the identity provider was refused");
            // The word, and what the provider or the check said: what an
            // administrator reading the journal needs. A run from one network
            // is one row at once and one that counts the rest.
            audit::record_failure(
                &state,
                audit::Failure::Provider,
                &ip,
                Some("identity provider"),
                Some(&format!(
                    "{}: {}",
                    failure.code(),
                    clean(&failure.to_string(), 200)
                )),
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
    secure: bool,
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

    let (session, _) = auth::open_session(state, user, headers, ip, secure)
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
        // What the tie was asked for with — the password, or the provider's
        // word — was the account's as it stood when the flow began. One that
        // has changed since, a password reset above all, is not tied by a
        // flow begun before: whoever began it may be the one the reset was
        // for, finishing at the provider what would outlive it.
        if owner
            .updated_at
            .as_deref()
            .and_then(crate::db::parse_rfc3339)
            .is_some_and(|changed| changed.timestamp() > vouched.began)
        {
            return Err(Failure::Expired);
        }
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
                // By its owner, who showed it was them — by the password, or
                // by signing in at the provider again — before they were sent
                // there: the journal names them, as it names whoever untied.
                let by = Identity::User(Box::new(owner.clone()));
                record(
                    state,
                    ip,
                    Some(&by),
                    Action::UserLinked,
                    &owner.username,
                    &format!(
                        "by its owner, {} · {} · {}",
                        if vouched.fresh {
                            "who signed in at the provider again"
                        } else {
                            "who gave their password"
                        },
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
    operation_id = "oidc_configuration",
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
    operation_id = "oidc_configure",
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
        oidc::reachable(&parsed)
            .map_err(|why| AppError::BadRequest(format!("this server will not ask for {why}")))?;
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

    // A value refused is the caller's (400); a database that failed is the
    // server's (500), logged rather than handed back.
    for (key, value) in &changes {
        match value {
            Some(value) => {
                state
                    .settings
                    .set(Scope::Server, "", key, value, Some(&identity.label()))
                    .await?;
            }
            None => {
                state.settings.clear(Scope::Server, "", key).await?;
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
    operation_id = "oidc_test",
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
    operation_id = "oidc_unlink",
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

// ─── what a person does with their own tie ───────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TieRequest {
    /// The account's current password: the proof, from an account that has
    /// one, that its owner is asking and not a session left open. An account
    /// with none leaves it out, and is sent to sign in at the provider again
    /// instead.
    pub current_password: Option<String>,
    /// The page of this site to come back to.
    pub next: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TieStarted {
    /// Where to send the browser: the identity provider's sign-in. The
    /// cookie that carries the flow back is set with this answer.
    pub url: String,
}

/// Tie your account to the person the identity provider knows, so that you
/// can sign in through it as well.
///
/// A session is not proof enough: one left open, or taken, would tie whoever
/// holds it, and the tie would outlive the password the account's owner then
/// resets. So the account's current password is asked for — or, for an account
/// with none, the provider is asked to have you sign in again, and its answer
/// must say you did. The answer is the provider's address to send the browser
/// to; the tie is made when the browser comes back from it.
#[utoipa::path(
    post, path = "/account/provider", tag = account::TAG,
    operation_id = "oidc_tie",
    request_body = TieRequest,
    responses(
        (status = 200, description = "Where to send the browser; the cookie of the flow is set", body = TieStarted),
        (status = 400, description = "The password was left out, or is too long"),
        (status = 401, description = "The password was wrong"),
        (status = 403, description = "A key cannot manage an account"),
        (status = 409, description = "The account is tied already, or no provider is set up"),
        (status = 429, description = "Too many wrong passwords lately"),
        (status = 502, description = "The provider did not answer"),
    ),
)]
async fn tie(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    peer: auth::Peer,
    headers: HeaderMap,
    Json(request): Json<TieRequest>,
) -> AppResult<(PrivateCookieJar, Json<TieStarted>)> {
    let me = identity.require_user()?;

    let provider = state.oidc_provider().ok_or_else(|| {
        AppError::Conflict("signing in through an identity provider is not set up here".into())
    })?;
    if me.oidc_linked {
        return Err(AppError::Conflict(
            "this account is tied to the identity provider already".into(),
        ));
    }

    // Proof that its owner is asking: the password, counted and checked as a
    // sign-in is; or, for an account that has none, the provider's word that
    // the person signed in again.
    let fresh = if me.has_password {
        let typed = request
            .current_password
            .as_deref()
            .filter(|typed| !typed.is_empty())
            .ok_or_else(|| {
                AppError::BadRequest("your current password is needed to tie this account".into())
            })?;
        if typed.len() > auth::MAX_CREDENTIAL {
            return Err(AppError::BadRequest("that password is too long".into()));
        }
        let creds = repo::user::credentials(&state.db, &me.id)
            .await?
            .ok_or(AppError::NotFound)?;
        auth::prove_password(&state, me, creds.password_hash, typed, &ip).await?;
        false
    } else {
        true
    };

    let started = oidc::start(
        &provider,
        safe_next(request.next.as_deref()),
        Some(me.id.clone()),
        fresh,
    )
    .await
    .map_err(AppError::UpstreamUnavailable)?;
    let jar = sealed(&state, &headers, &peer, &started)
        .map_err(|failure| AppError::Internal(anyhow::anyhow!("{failure}")))?;

    Ok((jar, Json(TieStarted { url: started.url })))
}

/// Whether an account would still be let in with its tie undone: by a
/// password, which this server asks for — passwords are not switched off, or
/// the account is the one the environment names, which keeps its door.
fn keeps_a_way_in(state: &AppState, user: &User) -> bool {
    user.has_password
        && (state.password_login()
            || (state.is_break_glass_name(&user.username)
                && user.role == Role::Admin
                && user.status == Status::Active))
}

/// Untie your account from the identity provider. Not one that would be left
/// with no way in: a password of its own, which this server asks for, comes
/// first. Your other sessions are closed — one of them may have been opened
/// through the tie.
#[utoipa::path(
    delete, path = "/account/provider", tag = account::TAG,
    operation_id = "oidc_untie",
    responses(
        (status = 204, description = "Untied; the other sessions are closed"),
        (status = 403, description = "A key cannot manage an account"),
        (status = 409, description = "The account is not tied, or would be left with no way in"),
    ),
)]
async fn untie(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    current: Option<Extension<CurrentSession>>,
    ip: ClientIp,
) -> AppResult<StatusCode> {
    let me = identity.require_user()?;
    let keep = current
        .map(|Extension(CurrentSession(id))| id)
        .unwrap_or_default();

    let _held = state.accounts_lock().await;
    let user = repo::user::get(&state.db, &me.id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !user.oidc_linked {
        return Err(AppError::Conflict(
            "this account is not tied to an identity provider".into(),
        ));
    }
    if !keeps_a_way_in(&state, &user) {
        return Err(AppError::Conflict(
            "untied, this account could not sign in again: it needs a password of its own, and \
             passwords switched on here"
                .into(),
        ));
    }

    state.caches.forget_sessions();
    repo::user::unlink_oidc(&state.db, &user.id).await?;
    let closed = repo::user::delete_other_sessions(&state.db, &user.id, &keep).await?;
    state.caches.forget_sessions();
    record(
        &state,
        &ip,
        Some(&identity),
        Action::UserUnlinked,
        &user.username,
        &format!("by its owner · {closed} other sessions closed"),
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

    // ─── a person's own tie, and what a return from the provider leaves ──────

    use std::{collections::HashMap, sync::Arc};

    use crate::{
        arr::client::tests::stand_in,
        auth::{
            secrets,
            testing::{client_at, server},
        },
        settings::Scope,
    };

    async fn account(
        state: &AppState,
        name: &str,
        password_hash: &str,
        oidc: Option<&str>,
        role: Role,
    ) -> User {
        repo::user::create(
            &state.db,
            repo::user::NewUser {
                username: name,
                password_hash,
                role,
                status: Status::Active,
                display_name: None,
                email: None,
                invited_by: None,
                oidc: oidc.map(|subject| ("https://idp.example", subject)),
            },
        )
        .await
        .expect("an account")
    }

    fn as_user(user: &User) -> Extension<Identity> {
        Extension(Identity::User(Box::new(user.clone())))
    }

    async fn provider_at(state: &AppState, issuer: &str) {
        for (key, value) in [
            ("oidc.enabled", "true"),
            ("oidc.issuer", issuer),
            ("oidc.clientId", "client"),
        ] {
            state
                .settings
                .set(Scope::Server, "", key, value, None)
                .await
                .unwrap_or_else(|e| panic!("{key}: {e:?}"));
        }
    }

    /// A provider small enough to read, on loopback: its discovery document,
    /// and a key set with no key in it. Its address.
    async fn stand_in_provider() -> String {
        let at = Arc::new(std::sync::OnceLock::<String>::new());
        let known = Arc::clone(&at);
        let (address, _seen) = stand_in(Arc::new(move |request| {
            let at = known.get().expect("known by the first request");
            match request.target.as_str() {
                "/.well-known/openid-configuration" => (
                    200,
                    serde_json::json!({
                        "issuer": at,
                        "authorization_endpoint": format!("{at}/authorize"),
                        "token_endpoint": format!("{at}/token"),
                        "jwks_uri": format!("{at}/keys"),
                        "response_types_supported": ["code"],
                        "subject_types_supported": ["public"],
                        "id_token_signing_alg_values_supported": ["RS256"],
                    })
                    .to_string(),
                ),
                "/keys" => (200, r#"{"keys":[]}"#.to_string()),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;
        at.set(address.clone()).unwrap();
        address
    }

    async fn journal(state: &AppState, action: Action) -> Vec<repo::audit::Entry> {
        repo::audit::list(
            &state.db,
            &repo::audit::Query {
                action: Some(action.as_str().into()),
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap()
    }

    fn password() -> String {
        secrets::generate_session_token().unwrap().0
    }

    /// A session is not the password. Tying asks for it, and starts nothing
    /// without; with it, the provider's address and a cookie that carries
    /// the flow home, naming the account.
    #[tokio::test]
    async fn an_account_with_a_password_is_tied_only_by_whoever_gives_it() {
        let state = server(10).await;
        let provider = stand_in_provider().await;
        provider_at(&state, &provider).await;
        let right = password();
        let alice = account(
            &state,
            "alice",
            &secrets::hash_password(&right).unwrap(),
            None,
            Role::Member,
        )
        .await;
        let ip = client_at(&state, "203.0.113.7").await;
        let ask = |typed: Option<&str>| {
            tie(
                State(state.clone()),
                as_user(&alice),
                ip,
                None,
                HeaderMap::new(),
                Json(TieRequest {
                    current_password: typed.map(str::to_string),
                    next: Some("/account".into()),
                }),
            )
        };

        assert!(matches!(ask(None).await, Err(AppError::BadRequest(_))));
        assert!(matches!(ask(Some("")).await, Err(AppError::BadRequest(_))));
        assert!(matches!(
            ask(Some(&password())).await,
            Err(AppError::Unauthorized)
        ));

        let (jar, Json(started)) = ask(Some(&right)).await.expect("started");
        assert!(
            started.url.starts_with(&format!("{provider}/authorize?")),
            "{}",
            started.url
        );
        let query: HashMap<String, String> = url::Url::parse(&started.url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert!(!query.contains_key("prompt") && !query.contains_key("max_age"));
        let flow = jar
            .get(&oidc::cookie_name(&query["state"], false))
            .expect("the flow's cookie");
        let flow: serde_json::Value = serde_json::from_str(flow.value()).unwrap();
        assert_eq!(flow["link"], alice.id);
        assert_eq!(flow["fresh"], false);
        assert_eq!(flow["next"], "/account");
    }

    /// An account with no password to give has the provider sign its person
    /// in again, whatever session it holds, and says so in the flow, so that
    /// the return is held to it.
    #[tokio::test]
    async fn an_account_with_no_password_has_the_provider_sign_its_person_in_again() {
        let state = server(10).await;
        let provider = stand_in_provider().await;
        provider_at(&state, &provider).await;
        let carol = account(&state, "carol", repo::user::NO_PASSWORD, None, Role::Member).await;
        let ip = client_at(&state, "203.0.113.8").await;

        let (jar, Json(started)) = tie(
            State(state.clone()),
            as_user(&carol),
            ip,
            None,
            HeaderMap::new(),
            Json(TieRequest {
                current_password: None,
                next: None,
            }),
        )
        .await
        .expect("started");
        let query: HashMap<String, String> = url::Url::parse(&started.url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(query["prompt"], "login");
        assert_eq!(query["max_age"], "0");
        let flow = jar
            .get(&oidc::cookie_name(&query["state"], false))
            .expect("the flow's cookie");
        let flow: serde_json::Value = serde_json::from_str(flow.value()).unwrap();
        assert_eq!(flow["fresh"], true);
        assert_eq!(flow["link"], carol.id);
    }

    #[tokio::test]
    async fn tying_is_refused_without_a_provider_to_a_tied_account_and_to_a_key() {
        let state = server(10).await;
        let alice = account(&state, "alice", "x", None, Role::Member).await;
        let ip = client_at(&state, "203.0.113.7").await;
        let ask = |who: Identity| {
            tie(
                State(state.clone()),
                Extension(who),
                ip,
                None,
                HeaderMap::new(),
                Json(TieRequest {
                    current_password: Some(password()),
                    next: None,
                }),
            )
        };

        // No provider is set up.
        assert!(matches!(
            ask(Identity::User(Box::new(alice.clone()))).await,
            Err(AppError::Conflict(_))
        ));

        let provider = stand_in_provider().await;
        provider_at(&state, &provider).await;
        // One already tied.
        let tied = account(&state, "tied", "x", Some("sub-tied"), Role::Member).await;
        assert!(matches!(
            ask(Identity::User(Box::new(tied))).await,
            Err(AppError::Conflict(_))
        ));
        // A key cannot manage an account.
        assert!(matches!(
            ask(Identity::Anonymous).await,
            Err(AppError::Forbidden)
        ));
    }

    /// A tie is its owner's doing, and the journal says so — and how they
    /// showed it was them.
    #[tokio::test]
    async fn a_tie_is_journalled_as_its_owners_doing() {
        let state = server(10).await;
        let alice = account(&state, "alice", "x", None, Role::Member).await;
        let vouched = |fresh: bool, subject: &str| Vouched {
            issuer: "https://idp.example".into(),
            subject: subject.into(),
            email: None,
            email_verified: false,
            preferred_username: None,
            name: None,
            role: None,
            next: "/".into(),
            link: Some(alice.id.clone()),
            fresh,
            began: chrono::Utc::now().timestamp(),
        };

        let tied = account_for(&state, &ClientIp::NONE, &vouched(false, "sub-alice"))
            .await
            .expect("tied");
        assert!(tied.oidc_linked);

        let rows = journal(&state, Action::UserLinked).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].actor.as_deref(), Some("member:alice"));
        assert_eq!(rows[0].target.as_deref(), Some("alice"));
        let said = rows[0].detail.as_deref().unwrap_or_default();
        assert!(said.contains("who gave their password"), "{said}");
        assert!(said.contains("sub-alice"), "{said}");

        // Another's tie is refused, and tied once more nowhere.
        let bob = account(&state, "bob", repo::user::NO_PASSWORD, None, Role::Member).await;
        let mut elsewhere = vouched(true, "sub-alice");
        elsewhere.link = Some(bob.id.clone());
        assert!(matches!(
            account_for(&state, &ClientIp::NONE, &elsewhere).await,
            Err(Failure::LinkedElsewhere)
        ));
        let mut again = vouched(true, "sub-bob");
        again.link = Some(bob.id.clone());
        account_for(&state, &ClientIp::NONE, &again)
            .await
            .expect("tied");
        let rows = journal(&state, Action::UserLinked).await;
        assert_eq!(rows.len(), 2);
        assert!(
            rows[0]
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("who signed in at the provider again")),
            "{:?}",
            rows[0].detail
        );
    }

    /// A flow begun before the account was changed — a password reset above
    /// all — ties nothing: whoever began it may be who the reset was for.
    #[tokio::test]
    async fn a_flow_begun_before_the_account_changed_does_not_tie_it() {
        let state = server(10).await;
        let alice = account(&state, "alice", "x", None, Role::Member).await;
        let began = chrono::Utc::now().timestamp();
        // The reset, a few seconds after the flow began.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        repo::user::set_password(&state.db, &alice.id, "y")
            .await
            .unwrap();

        let vouched = Vouched {
            issuer: "https://idp.example".into(),
            subject: "sub-attacker".into(),
            email: None,
            email_verified: false,
            preferred_username: None,
            name: None,
            role: None,
            next: "/".into(),
            link: Some(alice.id.clone()),
            fresh: false,
            began,
        };
        assert!(matches!(
            account_for(&state, &ClientIp::NONE, &vouched).await,
            Err(Failure::Expired)
        ));
        let after = repo::user::get(&state.db, &alice.id)
            .await
            .unwrap()
            .unwrap();
        assert!(!after.oidc_linked);
        assert!(journal(&state, Action::UserLinked).await.is_empty());

        // One begun after is let through.
        let later = Vouched {
            began: chrono::Utc::now().timestamp(),
            ..vouched
        };
        account_for(&state, &ClientIp::NONE, &later)
            .await
            .expect("tied");
    }

    /// Untied by its owner, who keeps a password to sign in with: the other
    /// sessions go, this one stays, and the journal names the owner.
    #[tokio::test]
    async fn an_owner_unties_their_account_while_a_password_keeps_them_in() {
        let state = server(10).await;
        let alice = account(&state, "alice", "x", Some("sub-alice"), Role::Member).await;
        for id in ["this-session", "another-session"] {
            repo::user::create_session(
                &state.db,
                id,
                &alice.id,
                chrono::Duration::hours(1),
                None,
                None,
            )
            .await
            .unwrap();
        }

        let untied = untie(
            State(state.clone()),
            as_user(&alice),
            Some(Extension(CurrentSession("this-session".into()))),
            ClientIp::NONE,
        )
        .await
        .expect("untied");
        assert_eq!(untied, StatusCode::NO_CONTENT);

        let after = repo::user::get(&state.db, &alice.id)
            .await
            .unwrap()
            .unwrap();
        assert!(!after.oidc_linked && after.has_password);
        let kept = repo::user::list_sessions(&state.db, &alice.id, None)
            .await
            .unwrap();
        assert_eq!(
            kept.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["this-session"]
        );
        let rows = journal(&state, Action::UserUnlinked).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].actor.as_deref(), Some("member:alice"));

        // Not tied any more: nothing to untie.
        assert!(matches!(
            untie(State(state.clone()), as_user(&after), None, ClientIp::NONE).await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn an_account_is_not_untied_to_be_left_with_no_way_in() {
        let state = server(10).await;
        let none = account(
            &state,
            "none",
            repo::user::NO_PASSWORD,
            Some("sub-n"),
            Role::Member,
        )
        .await;
        let one = account(&state, "one", "x", Some("sub-o"), Role::Member).await;
        let untie_as = |who: &User| untie(State(state.clone()), as_user(who), None, ClientIp::NONE);

        // No password of its own.
        assert!(matches!(untie_as(&none).await, Err(AppError::Conflict(_))));

        // A password that this server does not ask for is none to keep:
        // passwords are off once a provider is there to sign people in.
        provider_at(&state, "http://127.0.0.1:9").await;
        state
            .settings
            .set(Scope::Server, "", "auth.passwordLogin", "false", None)
            .await
            .unwrap();
        assert!(matches!(untie_as(&one).await, Err(AppError::Conflict(_))));
        let still = repo::user::get(&state.db, &one.id).await.unwrap().unwrap();
        assert!(still.oidc_linked);
        assert!(journal(&state, Action::UserUnlinked).await.is_empty());
    }

    /// The Location a response sends the browser to.
    fn location(answer: &Response) -> &str {
        answer.headers()[header::LOCATION].to_str().unwrap()
    }

    /// Anybody can send a return from the provider with nothing at all. A row
    /// of the journal is for a sign-in that was started here — and for a run
    /// of those, one row at once and one that counts the rest.
    #[tokio::test]
    async fn a_return_nobody_started_here_is_no_row_of_the_journal() {
        let state = server(10).await;
        let ip = client_at(&state, "192.0.2.201").await;
        let come_back = |headers: HeaderMap, query: CallbackQuery| {
            callback(State(state.clone()), ip, None, headers, Query(query))
        };
        let stray = || CallbackQuery {
            code: Some("code".into()),
            state: Some("state".into()),
            error: None,
        };

        // No provider is set up: not told to anybody.
        let answer = come_back(HeaderMap::new(), stray()).await;
        assert_eq!(location(&answer), "/login?sso=unavailable");

        // One is, and this return carries no flow it was started with.
        provider_at(&state, "http://127.0.0.1:9").await;
        let answer = come_back(HeaderMap::new(), stray()).await;
        assert_eq!(location(&answer), "/login?sso=expired");
        assert!(journal(&state, Action::SignInFailed).await.is_empty());

        // One that carries its flow, refused by the provider, is.
        let flow: Flow = serde_json::from_value(serde_json::json!({
            "state": "the-state", "nonce": "n", "verifier": "v", "next": "/",
            "at": chrono::Utc::now().timestamp(), "link": null, "fresh": false,
        }))
        .unwrap();
        let sealed = PrivateCookieJar::new(state.cookie_key.clone()).add(
            Cookie::build((
                oidc::cookie_name("the-state", false),
                serde_json::to_string(&flow).unwrap(),
            ))
            .path(FLOW_PATH),
        );
        let set = (sealed, ()).into_response();
        let carried = set.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        for _ in 0..5 {
            let mut headers = HeaderMap::new();
            headers.insert(header::COOKIE, carried.parse().unwrap());
            let answer = come_back(
                headers,
                CallbackQuery {
                    code: None,
                    state: Some("the-state".into()),
                    error: Some("access_denied".into()),
                },
            )
            .await;
            assert_eq!(location(&answer), "/login?sso=denied");
        }
        // Five of them from one address: one row now, and the rest held.
        let rows = journal(&state, Action::SignInFailed).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].target.as_deref(), Some("identity provider"));
        assert!(
            rows[0]
                .detail
                .as_deref()
                .is_some_and(|d| d.starts_with("denied:")),
            "{:?}",
            rows[0].detail
        );
    }

    /// An issuer at an address no provider is at is refused where it is
    /// saved and where it is tried, before anything is sent to it.
    #[tokio::test]
    async fn an_issuer_at_a_metadata_address_is_refused_when_saved_and_when_tried() {
        let state = server(10).await;
        let admin = account(&state, "root", "x", None, Role::Admin).await;
        let nothing = || OidcUpdate {
            enabled: None,
            issuer: None,
            client_id: None,
            client_secret: None,
            scopes: None,
            button_label: None,
            auto_register: None,
            role_claim: None,
            admin_values: None,
            editor_values: None,
            password_login: None,
        };

        let Err(refused) = configure(
            State(state.clone()),
            as_user(&admin),
            ClientIp::NONE,
            Json(OidcUpdate {
                issuer: Some("https://169.254.169.254/".into()),
                ..nothing()
            }),
        )
        .await
        else {
            panic!("an issuer at a metadata address was accepted");
        };
        assert!(
            matches!(&refused, AppError::BadRequest(why) if why.contains("169.254.169.254")),
            "{refused:?}"
        );
        assert!(
            state
                .settings
                .resolve("oidc.issuer", None, None)
                .unwrap_or_default()
                .is_empty()
        );

        let Json(tried) = test(
            State(state.clone()),
            as_user(&admin),
            Json(TestRequest {
                issuer: Some("https://169.254.169.254/".into()),
            }),
        )
        .await
        .expect("an answer");
        assert!(!tried.ok);
        let said = tried.error.unwrap_or_default();
        assert!(said.contains("no identity provider is at"), "{said}");
    }
}
