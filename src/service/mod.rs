//! Resolution: turning a client's request into a canonical entity.
//!
//! Every lookup follows the same ladder, and the order is the point of the
//! whole design:
//!
//! 1. **The local store.** If we hold the work and it is not stale, answer from
//!    it. This is what makes the server independent of its providers.
//! 2. **A provider.** Fetch, map to canonical, persist, answer.
//! 3. **A fallback provider**, if the first had nothing.
//!
//! Persisting on the way out is deliberate: an answer relayed from upstream
//! becomes a local entry, so the next request is served from step 1 and a human
//! can edit it.

pub mod gather;
pub mod ids;
pub mod language;
pub mod movie;
pub mod series;

use anyhow::Result;
use serde_json::Value;

use crate::{
    db::{repo, to_rfc3339},
    domain::{ExternalSource, MediaItem, fields},
    state::AppState,
};

/// Load a work with its children and manual overrides applied.
///
/// This is the only read path that should be used to answer a request: it is
/// what guarantees a locked field is honoured.
pub async fn load(state: &AppState, id: &str) -> Result<Option<MediaItem>> {
    let cache_key = format!("item:{id}");

    if let Some(cached) = state.caches.items.get(&cache_key).await {
        if let Ok(item) = serde_json::from_str::<MediaItem>(&cached) {
            return Ok(Some(item));
        }
        // A cache entry that no longer deserializes means the model changed
        // under us; drop it and fall through to the database.
        state.caches.items.invalidate(&cache_key).await;
    }

    let Some(mut item) = repo::item::get(&state.db, id).await? else {
        return Ok(None);
    };

    repo::item::load_children(&state.db, &mut item).await?;

    let overrides = repo::override_field::list(&state.db, id).await?;
    fields::apply(&mut item, &overrides)?;

    if let Ok(encoded) = serde_json::to_string(&item) {
        state.caches.items.insert(cache_key, encoded).await;
    }

    Ok(Some(item))
}

/// Apply stored overrides to a batch of works.
///
/// Every path that returns more than one item goes through this. Without it a
/// list view would show provider values while the detail view showed edited
/// ones — the lock would look like it had not taken.
pub async fn apply_overrides(state: &AppState, items: &mut [MediaItem]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let by_item = repo::override_field::list_for_many(&state.db, &ids).await?;

    for item in items.iter_mut() {
        if let Some(overrides) = by_item.get(&item.id) {
            fields::apply(item, overrides)?;
        }
    }

    Ok(())
}

/// Find the local work matching any of `item`'s external ids.
///
/// Checked in order of how strongly each id identifies a single work: an IMDb
/// id is shared between a film and its remake far more often than a TMDB id is.
async fn find_existing(state: &AppState, item: &MediaItem) -> Result<Option<String>> {
    for (source, value) in item.external_ids.rows(item.kind) {
        if source == ExternalSource::Imdb {
            continue;
        }
        if let Some(id) = repo::item::find_id_by_external(&state.db, source, &value).await? {
            return Ok(Some(id));
        }
    }

    if let Some(imdb) = &item.external_ids.imdb
        && let Some(id) =
            repo::item::find_id_by_external(&state.db, ExternalSource::Imdb, imdb).await?
    {
        // An IMDb id is only decisive when the kinds agree.
        if let Some(existing) = repo::item::get(&state.db, &id).await?
            && existing.kind == item.kind
        {
            return Ok(Some(id));
        }
    }

    Ok(None)
}

/// Store a freshly fetched work and its raw provider payload.
///
/// If the work already exists locally, its identity is preserved: the same row
/// id, creation time and manual flag. Overwriting those would orphan every
/// override attached to it, which is exactly the failure this design exists to
/// prevent.
pub async fn persist(
    state: &AppState,
    mut item: MediaItem,
    snapshots: &[(String, Value)],
) -> Result<MediaItem> {
    if let Some(existing_id) = find_existing(state, &item).await?
        && let Some(existing) = repo::item::get(&state.db, &existing_id).await?
    {
        item.id = existing.id;
        item.created_at = existing.created_at;
        item.is_manual = existing.is_manual;
        item.is_enabled = existing.is_enabled;
    }

    item.refreshed_at = Some(crate::db::now());
    item.refresh_after = Some(next_refresh(state, &item));
    item.refresh_error = None;

    repo::item::upsert(
        &state.db,
        repo::item::ItemWrite {
            item: &item,
            replace_children: true,
        },
    )
    .await?;

    // Every provider's raw answer is kept, so a mapping fix can be replayed
    // without spending the calls again.
    for (provider, payload) in snapshots {
        repo::snapshot::put(&state.db, &item.id, provider, payload, None).await?;
    }

    // A refresh can add episodes, so whatever was fetched for another language
    // no longer covers the whole run. The stored text stays — it is keyed by
    // episode number and still correct for the episodes it names — but the
    // marker goes, so the next request in that language fills in the rest.
    if let Err(e) = repo::translation::clear_fetched(&state.db, &item.id).await {
        tracing::warn!(id = %item.id, error = %e, "could not reset the language markers");
    }

    state
        .caches
        .items
        .invalidate(&format!("item:{}", item.id))
        .await;

    // Re-read so the caller sees the same thing every later request will: the
    // stored row, with manual overrides applied on top.
    load(state, &item.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("item vanished immediately after being written"))
}

/// When this work should next be refetched.
///
/// A running series changes weekly; an ended one effectively never does. Using
/// one interval for both either hammers providers or serves stale episodes.
fn next_refresh(state: &AppState, item: &MediaItem) -> String {
    let cfg = &state.config.refresh;

    let ttl = match item.status.as_deref() {
        Some("ended") | Some("released") | Some("deleted") => cfg.ended_ttl,
        _ => cfg.continuing_ttl,
    };

    let ttl = chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::hours(6));
    to_rfc3339(chrono::Utc::now() + ttl)
}

/// Whether a stored work is due a refresh.
pub fn is_stale(item: &MediaItem) -> bool {
    match item
        .refresh_after
        .as_deref()
        .and_then(crate::db::parse_rfc3339)
    {
        Some(due) => due <= chrono::Utc::now(),
        // Never scheduled: manual entries, or something written before the
        // scheduler existed. Not stale — there may be nothing to refresh from.
        None => false,
    }
}

/// Serve from cache, or run `fetch` and cache its result.
async fn cached_search<F, Fut>(state: &AppState, key: String, fetch: F) -> Result<Vec<MediaItem>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<MediaItem>>>,
{
    if let Some(cached) = state.caches.searches.get(&key).await {
        if let Ok(items) = serde_json::from_str::<Vec<MediaItem>>(&cached) {
            return Ok(items);
        }
        state.caches.searches.invalidate(&key).await;
    }

    let items = fetch().await?;

    // An empty result is not cached: it is usually a provider hiccup, and
    // caching it would keep a title invisible for the whole TTL.
    if !items.is_empty()
        && let Ok(encoded) = serde_json::to_string(&items)
    {
        state.caches.searches.insert(key, encoded).await;
    }

    Ok(items)
}
