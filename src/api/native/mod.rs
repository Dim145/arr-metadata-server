//! This server's own API, at `/api/v1`.
//!
//! It is what the web UI talks to, and what a script would use to curate the
//! catalogue. Unlike the compatibility surfaces, it speaks the canonical model
//! directly.

pub mod access;
pub mod account;
pub mod auth;
pub mod browse;
pub mod cache;
pub mod children;
pub mod clients;
pub mod discover;
pub mod export;
pub mod feeds;
pub mod figures;
pub mod invitations;
pub mod items;
pub mod lists;
pub mod locks;
pub mod media;
pub mod meta;
pub mod network;
pub mod oidc;
pub mod orders;
pub mod overrides;
pub mod recommend;
pub mod rules;
pub mod seasons;
pub mod settings;
pub mod signup;
pub mod tasks;
pub mod tls;
pub mod users;
pub mod watch;

use utoipa_axum::router::OpenApiRouter;

use crate::{
    auth::Identity,
    domain::MediaItem,
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// Whether a work is kept from this reader: switched off, or for adults where
/// the reader's policy would not show one even were they to ask. Whoever
/// maintains the catalogue is kept from nothing — they open it to put it right.
pub(crate) fn hidden_from(state: &AppState, identity: &Identity, item: &MediaItem) -> bool {
    !identity.can_write()
        && (!item.is_enabled
            || (item.is_adult
                && !state.adult_for(identity.client_id(), identity.peer_id(), Some(true))))
}

/// A work this reader may see, as its own page decides it; for one kept from
/// them, no work at all — knowing its id is no reason to be shown it.
pub(crate) async fn visible_work(
    state: &AppState,
    identity: &Identity,
    id: &str,
) -> AppResult<MediaItem> {
    let item = service::load(state, id).await?.ok_or(AppError::NotFound)?;
    if hidden_from(state, identity, &item) {
        return Err(AppError::NotFound);
    }
    Ok(item)
}

/// Routes that require an authenticated caller.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(items::router())
        .merge(browse::router())
        .merge(feeds::router())
        .merge(lists::router())
        .merge(watch::router())
        .merge(recommend::router())
        .merge(figures::router())
        .merge(orders::router())
        .merge(locks::router())
        .merge(seasons::router())
        .merge(children::router())
        .merge(export::router())
        .merge(tls::router())
        .merge(cache::router())
        .merge(overrides::router())
        .merge(clients::router())
        .merge(meta::router())
        .merge(network::router())
        .merge(settings::router())
        .merge(discover::router())
        .merge(account::router())
        .merge(users::router())
        .merge(invitations::router())
        .merge(access::router())
        .merge(oidc::router())
        .merge(tasks::router())
        .merge(rules::router())
        .merge(media::router())
        .merge(auth::authenticated_router())
        .merge(crate::api::audit::router())
}

/// Routes that must stay reachable without a credential, or nobody could ever
/// obtain one.
pub fn public_router() -> OpenApiRouter<AppState> {
    auth::public_router()
        .merge(signup::router())
        .merge(oidc::public_router())
}

/// What the tests of a handler run against.
#[cfg(test)]
pub(crate) mod testing {
    use crate::{config, state::AppState};

    /// The whole server, on a database in memory: no provider to ask, no
    /// cache server, nothing kept on disk, and adult titles hidden, as a new
    /// server has them.
    pub async fn server() -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        };
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tmdb.include_adult = false;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;

        AppState::bootstrap(config).await.expect("a server")
    }
}
