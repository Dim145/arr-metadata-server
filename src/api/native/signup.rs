//! Opening an account for yourself, where the server allows it.
//!
//! How is the operator's to say, in `registration.mode`:
//!
//! * `closed` — nobody. An administrator opens accounts from the Members page,
//!   and the invitations already sent stop working.
//! * `invite` — only with an invitation, which also says the account's role.
//! * `approval` — anybody, but the account waits for an administrator. An
//!   invitation skips the wait: an administrator made it.
//! * `open` — anybody, at once.
//!
//! An account made here is never an administrator's. An invitation opens a
//! member's or an editor's; approval gives the role `registration.role` says,
//! a member's or an editor's; an open door only ever opens a member's, or
//! anybody on the internet could edit the catalogue.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
        native::{auth, users},
    },
    auth::secrets,
    db::repo::{
        self,
        audit::Action,
        invitation::Invitation,
        user::{Role, Status},
    },
    error::{AppError, AppResult},
    state::{AppState, Registration},
};

// ─── how many, how fast ──────────────────────────────────────────────────────
//
// Sign-ups share the rate limit of everything else, but that one is sized for
// browsing: at a hundred requests a minute, an open door would let a single
// script open thousands of accounts an hour. So uninvited sign-ups are counted
// twice over — per address (`AMS_SIGNUPS_PER_HOUR`, 5) and for the whole server
// (`AMS_SIGNUPS_PER_HOUR_TOTAL`, 100) — and an invitation is not counted at all:
// an administrator vouched for it, and a family behind one router may well use
// five in an evening.

/// The places taken in the last hour: each address's, and all of them.
#[derive(Default)]
struct Recent {
    by_address: HashMap<IpAddr, Vec<Instant>>,
    all: Vec<Instant>,
}

static RECENT: LazyLock<Mutex<Recent>> = LazyLock::new(|| Mutex::new(Recent::default()));

const HOUR: Duration = Duration::from_secs(3600);

/// What an address is counted under: itself, or for IPv6 its /64 — one home,
/// one phone — since a single machine has a whole /64 to pick from.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut segments = v6.segments();
                segments[4..].fill(0);
                IpAddr::V6(Ipv6Addr::from(segments))
            }
        },
    }
}

/// A place in the last hour's count, given back when it is dropped unless
/// the account was made: a name already taken, a password refused, must not
/// cost the person trying one of their few.
struct Place {
    recent: &'static Mutex<Recent>,
    bucket: Option<IpAddr>,
    at: Instant,
    kept: bool,
}

impl Place {
    fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        let Ok(mut recent) = self.recent.lock() else {
            return;
        };
        if let Some(i) = recent.all.iter().rposition(|at| *at == self.at) {
            recent.all.remove(i);
        }
        if let Some(bucket) = self.bucket
            && let Some(times) = recent.by_address.get_mut(&bucket)
            && let Some(i) = times.iter().rposition(|at| *at == self.at)
        {
            times.remove(i);
        }
    }
}

/// A place, if one is free for this address and for the server.
fn take_place(ip: Option<IpAddr>, now: Instant, per_address: usize, total: usize) -> Option<Place> {
    take_place_in(&RECENT, ip, now, per_address, total)
}

fn take_place_in(
    counts: &'static Mutex<Recent>,
    ip: Option<IpAddr>,
    now: Instant,
    per_address: usize,
    total: usize,
) -> Option<Place> {
    let mut recent = counts.lock().ok()?;

    // Everything older than an hour goes first: what is left is bounded by
    // the server's own cap, whatever the number of addresses asking.
    let fresh = |at: &Instant| now.duration_since(*at) < HOUR;
    recent.all.retain(fresh);
    recent.by_address.retain(|_, times| {
        times.retain(fresh);
        !times.is_empty()
    });

    if recent.all.len() >= total {
        return None;
    }

    let bucket = ip.map(bucket);
    if let Some(bucket) = bucket {
        let times = recent.by_address.entry(bucket).or_default();
        if times.len() >= per_address {
            return None;
        }
        times.push(now);
    }
    recent.all.push(now);

    Some(Place {
        recent: counts,
        bucket,
        at: now,
        kept: false,
    })
}

/// Reachable without a credential, like signing in.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(options))
        .routes(routes!(check_invitation))
        .routes(routes!(register))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthOptions {
    /// `public`: anyone browses the catalogue. `private`: every page asks to
    /// sign in first.
    pub site: &'static str,
    pub registration: Registration,
}

/// What the sign-in page offers: whether one may sign up, and how.
#[utoipa::path(
    get, path = "/auth/options", tag = auth::TAG,
    responses((status = 200, body = AuthOptions)),
    security(),
)]
async fn options(State(state): State<AppState>) -> Json<AuthOptions> {
    Json(AuthOptions {
        site: if state.public_site() {
            "public"
        } else {
            "private"
        },
        registration: state.registration(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct InvitationCheck {
    pub code: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationOffer {
    /// The role the account will have.
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Whether an invitation still opens an account, and which.
///
/// A POST so the code travels in the body: a path or a query string is what
/// access logs keep.
#[utoipa::path(
    post, path = "/auth/invitations/check", tag = auth::TAG,
    request_body = InvitationCheck,
    responses(
        (status = 200, body = InvitationOffer),
        (status = 404, description = "No invitation answers to that code, or it can no longer be used"),
    ),
    security(),
)]
async fn check_invitation(
    State(state): State<AppState>,
    Json(request): Json<InvitationCheck>,
) -> AppResult<Json<InvitationOffer>> {
    if state.registration() == Registration::Closed {
        return Err(closed());
    }

    let invitation = usable(&state, &request.code)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(InvitationOffer {
        role: invitation.role,
        expires_at: invitation.expires_at,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub username: String,
    /// Twelve characters at least.
    pub password: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    /// The invitation, as its link or its sender gave it.
    pub code: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Registered {
    pub username: String,
    /// Waiting for an administrator: no session was opened.
    pub pending: bool,
}

/// Open an account, and sign into it unless it has to wait for approval.
#[utoipa::path(
    post, path = "/auth/register", tag = auth::TAG,
    request_body = RegisterRequest,
    responses(
        (status = 200, description = "Signed in; a session cookie is set", body = auth::LoginResponse),
        (status = 202, description = "Made, and waiting for an administrator", body = Registered),
        (status = 400, description = "A field was rejected"),
        (status = 403, description = "Sign-ups are closed, or the invitation cannot be used"),
        (status = 409, description = "That username is taken"),
    ),
    security(),
)]
async fn register(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    Json(request): Json<RegisterRequest>,
) -> AppResult<Response> {
    let mode = state.registration();
    if mode == Registration::Closed {
        return Err(closed());
    }

    // Bounded before anything is hashed, as signing in is.
    if request.password.len() > auth::MAX_CREDENTIAL {
        return Err(AppError::BadRequest("that password is too long".into()));
    }

    let username = users::clean_username(&request.username)?;
    let display_name = users::clean_display_name(request.display_name.as_deref())?;
    let email = users::clean_email(request.email.as_deref())?;

    let code = request
        .code
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let invitation = match code {
        Some(code) => Some(
            usable(&state, code)
                .await?
                .ok_or_else(|| AppError::Refused {
                    code: "invitation_invalid",
                    message: "that invitation is unknown, used up, expired or withdrawn".into(),
                })?,
        ),
        None if mode == Registration::Invite => {
            return Err(AppError::Refused {
                code: "invitation_required",
                message: "an invitation is needed to open an account here".into(),
            });
        }
        None => None,
    };

    // Counted before anything is looked up or hashed, so that a caller who is
    // over the limit costs nothing and learns nothing — not even whether the
    // name they tried is taken.
    let place = match &invitation {
        Some(_) => None,
        None => Some(
            take_place(
                ip.0,
                Instant::now(),
                state.config.security.signups_per_hour,
                state.config.security.signups_per_hour_total,
            )
            .ok_or(AppError::RateLimited)?,
        ),
    };

    if repo::user::find_by_username(&state.db, &username)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(format!(
            "the username {username:?} is taken"
        )));
    }

    // Hashed before the invitation is spent: a password the rules refuse
    // must not cost its sender a use.
    let hash = secrets::hash_password_async(request.password)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let (role, status) = match &invitation {
        Some(invitation) => (invitation.role, Status::Active),
        // Whoever an administrator approves may be given more; an open door
        // only ever opens a member's account.
        None if mode == Registration::Approval => (state.registration_role(), Status::Pending),
        None => (Role::Member, Status::Active),
    };

    // Taken in one statement, so two people racing for an invitation's last
    // use cannot both have it; given back if the account then fails.
    if let Some(invitation) = &invitation
        && !repo::invitation::take_use(&state.db, &invitation.id).await?
    {
        return Err(AppError::Refused {
            code: "invitation_invalid",
            message: "that invitation was used up a moment ago".into(),
        });
    }

    let made = repo::user::create(
        &state.db,
        repo::user::NewUser {
            username: &username,
            password_hash: &hash,
            role,
            status,
            display_name: display_name.as_deref(),
            email: email.as_deref(),
            invited_by: invitation.as_ref().and_then(|i| i.created_by.as_deref()),
            oidc: None,
        },
    )
    .await;

    let user = match made {
        Ok(user) => user,
        Err(e) => {
            if let Some(invitation) = &invitation
                && let Err(back) = repo::invitation::return_use(&state.db, &invitation.id).await
            {
                tracing::warn!(error = %back, "could not give an invitation its use back");
            }
            // The username check above and the insert are two statements:
            // somebody may have taken the name in between, which the unique
            // index says here.
            return Err(
                match repo::user::find_by_username(&state.db, &username).await {
                    Ok(Some(_)) => {
                        AppError::Conflict(format!("the username {username:?} is taken"))
                    }
                    _ => e.into(),
                },
            );
        }
    };

    if let Some(place) = place {
        place.keep();
    }

    audit::record(
        &state,
        Event {
            identity: None,
            ip: &ip,
            action: Action::UserRegistered,
            target: Some(&user.username),
            detail: Some(&match &invitation {
                Some(invitation) => format!("{} · {}", user.role.as_str(), invitation.code_prefix),
                None => format!("{} · {}", user.role.as_str(), user.status.as_str()),
            }),
        },
    )
    .await;

    if user.status == Status::Pending {
        return Ok((
            StatusCode::ACCEPTED,
            Json(Registered {
                username: user.username,
                pending: true,
            }),
        )
            .into_response());
    }

    let (cookie, answer) = auth::open_session(&state, user, &headers, &ip).await?;
    Ok((StatusCode::OK, [(header::SET_COOKIE, cookie)], Json(answer)).into_response())
}

/// The invitation a code names, when it can still open an account.
async fn usable(state: &AppState, code: &str) -> AppResult<Option<Invitation>> {
    let Some(hash) = secrets::hash_invitation_code(code) else {
        return Ok(None);
    };

    Ok(repo::invitation::find_by_code_hash(&state.db, &hash)
        .await?
        .filter(Invitation::is_usable)
        // Nothing makes one, but a database restored or edited by hand could
        // hold it, and it would open an administrator's account to anyone.
        .filter(|invitation| invitation.role != Role::Admin))
}

fn closed() -> AppError {
    AppError::Refused {
        code: "registration_closed",
        message: "this server does not take sign-ups".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A count of its own, so tests running side by side do not share one.
    fn counts() -> &'static Mutex<Recent> {
        Box::leak(Box::new(Mutex::new(Recent::default())))
    }

    #[test]
    fn an_address_opens_a_handful_of_accounts_an_hour() {
        let counts = counts();
        let take_place = |ip, now, per, total| take_place_in(counts, ip, now, per, total);
        let ip: IpAddr = "203.0.113.77".parse().unwrap();
        let start = Instant::now();

        let places: Vec<Place> = (0..5)
            .map(|_| take_place(Some(ip), start, 5, 1000).expect("a place"))
            .collect();
        assert!(take_place(Some(ip), start + Duration::from_secs(60), 5, 1000).is_none());

        // Another address is another count.
        let other = take_place(Some("203.0.113.78".parse().unwrap()), start, 5, 1000);
        assert!(other.is_some());

        for place in places {
            place.keep();
        }
        // An hour later the door opens again.
        assert!(take_place(Some(ip), start + Duration::from_secs(3601), 5, 1000).is_some());
    }

    #[test]
    fn a_place_not_used_is_given_back() {
        let counts = counts();
        let take_place = |ip, now, per, total| take_place_in(counts, ip, now, per, total);
        let ip: IpAddr = "198.51.100.9".parse().unwrap();
        let now = Instant::now();

        for _ in 0..3 {
            // Dropped without `keep`: the name was taken, say.
            drop(take_place(Some(ip), now, 1, 1000).expect("a place"));
        }
        let kept = take_place(Some(ip), now, 1, 1000).expect("still free");
        kept.keep();
        assert!(take_place(Some(ip), now, 1, 1000).is_none());
    }

    #[test]
    fn the_server_has_a_ceiling_of_its_own() {
        let counts = counts();
        let now = Instant::now();
        let places: Vec<Place> = (0..3)
            .map(|n| {
                take_place_in(counts, Some(IpAddr::from([192, 0, 2, n])), now, 5, 3)
                    .expect("a place")
            })
            .collect();
        assert!(take_place_in(counts, Some(IpAddr::from([192, 0, 2, 99])), now, 5, 3).is_none());
        // With no address to count under, the server's ceiling still holds.
        assert!(take_place_in(counts, None, now, 5, 3).is_none());
        drop(places);
        assert!(take_place_in(counts, None, now, 5, 3).is_some());
    }

    #[test]
    fn an_ipv6_network_counts_as_one_address() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(bucket(a), bucket(b));
        assert_ne!(bucket(a), bucket(c));

        let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
        assert_eq!(bucket(mapped), "192.0.2.1".parse::<IpAddr>().unwrap());
    }
}
