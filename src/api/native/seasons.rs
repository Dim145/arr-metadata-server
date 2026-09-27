//! A season of the catalogue: what premiered, returned and carried on in one
//! quarter of the calendar — and, for whoever maintains the catalogue, what
//! else did that it does not hold yet.
//!
//! Seasons are the calendar's quarters, the way broadcasters in Japan and the
//! charts that follow them count: winter from January, spring from April,
//! summer from July, autumn from October. The same four serve a catalogue of
//! films and of every kind of series.

use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo,
    domain::{ExternalSource, Kin, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// Filed with the rest of the catalogue in the documentation.
const TAG: &str = super::items::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(chart))
        .routes(routes!(candidates))
}

/// A quarter of the calendar, by the name a season chart gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SeasonName {
    Winter,
    Spring,
    Summer,
    Autumn,
}

impl SeasonName {
    fn parse(raw: &str) -> AppResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "winter" => Ok(Self::Winter),
            "spring" => Ok(Self::Spring),
            "summer" => Ok(Self::Summer),
            "autumn" | "fall" => Ok(Self::Autumn),
            other => Err(AppError::BadRequest(format!(
                "{other:?} is not a season: winter, spring, summer or autumn"
            ))),
        }
    }

    /// The quarter's first and last days, `YYYY-MM-DD`.
    fn days(self, year: i32) -> (String, String) {
        let (from, to) = match self {
            Self::Winter => ("01-01", "03-31"),
            Self::Spring => ("04-01", "06-30"),
            Self::Summer => ("07-01", "09-30"),
            Self::Autumn => ("10-01", "12-31"),
        };
        (format!("{year:04}-{from}"), format!("{year:04}-{to}"))
    }
}

/// The years a season can be asked for: every film and every broadcast.
const YEARS: std::ops::RangeInclusive<i32> = 1890..=2100;

fn quarter(year: i32, season: &str) -> AppResult<(SeasonName, String, String)> {
    if !YEARS.contains(&year) {
        return Err(AppError::BadRequest(format!(
            "{year} is not a year a season can be listed for"
        )));
    }
    let season = SeasonName::parse(season)?;
    let (from, to) = season.days(year);
    Ok((season, from, to))
}

// ─── the chart ───────────────────────────────────────────────────────────────

/// What an entry of the chart is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    /// A series whose first season starts in the quarter.
    NewSeries,
    /// A later season of a series, starting in the quarter.
    NewSeason,
    /// A season that began before the quarter and still airs in it.
    Continuing,
    Film,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NextEpisode {
    pub episode_number: i32,
    pub air_date: String,
}

/// One line of the chart.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub work_id: String,
    pub kind: EntryKind,
    /// The season, for a series whose episodes are known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_number: Option<i32>,
    /// The day it premiered, or will: `YYYY-MM-DD`. A season carrying on from
    /// the quarter before gives the day it began.
    pub starts: String,
    /// The last known episode's date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends: Option<String>,
    /// The season's episodes, and how many had aired by today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_episode: Option<NextEpisode>,
    /// What it is the sequel of, where a provider filed one: the nearest
    /// earlier work, and the work here that it is when the catalogue holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequel_of: Option<Kin>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    pub year: i32,
    pub season: SeasonName,
    /// The quarter's first and last days.
    pub from: String,
    pub to: String,
    /// In the order they premiered.
    pub entries: Vec<Entry>,
    /// Every work the entries belong to, drawn as a list draws them: artwork,
    /// scores and titles in the reader's language, no episodes.
    pub works: Vec<MediaItem>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChartQuery {
    /// Titles and synopses in this language, where a translation is held.
    pub language: Option<String>,
    /// The reader's own day, `YYYY-MM-DD`: what has aired, and what airs next,
    /// is counted from it. UTC's when absent, or more than a day off it.
    pub today: Option<String>,
}

/// The day the reader is on, where a clock somewhere could show it: at most a
/// day either side of UTC's, which is what it falls back to.
fn reader_day(asked: Option<&str>) -> String {
    let utc = chrono::Utc::now().date_naive();
    asked
        .and_then(|d| chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
        .filter(|d| (*d - utc).num_days().abs() <= 1)
        .unwrap_or(utc)
        .format("%Y-%m-%d")
        .to_string()
}

/// A language as translations are filed, or none: `fr`, `fr-FR` and `fra` are
/// one language and one cached chart, and a code that is not one is no
/// language — nor a way round the cache.
fn filed_language(asked: Option<&str>) -> Option<String> {
    asked
        .map(service::language::normalize)
        .filter(|l| l.len() == 3 && l.bytes().all(|b| b.is_ascii_lowercase()))
}

/// What a season brought to the catalogue: its new series, the series that
/// returned with a new season, those still airing from the season before,
/// and its films.
///
/// Only what this server holds. Episode dates are read with their
/// corrections, as a work's page shows them.
#[utoipa::path(
    get, path = "/seasons/{year}/{season}", tag = TAG,
    params(
        ("year" = i32, Path, description = "The year: `2026`"),
        ("season" = String, Path, description = "`winter` (January–March), `spring`, `summer` or `autumn`"),
        ChartQuery,
    ),
    responses(
        (status = 200, body = Chart),
        (status = 400, description = "Not a season, or not a year one can be listed for"),
    ),
)]
async fn chart(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((year, season)): Path<(i32, String)>,
    Query(query): Query<ChartQuery>,
) -> AppResult<Json<Chart>> {
    let (season, from, to) = quarter(year, &season)?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let today = reader_day(query.today.as_deref());
    let language = filed_language(query.language.as_deref());

    // Read across the whole catalogue on every visit, so kept a while: under a
    // mark of the catalogue, so a work stored or corrected is counted at once,
    // and under today, which says what has aired. Readers who may edit see
    // more of each work, so theirs are kept apart.
    let key = format!(
        "season:{from}:{adult}:{}:{}:{today}:{}:{}",
        language.as_deref().unwrap_or(""),
        identity.can_write(),
        state.caches.stamp(),
        repo::item::catalogue_stamp(&state.db).await?
    );
    if let Some(cached) = state.caches.searches.get(&key).await
        && let Ok(chart) = serde_json::from_str::<Chart>(&cached)
    {
        return Ok(Json(chart));
    }

    let runs = repo::season::runs(&state.db, &from, &to, &today, adult).await?;
    let premieres = repo::season::premieres(&state.db, &from, &to, adult).await?;

    // A series announced by its first date and then given one is placed by its
    // seasons, not listed again as announced.
    let running: HashSet<&str> = runs.iter().map(|r| r.media_id.as_str()).collect();
    let premieres: Vec<_> = premieres
        .into_iter()
        .filter(|(id, _, _)| !running.contains(id.as_str()))
        .collect();

    // In batches, a season of a large catalogue being more works than one
    // statement can name.
    let ids = repo::season::works(&runs, &premieres);
    let kin = repo::item::prequels_of(&state.db, &ids, adult).await?;
    let mut works = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let mut batch = repo::item::by_ids(&state.db, chunk).await?;
        service::apply_overrides(&state, &mut batch).await?;
        repo::item::load_artwork(&state.db, &mut batch).await?;
        works.append(&mut batch);
    }
    service::overlay_imdb_many(&state, &mut works).await;
    for work in &mut works {
        if let Some(language) = language.as_deref() {
            service::language::apply_shallow(&state, work, language);
        }
        state.media.localize(work);
        service::as_card(work);
    }
    service::redact_for_reader(&identity, &mut works);

    let mut entries: Vec<Entry> = runs
        .into_iter()
        .map(|run| Entry {
            sequel_of: kin.get(&run.media_id).cloned(),
            work_id: run.media_id,
            kind: if run.starts < from {
                EntryKind::Continuing
            } else if run.opens_series {
                EntryKind::NewSeries
            } else {
                EntryKind::NewSeason
            },
            season_number: Some(run.season_number),
            starts: run.starts,
            ends: Some(run.ends),
            episodes: Some(run.episodes),
            aired: Some(run.aired),
            next_episode: run.next.map(|(episode_number, air_date)| NextEpisode {
                episode_number,
                air_date,
            }),
        })
        .chain(premieres.into_iter().map(|(id, kind, day)| Entry {
            sequel_of: kin.get(&id).cloned(),
            work_id: id,
            kind: if kind == MediaKind::Movie {
                EntryKind::Film
            } else {
                EntryKind::NewSeries
            },
            season_number: None,
            starts: day.get(..10).unwrap_or(&day).to_string(),
            ends: None,
            episodes: None,
            aired: None,
            next_episode: None,
        }))
        .collect();

    // Only works that could be read, in the order they premiered.
    let found: HashSet<&str> = works.iter().map(|w| w.id.as_str()).collect();
    entries.retain(|e| found.contains(e.work_id.as_str()));
    entries.sort_by(|a, b| {
        a.starts
            .cmp(&b.starts)
            .then_with(|| a.work_id.cmp(&b.work_id))
            .then(a.season_number.cmp(&b.season_number))
    });

    let chart = Chart {
        year,
        season,
        from,
        to,
        entries,
        works,
    };
    if let Ok(encoded) = serde_json::to_string(&chart) {
        state.caches.searches.insert(key, encoded).await;
    }

    Ok(Json(chart))
}

// ─── what else premieres ─────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct CandidatesQuery {
    /// Titles and synopses in this language: `fr`, `en`…
    pub language: Option<String>,
    /// Only works made in this language, as the catalogue files it: `jpn`.
    pub original_language: Option<String>,
}

/// A work premiering in the season that this server does not hold.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub kind: MediaKind,
    pub tmdb_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    /// The day it premieres: `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premiere: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes: Option<i64>,
}

/// How many of each kind are offered at most.
const OFFERED: usize = 20;
/// How far down TMDB's list to look for them: five pages of twenty. A season
/// whose most popular works are all held already still has more to offer.
const PAGES: u32 = 5;

/// One page of TMDB's list, as it is kept.
#[derive(Serialize, Deserialize)]
struct Listed {
    works: Vec<Candidate>,
    more: bool,
}

/// What else premieres in a season, by TMDB, that nobody has asked this server
/// for: the new series and the films, most popular first, to import from.
///
/// For whoever maintains the catalogue — a visitor sees only what it holds —
/// and only where a TMDB key is configured.
#[utoipa::path(
    get, path = "/seasons/{year}/{season}/candidates", tag = TAG,
    params(
        ("year" = i32, Path, description = "The year: `2026`"),
        ("season" = String, Path, description = "`winter`, `spring`, `summer` or `autumn`"),
        CandidatesQuery,
    ),
    responses(
        (status = 200, body = Vec<Candidate>),
        (status = 400, description = "Not a season, or not a year one can be listed for"),
        (status = 403, description = "The caller may not edit the catalogue"),
        (status = 502, description = "TMDB could not be asked"),
        (status = 503, description = "No TMDB key is configured"),
    ),
)]
async fn candidates(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((year, season)): Path<(i32, String)>,
    Query(query): Query<CandidatesQuery>,
) -> AppResult<Json<Vec<Candidate>>> {
    if !identity.can_write() {
        return Err(AppError::Forbidden);
    }
    if !state.tmdb.is_configured() {
        return Err(AppError::ProviderNotConfigured);
    }

    let (_, from, to) = quarter(year, &season)?;
    // TMDB speaks two-letter languages; the catalogue files three.
    let original = query
        .original_language
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(service::language::two_letter);
    let language = query
        .language
        .as_deref()
        .map(str::trim)
        .filter(|l| l.len() == 2 && l.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase);
    let asking = Asking {
        from: &from,
        to: &to,
        original: original.as_deref(),
        language: language.as_deref(),
        adult: state.adult_for(
            identity.client_id(),
            identity.peer_id(),
            Some(state.adult_visible()),
        ),
    };

    let (series, films) = tokio::join!(
        offered(&state, MediaKind::Series, &asking),
        offered(&state, MediaKind::Movie, &asking),
    );
    let mut offered = series?;
    offered.extend(films?);
    Ok(Json(offered))
}

/// What a list of candidates is asked for.
struct Asking<'a> {
    from: &'a str,
    to: &'a str,
    original: Option<&'a str>,
    language: Option<&'a str>,
    adult: bool,
}

/// The works of one kind TMDB lists for the season that the store does not
/// hold, reading down its list a page at a time until there are enough.
async fn offered(
    state: &AppState,
    kind: MediaKind,
    asking: &Asking<'_>,
) -> AppResult<Vec<Candidate>> {
    let source = match kind {
        MediaKind::Series => ExternalSource::TmdbTv,
        MediaKind::Movie => ExternalSource::TmdbMovie,
    };
    let mut wanted = Vec::new();

    for page in 1..=PAGES {
        let listed = listed(state, kind, asking, page).await?;

        // Held already is asked of the store every time, not kept with the
        // list: a work imported a moment ago must not be offered again.
        let ids: Vec<String> = listed.works.iter().map(|c| c.tmdb_id.to_string()).collect();
        let held = repo::item::held_externals(&state.db, source, &ids).await?;
        wanted.extend(
            listed
                .works
                .into_iter()
                .filter(|c| !held.contains(&c.tmdb_id.to_string())),
        );

        if wanted.len() >= OFFERED || !listed.more {
            break;
        }
    }

    wanted.truncate(OFFERED);
    Ok(wanted)
}

/// One page of TMDB's list for the season, kept a while: under everything that
/// shapes it, the adult decision and the settings it was made under included.
async fn listed(
    state: &AppState,
    kind: MediaKind,
    asking: &Asking<'_>,
    page: u32,
) -> AppResult<Listed> {
    let key = format!(
        "season-candidates:{}:{}:{}:{}:{}:{}:{page}",
        kind.as_str(),
        asking.from,
        asking.original.unwrap_or(""),
        asking.language.unwrap_or(""),
        asking.adult,
        state.caches.stamp(),
    );
    if let Some(cached) = state.caches.searches.get(&key).await
        && let Ok(listed) = serde_json::from_str::<Listed>(&cached)
    {
        return Ok(listed);
    }

    let (from, to, original, language) = (asking.from, asking.to, asking.original, asking.language);
    let listed = match kind {
        MediaKind::Series => {
            let (found, more) = state
                .tmdb
                .discover_tv(from, to, original, language, page)
                .await
                .map_err(AppError::UpstreamUnavailable)?;
            Listed {
                works: found
                    .into_iter()
                    .filter(|s| asking.adult || !s.adult.unwrap_or(false))
                    .map(|s| Candidate {
                        kind,
                        tmdb_id: s.id,
                        original_title: s.original_name.filter(|o| o != &s.name),
                        title: s.name,
                        premiere: s.first_air_date.filter(|d| !d.is_empty()),
                        overview: s.overview.filter(|o| !o.trim().is_empty()),
                        poster: s
                            .poster_path
                            .as_deref()
                            .map(crate::providers::tmdb::image_url),
                        original_language: s.original_language,
                        score: s.vote_average.filter(|v| *v > 0.0),
                        votes: s.vote_count.filter(|v| *v > 0),
                    })
                    .collect(),
                more,
            }
        }
        MediaKind::Movie => {
            let (found, more) = state
                .tmdb
                .discover_movies(from, to, original, language, page)
                .await
                .map_err(AppError::UpstreamUnavailable)?;
            Listed {
                works: found
                    .into_iter()
                    .filter(|f| asking.adult || !f.adult.unwrap_or(false))
                    .map(|f| Candidate {
                        kind,
                        tmdb_id: f.id,
                        original_title: f.original_title.filter(|o| o != &f.title),
                        title: f.title,
                        premiere: f.release_date.filter(|d| !d.is_empty()),
                        overview: f.overview.filter(|o| !o.trim().is_empty()),
                        poster: f
                            .poster_path
                            .as_deref()
                            .map(crate::providers::tmdb::image_url),
                        original_language: f.original_language,
                        score: f.vote_average.filter(|v| *v > 0.0),
                        votes: f.vote_count.filter(|v| *v > 0),
                    })
                    .collect(),
                more,
            }
        }
    };

    if let Ok(encoded) = serde_json::to_string(&listed) {
        state.caches.searches.insert(key, encoded).await;
    }
    Ok(listed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_season_is_its_quarter() {
        assert_eq!(
            SeasonName::Winter.days(2026),
            ("2026-01-01".into(), "2026-03-31".into())
        );
        assert_eq!(
            SeasonName::Autumn.days(2026),
            ("2026-10-01".into(), "2026-12-31".into())
        );
        assert_eq!(SeasonName::parse("Fall").unwrap(), SeasonName::Autumn);
        assert!(SeasonName::parse("monsoon").is_err());
        assert!(quarter(12026, "autumn").is_err());
    }

    #[test]
    fn the_readers_day_is_taken_within_a_day_of_utcs_and_utcs_otherwise() {
        let utc = chrono::Utc::now().date_naive();
        let day = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();

        let tokyo = day(utc + chrono::Duration::days(1));
        assert_eq!(reader_day(Some(&tokyo)), tokyo);
        let hawaii = day(utc - chrono::Duration::days(1));
        assert_eq!(reader_day(Some(&hawaii)), hawaii);

        for wrong in [
            day(utc + chrono::Duration::days(3)),
            "tomorrow".into(),
            String::new(),
        ] {
            assert_eq!(reader_day(Some(&wrong)), day(utc), "{wrong:?}");
        }
        assert_eq!(reader_day(None), day(utc));
    }

    #[test]
    fn a_language_is_filed_under_one_code_or_none() {
        for asked in ["fr", "fr-FR", "fra", "FRA"] {
            assert_eq!(
                filed_language(Some(asked)).as_deref(),
                Some("fra"),
                "{asked}"
            );
        }
        for asked in ["", "x1", "zz-9", "français"] {
            assert_eq!(filed_language(Some(asked)), None, "{asked:?}");
        }
    }
}
