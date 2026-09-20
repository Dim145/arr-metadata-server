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

use utoipa_axum::router::OpenApiRouter;

use crate::state::AppState;

/// Routes that require an authenticated caller.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(items::router())
        .merge(overrides::router())
        .merge(clients::router())
        .merge(meta::router())
        .merge(auth::authenticated_router())
        .merge(crate::api::audit::router())
}

/// Routes that must stay reachable without a credential, or nobody could ever
/// obtain one.
pub fn public_router() -> OpenApiRouter<AppState> {
    auth::public_router()
}
