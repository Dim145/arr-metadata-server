//! Movie resolution, for Radarr's surface and the native API.

use anyhow::Result;
use futures::future::join_all;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::{names, tmdb::map as tmdb_map},
    service::{cached_search, ids, is_stale, load, persist},
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

    if let Some(item) = local(state, ExternalSource::Imdb, &normalized).await? {
        if item.kind == MediaKind::Movie {
            return Ok(Some(item));
        }
    }

    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    let found = state.tmdb.find("imdb_id", &normalized).await?;
    let Some(summary) = found.movie_results.first() else {
        return Ok(None);
    };

    fetch_from_tmdb(state, summary.id).await
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
        ids::TermLookup::Imdb(id) => return Ok(by_imdb_id(state, &id).await?.into_iter().collect()),
        ids::TermLookup::Text(_) => {}
        // A movie has no TVDB, MAL or AniList identity worth resolving here.
        _ => {}
    }

    let term = term.trim();
    if term.is_empty() {
        return Ok(Vec::new());
    }

    let key = format!(
        "movie:{}:{}",
        year.map(|y| y.to_string()).unwrap_or_default(),
        term.to_lowercase()
    );

    cached_search(state, key, || async {
        let mut results = local_search(state, term, year).await?;

        if state.tmdb.is_configured() {
            match state.tmdb.search_movie(term, year, state.config.tmdb.search_limit).await {
                Ok(hits) => {
                    for hit in &hits {
                        let candidate = tmdb_map::movie_summary_to_item(hit);
                        let known = results
                            .iter()
                            .any(|r| r.external_ids.tmdb == candidate.external_ids.tmdb);
                        if !known {
                            results.push(candidate);
                        }
                    }
                }
                Err(e) => tracing::warn!(term, error = %e, "TMDB movie search failed"),
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
pub async fn collection(state: &AppState, tmdb_id: i64) -> Result<Option<(String, Option<String>, Vec<MediaItem>)>> {
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
        limit: state.config.tmdb.search_limit as i64,
        ..Default::default()
    };

    let mut items = repo::item::search(&state.db, &query).await?;

    for item in &mut items {
        repo::item::load_children(&state.db, item).await?;
    }

    Ok(items)
}

async fn fetch_from_tmdb(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    let Some((raw, movie)) = state.tmdb.movie(tmdb_id).await? else {
        return Ok(None);
    };

    let item = tmdb_map::movie_to_item(&movie);

    Ok(Some(persist(state, item, names::TMDB, Some(&raw)).await?))
}
