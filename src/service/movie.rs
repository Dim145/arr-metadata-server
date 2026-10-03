//! Movie resolution, for Radarr's surface and the native API.

use anyhow::Result;
use futures::future::join_all;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::tmdb::map as tmdb_map,
    service::{FETCHING, Found, cached_search, gather, ids, load, persist, served_as_held},
    state::AppState,
};

/// How many titles a single bulk request may ask for.
///
/// Radarr sends its whole library on a full refresh; without a ceiling that is
/// one upstream call per title in a single request.
const MAX_BULK: usize = 100;

pub async fn by_tmdb_id(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    if let Some(item) = local(state, ExternalSource::TmdbMovie, &tmdb_id.to_string()).await? {
        return Ok(Some(item));
    }

    // One fetch per work at a time; see `FETCHING`. Radarr's bulk refresh asks
    // for a hundred at once, and a request page opening asks for the same film
    // it just listed.
    let _fetching = FETCHING.lock(&format!("movie:tmdb:{tmdb_id}")).await;
    if let Some(item) = local(state, ExternalSource::TmdbMovie, &tmdb_id.to_string()).await? {
        return Ok(Some(item));
    }

    fetch_from_tmdb(state, tmdb_id).await
}

pub async fn by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    let Some(normalized) = crate::domain::ids::normalize_imdb_id(imdb_id) else {
        return Ok(None);
    };

    if let Some(item) = local(state, ExternalSource::Imdb, &normalized).await?
        && item.kind == MediaKind::Movie
    {
        return Ok(Some(item));
    }

    let _fetching = FETCHING.lock(&format!("movie:imdb:{normalized}")).await;
    if let Some(item) = local(state, ExternalSource::Imdb, &normalized).await?
        && item.kind == MediaKind::Movie
    {
        return Ok(Some(item));
    }

    let tmdb_id = match state.tmdb.is_configured() {
        true => state
            .tmdb
            .find("imdb_id", &normalized)
            .await?
            .movie_results
            .first()
            .map(|r| r.id),
        false => None,
    };

    if let Some(item) = gather::movie(state, tmdb_id, Some(&normalized)).await? {
        return Ok(Some(item));
    }

    from_radarr(state, tmdb_id, Some(&normalized)).await
}

/// Several movies at once, in the order requested.
///
/// A title that cannot be resolved is skipped rather than failing the batch:
/// Radarr expects a partial answer, not an error, when one entry has gone away.
pub async fn bulk(state: &AppState, tmdb_ids: &[i64]) -> Result<Vec<MediaItem>> {
    let wanted: Vec<i64> = tmdb_ids.iter().copied().take(MAX_BULK).collect();

    let resolved = join_all(wanted.iter().map(|&id| by_tmdb_id(state, id))).await;

    Ok(resolved
        .into_iter()
        .zip(&wanted)
        .filter_map(|(result, id)| match result {
            Ok(item) => item,
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

/// A collection and the movies in it.
///
/// The parts come back as search-level detail: fetching each in full would be
/// dozens of upstream calls for a list Radarr only uses to offer suggestions.
pub async fn collection(
    state: &AppState,
    tmdb_id: i64,
) -> Result<Option<(String, Option<String>, Vec<MediaItem>)>> {
    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    let Some(collection) = state.tmdb.collection(tmdb_id).await? else {
        return Ok(None);
    };

    let parts = collection
        .parts
        .iter()
        .map(tmdb_map::movie_summary_to_item)
        .collect();

    Ok(Some((collection.name, collection.overview, parts)))
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

/// TMDB ids changed since `since`, which Radarr polls to know what to refetch.
pub async fn changed_since(state: &AppState, since: &str) -> Result<Vec<i64>> {
    if !state.tmdb.is_configured() {
        return Ok(Vec::new());
    }

    // TMDB wants a bare date; Radarr sends a full timestamp.
    let date = since.split('T').next().unwrap_or(since);

    state.tmdb.changed_ids(MediaKind::Movie, date).await
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

/// A locally stored film, if it exists and will do as it is: see
/// [`served_as_held`]. One switched off is served as it is held, and no
/// provider is asked for it.
async fn local(state: &AppState, source: ExternalSource, value: &str) -> Result<Option<MediaItem>> {
    let Some(id) = repo::item::find_id_by_external(&state.db, source, value).await? else {
        return Ok(None);
    };

    let Some(item) = load(state, &id).await? else {
        return Ok(None);
    };

    Ok(served_as_held(&item, chrono::Utc::now()).then_some(item))
}

async fn local_search(state: &AppState, term: &str, year: Option<i32>) -> Result<Vec<MediaItem>> {
    let query = repo::item::Query {
        term: Some(term.to_string()),
        kind: Some(MediaKind::Movie),
        year,
        limit: state.search_limit() as i64,
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
            let snapshots = vec![(crate::providers::names::RADARR.to_string(), raw)];

            let stored = persist(state, item, &snapshots, provenance).await?;
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
