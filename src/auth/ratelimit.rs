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
//! Signing in has counts of its own besides: the attempts on each account
//! from each address — so that one person's slips, or somebody's guessing,
//! shut out the address they come from and nobody else — and the attempts on
//! the account from every address together, so that guessing one person's
//! password from many addresses runs out as well. That second bar, higher,
//! does not apply to an address the account has signed in from lately: its
//! owner's usual places always get through with the right password.
//!
//! An attempt is counted before its password is checked and given back when
//! it was right, so that however many arrive at once, no more than the limit
//! are ever checked.

use std::{
    collections::{BTreeMap, HashMap},
    hash::Hash,
    net::{IpAddr, Ipv4Addr},
    num::NonZeroU32,
    sync::Arc,
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
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::{
    auth::{ip, middleware::ClientAddr},
    cache::RedisSlot,
    db::{Db, repo::keystore},
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

/// How long a run of failed sign-ins is remembered.
pub const SIGN_IN_WINDOW: Duration = Duration::from_secs(15 * 60);

/// How many times the attempts an account takes from one address it takes
/// from every address together, before the addresses it has not signed in
/// from lately are turned away too.
const ACCOUNT_BAR_TIMES: u32 = 5;

/// The most counts of one kind kept in memory at once. The names come from
/// whoever can reach the sign-in form; past this the oldest counts make room
/// for new ones, so that filling the map with names is never a way to stop
/// the count of the one under attack.
const SIGN_IN_COUNTS: usize = 65_536;

#[derive(Clone)]
pub struct Limiter {
    local: Option<Arc<Keyed>>,
    per_minute: u32,
    /// The server the count is kept on, and the prefix its keys carry.
    shared: Option<(RedisSlot, String)>,
    /// Attempts at passwords, by account and address.
    sign_ins: Arc<SignIns>,
}

/// Attempts at passwords in the current window, in memory: those on each
/// account from each address, and those on each account from all of them.
/// Keyed by a digest of the name as typed, folded to lower case, so the maps
/// hold no names and no more than a few bytes for each.
struct SignIns {
    /// Attempts an account takes from one address in a window; `0` turns the
    /// count off.
    per_pair: u32,
    pairs: Mutex<Counts<(u128, IpAddr)>>,
    accounts: Mutex<Counts<u128>>,
}

/// What a count is of: an account's attempts from one network, or from every
/// network.
#[derive(Clone, Copy, Debug)]
enum Which {
    Pair(u128, IpAddr),
    Account(u128),
}

impl Which {
    /// The key it is counted under on the cache server, for a window.
    fn shared_key(self, prefix: &str, window: u64) -> String {
        match self {
            Self::Pair(account, peer) => format!("{prefix}signin:{account:032x}:{peer}:{window}"),
            Self::Account(account) => format!("{prefix}signinall:{account:032x}:{window}"),
        }
    }
}

/// Where a count was made, to be reached again when its attempt is settled:
/// the window on the cache server, or the one opened in memory.
#[derive(Clone, Copy, Debug)]
enum Counted {
    Shared(u64),
    Memory(Instant),
}

/// An attempt at a password, counted before the password is checked. Settled
/// by [`Limiter::sign_in_succeeded`] when the password was right, or
/// [`Limiter::sign_in_given_back`] when nothing was checked; left alone, it
/// stands as the failure it is taken for.
#[derive(Clone, Copy, Debug)]
pub struct Reserved {
    account: u128,
    peer: IpAddr,
    /// Where each count was made; none when counting is off.
    pair: Option<Counted>,
    all: Option<Counted>,
    over: Option<Duration>,
}

impl Reserved {
    /// How long until the window closes, when the account has taken more
    /// attempts from every address together than its bar: asked of any
    /// address that has not signed in to it lately ([`address_known`]), and
    /// of that alone.
    pub fn over_account_bar(&self) -> Option<Duration> {
        self.over
    }
}

impl Limiter {
    /// `per_minute == 0` disables limiting. `shared` names the cache server
    /// the count is kept on among several instances. `sign_ins_per_account`
    /// is how many attempts an account takes from one address in
    /// [`SIGN_IN_WINDOW`] before that address is refused without its password
    /// being checked, and five times as many from every address together;
    /// `0` never refuses.
    pub fn new(
        per_minute: u32,
        shared: Option<(RedisSlot, String)>,
        sign_ins_per_account: u32,
    ) -> Self {
        let sign_ins = Arc::new(SignIns {
            per_pair: sign_ins_per_account,
            pairs: Mutex::new(Counts::new(SIGN_IN_COUNTS)),
            accounts: Mutex::new(Counts::new(SIGN_IN_COUNTS)),
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

    /// Count an attempt at `account`'s password from `peer`'s network, before
    /// the password is checked, so that however many arrive at once no more
    /// than the limit are let through to be checked — counted after, each of
    /// a burst found the count still at nothing. Refused, with how long until
    /// the window closes, once this address has made as many attempts on this
    /// account as a window allows: whatever the password, which is then not
    /// looked at.
    ///
    /// Counted on the cache server when there is one and it answers, in
    /// memory otherwise. The same account's password asked from another
    /// address is another count, so that one person's slips shut out their
    /// own address and nobody else's; [`Reserved::over_account_bar`] says
    /// when the account has taken too many from all of them together.
    pub async fn sign_in_begin(&self, account: &str, peer: IpAddr) -> Result<Reserved, Duration> {
        let limit = self.sign_ins.per_pair;
        if limit == 0 {
            return Ok(Reserved {
                account: 0,
                peer,
                pair: None,
                all: None,
                over: None,
            });
        }

        let account = digest(account);
        let peer = ip::bucket(peer);

        let (made, pair) = self.bump(Which::Pair(account, peer)).await;
        if made > u64::from(limit) {
            return Err(self.closes_in(pair));
        }
        let (made, all) = self.bump(Which::Account(account)).await;
        let over =
            (made > u64::from(limit) * u64::from(ACCOUNT_BAR_TIMES)).then(|| self.closes_in(all));

        Ok(Reserved {
            account,
            peer,
            pair: Some(pair),
            all: Some(all),
            over,
        })
    }

    /// The password was right: the attempt was no failure. This address's
    /// count on the account starts again, and the account's takes back the
    /// one this attempt made.
    pub async fn sign_in_succeeded(&self, reserved: Reserved) {
        if let Some(pair) = reserved.pair {
            self.clear(Which::Pair(reserved.account, reserved.peer), pair)
                .await;
        }
        if let Some(all) = reserved.all {
            self.give_back(Which::Account(reserved.account), all).await;
        }
    }

    /// Nothing was checked — every hashing slot stayed taken, or the account's
    /// bar turned the attempt away: it is taken back from both counts, as if
    /// it had not been made.
    pub async fn sign_in_given_back(&self, reserved: Reserved) {
        if let Some(pair) = reserved.pair {
            self.give_back(Which::Pair(reserved.account, reserved.peer), pair)
                .await;
        }
        if let Some(all) = reserved.all {
            self.give_back(Which::Account(reserved.account), all).await;
        }
    }

    /// One more on a count, and where it was made: in the shared window when
    /// there is one and the server answers, in memory otherwise.
    async fn bump(&self, which: Which) -> (u64, Counted) {
        if let Some((slot, prefix)) = &self.shared
            && let Some(redis) = slot.load_full()
        {
            let window = crate::cache::now_secs() / SIGN_IN_WINDOW.as_secs();
            // Twice the window: the key names its own window, and the one
            // before must still answer for a clock a little off. Counted and
            // compared in one step, so that two instances asked at once do
            // not both find room.
            if let Some(made) = redis
                .incr_window(&which.shared_key(prefix, window), SIGN_IN_WINDOW * 2)
                .await
            {
                return (made, Counted::Shared(window));
            }
        }
        let (made, since) = self.sign_ins.bump(which, Instant::now());
        (u64::from(made), Counted::Memory(since))
    }

    /// One fewer on the count an attempt was made in.
    async fn give_back(&self, which: Which, counted: Counted) {
        match counted {
            Counted::Memory(since) => self.sign_ins.give_back(which, since),
            // Where the server has gone away since, what it counted went
            // with it.
            Counted::Shared(window) => {
                if let Some((redis, prefix)) = self.server() {
                    redis.decr(&which.shared_key(&prefix, window)).await;
                }
            }
        }
    }

    /// The count an attempt was made in, started again.
    async fn clear(&self, which: Which, counted: Counted) {
        match counted {
            Counted::Memory(_) => self.sign_ins.clear(which),
            Counted::Shared(window) => {
                if let Some((redis, prefix)) = self.server() {
                    redis.unlink(&which.shared_key(&prefix, window)).await;
                }
            }
        }
    }

    /// How long until the window a count was made in closes.
    fn closes_in(&self, counted: Counted) -> Duration {
        match counted {
            Counted::Shared(_) => until_window_closes(crate::cache::now_secs(), SIGN_IN_WINDOW),
            Counted::Memory(since) => {
                SIGN_IN_WINDOW.saturating_sub(Instant::now().duration_since(since))
            }
        }
    }

    /// The cache server and the prefix of its keys, when there is one to
    /// count on.
    fn server(&self) -> Option<(Arc<crate::cache::Redis>, String)> {
        let (slot, prefix) = self.shared.as_ref()?;
        Some((slot.load_full()?, prefix.clone()))
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
    fn bump(&self, which: Which, now: Instant) -> (u32, Instant) {
        match which {
            Which::Pair(account, peer) => self.pairs.lock().bump((account, peer), now),
            Which::Account(account) => self.accounts.lock().bump(account, now),
        }
    }

    fn give_back(&self, which: Which, since: Instant) {
        match which {
            Which::Pair(account, peer) => self.pairs.lock().give_back((account, peer), since),
            Which::Account(account) => self.accounts.lock().give_back(account, since),
        }
    }

    fn clear(&self, which: Which) {
        match which {
            Which::Pair(account, peer) => self.pairs.lock().clear((account, peer)),
            Which::Account(account) => self.accounts.lock().clear(account),
        }
    }

    fn prune(&self, now: Instant) {
        self.pairs.lock().prune(now);
        self.accounts.lock().prune(now);
    }
}

/// Counts in windows of [`SIGN_IN_WINDOW`], each opened by the count that
/// found none, kept in memory: the count itself, and when its window opened.
struct Counts<K> {
    most: usize,
    held: HashMap<K, (u32, Instant)>,
}

impl<K: Hash + Eq + Copy> Counts<K> {
    fn new(most: usize) -> Self {
        Self {
            most,
            held: HashMap::new(),
        }
    }

    /// One more in `key`'s window — opened now if none is open — as the count
    /// then stands, and when its window opened. Always counts: where the map
    /// is full it makes room, and a count lost to that is one more chance
    /// for the account's guesser at worst, where a name left uncounted would
    /// be as many as there are names.
    fn bump(&mut self, key: K, now: Instant) -> (u32, Instant) {
        if !self.held.contains_key(&key) && self.held.len() >= self.most {
            self.make_room(now);
        }
        let entry = self.held.entry(key).or_insert((0, now));
        if now.duration_since(entry.1) >= SIGN_IN_WINDOW {
            *entry = (0, now);
        }
        entry.0 = entry.0.saturating_add(1);
        *entry
    }

    /// The windows that have closed go first; when every one is open, the
    /// oldest eighth, so that the next name does not do this again.
    fn make_room(&mut self, now: Instant) {
        self.held
            .retain(|_, (_, since)| now.duration_since(*since) < SIGN_IN_WINDOW);
        if self.held.len() < self.most {
            return;
        }
        let evicted = (self.most / 8).max(1);
        let mut oldest: Vec<(Instant, K)> = self
            .held
            .iter()
            .map(|(key, (_, since))| (*since, *key))
            .collect();
        oldest.select_nth_unstable_by_key(evicted - 1, |(since, _)| *since);
        for (_, key) in &oldest[..evicted] {
            self.held.remove(key);
        }
    }

    /// One fewer in the window opened at `since`; none in another.
    fn give_back(&mut self, key: K, since: Instant) {
        if let Some((count, opened)) = self.held.get_mut(&key)
            && *opened == since
        {
            *count = count.saturating_sub(1);
        }
    }

    fn clear(&mut self, key: K) {
        self.held.remove(&key);
    }

    fn prune(&mut self, now: Instant) {
        self.held
            .retain(|_, (_, since)| now.duration_since(*since) < SIGN_IN_WINDOW);
    }
}

// ─── the addresses an account has signed in from ─────────────────────────────

/// How long an address stays known to an account after a sign-in from it.
const KNOWN_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// An address already written down is written down again after this long,
/// not at every sign-in.
const KNOWN_REFRESH: Duration = Duration::from_secs(24 * 3600);

/// The most addresses an account keeps: its usual ones, with room for the
/// places and the phones it came from lately.
const KNOWN_MOST: usize = 32;

/// The addresses of an account, by network ([`ip::bucket`]), each with the
/// time of the last sign-in from it, in seconds since the epoch.
type Known = BTreeMap<String, i64>;

/// Where an account's are kept: with the material every instance shares.
fn known_entry(user_id: &str) -> String {
    format!("signin.known:{user_id}")
}

fn read_known(text: Option<&str>) -> Known {
    text.and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default()
}

/// Whether this address has signed in to the account, successfully, in the
/// last thirty days: the account's own, so that what is tried against it
/// from everywhere else cannot shut it out. An address that cannot be told
/// is nobody's.
pub async fn address_known(db: &Db, user_id: &str, peer: IpAddr) -> bool {
    let peer = ip::bucket(peer);
    if peer == UNKNOWN_PEER {
        return false;
    }
    match keystore::get(db, &known_entry(user_id)).await {
        Ok(text) => is_known(&read_known(text.as_deref()), peer, now_epoch()),
        Err(e) => {
            // Said no: an account whose addresses cannot be read is held to
            // the bar like any other.
            tracing::warn!(error = %e, "could not read the addresses an account signed in from");
            false
        }
    }
}

/// Write down that the account has signed in from this address, unless it
/// was written down a day ago or less. Best effort: a sign-in is not undone
/// for what could not be noted.
pub async fn remember_address(db: &Db, user_id: &str, peer: IpAddr) {
    let peer = ip::bucket(peer);
    if peer == UNKNOWN_PEER {
        return;
    }
    let entry = known_entry(user_id);
    let noted = async {
        let kept = read_known(keystore::get(db, &entry).await?.as_deref());
        let Some(known) = remembered(kept, peer, now_epoch()) else {
            return anyhow::Ok(());
        };
        keystore::put(db, &entry, &serde_json::to_string(&known)?).await
    }
    .await;
    if let Err(e) = noted {
        tracing::warn!(error = %e, "could not note the address an account signed in from");
    }
}

fn now_epoch() -> i64 {
    chrono::Utc::now().timestamp()
}

fn is_known(known: &Known, peer: IpAddr, now: i64) -> bool {
    known
        .get(&peer.to_string())
        .is_some_and(|at| now - at < KNOWN_FOR.as_secs() as i64)
}

/// What an account keeps once this address has signed in to it at `now`, or
/// nothing when what it keeps already says so: the other addresses that are
/// still known, at most [`KNOWN_MOST`] of them, the latest.
fn remembered(mut known: Known, peer: IpAddr, now: i64) -> Option<Known> {
    let key = peer.to_string();
    if known
        .get(&key)
        .is_some_and(|at| now - at < KNOWN_REFRESH.as_secs() as i64)
    {
        return None;
    }
    known.insert(key, now);
    known.retain(|_, at| now - *at < KNOWN_FOR.as_secs() as i64);
    while known.len() > KNOWN_MOST {
        let Some(oldest) = known
            .iter()
            .min_by_key(|(_, at)| **at)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        known.remove(&oldest);
    }
    Some(known)
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
    let peer = peer_of(&state, &request);
    spend(&state, peer, request.uri().path()).await?;
    Ok(next.run(request).await)
}

/// What a request is counted under: its caller's network ([`ip::bucket`]).
/// A guard that ran first leaves the address it resolved behind; a route
/// behind none has it resolved here. A caller whose address is unknown is
/// counted with every other unknown one.
pub fn peer_of(state: &AppState, request: &Request) -> IpAddr {
    request
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
        .map_or(UNKNOWN_PEER, ip::bucket)
}

/// What a sign-in's address is counted under: its network ([`ip::bucket`]),
/// or the one every caller whose address is unknown shares.
pub fn network_of(address: Option<IpAddr>) -> IpAddr {
    address.map_or(UNKNOWN_PEER, ip::bucket)
}

/// Count one request of `peer`'s against its quota, `path` naming it in
/// the log: refused, saying when to come back, once the quota is spent.
/// What [`limit`] asks of every request; what the guard of TMDB's pictures
/// asks only of those it turns away (see
/// [`crate::auth::middleware::guard_tmdb_images`]).
pub async fn spend(state: &AppState, peer: IpAddr, path: &str) -> AppResult<()> {
    if let Err(wait) = state.limiter.check(peer).await {
        tracing::warn!(%peer, path = %crate::telemetry::redact_path(path), "rate limited");
        return Err(AppError::RateLimited(Throttle::spent(
            state.limiter.per_minute,
            MINUTE,
            wait,
        )));
    }
    Ok(())
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

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[tokio::test]
    async fn an_address_is_refused_after_its_attempts_on_an_account_whatever_the_case() {
        let limiter = Limiter::new(0, None, 3);
        let home = address("203.0.113.7");

        for typed in [" Admin ", "admin", "ADMIN"] {
            limiter.sign_in_begin(typed, home).await.expect("counted");
        }
        // The fourth is refused, however the name is typed.
        assert!(limiter.sign_in_begin("aDmIn", home).await.is_err());
        // Another account from this address is another count, and so is this
        // account from another address.
        assert!(limiter.sign_in_begin("alice", home).await.is_ok());
        assert!(
            limiter
                .sign_in_begin("admin", address("203.0.113.8"))
                .await
                .is_ok()
        );
    }

    /// One machine with a /64 to pick from is one address to an account.
    #[tokio::test]
    async fn an_ipv6_network_is_one_address_to_an_account() {
        let limiter = Limiter::new(0, None, 2);

        for from in ["2001:db8:1:2::1", "2001:db8:1:2::2"] {
            limiter
                .sign_in_begin("admin", address(from))
                .await
                .expect("counted");
        }
        assert!(
            limiter
                .sign_in_begin("admin", address("2001:db8:1:2:ffff::9"))
                .await
                .is_err()
        );
        assert!(
            limiter
                .sign_in_begin("admin", address("2001:db8:1:3::1"))
                .await
                .is_ok()
        );
        assert_eq!(network_of(None), UNKNOWN_PEER);
        assert_eq!(
            network_of(Some(address("2001:db8:1:2::7"))),
            address("2001:db8:1:2::")
        );
    }

    /// What counting by address is for: somebody guessing at an account from
    /// their own address shuts out that address, and not the account's owner
    /// signing in from theirs.
    #[tokio::test]
    async fn guessing_at_an_account_does_not_shut_its_owner_out() {
        let limiter = Limiter::new(0, None, 3);
        let guesser = address("198.51.100.9");
        let owner = address("203.0.113.7");

        for _ in 0..10 {
            let _ = limiter.sign_in_begin("admin", guesser).await;
        }
        assert!(limiter.sign_in_begin("admin", guesser).await.is_err());

        let theirs = limiter
            .sign_in_begin("admin", owner)
            .await
            .expect("their address has a count of its own");
        assert!(theirs.over_account_bar().is_none());
        limiter.sign_in_succeeded(theirs).await;
    }

    /// A refusal says when its window closes, from the first attempt.
    #[tokio::test]
    async fn a_refusal_says_when_the_window_closes() {
        let limiter = Limiter::new(0, None, 1);
        let home = address("203.0.113.7");

        limiter.sign_in_begin("admin", home).await.unwrap();
        let wait = limiter
            .sign_in_begin("admin", home)
            .await
            .expect_err("refused");
        assert!(
            wait > SIGN_IN_WINDOW - Duration::from_secs(5) && wait <= SIGN_IN_WINDOW,
            "{wait:?}"
        );
    }

    /// The password was right: the attempt was no failure. The address's
    /// count starts again — three more before it is refused — and the
    /// account's takes back the attempt.
    #[tokio::test]
    async fn a_right_password_starts_the_address_again() {
        let limiter = Limiter::new(0, None, 3);
        let home = address("203.0.113.7");

        limiter.sign_in_begin("admin", home).await.unwrap();
        limiter.sign_in_begin("admin", home).await.unwrap();
        let right = limiter.sign_in_begin("admin", home).await.unwrap();
        limiter.sign_in_succeeded(right).await;

        for _ in 0..3 {
            limiter
                .sign_in_begin("admin", home)
                .await
                .expect("a fresh count");
        }
        assert!(limiter.sign_in_begin("admin", home).await.is_err());

        // Two failures and the success made three on the account; the success
        // was taken back, and the three since make five.
        let account = digest("admin");
        assert_eq!(
            limiter
                .sign_ins
                .accounts
                .lock()
                .held
                .get(&account)
                .map(|(count, _)| *count),
            Some(5)
        );
    }

    /// Nothing was checked, so nothing failed.
    #[tokio::test]
    async fn an_attempt_given_back_is_not_counted() {
        let limiter = Limiter::new(0, None, 2);
        let home = address("203.0.113.7");

        for _ in 0..10 {
            let unchecked = limiter.sign_in_begin("admin", home).await.expect("room");
            limiter.sign_in_given_back(unchecked).await;
        }
        limiter.sign_in_begin("admin", home).await.unwrap();
        limiter.sign_in_begin("admin", home).await.unwrap();
        assert!(limiter.sign_in_begin("admin", home).await.is_err());
    }

    /// An attempt is counted before it is checked, so that however many
    /// arrive together, no more than the limit are let through to be checked:
    /// counted after, every one of a burst found the count still at nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn however_many_arrive_at_once_no_more_than_the_limit_are_let_through() {
        let limiter = Limiter::new(0, None, 10);
        let home = address("203.0.113.7");

        let attempts: Vec<_> = (0..200)
            .map(|_| {
                let limiter = limiter.clone();
                tokio::spawn(async move { limiter.sign_in_begin("admin", home).await.is_ok() })
            })
            .collect();
        let mut let_through = 0;
        for attempt in attempts {
            if attempt.await.unwrap() {
                let_through += 1;
            }
        }
        assert_eq!(let_through, 10);
    }

    /// Spread over as many addresses as it takes, the attempts on one account
    /// run out at a bar five times each address's: past it, whoever has not
    /// signed in to the account is told so, and another account is not.
    #[tokio::test]
    async fn the_account_as_a_whole_has_a_higher_bar_than_each_address() {
        let limiter = Limiter::new(0, None, 2);

        let mut over = Vec::new();
        for n in 0..8 {
            let from = address(&format!("198.51.100.{n}"));
            for _ in 0..2 {
                let reserved = limiter
                    .sign_in_begin("admin", from)
                    .await
                    .expect("each address within its own count");
                over.push(reserved.over_account_bar().is_some());
            }
        }
        assert_eq!(over.len(), 16);
        assert!(over[..10].iter().all(|over| !over), "{over:?}");
        assert!(over[10..].iter().all(|over| *over), "{over:?}");

        let other = limiter
            .sign_in_begin("alice", address("198.51.100.99"))
            .await
            .unwrap();
        assert!(other.over_account_bar().is_none());
    }

    #[tokio::test]
    async fn no_limit_counts_nothing() {
        let limiter = Limiter::new(0, None, 0);
        let home = address("203.0.113.7");

        for _ in 0..100 {
            let reserved = limiter.sign_in_begin("admin", home).await.expect("never");
            assert!(reserved.over_account_bar().is_none());
            limiter.sign_in_succeeded(reserved).await;
        }
    }

    /// Where the map is full, a name not counted yet is counted all the same:
    /// the windows that have closed make room, and when none has, the oldest.
    #[test]
    fn a_full_map_makes_room_rather_than_leave_a_name_uncounted() {
        let start = Instant::now();

        let mut counts = Counts::new(16);
        for key in 0u32..16 {
            counts.bump(key, start + Duration::from_secs(u64::from(key)));
        }
        let later = start + Duration::from_secs(100);
        assert_eq!(counts.bump(99, later).0, 1);
        assert_eq!(counts.bump(99, later).0, 2);
        // The oldest eighth made way; the rest keep what they counted.
        assert_eq!(counts.held.len(), 15);
        assert!(!counts.held.contains_key(&0) && !counts.held.contains_key(&1));
        assert!((2u32..16).all(|key| counts.held.contains_key(&key)));
        assert_eq!(counts.bump(5, later).0, 2);

        // Windows that have closed go before any that is open.
        let mut counts = Counts::new(4);
        for key in 0u32..3 {
            counts.bump(key, start);
        }
        counts.bump(3, start + SIGN_IN_WINDOW);
        assert_eq!(counts.bump(9, start + SIGN_IN_WINDOW).0, 1);
        assert_eq!(counts.held.len(), 2);
        assert!(counts.held.contains_key(&3));
    }

    /// Fifteen minutes on, a window has closed and counts afresh; what is
    /// given back reaches only the window it was made in.
    #[test]
    fn a_window_closes_and_a_count_given_back_reaches_only_its_own() {
        let start = Instant::now();
        let mut counts = Counts::new(8);

        counts.bump(1u32, start);
        let (made, opened) = counts.bump(1, start);
        assert_eq!(made, 2);
        counts.give_back(1, opened);
        assert_eq!(counts.bump(1, start).0, 2);

        let later = start + SIGN_IN_WINDOW;
        let (made, fresh) = counts.bump(1, later);
        assert_eq!(made, 1);
        counts.give_back(1, opened);
        assert_eq!(
            counts.bump(1, later).0,
            2,
            "the old window's is not this one's"
        );
        counts.give_back(1, fresh);
        assert_eq!(counts.bump(1, later).0, 2);

        counts.prune(later + SIGN_IN_WINDOW);
        assert!(counts.held.is_empty());
    }

    /// With the real bound, a full map still counts the account under attack:
    /// filling it with names was a way to have none of them counted.
    #[tokio::test]
    async fn filling_the_map_with_names_does_not_stop_the_count() {
        let limiter = Limiter::new(0, None, 2);
        let guesser = address("198.51.100.9");

        let now = Instant::now();
        for name in 0..SIGN_IN_COUNTS as u128 {
            limiter.sign_ins.bump(Which::Pair(name, guesser), now);
            limiter.sign_ins.bump(Which::Account(name), now);
        }
        assert_eq!(limiter.sign_ins.pairs.lock().held.len(), SIGN_IN_COUNTS);

        limiter.sign_in_begin("target", guesser).await.unwrap();
        limiter.sign_in_begin("target", guesser).await.unwrap();
        assert!(limiter.sign_in_begin("target", guesser).await.is_err());
        assert!(limiter.sign_ins.pairs.lock().held.len() <= SIGN_IN_COUNTS);
    }

    #[test]
    fn an_address_is_known_for_thirty_days_and_written_down_once_a_day() {
        let home = address("203.0.113.7");
        let now = 1_800_000_000;
        let month = KNOWN_FOR.as_secs() as i64;

        let first = remembered(Known::new(), home, now).expect("written down");
        assert!(is_known(&first, home, now));
        assert!(!is_known(&first, address("203.0.113.8"), now));
        // Written down already today: nothing to write.
        assert!(remembered(first.clone(), home, now + 3600).is_none());
        // A day on, it is written again.
        assert!(remembered(first.clone(), home, now + 86_400).is_some());

        // Known for thirty days from the last sign-in, and not after.
        assert!(is_known(&first, home, now + month - 1));
        assert!(!is_known(&first, home, now + month));
        // What went stale is dropped the next time something is written.
        let later = remembered(first, address("198.51.100.1"), now + month + 10).unwrap();
        assert!(!later.contains_key(&home.to_string()));
        assert_eq!(later.len(), 1);
    }

    #[test]
    fn an_account_keeps_the_latest_addresses_and_no_more() {
        let now = 1_800_000_000;
        let mut kept = Known::new();
        for n in 0..(KNOWN_MOST as i64 + 10) {
            let from = address(&format!("198.51.{}.{}", n / 200, n % 200));
            kept = remembered(kept, from, now + n * 60).expect("a new address is written");
        }
        assert_eq!(kept.len(), KNOWN_MOST);
        assert!(!kept.contains_key("198.51.0.0"));
        assert!(kept.contains_key("198.51.0.41"));
    }

    #[tokio::test]
    async fn the_addresses_an_account_signed_in_from_are_kept_with_it() {
        let db = crate::db::testing::db().await;
        let home = address("203.0.113.7");

        assert!(!address_known(&db, "u1", home).await);
        remember_address(&db, "u1", home).await;
        assert!(address_known(&db, "u1", home).await);
        // Its own, and nobody else's.
        assert!(!address_known(&db, "u2", home).await);
        assert!(!address_known(&db, "u1", address("203.0.113.8")).await);

        // A /64 is one address.
        remember_address(&db, "u1", address("2001:db8:1:2::1")).await;
        assert!(address_known(&db, "u1", address("2001:db8:1:2:ffff::9")).await);

        // A caller nobody could tell is nobody's.
        remember_address(&db, "u1", UNKNOWN_PEER).await;
        assert!(!address_known(&db, "u1", UNKNOWN_PEER).await);
    }
}
