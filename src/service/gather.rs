//! Asking every provider, and folding the answers into one.
//!
//! Resolution — deciding *which* work a client means — lives in
//! [`super::series`] and [`super::movie`]. This is what happens once that is
//! settled: each enabled provider is asked, concurrently, and what they return
//! is merged by [`crate::merge`] and stored as a single entity with every raw
//! answer kept alongside.
//!
//! A provider that fails is logged and skipped. One source being down should
//! cost detail, not the whole answer.

use anyhow::Result;
use serde_json::Value;

use crate::{
    db::repo,
    domain::{ExternalIds, ExternalSource, MediaItem, MediaKind},
    merge::{self, Contribution},
    providers::{names, tmdb::map as tmdb_map},
    service::{anime, persist},
    state::AppState,
    wire,
};

/// The most seasons one series fetch will ask a provider about.
///
/// Each is its own HTTP call. Nothing real comes close — the longest-running
/// television on record is under a hundred — and the number is read off an
/// answer rather than known in advance.
const MAX_SEASONS: usize = 200;

/// Providers that add to a work and cannot describe one alone: artwork,
/// broadcast times, the anime sites' scores and titles.
///
/// When nothing else answered, what these returned is not stored. There is no
/// work to attach it to — only a name and some pictures filed under an id, or
/// a series with no episodes that would reach Sonarr as one.
const SUPPLEMENTS: &[&str] = &[
    names::FANART,
    names::TVMAZE,
    names::ANILIST,
    names::MAL,
    names::FANKAI_WIKI,
];

/// What one provider returned: its raw body, and the canonical form of it.
struct Answer {
    provider: &'static str,
    payload: Value,
    item: MediaItem,
}

/// Fetch a series from everything that can address it, and store the result.
///
/// `tmdb_id` and `tvdb_id` are what resolution worked out; either may be absent.
/// No language: each provider now reads the one setting that says which, and
/// Skyhook speaks only English whatever anyone asks for.
pub async fn series(
    state: &AppState,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
) -> Result<Option<MediaItem>> {
    let (from_tmdb, from_tvdb, from_skyhook, from_fanart, from_tvmaze) = tokio::join!(
        series_from_tmdb(state, tmdb_id),
        series_from_tvdb(state, tvdb_id),
        series_from_skyhook(state, tvdb_id),
        series_from_fanart(state, tvdb_id),
        series_from_tvmaze(state, tvdb_id),
    );

    let mut answers: Vec<Answer> = [from_tmdb, from_tvdb, from_skyhook, from_fanart, from_tvmaze]
        .into_iter()
        .flatten()
        .collect();

    // The anime sites second: which of their entries to ask about comes from
    // the identifier list, or failing that from the ids Skyhook just returned.
    if let Some(tvdb_id) = tvdb_id
        && anime::enabled(state)
        && describes_a_work(&answers)
    {
        let mut known = ExternalIds::default();
        for answer in &answers {
            known.mal.extend(&answer.item.external_ids.mal);
            known.anilist.extend(&answer.item.external_ids.anilist);
        }

        let chosen = anime::for_series(state, tvdb_id, &known).await;
        answers.extend(from_anime_sites(state, chosen, MediaKind::Series).await);
    }

    let stored = store(state, answers).await?;
    // The other orders TheTVDB numbers it in, kept beside the aired one.
    if let Some(item) = &stored {
        super::orders::gather(state, item).await;
    }
    Ok(stored)
}

/// Fetch a movie from everything that can address it, and store the result.
pub async fn movie(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Result<Option<MediaItem>> {
    let (from_tmdb, from_radarr, from_fanart) = tokio::join!(
        movie_from_tmdb(state, tmdb_id),
        movie_from_radarr(state, tmdb_id, imdb_id),
        movie_from_fanart(state, tmdb_id, imdb_id),
    );

    let mut answers: Vec<Answer> = [from_tmdb, from_radarr, from_fanart]
        .into_iter()
        .flatten()
        .collect();

    if let Some(tmdb_id) = tmdb_id
        && anime::enabled(state)
        && describes_a_work(&answers)
    {
        let chosen = anime::for_movie(state, tmdb_id).await;
        let mut from_sites = from_anime_sites(state, chosen, MediaKind::Movie).await;

        // A film keeps no AniList or MyAnimeList id. TheTVDB files films under
        // the series they belong to, and Skyhook lists them with its entries,
        // so a series already claims most of them — and a work is matched to
        // the stored one by any id it shares, whatever its kind. The film would
        // be written over the series.
        for answer in &mut from_sites {
            answer.item.external_ids = ExternalIds::default();
        }

        answers.extend(from_sites);
    }

    store(state, answers).await
}

/// Fetch a Fan-Kai production from Fankai, and store it.
///
/// Fankai alone: it is the only source that lists a recut, and the ids the
/// other providers would need are absent by design — TheTVDB's or TMDB's would
/// name the anime it was cut from, and merge the two into one work. A failure
/// is the caller's to see: with one source there is nothing to go on without.
pub async fn fankai_series(state: &AppState, fankai_id: i64) -> Result<Option<MediaItem>> {
    if !state.flag("fankai.enabled", false) {
        return Ok(None);
    }

    let Some((raw, item)) = state.fankai.series(fankai_id).await? else {
        return Ok(None);
    };

    let wiki = if state.flag("fankai.wiki", false) {
        fankai_from_wiki(state, &item).await
    } else {
        None
    };

    let mut answers = vec![Answer {
        provider: names::FANKAI,
        payload: raw,
        item,
    }];
    answers.extend(wiki);

    store(state, answers).await
}

/// What the Fankai wiki says a production was cut from, and which Fan-Kai
/// follows it, as the production's relations.
///
/// A supplement: its answer carries those and nothing else. The original's
/// title, year and cover come from AniList when that source is on, and from
/// the wiki's own links otherwise; a sequel is only named when Fankai lists it.
async fn fankai_from_wiki(state: &AppState, production: &MediaItem) -> Option<Answer> {
    use crate::providers::fankai::fold;

    // The wiki keeps a page per cut; whoever made this one tells them apart.
    let kaieurs: Vec<&str> = production
        .credits
        .iter()
        .filter(|c| {
            c.character_name
                .as_deref()
                .is_some_and(|r| fold(r) == "kaieur")
        })
        .map(|c| c.person_name.as_str())
        .collect();

    let (raw, page) = match state.fankai_wiki.page(&production.title, &kaieurs).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            tracing::debug!(title = %production.title, "the Fankai wiki has no page for this production");
            return None;
        }
        Err(e) => {
            tracing::warn!(
                title = %production.title,
                error = format_args!("{e:#}"),
                "the Fankai wiki could not be asked"
            );
            return None;
        }
    };

    let mut relations = Vec::new();

    for original in &page.originals {
        let from_anilist = match original.anilist {
            Some(id) if state.flag("anilist.enabled", false) => {
                match state.anilist.entry(id, "ORIGINAL").await {
                    Ok(found) => found,
                    Err(e) => {
                        tracing::warn!(
                            anilist_id = id,
                            error = format_args!("{e:#}"),
                            "AniList could not describe a Fan-Kai's original; using the wiki's"
                        );
                        None
                    }
                }
            }
            _ => None,
        };

        let relation = match from_anilist {
            Some(mut relation) => {
                relation.mal_id = relation.mal_id.or(original.mal);
                Some(relation)
            }
            None => original.relation(),
        };
        relations.extend(relation);
    }

    for sequel in &page.sequels {
        let name = crate::providers::fankai_wiki::base_name(sequel);
        match state.fankai.relation(name, "SEQUEL").await {
            Ok(Some(relation)) if Some(relation.external_id) != production.external_ids.fankai => {
                relations.push(relation);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                sequel = %sequel,
                error = format_args!("{e:#}"),
                "the Fan-Kai that follows could not be looked up"
            ),
        }
    }

    if relations.is_empty() {
        return None;
    }
    for (index, relation) in relations.iter_mut().enumerate() {
        relation.sort_order = i32::try_from(index).unwrap_or(i32::MAX);
    }

    let mut item = MediaItem::empty(MediaKind::Series);
    item.relations = relations;

    Some(Answer {
        provider: names::FANKAI_WIKI,
        payload: raw,
        item,
    })
}

/// Whether anything that can stand for a work on its own answered.
fn describes_a_work(answers: &[Answer]) -> bool {
    answers.iter().any(|a| !SUPPLEMENTS.contains(&a.provider))
}

/// AniList and MyAnimeList, asked at the same time about the entry `chosen`.
async fn from_anime_sites(state: &AppState, chosen: anime::Chosen, kind: MediaKind) -> Vec<Answer> {
    if chosen.is_empty() {
        return Vec::new();
    }

    let (from_anilist, from_mal) = tokio::join!(
        from_anilist(state, chosen.anilist, kind),
        from_mal(state, chosen.mal, kind),
    );

    [from_anilist, from_mal].into_iter().flatten().collect()
}

async fn from_anilist(state: &AppState, id: Option<i64>, kind: MediaKind) -> Option<Answer> {
    let id = id?;
    if !state.flag("anilist.enabled", false) {
        return None;
    }

    match state.anilist.media(id, kind).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::ANILIST,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                anilist_id = id,
                error = format_args!("{e:#}"),
                "AniList lookup failed"
            );
            None
        }
    }
}

async fn from_mal(state: &AppState, id: Option<i64>, kind: MediaKind) -> Option<Answer> {
    let id = id?;
    if !state.flag("mal.enabled", false) {
        return None;
    }

    match state.mal.anime(id, kind).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::MAL,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                mal_id = id,
                error = format_args!("{e:#}"),
                "MyAnimeList lookup failed"
            );
            None
        }
    }
}

/// Merge what came back and write it.
async fn store(state: &AppState, answers: Vec<Answer>) -> Result<Option<MediaItem>> {
    if answers.is_empty() {
        return Ok(None);
    }

    let providers: Vec<&str> = answers.iter().map(|a| a.provider).collect();
    tracing::debug!(?providers, "merging provider answers");

    if !describes_a_work(&answers) {
        tracing::warn!(
            ?providers,
            "only supplementary sources answered; not storing"
        );
        return Ok(None);
    }

    let snapshots: Vec<(String, Value)> = answers
        .iter()
        .map(|a| (a.provider.to_string(), a.payload.clone()))
        .collect();

    let contributions: Vec<Contribution> = answers
        .into_iter()
        .map(|a| Contribution {
            provider: a.provider.to_string(),
            item: a.item,
        })
        .collect();

    let Some(merged) = merge::combine(contributions, &state.config.provider_priority) else {
        return Ok(None);
    };

    // Answers from supplements alone were refused above; this is the provider
    // of record that answered with a blank title. Storing it would put a
    // nameless row in the catalogue and hand the client an entry it cannot
    // display.
    if merged.title.trim().is_empty() {
        tracing::warn!(?providers, "no provider named this work; not storing it");
        return Ok(None);
    }

    Ok(Some(persist(state, merged, &snapshots).await?))
}

// ─── per provider ────────────────────────────────────────────────────────────

async fn series_from_tmdb(state: &AppState, tmdb_id: Option<i64>) -> Option<Answer> {
    let tmdb_id = tmdb_id?;
    if !state.tmdb.is_configured() {
        return None;
    }

    let (raw, tv) = match state.tmdb.tv(tmdb_id).await {
        Ok(Some(found)) => found,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "TMDB series fetch failed"
            );
            return None;
        }
    };

    // Capped, because this is one outbound call per entry and the pool they
    // queue in is shared by everything else this process is doing. The season
    // count comes from the answer, and the answer comes from a URL an operator
    // can point elsewhere.
    let numbers: Vec<i32> = tv
        .seasons
        .iter()
        .map(|s| s.season_number)
        .take(MAX_SEASONS)
        .collect();

    if tv.seasons.len() > MAX_SEASONS {
        tracing::warn!(
            tmdb_id,
            seasons = tv.seasons.len(),
            "more seasons than this server will fetch; taking the first {MAX_SEASONS}"
        );
    }

    let seasons = state.tmdb.tv_seasons(tmdb_id, &numbers).await;

    Some(Answer {
        provider: names::TMDB,
        payload: raw,
        item: tmdb_map::tv_to_item(&tv, &seasons),
    })
}

/// Sonarr's own Skyhook, as a second opinion rather than only a fallback.
///
/// It carries things TMDB has no field for: the broadcast time of day, TVMaze
/// and AniList ids, and the air-order hints Sonarr uses for anime.
async fn series_from_skyhook(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    let tvdb_id = tvdb_id?;
    if !state.flag("skyhook.enrich", true) {
        return None;
    }

    match state.skyhook.show(tvdb_id).await {
        Ok(Some((raw, show))) => Some(Answer {
            provider: names::SKYHOOK,
            payload: raw,
            item: wire::sonarr::to_item(&show),
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "Skyhook enrichment failed"
            );
            None
        }
    }
}

/// TheTVDB, which is where absolute episode numbering comes from.
async fn series_from_tvdb(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    let tvdb_id = tvdb_id?;
    if !state.tvdb.is_enabled() {
        return None;
    }

    match state.tvdb.series(tvdb_id).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::TVDB,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TheTVDB lookup failed"
            );
            None
        }
    }
}

/// TVmaze, for the instant each episode aired.
///
/// What it says about an episode is only used where its broadcast date agrees
/// with the spine's — see `merge::apply_broadcast_times`.
async fn series_from_tvmaze(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    let tvdb_id = tvdb_id?;
    if !state.flag("tvmaze.enabled", false) {
        return None;
    }

    // A series fetched before has its TVmaze id on file — Skyhook and TheTVDB
    // both carry it — which turns three requests into one.
    let known = match repo::item::find_id_by_external(
        &state.db,
        ExternalSource::TvdbSeries,
        &tvdb_id.to_string(),
    )
    .await
    {
        Ok(Some(id)) => repo::item::load_external_ids(&state.db, &id)
            .await
            .ok()
            .and_then(|ids| ids.tvmaze),
        _ => None,
    };

    match state.tvmaze.series(tvdb_id, known).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::TVMAZE,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "TVmaze lookup failed"
            );
            None
        }
    }
}

/// Fanart.tv, which contributes artwork and nothing else.
///
/// It indexes television on TVDB ids only, so a series with no TVDB id cannot
/// be looked up there at all.
async fn series_from_fanart(state: &AppState, tvdb_id: Option<i64>) -> Option<Answer> {
    let tvdb_id = tvdb_id?;
    if !state.fanart.is_enabled() {
        return None;
    }

    match state.fanart.series(tvdb_id).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::FANART,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tvdb_id,
                error = format_args!("{e:#}"),
                "Fanart.tv lookup failed"
            );
            None
        }
    }
}

async fn movie_from_fanart(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Option<Answer> {
    if !state.fanart.is_enabled() {
        return None;
    }

    // It accepts either key; TMDB's is the one more titles are indexed under.
    let key = tmdb_id
        .map(|id| id.to_string())
        .or_else(|| imdb_id.map(String::from))?;

    match state.fanart.movie(&key).await {
        Ok(Some((raw, item))) => Some(Answer {
            provider: names::FANART,
            payload: raw,
            item,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(%key, error = format_args!("{e:#}"), "Fanart.tv lookup failed");
            None
        }
    }
}

async fn movie_from_tmdb(state: &AppState, tmdb_id: Option<i64>) -> Option<Answer> {
    let tmdb_id = tmdb_id?;
    if !state.tmdb.is_configured() {
        return None;
    }

    match state.tmdb.movie(tmdb_id).await {
        Ok(Some((raw, movie))) => Some(Answer {
            provider: names::TMDB,
            payload: raw,
            item: tmdb_map::movie_to_item(&movie),
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                tmdb_id,
                error = format_args!("{e:#}"),
                "TMDB movie fetch failed"
            );
            None
        }
    }
}

/// Radarr's own metadata service, as a second opinion.
///
/// It resolves certifications by country and carries ratings from IMDb,
/// Metacritic and Rotten Tomatoes that TMDB does not have at all.
async fn movie_from_radarr(
    state: &AppState,
    tmdb_id: Option<i64>,
    imdb_id: Option<&str>,
) -> Option<Answer> {
    if !state.flag("radarr.enrich", true) {
        return None;
    }

    let found = match tmdb_id {
        Some(id) => state.radarr_metadata.movie(id).await,
        None => match imdb_id {
            Some(id) => state.radarr_metadata.by_imdb_id(id).await,
            None => return None,
        },
    };

    match found {
        Ok(Some((raw, movie))) => Some(Answer {
            provider: names::RADARR,
            payload: raw,
            item: wire::radarr::to_item(&movie),
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                ?tmdb_id,
                error = format_args!("{e:#}"),
                "Radarr metadata enrichment failed"
            );
            None
        }
    }
}
