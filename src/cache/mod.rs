//! The caches: the hot layer in front of the database and the providers.
//!
//! The database is the durable cache; these are what is kept under the hand
//! to answer without it, sized and expired independently per kind of value.
//! Each space has two tiers: the process's own memory, and — when
//! `AMS_REDIS_URL` names one — a Valkey or Redis server behind it, shared
//! between instances and kept across restarts. Without a server the spaces
//! work the same, one tier deep.
//!
//! One space is memory alone by design: a session is a credential, and
//! belongs nowhere but here and in the database.

pub mod redis;
pub mod tier;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use moka::future::Cache as Moka;

use crate::{config, db::repo::user::User};

pub use redis::Redis;
pub use tier::{Cached, Space, Tally};

/// Where the server behind the second tier sits once it answers: empty
/// until it does, so a server that is down at start is attached later,
/// and a space asks it only while it is there.
pub type RedisSlot = Arc<arc_swap::ArcSwapOption<Redis>>;

/// What an entry weighs, taken as typical, to turn the number of entries the
/// configuration asks for into the bytes a cache is measured in: a film is a
/// few kilobytes, a series with all its episodes some hundreds.
///
/// A cache weighs its entries by their length, so its capacity is in bytes.
/// Given the number of entries as it stood, every cache held ten kilobytes —
/// less than one series with its episodes, which was refused outright, so
/// nothing but the smallest documents was ever served from memory.
const TYPICAL_ENTRY: u64 = 16 * 1024;

/// Room for a few relayed lists at once: the Top 250, the popular hundred,
/// and a user's ratings or two.
const LISTS_CAPACITY: u64 = 64 * 1024 * 1024;

/// Room for what the relay answers Jellyseerr and its like: a few thousand
/// TMDB documents of a few kilobytes.
const RELAY_CAPACITY: u64 = 32 * 1024 * 1024;

/// How long the relay keeps a TMDB document, by what it is. Far under the
/// six months TMDB's terms allow, and short where the answer moves.
pub fn relay_ttl(path: &str) -> Duration {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    let secs = if path.starts_with("/3/configuration") {
        24 * HOUR
    } else if path.starts_with("/3/search/")
        || path.starts_with("/3/trending/")
        || path.starts_with("/3/discover/")
        || path.contains("/popular")
        || path.contains("/now_playing")
        || path.contains("/upcoming")
        || path.contains("/on_the_air")
        || path.contains("/airing_today")
        || path.contains("/top_rated")
    {
        15 * MINUTE
    } else if path.starts_with("/3/person/") || path.starts_with("/3/genre/") {
        24 * HOUR
    } else if path.starts_with("/4/list/") {
        HOUR
    } else {
        6 * HOUR
    };
    Duration::from_secs(secs)
}

/// Every space, by the name the settings and the page use.
pub const SPACES: [&str; 5] = ["items", "searches", "lists", "relay", "sessions"];

/// A document the relay answered with, as it came from TMDB: its status,
/// its type and its bytes, patched with the local overrides on each serve.
#[derive(Clone, Debug)]
pub struct Relayed {
    pub status: u16,
    pub content_type: String,
    /// When it is no longer good, as seconds since the epoch: the memory
    /// keeps it the space's time, the document's own kind says less.
    pub expires_at: u64,
    pub body: Bytes,
}

impl Relayed {
    pub fn is_fresh(&self) -> bool {
        self.expires_at > now_secs()
    }
}

/// Seconds since the epoch, for what a document says of its own end.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Cached for Relayed {
    fn to_bytes(&self) -> Bytes {
        let head = format!(
            "{}\n{}\n{}\n",
            self.status, self.expires_at, self.content_type
        );
        let mut out = Vec::with_capacity(self.body.len() + head.len());
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&self.body);
        Bytes::from(out)
    }

    fn from_bytes(bytes: Bytes) -> Option<Self> {
        let mut cuts = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i);
        let (first, second, third) = (cuts.next()?, cuts.next()?, cuts.next()?);
        let text = |from: usize, to: usize| std::str::from_utf8(&bytes[from..to]).ok();
        Some(Self {
            status: text(0, first)?.parse().ok()?,
            expires_at: text(first + 1, second)?.parse().ok()?,
            content_type: text(second + 1, third)?.to_string(),
            body: bytes.slice(third + 1..),
        })
    }

    fn weight(&self) -> usize {
        self.body.len() + self.content_type.len() + 24
    }
}

pub struct Caches {
    /// Fully merged entities, keyed by `item:{id}`.
    pub items: Space<String>,
    /// Search result sets and catalogue pages, keyed under the stamp — the
    /// generation and the epoch — then by what was asked.
    pub searches: Space<String>,
    /// Whole lists relayed from a metadata service — IMDb's Top 250 is
    /// thirteen megabytes — kept a day, apart from the searches they would
    /// otherwise push out of memory. Bytes, handed out shared: a hit is a
    /// reference count, not a thirteen-megabyte copy.
    pub lists: Space<Bytes>,
    /// What the TMDB relay answered, keyed by the path and query asked,
    /// without the credential.
    pub relay: Space<Relayed>,
    /// A session's token, hashed, to the person it belongs to: read on every
    /// authenticated request, so kept a few seconds rather than asked of the
    /// database each time. Memory alone.
    sessions: Moka<String, Arc<User>>,
    sessions_on: AtomicBool,
    /// Moved on by every forgetting of the sessions: a session read from the
    /// database under an older number is not remembered, so a change that
    /// lands while a request is in flight is not undone by it.
    session_epoch: AtomicU64,
    session_hits: AtomicU64,
    session_misses: AtomicU64,
    /// Which settings a cached search was computed under.
    ///
    /// Bumped once a settings change has reached every provider, and part of
    /// every search key. A search can straddle the change — key computed with
    /// the new adult policy while TMDB is still being asked with the old one —
    /// and clearing the cache cannot catch that: the entry is written *after*
    /// the clear. A generation can, because the stale entry is filed under a
    /// number nobody asks for again. With a server it is the server's number,
    /// so every instance moves on together.
    generation: AtomicU64,
    /// Which state of the catalogue a cached list was drawn from: moved on
    /// by every write, so a list never outlives the works it shows. Part of
    /// every search and list key, beside the generation.
    epoch: Arc<AtomicU64>,
    /// Whether a sync of the epoch with the server is already on its way:
    /// bumps come in bursts — a refresh sweep, a media backfill — and the
    /// server is told once per burst.
    epoch_sync: Arc<AtomicBool>,
    wants_redis: bool,
    /// The server behind the second tier, when there is one.
    slot: RedisSlot,
    pub session_ttl: Duration,
}

impl Caches {
    pub fn new(cfg: &config::Cache, slot: RedisSlot) -> Self {
        let l2 = slot.clone();
        Self {
            items: Space::new(
                "items",
                cfg.max_entries.saturating_mul(TYPICAL_ENTRY),
                cfg.item_ttl,
                l2.clone(),
            ),
            searches: Space::new(
                "searches",
                (cfg.max_entries / 2).saturating_mul(TYPICAL_ENTRY),
                cfg.search_ttl,
                l2.clone(),
            ),
            // Memory alone: a relayed list runs to thirteen megabytes, more
            // than a round trip to the server is given.
            lists: Space::new(
                "lists",
                LISTS_CAPACITY,
                Duration::from_secs(24 * 60 * 60),
                RedisSlot::default(),
            ),
            // The relay's own TTL is by document; the space's is the longest.
            relay: Space::new(
                "relay",
                RELAY_CAPACITY,
                Duration::from_secs(24 * 60 * 60),
                l2,
            ),
            sessions: Moka::builder()
                .max_capacity(10_000)
                .time_to_live(cfg.session_ttl)
                .build(),
            sessions_on: AtomicBool::new(true),
            session_epoch: AtomicU64::new(0),
            session_hits: AtomicU64::new(0),
            session_misses: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            epoch: Arc::new(AtomicU64::new(0)),
            epoch_sync: Arc::new(AtomicBool::new(false)),
            wants_redis: cfg.redis_url.is_some(),
            slot,
            session_ttl: cfg.session_ttl,
        }
    }

    /// The server behind the second tier, while it is attached.
    pub fn redis(&self) -> Option<Arc<Redis>> {
        self.slot.load_full()
    }

    /// Whether a server was asked for at all, attached or not.
    pub fn wants_redis(&self) -> bool {
        self.wants_redis
    }

    /// Forget everything, every space.
    pub async fn invalidate_all(&self) {
        for space in SPACES {
            self.flush(space).await;
        }
    }

    /// Forget one space by name. How many keys the server let go of.
    pub async fn flush(&self, space: &str) -> Option<u64> {
        Some(match space {
            "items" => self.items.invalidate_all().await,
            "searches" => self.searches.invalidate_all().await,
            "lists" => {
                // Memory alone now; what an earlier version filed on the
                // server under the lists' prefix goes with the flush too.
                self.lists.invalidate_all().await;
                match self.redis() {
                    Some(redis) => redis.unlink_prefix(&redis.key("lists", "")).await,
                    None => 0,
                }
            }
            "relay" => self.relay.invalidate_all().await,
            "sessions" => {
                self.forget_sessions();
                0
            }
            _ => return None,
        })
    }

    // ── Sessions ────────────────────────────────────────────────────────

    pub async fn session(&self, token_hash: &str) -> Option<Arc<User>> {
        if !self.sessions_on.load(Ordering::Relaxed) {
            return None;
        }
        let found = self.sessions.get(token_hash).await;
        if found.is_some() {
            self.session_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.session_misses.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    /// The number to read before asking the database, and to hand back with
    /// what it answered.
    pub fn session_epoch(&self) -> u64 {
        self.session_epoch.load(Ordering::SeqCst)
    }

    /// Remember what the database answered — unless the sessions were
    /// forgotten meanwhile, in which case the answer may already be old.
    pub async fn remember_session(&self, token_hash: String, user: Arc<User>, seen: u64) {
        if self.sessions_on.load(Ordering::Relaxed) && self.session_epoch() == seen {
            self.sessions.insert(token_hash, user).await;
        }
    }

    /// Forget every session known: a sign-out, a role or a password changed,
    /// an account closed — anything that must take effect on the next
    /// request rather than within the TTL. Here, and on every instance.
    pub fn forget_sessions(&self) {
        self.sessions.invalidate_all();
        self.session_epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(redis) = self.redis()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move { redis.publish("flush:sessions").await });
        }
    }

    pub fn sessions_enabled(&self) -> bool {
        self.sessions_on.load(Ordering::Relaxed)
    }

    pub fn session_entries(&self) -> u64 {
        self.sessions.entry_count()
    }

    pub fn session_tally(&self) -> Tally {
        Tally {
            hits: self.session_hits.load(Ordering::Relaxed),
            misses: self.session_misses.load(Ordering::Relaxed),
        }
    }

    // ── Switches ────────────────────────────────────────────────────────

    /// Set every space's switch from the settings: `cache.<space>`, on
    /// unless said otherwise.
    pub fn sync_switches(&self, flag: impl Fn(&str) -> bool) {
        self.items.set_enabled(flag("cache.items"));
        self.searches.set_enabled(flag("cache.searches"));
        self.lists.set_enabled(flag("cache.lists"));
        self.relay.set_enabled(flag("cache.relay"));
        let sessions = flag("cache.sessions");
        self.sessions_on.store(sessions, Ordering::Relaxed);
        if !sessions {
            self.sessions.invalidate_all();
        }
    }

    // ── Generation ──────────────────────────────────────────────────────

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Move every search on to a new generation, this instance's and — with
    /// a server — every other's.
    pub async fn bump_generation(&self) -> u64 {
        let local = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(redis) = self.redis() else {
            return local;
        };
        let key = format!("{}gen", redis.prefix);
        match redis.incr(&key).await {
            Some(shared) => {
                let settled = self
                    .generation
                    .fetch_max(shared, Ordering::SeqCst)
                    .max(shared);
                // Bumps made while the server was away: the server catches up.
                if settled > shared {
                    redis.set_u64(&key, settled).await;
                }
                redis.publish(&format!("gen:{settled}")).await;
                settled
            }
            None => local,
        }
    }

    /// Take the generation and the epoch from the server, at start: what
    /// the other instances are on.
    pub async fn load_generation(&self) {
        let Some(redis) = self.redis() else {
            return;
        };
        if let Some(shared) = redis.get_u64(&format!("{}gen", redis.prefix)).await {
            self.generation.fetch_max(shared, Ordering::SeqCst);
        }
        if let Some(shared) = redis.get_u64(&format!("{}epoch", redis.prefix)).await {
            self.epoch.fetch_max(shared, Ordering::SeqCst);
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// What every search and list key carries: the generation and the
    /// epoch, so a settings change or a write files what follows elsewhere.
    pub fn stamp(&self) -> String {
        format!("{}.{}", self.generation(), self.epoch())
    }

    /// The catalogue changed: every list drawn so far is of the past.
    pub async fn bump_epoch(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let Some(redis) = self.redis() else {
            return;
        };
        // Told to the server in the background, once per burst: a write need
        // not wait for it, and a sweep writing a work a second need not ask
        // it a hundred times. The number that settles is the higher of the
        // two, written back where the server was behind, and published.
        if self.epoch_sync.swap(true, Ordering::SeqCst) {
            return;
        }
        let (epoch, pending) = (self.epoch.clone(), self.epoch_sync.clone());
        let key = format!("{}epoch", redis.prefix);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            pending.store(false, Ordering::SeqCst);
            let local = epoch.load(Ordering::SeqCst);
            let Some(shared) = redis.incr(&key).await else {
                return;
            };
            let settled = local.max(shared);
            epoch.fetch_max(settled, Ordering::SeqCst);
            if settled > shared {
                redis.set_u64(&key, settled).await;
            }
            redis.publish(&format!("epoch:{settled}")).await;
        });
    }

    /// A work was written: its own entry is forgotten and every list moves
    /// on, here and on every instance.
    pub async fn touched(&self, id: &str) {
        self.items.invalidate(&format!("item:{id}")).await;
        self.bump_epoch().await;
    }

    // ── Told by another instance ────────────────────────────────────────

    /// Act on a message from the channel: what another instance forgot, or
    /// the generation it moved to.
    pub async fn apply_message(&self, message: &str) {
        if let Some(n) = message.strip_prefix("gen:") {
            if let Ok(n) = n.parse::<u64>() {
                self.generation.fetch_max(n, Ordering::SeqCst);
            }
        } else if let Some(n) = message.strip_prefix("epoch:") {
            if let Ok(n) = n.parse::<u64>() {
                self.epoch.fetch_max(n, Ordering::SeqCst);
            }
        } else if let Some(space) = message.strip_prefix("flush:") {
            match space {
                "items" => self.items.forget_local(),
                "searches" => self.searches.forget_local(),
                "lists" => self.lists.forget_local(),
                "relay" => self.relay.forget_local(),
                "sessions" => {
                    self.sessions.invalidate_all();
                    self.session_epoch.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        } else if let Some(rest) = message.strip_prefix("drop:")
            && let Some((space, key)) = rest.split_once(':')
        {
            match space {
                "items" => self.items.forget_local_key(key).await,
                "searches" => self.searches.forget_local_key(key).await,
                "lists" => self.lists.forget_local_key(key).await,
                "relay" => self.relay.forget_local_key(key).await,
                _ => {}
            }
        }
    }
}

/// Attach the server named by the configuration, and keep at it until it
/// answers: a cache server that starts after this one, or restarts, is
/// found rather than given up on. Once attached it is pinged, and listened
/// to for what the other instances forget.
pub async fn attach(state: crate::state::AppState) {
    let Some(url) = state.config.cache.redis_url.clone() else {
        return;
    };
    let mut said = false;
    loop {
        match Redis::connect(
            &url.0,
            state.config.cache.redis_prefix.clone(),
            state.config.cache.redis_timeout,
        )
        .await
        {
            Ok(redis) => {
                let redis = Arc::new(redis);
                redis.ping().await;
                state.caches.slot.store(Some(redis.clone()));
                state.caches.load_generation().await;
                tracing::info!(
                    prefix = %redis.prefix,
                    "the cache server is attached: a second tier, shared and kept"
                );
                tokio::spawn(redis.clone().heartbeat());
                let listener = state.clone();
                tokio::spawn(redis.subscribe(move |message| {
                    let state = listener.clone();
                    tokio::spawn(
                        async move { crate::coord::apply_message(&state, &message).await },
                    );
                }));
                return;
            }
            Err(e) => {
                if !said {
                    tracing::warn!(
                        error = format_args!("{e:#}"),
                        "the cache server is not reachable; the caches keep to memory until it is"
                    );
                    said = true;
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caches() -> Caches {
        Caches::new(
            &config::Cache {
                max_entries: 100,
                item_ttl: Duration::from_secs(60),
                search_ttl: Duration::from_secs(60),
                session_ttl: Duration::from_secs(30),
                redis_url: None,
                redis_prefix: "ams:".into(),
                redis_timeout: Duration::from_millis(150),
                public_seconds: 60,
            },
            RedisSlot::default(),
        )
    }

    #[tokio::test]
    async fn the_spaces_are_switched_by_the_settings_and_forgotten_by_name() {
        let caches = caches();
        caches.items.insert("a".into(), "1".into()).await;
        caches
            .relay
            .insert(
                "b".into(),
                Relayed {
                    status: 200,
                    content_type: "application/json".into(),
                    expires_at: now_secs() + 60,
                    body: Bytes::from_static(b"{}"),
                },
            )
            .await;
        assert!(caches.items.get("a").await.is_some());
        assert!(caches.relay.get("b").await.is_some());

        caches.sync_switches(|key| key != "cache.items");
        assert!(!caches.items.enabled());
        assert!(caches.relay.enabled());
        assert!(caches.items.get("a").await.is_none());
        caches.sync_switches(|_| true);

        assert_eq!(caches.flush("relay").await, Some(0));
        assert!(caches.relay.get("b").await.is_none());
        assert_eq!(caches.flush("nothing").await, None);

        let before = caches.generation();
        assert_eq!(caches.bump_generation().await, before + 1);
        caches.apply_message("gen:40").await;
        assert_eq!(caches.generation(), 40);
        caches.apply_message("gen:3").await;
        assert_eq!(caches.generation(), 40, "never backwards");

        let epoch = caches.epoch();
        caches.touched("x").await;
        assert_eq!(caches.epoch(), epoch + 1);
        let seen = caches.session_epoch();
        caches.forget_sessions();
        assert_eq!(caches.session_epoch(), seen + 1);
        caches.apply_message("flush:sessions").await;
        assert_eq!(caches.session_epoch(), seen + 2);
        assert_eq!(caches.stamp(), format!("40.{}", epoch + 1));
        caches.apply_message("epoch:99").await;
        assert_eq!(caches.epoch(), 99);

        caches.items.insert("c".into(), "3".into()).await;
        caches.apply_message("drop:items:c").await;
        assert!(caches.items.get("c").await.is_none());
        caches.items.insert("d".into(), "4".into()).await;
        caches.apply_message("flush:items").await;
        assert!(caches.items.get("d").await.is_none());
    }

    #[test]
    fn a_relayed_document_survives_the_bytes() {
        let doc = Relayed {
            status: 404,
            content_type: "application/json;charset=utf-8".into(),
            expires_at: 1,
            body: Bytes::from_static(b"{\"success\":false}\n"),
        };
        let back = Relayed::from_bytes(doc.to_bytes()).unwrap();
        assert_eq!(back.status, 404);
        assert_eq!(back.content_type, doc.content_type);
        assert_eq!(back.expires_at, 1);
        assert_eq!(back.body, doc.body);
        assert!(!back.is_fresh(), "long past its end");
        assert!(
            Relayed {
                expires_at: now_secs() + 60,
                ..doc.clone()
            }
            .is_fresh()
        );
        assert!(Relayed::from_bytes(Bytes::from_static(b"nonsense")).is_none());
    }

    #[test]
    fn the_relay_keeps_a_document_as_long_as_its_kind_allows() {
        assert_eq!(relay_ttl("/3/configuration"), Duration::from_secs(86_400));
        assert_eq!(relay_ttl("/3/search/movie"), Duration::from_secs(900));
        assert_eq!(relay_ttl("/3/movie/popular"), Duration::from_secs(900));
        assert_eq!(relay_ttl("/3/movie/238"), Duration::from_secs(6 * 3600));
        assert_eq!(relay_ttl("/3/person/287"), Duration::from_secs(86_400));
        assert_eq!(relay_ttl("/4/list/8136"), Duration::from_secs(3600));
    }
}
