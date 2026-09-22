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
    domain::MediaItem,
    merge::{self, Contribution},
    providers::{names, tmdb::map as tmdb_map},
    service::persist,
    state::AppState,
    wire,
};

/// The most seasons one series fetch will ask a provider about.
///
/// Each is its own HTTP call. Nothing real comes close — the longest-running
/// television on record is under a hundred — and the number is read off an
/// answer rather than known in advance.
const MAX_SEASONS: usize = 200;

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
    let (from_tmdb, from_tvdb, from_skyhook, from_fanart) = tokio::join!(
        series_from_tmdb(state, tmdb_id),
        series_from_tvdb(state, tvdb_id),
        series_from_skyhook(state, tvdb_id),
        series_from_fanart(state, tvdb_id),
    );

    let answers: Vec<Answer> = [from_tmdb, from_tvdb, from_skyhook, from_fanart]
        .into_iter()
        .flatten()
        .collect();

    store(state, answers).await
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

    let answers: Vec<Answer> = [from_tmdb, from_radarr, from_fanart]
        .into_iter()
        .flatten()
        .collect();

    store(state, answers).await
}

/// Merge what came back and write it.
async fn store(state: &AppState, answers: Vec<Answer>) -> Result<Option<MediaItem>> {
    if answers.is_empty() {
        return Ok(None);
    }

    let providers: Vec<&str> = answers.iter().map(|a| a.provider).collect();
    tracing::debug!(?providers, "merging provider answers");

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

    // Fanart.tv answers with artwork and nothing else, so when it is the only
    // provider that replied there is no work here to speak of — just pictures
    // filed under an id. Storing that would put a nameless row in the catalogue
    // and hand the client an entry it cannot display.
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
