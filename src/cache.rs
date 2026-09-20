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
}

impl Caches {
    pub fn new(cfg: &config::Cache) -> Self {
        Self {
            items: build(cfg.max_entries, cfg.item_ttl),
            searches: build(cfg.max_entries / 2, cfg.search_ttl),
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
