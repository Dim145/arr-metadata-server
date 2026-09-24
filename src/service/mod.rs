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

pub mod anime;
pub mod gather;
pub mod ids;
pub mod language;
pub mod listing;
pub mod movie;
pub mod series;

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use crate::{
    db::{repo, to_rfc3339},
    domain::{ExternalSource, MediaItem, Rating, fields},
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

    if state.flag("imdb.enabled", false)
        && let Err(e) = overlay_imdb(state, &mut item).await
    {
        // A rating is not worth failing a read over.
        tracing::warn!(
            id,
            error = format_args!("{e:#}"),
            "could not read IMDb's rating"
        );
    }

    let overrides = repo::override_field::list(&state.db, id).await?;
    fields::apply(&mut item, &overrides)?;

    if let Ok(encoded) = serde_json::to_string(&item) {
        state.caches.items.insert(cache_key, encoded).await;
    }

    Ok(Some(item))
}

/// IMDb's figure for a work, from the daily list, where it is the newer one.
///
/// Skyhook and Radarr's metadata server both republish IMDb's rating, so a
/// stored one is often there already; the list is what has it when neither was
/// asked, and what keeps it current between refreshes. Newer means more votes —
/// a count that only grows is a better clock than either source's fetch date.
async fn overlay_imdb(state: &AppState, item: &mut MediaItem) -> Result<()> {
    let Some(tconst) = item.external_ids.imdb.as_deref() else {
        return Ok(());
    };
    let Some(listed) = repo::imdb::get(&state.db, tconst).await? else {
        return Ok(());
    };

    take_newer_imdb(&mut item.ratings, &listed);
    Ok(())
}

/// Leave out of works what only somebody maintaining the catalogue is told:
/// how the last refresh went — the error can name a provider's address, or the
/// host this server reaches it on — when the next one is due, and which fields
/// are locked. Applied last, since the language overlay reads the locks.
pub fn redact_for_reader(identity: &crate::auth::Identity, items: &mut [MediaItem]) {
    if identity.can_write() {
        return;
    }

    for item in items {
        item.refresh_error = None;
        item.refresh_after = None;
        item.locked_fields.clear();
    }
}

/// IMDb's figure for every work of a list, in one query.
///
/// A failure costs the list IMDb's figure and nothing else: the stored ratings
/// are already on every card.
pub async fn overlay_imdb_many(state: &AppState, items: &mut [MediaItem]) {
    if !state.flag("imdb.enabled", false) {
        return;
    }

    let tconsts: Vec<String> = items
        .iter()
        .filter_map(|i| i.external_ids.imdb.clone())
        .collect();

    match repo::imdb::get_many(&state.db, &tconsts).await {
        Ok(listed) => {
            for item in items.iter_mut() {
                if let Some(rating) = item
                    .external_ids
                    .imdb
                    .as_deref()
                    .and_then(|t| listed.get(t))
                {
                    take_newer_imdb(&mut item.ratings, rating);
                }
            }
        }
        Err(e) => tracing::warn!(
            error = format_args!("{e:#}"),
            "could not read IMDb's ratings for a list"
        ),
    }
}

/// Put IMDb's listed figure in place of the stored one, unless the stored one
/// has more votes and so is the newer of the two.
fn take_newer_imdb(ratings: &mut Vec<Rating>, listed: &repo::imdb::Rating) {
    let rating = Rating {
        source: "imdb".to_string(),
        value: Some(listed.rating),
        votes: Some(listed.votes),
        rating_type: Some("user".to_string()),
    };

    match ratings.iter_mut().find(|r| r.source == "imdb") {
        Some(stored) if stored.votes.unwrap_or(0) > listed.votes => {}
        Some(stored) => *stored = rating,
        None => ratings.push(rating),
    }
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
    let rows = item.external_ids.rows(item.kind);
    let (decisive, weak): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|(source, _)| names_one_work(*source));

    for (source, value) in decisive {
        if let Some(id) = repo::item::find_id_by_external(&state.db, source, &value).await? {
            return Ok(Some(id));
        }
    }

    // IMDb last: it is the one most often shared, between a film and its remake
    // or two TMDB entries for the same thing.
    let (imdb, others): (Vec<_>, Vec<_>) = weak
        .into_iter()
        .partition(|(source, _)| *source == ExternalSource::Imdb);

    for (source, value) in others.into_iter().chain(imdb) {
        if let Some(id) = repo::item::find_id_by_external(&state.db, source, &value).await?
            && let Some(existing) = repo::item::get(&state.db, &id).await?
            && same_work(&existing, item)
        {
            return Ok(Some(id));
        }
    }

    Ok(None)
}

/// Whether an id of this source names one work, and only one kind of work.
///
/// TMDB, TheTVDB and Trakt number films and series separately and give each
/// work its own id. The rest do not settle it alone: IMDb ids are shared
/// between TMDB entries; MyAnimeList and AniList number films and series in
/// one sequence, and one of their entries is filed under a series by Skyhook
/// and under another by the anime identifier list; TVmaze and TVRage ids are
/// only as right as the provider that relayed them.
fn names_one_work(source: ExternalSource) -> bool {
    matches!(
        source,
        ExternalSource::TmdbMovie
            | ExternalSource::TmdbTv
            | ExternalSource::TvdbSeries
            | ExternalSource::TvdbMovie
            | ExternalSource::TraktShow
            | ExternalSource::TraktMovie
    )
}

/// Whether a stored work, found by an id that does not settle it, can be the
/// one `incoming` describes: the same kind, and no TMDB or TheTVDB id of its
/// own that says it is something else.
///
/// Without this, a new series sharing a MyAnimeList id with a stored one was
/// written over it — its overrides, its identifiers, its episodes.
fn same_work(existing: &MediaItem, incoming: &MediaItem) -> bool {
    let clash = |a: Option<i64>, b: Option<i64>| matches!((a, b), (Some(a), Some(b)) if a != b);

    existing.kind == incoming.kind
        && !clash(existing.external_ids.tmdb, incoming.external_ids.tmdb)
        && !clash(existing.external_ids.tvdb, incoming.external_ids.tvdb)
}

/// Locks by name, made when first asked for and forgotten once nobody holds
/// them.
///
/// For work that must not happen twice at once *for the same thing* while
/// staying free to happen for different things at once — which is every case
/// here: two series refreshing side by side is the normal state of affairs.
pub struct Keyed {
    held: parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Keyed {
    fn new() -> Self {
        Self {
            held: parking_lot::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Wait for `key`, and hold it until the guard is dropped.
    pub async fn lock(&self, key: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut held = self.held.lock();

            // Bounded, because the keys are ids a client supplied. A lock
            // nobody is holding or waiting on has served its purpose: the
            // guard owns a clone, so anything in use has a count above one.
            if held.len() > 256 {
                held.retain(|_, lock| Arc::strong_count(lock) > 1);
            }

            Arc::clone(held.entry(key.to_string()).or_default())
        };

        lock.lock_owned().await
    }
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
static WRITING: std::sync::LazyLock<Keyed> = std::sync::LazyLock::new(Keyed::new);

/// One fetch at a time per work, as a client addressed it.
///
/// When a work goes stale, every request for it misses the store and runs the
/// whole fan-out — a TMDB call per season, TheTVDB's pages, Skyhook, Fanart —
/// and a library refresh or a request page opening is exactly when several
/// arrive at once. The first does the work; the rest wait for it and then find
/// what it stored, which is why every caller checks the store again after
/// getting through.
pub static FETCHING: std::sync::LazyLock<Keyed> = std::sync::LazyLock::new(Keyed::new);

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

    crate::merge::drop_own_title(&mut item);

    item.refreshed_at = Some(crate::db::now());
    item.refresh_after = Some(next_refresh(state, &item));
    item.refresh_error = None;

    let _writing = WRITING.lock(&item.id).await;

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

    // Listed by what it now holds, with its locks, before anyone lists it.
    listing::after_write(state, &item.id).await;

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
    // Read once, before anything is fetched, so the answer is filed under the
    // settings it was actually computed with. See `Caches::generation`.
    let generation = state
        .caches
        .generation
        .load(std::sync::atomic::Ordering::SeqCst);
    let key = format!("{generation}:{key}");

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
mod keyed_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn the_same_key_is_done_once_at_a_time() {
        // Eight callers asking for one work: the fetch must never run for two
        // of them at once, or the fan-out happens eight times over.
        let keyed = Arc::new(Keyed::new());
        let inside = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));

        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let (keyed, inside, most) = (keyed.clone(), inside.clone(), most.clone());
                tokio::spawn(async move {
                    let _held = keyed.lock("series:tvdb:81189").await;
                    let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();

        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(most.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_keys_do_not_wait_for_each_other() {
        // Two different series refreshing side by side is the normal case.
        let keyed = Keyed::new();

        let _first = keyed.lock("series:tvdb:1").await;
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            keyed.lock("series:tvdb:2"),
        )
        .await;

        assert!(second.is_ok(), "a different key was made to wait");
    }
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

#[cfg(test)]
mod imdb_overlay_tests {
    use super::*;

    fn listed(rating: f64, votes: i64) -> repo::imdb::Rating {
        repo::imdb::Rating {
            tconst: "tt0903747".into(),
            rating,
            votes,
        }
    }

    fn stored(value: f64, votes: i64) -> Rating {
        Rating {
            source: "imdb".into(),
            value: Some(value),
            votes: Some(votes),
            rating_type: Some("user".into()),
        }
    }

    #[test]
    fn the_list_supplies_a_rating_nobody_stored() {
        let mut ratings = Vec::new();
        take_newer_imdb(&mut ratings, &listed(9.5, 2_679_470));

        assert_eq!(ratings.len(), 1);
        assert_eq!(ratings[0].votes, Some(2_679_470));
        assert_eq!(ratings[0].rating_type.as_deref(), Some("user"));
    }

    #[test]
    fn the_list_replaces_an_older_figure() {
        // Stored at the last refresh, weeks ago; the list is this morning's.
        let mut ratings = vec![stored(9.4, 2_500_000)];
        take_newer_imdb(&mut ratings, &listed(9.5, 2_679_470));

        assert_eq!(ratings, vec![stored(9.5, 2_679_470)]);
    }

    #[test]
    fn a_newer_stored_figure_is_kept() {
        // Skyhook's is a day fresher than IMDb's own dataset.
        let mut ratings = vec![stored(9.5, 2_679_821)];
        take_newer_imdb(&mut ratings, &listed(9.5, 2_679_470));

        assert_eq!(ratings, vec![stored(9.5, 2_679_821)]);
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use crate::domain::{ExternalIds, MediaKind};

    fn work(kind: MediaKind, tmdb: Option<i64>, tvdb: Option<i64>, mal: &[i64]) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.external_ids = ExternalIds {
            tmdb,
            tvdb,
            mal: mal.to_vec(),
            ..Default::default()
        };
        item
    }

    #[test]
    fn a_shared_anime_id_does_not_make_two_series_one() {
        let stored = work(MediaKind::Series, Some(1429), Some(267440), &[16498]);
        let incoming = work(MediaKind::Series, None, Some(999999), &[16498]);

        assert!(!same_work(&stored, &incoming));
    }

    #[test]
    fn nor_a_film_and_a_series() {
        let series = work(MediaKind::Series, None, Some(81797), &[460]);
        let film = work(MediaKind::Movie, Some(23446), None, &[460]);

        assert!(!same_work(&series, &film));
    }

    #[test]
    fn a_work_the_ids_do_not_contradict_is_the_same_one() {
        // Stored before its TMDB id was known; nothing it holds says otherwise.
        let stored = work(MediaKind::Series, None, Some(267440), &[16498]);
        let incoming = work(MediaKind::Series, Some(1429), Some(267440), &[16498]);

        assert!(same_work(&stored, &incoming));
    }

    #[test]
    fn only_the_ids_that_settle_a_work_decide_alone() {
        assert!(names_one_work(ExternalSource::TvdbSeries));
        assert!(names_one_work(ExternalSource::TmdbMovie));
        for weak in [
            ExternalSource::Imdb,
            ExternalSource::Mal,
            ExternalSource::AniList,
            ExternalSource::TvMaze,
            ExternalSource::TvRage,
        ] {
            assert!(!names_one_work(weak), "{weak:?}");
        }
    }
}
