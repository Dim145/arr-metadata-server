//! Reading the catalogue across works: what it can be narrowed by, and what
//! airs and comes out when. Who is in it is `people`'s.
//!
//! Visitors may read both when browsing is public — they are the catalogue
//! seen from another side, not how the server is run.

use std::collections::{BTreeMap, HashMap};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo::{
        self,
        editorial::{Note, Scope},
        item::Facets,
    },
    domain::{CreditType, Episode, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// Filed with the rest of the catalogue in the documentation.
const TAG: &str = super::items::TAG;

/// The longest window the calendar answers for.
const MAX_DAYS: i64 = 62;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(facets))
        .routes(routes!(calendar))
        .routes(routes!(genre))
        .routes(routes!(network))
        .routes(routes!(year))
        .routes(routes!(decade))
        .routes(routes!(compare))
}

// ─── facets ──────────────────────────────────────────────────────────────────

/// What the catalogue can be narrowed by next, with how many works each
/// choice would leave: the list's facets, and the countries and the ratings
/// its further filters choose among.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = CatalogueFacets)]
pub struct CatalogueFacets {
    #[serde(flatten)]
    pub facets: Facets,
    /// The countries the works were made in, ISO 3166-1 alpha-2, most common
    /// first — counted as if no country were chosen.
    pub countries: Vec<repo::item::Facet>,
    /// The ratings the works carry in the reader's country (`region`), as
    /// that country writes them; mildest first where they are ages, most
    /// common first otherwise. Counted as if no rating were chosen.
    pub certifications: Vec<repo::item::Facet>,
    /// The country those ratings are read in: the one asked for, else the
    /// one this caller is answered for. Absent, the ratings are the ones the
    /// works carry as their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// What the catalogue can be narrowed by next, with how many works each
/// choice would leave.
///
/// Takes the list's own filters and counts under them — see
/// [`repo::item::facets`] for how each is counted — and over what this caller
/// may see: an adult title's genre is not counted for somebody who would never
/// be shown it. A term counts the works the list finds for it, a misspelt one
/// included.
#[utoipa::path(
    get, path = "/facets", tag = TAG,
    operation_id = "browse_facets",
    params(super::items::ListQuery),
    responses(
        (status = 200, body = CatalogueFacets),
        (status = 400, description = "A filter was not one the list understands"),
    ),
)]
async fn facets(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<super::items::ListQuery>,
) -> AppResult<Json<CatalogueFacets>> {
    let mut filters = super::items::to_query(&state, &identity, query)?;
    // The page and its order are the list's; the counts are the same for all.
    filters.sort = repo::item::Sort::default();
    filters.descending = None;
    filters.limit = 0;
    filters.offset = 0;

    // Asked for on every visit to the catalogue, so kept a while — under the
    // settings generation, so a change to what adults see is not answered from
    // before it, and under a mark of the catalogue, so a work added, removed
    // or switched off is counted at once. An empty answer is not kept: it is
    // what a new install says for the minutes before its first works arrive.
    let key = format!(
        "facets:{filters:?}:{}:{}",
        state.caches.stamp(),
        repo::item::catalogue_stamp(&state.db).await?
    );

    if let Some(cached) = state.caches.searches.get(&key).await
        && let Ok(facets) = serde_json::from_str::<CatalogueFacets>(&cached)
    {
        return Ok(Json(facets));
    }

    // The works a misspelt term is taken for, as the list takes it: settled
    // once the answer is known not to be kept already.
    repo::item::tolerate(&state.db, &mut filters).await?;
    let found = CatalogueFacets {
        facets: repo::item::facets(&state.db, &filters).await?,
        countries: repo::item::country_facets(&state.db, &filters).await?,
        certifications: repo::item::certification_facets(&state.db, &filters).await?,
        region: filters.region.clone(),
    };
    if found.facets.total > 0
        && let Ok(encoded) = serde_json::to_string(&found)
    {
        state.caches.searches.insert(key, encoded).await;
    }

    Ok(Json(found))
}

// ─── calendar ────────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CalendarQuery {
    /// The first instant of the window, RFC 3339: `2026-09-21T22:00:00Z`.
    pub from: String,
    /// The instant the window ends, not included. At most 62 days after `from`.
    pub to: String,
    /// Titles in this language, where a translation is held.
    pub language: Option<String>,
    /// The reader's country, ISO 3166-1 alpha-2: `FR`. A film is listed on
    /// the days it comes out there, where those are known, and on its own
    /// dates otherwise. Absent, the country this caller is answered for:
    /// `tmdb.watchRegion` as set for it, else its language's.
    pub region: Option<String>,
}

/// One episode of the window, and which of the listed works it belongs to.
#[derive(Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Airing {
    pub work_id: String,
    pub episode: Episode,
}

/// How a film reaches the public: the three dates Radarr keeps for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum ReleaseType {
    /// In cinemas: the work's `inCinemas`.
    Cinema,
    /// To buy or rent online: its `digitalRelease`.
    Digital,
    /// On disc: its `physicalRelease`.
    Physical,
}

impl ReleaseType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cinema => "cinema",
            Self::Digital => "digital",
            Self::Physical => "physical",
        }
    }
}

/// A film coming out on one day of the window, and which of the listed works
/// it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub work_id: String,
    /// The day, `YYYY-MM-DD`. A release has no hour: it is placed at midnight
    /// UTC on its day, as an episode with only a date is.
    pub date: String,
    /// What comes out that day, cinemas first. Two on the same day are one
    /// release, not two.
    pub release_types: Vec<ReleaseType>,
}

/// One thing the schedule lists: an episode airing, or a film coming out.
#[derive(Clone, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CalendarEntry {
    /// An episode, and the work it belongs to. Boxed: an episode is four
    /// times a release, and a window holds hundreds of either.
    Episode(Box<Airing>),
    /// A film's release: in cinemas, digitally, on disc.
    Release(Release),
}

#[derive(Serialize, ToSchema)]
pub struct Calendar {
    /// The episodes and the films' releases together, earliest first: an
    /// episode at its moment, a release at midnight UTC on its day, and of
    /// two at the same moment the episode first.
    pub entries: Vec<CalendarEntry>,
    /// Every work the entries belong to, drawn as a list is: artwork and
    /// scores, no episodes.
    pub works: Vec<MediaItem>,
    /// The window held more episodes, or more releases, than one answer
    /// carries, and the latest are missing: ask for a shorter one.
    pub truncated: bool,
}

/// What airs and what comes out in a window: the episodes of every series,
/// and the films out in cinemas, digitally or on disc.
///
/// An episode by the moment it airs, in UTC — where no provider knew the
/// time, midnight UTC on its date, which is what Sonarr is given; a release
/// by its day, at midnight UTC too, once for each day a film comes out. A
/// film's days are the reader's country's where its releases there are
/// known, its own otherwise, and a day somebody corrected by hand stays as
/// they put it. The window is the caller's to choose, so a day is the
/// caller's own day wherever it is. Switched off, or for adults where the
/// reader would not be shown one, a work is left out of both.
#[utoipa::path(
    get, path = "/calendar", tag = TAG,
    operation_id = "browse_calendar",
    params(CalendarQuery),
    responses(
        (status = 200, body = Calendar),
        (status = 400, description = "The window is not two instants, or is longer than 62 days, or the region is not a country code"),
    ),
)]
async fn calendar(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<CalendarQuery>,
) -> AppResult<Json<Calendar>> {
    let instant = |raw: &str| {
        chrono::DateTime::parse_from_rfc3339(raw.trim())
            .map(|t| t.with_timezone(&chrono::Utc))
            .map_err(|_| AppError::BadRequest(format!("{raw:?} is not an RFC 3339 instant")))
    };

    let from = instant(&query.from)?;
    let to = instant(&query.to)?;
    if to <= from || to - from > chrono::Duration::days(MAX_DAYS) {
        return Err(AppError::BadRequest(format!(
            "the window must end after it starts, and last at most {MAX_DAYS} days"
        )));
    }

    let language = crate::api::extract::language(query.language.as_deref())?;
    let region = match query
        .region
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        Some(asked) => Some(country_code(asked).ok_or_else(|| {
            AppError::BadRequest("region is a country code such as FR or US".into())
        })?),
        None => None,
    };
    Ok(Json(
        window_in(
            &state,
            &identity,
            from,
            to,
            language.as_deref(),
            region.as_deref(),
        )
        .await?,
    ))
}

/// A country as releases are filed under it: two letters, upper case.
fn country_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
}

/// The schedule of a window, as the calendar answers it and the feeds repeat
/// it: every episode airing from `from` to `to` and every film coming out in
/// it, and the works they belong to, with the overrides and the reader's
/// language applied.
pub(super) async fn window(
    state: &AppState,
    identity: &Identity,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
    language: Option<&str>,
) -> AppResult<Calendar> {
    window_in(state, identity, from, to, language, None).await
}

/// [`window`], for a reader in `region` — the country the caller is
/// answered for when it is `None`.
pub(super) async fn window_in(
    state: &AppState,
    identity: &Identity,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
    language: Option<&str>,
    region: Option<&str>,
) -> AppResult<Calendar> {
    let stamp = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let region = region
        .map(str::to_string)
        .or_else(|| state.watch_region(identity.client_id(), identity.peer_id()));
    let (airing, episodes_cut) =
        repo::item::airing(&state.db, &stamp(from), &stamp(to), adult).await?;
    let (films, releases_cut) =
        repo::item::releasing(&state.db, &stamp(from), &stamp(to), adult).await?;
    // And the films coming out in the reader's country in the window,
    // whatever their own dates say.
    let local = match region.as_deref() {
        Some(country) => {
            repo::release::releasing_in(&state.db, country, &stamp(from), &stamp(to), adult).await?
        }
        None => Vec::new(),
    };

    let ids: Vec<String> = {
        let mut ids: Vec<String> = airing
            .iter()
            .map(|a| a.media_id.clone())
            .chain(films)
            .chain(local)
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };

    let mut works = repo::item::by_ids(&state.db, &ids).await?;

    // Each work carries its own episodes of the window while the overrides and
    // the reader's language are applied, so a date or a title somebody
    // corrected is the one listed; then they are handed back separately.
    let mut by_work: HashMap<String, Vec<Episode>> = HashMap::new();
    for mut a in airing {
        if let Some(still) = &a.episode.image {
            a.episode.image = Some(state.media.localized(still));
        }
        by_work.entry(a.media_id).or_default().push(a.episode);
    }
    for work in &mut works {
        work.episodes = by_work.remove(&work.id).unwrap_or_default();
    }

    service::apply_overrides(state, &mut works).await?;
    // A film's days as its reader's country has them, the corrections laid
    // over first: one somebody fixed stays where they put it.
    if let Some(country) = region.as_deref() {
        let films: Vec<String> = works
            .iter()
            .filter(|w| w.kind == MediaKind::Movie)
            .map(|w| w.id.clone())
            .collect();
        let mut held = repo::release::load_many(&state.db, &films).await?;
        for work in works.iter_mut().filter(|w| w.kind == MediaKind::Movie) {
            work.releases = held.remove(&work.id).unwrap_or_default();
            in_country(work, country);
        }
    }
    // Before the language: the stored translations the overlay reads come
    // with the artwork, and without them every series kept its own title.
    // The window's works at once: one query a series was up to a thousand.
    repo::item::load_artwork(&state.db, &mut works).await?;
    if let Some(language) = language {
        service::language::apply_stored_many(state, &mut works, language).await?;
    }
    service::overlay_imdb_many(state, &mut works).await;
    for work in &mut works {
        state.media.localize(work);
        service::as_card(work);
    }
    service::redact_for_reader(identity, &mut works);

    let titles: HashMap<String, String> = works
        .iter()
        .map(|w| (w.id.clone(), w.title.to_lowercase()))
        .collect();

    let in_window = |t: chrono::DateTime<chrono::Utc>| t >= from && t < to;
    let mut entries: Vec<CalendarEntry> = works
        .iter_mut()
        .flat_map(|work| {
            let id = work.id.clone();
            std::mem::take(&mut work.episodes)
                .into_iter()
                .map(move |episode| Airing {
                    work_id: id.clone(),
                    episode,
                })
        })
        // An override can move an episode out of the window it was found in.
        .filter(|a| aired_at(&a.episode).is_some_and(in_window))
        .map(|a| CalendarEntry::Episode(Box::new(a)))
        .collect();
    // A film's days are read with its corrections laid over, as an
    // episode's are: one somebody moved is listed where they put it.
    entries.extend(
        works
            .iter()
            .filter(|w| w.kind == MediaKind::Movie)
            .flat_map(releases_of)
            .filter(|r| released_at(r).is_some_and(in_window))
            .map(CalendarEntry::Release),
    );

    // Overrides may have moved some, so the order is settled here: by the
    // moment — of an episode and a release at the same one, the episode
    // first — then the work, then the episode, so a season dropped at once
    // is listed in its own order.
    entries.sort_by_cached_key(|entry| {
        let (at, rank, work_id, season, number) = match entry {
            CalendarEntry::Episode(a) => (
                aired_at(&a.episode),
                0,
                &a.work_id,
                a.episode.season_number,
                a.episode.episode_number,
            ),
            CalendarEntry::Release(r) => (released_at(r), 1, &r.work_id, 0, 0),
        };
        (
            at,
            rank,
            titles.get(work_id).cloned().unwrap_or_default(),
            work_id.clone(),
            season,
            number,
        )
    });

    // A work whose every date of the window was moved out of it by hand has
    // nothing listed, and is not drawn.
    let listed: std::collections::HashSet<&str> = entries
        .iter()
        .map(|entry| match entry {
            CalendarEntry::Episode(a) => a.work_id.as_str(),
            CalendarEntry::Release(r) => r.work_id.as_str(),
        })
        .collect();
    works.retain(|w| listed.contains(w.id.as_str()));

    Ok(Calendar {
        entries,
        works,
        truncated: episodes_cut || releases_cut,
    })
}

/// When an episode airs, as the calendar compares it: its instant, or
/// midnight UTC on its day where no provider knew the time.
///
/// Read as an instant, not compared as text. A time corrected by hand is any
/// RFC 3339 instant the registry takes — `2026-10-05T21:00:00+02:00`,
/// `…T20:00:00.000Z` — and as text those landed on the wrong day, in the
/// wrong order; a day given as an instant became `…ZT00:00:00Z`.
pub(super) fn aired_at(episode: &Episode) -> Option<chrono::DateTime<chrono::Utc>> {
    let instant = |text: &str| {
        chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc))
    };
    episode
        .air_date_utc
        .as_deref()
        .and_then(instant)
        .or_else(|| {
            let day = episode.air_date.as_deref()?.trim();
            chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc())
                .or_else(|| instant(day))
        })
}

/// A film's releases, a day each, earliest first: one for each distinct day
/// among its dates in cinemas, digitally and on disc, with every kind that
/// falls on it.
///
/// A date is read by its day, the first ten characters: a provider gives a
/// day, Radarr an instant at midnight, a correction either. One that reads
/// as no day is no release.
pub(super) fn releases_of(work: &MediaItem) -> Vec<Release> {
    let mut days: Vec<(chrono::NaiveDate, Vec<ReleaseType>)> = Vec::new();
    for (kind, date) in [
        (ReleaseType::Cinema, &work.in_cinemas),
        (ReleaseType::Digital, &work.digital_release),
        (ReleaseType::Physical, &work.physical_release),
    ] {
        let Some(day) = date
            .as_deref()
            .and_then(|d| d.trim().get(..10))
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        match days.iter_mut().find(|(d, _)| *d == day) {
            Some((_, kinds)) => kinds.push(kind),
            None => days.push((day, vec![kind])),
        }
    }
    days.sort_by_key(|(day, _)| *day);
    days.into_iter()
        .map(|(day, release_types)| Release {
            work_id: work.id.clone(),
            date: day.format("%Y-%m-%d").to_string(),
            release_types,
        })
        .collect()
}

/// A film's days as a reader in `country` has them: its first in cinemas,
/// digitally and on disc there, wherever its releases there give one and
/// nobody fixed that date by hand; its own dates otherwise. The Plex provider
/// reads a film's day by the same rule (`release_day`, in `wire/plex.rs`).
pub(super) fn in_country(work: &mut MediaItem, country: &str) {
    let days = work.days_in(country);
    if let Some(day) = days.cinema
        && !work.is_locked("inCinemas")
    {
        work.in_cinemas = Some(day);
    }
    if let Some(day) = days.digital
        && !work.is_locked("digitalRelease")
    {
        work.digital_release = Some(day);
    }
    if let Some(day) = days.physical
        && !work.is_locked("physicalRelease")
    {
        work.physical_release = Some(day);
    }
}

/// When a release is, as the calendar compares it: midnight UTC on its day.
pub(super) fn released_at(release: &Release) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::NaiveDate::parse_from_str(&release.date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc())
}

// ─── a genre, a network, a year, a decade ────────────────────────────────────

/// How many works a page leads with, rated highest first.
const BEST: i64 = 5;

/// What a page of the catalogue is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FacetKind {
    Genre,
    Network,
    Year,
    Decade,
}

/// A page of the catalogue: a genre's, a network's or a studio's, a year's,
/// a decade's — what it is called, how many works it holds, the five rated
/// highest, how to list the rest, and what the programmer wrote about it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FacetPage {
    pub kind: FacetKind,
    /// What its address names it by: `drama`, `hbo`, `1999`, `1990`. A genre
    /// asked for by another of its names, or a decade by another of its
    /// years, is answered under this one, which the page's address becomes.
    pub slug: String,
    /// What it is called: a genre in the reader's language where one is
    /// known, a network as the works most often name it, a year and a decade
    /// as their first year, which the interface words.
    pub name: String,
    /// A genre's names in every language known: `en`, `fr`, and `und` for
    /// the one a source gave it by.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
    /// How many works it holds that the reader may see.
    pub count: i64,
    /// The five of them rated highest, as cards are drawn.
    pub best: Vec<MediaItem>,
    /// The list's own filters that list them all, for `/items` and
    /// `/facets`: `{"genre": "drama"}`, `{"yearFrom": "1990", "yearTo": "1999"}`.
    pub filter: BTreeMap<String, String>,
    /// The programmer's note on it: in the reader's language, else in
    /// English, else the one written last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<Note>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// The reader's language: `fr`. The titles, the genre's name and the note
    /// are read in it where they are written in it.
    pub language: Option<String>,
}

/// A genre's page: by its slug — `drama` — or by any name it goes by, in any
/// language: "Drame" is the same page.
#[utoipa::path(
    get, path = "/genres/{slug}", tag = TAG,
    operation_id = "browse_genre",
    params(("slug" = String, Path, description = "The genre's slug, or any name it goes by"), PageQuery),
    responses(
        (status = 200, body = FacetPage),
        (status = 400, description = "The language is not one"),
        (status = 404, description = "No genre goes by that name"),
    ),
)]
async fn genre(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(asked): Path<String>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<FacetPage>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    if asked.chars().count() > 128 {
        return Err(AppError::NotFound);
    }
    let genre = repo::genre::find(&state.db, &asked)
        .await?
        .ok_or(AppError::NotFound)?;
    let name = genre.name(language.as_deref());
    let filters = repo::item::Query {
        genres: vec![genre.slug.clone()],
        ..Default::default()
    };
    let shown = BTreeMap::from([("genre".to_string(), genre.slug.clone())]);
    Ok(Json(
        facet_page(
            &state,
            &identity,
            Listing {
                kind: FacetKind::Genre,
                slug: genre.slug,
                name,
                names: genre.names,
                filters,
                shown,
            },
            language.as_deref(),
        )
        .await?,
    ))
}

/// A network's or a studio's page, by the slug of its name — `hbo`, `a24` —
/// as the works name it most often. Several spellings that make one slug are
/// one page, under the most common.
#[utoipa::path(
    get, path = "/networks/{slug}", tag = TAG,
    operation_id = "browse_network",
    params(("slug" = String, Path, description = "The slug of the network's or the studio's name"), PageQuery),
    responses(
        (status = 200, body = FacetPage),
        (status = 400, description = "The language is not one"),
        (status = 404, description = "No work this reader may see names that network or studio"),
    ),
)]
async fn network(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(asked): Path<String>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<FacetPage>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let slug = slug::slugify(asked.trim());
    if slug.is_empty() || slug.len() > 128 {
        return Err(AppError::NotFound);
    }
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let name = network_named(&state, &slug, adult)
        .await?
        .ok_or(AppError::NotFound)?;
    let filters = repo::item::Query {
        network: Some(name.clone()),
        ..Default::default()
    };
    let shown = BTreeMap::from([("network".to_string(), name.clone())]);
    Ok(Json(
        facet_page(
            &state,
            &identity,
            Listing {
                kind: FacetKind::Network,
                slug,
                name,
                names: BTreeMap::new(),
                filters,
                shown,
            },
            language.as_deref(),
        )
        .await?,
    ))
}

/// The name of the network or the studio a slug stands for, as the works
/// name it most often, or none: kept a while under the catalogue's mark,
/// since every visit to the page asks.
pub(crate) async fn network_named(
    state: &AppState,
    slug: &str,
    adult: bool,
) -> AppResult<Option<String>> {
    let key = format!(
        "network-name:{slug}:{adult}:{}",
        repo::item::catalogue_stamp(&state.db).await?
    );
    if let Some(cached) = state.caches.searches.get(&key).await {
        return Ok(Some(cached).filter(|name| !name.is_empty()));
    }
    let named = repo::item::network_names(&state.db, adult)
        .await?
        .into_iter()
        .filter(|(name, _)| slug::slugify(name) == slug)
        // The most common spelling, and of two as common the first in order.
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(name, _)| name);
    state
        .caches
        .searches
        .insert(key, named.clone().unwrap_or_default())
        .await;
    Ok(named)
}

/// A year's page: the works that came out in it, as their pages date them.
#[utoipa::path(
    get, path = "/years/{year}", tag = TAG,
    operation_id = "browse_year",
    params(("year" = String, Path, description = "Four digits, from 1800 to 2200"), PageQuery),
    responses(
        (status = 200, body = FacetPage),
        (status = 400, description = "The language is not one"),
        (status = 404, description = "Not a year"),
    ),
)]
async fn year(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(asked): Path<String>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<FacetPage>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let year = year_of(&asked).ok_or(AppError::NotFound)?;
    Ok(Json(
        facet_page(
            &state,
            &identity,
            years(FacetKind::Year, year, year),
            language.as_deref(),
        )
        .await?,
    ))
}

/// A decade's page, by its first year — `1990` — or by any of its years,
/// which is answered under the first.
#[utoipa::path(
    get, path = "/decades/{year}", tag = TAG,
    operation_id = "browse_decade",
    params(("year" = String, Path, description = "Its first year, or any of its years"), PageQuery),
    responses(
        (status = 200, body = FacetPage),
        (status = 400, description = "The language is not one"),
        (status = 404, description = "Not a year"),
    ),
)]
async fn decade(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(asked): Path<String>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<FacetPage>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let year = year_of(&asked).ok_or(AppError::NotFound)?;
    let first = year - year.rem_euclid(10);
    Ok(Json(
        facet_page(
            &state,
            &identity,
            years(FacetKind::Decade, first, first + 9),
            language.as_deref(),
        )
        .await?,
    ))
}

/// A year as an address gives it: four digits, from 1800 to 2200.
pub(crate) fn year_of(raw: &str) -> Option<i32> {
    let raw = raw.trim();
    (raw.len() == 4 && raw.bytes().all(|b| b.is_ascii_digit()))
        .then(|| raw.parse::<i32>().ok())
        .flatten()
        .filter(|year| (1800..=2200).contains(year))
}

/// The page of the years from `from` to `to`, named and addressed by the first.
fn years(kind: FacetKind, from: i32, to: i32) -> Listing {
    Listing {
        kind,
        slug: from.to_string(),
        name: from.to_string(),
        names: BTreeMap::new(),
        filters: repo::item::Query {
            year_from: Some(from),
            year_to: Some(to),
            ..Default::default()
        },
        shown: BTreeMap::from([
            ("yearFrom".to_string(), from.to_string()),
            ("yearTo".to_string(), to.to_string()),
        ]),
    }
}

/// What a page is of, before what it holds is read.
struct Listing {
    kind: FacetKind,
    slug: String,
    name: String,
    names: BTreeMap<String, String>,
    /// The works it holds, as the store narrows them.
    filters: repo::item::Query,
    /// The same, as the list's own parameters.
    shown: BTreeMap<String, String>,
}

/// A page, read: how many works it holds that the reader may see, the five
/// rated highest — by the score each card leads with — and its note.
async fn facet_page(
    state: &AppState,
    identity: &Identity,
    listing: Listing,
    language: Option<&str>,
) -> AppResult<FacetPage> {
    let filters = repo::item::Query {
        include_adult: state.adult_for(identity.client_id(), identity.peer_id(), None),
        sort: repo::item::Sort::Rating,
        limit: BEST,
        offset: 0,
        ..listing.filters
    };
    let (found, count) = repo::item::search_counted(&state.db, &filters).await?;
    let best = cards(state, identity, found, language).await?;

    let scope = match listing.kind {
        FacetKind::Genre => Scope::Genre,
        FacetKind::Network => Scope::Network,
        FacetKind::Year => Scope::Year,
        FacetKind::Decade => Scope::Decade,
    };
    let reader = language.map_or_else(|| "en".to_string(), repo::genre::language_of);
    let note = repo::editorial::read(&state.db, scope, &listing.slug, &reader).await?;

    Ok(FacetPage {
        kind: listing.kind,
        slug: listing.slug,
        name: listing.name,
        names: listing.names,
        count,
        best,
        filter: listing.shown,
        note,
    })
}

/// Works drawn as a list draws them: their locks applied, their pictures and
/// scores, IMDb's figure where it is newer, their titles in the reader's
/// language, and nothing a card does not show.
async fn cards(
    state: &AppState,
    identity: &Identity,
    mut items: Vec<MediaItem>,
    language: Option<&str>,
) -> AppResult<Vec<MediaItem>> {
    service::apply_overrides(state, &mut items).await?;
    repo::item::load_artwork(&state.db, &mut items).await?;
    service::overlay_imdb_many(state, &mut items).await;
    if let Some(language) = language {
        for item in &mut items {
            service::language::apply_shallow(state, item, language);
        }
    }
    for item in &mut items {
        state.media.localize(item);
        service::as_card(item);
    }
    service::redact_for_reader(identity, &mut items);
    Ok(items)
}

// ─── two works side by side ──────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CompareQuery {
    /// The first work's id.
    pub a: String,
    /// The second's.
    pub b: String,
    /// Titles in this language: `fr`.
    pub language: Option<String>,
}

/// One work of the two, as the comparison draws it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Compared {
    /// The work as a card draws it — its pictures, its scores, its running
    /// time, its genres — without its seasons, episodes or credits.
    pub work: MediaItem,
    /// A series' seasons, its specials aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seasons: Option<i64>,
    /// A series' episodes, its specials aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episodes: Option<i64>,
}

/// What somebody did on a work: their job, and the part they played.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub credit_type: CreditType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
}

/// Somebody both works credit.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedPerson {
    /// Their record: `/people/{id}` is their page.
    pub person_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// What they did on the first work.
    pub first: Vec<Part>,
    /// And on the second.
    pub second: Vec<Part>,
}

#[derive(Serialize, ToSchema)]
pub struct Comparison {
    pub first: Compared,
    pub second: Compared,
    /// The people both credit, as the works' own credits file them — their
    /// casts and crews, not single episodes' — those who made them first,
    /// then by their billing.
    pub shared: Vec<SharedPerson>,
}

/// Two works side by side: their scores, their running times, their seasons,
/// and the people they share.
#[utoipa::path(
    get, path = "/compare", tag = TAG,
    operation_id = "browse_compare",
    params(CompareQuery),
    responses(
        (status = 200, body = Comparison),
        (status = 400, description = "The two are one work, or the language is not one"),
        (status = 404, description = "One of them is not a work this reader may see"),
    ),
)]
async fn compare(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<CompareQuery>,
) -> AppResult<Json<Comparison>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let (a, b) = (query.a.trim(), query.b.trim());
    for id in [a, b] {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(AppError::NotFound);
        }
    }
    let first = super::visible_work(&state, &identity, a).await?;
    let second = super::visible_work(&state, &identity, b).await?;
    if first.id == second.id {
        return Err(AppError::BadRequest(
            "two works are compared, not one with itself".into(),
        ));
    }

    let shared = shared_people(&first, &second);
    let drawn = |mut work: MediaItem| {
        if let Some(language) = language.as_deref() {
            service::language::apply_shallow(&state, &mut work, language);
        }
        let (seasons, episodes) = match work.kind {
            MediaKind::Series => {
                let seasons = work.seasons.iter().filter(|s| s.season_number > 0).count();
                let episodes = work.episodes.iter().filter(|e| e.season_number > 0).count();
                (Some(seasons as i64), Some(episodes as i64))
            }
            MediaKind::Movie => (None, None),
        };
        work.seasons.clear();
        work.episodes.clear();
        work.credits.clear();
        work.people = Default::default();
        service::as_card(&mut work);
        service::redact_for_reader(&identity, std::slice::from_mut(&mut work));
        Compared {
            work,
            seasons,
            episodes,
        }
    };
    let (first, second) = (drawn(first), drawn(second));

    Ok(Json(Comparison {
        first,
        second,
        shared,
    }))
}

/// The people two works both credit, by the records their credits are filed
/// under: what each did on each, those who made the works first — directing,
/// creating, writing, producing — then by where the works bill them.
fn shared_people(first: &MediaItem, second: &MediaItem) -> Vec<SharedPerson> {
    /// Who each credit of a work names: their record, their name, a portrait,
    /// what they did, and their best place in the billing.
    fn by_person(work: &MediaItem) -> BTreeMap<String, (String, Option<String>, Vec<Part>, i32)> {
        let mut found: BTreeMap<String, (String, Option<String>, Vec<Part>, i32)> = BTreeMap::new();
        for credit in &work.credits {
            let Some(person) = work.people.of(credit).and_then(|p| p.person_id.clone()) else {
                continue;
            };
            let entry = found.entry(person).or_insert_with(|| {
                (
                    credit.person_name.clone(),
                    credit.image.clone(),
                    Vec::new(),
                    credit.sort_order,
                )
            });
            if entry.1.is_none() {
                entry.1 = credit.image.clone();
            }
            entry.3 = entry.3.min(credit.sort_order);
            let part = Part {
                credit_type: credit.credit_type,
                character: credit
                    .character_name
                    .clone()
                    .filter(|c| !c.trim().is_empty()),
            };
            if !entry.2.contains(&part) {
                entry.2.push(part);
            }
        }
        found
    }

    let mut theirs = by_person(second);
    let mut shared: Vec<(bool, i32, SharedPerson)> = by_person(first)
        .into_iter()
        .filter_map(|(person_id, (name, image, first_parts, first_place))| {
            let (_, other_image, second_parts, second_place) = theirs.remove(&person_id)?;
            let made = first_parts
                .iter()
                .chain(&second_parts)
                .any(|part| part.credit_type != CreditType::Actor);
            Some((
                !made,
                first_place.min(second_place),
                SharedPerson {
                    person_id,
                    name,
                    image: image.or(other_image),
                    first: first_parts,
                    second: second_parts,
                },
            ))
        })
        .collect();
    shared.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.name.cmp(&b.2.name))
    });
    shared.into_iter().map(|(_, _, person)| person).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_episode_without_a_time_is_placed_at_midnight_utc() {
        let at = |text: &str| {
            chrono::DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let mut episode = crate::db::repo::child::blank_episode(1, 1);
        episode.air_date = Some("2026-09-24".into());
        episode.air_date_utc = None;
        assert_eq!(aired_at(&episode), Some(at("2026-09-24T00:00:00Z")));

        episode.air_date_utc = Some("2026-09-25T01:30:00Z".into());
        assert_eq!(aired_at(&episode), Some(at("2026-09-25T01:30:00Z")));
    }

    #[test]
    fn a_time_corrected_by_hand_is_compared_as_the_instant_it_names() {
        let at = |text: &str| {
            chrono::DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let mut episode = crate::db::repo::child::blank_episode(1, 1);

        // Nine in the evening in Paris is seven in UTC, the same day; as text
        // it sorted after a quarter past eight UTC.
        episode.air_date_utc = Some("2026-10-05T21:00:00+02:00".into());
        let paris = aired_at(&episode).unwrap();
        assert_eq!(paris, at("2026-10-05T19:00:00Z"));
        episode.air_date_utc = Some("2026-10-05T20:15:00.000Z".into());
        assert!(paris < aired_at(&episode).unwrap());

        // Two in the morning in Tokyo is the day before in UTC.
        episode.air_date_utc = Some("2026-10-06T02:00:00+09:00".into());
        assert_eq!(aired_at(&episode), Some(at("2026-10-05T17:00:00Z")));

        // A day given as an instant is that instant, not `…ZT00:00:00Z`.
        episode.air_date_utc = None;
        episode.air_date = Some("2026-10-05T21:00:00Z".into());
        assert_eq!(aired_at(&episode), Some(at("2026-10-05T21:00:00Z")));

        // What reads as neither is not in any window.
        episode.air_date = Some("TBA".into());
        assert_eq!(aired_at(&episode), None);
    }

    #[test]
    fn a_film_is_a_release_a_day_with_every_kind_that_falls_on_it() {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.id = "f1".into();
        film.in_cinemas = Some("2014-11-05".into());
        // Digitally and on disc the same day, the one written as Radarr
        // writes it: one release, both kinds.
        film.digital_release = Some("2015-03-31T00:00:00Z".into());
        film.physical_release = Some(" 2015-03-31".into());
        let releases = releases_of(&film);
        assert_eq!(
            releases,
            [
                Release {
                    work_id: "f1".into(),
                    date: "2014-11-05".into(),
                    release_types: vec![ReleaseType::Cinema],
                },
                Release {
                    work_id: "f1".into(),
                    date: "2015-03-31".into(),
                    release_types: vec![ReleaseType::Digital, ReleaseType::Physical],
                },
            ]
        );
        assert_eq!(
            released_at(&releases[1]),
            chrono::NaiveDate::from_ymd_opt(2015, 3, 31)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc())
        );

        // Earliest first, whatever the kind; a year alone, or a word, is no day.
        film.in_cinemas = Some("2016".into());
        film.digital_release = Some("TBA".into());
        film.physical_release = Some("2015-01-02".into());
        let dates = |film: &MediaItem| {
            releases_of(film)
                .into_iter()
                .map(|r| r.date)
                .collect::<Vec<_>>()
        };
        assert_eq!(dates(&film), ["2015-01-02"]);
        film.in_cinemas = Some("2015-07-01".into());
        film.digital_release = Some("2015-06-01".into());
        assert_eq!(dates(&film), ["2015-01-02", "2015-06-01", "2015-07-01"]);
        assert!(releases_of(&MediaItem::empty(MediaKind::Movie)).is_empty());
    }

    /// A work as a provider would have left it.
    async fn stored(
        state: &AppState,
        kind: MediaKind,
        title: &str,
        adjust: impl FnOnce(&mut MediaItem),
    ) -> MediaItem {
        let mut work = MediaItem::empty(kind);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, None);
        adjust(&mut work);
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        work
    }

    /// The schedule's entries, a line each: what, whose, when.
    fn lines(calendar: &Calendar) -> Vec<String> {
        calendar
            .entries
            .iter()
            .map(|entry| match entry {
                CalendarEntry::Episode(a) => format!(
                    "{} episode {}",
                    a.work_id,
                    aired_at(&a.episode)
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_default()
                ),
                CalendarEntry::Release(r) => format!(
                    "{} release {} {}",
                    r.work_id,
                    r.date,
                    r.release_types
                        .iter()
                        .map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join("+")
                ),
            })
            .collect()
    }

    #[tokio::test]
    async fn the_films_coming_out_are_listed_beside_the_episodes() {
        let state = super::super::testing::server().await;
        let at = |text: &str| {
            chrono::DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let (from, to) = (at("2026-11-02T00:00:00Z"), at("2026-11-09T00:00:00Z"));

        // In cinemas and digitally the same day, on disc two days after.
        let interstellar = stored(&state, MediaKind::Movie, "Interstellar", |w| {
            w.in_cinemas = Some("2026-11-05".into());
            w.digital_release = Some("2026-11-05".into());
            w.physical_release = Some("2026-11-07T00:00:00Z".into());
        })
        .await;
        let dune = stored(&state, MediaKind::Movie, "Dune", |w| {
            w.in_cinemas = Some("2026-12-18".into());
        })
        .await;
        for (title, enabled, adult) in [("Switched off", false, false), ("For adults", true, true)]
        {
            stored(&state, MediaKind::Movie, title, |w| {
                w.in_cinemas = Some("2026-11-05".into());
                w.is_enabled = enabled;
                w.is_adult = adult;
            })
            .await;
        }
        let dark = stored(&state, MediaKind::Series, "Dark", |w| {
            let mut episode = crate::db::repo::child::blank_episode(1, 1);
            episode.is_manual = false;
            episode.air_date = Some("2026-11-05".into());
            episode.air_date_utc = Some("2026-11-05T20:00:00Z".into());
            w.episodes = vec![episode];
        })
        .await;

        let calendar = window(&state, &Identity::Visitor, from, to, None)
            .await
            .expect("listed");
        // In one list by the moment: a release at midnight UTC on its day, so
        // before that evening's episode. Out of the window, switched off, for
        // adults where they are hidden — as a new server has them: not there.
        assert_eq!(
            lines(&calendar),
            [
                format!("{} release 2026-11-05 cinema+digital", interstellar.id),
                format!("{} episode 2026-11-05T20:00:00+00:00", dark.id),
                format!("{} release 2026-11-07 physical", interstellar.id),
            ]
        );
        assert!(!calendar.truncated);
        let mut drawn: Vec<&str> = calendar.works.iter().map(|w| w.id.as_str()).collect();
        drawn.sort_unstable();
        let mut expected = [interstellar.id.as_str(), dark.id.as_str()];
        expected.sort_unstable();
        assert_eq!(drawn, expected);

        // The entries say which kind each is. The episodes alone, which
        // repeated them, are no longer sent.
        let json = serde_json::to_value(&calendar).expect("serialised");
        assert!(json.get("episodes").is_none(), "{json}");
        assert_eq!(
            json["entries"][0],
            serde_json::json!({
                "kind": "release",
                "workId": interstellar.id,
                "date": "2026-11-05",
                "releaseTypes": ["cinema", "digital"],
            })
        );
        assert_eq!(json["entries"][1]["kind"], "episode");
        assert_eq!(json["entries"][1]["episode"]["seasonNumber"], 1);

        // Corrected by hand: Dune brought into the week digitally, the disc
        // of Interstellar put back into the new year. Each listed where it
        // was put, for a visitor and an editor alike; the editor is told the
        // date is locked, the visitor is not.
        let correct = async |work: &MediaItem, field: &str, day: &str| {
            repo::override_field::set(
                &state.db,
                &work.id,
                crate::domain::fields::Scope::Item,
                field,
                Some(&serde_json::json!(day)),
                None,
            )
            .await
            .expect("corrected");
        };
        correct(&dune, "digitalRelease", "2026-11-08").await;
        correct(&interstellar, "physicalRelease", "2027-01-10").await;

        for (who, identity) in [
            ("a visitor", Identity::Visitor),
            ("anybody, with authentication off", Identity::Anonymous),
        ] {
            let calendar = window(&state, &identity, from, to, None)
                .await
                .expect("listed");
            assert_eq!(
                lines(&calendar),
                [
                    format!("{} release 2026-11-05 cinema+digital", interstellar.id),
                    format!("{} episode 2026-11-05T20:00:00+00:00", dark.id),
                    format!("{} release 2026-11-08 digital", dune.id),
                ],
                "{who}"
            );
            let locked = &calendar
                .works
                .iter()
                .find(|w| w.id == dune.id)
                .expect("drawn")
                .locked_fields;
            if identity.can_write() {
                assert_eq!(locked, &["item/digitalRelease"]);
            } else {
                assert!(locked.is_empty(), "{locked:?}");
            }
        }
    }

    #[tokio::test]
    async fn a_film_is_listed_on_its_day_in_the_reader_s_country() {
        use crate::domain::{CountryRelease, ReleaseKind};

        let state = super::super::testing::server().await;
        let at = |text: &str| {
            chrono::DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let release = |country: &str, kind: ReleaseKind, day: &str| CountryRelease {
            country: country.into(),
            release_type: Some(kind),
            date: Some(day.into()),
            certification: None,
            note: None,
            source: Some("tmdb".into()),
        };
        // Arrival, as TMDB has it: in American cinemas on 11 November, in
        // French ones on 7 December after a festival, on French VOD in April.
        let arrival = stored(&state, MediaKind::Movie, "Arrival", |w| {
            w.in_cinemas = Some("2016-11-11".into());
            w.digital_release = Some("2017-01-31".into());
            w.releases = vec![
                release("US", ReleaseKind::Theatrical, "2016-11-11"),
                release("FR", ReleaseKind::Premiere, "2016-11-05"),
                release("FR", ReleaseKind::Theatrical, "2016-12-07"),
                release("FR", ReleaseKind::Digital, "2017-04-07"),
            ];
        })
        .await;
        let december = (at("2016-12-05T00:00:00Z"), at("2016-12-12T00:00:00Z"));
        type Window = (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>);
        let listed = async |(from, to): Window, region: &str| {
            lines(
                &window_in(&state, &Identity::Visitor, from, to, None, Some(region))
                    .await
                    .expect("listed"),
            )
        };

        // In France, on the French day; in America, and in a country it has
        // no day in, on its own: not that week.
        assert_eq!(
            listed(december, "FR").await,
            [format!("{} release 2016-12-07 cinema", arrival.id)]
        );
        assert!(listed(december, "US").await.is_empty());
        assert!(listed(december, "DE").await.is_empty());
        // Found by its French day alone, its own being January's.
        let april = (at("2017-04-03T00:00:00Z"), at("2017-04-10T00:00:00Z"));
        assert_eq!(
            listed(april, "FR").await,
            [format!("{} release 2017-04-07 digital", arrival.id)]
        );

        // A day somebody fixed by hand stays where they put it.
        repo::override_field::set(
            &state.db,
            &arrival.id,
            crate::domain::fields::Scope::Item,
            "inCinemas",
            Some(&serde_json::json!("2016-12-09")),
            None,
        )
        .await
        .expect("corrected");
        assert_eq!(
            listed(december, "FR").await,
            [format!("{} release 2016-12-09 cinema", arrival.id)]
        );
    }
    /// A credit as a source gives it, naming somebody by TMDB's id.
    fn credited(kind: CreditType, name: &str, tmdb: i64, place: i32) -> crate::domain::Credit {
        crate::domain::Credit {
            id: crate::db::new_id(),
            credit_type: kind,
            person_name: name.into(),
            character_name: (kind == CreditType::Actor).then(|| format!("{name}'s part")),
            image: None,
            tmdb_person_id: Some(tmdb),
            credit_tmdb_id: None,
            sort_order: place,
            is_manual: false,
        }
    }

    fn rated(score: f64) -> Vec<crate::domain::Rating> {
        vec![crate::domain::Rating {
            source: "tmdb".into(),
            value: Some(score),
            votes: Some(1_000),
            rating_type: Some("user".into()),
        }]
    }

    async fn page_of(
        state: &AppState,
        kind: FacetKind,
        asked: &str,
        language: &str,
    ) -> AppResult<FacetPage> {
        let identity = Extension(Identity::Visitor);
        let path = Path(asked.to_string());
        let query = Query(PageQuery {
            language: Some(language.into()),
        });
        let state = State(state.clone());
        match kind {
            FacetKind::Genre => genre(state, identity, path, query).await,
            FacetKind::Network => network(state, identity, path, query).await,
            FacetKind::Year => year(state, identity, path, query).await,
            FacetKind::Decade => decade(state, identity, path, query).await,
        }
        .map(|Json(page)| page)
    }

    #[tokio::test]
    async fn a_genre_s_page_holds_its_works_whatever_language_named_it_and_its_note() {
        let state = super::super::testing::server().await;
        // Three fetched in English, three in French, the best of them a
        // comedy as well, and a comedy alone.
        for (n, genres, score) in [
            (1, vec!["Drama"], 6.1),
            (2, vec!["Drama", "Crime"], 8.4),
            (3, vec!["Drama"], 7.0),
            (4, vec!["Drame"], 7.7),
            (5, vec!["Drame", "Comédie"], 9.1),
            (6, vec!["Drame"], 5.5),
            (7, vec!["Comedy"], 9.9),
        ] {
            stored(&state, MediaKind::Movie, &format!("Film {n}"), |w| {
                w.genres = genres.into_iter().map(String::from).collect();
                w.ratings = rated(score);
            })
            .await;
        }
        crate::service::listing::relist_stale(&state.db, false, 100)
            .await
            .expect("listed");

        // By its French name, read in French: the drama page, named in French.
        let page = page_of(&state, FacetKind::Genre, "Drame", "fr")
            .await
            .expect("a page");
        assert_eq!(page.slug, "drama");
        assert_eq!(page.name, "Drame");
        assert_eq!(page.names["en"], "Drama");
        assert_eq!(page.count, 6);
        let best: Vec<&str> = page.best.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(best, ["Film 5", "Film 2", "Film 4", "Film 3", "Film 1"]);
        assert_eq!(
            page.filter,
            BTreeMap::from([("genre".to_string(), "drama".to_string())])
        );
        assert!(page.note.is_none());
        // Drawn as cards are: no episodes, no credits.
        assert!(page.best.iter().all(|w| w.episodes.is_empty()));

        // The list's own filter, given back, lists the same six.
        let all = repo::item::search_counted(
            &state.db,
            &repo::item::Query {
                genres: vec![page.filter["genre"].clone()],
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .expect("listed");
        assert_eq!(all.1, 6);

        // With a note in each language, each reader is given theirs.
        for (language, body) in [("en", "The *quiet* ones."), ("fr", "Les *calmes*.")] {
            repo::editorial::put(&state.db, Scope::Genre, "drama", language, body, None)
                .await
                .unwrap();
        }
        let french = page_of(&state, FacetKind::Genre, "drama", "fr")
            .await
            .expect("a page");
        assert_eq!(
            french.note.map(|n| n.body).as_deref(),
            Some("Les *calmes*.")
        );
        let english = page_of(&state, FacetKind::Genre, "drama", "en")
            .await
            .expect("a page");
        assert_eq!(english.name, "Drama");
        assert_eq!(
            english.note.map(|n| n.body).as_deref(),
            Some("The *quiet* ones.")
        );

        // A genre the server knows and nothing is filed under is a page with
        // nothing on it; a name nobody gave a genre is no page.
        let western = page_of(&state, FacetKind::Genre, "western", "en")
            .await
            .expect("a page");
        assert_eq!((western.count, western.best.len()), (0, 0));
        assert!(matches!(
            page_of(&state, FacetKind::Genre, "isekai", "en").await,
            Err(AppError::NotFound)
        ));
    }

    #[tokio::test]
    async fn a_network_s_page_is_found_by_the_slug_of_its_name() {
        let state = super::super::testing::server().await;
        for (title, network, studio) in [
            ("On Canal", Some("Canal+"), None),
            ("Also on Canal", Some("Canal+"), None),
            ("Made by Canal", None, Some("Canal +")),
            ("Elsewhere", Some("HBO"), None),
        ] {
            stored(&state, MediaKind::Series, title, |w| {
                w.network = network.map(String::from);
                w.studio = studio.map(String::from);
            })
            .await;
        }
        crate::service::listing::relist_stale(&state.db, false, 100)
            .await
            .expect("listed");

        // Two spellings of one slug: one page, under the more common.
        let page = page_of(&state, FacetKind::Network, "canal", "en")
            .await
            .expect("a page");
        assert_eq!(
            (page.slug.as_str(), page.name.as_str()),
            ("canal", "Canal+")
        );
        assert_eq!(page.count, 2);
        assert_eq!(page.filter["network"], "Canal+");
        let page = page_of(&state, FacetKind::Network, "HBO", "en")
            .await
            .expect("a page");
        assert_eq!((page.slug.as_str(), page.count), ("hbo", 1));
        assert!(matches!(
            page_of(&state, FacetKind::Network, "nobody", "en").await,
            Err(AppError::NotFound)
        ));
    }

    #[tokio::test]
    async fn a_year_and_a_decade_are_pages_of_their_own() {
        let state = super::super::testing::server().await;
        for (title, year) in [("Early", 1994), ("Late", 1999), ("Next", 2001)] {
            stored(&state, MediaKind::Movie, title, |w| w.year = Some(year)).await;
        }
        crate::service::listing::relist_stale(&state.db, false, 100)
            .await
            .expect("listed");

        let year = page_of(&state, FacetKind::Year, "1999", "en")
            .await
            .expect("a page");
        assert_eq!((year.slug.as_str(), year.count), ("1999", 1));
        assert_eq!(year.filter["yearFrom"], "1999");
        assert_eq!(year.filter["yearTo"], "1999");

        // By any of its years, under its first.
        let decade = page_of(&state, FacetKind::Decade, "1997", "en")
            .await
            .expect("a page");
        assert_eq!((decade.slug.as_str(), decade.count), ("1990", 2));
        assert_eq!(decade.filter["yearTo"], "1999");

        for asked in ["19999", "abcd", "1700", " "] {
            assert!(
                matches!(
                    page_of(&state, FacetKind::Year, asked, "en").await,
                    Err(AppError::NotFound)
                ),
                "{asked}"
            );
        }
    }

    #[tokio::test]
    async fn two_works_side_by_side_say_who_they_share() {
        let state = super::super::testing::server().await;
        let heat = stored(&state, MediaKind::Movie, "Heat", |w| {
            w.runtime = Some(170);
            w.ratings = rated(8.3);
            w.credits = vec![
                credited(CreditType::Actor, "Al Pacino", 1158, 0),
                credited(CreditType::Actor, "Robert De Niro", 380, 1),
                credited(CreditType::Director, "Michael Mann", 638, 0),
            ];
        })
        .await;
        let crime_story = stored(&state, MediaKind::Series, "Crime Story", |w| {
            // As a source gives them: a refresh writes only those.
            w.seasons = (0..3)
                .map(|n| {
                    let mut season = crate::db::repo::child::blank_season(n);
                    season.is_manual = false;
                    season
                })
                .collect();
            w.credits = vec![
                credited(CreditType::Actor, "Dennis Farina", 1, 0),
                credited(CreditType::Producer, "Michael Mann", 638, 3),
                credited(CreditType::Actor, "Al Pacino", 1158, 9),
            ];
        })
        .await;

        let compared = |a: &str, b: &str| {
            compare(
                State(state.clone()),
                Extension(Identity::Visitor),
                Query(CompareQuery {
                    a: a.into(),
                    b: b.into(),
                    language: None,
                }),
            )
        };
        let Json(both) = compared(&heat.id, &crime_story.id).await.expect("compared");
        assert_eq!(both.first.work.title, "Heat");
        assert_eq!(both.first.work.runtime, Some(170));
        assert_eq!((both.first.seasons, both.first.episodes), (None, None));
        // Specials aside.
        assert_eq!(both.second.seasons, Some(2));
        assert!(both.second.work.credits.is_empty() && both.second.work.seasons.is_empty());

        // Who made them first, then by their billing.
        let shared: Vec<&str> = both.shared.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(shared, ["Michael Mann", "Al Pacino"]);
        assert_eq!(both.shared[0].first[0].credit_type, CreditType::Director);
        assert_eq!(both.shared[0].second[0].credit_type, CreditType::Producer);
        assert_eq!(
            both.shared[1].first[0].character.as_deref(),
            Some("Al Pacino's part")
        );

        assert!(matches!(
            compared(&heat.id, &heat.id).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            compared(&heat.id, "not-an-id").await,
            Err(AppError::NotFound)
        ));
    }
}
