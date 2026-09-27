//! Reading the catalogue across works: what it can be narrowed by, what airs
//! when, and who is in it.
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
    domain::{CreditType, Episode, MediaItem},
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
    /// Episode titles in this language, where a translation is held.
    pub language: Option<String>,
}

/// One episode of the window, and which of the listed works it belongs to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Airing {
    pub work_id: String,
    pub episode: Episode,
}

#[derive(Serialize, ToSchema)]
pub struct Calendar {
    /// Earliest first.
    pub episodes: Vec<Airing>,
    /// Every work the episodes belong to, drawn as a list is: artwork and
    /// scores, no episodes.
    pub works: Vec<MediaItem>,
    /// The window held more episodes than one answer carries, and the latest
    /// are missing: ask for a shorter one.
    pub truncated: bool,
}

/// The episodes that air in a window, across every series.
///
/// By the moment each airs, in UTC — where no provider knew the time, midnight
/// UTC on its date, which is what Sonarr is given. The window is the caller's
/// to choose, so a day is the caller's own day wherever it is.
#[utoipa::path(
    get, path = "/calendar", tag = TAG,
    params(CalendarQuery),
    responses(
        (status = 200, body = Calendar),
        (status = 400, description = "The window is not two instants, or is longer than 62 days"),
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

    let language = query.language.as_deref().filter(|l| !l.is_empty());
    Ok(Json(window(&state, &identity, from, to, language).await?))
}

/// The schedule of a window, as the calendar answers it and the feeds repeat
/// it: every episode airing from `from` to `to`, and the works they belong
/// to, with the overrides and the reader's language applied.
pub(super) async fn window(
    state: &AppState,
    identity: &Identity,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
    language: Option<&str>,
) -> AppResult<Calendar> {
    let stamp = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let (airing, truncated) =
        repo::item::airing(&state.db, &stamp(from), &stamp(to), adult).await?;

    let ids: Vec<String> = {
        let mut ids: Vec<String> = airing.iter().map(|a| a.media_id.clone()).collect();
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
    // Before the language: the stored translations the overlay reads come
    // with the artwork, and without them every series kept its own title.
    repo::item::load_artwork(&state.db, &mut works).await?;
    if let Some(language) = language {
        for work in &mut works {
            service::language::apply_stored(state, work, language).await?;
        }
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

    let (from, to) = (stamp(from), stamp(to));
    let mut episodes: Vec<Airing> = works
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
        .filter(|a| aired_at(&a.episode).is_some_and(|t| t >= from && t < to))
        .collect();

    // Overrides may have moved some, so the order is settled here: by the
    // moment, then the series, then the episode, so a season dropped at once
    // is listed in its own order.
    episodes.sort_by_cached_key(|a| {
        (
            aired_at(&a.episode),
            titles.get(&a.work_id).cloned().unwrap_or_default(),
            a.work_id.clone(),
            a.episode.season_number,
            a.episode.episode_number,
        )
    });

    Ok(Calendar {
        episodes,
        works,
        truncated,
    })
}

/// When an episode airs, as the calendar compares it.
pub(super) fn aired_at(episode: &Episode) -> Option<String> {
    episode
        .air_date_utc
        .clone()
        .or_else(|| episode.air_date.as_ref().map(|d| format!("{d}T00:00:00Z")))
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
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let credits = repo::item::person_credits(&state.db, tmdb_id, adult).await?;

    let Some(first) = credits.first() else {
        return Err(AppError::NotFound);
    };

    let name = first.credit.person_name.clone();
    let details = details_for(&state, tmdb_id, query.language.as_deref()).await;
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
    if let Some(language) = query.language.as_deref().filter(|l| !l.is_empty()) {
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
/// reader's language, with the English biography where that language has
/// none. Nothing when TMDB is not configured or did not answer, which costs
/// the page its biography and nothing else.
async fn details_for(
    state: &AppState,
    tmdb_id: i64,
    language: Option<&str>,
) -> Option<PersonDetails> {
    if !state.tmdb.is_configured() {
        return None;
    }
    let language = language.and_then(tmdb_language).unwrap_or("en-US");
    let mut details = person_details(&fetch_person(state, tmdb_id, language).await?);
    if details.biography.is_none()
        && !language.to_ascii_lowercase().starts_with("en")
        && let Some(english) = fetch_person(state, tmdb_id, "en-US").await
    {
        details.biography = person_details(&english).biography;
    }
    Some(details)
}

/// A language as TMDB takes one — `fr`, `pt-BR` — and nothing else: the
/// value is a cache key and a request parameter, and whoever reads the
/// page chose it.
fn tmdb_language(asked: &str) -> Option<&str> {
    let asked = asked.trim();
    let (base, region) = asked.split_once('-').unwrap_or((asked, ""));
    let base_ok = matches!(base.len(), 2 | 3) && base.bytes().all(|b| b.is_ascii_lowercase());
    let region_ok =
        region.is_empty() || (region.len() == 2 && region.bytes().all(|b| b.is_ascii_uppercase()));
    (base_ok && region_ok).then_some(asked)
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
        assert_eq!(details.also_known_as, ["Bryan Lee Cranston", "Lee Stone"]);
        assert_eq!(details.homepage, None);
        assert_eq!(details.imdb_id.as_deref(), Some("nm0186505"));
        assert_eq!(details.photos.len(), MOST_PHOTOS);
        assert!(details.photos[0].starts_with("https://"));
        assert!(details.photos[0].ends_with("/p0.jpg"));
    }

    #[test]
    fn only_a_language_shaped_like_one_reaches_tmdb() {
        for fine in ["fr", "en-US", "pt-BR", "ast", " de "] {
            assert!(tmdb_language(fine).is_some(), "{fine:?}");
        }
        for odd in [
            "",
            "f",
            "FR",
            "fr-fr",
            "fr-FRA",
            "en_US",
            "../x",
            "fr-FR&x=1",
            "français",
        ] {
            assert_eq!(tmdb_language(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn an_episode_without_a_time_is_placed_at_midnight_utc() {
        let mut episode = crate::db::repo::child::blank_episode(1, 1);
        episode.air_date = Some("2026-09-24".into());
        episode.air_date_utc = None;
        assert_eq!(aired_at(&episode).as_deref(), Some("2026-09-24T00:00:00Z"));

        episode.air_date_utc = Some("2026-09-25T01:30:00Z".into());
        assert_eq!(aired_at(&episode).as_deref(), Some("2026-09-25T01:30:00Z"));
    }
}
