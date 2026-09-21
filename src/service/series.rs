//! Series resolution, for Sonarr's surface and the native API.

use anyhow::Result;
use futures::future::join_all;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::{names, tmdb::map as tmdb_map},
    service::{cached_search, gather, ids, is_stale, load, persist},
    state::AppState,
    wire::sonarr,
};

/// Resolve a series by the id Sonarr asked for.
///
/// The id is either a real TVDB id or one this server synthesised for a
/// TMDB-only show — see [`crate::service::ids`].
///
/// No language: a work is stored once, in the language the server fetches in,
/// and [`crate::service::language`] overlays the one the caller asked for on
/// the way out. Resolution is the same work whoever is asking.
pub async fn by_client_id(state: &AppState, requested_id: i64) -> Result<Option<MediaItem>> {
    if let Some(tmdb_id) = ids::from_synthetic(requested_id) {
        return by_tmdb_id(state, tmdb_id).await;
    }

    by_tvdb_id(state, requested_id).await
}

pub async fn by_tvdb_id(state: &AppState, tvdb_id: i64) -> Result<Option<MediaItem>> {
    if let Some(item) = local(state, ExternalSource::TvdbSeries, &tvdb_id.to_string()).await? {
        return Ok(Some(item));
    }

    // TMDB indexes by its own ids, so ask it which work this TVDB id is before
    // gathering: knowing both lets every provider be asked at once.
    let tmdb_id = if state.tmdb.is_configured() {
        match state.tmdb.find("tvdb_id", &tvdb_id.to_string()).await {
            Ok(found) => found.tv_results.first().map(|r| r.id),
            Err(e) => {
                tracing::warn!(tvdb_id, error = %e, "TMDB lookup failed");
                None
            }
        }
    } else {
        None
    };

    if let Some(item) = gather::series(state, tmdb_id, Some(tvdb_id)).await? {
        return Ok(Some(item));
    }

    // Nothing had it. Skyhook is asked again here only when enrichment is off;
    // otherwise `gather` already tried it, and asking twice would be the same
    // answer at twice the cost.
    if state.flag("skyhook.fallback", true) && !state.flag("skyhook.enrich", true) {
        match state.skyhook.show(tvdb_id).await {
            Ok(Some((raw, show))) => {
                let item = sonarr::to_item(&show);
                let snapshots = vec![(names::SKYHOOK.to_string(), raw)];
                return Ok(Some(persist(state, item, &snapshots).await?));
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(tvdb_id, error = %e, "Skyhook fallback failed"),
        }
    }

    Ok(None)
}

pub async fn by_tmdb_id(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    if let Some(item) = local(state, ExternalSource::TmdbTv, &tmdb_id.to_string()).await? {
        return Ok(Some(item));
    }

    fetch_from_tmdb(state, tmdb_id).await
}

pub async fn by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    if let Some(item) = local(state, ExternalSource::Imdb, imdb_id).await?
        && item.kind == MediaKind::Series
    {
        return Ok(Some(item));
    }

    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    let found = state.tmdb.find("imdb_id", imdb_id).await?;
    let Some(summary) = found.tv_results.first() else {
        return Ok(None);
    };

    fetch_from_tmdb(state, summary.id).await
}

/// Search, in the order a client expects results to appear.
pub async fn search(state: &AppState, term: &str) -> Result<Vec<MediaItem>> {
    // A prefixed term is a lookup, not a search: answer with the one match.
    match ids::classify(term) {
        ids::TermLookup::Tvdb(id) => {
            return Ok(by_tvdb_id(state, id).await?.into_iter().collect());
        }
        ids::TermLookup::Tmdb(id) => return Ok(by_tmdb_id(state, id).await?.into_iter().collect()),
        ids::TermLookup::Imdb(id) => {
            return Ok(by_imdb_id(state, &id).await?.into_iter().collect());
        }
        ids::TermLookup::Mal(id) => {
            return Ok(local(state, ExternalSource::Mal, &id.to_string())
                .await?
                .into_iter()
                .collect());
        }
        ids::TermLookup::AniList(id) => {
            return Ok(local(state, ExternalSource::AniList, &id.to_string())
                .await?
                .into_iter()
                .collect());
        }
        ids::TermLookup::Text(_) => {}
    }

    let term = term.trim();
    if term.is_empty() {
        return Ok(Vec::new());
    }

    // The server's language, not the caller's: these results are whatever the
    // providers were asked in, and the caller's language is applied to them
    // afterwards. Keying on what was asked for would split the cache in two
    // over entries holding the same thing.
    let key = crate::service::search_key(
        "series",
        &state.language(None, None),
        state.adult_visible(),
        "",
        term,
    );

    cached_search(state, key, || async {
        let mut results = local_search(state, term).await?;

        if state.tmdb.is_configured() {
            match tmdb_search(state, term).await {
                Ok(remote) => merge_results(&mut results, remote),
                Err(e) => tracing::warn!(term, error = %e, "TMDB search failed"),
            }
        }

        // Fallbacks, in order of authority and only while nothing has been
        // found. TheTVDB knows series TMDB has never heard of — it is the one
        // that settles their numbering, after all — and Skyhook republishes its
        // catalogue without needing a key, so it answers last for a deployment
        // that has no TheTVDB key at all.
        if results.is_empty() && state.flag("tvdb.searchFallback", true) {
            match state.tvdb.search(term, state.search_limit()).await {
                Ok(hits) => merge_results(&mut results, hits),
                Err(e) => {
                    tracing::warn!(term, error = format_args!("{e:#}"), "TheTVDB search failed")
                }
            }
        }

        if results.is_empty() && state.flag("skyhook.fallback", true) {
            match state.skyhook.search(term).await {
                Ok(shows) => {
                    for show in shows.iter().take(state.search_limit()) {
                        results.push(sonarr::to_item(show));
                    }
                }
                Err(e) => tracing::warn!(term, error = %e, "Skyhook search failed"),
            }
        }

        Ok(results)
    })
    .await
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// A locally stored series, if it exists and is still fresh.
///
/// A stale entry is still returned when the provider cannot be reached; the
/// caller decides. Here, returning `None` lets the ladder continue to a refetch.
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

    // A manual entry has no provider behind it, so staleness is meaningless.
    if item.is_manual || !is_stale(&item) {
        return Ok(Some(item));
    }

    Ok(None)
}

async fn local_search(state: &AppState, term: &str) -> Result<Vec<MediaItem>> {
    let query = repo::item::Query {
        term: Some(term.to_string()),
        kind: Some(MediaKind::Series),
        limit: state.search_limit() as i64,
        ..Default::default()
    };

    let mut items = repo::item::search(&state.db, &query).await?;

    // Search results are shallow; a client needs at least the artwork.
    for item in &mut items {
        repo::item::load_children(&state.db, item).await?;
    }

    crate::service::apply_overrides(state, &mut items).await?;

    Ok(items)
}

/// Fetch a series known only by its TMDB id.
///
/// Its TVDB id comes from TMDB's own external ids, so Skyhook can be asked too.
async fn fetch_from_tmdb(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    let tvdb_id = match state.tmdb.tv_external_ids(tmdb_id).await {
        Ok(ids) => ids.tvdb_id,
        Err(e) => {
            tracing::warn!(tmdb_id, error = %e, "could not resolve the TVDB id");
            None
        }
    };

    gather::series(state, Some(tmdb_id), tvdb_id).await
}

/// Map TMDB search hits, resolving each one's TVDB id so Sonarr can address it.
async fn tmdb_search(state: &AppState, term: &str) -> Result<Vec<MediaItem>> {
    let limit = state.search_limit();
    let hits = state.tmdb.search_tv(term, limit).await?;

    if hits.is_empty() {
        return Ok(Vec::new());
    }

    // One external-ids call per hit, concurrently. Without the TVDB id a result
    // is useless to Sonarr, so this cannot be deferred to the detail request.
    let external = join_all(hits.iter().map(|h| state.tmdb.tv_external_ids(h.id))).await;

    Ok(hits
        .iter()
        .zip(external)
        .map(|(hit, ids)| {
            let mut item = tmdb_map::tv_summary_to_item(hit);
            if let Ok(ids) = ids {
                item.external_ids.tvdb = ids.tvdb_id;
                item.external_ids.imdb = ids
                    .imdb_id
                    .as_deref()
                    .and_then(crate::domain::ids::normalize_imdb_id);
                item.external_ids.tvrage = ids.tvrage_id;
            }
            item
        })
        .collect())
}

/// Append `incoming` to `into`, skipping works already present.
fn merge_results(into: &mut Vec<MediaItem>, incoming: Vec<MediaItem>) {
    for candidate in incoming {
        let already_present = into.iter().any(|existing| {
            same_id(existing.external_ids.tmdb, candidate.external_ids.tmdb)
                || same_id(existing.external_ids.tvdb, candidate.external_ids.tvdb)
        });

        if !already_present {
            into.push(candidate);
        }
    }
}

fn same_id(a: Option<i64>, b: Option<i64>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a == b)
}

/// The id a client should be handed for this series.
///
/// A real TVDB id when there is one; otherwise a synthetic id derived from TMDB.
/// Returns `None` for a work the client has no way to address.
pub fn client_id(item: &MediaItem) -> Option<i64> {
    item.external_ids
        .tvdb
        .or_else(|| item.external_ids.tmdb.and_then(ids::to_synthetic))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ExternalIds;

    fn series(tmdb: Option<i64>, tvdb: Option<i64>) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.external_ids = ExternalIds {
            tmdb,
            tvdb,
            ..Default::default()
        };
        item
    }

    #[test]
    fn a_real_tvdb_id_is_preferred_over_a_synthetic_one() {
        assert_eq!(client_id(&series(Some(1396), Some(81189))), Some(81189));
    }

    #[test]
    fn a_tmdb_only_series_gets_a_synthetic_id() {
        assert_eq!(client_id(&series(Some(1396), None)), Some(100_001_396));
    }

    #[test]
    fn an_unaddressable_series_yields_nothing() {
        assert_eq!(client_id(&series(None, None)), None);
    }

    #[test]
    fn merging_skips_works_already_in_the_list() {
        let mut results = vec![series(Some(1), Some(10))];

        merge_results(
            &mut results,
            vec![
                series(Some(1), None),  // same TMDB id
                series(None, Some(10)), // same TVDB id
                series(Some(2), Some(20)),
            ],
        );

        assert_eq!(results.len(), 2);
        assert_eq!(results[1].external_ids.tmdb, Some(2));
    }

    #[test]
    fn two_works_with_no_ids_are_not_treated_as_duplicates() {
        let mut results = vec![series(None, None)];
        merge_results(&mut results, vec![series(None, None)]);
        assert_eq!(results.len(), 2);
    }
}
