//! Series resolution, for Sonarr's surface and the native API.

use anyhow::{Context, Result};
use futures::future::join_all;

use crate::{
    db::repo,
    domain::{ExternalSource, MediaItem, MediaKind},
    providers::{self, names, skyhook, tmdb::map as tmdb_map},
    service::{FETCHING, Found, cached_search, gather, held, ids, load, local, or_held, persist},
    state::AppState,
};

/// The sources that can resolve a series by an id a client asks by — as
/// against those that only add to one, `gather`'s supplements. A fetch that
/// found nothing while one of these did not answer found nothing *yet*.
const RESOLVING: &[&str] = &[names::TMDB, names::TVDB, names::SKYHOOK, names::FANKAI];

/// Resolve a series by the id Sonarr asked for.
///
/// The id is either a real TVDB id or one this server synthesised for a
/// TMDB-only show or a Fan-Kai production — see [`crate::service::ids`].
///
/// No language: a work is stored once, in the language the server fetches in,
/// and [`crate::service::language`] overlays the one the caller asked for on
/// the way out. Resolution is the same work whoever is asking.
///
/// `None` only when every source that could resolve the id answered that
/// there is no such series; an error when nothing is held and a source did
/// not answer, which a client is told as a 502 (see [`resolved`]).
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
    or_held(
        state,
        MediaKind::Series,
        ExternalSource::TvdbSeries,
        &value,
        fetched,
    )
    .await
}

/// A series fetched again by its TVDB id, whatever is held: for a refresh,
/// which the copy held does not answer. `None` when nothing answered.
pub async fn refetch_by_tvdb_id(state: &AppState, tvdb_id: i64) -> Result<Option<MediaItem>> {
    let _fetching = FETCHING.lock(&format!("series:tvdb:{tvdb_id}")).await;
    fetch_by_tvdb_id(state, tvdb_id).await
}

/// What a client's fetch came to: what the sources said — or, when nothing
/// was found while a source that could have resolved the work did not
/// answer, the error that says so, which the client is answered 502 with.
///
/// The rule: a series the catalogue holds is answered from it whatever the
/// sources say ([`or_held`]); an id nobody holds is not found only when every
/// source that could resolve it answered that there is no such series; a
/// source that did not answer — a timeout, a 5xx, a refused connection — is
/// never a "not found". Sonarr takes a series for deleted on a 404 and keeps
/// what it has on anything else, so the doubt goes to the 502, which is asked
/// again. The failures themselves are folded away by `gather`, where a source
/// being down costs detail rather than the answer; what is read here is what
/// the providers' doors noted over the fetch ([`providers::noting`]).
async fn resolved<F>(fetch: F) -> Result<Option<MediaItem>>
where
    F: std::future::Future<Output = Result<Option<MediaItem>>>,
{
    let (fetched, unanswered) = providers::noting(fetch).await;
    let unanswered: Vec<&'static str> = unanswered
        .into_iter()
        .filter(|provider| RESOLVING.contains(provider))
        .collect();
    match fetched {
        Ok(None) if !unanswered.is_empty() => Err(providers::Unanswered(unanswered).into()),
        Err(e) if !unanswered.is_empty() => Err(e.context(providers::Unanswered(unanswered))),
        other => other,
    }
}

/// Ask the providers for a series by its TVDB id, and store what they say.
async fn fetch_by_tvdb_id(state: &AppState, tvdb_id: i64) -> Result<Option<MediaItem>> {
    resolved(ask_by_tvdb_id(state, tvdb_id))
        .await
        .with_context(|| format!("tvdb:{tvdb_id} could not be resolved"))
}

async fn ask_by_tvdb_id(state: &AppState, tvdb_id: i64) -> Result<Option<MediaItem>> {
    // TMDB indexes by its own ids, so ask it which work this TVDB id is before
    // gathering: knowing both lets every provider be asked at once.
    let tmdb_id = if state.tmdb.is_configured() {
        match state.tmdb.find("tvdb_id", &tvdb_id.to_string()).await {
            Ok(found) => found.tv_results.first().map(|r| r.id),
            Err(e) => {
                tracing::warn!(tvdb_id, error = %e, "TMDB lookup failed");
                // Noted here besides at its door: a key TMDB refuses is no
                // word about the series either.
                providers::went_unanswered(names::TMDB);
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
            Ok(Some((_, show))) if show.title().trim().is_empty() => {
                tracing::warn!(tvdb_id, "Skyhook named nothing; not storing its answer");
            }
            Ok(Some((raw, show))) => {
                let item = skyhook::to_item(&show);
                let provenance = crate::merge::provenance::single(names::SKYHOOK, &item);
                let snapshots = vec![(names::SKYHOOK.to_string(), raw)];
                let stored = persist(state, item, &snapshots, provenance, None).await?;
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
    or_held(
        state,
        MediaKind::Series,
        ExternalSource::TmdbTv,
        &tmdb_id.to_string(),
        fetched,
    )
    .await
}

pub async fn by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    if let Some(item) = local(state, ExternalSource::Imdb, imdb_id).await?
        && item.kind == MediaKind::Series
    {
        return Ok(Some(item));
    }

    // One fetch per work at a time, as by the other ids; see `FETCHING`.
    let _fetching = FETCHING.lock(&format!("series:imdb:{imdb_id}")).await;
    if let Some(item) = local(state, ExternalSource::Imdb, imdb_id).await?
        && item.kind == MediaKind::Series
    {
        return Ok(Some(item));
    }

    let fetched = fetch_by_imdb_id(state, imdb_id).await;
    or_held(
        state,
        MediaKind::Series,
        ExternalSource::Imdb,
        imdb_id,
        fetched,
    )
    .await
}

async fn fetch_by_imdb_id(state: &AppState, imdb_id: &str) -> Result<Option<MediaItem>> {
    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    resolved(async {
        let found = match state.tmdb.find("imdb_id", imdb_id).await {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(imdb_id, error = %e, "TMDB lookup failed");
                providers::went_unanswered(names::TMDB);
                return Ok(None);
            }
        };
        let Some(summary) = found.tv_results.first() else {
            return Ok(None);
        };

        ask_from_tmdb(state, summary.id).await
    })
    .await
    .with_context(|| format!("imdb:{imdb_id} could not be resolved"))
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

    let fetched = resolved(gather::fankai_series(state, fankai_id))
        .await
        .with_context(|| format!("fankai:{fankai_id} could not be resolved"));
    or_held(
        state,
        MediaKind::Series,
        ExternalSource::Fankai,
        &value,
        fetched,
    )
    .await
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

    // The sources that did not answer: an answer they would have added to is
    // not kept, and one nothing else gave is not given as "no results".
    let mut unanswered: Vec<&'static str> = Vec::new();
    let items = cached_search(state, key, || async {
        let mut results = local_search(state, term).await?;

        if state.tmdb.is_configured() {
            match tmdb_search(state, term).await {
                Ok(remote) => merge_results(&mut results, remote),
                Err(e) => {
                    tracing::warn!(term, error = %e, "TMDB search failed");
                    unanswered.push(names::TMDB);
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
                    unanswered.push(names::TVDB);
                }
            }
        }

        if results.is_empty() && state.flag("skyhook.fallback", true) {
            match state.skyhook.search(term).await {
                Ok(shows) => {
                    for show in shows.iter().take(state.search_limit()) {
                        results.push(skyhook::to_item(show));
                    }
                }
                Err(e) => {
                    tracing::warn!(term, error = %e, "Skyhook search failed");
                    unanswered.push(names::SKYHOOK);
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
                    unanswered.push(names::FANKAI);
                }
            }
        }

        Ok(Found {
            items: results,
            degraded: !unanswered.is_empty(),
        })
    })
    .await?;

    // Nothing found, and a source that could have found it did not answer:
    // a 502, which a client asks again on, rather than the empty list Sonarr
    // reads as "no such series". What the catalogue holds by that name is
    // answered, with or without the sources (see [`resolved`]).
    if items.is_empty() && !unanswered.is_empty() {
        return Err(providers::Unanswered(unanswered))
            .with_context(|| format!("the search for {term:?} could not be answered"));
    }

    Ok(items)
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn local_search(state: &AppState, term: &str) -> Result<Vec<MediaItem>> {
    // Ranked by how well each answers the term, so that the series held here
    // by that very name comes first in Sonarr's list, before TMDB's.
    let query = repo::item::Query {
        term: Some(term.to_string()),
        kind: Some(MediaKind::Series),
        sort: repo::item::Sort::Relevance,
        // By its letters alone: TMDB's search, which follows, forgives a
        // misspelling, and a series held here that only resembles the title
        // would come first in the list Sonarr picks from.
        tolerance: Some(Vec::new()),
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
    resolved(ask_from_tmdb(state, tmdb_id))
        .await
        .with_context(|| format!("tmdb:{tmdb_id} could not be resolved"))
}

async fn ask_from_tmdb(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    let tvdb_id = match state.tmdb.tv_external_ids(tmdb_id).await {
        Ok(ids) => ids.tvdb_id,
        Err(e) => {
            tracing::warn!(tmdb_id, error = %e, "could not resolve the TVDB id");
            providers::went_unanswered(names::TMDB);
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
            .map(skyhook::to_item)
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

    #[tokio::test]
    async fn a_series_held_here_by_the_title_searched_for_comes_first() {
        // What Sonarr's search is answered with first: the catalogue's, then
        // TMDB's. The series named exactly so leads, ahead of a better-known
        // one that only starts with the title; one that merely resembles it
        // is left to TMDB, which forgives misspellings itself.
        let (state, _) = crate::service::testing::server().await;
        for (title, popularity, tvdb) in [
            ("Breaking Bad: Original Minisodes", 900.0, 1),
            ("Breaking Bad", 300.0, 2),
            ("Braking Bad", 50.0, 3),
        ] {
            let mut item = series(None, Some(tvdb));
            item.id = crate::db::new_id();
            item.title = title.into();
            item.slug = crate::domain::make_slug(title, None);
            item.popularity = Some(popularity);
            repo::item::upsert(
                &state.db,
                repo::item::ItemWrite {
                    item: &item,
                    replace_children: true,
                },
            )
            .await
            .expect("stored");
        }

        let found = search(&state, "breaking bad").await.expect("answered");
        let titles: Vec<&str> = found.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, ["Breaking Bad", "Breaking Bad: Original Minisodes"]);
    }
}
