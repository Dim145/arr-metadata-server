//! Movie resolution, for Radarr's surface and the native API.

use anyhow::Result;
use futures::future::join_all;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::tmdb::map as tmdb_map,
    service::{cached_search, gather, ids, is_stale, load, persist},
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
                Err(e) => tracing::warn!(term, error = %e, "TMDB movie search failed"),
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
                Err(e) => tracing::warn!(term, error = %e, "Radarr metadata search failed"),
            }
        }

        Ok(results)
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
        Ok(hits.iter().map(tmdb_map::movie_summary_to_item).collect())
    })
    .await
}

pub async fn trending(state: &AppState) -> Result<Vec<MediaItem>> {
    if !state.tmdb.is_configured() {
        return Ok(Vec::new());
    }

    cached_search(state, "list:trending".to_string(), || async {
        let hits = state.tmdb.trending_movies().await?;
        Ok(hits.iter().map(tmdb_map::movie_summary_to_item).collect())
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

async fn local(state: &AppState, source: ExternalSource, value: &str) -> Result<Option<MediaItem>> {
    let Some(id) = repo::item::find_id_by_external(&state.db, source, value).await? else {
        return Ok(None);
    };

    let Some(item) = load(state, &id).await? else {
        return Ok(None);
    };

    if !item.is_enabled {
        return Ok(None);
    }

    if item.is_manual || !is_stale(&item) {
        return Ok(Some(item));
    }

    Ok(None)
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
            let snapshots = vec![(crate::providers::names::RADARR.to_string(), raw)];

            Ok(Some(persist(state, item, &snapshots).await?))
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
