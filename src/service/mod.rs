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

use std::sync::Arc;

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

/// One write at a time per work.
///
/// `replace_children` deletes a work's provider rows and re-inserts them, and
/// nothing stops a refresh someone asked for by hand from landing on top of the
/// scheduler's sweep of the same entry. On PostgreSQL, where the two are not
/// serialised by the engine the way SQLite serialises them, the second
/// transaction's DELETE only sees the rows its own scan found: the first
/// transaction's freshly inserted credits survive it, the second adds its own,
/// and the cast is doubled. `media_credit` is the one child table with no key
/// to conflict on, so nothing catches it afterwards either.
///
/// A lock keyed on the work rather than one lock for all of them: two different
/// series being refreshed at once is the normal case and has never been a
/// problem.
static WRITING: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

fn write_lock(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut held = WRITING.lock();

    // Bounded: the keys are work ids, and a busy sweep touches a batch at a
    // time. Anything nobody else is holding has served its purpose.
    if held.len() > 256 {
        held.retain(|_, lock| Arc::strong_count(lock) > 1);
    }

    Arc::clone(held.entry(id.to_string()).or_default())
}

/// Carry over every child list this fetch came back empty-handed on.
///
/// The write replaces children wholesale, which is right when the providers
/// answered and wrong when they did not: a rate-limited TMDB and an expired
/// TheTVDB key between them turn a series with sixty-two episodes, its cast and
/// its artwork into a title and nothing else, and the row would then claim a
/// clean refresh. An empty list out of a merge of four providers means nobody
/// said, not that there are none — so what is already stored stands.
///
/// A list that came back with *fewer* entries is left alone. That is a provider
/// disagreeing rather than a provider missing, and picking a winner there is
/// what the merge is for.
fn keep_what_nobody_answered(item: &mut MediaItem, stored: MediaItem) {
    fn keep<T>(fresh: &mut Vec<T>, stored: Vec<T>) {
        if fresh.is_empty() {
            *fresh = stored;
        }
    }

    keep(&mut item.seasons, stored.seasons);
    keep(&mut item.episodes, stored.episodes);
    keep(&mut item.images, stored.images);
    keep(&mut item.credits, stored.credits);
    keep(&mut item.alternative_titles, stored.alternative_titles);
    keep(&mut item.ratings, stored.ratings);
    keep(&mut item.translations, stored.translations);
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
        && let Some(mut existing) = repo::item::get(&state.db, &existing_id).await?
    {
        item.id = existing.id.clone();
        item.created_at = existing.created_at.clone();
        item.is_manual = existing.is_manual;
        item.is_enabled = existing.is_enabled;

        repo::item::load_children(&state.db, &mut existing).await?;
        keep_what_nobody_answered(&mut item, existing);
    }

    item.refreshed_at = Some(crate::db::now());
    item.refresh_after = Some(next_refresh(state, &item));
    item.refresh_error = None;

    let serialised = write_lock(&item.id);
    let _writing = serialised.lock().await;

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
    // Loud, because the consequence is permanent and invisible: the marker
    // stays true, so the episodes this refresh added are never fetched in that
    // language again and nothing ever retries. An operator seeing French titles
    // stop appearing has nothing else to go on.
    if let Err(e) = repo::translation::clear_fetched(&state.db, &item.id).await {
        tracing::error!(
            id = %item.id,
            error = %e,
            "could not reset the language markers; episodes added by this refresh will not be \
             translated until the next successful one"
        );
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

    // Both steps can fail on an operator's typo — a TTL of a hundred million
    // years converts fine and then leaves the representable range on the add,
    // which chrono answers with a panic. Six hours is the answer to either.
    let fallback = chrono::TimeDelta::hours(6);
    let ttl = chrono::TimeDelta::from_std(ttl).unwrap_or(fallback);

    let due = chrono::Utc::now()
        .checked_add_signed(ttl)
        .unwrap_or_else(|| chrono::Utc::now() + fallback);

    to_rfc3339(due)
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
/// The key a search is cached under.
///
/// Everything that shapes the answer has to be in it. The adult flag
/// especially: without it a result computed for a caller who may see adult
/// titles would be handed to one who may not, and the cache would quietly undo
/// the policy — which is the kind of bug that looks like it works.
pub fn search_key(kind: &str, language: &str, adult: bool, extra: &str, term: &str) -> String {
    format!(
        "{kind}:{language}:{adult}:{extra}:{}",
        term.trim().to_lowercase()
    )
}

/// What a search produced, and whether it is the whole answer.
///
/// A search asks several providers and logs past the ones that fail, which is
/// right — one source being down should cost detail, not the result. It is not
/// a thing to remember for half an hour, though: a TMDB rate limit lasting ten
/// seconds used to fix one wrong answer in place for the whole TTL, with no way
/// to tell it from a complete one.
pub struct Found {
    pub items: Vec<MediaItem>,
    pub degraded: bool,
}

impl Found {
    pub fn complete(items: Vec<MediaItem>) -> Self {
        Self {
            items,
            degraded: false,
        }
    }
}

async fn cached_search<F, Fut>(state: &AppState, key: String, fetch: F) -> Result<Vec<MediaItem>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Found>>,
{
    if let Some(cached) = state.caches.searches.get(&key).await {
        if let Ok(items) = serde_json::from_str::<Vec<MediaItem>>(&cached) {
            return Ok(items);
        }
        state.caches.searches.invalidate(&key).await;
    }

    let found = fetch().await?;

    // An empty result is not cached: it is usually a provider hiccup, and
    // caching it would keep a title invisible for the whole TTL. Nor is a
    // partial one, for the same reason with more of it showing.
    if !found.items.is_empty()
        && !found.degraded
        && let Ok(encoded) = serde_json::to_string(&found.items)
    {
        state.caches.searches.insert(key, encoded).await;
    }

    Ok(found.items)
}

#[cfg(test)]
mod cache_key_tests {
    use super::search_key;

    #[test]
    fn two_policies_never_share_an_entry() {
        // The one that matters: same term, same language, different answer to
        // "may this caller see adult titles".
        assert_ne!(
            search_key("movie", "en-US", false, "", "matrix"),
            search_key("movie", "en-US", true, "", "matrix"),
        );
    }

    #[test]
    fn two_languages_never_share_an_entry() {
        assert_ne!(
            search_key("series", "en-US", false, "", "matrix"),
            search_key("series", "fr-FR", false, "", "matrix"),
        );
    }

    #[test]
    fn the_same_search_is_the_same_key() {
        assert_eq!(
            search_key("movie", "en-US", false, "1999", " The Matrix "),
            search_key("movie", "en-US", false, "1999", "the matrix"),
        );
    }
}
