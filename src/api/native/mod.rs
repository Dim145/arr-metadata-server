//! This server's own API, at `/api/v1`.
//!
//! It is what the web UI talks to, and what a script would use to curate the
//! catalogue. Unlike the compatibility surfaces, it speaks the canonical model
//! directly.

pub mod auth;
pub mod browse;
pub mod children;
pub mod clients;
pub mod discover;
pub mod export;
pub mod feeds;
pub mod figures;
pub mod items;
pub mod lists;
pub mod locks;
pub mod meta;
pub mod network;
pub mod orders;
pub mod overrides;
pub mod recommend;
pub mod seasons;
pub mod settings;
pub mod watch;

use utoipa_axum::router::OpenApiRouter;

use crate::state::AppState;

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
        .merge(overrides::router())
        .merge(clients::router())
        .merge(meta::router())
        .merge(network::router())
        .merge(settings::router())
        .merge(discover::router())
        .merge(auth::authenticated_router())
        .merge(crate::api::audit::router())
}

/// Routes that must stay reachable without a credential, or nobody could ever
/// obtain one.
pub fn public_router() -> OpenApiRouter<AppState> {
    auth::public_router()
}
