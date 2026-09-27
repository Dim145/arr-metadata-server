//! One space of the cache, in two tiers: the process's own memory first,
//! the shared server behind it when there is one.
//!
//! A read looks in memory, then asks the server and keeps what it answers
//! in memory for the next read. A write goes to both, the server's part
//! without waiting for it. Forgetting goes to both too, and is told to the
//! other instances, whose memory is then behind. Every space has a switch,
//! and counts what it answered from each tier, so the page can say what the
//! cache is worth.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};

use bytes::Bytes;
use moka::future::Cache as Moka;

use super::{RedisSlot, redis::Redis};

/// The largest value sent to the server: what a round trip under the
/// deadline can carry. Anything bigger stays in memory alone.
const L2_MAX_BYTES: usize = 1024 * 1024;

/// A value a space keeps: bytes both ways, and a weight for the memory.
pub trait Cached: Clone + Send + Sync + 'static {
    fn to_bytes(&self) -> Bytes;
    fn from_bytes(bytes: Bytes) -> Option<Self>;
    fn weight(&self) -> usize;
}

impl Cached for String {
    fn to_bytes(&self) -> Bytes {
        Bytes::copy_from_slice(self.as_bytes())
    }
    fn from_bytes(bytes: Bytes) -> Option<Self> {
        String::from_utf8(bytes.to_vec()).ok()
    }
    fn weight(&self) -> usize {
        self.len()
    }
}

impl Cached for Bytes {
    fn to_bytes(&self) -> Bytes {
        self.clone()
    }
    fn from_bytes(bytes: Bytes) -> Option<Self> {
        Some(bytes)
    }
    fn weight(&self) -> usize {
        self.len()
    }
}

/// What a tier answered, since the start.
#[derive(Clone, Copy, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub hits: u64,
    pub misses: u64,
}

pub struct Space<V> {
    pub id: &'static str,
    l1: Moka<String, V>,
    slot: RedisSlot,
    ttl: Duration,
    enabled: AtomicBool,
    l1_hits: AtomicU64,
    l1_misses: AtomicU64,
    l2_hits: AtomicU64,
    l2_misses: AtomicU64,
}

impl<V: Cached> Space<V> {
    /// A space of `capacity` bytes in memory, kept `ttl`, behind which the
    /// server keeps the same for the same time.
    pub fn new(id: &'static str, capacity: u64, ttl: Duration, slot: RedisSlot) -> Self {
        let l1 = Moka::builder()
            .max_capacity(capacity.max(1))
            .time_to_live(ttl)
            // Values are documents; weigh by byte length so a few huge series
            // with full episode lists cannot evict everything else.
            .weigher(|k: &String, v: &V| (k.len() + v.weight()).try_into().unwrap_or(u32::MAX))
            .build();
        Self {
            id,
            l1,
            slot,
            ttl,
            enabled: AtomicBool::new(true),
            l1_hits: AtomicU64::new(0),
            l1_misses: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            l2_misses: AtomicU64::new(0),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Relaxed)
    }

    /// Switch the space off or on. Off, it answers nothing and keeps nothing;
    /// what it held stays until forgotten or expired.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Relaxed);
    }

    /// The server behind the space, when one is attached.
    fn l2(&self) -> Option<Arc<Redis>> {
        self.slot.load_full()
    }

    pub fn has_l2(&self) -> bool {
        self.l2().is_some()
    }

    pub async fn get(&self, key: &str) -> Option<V> {
        if !self.enabled() {
            return None;
        }
        if let Some(value) = self.l1.get(key).await {
            self.l1_hits.fetch_add(1, Relaxed);
            return Some(value);
        }
        self.l1_misses.fetch_add(1, Relaxed);
        let redis = self.l2()?;
        match redis.get(&redis.key(self.id, key)).await {
            Some(bytes) => match V::from_bytes(bytes) {
                Some(value) => {
                    self.l2_hits.fetch_add(1, Relaxed);
                    self.l1.insert(key.to_string(), value.clone()).await;
                    Some(value)
                }
                None => {
                    // Not what this server writes: somebody else's, or an
                    // older shape. Forgotten rather than served.
                    self.l2_misses.fetch_add(1, Relaxed);
                    redis.unlink(&redis.key(self.id, key)).await;
                    None
                }
            },
            None => {
                self.l2_misses.fetch_add(1, Relaxed);
                None
            }
        }
    }

    /// Keep a value: in memory now, on the server in the background.
    pub async fn insert(&self, key: String, value: V) {
        self.insert_for(key, value, self.ttl).await;
    }

    /// Keep a value the server holds for `ttl` rather than the space's
    /// time — a document whose kind says how long it stays good. The memory
    /// keeps it the space's time at most; the caller judges the rest.
    pub async fn insert_for(&self, key: String, value: V, ttl: Duration) {
        if !self.enabled() {
            return;
        }
        if let Some(redis) = self.l2()
            && value.weight() <= L2_MAX_BYTES
        {
            let full = redis.key(self.id, &key);
            let bytes = value.to_bytes();
            let ttl = ttl.min(self.ttl);
            tokio::spawn(async move {
                redis.set_ex(&full, bytes, ttl).await;
            });
        }
        self.l1.insert(key, value).await;
    }

    /// One computation for every caller asking at once — moka's own
    /// single flight — with the server looked in before it runs, and told
    /// what it found afterwards. The value, or the computation's error
    /// shared between the callers that waited for it.
    pub async fn try_get_with<F, E>(&self, key: String, init: F) -> Result<V, Arc<E>>
    where
        F: std::future::Future<Output = Result<V, E>> + Send + 'static,
        E: Send + Sync + 'static,
    {
        if !self.enabled() {
            return init.await.map_err(Arc::new);
        }
        let l2 = self.l2();
        let id = self.id;
        let looked = Arc::new(AtomicBool::new(false));
        let computed = Arc::new(AtomicBool::new(false));
        let result = self
            .l1
            .try_get_with(key.clone(), {
                let (l2, key, looked, computed) =
                    (l2.clone(), key.clone(), looked.clone(), computed.clone());
                async move {
                    if let Some(redis) = &l2 {
                        looked.store(true, Relaxed);
                        if let Some(bytes) = redis.get(&redis.key(id, &key)).await
                            && let Some(value) = V::from_bytes(bytes)
                        {
                            return Ok(value);
                        }
                    }
                    computed.store(true, Relaxed);
                    init.await
                }
            })
            .await;
        let (looked, computed) = (looked.load(Relaxed), computed.load(Relaxed));
        match (looked, computed) {
            (false, false) => {
                self.l1_hits.fetch_add(1, Relaxed);
            }
            (true, false) => {
                self.l1_misses.fetch_add(1, Relaxed);
                self.l2_hits.fetch_add(1, Relaxed);
            }
            (looked, true) => {
                self.l1_misses.fetch_add(1, Relaxed);
                if looked {
                    self.l2_misses.fetch_add(1, Relaxed);
                }
                if let (Ok(value), Some(redis)) = (&result, l2)
                    && value.weight() <= L2_MAX_BYTES
                {
                    let full = redis.key(id, &key);
                    let bytes = value.to_bytes();
                    let ttl = self.ttl;
                    tokio::spawn(async move {
                        redis.set_ex(&full, bytes, ttl).await;
                    });
                }
            }
        }
        result
    }

    /// Forget one key, everywhere: this memory, the server, and — told over
    /// the channel — the other instances' memory.
    pub async fn invalidate(&self, key: &str) {
        self.l1.invalidate(key).await;
        if let Some(redis) = self.l2() {
            redis.unlink(&redis.key(self.id, key)).await;
            redis.publish(&format!("drop:{}:{key}", self.id)).await;
        }
    }

    /// Forget everything the space holds, everywhere. How many keys the
    /// server let go of.
    pub async fn invalidate_all(&self) -> u64 {
        self.l1.invalidate_all();
        let Some(redis) = self.l2() else {
            return 0;
        };
        let removed = redis.unlink_prefix(&redis.key(self.id, "")).await;
        redis.publish(&format!("flush:{}", self.id)).await;
        removed
    }

    /// Forget what this memory holds — on another instance's word, so
    /// without telling anyone again.
    pub fn forget_local(&self) {
        self.l1.invalidate_all();
    }

    pub async fn forget_local_key(&self, key: &str) {
        self.l1.invalidate(key).await;
    }

    pub fn l1_entries(&self) -> u64 {
        self.l1.entry_count()
    }

    pub fn l1_bytes(&self) -> u64 {
        self.l1.weighted_size()
    }

    pub fn l1_tally(&self) -> Tally {
        Tally {
            hits: self.l1_hits.load(Relaxed),
            misses: self.l1_misses.load(Relaxed),
        }
    }

    pub fn l2_tally(&self) -> Tally {
        Tally {
            hits: self.l2_hits.load(Relaxed),
            misses: self.l2_misses.load(Relaxed),
        }
    }

    /// Bring the memory's housekeeping up to date, for figures read right
    /// after a change.
    pub async fn settle(&self) {
        self.l1.run_pending_tasks().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_space_answers_from_memory_and_counts_what_it_did() {
        let space: Space<String> = Space::new(
            "test",
            1024 * 1024,
            Duration::from_secs(60),
            RedisSlot::default(),
        );
        assert!(space.get("a").await.is_none());
        space.insert("a".into(), "one".into()).await;
        assert_eq!(space.get("a").await.as_deref(), Some("one"));
        let tally = space.l1_tally();
        assert_eq!((tally.hits, tally.misses), (1, 1));
        assert!(!space.has_l2());

        space.set_enabled(false);
        assert!(space.get("a").await.is_none(), "off, it answers nothing");
        space.insert("b".into(), "two".into()).await;
        space.set_enabled(true);
        assert!(space.get("b").await.is_none(), "off, it kept nothing");
        assert_eq!(space.get("a").await.as_deref(), Some("one"));

        space.invalidate("a").await;
        assert!(space.get("a").await.is_none());
        space.insert("c".into(), "three".into()).await;
        assert_eq!(space.invalidate_all().await, 0);
        assert!(space.get("c").await.is_none());
    }

    #[test]
    fn bytes_round_trip() {
        let text = String::from("héllo");
        assert_eq!(
            String::from_bytes(text.to_bytes()).as_deref(),
            Some("héllo")
        );
        assert_eq!(String::from_bytes(Bytes::from_static(&[0xff, 0xfe])), None);
        assert_eq!(text.weight(), 6);
        let raw = Bytes::from_static(b"abc");
        assert_eq!(Bytes::from_bytes(raw.to_bytes()), Some(raw));
    }
}
