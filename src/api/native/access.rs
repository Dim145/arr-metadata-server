//! The way in, as an administrator sees it on one page: whether the site is
//! open, who may sign up, and which APIs answer, how and how much.
//!
//! The answers themselves are settings (`site.access`, `registration.*`,
//! `api.*`, `keys.maxPerUser`, `tmdb.relayCredential`,
//! `tvdb.relayCredential`) and are changed through `/settings/server/-` like
//! any other. This reads them back together with what they govern.

use axum::{Extension, Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        native::{figures, meta::Policy},
        relay,
    },
    auth::Identity,
    config::{Api, Surface},
    db::repo::{self, user::Role},
    error::AppResult,
    state::{AppState, Registration},
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(access))
}

/// Who may browse the catalogue: `site.access`, unless
/// `AMS_PUBLIC_BROWSE=false` keeps the site private whatever it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SiteAccess {
    /// Anyone browses the catalogue.
    Public,
    /// Every page asks to sign in first.
    Private,
}

impl SiteAccess {
    /// As it applies now.
    pub fn of(state: &AppState) -> Self {
        if state.public_site() {
            Self::Public
        } else {
            Self::Private
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiState {
    pub api: Api,
    /// Whether an administrator left it on.
    pub enabled: bool,
    /// What it asks of a caller. Set by the deployment's environment, not
    /// here — except that a relay asked with each client's own key asks for
    /// a listed address where the environment says a key.
    pub policy: Policy,
    /// Calls answered and refused since the server started.
    pub served: u64,
    pub refused: u64,
}

/// A relay whose service takes a key of its own: TMDB's and TheTVDB's.
/// AniList's takes none, and has no credential to choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RelayApi {
    Tmdb,
    Tvdb,
}

/// Whose key a relay asks its service with (`tmdb.relayCredential`,
/// `tvdb.relayCredential`), and what stands around that choice.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelayCredentialState {
    pub api: RelayApi,
    /// `server`: this server's key in place of the client's. `client`: the
    /// client's own, as it came.
    pub credential: relay::RelayCredential,
    /// Whether this server holds a key of the service's own to stand in
    /// with: without one, `server` answers 503.
    pub operator_key: bool,
    /// What the environment asks of a caller (`AMS_TMDB_AUTH`,
    /// `AMS_TVDB_AUTH`), before the choice above reads a key policy as the
    /// address list.
    pub configured_policy: Policy,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    pub site: SiteAccess,
    /// AMS_PUBLIC_BROWSE=false keeps the site private whatever is chosen here.
    pub site_locked: bool,
    /// Whether members may use the relays — TMDB's, TheTVDB's, AniList's —
    /// with their keys.
    pub relay_for_members: bool,
    pub registration: Registration,
    /// The role of an account opened without an invitation.
    pub registration_role: Role,
    /// Keys a member or an editor may hold.
    pub keys_per_user: i64,
    /// AMS_AUTH_DISABLED: every surface is open, whatever the rest says.
    pub auth_disabled: bool,
    pub uptime_seconds: u64,
    /// Since when the APIs' numbers count, among several instances: the
    /// tally on the cache server outlives any one of them. Absent alone,
    /// where they count since the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counted_since: Option<String>,
    pub apis: Vec<ApiState>,
    /// Whose key each relay that takes one asks its service with.
    pub relay_credentials: Vec<RelayCredentialState>,
    /// Accounts waiting for an administrator.
    pub pending: i64,
    /// Invitations still usable, and the accounts they may still open.
    pub invitations: i64,
    pub places: i64,
    /// Addresses the allowlist lets through.
    pub allowlist_rules: usize,
    /// Keys the server holds, and keys people hold.
    pub server_keys: usize,
    pub personal_keys: usize,
}

#[utoipa::path(
    get, path = "/admin/access", tag = TAG,
    operation_id = "access_access",
    responses(
        (status = 200, body = Access),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn access(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Access>> {
    identity.require_admin()?;

    // Among several instances the counts are added up on the cache server,
    // so the page shows what every instance answered.
    let shared = state
        .coord
        .is_multi()
        .then(|| state.caches.redis())
        .flatten();
    let mut apis = Vec::with_capacity(Api::ALL.len());
    for api in Api::ALL {
        let surface = match api {
            Api::Sonarr | Api::Radarr => Surface::Arr,
            Api::Tmdb => Surface::Tmdb,
            Api::Tvdb => Surface::Tvdb,
            Api::Anilist => Surface::Anilist,
            Api::Native => Surface::Native,
            Api::Plex => Surface::Plex,
        };
        let (served, refused) = match &shared {
            Some(redis) => {
                state
                    .calls
                    .read_shared(redis, &state.config.cache.redis_prefix, api)
                    .await
            }
            None => state.calls.read(api),
        };
        apis.push(ApiState {
            api,
            enabled: state.api_on(api),
            policy: relay::policy(&state, surface).into(),
            served,
            refused,
        });
    }

    let relay_credentials = [
        (
            RelayApi::Tmdb,
            Surface::Tmdb,
            state.config.tmdb.api_key.is_some(),
        ),
        (RelayApi::Tvdb, Surface::Tvdb, state.tvdb.is_configured()),
    ]
    .into_iter()
    .map(|(api, surface, operator_key)| RelayCredentialState {
        api,
        credential: relay::credential(&state, surface),
        operator_key,
        configured_policy: state.config.policy_for(surface).into(),
    })
    .collect();

    let (invitations, places) = repo::invitation::count_usable(&state.db).await?;
    let keys = repo::client::list(&state.db, repo::client::Owner::Any).await?;
    let personal_keys = keys.iter().filter(|k| k.owner_id.is_some()).count();

    Ok(Json(Access {
        site: SiteAccess::of(&state),
        site_locked: state.site_locked(),
        relay_for_members: state.relay_for_members(),
        registration: state.registration(),
        registration_role: state.registration_role(),
        keys_per_user: state.keys_per_user(),
        auth_disabled: state.config.security.auth_disabled,
        uptime_seconds: figures::uptime_seconds(),
        counted_since: match &shared {
            Some(redis) => redis
                .get_text(&crate::state::Calls::since_key(
                    &state.config.cache.redis_prefix,
                ))
                .await
                .flatten(),
            None => None,
        },
        apis,
        relay_credentials,
        pending: repo::user::counts(&state.db).await?.pending,
        invitations,
        places,
        allowlist_rules: state.allowlist().len(),
        server_keys: keys.len() - personal_keys,
        personal_keys,
    }))
}
