//! HTTP surfaces.
//!
//! Three compatibility surfaces plus this server's own API. They are separate
//! routers because they have separate authentication policies — see
//! [`crate::config::Surface`].

pub mod audit;
pub mod extract;
pub mod native;
pub mod radarr;
pub mod sonarr;
pub mod tmdb;

use axum::{Router, middleware::from_fn_with_state};

use crate::{auth::middleware as guards, state::AppState};

/// Sonarr and Radarr compatibility, guarded by network policy.
pub fn arr_router(state: AppState) -> Router<AppState> {
    sonarr::router()
        .merge(radarr::router())
        .layer(from_fn_with_state(state, guards::guard_arr))
}

/// TMDB compatibility, guarded by API key.
pub fn tmdb_router(state: AppState) -> Router<AppState> {
    tmdb::router().layer(from_fn_with_state(state, guards::guard_tmdb))
}

/// This server's own API, guarded by API key or an admin session.
pub fn native_router(state: AppState) -> Router<AppState> {
    native::router().layer(from_fn_with_state(state, guards::guard_native))
}
