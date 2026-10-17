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
//!
//! An address is counted by [`ip::bucket`]: an IPv6 client by its /64, since
//! one machine has a whole /64 to pick a fresh address from.
//!
//! Signing in has a count of its own besides: the failed attempts on each
//! account, so that guessing one person's password from many addresses runs
//! out as surely as guessing it from one.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use governor::{
    Quota, RateLimiter,
    clock::{Clock as _, DefaultClock},
    state::keyed::DefaultKeyedStateStore,
};
use sha2::{Digest, Sha256};

use crate::{
    auth::{ip, middleware::ClientAddr},
    cache::RedisSlot,
    error::{AppError, AppResult, Throttle},
    state::AppState,
};

/// The window an address's quota is counted over.
const MINUTE: Duration = Duration::from_secs(60);

type Keyed = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

/// Peers with no resolvable address share this bucket rather than bypassing the
/// limiter entirely.
const UNKNOWN_PEER: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// A window's key lives this long past its minute, so a count is never
/// lost to a clock a second off between two instances.
const WINDOW_TTL: Duration = Duration::from_secs(120);

/// How long a run of failed sign-ins on one account is remembered.
pub const SIGN_IN_WINDOW: Duration = Duration::from_secs(15 * 60);

/// The most accounts whose failures are kept in memory at once. The names
/// come from whoever can reach the sign-in form; past this, a name not
/// already counted is not counted — one already counted, the one under
/// attack, keeps its count.
const SIGN_IN_ACCOUNTS: usize = 65_536;

#[derive(Clone)]
pub struct Limiter {
    local: Option<Arc<Keyed>>,
    per_minute: u32,
    /// The server the count is kept on, and the prefix its keys carry.
    shared: Option<(RedisSlot, String)>,
    /// Failed sign-ins, by account.
    sign_ins: Arc<SignIns>,
}

/// Failed sign-ins on each account in the current window, in memory: the
/// count itself, and when its window opened. Keyed by a digest of the name
/// as typed, folded to lower case, so the map holds no names and no more
/// than a few bytes for each.
struct SignIns {
    /// `0` turns the count off.
    per_account: u32,
    counts: Mutex<HashMap<u128, (u32, Instant)>>,
}

impl Limiter {
    /// `per_minute == 0` disables limiting. `shared` names the cache server
    /// the count is kept on among several instances. `sign_ins_per_account`
    /// is how many failed sign-ins an account takes in [`SIGN_IN_WINDOW`]
    /// before it is refused without being checked; `0` never refuses.
    pub fn new(
        per_minute: u32,
        shared: Option<(RedisSlot, String)>,
        sign_ins_per_account: u32,
    ) -> Self {
        let sign_ins = Arc::new(SignIns {
            per_account: sign_ins_per_account,
            counts: Mutex::new(HashMap::new()),
        });

        let Some(quota) = NonZeroU32::new(per_minute) else {
            tracing::info!("rate limiting is disabled");
            return Self {
                local: None,
                per_minute,
                shared,
                sign_ins,
            };
        };

        Self {
            local: Some(Arc::new(RateLimiter::keyed(Quota::per_minute(quota)))),
            per_minute,
            shared,
            sign_ins,
        }
    }

    /// Whether the peer may go on — within the shared window when there is
    /// one and the server answers, within the memory's bucket otherwise — or
    /// how long until it may.
    async fn check(&self, peer: IpAddr) -> Result<(), Duration> {
        let Some(limiter) = &self.local else {
            return Ok(());
        };
        if let Some((slot, prefix)) = &self.shared
            && let Some(redis) = slot.load_full()
        {
            let now = crate::cache::now_secs();
            let minute = now / MINUTE.as_secs();
            let key = format!("{prefix}rl:{peer}:{minute}");
            if let Some(count) = redis.incr_window(&key, WINDOW_TTL).await {
                // Spent, the window is the minute: the next one opens it again.
                return if count <= u64::from(self.per_minute) {
                    Ok(())
                } else {
                    Err(until_window_closes(now, MINUTE))
                };
            }
        }
        limiter
            .check_key(&peer)
            .map(|_| ())
            .map_err(|refused| refused.wait_time_from(limiter.clock().now()))
    }

    /// Whether `account` may try a password now — fewer failures in this
    /// window than the limit — or how long until its window closes. Asked
    /// before anything is hashed, so a refused attempt costs nothing.
    pub async fn sign_in_allowed(&self, account: &str) -> Result<(), Duration> {
        let limit = self.sign_ins.per_account;
        if limit == 0 {
            return Ok(());
        }
        if let Some((key, redis)) = self.shared_sign_in_key(account)
            && let Some(counts) = redis.mget_u64(std::slice::from_ref(&key)).await
        {
            return if counts.first().copied().unwrap_or_default() < u64::from(limit) {
                Ok(())
            } else {
                Err(until_window_closes(
                    crate::cache::now_secs(),
                    SIGN_IN_WINDOW,
                ))
            };
        }
        let (key, now) = (digest(account), Instant::now());
        if self.sign_ins.failures(key, now) < limit {
            Ok(())
        } else {
            Err(self.sign_ins.closes_in(key, now))
        }
    }

    /// Count a failed sign-in on `account`.
    pub async fn sign_in_failed(&self, account: &str) {
        if self.sign_ins.per_account == 0 {
            return;
        }
        if let Some((key, redis)) = self.shared_sign_in_key(account) {
            // Twice the window: the key names its own window, and the one
            // before must still answer for a clock a little off.
            if redis.incr_window(&key, SIGN_IN_WINDOW * 2).await.is_some() {
                return;
            }
        }
        self.sign_ins.fail(digest(account), Instant::now());
    }

    /// A sign-in went through: the account's count starts again.
    pub async fn sign_in_succeeded(&self, account: &str) {
        if self.sign_ins.per_account == 0 {
            return;
        }
        if let Some((key, redis)) = self.shared_sign_in_key(account) {
            redis.unlink(&key).await;
        }
        if let Ok(mut counts) = self.sign_ins.counts.lock() {
            counts.remove(&digest(account));
        }
    }

    /// The key an account's failures are counted under on the cache server,
    /// and the server, when there is one to count on.
    fn shared_sign_in_key(&self, account: &str) -> Option<(String, Arc<crate::cache::Redis>)> {
        let (slot, prefix) = self.shared.as_ref()?;
        let redis = slot.load_full()?;
        let window = crate::cache::now_secs() / SIGN_IN_WINDOW.as_secs();
        Some((
            format!("{prefix}signin:{:032x}:{window}", digest(account)),
            redis,
        ))
    }

    /// Drop buckets for peers that have gone quiet.
    ///
    /// Without this, one bucket accumulates per distinct address seen, forever.
    pub fn prune(&self) {
        if let Some(limiter) = &self.local {
            limiter.retain_recent();
        }
        self.sign_ins.prune(Instant::now());
    }
}

impl SignIns {
    /// The failures counted in the window open at `now`.
    fn failures(&self, key: u128, now: Instant) -> u32 {
        let Ok(counts) = self.counts.lock() else {
            // A poisoned lock refuses nobody: the address limiter and the
            // hashing bound still hold, and a lock-out of everyone would be
            // worse than a count lost.
            return 0;
        };
        match counts.get(&key) {
            Some((count, since)) if now.duration_since(*since) < SIGN_IN_WINDOW => *count,
            _ => 0,
        }
    }

    /// How long until the window counted at `now` closes; nothing when none
    /// is open.
    fn closes_in(&self, key: u128, now: Instant) -> Duration {
        let Ok(counts) = self.counts.lock() else {
            return Duration::ZERO;
        };
        counts.get(&key).map_or(Duration::ZERO, |(_, since)| {
            SIGN_IN_WINDOW.saturating_sub(now.duration_since(*since))
        })
    }

    fn fail(&self, key: u128, now: Instant) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        if !counts.contains_key(&key) && counts.len() >= SIGN_IN_ACCOUNTS {
            counts.retain(|_, (_, since)| now.duration_since(*since) < SIGN_IN_WINDOW);
            if counts.len() >= SIGN_IN_ACCOUNTS {
                return;
            }
        }
        let entry = counts.entry(key).or_insert((0, now));
        if now.duration_since(entry.1) >= SIGN_IN_WINDOW {
            *entry = (0, now);
        }
        entry.0 = entry.0.saturating_add(1);
    }

    fn prune(&self, now: Instant) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.retain(|_, (_, since)| now.duration_since(*since) < SIGN_IN_WINDOW);
        }
    }
}

/// How long until a window that started on a multiple of its length, as the
/// shared counts' keys do, gives way to the next: from `now`, in seconds
/// since the epoch.
fn until_window_closes(now: u64, window: Duration) -> Duration {
    let length = window.as_secs().max(1);
    Duration::from_secs(length - now % length)
}

/// What an account is counted under: its name as typed, trimmed and folded
/// to lower case — usernames compare without regard to case — digested.
fn digest(account: &str) -> u128 {
    let folded = account.trim().to_lowercase();
    let hash = Sha256::digest(folded.as_bytes());
    let mut head = [0u8; 16];
    head.copy_from_slice(&hash[..16]);
    u128::from_be_bytes(head)
}

pub async fn limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    // The guard runs first and leaves the resolved address behind; fall back to
    // resolving it here for routes that are not behind a guard. A caller whose
    // address is unknown is counted with every other unknown one.
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
        .map_or(UNKNOWN_PEER, ip::bucket);

    if let Err(wait) = state.limiter.check(peer).await {
        tracing::warn!(%peer, path = %crate::telemetry::redact_path(request.uri().path()), "rate limited");
        return Err(AppError::RateLimited(Throttle::spent(
            state.limiter.per_minute,
            MINUTE,
            wait,
        )));
    }

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_disabled_limiter_always_allows() {
        let limiter = Limiter::new(0, None, 0);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        for _ in 0..1000 {
            assert!(limiter.check(peer).await.is_ok());
        }
    }

    #[tokio::test]
    async fn a_peer_is_cut_off_once_its_quota_is_spent() {
        let limiter = Limiter::new(3, None, 0);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert!(limiter.check(peer).await.is_ok());
        assert!(limiter.check(peer).await.is_ok());
        assert!(limiter.check(peer).await.is_ok());
        assert!(limiter.check(peer).await.is_err());
    }

    #[tokio::test]
    async fn peers_do_not_consume_each_others_quota() {
        let limiter = Limiter::new(2, None, 0);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        assert!(limiter.check(a).await.is_ok());
        assert!(limiter.check(a).await.is_ok());
        assert!(limiter.check(a).await.is_err());

        assert!(limiter.check(b).await.is_ok());
        assert!(limiter.check(b).await.is_ok());
    }

    /// One machine with a /64 to pick from is one peer, whichever address
    /// it calls from.
    #[tokio::test]
    async fn an_ipv6_network_shares_one_quota() {
        let limiter = Limiter::new(2, None, 0);
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2::2".parse().unwrap();

        assert!(limiter.check(ip::bucket(a)).await.is_ok());
        assert!(limiter.check(ip::bucket(b)).await.is_ok());
        assert!(limiter.check(ip::bucket(a)).await.is_err());
        let elsewhere: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert!(limiter.check(ip::bucket(elsewhere)).await.is_ok());
    }

    /// A shared limiter whose server is not attached counts in memory.
    #[tokio::test]
    async fn without_the_server_the_memory_counts() {
        let limiter = Limiter::new(1, Some((RedisSlot::default(), "ams:".into())), 0);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
        assert!(limiter.check(peer).await.is_ok());
        assert!(limiter.check(peer).await.is_err());
    }

    #[tokio::test]
    async fn an_account_is_refused_after_its_failures_whatever_the_case() {
        let limiter = Limiter::new(0, None, 3);

        for _ in 0..3 {
            assert!(limiter.sign_in_allowed("Admin").await.is_ok());
            limiter.sign_in_failed(" admin ").await;
        }
        assert!(limiter.sign_in_allowed("ADMIN").await.is_err());
        // Another account is another count.
        assert!(limiter.sign_in_allowed("alice").await.is_ok());

        // A sign-in that went through starts the count again.
        limiter.sign_in_succeeded("admin").await;
        assert!(limiter.sign_in_allowed("admin").await.is_ok());
    }

    /// A peer refused is told how long until a request would go through:
    /// three a minute is one every twenty seconds, once the three are spent.
    #[tokio::test]
    async fn a_refusal_says_how_long_to_wait() {
        let limiter = Limiter::new(3, None, 0);
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4));
        for _ in 0..3 {
            assert!(limiter.check(peer).await.is_ok());
        }
        let wait = limiter.check(peer).await.expect_err("refused");
        assert!(
            wait > Duration::from_secs(19) && wait <= Duration::from_secs(20),
            "{wait:?}"
        );
    }

    /// An account refused is told when its window closes, from its first
    /// failure.
    #[tokio::test]
    async fn an_account_refused_is_told_when_its_window_closes() {
        let limiter = Limiter::new(0, None, 1);
        limiter.sign_in_failed("admin").await;
        let wait = limiter.sign_in_allowed("admin").await.expect_err("refused");
        assert!(
            wait > SIGN_IN_WINDOW - Duration::from_secs(5) && wait <= SIGN_IN_WINDOW,
            "{wait:?}"
        );
    }

    #[test]
    fn a_shared_window_closes_at_the_next_multiple_of_its_length() {
        assert_eq!(until_window_closes(120, MINUTE), Duration::from_secs(60));
        assert_eq!(until_window_closes(121, MINUTE), Duration::from_secs(59));
        assert_eq!(until_window_closes(179, MINUTE), Duration::from_secs(1));
        assert_eq!(
            until_window_closes(900 * 7 + 300, SIGN_IN_WINDOW),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn a_window_closes_and_a_full_map_keeps_what_it_counts() {
        let sign_ins = SignIns {
            per_account: 1,
            counts: Mutex::new(HashMap::new()),
        };
        let start = Instant::now();
        sign_ins.fail(1, start);
        sign_ins.fail(1, start);
        assert_eq!(sign_ins.failures(1, start), 2);
        // Fifteen minutes on, the window has closed.
        let later = start + SIGN_IN_WINDOW;
        assert_eq!(sign_ins.failures(1, later), 0);
        sign_ins.fail(1, later);
        assert_eq!(sign_ins.failures(1, later), 1);
        sign_ins.prune(later + SIGN_IN_WINDOW);
        assert!(sign_ins.counts.lock().unwrap().is_empty());

        // Filled to the brim with fresh names, an account counted already
        // is still counted, and a new one is not.
        let full = SignIns {
            per_account: 1,
            counts: Mutex::new(
                (0..SIGN_IN_ACCOUNTS as u128)
                    .map(|k| (k, (1, start)))
                    .collect(),
            ),
        };
        full.fail(7, start);
        assert_eq!(full.failures(7, start), 2);
        full.fail(u128::MAX, start);
        assert_eq!(full.failures(u128::MAX, start), 0);
    }

    #[tokio::test]
    async fn no_limit_counts_nothing() {
        let limiter = Limiter::new(0, None, 0);
        for _ in 0..100 {
            limiter.sign_in_failed("admin").await;
        }
        assert!(limiter.sign_in_allowed("admin").await.is_ok());
    }
}
