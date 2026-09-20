//! This server's own API, at `/api/v1`.
//!
//! It is what the web UI talks to, and what a script would use to curate the
//! catalogue. Unlike the compatibility surfaces, it speaks the canonical model
//! directly.

pub mod auth;
pub mod clients;
pub mod items;
pub mod meta;
pub mod overrides;

use axum::Router;

use crate::state::AppState;

/// Routes that require an authenticated caller.
pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/api/v1", items::router())
        .nest("/api/v1", overrides::router())
        .nest("/api/v1", clients::router())
        .nest("/api/v1", meta::router())
        .nest("/api/v1", auth::authenticated_router())
}

/// Routes that must stay reachable without a credential, or nobody could ever
/// obtain one.
pub fn public_router() -> Router<AppState> {
    Router::new().nest("/api/v1", auth::public_router())
}
