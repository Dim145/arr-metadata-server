//! A season of the catalogue: what premiered, returned and carried on in one
//! quarter of the calendar — and, for whoever maintains the catalogue, what
//! else did that it does not hold yet.
//!
//! Seasons are the calendar's quarters, the way broadcasters in Japan and the
//! charts that follow them count: winter from January, spring from April,
//! summer from July, autumn from October. The same four serve a catalogue of
//! films and of every kind of series.

use std::collections::{HashMap, HashSet};

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
    providers::mal,
    service::{self, cour},
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
// Named explicitly: what a run did to one work is an `Entry` too, and utoipa
// keys schemas on the leaf name alone.
#[schema(as = SeasonEntry)]
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
    /// The AniList and MyAnimeList entries of the season that premiere in
    /// the quarter: the season itself where it is one entry, or the cour of
    /// it that starts here — the second half of a split season among those
    /// carrying on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cours: Vec<CourLine>,
}

/// One AniList and MyAnimeList entry a season is, or one of the cours it is
/// split into, as the anime identifier list places it on the season's
/// episodes, and described from them.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SeasonCour)]
pub struct CourLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anilist_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mal_id: Option<i64>,
    /// `TV`, `ONA`, `OVA`…, as the list files it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Which of the season's entries it is, the first being 1, and how many
    /// the list splits the season into.
    pub part: i32,
    pub parts: i32,
    /// Its first and last episodes the catalogue holds, by their number in
    /// the season.
    pub first_episode: i32,
    pub last_episode: i32,
    /// The day it premiered, or will: `YYYY-MM-DD`.
    pub starts: String,
    /// Its last episode's day, once it has finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends: Option<String>,
    /// Its episodes, and how many have aired.
    pub episodes: i64,
    pub aired: i64,
    pub status: cour::Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_episode: Option<CourNext>,
    /// What the catalogue already calls it, where the work's related
    /// entries name it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// MyAnimeList's own description of it, when the chart was asked to
    /// describe the entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mal: Option<mal::Described>,
}

/// The next episode of an entry to air.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SeasonCourNext)]
pub struct CourNext {
    /// Its number within the entry, the entry's first being 1, as AniList
    /// counts.
    pub episode: i32,
    /// Its number in the season.
    pub episode_number: i32,
    pub air_date: String,
    /// When it airs, in seconds since the epoch.
    pub airing_at: i64,
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
    /// Describe each AniList and MyAnimeList entry listed under an entry's
    /// `cours` as MyAnimeList does, asking it where this server has nothing
    /// at hand: slower, a few seconds for a season of sequels.
    pub describe: Option<bool>,
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
/// one language and one cached chart. One that is not a language is refused
/// where it arrives ([`crate::api::extract::language`]), and a code no table
/// knows is no language — neither is a way round the cache.
fn filed_language(asked: Option<&str>) -> AppResult<Option<String>> {
    Ok(crate::api::extract::language(asked)?
        .map(|tag| service::language::normalize(&tag))
        .filter(|l| service::language::is_known(l)))
}

/// What a season brought to the catalogue: its new series, the series that
/// returned with a new season, those still airing from the season before,
/// and its films.
///
/// Only what this server holds. Episode dates are read with their
/// corrections, as a work's page shows them.
#[utoipa::path(
    get, path = "/seasons/{year}/{season}", tag = TAG,
    operation_id = "seasons_chart",
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
    let language = filed_language(query.language.as_deref())?;
    let describe = query.describe.unwrap_or(false);

    // Read across the whole catalogue on every visit, so kept a while: under a
    // mark of the catalogue, so a work stored or corrected is counted at once,
    // and under today, which says what has aired. Readers who may edit see
    // more of each work, so theirs are kept apart.
    let key = format!(
        "season:{from}:{adult}:{}:{}:{today}:{describe}:{}:{}",
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
    let mut cours = premiering(&state, &works, &runs, (season, year), describe).await;

    let mut entries: Vec<Entry> = runs
        .into_iter()
        .map(|run| Entry {
            cours: cours
                .remove(&(run.media_id.clone(), run.season_number))
                .unwrap_or_default(),
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
            cours: Vec::new(),
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

/// The AniList and MyAnimeList entries that premiere in the quarter, by the
/// work and the season they are of: the series of the chart the anime
/// identifier list knows, each placed on its seasons ([`cour::of_work`]).
/// Each is described from its episodes as the work serves them, named as
/// the work's related entries name it, and — when `describe` — as
/// MyAnimeList describes it. A work that cannot be read is left without.
async fn premiering(
    state: &AppState,
    works: &[MediaItem],
    runs: &[repo::season::Run],
    asked: (SeasonName, i32),
    describe: bool,
) -> HashMap<(String, i32), Vec<CourLine>> {
    let mut found = HashMap::new();
    let running: HashSet<&str> = runs.iter().map(|run| run.media_id.as_str()).collect();
    let series: Vec<(&str, i64)> = works
        .iter()
        .filter(|work| work.kind == MediaKind::Series && running.contains(work.id.as_str()))
        .filter_map(|work| Some((work.id.as_str(), work.external_ids.tvdb?)))
        .collect();
    let tvdb_ids: Vec<i64> = series.iter().map(|(_, tvdb)| *tvdb).collect();
    let listed = match repo::anime::listed_series(&state.db, &tvdb_ids).await {
        Ok(listed) => listed,
        Err(e) => {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not read the anime identifier list for a season"
            );
            return found;
        }
    };

    let now = chrono::Utc::now();
    for (id, _) in series.into_iter().filter(|(_, tvdb)| listed.contains(tvdb)) {
        let item = match service::load(state, id).await {
            Ok(Some(item)) => item,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(
                    media_id = id,
                    error = format_args!("{e:#}"),
                    "could not read a work for its cours"
                );
                continue;
            }
        };
        let cours = cour::of_work(state, &item, now).await;
        let premiering: Vec<&cour::Cour> = cours
            .iter()
            .filter(|c| {
                matches!(c.span, Some(cour::Span::Season { number, .. }) if number > 0)
                    && c.quarter
                        .is_some_and(|(quarter, year)| (season_name(quarter), year) == asked)
            })
            .collect();
        if premiering.is_empty() {
            continue;
        }
        let named = match repo::anime::named_beside(&state.db, &item.id).await {
            Ok(named) => named,
            Err(e) => {
                tracing::warn!(
                    media_id = id,
                    error = format_args!("{e:#}"),
                    "could not read the entries named beside a work"
                );
                Vec::new()
            }
        };
        let described = if describe {
            let wanted: Vec<cour::Cour> = premiering.iter().map(|c| (*c).clone()).collect();
            cour::described(state, &item, &wanted, true).await
        } else {
            HashMap::new()
        };

        for line in premiering {
            let Some(cour::Span::Season { number, first, .. }) = line.span else {
                continue;
            };
            let (Some(starts), Some(status), Some((first_episode, last_episode))) =
                (line.starts.clone(), line.status, line.numbers)
            else {
                continue;
            };
            // Its place among the season's own entries.
            let mut starts_of: Vec<i32> = cours
                .iter()
                .filter_map(|c| match c.span {
                    Some(cour::Span::Season {
                        number: theirs,
                        first,
                        ..
                    }) if theirs == number => Some(first),
                    _ => None,
                })
                .collect();
            starts_of.sort_unstable();
            let part = starts_of.iter().position(|f| *f == first).unwrap_or(0) + 1;

            found
                .entry((item.id.clone(), number))
                .or_insert_with(Vec::new)
                .push(CourLine {
                    anilist_id: line.anilist,
                    mal_id: line.mal,
                    format: line.format.clone(),
                    part: i32::try_from(part).unwrap_or(1),
                    parts: i32::try_from(starts_of.len()).unwrap_or(1),
                    first_episode,
                    last_episode,
                    starts,
                    ends: line.ends.clone(),
                    episodes: line.episodes,
                    aired: line.aired,
                    status,
                    next_episode: line.next.as_ref().map(|next| CourNext {
                        episode: next.episode,
                        episode_number: next.episode_number,
                        air_date: next.day.clone(),
                        airing_at: next.airing_at,
                    }),
                    title: cour::kept_title(&named, line),
                    mal: line.mal.and_then(|id| described.get(&id).cloned()),
                });
        }
    }
    found
}

/// The chart's name for a quarter.
fn season_name(quarter: cour::Quarter) -> SeasonName {
    match quarter {
        cour::Quarter::Winter => SeasonName::Winter,
        cour::Quarter::Spring => SeasonName::Spring,
        cour::Quarter::Summer => SeasonName::Summer,
        cour::Quarter::Autumn => SeasonName::Autumn,
    }
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
    operation_id = "seasons_candidates",
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
    identity.require_write()?;
    if !state.tmdb.is_configured() {
        return Err(AppError::ProviderNotConfigured);
    }

    let (_, from, to) = quarter(year, &season)?;
    // TMDB speaks two-letter languages; the catalogue files three. Both are
    // checked where they arrive: each goes into a request to TMDB, and into
    // the key its answer is kept under.
    let original = crate::api::extract::language(query.original_language.as_deref())?
        .map(|tag| service::language::two_letter(&service::language::normalize(&tag)));
    let language = crate::api::extract::language(query.language.as_deref())?.map(|tag| {
        service::language::tmdb_locale(
            Some(&tag),
            &state.language(identity.client_id(), identity.peer_id()),
        )
    });
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
    fn a_language_is_filed_under_one_code_or_refused() {
        for asked in ["fr", "fr-FR", "fra", "FRA"] {
            assert_eq!(
                filed_language(Some(asked)).unwrap().as_deref(),
                Some("fra"),
                "{asked}"
            );
        }
        assert_eq!(filed_language(Some("")).unwrap(), None);
        assert_eq!(filed_language(None).unwrap(), None);
        // The shape of one, and no language the tables know: as none.
        assert_eq!(filed_language(Some("qqq")).unwrap(), None);
        for asked in ["x1", "zz-9", "français", "../x"] {
            assert!(filed_language(Some(asked)).is_err(), "{asked:?}");
        }
    }

    // ─── the cours that premiere ─────────────────────────────────────────────

    async fn server() -> AppState {
        let mut config = crate::config::Config::from_env().expect("a configuration");
        config.mode = crate::config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = crate::config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        AppState::bootstrap(config).await.expect("a server")
    }

    /// A series of one season split in two: six episodes in summer 2019,
    /// six from October, and the list's two entries for it.
    async fn split_in_two(state: &AppState, tvdb: i64, title: &str) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = crate::db::new_id();
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        item.status = Some("ended".into());
        item.external_ids.tvdb = Some(tvdb);
        // What AniList filed beside its first half: the second.
        item.relations = vec![crate::domain::Relation {
            id: String::new(),
            relation_type: "SEQUEL".into(),
            source: "anilist".into(),
            external_id: tvdb + 11,
            mal_id: None,
            title: format!("{title} Part 2"),
            medium: "anime".into(),
            format: Some("TV".into()),
            year: Some(2019),
            image: None,
            is_adult: false,
            work_id: None,
            sort_order: 0,
        }];
        item.episodes = (1..=12)
            .map(|n: i32| {
                let mut episode = crate::db::repo::child::blank_episode(1, n);
                episode.is_manual = false;
                let day = if n <= 6 {
                    chrono::NaiveDate::from_ymd_opt(2019, 7, 7).unwrap()
                        + chrono::Duration::weeks(i64::from(n) - 1)
                } else {
                    chrono::NaiveDate::from_ymd_opt(2019, 10, 6).unwrap()
                        + chrono::Duration::weeks(i64::from(n) - 7)
                };
                episode.air_date = Some(day.format("%Y-%m-%d").to_string());
                episode
            })
            .collect();
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item
    }

    #[tokio::test]
    async fn the_cour_that_premieres_in_the_quarter_is_shown_beside_its_season() {
        let state = server().await;
        let split = split_in_two(&state, 9_900_401, "A season split in two, charted").await;
        let whole = split_in_two(&state, 9_900_402, "A season the list does not split").await;
        repo::anime::replace_all(
            &state.db,
            &[
                repo::anime::Entry {
                    anilist_id: Some(9_900_411),
                    tvdb_id: Some(9_900_401),
                    tvdb_season: Some(1),
                    kind: Some("TV".into()),
                    ..Default::default()
                },
                repo::anime::Entry {
                    anilist_id: Some(9_900_412),
                    mal_id: Some(9_900_512),
                    tvdb_id: Some(9_900_401),
                    tvdb_season: Some(1),
                    tvdb_offset: Some(6),
                    kind: Some("TV".into()),
                    ..Default::default()
                },
            ],
        )
        .await
        .unwrap();
        let ids = [split.id.clone(), whole.id.clone()];
        let works = repo::item::by_ids(&state.db, &ids).await.unwrap();
        let charted = |from: &'static str, to: &'static str| {
            let state = state.clone();
            async move {
                repo::season::runs(&state.db, from, to, "2026-11-01", true)
                    .await
                    .unwrap()
            }
        };

        // Autumn: the season carries on, and its second half premieres.
        let runs = charted("2019-10-01", "2019-12-31").await;
        assert_eq!(runs.len(), 2);
        let mut cours = premiering(&state, &works, &runs, (SeasonName::Autumn, 2019), false).await;
        assert_eq!(cours.len(), 1, "the list does not know the other series");
        let lines = cours
            .remove(&(split.id.clone(), 1))
            .expect("the split season's");
        let [line] = lines.as_slice() else {
            panic!("one line")
        };
        assert_eq!(
            (line.anilist_id, line.mal_id),
            (Some(9_900_412), Some(9_900_512))
        );
        assert_eq!((line.part, line.parts), (2, 2));
        assert_eq!((line.first_episode, line.last_episode), (7, 12));
        assert_eq!(line.starts, "2019-10-06");
        assert_eq!(line.ends.as_deref(), Some("2019-11-10"));
        assert_eq!((line.episodes, line.aired), (6, 6));
        assert_eq!(line.status, cour::Status::Finished);
        assert!(line.next_episode.is_none() && line.mal.is_none());
        assert_eq!(
            line.title.as_deref(),
            Some("A season split in two, charted Part 2"),
            "as its first half names it"
        );

        // Summer: the first half, which is the season's start.
        let runs = charted("2019-07-01", "2019-09-30").await;
        let cours = premiering(&state, &works, &runs, (SeasonName::Summer, 2019), false).await;
        let line = &cours[&(split.id.clone(), 1)][0];
        assert_eq!(
            (line.anilist_id, line.part, line.starts.as_str()),
            (Some(9_900_411), 1, "2019-07-07")
        );
        assert_eq!(line.last_episode, 6);
    }

    #[test]
    fn a_chart_entry_without_cours_reads_as_it_always_has() {
        let entry = Entry {
            work_id: "w".into(),
            kind: EntryKind::NewSeason,
            season_number: Some(2),
            starts: "2026-10-04".into(),
            ends: None,
            episodes: None,
            aired: None,
            next_episode: None,
            sequel_of: None,
            cours: Vec::new(),
        };
        let written = serde_json::to_value(&entry).unwrap();
        assert!(written.get("cours").is_none(), "{written}");
        // And a chart kept before it had them still reads.
        let kept: Entry = serde_json::from_value(serde_json::json!({
            "workId": "w", "kind": "continuing", "starts": "2026-07-01"
        }))
        .unwrap();
        assert!(kept.cours.is_empty());
    }
}
