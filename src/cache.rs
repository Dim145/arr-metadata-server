//! In-process caches.
//!
//! The database is the durable cache; these are the hot layer in front of it,
//! sized and expired independently per kind of value.

use std::time::Duration;

use moka::future::Cache as Moka;

use crate::config;

#[derive(Clone)]
pub struct Caches {
    /// Fully merged entities, keyed by `{surface}:{language}:{id}`.
    pub items: Moka<String, String>,
    /// Search result sets, keyed by `{surface}:{language}:{term}`.
    pub searches: Moka<String, String>,
    /// Which settings a cached search was computed under.
    ///
    /// Bumped once a settings change has reached every provider, and part of
    /// every search key. A search can straddle the change — key computed with
    /// the new adult policy while TMDB is still being asked with the old one —
    /// and clearing the cache cannot catch that: the entry is written *after*
    /// the clear. A generation can, because the stale entry is filed under a
    /// number nobody asks for again.
    /// Shared, like the caches beside it: a clone of `Caches` is the same cache.
    pub generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Caches {
    pub fn new(cfg: &config::Cache) -> Self {
        Self {
            items: build(cfg.max_entries, cfg.item_ttl),
            searches: build(cfg.max_entries / 2, cfg.search_ttl),
            generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub async fn invalidate_all(&self) {
        self.items.invalidate_all();
        self.searches.invalidate_all();
    }
}

fn build(capacity: u64, ttl: Duration) -> Moka<String, String> {
    Moka::builder()
        .max_capacity(capacity.max(1))
        .time_to_live(ttl)
        // Values are serialized documents; weigh by byte length so a few huge
        // series with full episode lists cannot evict everything else.
        .weigher(|k: &String, v: &String| (k.len() + v.len()).try_into().unwrap_or(u32::MAX))
        .build()
}
