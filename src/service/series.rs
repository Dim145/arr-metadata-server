//! Series resolution, for Sonarr's surface and the native API.

use anyhow::Result;
use futures::future::join_all;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::{names, tmdb::map as tmdb_map},
    service::{
        FETCHING, Found, cached_search, due_on_read, gather, ids, is_stale, load, persist,
        retry_after_failure,
    },
    state::AppState,
    wire::sonarr,
};

/// Resolve a series by the id Sonarr asked for.
///
/// The id is either a real TVDB id or one this server synthesised for a
/// TMDB-only show or a Fan-Kai production — see [`crate::service::ids`].
///
/// No language: a work is stored once, in the language the server fetches in,
/// and [`crate::service::language`] overlays the one the caller asked for on
/// the way out. Resolution is the same work whoever is asking.
pub async fn by_client_id(state: &AppState, requested_id: i64) -> Result<Option<MediaItem>> {
    if let Some(fankai_id) = ids::from_fankai(requested_id) {
        return by_fankai_id(state, fankai_id).await;
    }

    if let Some(tmdb_id) = ids::from_synthetic(requested_id) {
        return by_tmdb_id(state, tmdb_id).await;
    }

    by_tvdb_id(state, requested_id).await
}

pub async fn by_tvdb_id(state: &AppState, tvdb_id: i64) -> Result<Option<MediaItem>> {
    let value = tvdb_id.to_string();
    if let Some(item) = local(state, ExternalSource::TvdbSeries, &value).await? {
        return Ok(Some(item));
    }

    // One fetch per work at a time; see `FETCHING`. Checked again after the
    // wait, because whoever held it has usually just stored the answer.
    let _fetching = FETCHING.lock(&format!("series:tvdb:{tvdb_id}")).await;
    if let Some(item) = local(state, ExternalSource::TvdbSeries, &value).await? {
        return Ok(Some(item));
    }

    let fetched = fetch_by_tvdb_id(state, tvdb_id).await;
    or_held(state, ExternalSource::TvdbSeries, &value, fetched).await
}

/// Ask the providers for a series by its TVDB id, and store what they say.
async fn fetch_by_tvdb_id(state: &AppState, tvdb_id: i64) -> Result<Option<MediaItem>> {
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
            // Nameless, it would be a row nothing could be listed by; with
            // other providers, one of them names it (`gather::store`).
            Ok(Some((_, show))) if show.title.trim().is_empty() => {
                tracing::warn!(tvdb_id, "Skyhook named nothing; not storing its answer");
            }
            Ok(Some((raw, show))) => {
                let item = sonarr::to_item(&show);
                let provenance = crate::merge::provenance::single(names::SKYHOOK, &item);
                let snapshots = vec![(names::SKYHOOK.to_string(), raw)];
                let stored = persist(state, item, &snapshots, provenance).await?;
                return Ok(Some(stored));
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

    let _fetching = FETCHING.lock(&format!("series:tmdb:{tmdb_id}")).await;
    if let Some(item) = local(state, ExternalSource::TmdbTv, &tmdb_id.to_string()).await? {
        return Ok(Some(item));
    }

    let fetched = fetch_from_tmdb(state, tmdb_id).await;
    or_held(state, ExternalSource::TmdbTv, &tmdb_id.to_string(), fetched).await
}

pub async fn by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    if let Some(item) = local(state, ExternalSource::Imdb, imdb_id).await?
        && item.kind == MediaKind::Series
    {
        return Ok(Some(item));
    }

    let fetched = fetch_by_imdb_id(state, imdb_id).await;
    or_held(state, ExternalSource::Imdb, imdb_id, fetched).await
}

async fn fetch_by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    let found = state.tmdb.find("imdb_id", imdb_id).await?;
    let Some(summary) = found.tv_results.first() else {
        return Ok(None);
    };

    fetch_from_tmdb(state, summary.id).await
}

/// A Fan-Kai production, by Fankai's id for it.
///
/// Fetched from Fankai alone: nothing else lists a recut, and the TheTVDB or
/// TMDB entry nearest to one is the anime it was cut from — another work, with
/// other episodes. Nothing is fetched until a client or the import page asks
/// for it by id, so Fankai's catalogue never lands here on its own.
pub async fn by_fankai_id(state: &AppState, fankai_id: i64) -> Result<Option<MediaItem>> {
    let value = fankai_id.to_string();

    // Off means not asked again, not forgotten: a production fetched while
    // the source was on is served as it was, however old, rather than going
    // missing from Sonarr the day its refresh falls due.
    if !state.flag("fankai.enabled", false) {
        return held(state, ExternalSource::Fankai, &value).await;
    }

    if let Some(item) = local(state, ExternalSource::Fankai, &value).await? {
        return Ok(Some(item));
    }

    let _fetching = FETCHING.lock(&format!("series:fankai:{fankai_id}")).await;
    if let Some(item) = local(state, ExternalSource::Fankai, &value).await? {
        return Ok(Some(item));
    }

    let fetched = gather::fankai_series(state, fankai_id).await;
    or_held(state, ExternalSource::Fankai, &value, fetched).await
}

/// A series by one of its MyAnimeList or AniList entries.
///
/// This is how Sonarr's AniList and MyAnimeList import lists find a series:
/// each entry on the list is searched for as `mal:{id}` or `anilist:{id}`. It
/// used to be answered from the store alone, so a series nobody had added yet
/// — the whole point of an import list — was never found, and neither was one
/// held here but due a refresh. The id Sonarr keeps a series under settles
/// both: the stored series' — its TheTVDB id, or the one made for a work
/// TheTVDB does not list (see [`client_id`]) — or the TheTVDB id the anime
/// identifier list files the entry under.
async fn by_anime_id(
    state: &AppState,
    source: ExternalSource,
    id: i64,
) -> Result<Option<MediaItem>> {
    let value = id.to_string();

    if let Some(item) = local(state, source, &value).await?
        && item.kind == MediaKind::Series
    {
        return Ok(Some(item));
    }

    // Not by its TheTVDB id alone: a series only TMDB lists has none, and the
    // identifier list seldom files a new one. Due a refresh, or fetched again
    // on request for having no episodes yet, it was not found at all — when
    // an import list looks for it most.
    let stored = match repo::item::find_id_by_external(&state.db, source, &value).await? {
        Some(media_id) => load(state, &media_id)
            .await?
            .filter(|item| item.kind == MediaKind::Series)
            .and_then(|item| client_id(&item)),
        None => None,
    };

    let requested_id = match stored {
        Some(requested_id) => Some(requested_id),
        None => crate::service::anime::series_for(state, source, id).await?,
    };

    match requested_id {
        Some(requested_id) => by_client_id(state, requested_id).await,
        None => Ok(None),
    }
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
            return Ok(by_anime_id(state, ExternalSource::Mal, id)
                .await?
                .into_iter()
                .collect());
        }
        ids::TermLookup::AniList(id) => {
            return Ok(by_anime_id(state, ExternalSource::AniList, id)
                .await?
                .into_iter()
                .collect());
        }
        ids::TermLookup::Fankai(id) => {
            return Ok(by_fankai_id(state, id).await?.into_iter().collect());
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
        let mut degraded = false;

        if state.tmdb.is_configured() {
            match tmdb_search(state, term).await {
                Ok(remote) => merge_results(&mut results, remote),
                Err(e) => {
                    tracing::warn!(term, error = %e, "TMDB search failed");
                    degraded = true;
                }
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
                    tracing::warn!(term, error = format_args!("{e:#}"), "TheTVDB search failed");
                    degraded = true;
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
                Err(e) => {
                    tracing::warn!(term, error = %e, "Skyhook search failed");
                    degraded = true;
                }
            }
        }

        // Fankai's productions, after everything else. Not a fallback: a recut
        // is asked for by its name, and its name carries the kind — Kaï, Yabai,
        // Henshū — so it stands beside the anime it was cut from rather than
        // in its place, whether or not the others answered.
        if state.flag("fankai.enabled", false) {
            match state.fankai.search(term, state.search_limit()).await {
                Ok(hits) => merge_results(&mut results, hits),
                Err(e) => {
                    tracing::warn!(term, error = format_args!("{e:#}"), "Fankai search failed");
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

// ─── helpers ─────────────────────────────────────────────────────────────────

/// A locally stored series, if it exists and will do as it is.
///
/// Not when it is due: its refresh has come, or a client asking for it now is
/// to be given a fresh copy (see [`due_on_read`]). Returning `None` then lets
/// the ladder continue to a refetch, and [`or_held`] answers with this copy
/// after all when that comes to nothing.
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
    let due = is_stale(&item) || due_on_read(&item, chrono::Utc::now());
    if item.is_manual || !due {
        return Ok(Some(item));
    }

    Ok(None)
}

/// What a fetch came to, or the stored copy when it came to nothing.
///
/// A series held here and due again is fetched before it is served; when no
/// provider answers, or the fetch fails, the copy held is served as it is —
/// answering when the providers do not is what keeping one is for — rather
/// than a 404 Sonarr would act on. The attempt is recorded as a failed
/// refresh, so the requests that follow are answered from the store at once
/// instead of each waiting on the same providers to fail again: until the
/// retry the failure sets ([`retry_after_failure`]), and for a series near
/// its premiere, for the hour [`due_on_read`] leaves between two attempts.
async fn or_held(
    state: &AppState,
    source: ExternalSource,
    value: &str,
    fetched: Result<Option<MediaItem>>,
) -> Result<Option<MediaItem>> {
    let failure = match &fetched {
        Ok(Some(_)) => return fetched,
        Ok(None) => "no provider answered; the stored entry was kept".to_string(),
        Err(e) => format!("{e:#}"),
    };

    let item = match held(state, source, value).await {
        Ok(Some(item)) if item.kind == MediaKind::Series => item,
        _ => return fetched,
    };

    // Refreshed meanwhile — by the sweep, say: the copy is as fresh as any.
    if !is_stale(&item) && !due_on_read(&item, chrono::Utc::now()) {
        return Ok(Some(item));
    }

    tracing::warn!(
        id = %item.id,
        error = %failure,
        "the series could not be fetched again; answering with the stored copy"
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

/// A locally stored series whatever its age: for a source that is switched
/// off, what it fetched while on is served as it is; and it is what
/// [`or_held`] answers with when fetching it again came to nothing.
async fn held(state: &AppState, source: ExternalSource, value: &str) -> Result<Option<MediaItem>> {
    let Some(id) = repo::item::find_id_by_external(&state.db, source, value).await? else {
        return Ok(None);
    };

    Ok(load(state, &id).await?.filter(|item| item.is_enabled))
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

/// What one source lists under `term`, and no other: the search a person runs
/// to find the entry they know is there when the usual order would have
/// stopped at another. Not cached — it is asked for by hand, rarely.
pub async fn search_at(state: &AppState, source: &str, term: &str) -> Result<Vec<MediaItem>> {
    let term = term.trim();
    let limit = state.search_limit();

    match source {
        names::TMDB => tmdb_search(state, term).await,
        names::TVDB => state.tvdb.search(term, limit).await,
        names::SKYHOOK => Ok(state
            .skyhook
            .search(term)
            .await?
            .iter()
            .take(limit)
            .map(sonarr::to_item)
            .collect()),
        names::FANKAI => state.fankai.search(term, limit).await,
        _ => Ok(Vec::new()),
    }
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
                || same_id(existing.external_ids.fankai, candidate.external_ids.fankai)
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
/// A real TVDB id when there is one; otherwise a synthetic id derived from
/// TMDB's, or from Fankai's for a Fan-Kai. Returns `None` for a work the client
/// has no way to address.
pub fn client_id(item: &MediaItem) -> Option<i64> {
    let ids = &item.external_ids;
    client_id_of(ids.tvdb, ids.tmdb, ids.fankai)
}

/// The title Sonarr is given for a series: its own — a locked one exactly as
/// it was locked — with what tells it from a homonym, as Skyhook names it.
///
/// TheTVDB's qualifier when it has one, *Rurouni Kenshin (2023)*; for a work
/// TheTVDB has no entry for, its year when another series of the catalogue is
/// given the same title (`homonym_year`). Two series Sonarr knows by one
/// title are what its title lookup throws on, so a release by that name is
/// dropped rather than matched.
pub fn sonarr_title(
    title: &str,
    locked: bool,
    qualifier: Option<&str>,
    homonym_year: Option<i32>,
) -> String {
    if locked {
        return title.to_string();
    }
    let added = match (qualifier, homonym_year) {
        (Some(qualifier), _) => qualifier.to_string(),
        (None, Some(year)) if year > 0 => year.to_string(),
        _ => return title.to_string(),
    };
    if title.trim_end().ends_with(&format!("({added})")) {
        title.to_string()
    } else {
        format!("{} ({added})", title.trim_end())
    }
}

/// [`client_id`], from the three ids it is chosen among.
pub fn client_id_of(tvdb: Option<i64>, tmdb: Option<i64>, fankai: Option<i64>) -> Option<i64> {
    tvdb.or_else(|| tmdb.and_then(ids::to_synthetic))
        .or_else(|| fankai.and_then(ids::to_fankai))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonarr_is_given_the_title_as_skyhook_tells_homonyms_apart() {
        assert_eq!(
            sonarr_title("Rurouni Kenshin", false, Some("2023"), None),
            "Rurouni Kenshin (2023)"
        );
        assert_eq!(
            sonarr_title("Rurouni Kenshin", false, None, None),
            "Rurouni Kenshin"
        );
        assert_eq!(
            sonarr_title("The Office", false, Some("US"), None),
            "The Office (US)"
        );
        // Said already: not twice.
        assert_eq!(
            sonarr_title("Rurouni Kenshin (2023)", false, Some("2023"), None),
            "Rurouni Kenshin (2023)"
        );
        // A locked title goes exactly as it was locked.
        assert_eq!(
            sonarr_title("Rurouni Kenshin", true, Some("2023"), Some(2023)),
            "Rurouni Kenshin"
        );
        // No entry on TheTVDB, and a homonym here: its year.
        assert_eq!(
            sonarr_title("Rurouni Kenshin", false, None, Some(2010)),
            "Rurouni Kenshin (2010)"
        );
        assert_eq!(
            sonarr_title("Rurouni Kenshin", false, None, Some(0)),
            "Rurouni Kenshin"
        );
    }
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
    fn a_fan_kai_is_addressed_by_fankai_s_id() {
        let mut item = series(None, None);
        item.external_ids.fankai = Some(12);
        assert_eq!(client_id(&item), Some(200_000_012));
    }

    #[test]
    fn merging_knows_a_fan_kai_already_listed() {
        let mut held = series(None, None);
        held.external_ids.fankai = Some(12);
        let mut results = vec![held];

        let mut again = series(None, None);
        again.external_ids.fankai = Some(12);
        let mut other = series(None, None);
        other.external_ids.fankai = Some(13);
        merge_results(&mut results, vec![again, other]);

        assert_eq!(results.len(), 2);
        assert_eq!(results[1].external_ids.fankai, Some(13));
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
