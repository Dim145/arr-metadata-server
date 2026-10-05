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
pub mod orders;
pub mod scene;
pub mod series;
pub mod webhook;

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use crate::{
    db::{repo, to_rfc3339},
    domain::{ExternalSource, MediaItem, MediaKind, Rating, fields},
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

    // The caches' marks as the read begins. A write that lands while the
    // work is being read — a lock put on it from another request, here or
    // on another instance — moves them, and what was read is then a moment
    // too old to keep: answered, but not cached, so the next read starts
    // afresh rather than finding the stale copy that an insert after the
    // write's invalidation would leave. Checked again once the copy is kept,
    // on the server too; see `Space::insert_unless_moved`.
    let mark = state.caches.read_mark();

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

    // After the locks: a still put on an episode by hand is a provider's
    // address too, and may be kept.
    state.media.localize(&mut item);

    if state.caches.read_mark() == mark
        && let Ok(encoded) = serde_json::to_string(&item)
    {
        state
            .caches
            .items
            .insert_unless_moved(cache_key, encoded, || state.caches.read_mark() == mark)
            .await;
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
/// What a card draws of a work, and nothing more.
///
/// A list, a schedule, a filmography, a season chart: each is drawn as cards,
/// one poster, a title and a score apiece — and each was sent every work
/// whole, ninety-four translations and forty-seven pictures for one film,
/// fifty kilobytes a card, a megabyte and a half for a page of the catalogue.
/// Kept: the poster a card shows, a hand-picked one first; one backdrop and
/// one landscape for a page that lays the work out wide; one logo for the
/// front page. The text is in the reader's language by the time this runs, so
/// the translations it was taken from can go, and the keywords with them.
pub fn as_card(work: &mut MediaItem) {
    use crate::domain::{CoverType, Image};

    work.translations.clear();
    work.keywords.clear();
    work.relations.clear();

    let mut kept = Vec::with_capacity(4);
    for kind in [
        CoverType::Poster,
        CoverType::Fanart,
        CoverType::Landscape,
        CoverType::Clearlogo,
    ] {
        let own = |i: &Image| i.cover_type == kind && i.season_number.is_none();
        let shown = work
            .images
            .iter()
            .position(|i| own(i) && i.is_manual)
            .or_else(|| work.images.iter().position(own));
        if let Some(index) = shown {
            kept.push(work.images[index].clone());
        }
    }
    work.images = kept;
}

/// Put first among a work's relations the Fan-Kai cut from it, which the
/// catalogue holds by definition — see `repo::item::recuts`. A failure costs
/// the links, not the page.
pub async fn add_recuts(state: &AppState, item: &mut MediaItem) {
    if item.external_ids.anilist.is_empty() && item.external_ids.mal.is_empty() {
        return;
    }

    match repo::item::recuts(&state.db, &item.id, &item.external_ids).await {
        Ok(found) => {
            let fresh: Vec<_> = found
                .into_iter()
                .filter(|r| {
                    !item
                        .relations
                        .iter()
                        .any(|known| known.work_id == r.work_id)
                })
                .map(|mut r| {
                    if let Some(image) = &r.image {
                        r.image = Some(state.media.localized(image));
                    }
                    r
                })
                .collect();
            item.relations.splice(0..0, fresh);
        }
        Err(e) => {
            tracing::warn!(id = %item.id, error = %e, "the Fan-Kai cut from a work could not be read")
        }
    }
}

/// Keep from a reader who may not see adult works the entries filed beside
/// a work that are for adults — by AniList's flag, or by what the catalogue
/// holds of them — as the list would have kept the works themselves.
pub fn hide_adult_relations(items: &mut [MediaItem]) {
    for item in items {
        item.relations.retain(|r| !r.is_adult);
    }
}

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
pub(crate) async fn find_existing(state: &AppState, item: &MediaItem) -> Result<Option<String>> {
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
/// only as right as the provider that relayed them. Fankai numbers each of
/// its productions once, and nothing else carries its ids.
fn names_one_work(source: ExternalSource) -> bool {
    matches!(
        source,
        ExternalSource::TmdbMovie
            | ExternalSource::TmdbTv
            | ExternalSource::TvdbSeries
            | ExternalSource::TvdbMovie
            | ExternalSource::TraktShow
            | ExternalSource::TraktMovie
            | ExternalSource::Fankai
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
///
/// Relations too: they come from one source each — AniList for an anime, the
/// Fankai wiki for a Fan-Kai — so one of them timing out on a refresh left a
/// work with none, and an anime without the Fan-Kai cut from it.
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
    keep(&mut item.relations, stored.relations);
}

/// Carry over what TheTVDB adds to the series' name when neither TheTVDB nor
/// Skyhook is among the answers written (`snapshots`).
///
/// The merge reads it off their own answers alone, so without them it comes
/// back empty though nothing said it was gone: a refresh while both were
/// down, or a sync that asked only the others, sent *Rurouni Kenshin (2023)*
/// to Sonarr as *Rurouni Kenshin* — the other series' title, the clash it is
/// kept to prevent — until a refresh they answered. When one of them did
/// answer, what the merge made of it stands, none included.
fn keep_title_qualifier(item: &mut MediaItem, stored: &MediaItem, snapshots: &[(String, Value)]) {
    let named = snapshots
        .iter()
        .any(|(provider, _)| crate::merge::TVDB_NAMED.contains(&provider.as_str()));
    if !named {
        item.title_qualifier.clone_from(&stored.title_qualifier);
    }
}

/// The most of a failure kept on a work as its `refreshError`.
pub const REFRESH_ERROR_CHARS: usize = 500;

/// Store a freshly fetched work and its raw provider payload.
///
/// If the work already exists locally, its identity is preserved: the same row
/// id, creation time and manual flag. Overwriting those would orphan every
/// override attached to it, which is exactly the failure this design exists to
/// prevent.
///
/// `failure` says which providers did not answer, when some did not: the
/// work is then written as refreshed in part — the failure kept as its
/// `refreshError`, and its next refresh brought forward to when a failed one
/// is tried again ([`retry_after_failure`]) rather than a full interval away.
pub async fn persist(
    state: &AppState,
    mut item: MediaItem,
    snapshots: &[(String, Value)],
    provenance: crate::merge::provenance::Provenance,
    failure: Option<&str>,
) -> Result<MediaItem> {
    // As the merge came back, before what it lacked is put back.
    let returned = crate::merge::provenance::Returned::of(&item);

    if let Some(existing_id) = find_existing(state, &item).await?
        && let Some(mut existing) = repo::item::get(&state.db, &existing_id).await?
    {
        item.id = existing.id.clone();
        item.created_at = existing.created_at.clone();
        item.is_manual = existing.is_manual;
        item.is_enabled = existing.is_enabled;

        repo::item::load_children(&state.db, &mut existing).await?;
        keep_title_qualifier(&mut item, &existing, snapshots);
        keep_what_nobody_answered(&mut item, existing);

        // What is locked of its identity is written as locked, not as a
        // provider said it: the row is what the lists and the lookups read.
        let locked = repo::override_field::list(&state.db, &item.id).await?;
        crate::domain::fields::pin_identity(&mut item, &locked);
    }

    crate::merge::drop_own_title(&mut item);

    item.refreshed_at = Some(crate::db::now());
    match failure {
        None => {
            item.refresh_after = Some(next_refresh(state, &item));
            item.refresh_error = None;
        }
        Some(failure) => {
            item.refresh_after = Some(next_refresh_in_part(state, &item));
            item.refresh_error = Some(crate::providers::clip(failure, REFRESH_ERROR_CHARS));
        }
    }

    let _writing = WRITING.lock(&item.id).await;

    // What the write keeps when every provider came back without it is still
    // whoever gave it before. Read under the lock every writer of the record
    // takes, and written in the same transaction as the work.
    let mut provenance = provenance;
    match repo::item::provenance(&state.db, &item.id).await {
        Ok(Some(before)) => {
            crate::merge::provenance::carry_over(&mut provenance, &before, &returned);
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(id = %item.id, error = %e, "could not read where the work's values came from")
        }
    }

    repo::item::upsert_traced(
        &state.db,
        repo::item::ItemWrite {
            item: &item,
            replace_children: true,
        },
        &provenance,
    )
    .await?;

    after_write(state, &item.id, snapshots).await
}

/// What became of a sync's write.
pub enum SyncWrite {
    Written(Box<MediaItem>),
    /// Something else wrote the work after the sync read it. Nothing was
    /// written: values rebuilt from the older copy would have undone it.
    Changed,
    /// The work was deleted meanwhile.
    Gone,
}

/// Store a sync's result in the work it was made from.
///
/// Into that row and no other: a fresh answer can name an id another work
/// holds, and the lookup [`persist`] makes by id would write this work over
/// that one. With the work's own identity, identifiers and schedule — a sync
/// asks some sources again, it neither re-keys the work nor counts as its
/// refresh — and only if nothing else has written the work since `stored` was
/// read, checked in the write's own transaction. The provenance is written in
/// it too, so it always describes the write beside it.
pub async fn persist_sync(
    state: &AppState,
    mut item: MediaItem,
    snapshots: &[(String, Value)],
    stored: &MediaItem,
    provenance: &crate::merge::provenance::Provenance,
) -> Result<SyncWrite> {
    item.id.clone_from(&stored.id);
    item.slug.clone_from(&stored.slug);
    item.created_at.clone_from(&stored.created_at);
    item.updated_at = crate::db::now();
    item.is_manual = stored.is_manual;
    item.is_enabled = stored.is_enabled;
    item.external_ids = stored.external_ids.clone();
    keep_title_qualifier(&mut item, stored, snapshots);
    keep_what_nobody_answered(&mut item, stored.clone());
    crate::merge::drop_own_title(&mut item);

    item.refreshed_at.clone_from(&stored.refreshed_at);
    item.refresh_after.clone_from(&stored.refresh_after);
    item.refresh_error.clone_from(&stored.refresh_error);

    let _writing = WRITING.lock(&item.id).await;

    let written = repo::item::upsert_unchanged(
        &state.db,
        repo::item::ItemWrite {
            item: &item,
            replace_children: true,
        },
        &stored.updated_at,
        provenance,
    )
    .await?;

    match written {
        repo::item::Conditional::Written => after_write(state, &item.id, snapshots)
            .await
            .map(|item| SyncWrite::Written(Box::new(item))),
        repo::item::Conditional::Changed => Ok(SyncWrite::Changed),
        repo::item::Conditional::Gone => Ok(SyncWrite::Gone),
    }
}

/// Everything a write of a work entails beyond its row, under its lock.
async fn after_write(
    state: &AppState,
    id: &str,
    snapshots: &[(String, Value)],
) -> Result<MediaItem> {
    // Every provider's raw answer is kept, so a mapping fix can be replayed
    // without spending the calls again.
    for (provider, payload) in snapshots {
        repo::snapshot::put(&state.db, id, provider, payload, None).await?;
    }

    // Listed by what it now holds, with its locks, before anyone lists it.
    listing::after_write(state, id).await;

    // A refresh can add episodes, so whatever was fetched for another language
    // no longer covers the whole run. The stored text stays — it is keyed by
    // episode number and still correct for the episodes it names — but the
    // marker goes, so the next request in that language fills in the rest.
    // Loud, because the consequence is permanent and invisible: the marker
    // stays true, so the episodes this refresh added are never fetched in that
    // language again and nothing ever retries. An operator seeing French titles
    // stop appearing has nothing else to go on.
    if let Err(e) = repo::translation::clear_fetched(&state.db, id).await {
        tracing::error!(
            id = %id,
            error = %e,
            "could not reset the language markers; episodes added by this refresh will not be \
             translated until the next successful one"
        );
    }

    state.caches.touched(id).await;

    // Re-read so the caller sees the same thing every later request will: the
    // stored row, with manual overrides applied on top.
    let item = load(state, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("item vanished immediately after being written"))?;

    // Its pictures and its theme, in line to be kept.
    state.media.enqueue_for(state, &item).await;

    Ok(item)
}

/// When this work should next be refetched.
///
/// A running series changes weekly; an ended one effectively never does. Using
/// one interval for both either hammers providers or serves stale episodes.
/// A series near an air date comes back sooner: see [`near_air`].
fn next_refresh(state: &AppState, item: &MediaItem) -> String {
    let cfg = &state.config.refresh;

    let ttl = if has_finished(item) {
        cfg.ended_ttl
    } else {
        cfg.continuing_ttl
    };

    to_rfc3339(refresh_at(item, ttl, Utc::now()))
}

/// When a work refreshed in part — some providers did not answer — is fetched
/// again: when a failed refresh would be, and never later than a complete one
/// would have been.
fn next_refresh_in_part(state: &AppState, item: &MediaItem) -> String {
    let cfg = &state.config.refresh;
    let ttl = if has_finished(item) {
        cfg.ended_ttl
    } else {
        cfg.continuing_ttl
    };
    let now = Utc::now();

    to_rfc3339(retry_at(Some(item), now).min(refresh_at(item, ttl, now)))
}

/// [`next_refresh`], as of `now`.
fn refresh_at(item: &MediaItem, ttl: std::time::Duration, now: DateTime<Utc>) -> DateTime<Utc> {
    // Both steps can fail on an operator's typo — a TTL of a hundred million
    // years converts fine and then leaves the representable range on the add,
    // which chrono answers with a panic. Six hours is the answer to either.
    let fallback = TimeDelta::hours(6);
    let ttl = TimeDelta::from_std(ttl).unwrap_or(fallback);

    let due = now
        .checked_add_signed(ttl)
        .unwrap_or_else(|| now + fallback);

    sooner(due, near_air(item, now))
}

/// Whether a work is done changing: an ended or deleted series, a released
/// film.
fn has_finished(item: &MediaItem) -> bool {
    matches!(
        item.status.as_deref(),
        Some("ended" | "released" | "deleted")
    )
}

/// The earlier of the two, when there is another.
fn sooner(due: DateTime<Utc>, other: Option<DateTime<Utc>>) -> DateTime<Utc> {
    other.map_or(due, |other| due.min(other))
}

/// When a series near an air date is due again, if it is near one.
///
/// Any series still running is fetched again an hour after its next episode
/// airs, whatever its interval: Sonarr searches for an episode once it has
/// aired, with what it was last told of it. And one that has not started —
/// upcoming, or with no episodes yet — every two hours in the week of its
/// premiere or its next episode, every hour in the day of it, after the
/// premiere as before it: that is when its episodes reach the providers.
/// *Magical Explorer*'s reached TheTVDB and TMDB hours after it premiered,
/// and a six-hour interval kept them from Sonarr for longer still.
///
/// Nothing for a film, a series that has ended, or one with no date near:
/// they keep their interval.
fn near_air(item: &MediaItem, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if item.kind != MediaKind::Series || has_finished(item) {
        return None;
    }

    let next = next_air(item, now);
    let mut due = next.and_then(|at| at.checked_add_signed(TimeDelta::hours(1)));

    if item.status.as_deref() == Some("upcoming") || item.episodes.is_empty() {
        let premiere = item.first_aired.as_deref().and_then(instant);
        let nearest = [premiere, next]
            .into_iter()
            .flatten()
            .map(|at| (at - now).abs())
            .min();
        let every = match nearest {
            Some(gap) if gap <= TimeDelta::days(1) => Some(TimeDelta::hours(1)),
            Some(gap) if gap <= TimeDelta::days(7) => Some(TimeDelta::hours(2)),
            _ => None,
        };
        if let Some(every) = every {
            due = Some(sooner(now + every, due));
        }
    }

    due
}

/// When a series next airs, as far as the copy at hand says: its earliest
/// episode still to come, at the instant a provider gave or else at midnight
/// UTC of its day — what Sonarr is told (`wire::sonarr`) — or its premiere
/// when no episode is dated. An airing is still to come until an hour after
/// it, so that a refresh within that hour does not skip the one after it.
fn next_air(item: &MediaItem, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let dated: Vec<DateTime<Utc>> = item.episodes.iter().filter_map(airs).collect();
    let airings = if dated.is_empty() {
        item.first_aired
            .as_deref()
            .and_then(instant)
            .into_iter()
            .collect()
    } else {
        dated
    };

    let since = now - TimeDelta::hours(1);
    airings.into_iter().filter(|at| *at > since).min()
}

/// When an episode airs: the instant a provider gave, else its day.
fn airs(episode: &crate::domain::Episode) -> Option<DateTime<Utc>> {
    episode
        .air_date_utc
        .as_deref()
        .and_then(crate::db::parse_rfc3339)
        .or_else(|| episode.air_date.as_deref().and_then(instant))
}

/// A date or a date-time as an instant; a day alone is midnight UTC.
fn instant(value: &str) -> Option<DateTime<Utc>> {
    crate::domain::midnight_utc(value)
        .as_deref()
        .and_then(crate::db::parse_rfc3339)
}

/// How long a work whose refresh failed waits before it is tried again.
///
/// Without this, an id that has been deleted upstream would be retried on
/// every scheduler tick forever — and a series a client asks for, on every
/// request, each one waiting on the same providers to fail.
const FAILURE_BACKOFF: TimeDelta = TimeDelta::hours(6);

/// When a work whose refresh just failed is next tried: after the backoff,
/// or sooner for a series near an air date, which keeps its cadence through a
/// provider's outage. `None` for a work that could not even be read.
pub fn retry_after_failure(item: Option<&MediaItem>) -> String {
    to_rfc3339(retry_at(item, Utc::now()))
}

/// [`retry_after_failure`], as of `now`.
fn retry_at(item: Option<&MediaItem>, now: DateTime<Utc>) -> DateTime<Utc> {
    let due = now + FAILURE_BACKOFF;
    item.map_or(due, |item| sooner(due, near_air(item, now)))
}

/// Whether a stored series should be fetched again because a client asks for
/// it now, though its refresh is not due.
///
/// When it has no episodes, or it premieres within two days either side: a
/// new series' episodes reach the providers around its premiere, often only
/// once it has aired, and Sonarr would otherwise be given the copy from
/// before, and keep it until it next asks, hours later. Not within the hour
/// of the last fetch, or of the last attempt — a provider that is down would
/// otherwise be waited on by every request. Never for an entry made by hand,
/// nor one never fetched.
pub fn due_on_read(item: &MediaItem, now: DateTime<Utc>) -> bool {
    if item.kind != MediaKind::Series || item.is_manual {
        return false;
    }
    let Some(refreshed) = item
        .refreshed_at
        .as_deref()
        .and_then(crate::db::parse_rfc3339)
    else {
        return false;
    };
    if now - refreshed <= TimeDelta::hours(1) {
        return false;
    }

    item.episodes.is_empty()
        || item
            .first_aired
            .as_deref()
            .and_then(instant)
            .is_some_and(|premiere| (premiere - now).abs() <= TimeDelta::days(2))
}

/// Whether a client asking for a stored work at `now` is answered with the
/// copy held, rather than with one fetched again first.
///
/// When it is not due: its refresh has not come, and a series has no reason
/// to be fetched for the client asking ([`due_on_read`]). Always for an entry
/// made by hand, which has no provider behind it. And always for one switched
/// off in the catalogue: Sonarr and Radarr are still answered with it — a 404
/// is what Sonarr takes a series for deleted on — but from the store, however
/// old the copy. Taken for absent, it was fetched again from every provider
/// on each of their requests and served all the same. The sweep passes it by
/// too ([`repo::item::due_for_refresh`]); a refresh asked for by hand is what
/// fetches it again.
pub fn served_as_held(item: &MediaItem, now: DateTime<Utc>) -> bool {
    !item.is_enabled || item.is_manual || !(is_stale(item, now) || due_on_read(item, now))
}

/// Whether a stored work is due a refresh, as of `now`.
fn is_stale(item: &MediaItem, now: DateTime<Utc>) -> bool {
    match item
        .refresh_after
        .as_deref()
        .and_then(crate::db::parse_rfc3339)
    {
        Some(due) => due <= now,
        // Never scheduled: manual entries, or something written before the
        // scheduler existed. Not stale — there may be nothing to refresh from.
        None => false,
    }
}

/// A stored work by an id a client knows it by, whatever its age, switched
/// off or not: what [`local`] weighs, and what [`or_held`] answers with when
/// fetching it again came to nothing. For a source that is switched off, what
/// it fetched while on is served as it is. A work switched off in the
/// catalogue is still Sonarr's or Radarr's — a 404 would have Sonarr take a
/// series for deleted — so it is served from here like any other.
async fn held(state: &AppState, source: ExternalSource, value: &str) -> Result<Option<MediaItem>> {
    let Some(id) = repo::item::find_id_by_external(&state.db, source, value).await? else {
        return Ok(None);
    };

    load(state, &id).await
}

/// A stored work, if it exists and will do as it is: see [`served_as_held`].
///
/// Not when it is due: its refresh has come, or a client asking for a series
/// now is to be given a fresh copy (see [`due_on_read`]). Returning `None`
/// then lets the ladder continue to a refetch, and [`or_held`] answers with
/// this copy after all when that comes to nothing. One switched off is never
/// due here: it is served as it is held, and no provider is asked.
async fn local(state: &AppState, source: ExternalSource, value: &str) -> Result<Option<MediaItem>> {
    let Some(item) = held(state, source, value).await? else {
        return Ok(None);
    };

    Ok(served_as_held(&item, Utc::now()).then_some(item))
}

/// What a fetch came to, or the stored work of that `kind` when it came to
/// nothing.
///
/// A series or a film held here and due again is fetched before it is
/// served; when no provider answers, or the fetch fails, the copy held is
/// served as it is — answering when the providers do not is what keeping one
/// is for — rather than a 404 Sonarr or Radarr would act on. The attempt is
/// recorded as a failed refresh, so the requests that follow are answered
/// from the store at once instead of each waiting on the same providers to
/// fail again: until the retry the failure sets ([`retry_after_failure`]),
/// and for a series near its premiere, for the hour [`due_on_read`] leaves
/// between two attempts.
async fn or_held(
    state: &AppState,
    kind: MediaKind,
    source: ExternalSource,
    value: &str,
    fetched: Result<Option<MediaItem>>,
) -> Result<Option<MediaItem>> {
    let failure = match &fetched {
        Ok(Some(_)) => return fetched,
        Ok(None) => "no provider answered; the stored entry was kept".to_string(),
        Err(e) => crate::providers::clip(&format!("{e:#}"), REFRESH_ERROR_CHARS),
    };

    let item = match held(state, source, value).await {
        Ok(Some(item)) if item.kind == kind => item,
        _ => return fetched,
    };

    // Refreshed meanwhile — by the sweep, say — or switched off since: the
    // copy is the one to serve, and no refresh of it failed.
    if served_as_held(&item, Utc::now()) {
        return Ok(Some(item));
    }

    tracing::warn!(
        id = %item.id,
        error = %failure,
        "the work could not be fetched again; answering with the stored copy"
    );
    let next = retry_after_failure(Some(&item));
    if let Err(e) =
        repo::item::mark_refreshed(&state.db, &item.id, Some(&next), Some(&failure)).await
    {
        tracing::warn!(id = %item.id, error = %e, "could not record the failed refresh");
    }
    state.caches.touched(&item.id).await;

    Ok(Some(item))
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
    let generation = state.caches.stamp();
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

    #[test]
    fn a_refresh_nobody_answered_for_keeps_what_was_filed_beside_the_work() {
        let related = |id: i64| crate::domain::Relation {
            id: String::new(),
            relation_type: "ORIGINAL".into(),
            source: "anilist".into(),
            external_id: id,
            mal_id: None,
            title: "Naruto: Shippuden".into(),
            medium: "anime".into(),
            format: None,
            year: None,
            image: None,
            is_adult: false,
            work_id: None,
            sort_order: 0,
        };

        let mut stored = MediaItem::empty(crate::domain::MediaKind::Series);
        stored.relations = vec![related(1735)];

        // The wiki timed out: nothing said about relations, so they stand.
        let mut fresh = MediaItem::empty(crate::domain::MediaKind::Series);
        keep_what_nobody_answered(&mut fresh, stored.clone());
        assert_eq!(fresh.relations.len(), 1);

        // It answered: what it said replaces what was there.
        let mut answered = MediaItem::empty(crate::domain::MediaKind::Series);
        answered.relations = vec![related(20)];
        keep_what_nobody_answered(&mut answered, stored);
        assert_eq!(answered.relations[0].external_id, 20);
    }

    #[test]
    fn what_thetvdb_adds_to_the_name_stands_until_thetvdb_or_skyhook_answers_again() {
        let answers = |providers: &[&str]| -> Vec<(String, Value)> {
            providers
                .iter()
                .map(|p| (p.to_string(), Value::Null))
                .collect()
        };
        let mut stored = MediaItem::empty(crate::domain::MediaKind::Series);
        stored.title = "Rurouni Kenshin".into();
        stored.title_qualifier = Some("2023".into());

        // TMDB and Fanart answered a refresh, or a sync asked only them: the
        // merge had no name of TheTVDB's to read it off, so it stands.
        // Dropped, Sonarr was sent the title of the other series by the name.
        let mut fresh = MediaItem::empty(crate::domain::MediaKind::Series);
        keep_title_qualifier(&mut fresh, &stored, &answers(&["tmdb", "fanart"]));
        assert_eq!(fresh.title_qualifier.as_deref(), Some("2023"));

        // Skyhook answered with the name alone: TheTVDB no longer tells the
        // series apart, and neither does Sonarr's title.
        let mut renamed = MediaItem::empty(crate::domain::MediaKind::Series);
        keep_title_qualifier(&mut renamed, &stored, &answers(&["tmdb", "skyhook"]));
        assert_eq!(renamed.title_qualifier, None);

        // TheTVDB answered with another: that one.
        let mut other = MediaItem::empty(crate::domain::MediaKind::Series);
        other.title_qualifier = Some("JP".into());
        keep_title_qualifier(&mut other, &stored, &answers(&["tvdb"]));
        assert_eq!(other.title_qualifier.as_deref(), Some("JP"));
    }

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

#[cfg(test)]
mod card_tests {
    use super::*;
    use crate::domain::{CoverType, Image, MediaKind, Translation};

    fn picture(kind: CoverType, url: &str, manual: bool) -> Image {
        Image {
            id: url.into(),
            season_number: None,
            cover_type: kind,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: manual,
        }
    }

    #[test]
    fn a_card_keeps_one_picture_of_each_kind_it_draws_and_the_hand_picked_first() {
        let mut work = MediaItem::empty(MediaKind::Movie);
        work.images = vec![
            picture(CoverType::Poster, "poster-1", false),
            picture(CoverType::Poster, "poster-2", true),
            picture(CoverType::Fanart, "fanart-1", false),
            picture(CoverType::Fanart, "fanart-2", false),
            picture(CoverType::Banner, "banner", false),
            picture(CoverType::Clearlogo, "logo", false),
        ];
        work.keywords = vec!["cartel".into()];
        work.translations = vec![Translation {
            language: "fra".into(),
            title: Some("Le Parrain".into()),
            overview: None,
            is_manual: false,
        }];

        as_card(&mut work);

        let urls: Vec<&str> = work.images.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(urls, ["poster-2", "fanart-1", "logo"]);
        assert!(work.keywords.is_empty() && work.translations.is_empty());
    }
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    use crate::domain::Episode;

    /// The interval a running series is given by default.
    const INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

    fn at(value: &str) -> DateTime<Utc> {
        crate::db::parse_rfc3339(value).expect("a date-time")
    }

    /// The evening *Magical Explorer* premiered, five hours after its first
    /// episode aired in Japan.
    fn now() -> DateTime<Utc> {
        at("2026-10-03T20:00:00Z")
    }

    fn after(minutes: i64) -> DateTime<Utc> {
        now() + TimeDelta::minutes(minutes)
    }

    fn series(status: &str, first_aired: Option<&str>) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.status = Some(status.into());
        item.first_aired = first_aired.map(Into::into);
        item
    }

    fn episode(number: i32, air_date: Option<&str>, air_date_utc: Option<&str>) -> Episode {
        let mut episode = crate::db::repo::child::blank_episode(1, number);
        episode.air_date = air_date.map(Into::into);
        episode.air_date_utc = air_date_utc.map(Into::into);
        episode
    }

    fn due(item: &MediaItem) -> DateTime<Utc> {
        refresh_at(item, INTERVAL, now())
    }

    #[test]
    fn a_series_far_from_an_air_date_keeps_its_interval() {
        let mut running = series("continuing", Some("2026-09-01"));
        running.episodes = vec![
            episode(1, Some("2026-09-01"), None),
            episode(2, None, Some("2026-10-20T15:00:00Z")),
        ];
        assert_eq!(due(&running), after(6 * 60));

        // Announced for next month, or for no date at all.
        assert_eq!(due(&series("upcoming", Some("2026-11-01"))), after(6 * 60));
        assert_eq!(due(&series("upcoming", None)), after(6 * 60));
    }

    #[test]
    fn a_series_that_has_not_started_is_fetched_every_two_hours_in_the_week_of_its_premiere() {
        // Upcoming, or with no episodes listed yet, whatever its status says.
        assert_eq!(due(&series("upcoming", Some("2026-10-08"))), after(2 * 60));
        assert_eq!(
            due(&series("continuing", Some("2026-10-08"))),
            after(2 * 60)
        );
        // After the premiere too: the episodes are often listed only then.
        assert_eq!(
            due(&series("continuing", Some("2026-09-30"))),
            after(2 * 60)
        );
        assert_eq!(
            due(&series("continuing", Some("2026-09-20"))),
            after(6 * 60)
        );
    }

    #[test]
    fn and_every_hour_in_the_day_of_it_on_either_side() {
        assert_eq!(due(&series("upcoming", Some("2026-10-04"))), after(60));
        assert_eq!(due(&series("continuing", Some("2026-10-03"))), after(60));

        // Its first episode, when that is nearer than the date of the premiere.
        let mut upcoming = series("upcoming", Some("2026-10-08"));
        upcoming.episodes = vec![episode(1, None, Some("2026-10-04T10:00:00Z"))];
        assert_eq!(due(&upcoming), after(60));
    }

    #[test]
    fn magical_explorer_on_the_evening_it_premiered_is_fetched_within_the_hour() {
        // As stored then: TMDB still had it upcoming, its premiere dated by
        // the Japanese day; the first episode had aired five hours before,
        // the second airs in a week.
        let mut item = series("upcoming", Some("2026-10-04"));
        item.episodes = vec![
            episode(1, Some("2026-10-04"), Some("2026-10-03T15:00:00Z")),
            episode(2, Some("2026-10-11"), Some("2026-10-10T15:00:00Z")),
        ];
        assert_eq!(due(&item), after(60));
    }

    #[test]
    fn a_running_series_is_fetched_an_hour_after_its_next_episode_airs() {
        let mut item = series("continuing", Some("2026-01-10"));
        item.episodes = vec![
            episode(1, None, Some("2026-09-26T21:30:00Z")),
            episode(2, None, Some("2026-10-03T22:30:00Z")),
        ];
        assert_eq!(due(&item), after(3 * 60 + 30));

        // Not later than its interval says, though.
        item.episodes[1].air_date_utc = Some("2026-10-04T05:00:00Z".into());
        assert_eq!(due(&item), after(6 * 60));
    }

    #[test]
    fn an_episode_known_only_by_its_day_airs_at_midnight_utc_as_sonarr_is_told() {
        let mut item = series("continuing", Some("2026-01-10"));
        item.episodes = vec![episode(2, Some("2026-10-04"), None)];
        assert_eq!(due(&item), after(5 * 60));
    }

    #[test]
    fn an_episode_that_aired_within_the_hour_is_still_followed() {
        // A refresh between its airing and the hour after it must not move
        // the one after it to the next episode, a week away.
        let mut item = series("continuing", Some("2026-01-10"));
        item.episodes = vec![
            episode(1, None, Some("2026-10-03T19:30:00Z")),
            episode(2, None, Some("2026-10-10T19:30:00Z")),
        ];
        assert_eq!(due(&item), after(30));

        item.episodes[0].air_date_utc = Some("2026-10-03T18:30:00Z".into());
        assert_eq!(due(&item), after(6 * 60));
    }

    #[test]
    fn an_ended_series_and_a_film_keep_their_interval() {
        let week = std::time::Duration::from_secs(7 * 24 * 3600);
        let mut ended = series("ended", Some("2026-10-04"));
        ended.episodes = vec![episode(1, None, Some("2026-10-03T20:30:00Z"))];
        assert_eq!(refresh_at(&ended, week, now()), after(7 * 24 * 60));

        let mut film = MediaItem::empty(MediaKind::Movie);
        film.first_aired = Some("2026-10-04".into());
        assert_eq!(due(&film), after(6 * 60));
    }

    #[test]
    fn dates_that_cannot_be_read_change_nothing() {
        let mut item = series("upcoming", Some("TBA"));
        item.episodes = vec![
            episode(1, Some("soon"), Some("not a date")),
            episode(2, None, None),
            episode(3, Some("9999-12-31"), None),
        ];
        assert_eq!(due(&item), after(6 * 60));
        assert_eq!(due(&series("upcoming", Some("2026"))), after(6 * 60));
    }

    #[test]
    fn a_failed_refresh_waits_six_hours_unless_the_series_is_near_an_air_date() {
        assert_eq!(retry_at(None, now()), after(6 * 60));
        assert_eq!(
            retry_at(Some(&series("continuing", Some("2026-01-10"))), now()),
            after(6 * 60)
        );
        // Not an ended series' week: a failure is not a success.
        assert_eq!(
            retry_at(Some(&series("ended", Some("2020-01-10"))), now()),
            after(6 * 60)
        );
        // A provider down on the day of a premiere does not cost the day.
        assert_eq!(
            retry_at(Some(&series("upcoming", Some("2026-10-04"))), now()),
            after(60)
        );
    }

    fn refreshed(mut item: MediaItem, minutes_ago: i64) -> MediaItem {
        item.refreshed_at = Some(to_rfc3339(after(-minutes_ago)));
        item
    }

    #[test]
    fn a_series_with_no_episodes_is_fetched_again_when_asked_for_once_the_hour_is_past() {
        let empty = series("continuing", Some("2025-04-01"));
        assert!(due_on_read(&refreshed(empty.clone(), 120), now()));
        // Within the hour: what the last fetch, or the last failure, left.
        assert!(!due_on_read(&refreshed(empty, 30), now()));
    }

    #[test]
    fn so_is_a_series_premiering_within_two_days_either_side() {
        let with = |first_aired: &str| {
            let mut item = series("upcoming", Some(first_aired));
            item.episodes = vec![episode(1, Some(first_aired), None)];
            refreshed(item, 120)
        };
        assert!(due_on_read(&with("2026-10-04"), now()));
        assert!(due_on_read(&with("2026-10-02"), now()));
        assert!(!due_on_read(&with("2026-09-30"), now()));
        assert!(!due_on_read(&with("2026-10-06"), now()));
    }

    #[test]
    fn a_film_an_entry_made_by_hand_or_one_never_fetched_is_not() {
        let film = refreshed(MediaItem::empty(MediaKind::Movie), 120);
        assert!(!due_on_read(&film, now()));

        let mut manual = refreshed(series("upcoming", Some("2026-10-04")), 120);
        manual.is_manual = true;
        assert!(!due_on_read(&manual, now()));

        let never = series("upcoming", Some("2026-10-04"));
        assert!(!due_on_read(&never, now()));
    }

    #[test]
    fn a_work_is_served_as_held_until_it_is_due() {
        let mut held = refreshed(series("continuing", Some("2025-04-01")), 120);
        held.episodes = vec![episode(1, Some("2025-04-01"), None)];
        held.refresh_after = Some(to_rfc3339(after(30)));
        assert!(served_as_held(&held, now()));

        // Its refresh has come.
        let mut stale = held.clone();
        stale.refresh_after = Some(to_rfc3339(after(-30)));
        assert!(!served_as_held(&stale, now()));

        // Or the client asking is to be given a fresh copy: no episodes yet.
        let mut empty = held;
        empty.episodes.clear();
        assert!(!served_as_held(&empty, now()));

        // Made by hand: there is nothing to fetch it from.
        stale.is_manual = true;
        assert!(served_as_held(&stale, now()));
    }

    #[test]
    fn a_work_switched_off_is_served_as_held_however_due() {
        // Due by its schedule, and for the client asking: it premieres
        // tomorrow, with no episodes listed, and was fetched two hours ago.
        let mut upcoming = refreshed(series("upcoming", Some("2026-10-04")), 120);
        upcoming.refresh_after = Some(to_rfc3339(after(-30)));
        assert!(!served_as_held(&upcoming, now()));
        upcoming.is_enabled = false;
        assert!(served_as_held(&upcoming, now()));

        let mut film = refreshed(MediaItem::empty(MediaKind::Movie), 120);
        film.refresh_after = Some(to_rfc3339(after(-30)));
        assert!(!served_as_held(&film, now()));
        film.is_enabled = false;
        assert!(served_as_held(&film, now()));
    }
}

/// What the tests of a client's request run against: the whole server, on a
/// database in memory, TMDB and TheTVDB given a key, and every provider at an
/// address that answers nothing and counts what it is asked.
#[cfg(test)]
mod testing {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{config, db::Db, domain::ExternalIds};

    /// Where every provider is: a port that takes each connection, counts it
    /// and hangs up. Nothing answers, and nothing is asked unseen.
    pub struct Nowhere(Arc<AtomicUsize>);

    impl Nowhere {
        /// How many times a provider was asked since the last look.
        pub fn asked(&self) -> usize {
            self.0.swap(0, Ordering::SeqCst)
        }
    }

    /// The server, with its providers at [`Nowhere`].
    pub async fn server() -> (AppState, Nowhere) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let at = format!("http://{}", listener.local_addr().expect("its address"));
        let asked = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&asked);
        tokio::spawn(async move {
            while let Ok((connection, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                drop(connection);
            }
        });

        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        };
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = Some("key".into());
        config.tmdb.language = "en-US".into();
        config.tvdb.api_key = Some("key".into());
        config.tvdb.enabled = true;
        for upstream in [
            &mut config.tmdb.upstream,
            &mut config.tvdb.upstream,
            &mut config.fanart.upstream,
            &mut config.skyhook.upstream,
            &mut config.sonarr_services.upstream,
            &mut config.sonarr_services.xem_upstream,
            &mut config.radarr_metadata.upstream,
            &mut config.tvmaze.upstream,
            &mut config.anilist.upstream,
            &mut config.mal.upstream,
            &mut config.mal.jikan_upstream,
            &mut config.fankai.upstream,
            &mut config.fankai_wiki.upstream,
            &mut config.imdb.datasets,
            &mut config.anime_mapping.url,
        ] {
            upstream.clone_from(&at);
        }

        let state = AppState::bootstrap(config).await.expect("a server");
        (state, Nowhere(asked))
    }

    /// A work fetched a day ago, and due again since an hour ago.
    pub async fn overdue(
        db: &Db,
        kind: MediaKind,
        title: &str,
        external_ids: ExternalIds,
        enabled: bool,
    ) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.id = crate::db::new_id();
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        item.external_ids = external_ids;
        item.is_enabled = enabled;
        item.refreshed_at = Some(to_rfc3339(Utc::now() - TimeDelta::days(1)));
        item.refresh_after = Some(to_rfc3339(Utc::now() - TimeDelta::hours(1)));

        repo::item::upsert(
            db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item
    }
}

#[cfg(test)]
mod switched_off_tests {
    use super::{testing::overdue, *};
    use crate::{config, db::Db, domain::ExternalIds};

    /// A real database, in memory, with the real migrations applied.
    async fn db() -> Db {
        let db = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .expect("in-memory database");

        db.migrate().await.expect("migrations");
        db
    }

    #[tokio::test]
    async fn a_work_switched_off_is_answered_from_the_store_and_never_fetched() {
        let db = db().await;
        let series = overdue(
            &db,
            MediaKind::Series,
            "Attack on Titan",
            ExternalIds {
                tvdb: Some(267440),
                tmdb: Some(1429),
                imdb: Some("tt2560140".into()),
                mal: vec![16498],
                anilist: vec![16498],
                ..Default::default()
            },
            false,
        )
        .await;
        let film = overdue(
            &db,
            MediaKind::Movie,
            "The Matrix",
            ExternalIds {
                tmdb: Some(603),
                imdb: Some("tt0133093".into()),
                ..Default::default()
            },
            false,
        )
        .await;
        let switched_on = overdue(
            &db,
            MediaKind::Series,
            "Breaking Bad",
            ExternalIds {
                tvdb: Some(81189),
                ..Default::default()
            },
            true,
        )
        .await;

        // Found by every id Sonarr and Radarr ask for it by, switched off as
        // it is: the store answers, not a provider.
        for (source, value, work) in [
            (ExternalSource::TvdbSeries, "267440", &series),
            (ExternalSource::TmdbTv, "1429", &series),
            (ExternalSource::Imdb, "tt2560140", &series),
            (ExternalSource::Mal, "16498", &series),
            (ExternalSource::AniList, "16498", &series),
            (ExternalSource::TmdbMovie, "603", &film),
            (ExternalSource::Imdb, "tt0133093", &film),
        ] {
            let found = repo::item::find_id_by_external(&db, source, value)
                .await
                .expect("looked up");
            assert_eq!(
                found.as_deref(),
                Some(work.id.as_str()),
                "{source:?} {value}"
            );
        }

        // And served as it is held, overdue as it is, where a work switched on
        // would be fetched again first: the lookup stops at the store.
        for work in [&series, &film, &switched_on] {
            let held = repo::item::get(&db, &work.id)
                .await
                .expect("read")
                .expect("held");
            assert!(is_stale(&held, Utc::now()), "{}", held.title);
            assert_eq!(
                served_as_held(&held, Utc::now()),
                !held.is_enabled,
                "{}",
                held.title
            );
        }

        // Nor does the sweep fetch it, or a refresh of everything: only the
        // work switched on is due.
        let due = repo::item::due_for_refresh(&db, 10).await.expect("listed");
        assert_eq!(due, [(switched_on.id.clone(), MediaKind::Series)]);
        let everything = repo::item::refresh_candidates(&db, None, 10)
            .await
            .expect("listed");
        assert_eq!(everything, due);
    }
}
