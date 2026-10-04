//! The way in, as an administrator sees it on one page: whether the site is
//! open, who may sign up, and which APIs answer, how and how much.
//!
//! The answers themselves are settings (`site.access`, `registration.*`,
//! `api.*`, `keys.maxPerUser`) and are changed through `/settings/server/-`
//! like any other. This reads them back together with what they govern.

use axum::{Extension, Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::native::{figures, meta},
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

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiState {
    pub api: Api,
    /// Whether an administrator left it on.
    pub enabled: bool,
    /// What it asks of a caller: `apikey`, `allowlist` or `open`. Set by the
    /// deployment's environment, not here.
    pub policy: &'static str,
    /// Calls answered and refused since the server started.
    pub served: u64,
    pub refused: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    /// `public` or `private`.
    pub site: &'static str,
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
            policy: meta::policy_name(state.config.policy_for(surface)),
            served,
            refused,
        });
    }

    let (invitations, places) = repo::invitation::count_usable(&state.db).await?;
    let keys = repo::client::list(&state.db, repo::client::Owner::Any).await?;
    let personal_keys = keys.iter().filter(|k| k.owner_id.is_some()).count();

    Ok(Json(Access {
        site: if state.public_site() {
            "public"
        } else {
            "private"
        },
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
        pending: repo::user::counts(&state.db).await?.pending,
        invitations,
        places,
        allowlist_rules: state.allowlist().len(),
        server_keys: keys.len() - personal_keys,
        personal_keys,
    }))
}
