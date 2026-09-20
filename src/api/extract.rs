//! Shared extractors.

use std::net::IpAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::request::Parts,
};

use crate::{auth::ip, error::AppError, state::AppState};

/// The caller's address, as the allowlist and the audit log understand it.
///
/// A guard normally resolves this once and leaves it in the request extensions.
/// Routes that run before any guard — sign-in, most obviously — have no such
/// extension, so this falls back to resolving it here. Both paths honour
/// `X-Forwarded-For` only from configured proxies.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub Option<IpAddr>);

impl ClientIp {
    pub fn as_text(&self) -> Option<String> {
        self.0.map(|ip| ip.to_string())
    }
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(addr) = parts
            .extensions
            .get::<crate::auth::middleware::ClientAddr>()
        {
            return Ok(Self(Some(addr.0)));
        }

        let peer = parts
            .extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0);

        Ok(Self(ip::resolve(
            peer,
            &parts.headers,
            &state.config.server.trusted_proxies,
        )))
    }
}
