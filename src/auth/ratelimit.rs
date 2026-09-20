//! Per-peer rate limiting for the native API.
//!
//! The compatibility surfaces are deliberately not limited: Sonarr and Radarr
//! refresh whole libraries in bursts, and throttling them would look like the
//! metadata server being down. The native API is interactive, so a burst there
//! is either a bug or an attack.

use std::{
    net::{IpAddr, Ipv4Addr},
    num::NonZeroU32,
    sync::Arc,
};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use governor::{Quota, RateLimiter, clock::DefaultClock, state::keyed::DefaultKeyedStateStore};

use crate::{
    auth::{ip, middleware::ClientAddr},
    error::{AppError, AppResult},
    state::AppState,
};

type Keyed = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

/// Peers with no resolvable address share this bucket rather than bypassing the
/// limiter entirely.
const UNKNOWN_PEER: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

#[derive(Clone)]
pub struct Limiter(Option<Arc<Keyed>>);

impl Limiter {
    /// `per_minute == 0` disables limiting.
    pub fn new(per_minute: u32) -> Self {
        let Some(quota) = NonZeroU32::new(per_minute) else {
            tracing::info!("rate limiting is disabled");
            return Self(None);
        };

        Self(Some(Arc::new(RateLimiter::keyed(Quota::per_minute(quota)))))
    }

    fn check(&self, peer: IpAddr) -> bool {
        match &self.0 {
            None => true,
            Some(limiter) => limiter.check_key(&peer).is_ok(),
        }
    }

    /// Drop buckets for peers that have gone quiet.
    ///
    /// Without this, one bucket accumulates per distinct address seen, forever.
    pub fn prune(&self) {
        if let Some(limiter) = &self.0 {
            limiter.retain_recent();
        }
    }
}

pub async fn limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    // The guard runs first and leaves the resolved address behind; fall back to
    // resolving it here for routes that are not behind a guard.
    let peer = request
        .extensions()
        .get::<ClientAddr>()
        .map(|c| c.0)
        .or_else(|| {
            let connect = request
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|ci| ci.0);
            ip::resolve(connect, request.headers(), &state.config.server.trusted_proxies)
        })
        .unwrap_or(UNKNOWN_PEER);

    if !state.limiter.check(peer) {
        tracing::warn!(%peer, path = %request.uri().path(), "rate limited");
        return Err(AppError::RateLimited);
    }

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_limiter_always_allows() {
        let limiter = Limiter::new(0);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        for _ in 0..1000 {
            assert!(limiter.check(peer));
        }
    }

    #[test]
    fn a_peer_is_cut_off_once_its_quota_is_spent() {
        let limiter = Limiter::new(3);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert!(limiter.check(peer));
        assert!(limiter.check(peer));
        assert!(limiter.check(peer));
        assert!(!limiter.check(peer));
    }

    #[test]
    fn peers_do_not_consume_each_others_quota() {
        let limiter = Limiter::new(2);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        assert!(limiter.check(a));
        assert!(limiter.check(a));
        assert!(!limiter.check(a));

        assert!(limiter.check(b));
        assert!(limiter.check(b));
    }
}
