//! Movie resolution, for Radarr's surface and the native API.

use anyhow::Result;
use futures::future::join_all;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::tmdb::map as tmdb_map,
    service::{FETCHING, Found, cached_search, gather, ids, local, or_held, persist},
    state::AppState,
};

/// How many titles a single bulk request may ask for.
///
/// Radarr sends its whole library on a full refresh; without a ceiling that is
/// one upstream call per title in a single request.
const MAX_BULK: usize = 100;

/// A film by its TMDB id: as it is held, or fetched again when it is due —
/// and as it is held after all when nothing answers (see
/// [`crate::service::or_held`]).
pub async fn by_tmdb_id(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    let value = tmdb_id.to_string();
    if let Some(item) = local(state, ExternalSource::TmdbMovie, &value).await? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::ExternalIds,
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
            bulk(&state, &[603]).await.map(ids),
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
        assert_eq!(ids(bulked), [spirited_away.id.as_str()]);
        assert!(
            now_held(&state, &spirited_away)
                .await
                .refresh_error
                .is_some()
        );
        assert!(nowhere.asked() > 0, "the providers were asked");
    }
}
