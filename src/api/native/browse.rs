//! Reading the catalogue across works: what it can be narrowed by, what airs
//! and comes out when, and who is in it.
//!
//! Visitors may read all three when browsing is public — they are the
//! catalogue seen from another side, not how the server is run.

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo::{self, item::Facets},
    domain::{CreditType, Episode, MediaItem, MediaKind},
    error::{AppError, AppResult},
    providers::tmdb,
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
        .routes(routes!(person))
}

// ─── facets ──────────────────────────────────────────────────────────────────

/// What the catalogue can be narrowed by next, with how many works each
/// choice would leave.
///
/// Takes the list's own filters and counts under them — see
/// [`repo::item::facets`] for how each is counted — and over what this caller
/// may see: an adult title's genre is not counted for somebody who would never
/// be shown it.
#[utoipa::path(
    get, path = "/facets", tag = TAG,
    params(super::items::ListQuery),
    responses(
        (status = 200, body = Facets),
        (status = 400, description = "A filter was not one the list understands"),
    ),
)]
async fn facets(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<super::items::ListQuery>,
) -> AppResult<Json<Facets>> {
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
        && let Ok(facets) = serde_json::from_str::<Facets>(&cached)
    {
        return Ok(Json(facets));
    }

    let found = repo::item::facets(&state.db, &filters).await?;
    if found.total > 0
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
    /// An episode, as `episodes` lists it. Boxed: an episode is four times
    /// a release, and a window holds hundreds of either.
    Episode(Box<Airing>),
    /// A film's release: in cinemas, digitally, on disc.
    Release(Release),
}

#[derive(Serialize, ToSchema)]
pub struct Calendar {
    /// Deprecated: the episodes alone, earliest first — the schedule as it
    /// was answered before it held films, which `entries` repeats beside
    /// them. Kept for whoever reads it so, and to be removed in a later
    /// version.
    #[schema(deprecated)]
    pub episodes: Vec<Airing>,
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

    let episodes = entries
        .iter()
        .filter_map(|entry| match entry {
            CalendarEntry::Episode(a) => Some(Airing::clone(a)),
            CalendarEntry::Release(_) => None,
        })
        .collect();

    Ok(Calendar {
        episodes,
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

// ─── people ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PersonQuery {
    /// Titles in this language, where a translation is held.
    pub language: Option<String>,
}

/// One part a person played, or one job they did, on one work.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub work_id: String,
    pub credit_type: CreditType,
    /// The character, or the job title for crew.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
}

/// Somebody as TMDB knows them, beside what this catalogue holds of theirs.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biography: Option<String>,
    /// `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deathday: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_of_birth: Option<String>,
    /// What TMDB files them under: `Acting`, `Directing`, `Writing`, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_for: Option<String>,
    /// As TMDB has it — 1 a woman, 2 a man, 3 non-binary — for a language
    /// that agrees its participles. Absent where TMDB does not know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gender: Option<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_known_as: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    /// A few portraits, in TMDB's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photos: Vec<String>,
}

/// The most portraits a page is given.
const MOST_PHOTOS: usize = 6;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub tmdb_id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Newest work first.
    pub roles: Vec<Role>,
    pub works: Vec<MediaItem>,
    /// What TMDB says of them, when it is configured and answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<PersonDetails>,
}

/// Somebody's work, as far as this catalogue holds it.
///
/// By TMDB's person id, which is how TMDB files cast and crew; TheTVDB's
/// credits carry none and are not found here.
#[utoipa::path(
    get, path = "/people/{tmdb_id}", tag = TAG,
    params(("tmdb_id" = i64, Path, description = "The person's TMDB id"), PersonQuery),
    responses(
        (status = 200, body = Person),
        (status = 404, description = "Nobody by that id is credited on a work held here"),
    ),
)]
async fn person(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(tmdb_id): Path<i64>,
    Query(query): Query<PersonQuery>,
) -> AppResult<Json<Person>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let credits = repo::item::person_credits(&state.db, tmdb_id, adult).await?;

    let Some(first) = credits.first() else {
        return Err(AppError::NotFound);
    };

    let name = first.credit.person_name.clone();
    let locale = service::language::tmdb_locale(
        language.as_deref(),
        &state.language(identity.client_id(), identity.peer_id()),
    );
    let details = details_for(&state, tmdb_id, &locale).await;
    let image = credits
        .iter()
        .find_map(|c| c.credit.image.as_deref().map(|u| state.media.localized(u)))
        .or_else(|| details.as_ref().and_then(|d| d.photos.first().cloned()));

    let mut ids: Vec<String> = Vec::new();
    for c in &credits {
        if !ids.contains(&c.media_id) {
            ids.push(c.media_id.clone());
        }
    }

    let mut works = repo::item::by_ids(&state.db, &ids).await?;
    service::apply_overrides(&state, &mut works).await?;
    repo::item::load_artwork(&state.db, &mut works).await?;
    service::overlay_imdb_many(&state, &mut works).await;
    if let Some(language) = language.as_deref() {
        for work in &mut works {
            service::language::apply_shallow(&state, work, language);
        }
    }

    for work in &mut works {
        state.media.localize(work);
        service::as_card(work);
    }
    service::redact_for_reader(&identity, &mut works);

    // In the order the credits came, newest work first.
    works.sort_by_key(|w| ids.iter().position(|id| id == &w.id));

    let roles = credits
        .into_iter()
        .map(|c| Role {
            work_id: c.media_id,
            credit_type: c.credit.credit_type,
            character: c.credit.character_name,
        })
        .collect();

    Ok(Json(Person {
        tmdb_id,
        name,
        image,
        roles,
        works,
        details,
    }))
}

/// What TMDB says of somebody, from the day-long cache or fetched — in the
/// reader's language, `locale` ([`service::language::tmdb_locale`]: the
/// value is a cache key and a request parameter, and whoever reads the page
/// chose it), with the English biography where that language has none.
/// Nothing when TMDB is not configured or did not answer, which costs the
/// page its biography and nothing else.
async fn details_for(state: &AppState, tmdb_id: i64, locale: &str) -> Option<PersonDetails> {
    if !state.tmdb.is_configured() {
        return None;
    }
    let mut details = person_details(&fetch_person(state, tmdb_id, locale).await?);
    if details.biography.is_none()
        && !locale.to_ascii_lowercase().starts_with("en")
        && let Some(english) = fetch_person(state, tmdb_id, "en-US").await
    {
        details.biography = person_details(&english).biography;
    }
    Some(details)
}

async fn fetch_person(state: &AppState, tmdb_id: i64, language: &str) -> Option<Value> {
    let key = format!("person:{tmdb_id}:{language}");
    let fetched = super::recommend::cached_value(state, &key, || async {
        Ok(state
            .tmdb
            .person(tmdb_id, language)
            .await?
            .unwrap_or(Value::Null))
    })
    .await;
    match fetched {
        Ok(Value::Null) => None,
        Ok(value) => Some(value),
        Err(e) => {
            tracing::debug!(tmdb_id, error = ?e, "TMDB could not say who this is");
            None
        }
    }
}

/// TMDB's record of somebody, read down to what the page shows. A blank
/// where TMDB holds nothing — an untranslated biography, no homepage — is
/// nothing, not an empty line.
fn person_details(raw: &Value) -> PersonDetails {
    let text = |key: &str| {
        raw.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let names = |value: Option<&Value>| -> Vec<String> {
        value
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let photos = raw
        .pointer("/images/profiles")
        .and_then(Value::as_array)
        .map(|profiles| {
            profiles
                .iter()
                .filter_map(|p| p.get("file_path").and_then(Value::as_str))
                .filter(|path| path.starts_with('/'))
                .take(MOST_PHOTOS)
                .map(tmdb::image_url)
                .collect()
        })
        .unwrap_or_default();
    PersonDetails {
        biography: text("biography"),
        birthday: text("birthday"),
        deathday: text("deathday"),
        place_of_birth: text("place_of_birth"),
        known_for: text("known_for_department"),
        gender: raw
            .get("gender")
            .and_then(Value::as_u64)
            .filter(|g| (1..=3).contains(g))
            .map(|g| g as u8),
        also_known_as: names(raw.get("also_known_as")),
        homepage: text("homepage").filter(|h| h.starts_with("http")),
        imdb_id: text("imdb_id").or_else(|| {
            raw.pointer("/external_ids/imdb_id")
                .and_then(Value::as_str)
                .map(String::from)
        }),
        photos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn somebody_is_read_off_tmdb_s_record_down_to_what_is_shown() {
        let profiles: Vec<Value> = (0..9)
            .map(|n| serde_json::json!({ "file_path": format!("/p{n}.jpg") }))
            .collect();
        let raw = serde_json::json!({
            "id": 17419,
            "name": "Bryan Cranston",
            "biography": "  ",
            "birthday": "1956-03-07",
            "deathday": null,
            "place_of_birth": "Hollywood, Los Angeles, California, USA",
            "known_for_department": "Acting",
            "gender": 2,
            "also_known_as": ["Bryan Lee Cranston", " ", "Lee Stone"],
            "homepage": "",
            "imdb_id": null,
            "external_ids": { "imdb_id": "nm0186505", "wikidata_id": "Q23547" },
            "images": { "profiles": profiles }
        });
        let details = person_details(&raw);
        assert_eq!(details.biography, None, "blank is nothing");
        assert_eq!(details.birthday.as_deref(), Some("1956-03-07"));
        assert_eq!(details.deathday, None);
        assert_eq!(
            details.place_of_birth.as_deref(),
            Some("Hollywood, Los Angeles, California, USA")
        );
        assert_eq!(details.known_for.as_deref(), Some("Acting"));
        assert_eq!(details.gender, Some(2));
        assert_eq!(details.also_known_as, ["Bryan Lee Cranston", "Lee Stone"]);
        // TMDB's 0 is "not set", which is nothing to agree with.
        let unknown = person_details(&serde_json::json!({ "gender": 0 }));
        assert_eq!(unknown.gender, None);
        assert_eq!(details.homepage, None);
        assert_eq!(details.imdb_id.as_deref(), Some("nm0186505"));
        assert_eq!(details.photos.len(), MOST_PHOTOS);
        assert!(details.photos[0].starts_with("https://"));
        assert!(details.photos[0].ends_with("/p0.jpg"));
    }

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

        // The episodes alone stay where whoever read them before reads them,
        // in the shape they had; the entries say which kind each is.
        let json = serde_json::to_value(&calendar).expect("serialised");
        assert_eq!(json["episodes"].as_array().map(Vec::len), Some(1));
        assert!(json["episodes"][0].get("kind").is_none());
        assert_eq!(json["episodes"][0]["workId"], dark.id.as_str());
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

        for identity in [Identity::Visitor, Identity::Anonymous] {
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
                "{}",
                identity.label()
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
}
