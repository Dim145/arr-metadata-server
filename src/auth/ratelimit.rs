//! Per-peer rate limiting for the native API.
//!
//! The compatibility surfaces are deliberately not limited: Sonarr and Radarr
//! refresh whole libraries in bursts, and throttling them would look like the
//! metadata server being down. The native API is interactive, so a burst there
//! is either a bug or an attack.
//!
//! Alone, the count is kept in memory. Among several instances it is kept on
//! the cache server, in windows of a minute, so a peer's quota is one quota
//! whichever instance answers it — with the memory's count as the fallback
//! while the server does not answer.

use std::{
    net::{IpAddr, Ipv4Addr},
    num::NonZeroU32,
    sync::Arc,
    time::Duration,
};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use governor::{Quota, RateLimiter, clock::DefaultClock, state::keyed::DefaultKeyedStateStore};

use crate::{
    auth::{ip, middleware::ClientAddr},
    cache::RedisSlot,
    error::{AppError, AppResult},
    state::AppState,
};

type Keyed = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

/// Peers with no resolvable address share this bucket rather than bypassing the
/// limiter entirely.
const UNKNOWN_PEER: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// A window's key lives this long past its minute, so a count is never
/// lost to a clock a second off between two instances.
const WINDOW_TTL: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct Limiter {
    local: Option<Arc<Keyed>>,
    per_minute: u32,
    /// The server the count is kept on, and the prefix its keys carry.
    shared: Option<(RedisSlot, String)>,
}

impl Limiter {
    /// `per_minute == 0` disables limiting. `shared` names the cache server
    /// the count is kept on among several instances.
    pub fn new(per_minute: u32, shared: Option<(RedisSlot, String)>) -> Self {
        let Some(quota) = NonZeroU32::new(per_minute) else {
            tracing::info!("rate limiting is disabled");
            return Self {
                local: None,
                per_minute,
                shared: None,
            };
        };

        Self {
            local: Some(Arc::new(RateLimiter::keyed(Quota::per_minute(quota)))),
            per_minute,
            shared,
        }
    }

    fn check_local(&self, peer: IpAddr) -> bool {
        match &self.local {
            None => true,
            Some(limiter) => limiter.check_key(&peer).is_ok(),
        }
    }

    /// Whether the peer may go on: within the shared window when there is
    /// one and the server answers, within the memory's bucket otherwise.
    async fn check(&self, peer: IpAddr) -> bool {
        if self.local.is_none() {
            return true;
        }
        if let Some((slot, prefix)) = &self.shared
            && let Some(redis) = slot.load_full()
        {
            let minute = crate::cache::now_secs() / 60;
            let key = format!("{prefix}rl:{peer}:{minute}");
            if let Some(count) = redis.incr_window(&key, WINDOW_TTL).await {
                return count <= u64::from(self.per_minute);
            }
        }
        self.check_local(peer)
    }

    /// Drop buckets for peers that have gone quiet.
    ///
    /// Without this, one bucket accumulates per distinct address seen, forever.
    pub fn prune(&self) {
        if let Some(limiter) = &self.local {
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
            ip::resolve(
                connect,
                request.headers(),
                &state.config.server.trusted_proxies,
            )
        })
        .unwrap_or(UNKNOWN_PEER);

    if !state.limiter.check(peer).await {
        tracing::warn!(%peer, path = %request.uri().path(), "rate limited");
        return Err(AppError::RateLimited);
    }

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_disabled_limiter_always_allows() {
        let limiter = Limiter::new(0, None);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        for _ in 0..1000 {
            assert!(limiter.check(peer).await);
        }
    }

    #[tokio::test]
    async fn a_peer_is_cut_off_once_its_quota_is_spent() {
        let limiter = Limiter::new(3, None);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert!(limiter.check(peer).await);
        assert!(limiter.check(peer).await);
        assert!(limiter.check(peer).await);
        assert!(!limiter.check(peer).await);
    }

    #[tokio::test]
    async fn peers_do_not_consume_each_others_quota() {
        let limiter = Limiter::new(2, None);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        assert!(limiter.check(a).await);
        assert!(limiter.check(a).await);
        assert!(!limiter.check(a).await);

        assert!(limiter.check(b).await);
        assert!(limiter.check(b).await);
    }

    /// A shared limiter whose server is not attached counts in memory.
    #[tokio::test]
    async fn without_the_server_the_memory_counts() {
        let limiter = Limiter::new(1, Some((RedisSlot::default(), "ams:".into())));
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
        assert!(limiter.check(peer).await);
        assert!(!limiter.check(peer).await);
    }
}
