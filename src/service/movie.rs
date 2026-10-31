//! Movie resolution, for Radarr's surface and the native API.

use anyhow::{Context, Result};
use bytes::Bytes;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use futures::{StreamExt as _, future::join_all};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::{names, tmdb, tmdb::map as tmdb_map},
    service::{FETCHING, Found, cached_search, gather, ids, local, merge, or_held, persist},
    state::AppState,
    wire::radarr::{Beside, NamedCollection},
};

/// How many titles a single bulk request may ask for.
///
/// Radarr sends its whole library on a full refresh; without a ceiling that is
/// one upstream call per title in a single request.
const MAX_BULK: usize = 100;

/// A film by its TMDB id: as it is held, or fetched again when it is due —
/// and as it is held after all when nothing answers (see
/// [`crate::service::or_held`]).
///
/// Or by the id Radarr was handed in its stead for a film entered by hand
/// that TMDB does not name — see [`ids::own`]: the catalogue's alone, so
/// TMDB is not asked for it.
pub async fn by_tmdb_id(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    if ids::is_own(tmdb_id) {
        return crate::service::by_own_id(state, MediaKind::Movie, tmdb_id).await;
    }
    let value = tmdb_id.to_string();
    if let Some(item) = local(state, ExternalSource::TmdbMovie, &value).await? {
        return Ok(Some(item));
    }
    // An id a merge folded into another film is that film's, as it is held:
    // fetched again by it, it would be stored as a film anew.
    if let Some(item) =
        merge::aliased(state, MediaKind::Movie, ExternalSource::TmdbMovie, &value).await?
    {
        return Ok(Some(item));
    }

    // One fetch per work at a time; see `FETCHING`. Radarr's bulk refresh asks
    // for a hundred at once, and a request page opening asks for the same film
    // it just listed.
    let _fetching = FETCHING.lock(&format!("movie:tmdb:{tmdb_id}")).await;
    if let Some(item) = local(state, ExternalSource::TmdbMovie, &value).await? {
        return Ok(Some(item));
    }

    let fetched = fetch_from_tmdb(state, tmdb_id).await;
    or_held(
        state,
        MediaKind::Movie,
        ExternalSource::TmdbMovie,
        &value,
        fetched,
    )
    .await
}

/// A film by its IMDb id, as [`by_tmdb_id`] has it by TMDB's.
pub async fn by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    let Some(normalized) = crate::domain::ids::normalize_imdb_id(imdb_id) else {
        return Ok(None);
    };

    if let Some(item) = local(state, ExternalSource::Imdb, &normalized).await?
        && item.kind == MediaKind::Movie
    {
        return Ok(Some(item));
    }
    // One a merge folded into another film: see `by_tmdb_id`.
    if let Some(item) =
        merge::aliased(state, MediaKind::Movie, ExternalSource::Imdb, &normalized).await?
    {
        return Ok(Some(item));
    }

    let _fetching = FETCHING.lock(&format!("movie:imdb:{normalized}")).await;
    if let Some(item) = local(state, ExternalSource::Imdb, &normalized).await?
        && item.kind == MediaKind::Movie
    {
        return Ok(Some(item));
    }

    let fetched = fetch_by_imdb_id(state, &normalized).await;
    or_held(
        state,
        MediaKind::Movie,
        ExternalSource::Imdb,
        &normalized,
        fetched,
    )
    .await
}

/// Ask the providers for a film by its IMDb id, and store what they say.
async fn fetch_by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    let tmdb_id = match state.tmdb.is_configured() {
        true => state
            .tmdb
            .find("imdb_id", imdb_id)
            .await?
            .movie_results
            .first()
            .map(|r| r.id),
        false => None,
    };

    if let Some(item) = gather::movie(state, tmdb_id, Some(imdb_id)).await? {
        return Ok(Some(item));
    }

    from_radarr(state, tmdb_id, Some(imdb_id)).await
}

/// Several movies at once, in the order requested, each with the id it was
/// asked by: a film a merge folded into another is the survivor, and Radarr
/// matches each answer to the film it holds by that id.
///
/// A title that cannot be resolved is skipped rather than failing the batch:
/// Radarr expects a partial answer, not an error, when one entry has gone away.
pub async fn bulk(state: &AppState, tmdb_ids: &[i64]) -> Result<Vec<(i64, MediaItem)>> {
    let wanted: Vec<i64> = tmdb_ids.iter().copied().take(MAX_BULK).collect();

    let resolved = join_all(wanted.iter().map(|&id| by_tmdb_id(state, id))).await;

    Ok(resolved
        .into_iter()
        .zip(&wanted)
        .filter_map(|(result, id)| match result {
            Ok(item) => item.map(|item| (*id, item)),
            Err(e) => {
                tracing::warn!(tmdb_id = id, error = %e, "bulk lookup failed for one title");
                None
            }
        })
        .collect())
}

/// What one source lists under `term`, and no other. See
/// [`super::series::search_at`].
pub async fn search_at(
    state: &AppState,
    source: &str,
    term: &str,
    year: Option<i32>,
) -> Result<Vec<MediaItem>> {
    let term = term.trim();
    let limit = state.search_limit();

    match source {
        crate::providers::names::TMDB => Ok(state
            .tmdb
            .search_movie(term, year, limit)
            .await?
            .iter()
            .map(tmdb_map::movie_summary_to_item)
            .collect()),
        crate::providers::names::RADARR => Ok(state
            .radarr_metadata
            .search(term, year)
            .await?
            .iter()
            .take(limit)
            .map(crate::wire::radarr::to_item)
            .collect()),
        _ => Ok(Vec::new()),
    }
}

pub async fn search(state: &AppState, term: &str, year: Option<i32>) -> Result<Vec<MediaItem>> {
    match ids::classify(term) {
        ids::TermLookup::Tmdb(id) => return Ok(by_tmdb_id(state, id).await?.into_iter().collect()),
        ids::TermLookup::Imdb(id) => {
            return Ok(by_imdb_id(state, &id).await?.into_iter().collect());
        }
        ids::TermLookup::Text(_) => {}
        // A movie has no TVDB, MAL or AniList identity worth resolving here.
        _ => {}
    }

    let term = term.trim();
    if term.is_empty() {
        return Ok(Vec::new());
    }

    let key = crate::service::search_key(
        "movie",
        &state.language(None, None),
        state.adult_visible(),
        &year.map(|y| y.to_string()).unwrap_or_default(),
        term,
    );

    cached_search(state, key, || async {
        let mut results = local_search(state, term, year).await?;
        let mut degraded = false;

        if state.tmdb.is_configured() {
            match state
                .tmdb
                .search_movie(term, year, state.search_limit())
                .await
            {
                Ok(hits) => {
                    for hit in &hits {
                        add_unseen(&mut results, tmdb_map::movie_summary_to_item(hit));
                    }
                }
                Err(e) => {
                    tracing::warn!(term, error = %e, "TMDB movie search failed");
                    degraded = true;
                }
            }
        }

        // Radarr's own search finds titles TMDB's ranking buries, so it is worth
        // asking even when TMDB answered.
        if state.flag("radarr.enrich", true) {
            match state.radarr_metadata.search(term, year).await {
                Ok(hits) => {
                    for hit in hits.iter().take(state.search_limit()) {
                        add_unseen(&mut results, crate::wire::radarr::to_item(hit));
                    }
                }
                Err(e) => {
                    tracing::warn!(term, error = %e, "Radarr metadata search failed");
                    degraded = true;
                }
            }
        }

        Ok(Found {
            items: results,
            degraded,
        })
    })
    .await
}

/// A collection and the movies in it, as Radarr is told of it.
pub struct Collection {
    pub name: String,
    pub overview: Option<String>,
    /// Its pictures' addresses, where TMDB has them.
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub parts: Vec<MediaItem>,
}

/// A collection and the movies in it, in `language`, a TMDB locale.
///
/// The parts come back as search-level detail: fetching each in full would be
/// dozens of upstream calls for a list Radarr only uses to offer suggestions.
/// TMDB's record is kept a day, under the key the catalogue's own collection
/// pages keep it under: either one asking warms it for the other, and for the
/// films of the collection that Radarr is sent.
pub async fn collection(
    state: &AppState,
    tmdb_id: i64,
    language: &str,
) -> Result<Option<Collection>> {
    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    let key = collection_key(tmdb_id, language);
    let raw = match cached(state, &key).await {
        Some(raw) => raw,
        None => {
            // Kept as `null` where TMDB has no such collection, as the
            // catalogue's pages keep it.
            let raw = state
                .tmdb
                .collection_raw(tmdb_id, language)
                .await?
                .unwrap_or(Value::Null);
            state
                .caches
                .lists
                .insert(key, Bytes::from(serde_json::to_vec(&raw)?))
                .await;
            raw
        }
    };
    if raw.is_null() {
        return Ok(None);
    }

    let collection: tmdb::models::Collection =
        serde_json::from_value(raw).context("could not interpret the TMDB collection response")?;

    Ok(Some(Collection {
        name: collection.name,
        overview: collection.overview.filter(|o| !o.trim().is_empty()),
        poster: collection.poster_path.as_deref().map(tmdb::image_url),
        backdrop: collection.backdrop_path.as_deref().map(tmdb::image_url),
        parts: collection
            .parts
            .iter()
            .map(tmdb_map::movie_summary_to_item)
            .collect(),
    }))
}

/// The key TMDB's record of a collection is kept under in the lists' cache,
/// in a TMDB locale: one copy for the catalogue's collection pages and for
/// Radarr.
pub fn collection_key(tmdb_id: i64, language: &str) -> String {
    format!("collection:{tmdb_id}:{language}")
}

/// The key what TMDB recommends beside a work is kept under in the lists'
/// cache, once the editor has asked for it.
pub fn suggestions_key(kind: MediaKind, tmdb_id: i64) -> String {
    format!("suggest:{}:{tmdb_id}", kind.as_str())
}

/// A document kept in the lists' cache, where it is.
async fn cached(state: &AppState, key: &str) -> Option<Value> {
    let bytes = state.caches.lists.get(key).await?;
    serde_json::from_slice(&bytes).ok()
}

// ─── what goes beside a film ─────────────────────────────────────────────────

/// How many films' snapshots are read at once, where what goes beside them is
/// not known yet: Radarr refreshes a list a hundred films at a time.
const BESIDE_AT_ONCE: usize = 8;

/// What goes beside each of `items` as Radarr is sent them, in their order —
/// see [`Beside`] — from what is held: the providers' answers for the film as
/// they were kept, and what TMDB told the catalogue's own pages since. Never
/// fetched: Radarr refreshes its library through these routes, and what a
/// film suggests is not worth a call to TMDB per film. `language` is the
/// caller's, as the settings have it.
pub async fn beside(state: &AppState, items: &[MediaItem], language: &str) -> Vec<Beside> {
    let locale = crate::service::language::tmdb_locale(None, language);

    // Made before they are run, rather than by a closure the stream keeps:
    // the handlers awaiting this did not compile around one, its `FnOnce`
    // not general enough for the lifetimes it borrows.
    let each: Vec<_> = items
        .iter()
        .map(|item| beside_one(state, item, &locale))
        .collect();

    futures::stream::iter(each)
        .buffered(BESIDE_AT_ONCE)
        .collect()
        .await
}

async fn beside_one(state: &AppState, item: &MediaItem, locale: &str) -> Beside {
    let Some(tmdb_id) = item.external_ids.tmdb else {
        return Beside::default();
    };
    let mut beside = kept_beside(state, item).await;

    // TMDB's own record of the collection, in the caller's language, where
    // the catalogue's pages or Radarr asked for it lately: the snapshot's is
    // in the language the works are stored in, and as old as the film's.
    if let Some(collection) = item.collection_tmdb_id
        && let Some(named) = cached(state, &collection_key(collection, locale))
            .await
            .and_then(|record| named_collection(collection, &record))
    {
        beside.collection = Some(named);
    }

    // A film whose snapshots recommend nothing — taken before TMDB's
    // recommendations came with the film, Radarr's service not asked — is
    // sent what the editor had TMDB suggest for it.
    if beside.recommended.is_empty()
        && let Some(suggested) = cached(state, &suggestions_key(MediaKind::Movie, tmdb_id)).await
    {
        beside.recommended = tmdb_recommended(&suggested);
    }

    beside
}

/// What a film's own snapshots say goes beside it, kept a day under the work
/// and the moment it was last written: a refresh writes new snapshots, and
/// moves that moment.
async fn kept_beside(state: &AppState, item: &MediaItem) -> Beside {
    let key = format!("radarr:beside:{}:{}", item.id, item.updated_at);
    if let Some(bytes) = state.caches.lists.get(&key).await
        && let Ok(kept) = serde_json::from_slice::<Beside>(&bytes)
    {
        return kept;
    }

    let texts =
        match repo::snapshot::texts(&state.db, &item.id, &[names::TMDB, names::RADARR]).await {
            Ok(texts) => texts,
            Err(e) => {
                // What a film suggests is not worth failing it over.
                tracing::warn!(
                    id = %item.id,
                    error = format_args!("{e:#}"),
                    "could not read what goes beside a film"
                );
                return Beside::default();
            }
        };

    let beside = from_snapshots(item.collection_tmdb_id, &texts);
    if let Ok(bytes) = serde_json::to_vec(&beside) {
        state.caches.lists.insert(key, Bytes::from(bytes)).await;
    }
    beside
}

/// The little of TMDB's record of a film that goes beside it. The rest of the
/// document is passed over, not read.
#[derive(Deserialize)]
struct TmdbBeside {
    belongs_to_collection: Option<Value>,
    recommendations: Option<Value>,
}

/// The same of api.radarr.video's.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RadarrBeside {
    collection: Option<Value>,
    recommendations: Option<Value>,
}

/// What a film's snapshots, by provider, say goes beside it: TMDB's, and
/// Radarr's service's — which republishes TMDB's — for what TMDB's does not
/// say. A collection only where it is `collection`, the one the film is
/// filed under.
fn from_snapshots(collection: Option<i64>, texts: &[(String, String)]) -> Beside {
    let text = |provider: &str| {
        texts
            .iter()
            .find(|(p, _)| p == provider)
            .map(|(_, text)| text.as_str())
    };
    let tmdb = text(names::TMDB).and_then(|t| serde_json::from_str::<TmdbBeside>(t).ok());
    let radarr = text(names::RADARR).and_then(|t| serde_json::from_str::<RadarrBeside>(t).ok());

    let collection = collection.and_then(|wanted| {
        let from_tmdb = tmdb
            .as_ref()
            .and_then(|t| t.belongs_to_collection.as_ref())
            .and_then(|c| named_collection(wanted, c));
        from_tmdb.or_else(|| {
            radarr
                .as_ref()
                .and_then(|r| r.collection.as_ref())
                .and_then(|c| radarr_collection(wanted, c))
        })
    });

    let recommended = tmdb
        .as_ref()
        .and_then(|t| t.recommendations.as_ref())
        .map(tmdb_recommended)
        .filter(|found| !found.is_empty())
        .or_else(|| {
            radarr
                .as_ref()
                .and_then(|r| r.recommendations.as_ref())
                .map(radarr_recommended)
        })
        .unwrap_or_default();

    Beside {
        collection,
        recommended,
    }
}

/// A collection as TMDB writes one — whole, or as a film's
/// `belongs_to_collection` — when it is `wanted`, and named.
fn named_collection(wanted: i64, record: &Value) -> Option<NamedCollection> {
    if record.get("id").and_then(Value::as_i64) != Some(wanted) {
        return None;
    }
    let text = |field: &str| {
        record
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
    };

    Some(NamedCollection {
        tmdb_id: wanted,
        name: text("name")?.to_string(),
        poster: text("poster_path").map(tmdb::image_url),
        fanart: text("backdrop_path").map(tmdb::image_url),
    })
}

/// api.radarr.video's `Collection`, when it is `wanted`: an id and a name.
fn radarr_collection(wanted: i64, record: &Value) -> Option<NamedCollection> {
    if record.get("TmdbId").and_then(Value::as_i64) != Some(wanted) {
        return None;
    }
    let name = record
        .get("Name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())?;

    Some(NamedCollection {
        tmdb_id: wanted,
        name: name.to_string(),
        poster: None,
        fanart: None,
    })
}

/// TMDB's recommendations — a page of them, as a film's record carries them,
/// or its results alone, as the editor's suggestions are kept — by id and
/// title. Those TMDB marks adult are left out: whoever is sent them may not
/// be allowed adult titles.
fn tmdb_recommended(found: &Value) -> Vec<(i64, String)> {
    let results = found.get("results").unwrap_or(found);
    results
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| !entry.get("adult").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|entry| {
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some((entry.get("id")?.as_i64()?, title.to_string()))
        })
        .collect()
}

/// api.radarr.video's `Recommendations`, by `TmdbId` and `Title`.
fn radarr_recommended(found: &Value) -> Vec<(i64, String)> {
    found
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let title = entry
                .get("Title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some((entry.get("TmdbId")?.as_i64()?, title.to_string()))
        })
        .collect()
}

pub async fn popular(state: &AppState) -> Result<Vec<MediaItem>> {
    if !state.tmdb.is_configured() {
        return Ok(Vec::new());
    }

    cached_search(state, "list:popular".to_string(), || async {
        let hits = state.tmdb.popular_movies(1).await?;
        Ok(Found::complete(
            hits.iter().map(tmdb_map::movie_summary_to_item).collect(),
        ))
    })
    .await
}

pub async fn trending(state: &AppState) -> Result<Vec<MediaItem>> {
    if !state.tmdb.is_configured() {
        return Ok(Vec::new());
    }

    cached_search(state, "list:trending".to_string(), || async {
        let hits = state.tmdb.trending_movies().await?;
        Ok(Found::complete(
            hits.iter().map(tmdb_map::movie_summary_to_item).collect(),
        ))
    })
    .await
}

/// TMDB ids changed since `since`, which Radarr polls to know what to refetch:
/// those TMDB lists, and the films held here that changed since — fetched
/// again, a lock put on, changed or lifted, IMDb's rating moved.
///
/// Radarr's scheduled refresh fetches again what this names and, once it
/// names anything, only that: a film locked here, unchanged at TMDB, waited
/// for a refresh by hand. Without a TMDB key, or with TMDB not answering, the
/// films held here are named all the same.
pub async fn changed_since(state: &AppState, since: DateTime<Utc>) -> Result<Vec<i64>> {
    let mut listed = Vec::new();
    if state.tmdb.is_configured() {
        // TMDB wants a bare date.
        let date = since.date_naive().format("%Y-%m-%d").to_string();
        match state.tmdb.changed_ids(MediaKind::Movie, &date).await {
            Ok(ids) => listed = ids,
            Err(e) => tracing::warn!(
                error = format_args!("{e:#}"),
                "TMDB's changed films could not be listed; those changed here are named alone"
            ),
        }
    }

    let since = crate::db::to_rfc3339(since);
    let mut held = repo::item::movies_changed_since(&state.db, &since).await?;
    // A film entered by hand under the id Radarr was handed for it, whether
    // or not it has a TMDB id since.
    held.extend(
        repo::item::films_by_hand_changed_since(&state.db, &since)
            .await?
            .iter()
            .map(|id| ids::own(id)),
    );

    Ok(merged(listed, held))
}

/// The id Radarr is handed for a film: its TMDB id, or for a film entered by
/// hand that TMDB does not name, one made of the catalogue's own id
/// ([`ids::own`]). `None` for a film Radarr has no way to address.
pub fn client_id(item: &MediaItem) -> Option<i64> {
    ids::for_client(
        MediaKind::Movie,
        &item.id,
        item.is_manual,
        &item.external_ids,
    )
}

/// The id Radarr is handed for each of `items`, as [`client_id`] gives it,
/// in their order — a film entered by hand only under a number that leads
/// back to it: see [`crate::service::addressed`].
pub async fn client_ids(state: &AppState, items: &[MediaItem]) -> Result<Vec<Option<i64>>> {
    crate::service::addressed(state, MediaKind::Movie, items, client_id).await
}

/// The instant `since` names: as Radarr writes it — `2026-10-09T11:45:00`,
/// in UTC without saying so — with an offset, or a bare date, from midnight
/// UTC.
pub fn instant(since: &str) -> Option<DateTime<Utc>> {
    let since = since.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(since) {
        return Some(at.with_timezone(&Utc));
    }
    if let Ok(at) = NaiveDateTime::parse_from_str(since, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(at.and_utc());
    }
    NaiveDate::parse_from_str(since, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|at| at.and_utc())
}

/// Every id of `first`, then those of `then` not in it, each once.
fn merged(first: Vec<i64>, then: Vec<i64>) -> Vec<i64> {
    let mut seen = std::collections::HashSet::new();
    first
        .into_iter()
        .chain(then)
        .filter(|id| seen.insert(*id))
        .collect()
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Append a result unless the same work is already listed.
///
/// A work with no TMDB id cannot be compared this way, so it is kept: two
/// unidentifiable results are more likely two works than one duplicate.
fn add_unseen(results: &mut Vec<MediaItem>, candidate: MediaItem) {
    let duplicate = match candidate.external_ids.tmdb {
        Some(id) => results.iter().any(|r| r.external_ids.tmdb == Some(id)),
        None => false,
    };

    if !duplicate {
        results.push(candidate);
    }
}

async fn local_search(state: &AppState, term: &str, year: Option<i32>) -> Result<Vec<MediaItem>> {
    // Ranked by how well each answers the term, so that the film held here by
    // that very name comes first in Radarr's list, before TMDB's.
    let query = repo::item::Query {
        term: Some(term.to_string()),
        kind: Some(MediaKind::Movie),
        year,
        sort: repo::item::Sort::Relevance,
        // By its letters alone: TMDB's search, which follows, forgives a
        // misspelling, and a film held here that only resembles the title
        // would come first in the list Radarr picks from.
        tolerance: Some(Vec::new()),
        limit: state.search_limit() as i64,
        // The works for adults the server shows, as for a series (see
        // `series::local_search`): each surface keeps what its caller may see.
        include_adult: state.adult_visible(),
        ..Default::default()
    };

    let mut items = repo::item::search(&state.db, &query).await?;

    for item in &mut items {
        repo::item::load_children(&state.db, item).await?;
    }

    crate::service::apply_overrides(state, &mut items).await?;

    Ok(items)
}

async fn fetch_from_tmdb(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    if let Some(item) = gather::movie(state, Some(tmdb_id), None).await? {
        return Ok(Some(item));
    }

    from_radarr(state, Some(tmdb_id), None).await
}

/// Radarr's own service, asked alone when nothing else could answer.
///
/// The mirror of the Skyhook fallback for series, and the thing
/// `radarr.fallback` actually switches. It only fires when enrichment is off:
/// with enrichment on, `gather` has already asked and a second call would buy
/// the same answer twice.
async fn from_radarr(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Result<Option<MediaItem>> {
    if !state.flag("radarr.fallback", true) || state.flag("radarr.enrich", true) {
        return Ok(None);
    }

    let found = match (tmdb_id, imdb_id) {
        (Some(id), _) => state.radarr_metadata.movie(id).await,
        (_, Some(id)) => state.radarr_metadata.by_imdb_id(id).await,
        _ => return Ok(None),
    };

    match found {
        Ok(Some((raw, movie))) => {
            let item = crate::wire::radarr::to_item(&movie);
            let provenance =
                crate::merge::provenance::single(crate::providers::names::RADARR, &item);
            let snapshots = vec![(crate::providers::names::RADARR.to_string(), raw.to_string())];

            let stored = persist(state, item, &snapshots, provenance, None).await?;
            Ok(Some(stored))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            tracing::warn!(
                ?tmdb_id,
                error = format_args!("{e:#}"),
                "Radarr fallback failed"
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{ExternalIds, fields::Scope},
        service::{
            served_as_held,
            testing::{overdue, server},
        },
    };

    /// A film fetched a day ago, and due again since an hour ago.
    async fn due(state: &AppState, title: &str, tmdb: i64, imdb: &str) -> MediaItem {
        let ids = ExternalIds {
            tmdb: Some(tmdb),
            imdb: Some(imdb.into()),
            ..Default::default()
        };
        overdue(&state.db, MediaKind::Movie, title, ids, true).await
    }

    /// The film as the store holds it now.
    async fn now_held(state: &AppState, film: &MediaItem) -> MediaItem {
        repo::item::get(&state.db, &film.id)
            .await
            .expect("read")
            .expect("held")
    }

    /// What was found, by id.
    fn ids(films: impl IntoIterator<Item = MediaItem>) -> Vec<String> {
        films.into_iter().map(|film| film.id).collect()
    }

    #[tokio::test]
    async fn a_film_due_again_that_no_provider_answers_for_is_served_as_held() {
        let (state, nowhere) = server().await;
        let matrix = due(&state, "The Matrix", 603, "tt0133093").await;

        // Due: the providers are asked again, none answers, and Radarr is
        // given the copy held rather than a 404.
        let found = by_tmdb_id(&state, 603).await.expect("answered");
        assert_eq!(ids(found), [matrix.id.as_str()]);
        assert!(nowhere.asked() > 0, "the providers were asked");

        // As a failed refresh, tried again later, as a series' is...
        let held = now_held(&state, &matrix).await;
        assert_eq!(
            held.refresh_error.as_deref(),
            Some("no provider answered; the stored entry was kept")
        );
        assert!(served_as_held(&held, chrono::Utc::now()));

        // ...so what Radarr asks next is answered from the store at once.
        for found in [
            by_tmdb_id(&state, 603).await.map(ids),
            by_imdb_id(&state, "tt0133093").await.map(ids),
            search(&state, "tmdb:603", None).await.map(ids),
            bulk(&state, &[603])
                .await
                .map(|found| ids(found.into_iter().map(|(_, film)| film))),
        ] {
            assert_eq!(found.expect("answered"), [matrix.id.as_str()]);
        }
        assert_eq!(nowhere.asked(), 0, "no provider was asked again");
    }

    #[tokio::test]
    async fn so_is_one_asked_for_by_its_imdb_id_or_in_bulk() {
        let (state, nowhere) = server().await;
        let matrix = due(&state, "The Matrix", 603, "tt0133093").await;
        let spirited_away = due(&state, "Spirited Away", 129, "tt0245429").await;

        // By its IMDb id, TMDB is first asked which film it is: that failing
        // is no 404 either.
        let found = by_imdb_id(&state, "tt0133093").await.expect("answered");
        assert_eq!(ids(found), [matrix.id.as_str()]);
        let failure = now_held(&state, &matrix).await.refresh_error;
        assert!(
            failure
                .as_deref()
                .is_some_and(|failure| failure.starts_with("TMDB request failed")),
            "{failure:?}"
        );

        // Radarr's bulk request keeps it, and leaves out a film nobody holds.
        let bulked = bulk(&state, &[129, 1]).await.expect("answered");
        assert_eq!(
            bulked.iter().map(|(asked, _)| *asked).collect::<Vec<_>>(),
            [129]
        );
        assert_eq!(
            ids(bulked.into_iter().map(|(_, film)| film)),
            [spirited_away.id.as_str()]
        );
        assert!(
            now_held(&state, &spirited_away)
                .await
                .refresh_error
                .is_some()
        );
        assert!(nowhere.asked() > 0, "the providers were asked");
    }

    // ─── what goes beside a film ─────────────────────────────────────────────

    /// The Matrix, as TMDB answers for it: its collection, and a page of
    /// recommendations, one of them marked adult.
    fn the_matrix_at_tmdb() -> Value {
        serde_json::json!({
            "id": 603,
            "title": "The Matrix",
            "belongs_to_collection": {
                "id": 2344,
                "name": "The Matrix Collection",
                "poster_path": "/bV9qTVHTVf0gkW0j7p7M0ILD4pG.jpg",
                "backdrop_path": null
            },
            "credits": { "cast": [{ "name": "Keanu Reeves" }] },
            "recommendations": {
                "page": 1,
                "results": [
                    { "id": 604, "title": "The Matrix Reloaded", "adult": false },
                    { "id": 1, "title": "Not for everyone", "adult": true },
                    { "id": 605, "title": "The Matrix Revolutions" }
                ],
                "total_pages": 2
            }
        })
    }

    /// The Matrix, as api.radarr.video answers for it.
    fn the_matrix_at_radarr() -> Value {
        serde_json::json!({
            "TmdbId": 603,
            "Title": "The Matrix",
            "Collection": {
                "TmdbId": 2344,
                "Name": "The Matrix Collection",
                "Images": null,
                "Parts": null
            },
            "Recommendations": [{ "TmdbId": 99861, "Title": "Avengers: Age of Ultron" }]
        })
    }

    fn texts(answers: &[(&str, Value)]) -> Vec<(String, String)> {
        answers
            .iter()
            .map(|(provider, answer)| (provider.to_string(), answer.to_string()))
            .collect()
    }

    fn recommended(beside: &Beside) -> Vec<i64> {
        beside.recommended.iter().map(|(id, _)| *id).collect()
    }

    /// A film held, filed under `collection`, with what its providers said.
    async fn held_with(
        state: &AppState,
        tmdb: i64,
        collection: Option<i64>,
        answers: &[(&str, Value)],
    ) -> MediaItem {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.title = format!("Film {tmdb}");
        film.slug = format!("film-{tmdb}");
        film.external_ids = ExternalIds {
            tmdb: Some(tmdb),
            ..Default::default()
        };
        film.collection_tmdb_id = collection;
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &film,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        for (provider, answer) in answers {
            repo::snapshot::put(&state.db, &film.id, provider, &answer.to_string(), None)
                .await
                .expect("kept");
        }
        film
    }

    #[test]
    fn what_goes_beside_a_film_is_read_from_its_providers_answers() {
        let both = texts(&[
            ("tmdb", the_matrix_at_tmdb()),
            ("radarr", the_matrix_at_radarr()),
        ]);

        let found = from_snapshots(Some(2344), &both);
        let collection = found.collection.clone().expect("its collection");
        assert_eq!(
            (collection.tmdb_id, collection.name.as_str()),
            (2344, "The Matrix Collection")
        );
        assert!(
            collection
                .poster
                .is_some_and(|p| p.ends_with("/bV9qTVHTVf0gkW0j7p7M0ILD4pG.jpg"))
        );
        // TMDB's, without the one it marks adult.
        assert_eq!(recommended(&found), [604, 605]);

        // Kept before TMDB's recommendations came with the film: Radarr's
        // service's, which republishes them.
        let mut older = the_matrix_at_tmdb();
        older
            .as_object_mut()
            .expect("an object")
            .remove("recommendations");
        let found = from_snapshots(
            Some(2344),
            &texts(&[("tmdb", older), ("radarr", the_matrix_at_radarr())]),
        );
        assert_eq!(recommended(&found), [99861]);
        assert!(found.collection.is_some());

        // Radarr's service names the collection where TMDB was not asked.
        let found = from_snapshots(Some(2344), &texts(&[("radarr", the_matrix_at_radarr())]));
        assert_eq!(
            found.collection.map(|c| c.name).as_deref(),
            Some("The Matrix Collection")
        );

        // A film filed under no collection, or another, is given none of these.
        assert!(from_snapshots(None, &both).collection.is_none());
        assert!(from_snapshots(Some(10), &both).collection.is_none());
        // Nothing kept, or nothing that reads: nothing beside.
        assert_eq!(from_snapshots(Some(2344), &[]), Beside::default());
        assert_eq!(
            from_snapshots(Some(2344), &[("tmdb".into(), "{".into())]),
            Beside::default()
        );
    }

    #[tokio::test]
    async fn what_goes_beside_a_film_is_read_from_the_store_and_never_fetched() {
        let (state, nowhere) = server().await;
        let matrix = held_with(&state, 603, Some(2344), &[("tmdb", the_matrix_at_tmdb())]).await;
        let lone = held_with(&state, 129, None, &[]).await;

        let found = beside(&state, &[matrix.clone(), lone.clone()], "en-US").await;
        assert_eq!(found[0].collection.as_ref().map(|c| c.tmdb_id), Some(2344));
        assert_eq!(recommended(&found[0]), [604, 605]);
        assert_eq!(found[1], Beside::default());

        // TMDB's own record of the collection, kept by the catalogue's pages
        // in the caller's language, names it in that language.
        let record =
            serde_json::json!({ "id": 2344, "name": "Matrix - Saga", "poster_path": "/fr.jpg" });
        state
            .caches
            .lists
            .insert(
                collection_key(2344, "fr-FR"),
                Bytes::from(serde_json::to_vec(&record).expect("written")),
            )
            .await;
        let in_french = beside(&state, std::slice::from_ref(&matrix), "fr-FR").await;
        assert_eq!(
            in_french[0].collection.as_ref().map(|c| c.name.as_str()),
            Some("Matrix - Saga")
        );

        // A film whose snapshots recommend nothing is sent what the editor
        // had TMDB suggest for it.
        let suggested = serde_json::json!([{ "id": 128, "title": "Princess Mononoke" }]);
        state
            .caches
            .lists
            .insert(
                suggestions_key(MediaKind::Movie, 129),
                Bytes::from(serde_json::to_vec(&suggested).expect("written")),
            )
            .await;
        let found = beside(&state, &[lone], "en-US").await;
        assert_eq!(recommended(&found[0]), [128]);

        assert_eq!(nowhere.asked(), 0, "no provider was asked");
    }

    // ─── what changed ────────────────────────────────────────────────────────

    #[test]
    fn since_is_read_as_radarr_writes_it() {
        let at = |since: &str| instant(since).map(crate::db::to_rfc3339);

        // Rounded to the hour, a quarter before, without its zone.
        assert_eq!(
            at("2026-10-09T11:45:00").as_deref(),
            Some("2026-10-09T11:45:00.000Z")
        );
        assert_eq!(
            at("2026-10-09T13:45:00.250+02:00").as_deref(),
            Some("2026-10-09T11:45:00.250Z")
        );
        assert_eq!(
            at("2026-10-09").as_deref(),
            Some("2026-10-09T00:00:00.000Z")
        );
        assert_eq!(at("yesterday"), None);
        assert_eq!(at("2026-10-09T25:00:00"), None);
    }

    #[test]
    fn what_tmdb_lists_comes_first_and_each_once() {
        assert_eq!(
            merged(vec![603, 129, 603], vec![129, 329865]),
            [603, 129, 329865]
        );
        assert_eq!(merged(Vec::new(), vec![329865]), [329865]);
    }

    #[tokio::test]
    async fn the_films_changed_here_are_named_beside_those_tmdb_lists() {
        // TMDB has a key here, and does not answer: what changed here is
        // named all the same.
        let (state, nowhere) = server().await;
        let since = Utc::now() - chrono::TimeDelta::hours(1);
        let long_ago = crate::db::to_rfc3339(Utc::now() - chrono::TimeDelta::days(2));

        // Fetched again a moment ago.
        due(&state, "The Matrix", 603, "tt0133093").await;
        let untouched = due(&state, "Spirited Away", 129, "tt0245429").await;
        let locked = due(&state, "Arrival", 329865, "tt2543164").await;
        let unlocked = due(&state, "Dune", 438631, "tt1160419").await;
        for film in [&untouched, &locked, &unlocked] {
            sqlx::query(
                state
                    .db
                    .sql("UPDATE media_item SET updated_at = ? WHERE id = ?"),
            )
            .bind(&long_ago)
            .bind(&film.id)
            .execute(state.db.pool())
            .await
            .expect("written two days ago");
        }

        // A lock put on since moves neither `updated_at` nor `refreshed_at`.
        let title = |text: &str| Some(serde_json::json!(text));
        repo::override_field::set(
            &state.db,
            &locked.id,
            Scope::Item,
            "title",
            title("Premier Contact").as_ref(),
            Some("test"),
        )
        .await
        .expect("locked");

        // One put on two days ago and lifted since leaves no lock behind: the
        // work is listed again.
        repo::override_field::set(
            &state.db,
            &unlocked.id,
            Scope::Item,
            "title",
            title("Dune : première partie").as_ref(),
            Some("test"),
        )
        .await
        .expect("locked");
        sqlx::query(
            state
                .db
                .sql("UPDATE media_override SET updated_at = ? WHERE media_id = ?"),
        )
        .bind(&long_ago)
        .bind(&unlocked.id)
        .execute(state.db.pool())
        .await
        .expect("locked two days ago");
        repo::override_field::unset(&state.db, &unlocked.id, Scope::Item, "title")
            .await
            .expect("lifted");
        crate::service::listing::after_write(&state, &unlocked.id).await;

        let mut changed = changed_since(&state, since).await.expect("answered");
        changed.sort_unstable();
        assert_eq!(changed, [603, 329865, 438631]);
        assert!(nowhere.asked() > 0, "TMDB was asked");

        // Nothing changed after now.
        let later = changed_since(&state, Utc::now() + chrono::TimeDelta::minutes(1))
            .await
            .expect("answered");
        assert!(later.is_empty(), "{later:?}");
    }
}
