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

/// What an entry weighs, taken as typical, to turn the number of entries the
/// configuration asks for into the bytes a cache is measured in: a film is a
/// few kilobytes, a series with all its episodes some hundreds.
///
/// A cache weighs its entries by their length, so its capacity is in bytes.
/// Given the number of entries as it stood, every cache held ten kilobytes —
/// less than one series with its episodes, which was refused outright, so
/// nothing but the smallest documents was ever served from memory.
const TYPICAL_ENTRY: u64 = 16 * 1024;

impl Caches {
    pub fn new(cfg: &config::Cache) -> Self {
        Self {
            items: build(cfg.max_entries.saturating_mul(TYPICAL_ENTRY), cfg.item_ttl),
            searches: build(
                (cfg.max_entries / 2).saturating_mul(TYPICAL_ENTRY),
                cfg.search_ttl,
            ),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_document_the_size_of_a_long_series_is_kept() {
        let caches = Caches::new(&config::Cache {
            max_entries: 100,
            item_ttl: Duration::from_secs(60),
            search_ttl: Duration::from_secs(60),
        });

        // A series with five hundred episodes, give or take.
        let document = "x".repeat(400 * 1024);
        caches.items.insert("series".into(), document.clone()).await;
        caches.searches.insert("season".into(), document).await;
        caches.items.run_pending_tasks().await;
        caches.searches.run_pending_tasks().await;

        assert!(caches.items.get("series").await.is_some());
        assert!(caches.searches.get("season").await.is_some());
    }
}
