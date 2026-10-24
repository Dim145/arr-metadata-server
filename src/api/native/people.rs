//! The people the catalogue credits: an index of them, each one's page, and
//! their records as those who keep the catalogue put them right — entered by
//! hand, corrected, merged into one, split in two.
//!
//! A page is read by the record's id, or by the TMDB id the pages were filed
//! under before there were records (`/people/17419`), which answers the same.
//! What TMDB says of somebody — their life, their portraits — is asked the
//! first time their page is read, kept, and asked again once it is a month
//! old: never for everybody a refresh credits.

use std::collections::{BTreeSet, HashSet};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action, person::Refused},
    domain::{
        CreditType, MediaItem,
        person::{Job, Person, name_language},
    },
    error::{AppError, AppResult},
    providers::tmdb,
    service,
    state::AppState,
};

/// Filed with the rest of the catalogue in the documentation.
const TAG: &str = super::items::TAG;

/// How long what TMDB says of somebody is kept before it is asked again.
const KEPT_FOR_DAYS: i64 = 30;

/// The people an index page lists when nothing says how many.
const PAGE: i64 = 60;

/// The most one index page lists.
const MOST: i64 = 200;

/// The most portraits a page is given.
const MOST_PHOTOS: usize = 6;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(index, create))
        .routes(routes!(page, update))
        .routes(routes!(merge))
        .routes(routes!(split))
        .routes(routes!(attach))
}

// ─── the index ───────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct IndexQuery {
    /// `A` to `Z`, or `#` for the names that start with anything else —
    /// the name they sort by, its accent and case aside.
    pub letter: Option<String>,
    /// `acting`, `directing`, `writing` or `producing`: those who did it,
    /// with the works they did it on.
    pub job: Option<String>,
    /// `works` — the most works first, the default — or `name`.
    pub sort: Option<String>,
    /// For those who keep the catalogue: a part of a name, theirs or one they
    /// go by, accents and case aside.
    pub term: Option<String>,
    /// For those who keep the catalogue: everybody, credited on anything or
    /// not.
    #[serde(default)]
    pub all: bool,
    /// For those who keep the catalogue: only those known by no TMDB id.
    #[serde(default)]
    pub without_tmdb: bool,
    /// For those who keep the catalogue: only those who share a name — theirs
    /// or one they go by — with somebody else: the likely homonyms and the
    /// likely duplicates.
    #[serde(default)]
    pub homonyms: bool,
    /// From 1 to 200; 60 when left out.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub offset: Option<i64>,
}

/// A page of the index.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeoplePage {
    pub people: Vec<PersonCard>,
    /// How many there are, past this page.
    pub total: i64,
    /// The letters somebody is filed under, `#` among them, before the one
    /// asked for narrowed the list: the ones worth offering.
    pub letters: Vec<String>,
}

/// Somebody, as the index lists them.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonCard {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// How many works credit them — with the job asked for, when one was.
    pub works: i64,
    /// What they did on those works.
    pub jobs: Vec<Job>,
    /// For those who keep the catalogue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    /// Entered or put right by hand.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_manual: bool,
    /// The other names they go by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// In what order the index lists people.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sort {
    Works,
    Name,
}

/// The people the catalogue credits, as an index: by letter, by job, the
/// most credited first or in the order of their names.
///
/// Those credited on a work anybody may see — switched on, and not for
/// adults — by a credit no lock hides, each counted by those works, as their
/// records keep the count. Whoever keeps the catalogue may also ask for
/// everybody, those known by no TMDB id, and those who share a name: the
/// records worth a second look.
#[utoipa::path(
    get, path = "/people", tag = TAG,
    operation_id = "people_index",
    params(IndexQuery),
    responses(
        (status = 200, body = PeoplePage),
        (status = 400, description = "A letter, a job or an order this index does not know"),
    ),
)]
async fn index(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<IndexQuery>,
) -> AppResult<Response> {
    let job = query
        .job
        .as_deref()
        .map(str::trim)
        .filter(|j| !j.is_empty())
        .map(str::parse::<Job>)
        .transpose()
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let sort = match query.sort.as_deref().map(str::trim) {
        None | Some("" | "works") => Sort::Works,
        Some("name") => Sort::Name,
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unknown sort {other:?}: expected works or name"
            )));
        }
    };
    let letter = match query.letter.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(given) => {
            let mut chars = given.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() || c == '#' => {
                    Some(c.to_ascii_uppercase())
                }
                _ => {
                    return Err(AppError::BadRequest(
                        "a letter is one of A to Z, or #".into(),
                    ));
                }
            }
        }
    };
    let limit = query.limit.unwrap_or(PAGE).clamp(1, MOST);
    let offset = query.offset.unwrap_or(0).max(0);
    let keeper = identity.can_write();
    let term = query
        .term
        .as_deref()
        .map(crate::search::norm)
        .filter(|t| !t.is_empty() && keeper);
    let checking = keeper && (query.all || query.without_tmdb || query.homonyms || term.is_some());

    // Under the stamp every write moves on, by what was asked and who asks:
    // a page asked again is answered as it was.
    let key = format!(
        "people:{}:{keeper}:{checking}:{job:?}:{sort:?}:{letter:?}:{term:?}:{}:{}:{}:{limit}:{offset}",
        state.caches.stamp(),
        query.all,
        query.without_tmdb,
        query.homonyms,
    );
    if let Some(cached) = state.caches.searches.get(&key).await {
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            cached,
        )
            .into_response());
    }

    let indexed = if checking {
        let everybody = repo::person::everybody(&state.db).await?;
        let sharing = query
            .homonyms
            .then(|| repo::person::sharing_a_name(&everybody));
        let mut found: Vec<repo::person::Listed> = everybody
            .into_iter()
            .filter(|p| query.all || p.works > 0)
            .filter(|p| !query.without_tmdb || p.tmdb_id.is_none())
            .filter(|p| sharing.as_ref().is_none_or(|ids| ids.contains(&p.id)))
            .filter(|p| job.is_none_or(|job| p.jobs.contains(&job)))
            .filter(|p| {
                term.as_deref().is_none_or(|term| {
                    std::iter::once(&p.name)
                        .chain(&p.aliases)
                        .any(|name| crate::search::norm(name).contains(term))
                })
            })
            .collect();
        let letters: BTreeSet<char> = found.iter().map(|p| p.letter).collect();
        if let Some(letter) = letter {
            found.retain(|p| p.letter == letter);
        }
        // By name already; the most credited first keeps that order among
        // those credited as often.
        if sort == Sort::Works {
            found.sort_by_key(|p| std::cmp::Reverse(p.works));
        }
        repo::person::Indexed {
            total: found.len() as i64,
            people: found
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect(),
            letters: letters.into_iter().collect(),
        }
    } else {
        repo::person::index(
            &state.db,
            repo::person::Asked {
                job,
                letter,
                by_name: sort == Sort::Name,
                limit,
                offset,
            },
        )
        .await?
    };

    let page = PeoplePage {
        people: indexed
            .people
            .into_iter()
            .map(|p| PersonCard {
                id: p.id,
                name: p.name,
                tmdb_id: p.tmdb_id,
                image: p.image.as_deref().map(|url| state.media.localized(url)),
                works: p.works,
                jobs: p.jobs,
                imdb_id: p.imdb_id.filter(|_| keeper),
                tvdb_id: p.tvdb_id.filter(|_| keeper),
                is_manual: keeper && p.is_manual,
                aliases: if keeper { p.aliases } else { Vec::new() },
            })
            .collect(),
        total: indexed.total,
        letters: indexed.letters.into_iter().map(String::from).collect(),
    };
    if let Ok(encoded) = serde_json::to_string(&page) {
        state.caches.searches.insert(key, encoded).await;
    }
    Ok(Json(page).into_response())
}

// ─── a person's page ─────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PersonQuery {
    /// Titles, and the biography, in this language where one is held.
    pub language: Option<String>,
}

/// One part a person played, or one job they did, on one work — or on one
/// episode of it.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: what an account may do is a `Role` too, and utoipa keys
// schemas on the leaf name alone.
#[schema(as = PersonRole)]
pub struct Role {
    pub work_id: String,
    pub credit_type: CreditType,
    /// The character, or the job title for crew.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    /// The credit, which those who keep the catalogue move from one person
    /// to another by.
    pub credit_id: String,
    /// The episode it is on, where it is on one — a guest's, an episode's
    /// director's — with its title and the day it aired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    /// `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
}

/// Somebody as TMDB knows them, beside what this catalogue holds of theirs.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
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

impl PersonDetails {
    fn is_empty(&self) -> bool {
        self.biography.is_none()
            && self.birthday.is_none()
            && self.deathday.is_none()
            && self.place_of_birth.is_none()
            && self.known_for.is_none()
            && self.gender.is_none()
            && self.also_known_as.is_empty()
            && self.homepage.is_none()
            && self.imdb_id.is_none()
            && self.photos.is_empty()
    }
}

/// Somebody, and the part of their work this catalogue holds.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Person)]
pub struct PersonPage {
    /// Their record's id: their page is `/people/{id}`.
    pub id: String,
    /// Their TMDB id, where they are known by one: `/people/{tmdbId}` is
    /// their page too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    pub name: String,
    /// Their name in the script of their own language, where it is another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    /// The language that name is in, as a page's `lang` takes it: `ja`.
    /// None where neither its script nor their works say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_name_lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The other names they go by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Entered or put right by hand.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_manual: bool,
    /// Every credit on a work the caller may see, the newest work first: an
    /// episode's under the work it is of.
    pub roles: Vec<Role>,
    /// Those works, as cards, in the same order.
    pub works: Vec<MediaItem>,
    /// What TMDB says of them, once it has been asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<PersonDetails>,
    /// The record as it is kept — every field as `PUT /people/{id}` takes
    /// it, the portrait's address at its source, the other names with where
    /// each came from — for whoever may put it right; nobody else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Person>,
}

/// Somebody, and their work as far as this catalogue holds it.
///
/// By their record's id, or by their TMDB id — every digit — as the pages
/// were filed before there were records. A record merged into another
/// answers as that one. What TMDB says of them is asked the first time the
/// page is read, kept, and asked again in the background once it is a month
/// old; the biography is in the language asked for where TMDB has one.
#[utoipa::path(
    get, path = "/people/{id}", tag = TAG,
    operation_id = "people_page",
    params(
        ("id" = String, Path, description = "The person's record id, or their TMDB id"),
        PersonQuery,
    ),
    responses(
        (status = 200, body = PersonPage),
        (status = 404, description = "Nobody by that id is credited on a work the caller may see"),
    ),
)]
async fn page(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(address): Path<String>,
    Query(query): Query<PersonQuery>,
) -> AppResult<Json<PersonPage>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let Some(person) = repo::person::find(&state.db, &address).await? else {
        return Err(AppError::NotFound);
    };
    let credits = repo::person::credits(&state.db, &person.id, adult).await?;
    // Somebody credited on nothing the caller may see is nobody to them;
    // whoever keeps the catalogue opens the record whatever it holds.
    if credits.is_empty() && !identity.can_write() {
        return Err(AppError::NotFound);
    }

    let person = refreshed(&state, person).await;
    let locale = service::language::tmdb_locale(
        language.as_deref(),
        &state.language(identity.client_id(), identity.peer_id()),
    );
    let stored = service::language::tmdb_locale(None, &state.language(None, None));
    let mut details = details_of(&state, &person);
    // A biography in the reader's language, where TMDB has one: kept a day,
    // as TMDB's answers are, and never written over the one the record keeps.
    if let (Some(tmdb_id), true) = (person.tmdb_id, locale != stored)
        && state.tmdb.is_configured()
        && let Some(raw) = fetch_person(&state, tmdb_id, &locale).await
        && let Some(biography) = text(&raw, "biography")
    {
        details.biography = Some(biography);
    }

    let mut ids: Vec<String> = Vec::new();
    for credited in &credits {
        if !ids.contains(&credited.media_id) {
            ids.push(credited.media_id.clone());
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
    works.sort_by_key(|w| ids.iter().position(|id| id == &w.id));

    let languages = if person.original_name.is_some() {
        repo::person::languages(&state.db, &person.id).await?
    } else {
        Vec::new()
    };
    let roles = credits
        .into_iter()
        .map(|credited| Role {
            work_id: credited.media_id,
            credit_type: credited.credit.credit_type,
            character: credited.credit.character_name,
            credit_id: credited.credit.id,
            season_number: credited.season_number,
            episode_number: credited.episode_number,
            episode_title: credited.episode_title,
            air_date: credited.air_date,
        })
        .collect();
    let image = person
        .image
        .as_deref()
        .map(|url| state.media.localized(url))
        .or_else(|| details.photos.first().cloned());

    let record = identity.can_write().then(|| person.clone());
    Ok(Json(PersonPage {
        record,
        id: person.id.clone(),
        tmdb_id: person.tmdb_id,
        tvdb_id: person.tvdb_id,
        original_name_lang: person
            .original_name
            .as_deref()
            .and_then(|name| name_language(name, &languages)),
        original_name: person.original_name.clone(),
        image,
        aliases: person.aliases.iter().map(|a| a.name.clone()).collect(),
        is_manual: person.is_manual,
        name: person.name,
        roles,
        works,
        details: (!details.is_empty()).then_some(details),
    }))
}

/// What the page says of somebody's life, from their record.
fn details_of(state: &AppState, person: &Person) -> PersonDetails {
    PersonDetails {
        biography: person.biography.clone(),
        birthday: person.birthday.clone(),
        deathday: person.deathday.clone(),
        place_of_birth: person.place_of_birth.clone(),
        known_for: person.known_for.clone(),
        gender: person.gender,
        // The name in their own script is shown beside their name already.
        also_known_as: person
            .aliases
            .iter()
            .filter(|a| a.source == "tmdb" && Some(&a.name) != person.original_name.as_ref())
            .map(|a| a.name.clone())
            .collect(),
        homepage: person.homepage.clone(),
        imdb_id: person.imdb_id.clone(),
        photos: person
            .photos
            .iter()
            .map(|url| state.media.localized(url))
            .collect(),
    }
}

/// The records being asked of TMDB in the background, each once at a time.
static REFRESHING: std::sync::LazyLock<parking_lot::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(HashSet::new()));

/// Somebody with what TMDB says of them: asked now, and waited for, the first
/// time their page is read; asked in the background once what is kept is a
/// month old, the page drawn meanwhile with what is kept. Somebody known by
/// no TMDB id, or a server with no TMDB key, is as kept.
async fn refreshed(state: &AppState, person: Person) -> Person {
    let Some(tmdb_id) = person.tmdb_id else {
        return person;
    };
    if !state.tmdb.is_configured() {
        return person;
    }
    let asked = person
        .refreshed_at
        .as_deref()
        .and_then(crate::db::parse_rfc3339);
    let old = |at: chrono::DateTime<chrono::Utc>| {
        chrono::Utc::now() - at > chrono::TimeDelta::days(KEPT_FOR_DAYS)
    };
    match asked {
        None => {
            if refresh(state, &person.id, tmdb_id).await
                && let Ok(Some(fresh)) = repo::person::get(&state.db, &person.id).await
            {
                return fresh;
            }
            person
        }
        Some(at) if old(at) => {
            if REFRESHING.lock().insert(person.id.clone()) {
                let (state, id) = (state.clone(), person.id.clone());
                tokio::spawn(async move {
                    refresh(&state, &id, tmdb_id).await;
                    REFRESHING.lock().remove(&id);
                });
            }
            person
        }
        Some(_) => person,
    }
}

/// Ask TMDB about somebody, in the language the catalogue is kept in — the
/// biography in English where that language has none — and keep what it
/// says. Whether it answered.
async fn refresh(state: &AppState, id: &str, tmdb_id: i64) -> bool {
    let locale = service::language::tmdb_locale(None, &state.language(None, None));
    let Some(raw) = fetch_person(state, tmdb_id, &locale).await else {
        return false;
    };
    let mut details = read_details(&raw);
    if details.biography.is_none()
        && !locale.to_ascii_lowercase().starts_with("en")
        && let Some(english) = fetch_person(state, tmdb_id, "en-US").await
    {
        details.biography = read_details(&english).biography;
    }
    // Their name in their own script, where TMDB lists one among their other
    // names and the works they are credited on say which language it is in.
    let languages = repo::person::languages(&state.db, id)
        .await
        .unwrap_or_default();
    details.original_name =
        crate::domain::person::original_name(&details.also_known_as, &languages);

    let before = repo::person::get(&state.db, id).await.ok().flatten();
    if let Err(e) = repo::person::write_details(&state.db, id, &details).await {
        tracing::warn!(
            person = id,
            error = format_args!("{e:#}"),
            "could not keep what TMDB says of somebody"
        );
        return false;
    }
    let after = repo::person::get(&state.db, id).await.ok().flatten();
    let renamed = match (&before, &after) {
        (Some(before), Some(after)) => {
            before.name != after.name
                || before.image != after.image
                || before.aliases != after.aliases
                || before.original_name != after.original_name
        }
        _ => false,
    };
    if renamed {
        // Found by its new names, and listed with its portrait.
        if let Err(e) =
            repo::search::index_people(&state.db, &BTreeSet::from([id.to_string()])).await
        {
            tracing::warn!(
                person = id,
                error = format_args!("{e:#}"),
                "could not write somebody's new names into the search index"
            );
        }
        state.caches.bump_epoch().await;
    }
    true
}

/// TMDB's record of somebody in a language, from the day-long cache or
/// fetched — `locale` is a cache key and a request parameter, read with
/// [`service::language::tmdb_locale`]. Nothing when TMDB did not answer.
async fn fetch_person(state: &AppState, tmdb_id: i64, locale: &str) -> Option<Value> {
    let key = format!("person:{tmdb_id}:{locale}");
    let fetched = super::recommend::cached_value(state, &key, || async {
        Ok(state
            .tmdb
            .person(tmdb_id, locale)
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

/// A text of TMDB's record, nothing where it is blank.
fn text(raw: &Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// TMDB's record of somebody, read down to what their record keeps. A blank
/// where TMDB holds nothing — an untranslated biography, no homepage — is
/// nothing, not an empty line.
fn read_details(raw: &Value) -> repo::person::Details {
    let names = raw
        .get("also_known_as")
        .and_then(Value::as_array)
        .map(|names| {
            let mut seen: Vec<String> = Vec::new();
            for name in names
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty())
            {
                if !seen.iter().any(|s| s == name) {
                    seen.push(name.to_string());
                }
            }
            seen
        })
        .unwrap_or_default();
    let photos: Vec<String> = raw
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
    repo::person::Details {
        name: text(raw, "name"),
        imdb_id: text(raw, "imdb_id")
            .or_else(|| {
                raw.pointer("/external_ids/imdb_id")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .filter(|id| is_imdb_person(id)),
        gender: raw
            .get("gender")
            .and_then(Value::as_u64)
            .filter(|g| (1..=3).contains(g))
            .map(|g| g as u8),
        birthday: text(raw, "birthday"),
        deathday: text(raw, "deathday"),
        place_of_birth: text(raw, "place_of_birth"),
        biography: text(raw, "biography"),
        image: text(raw, "profile_path")
            .filter(|path| path.starts_with('/'))
            .map(|path| tmdb::image_url(&path)),
        photos,
        known_for: text(raw, "known_for_department"),
        homepage: text(raw, "homepage").filter(|h| h.starts_with("http")),
        popularity: raw.get("popularity").and_then(Value::as_f64),
        also_known_as: names,
        original_name: None,
    }
}

/// Whether this is how IMDb files a person: `nm` and digits.
fn is_imdb_person(id: &str) -> bool {
    id.strip_prefix("nm")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

// ─── put right by hand ───────────────────────────────────────────────────────

/// Somebody as they are entered, or put right, by hand.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnteredPerson {
    pub name: String,
    #[serde(default)]
    pub tmdb_id: Option<i64>,
    /// `nm0186505`.
    #[serde(default)]
    pub imdb_id: Option<String>,
    #[serde(default)]
    pub tvdb_id: Option<i64>,
    /// What the index files and sorts them under, where it is not their
    /// name: `Toro, Guillermo del`.
    #[serde(default)]
    pub sort_name: Option<String>,
    /// Their name in their own script: 宮崎駿.
    #[serde(default)]
    pub original_name: Option<String>,
    /// 1 a woman, 2 a man, 3 non-binary.
    #[serde(default)]
    pub gender: Option<u8>,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub birthday: Option<String>,
    #[serde(default)]
    pub deathday: Option<String>,
    #[serde(default)]
    pub place_of_birth: Option<String>,
    #[serde(default)]
    pub biography: Option<String>,
    /// Their portrait: an address a client can fetch.
    #[serde(default)]
    pub image: Option<String>,
    /// The other names they go by, typed by hand: those any other way —
    /// their credits', TMDB's, a merged record's — are kept as they are.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Blank is nothing.
fn given(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

impl EnteredPerson {
    /// What is entered, checked: a name, ids that are ids, days that are
    /// days, a portrait a client can fetch.
    fn checked(&self) -> AppResult<repo::person::Entered> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("name must not be empty".into()));
        }
        for (what, id) in [("tmdbId", self.tmdb_id), ("tvdbId", self.tvdb_id)] {
            if id.is_some_and(|id| id <= 0) {
                return Err(AppError::BadRequest(format!("{what} is a positive number")));
            }
        }
        let imdb_id = given(&self.imdb_id);
        if imdb_id.as_deref().is_some_and(|id| !is_imdb_person(id)) {
            return Err(AppError::BadRequest(
                "imdbId is how IMDb files a person: nm and digits".into(),
            ));
        }
        if self.gender.is_some_and(|g| !(1..=3).contains(&g)) {
            return Err(AppError::BadRequest(
                "gender is 1 (a woman), 2 (a man) or 3 (non-binary)".into(),
            ));
        }
        for (what, day) in [("birthday", &self.birthday), ("deathday", &self.deathday)] {
            if let Some(day) = given(day)
                && chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").is_err()
            {
                return Err(AppError::BadRequest(format!("{what} is a day, YYYY-MM-DD")));
            }
        }
        let image = given(&self.image);
        if let Some(url) = &image
            && ((!url.starts_with("http://") && !url.starts_with("https://"))
                || crate::outbound::names_internal_host(url)
                || crate::media::Media::key_in(url).is_some())
        {
            return Err(AppError::BadRequest(
                "image is an address a client can fetch, the source's rather than this server's copy"
                    .into(),
            ));
        }
        Ok(repo::person::Entered {
            name: name.to_string(),
            tmdb_id: self.tmdb_id,
            imdb_id,
            tvdb_id: self.tvdb_id,
            sort_name: given(&self.sort_name),
            original_name: given(&self.original_name),
            gender: self.gender,
            birthday: given(&self.birthday),
            deathday: given(&self.deathday),
            place_of_birth: given(&self.place_of_birth),
            biography: given(&self.biography),
            image,
            aliases: self
                .aliases
                .iter()
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect(),
        })
    }
}

/// What became of a record: its id.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonId {
    pub id: String,
}

/// The refusal a write of a record meets, said.
fn refused(why: Refused) -> AppError {
    match why {
        Refused::TmdbHeld(holder) => AppError::Conflict(format!(
            "that TMDB id is another person's: {holder} — put that record right, or merge the two"
        )),
        Refused::Gone => AppError::NotFound,
        Refused::Same => AppError::BadRequest("a person cannot be merged into themselves".into()),
        Refused::NotTheirs(credit) if credit.is_empty() => {
            AppError::BadRequest("name at least one credit".into())
        }
        Refused::NotTheirs(credit) => {
            AppError::BadRequest(format!("credit {credit} is not one of this person's"))
        }
    }
}

/// After a write: the people's rows of the search index written again, the
/// lists and searches drawn so far set aside, and each work whose credits
/// were filed anew read afresh — a work's page names the person each credit
/// is of.
async fn after(state: &AppState, people: &[&str], works: &[String]) {
    let ids: BTreeSet<String> = people.iter().map(|id| (*id).to_string()).collect();
    if let Err(e) = repo::search::index_people(&state.db, &ids).await {
        tracing::warn!(
            error = format_args!("{e:#}"),
            "could not write people's rows of the search index"
        );
    }
    for work in works {
        state.caches.touched(work).await;
    }
    state.caches.bump_epoch().await;
}

/// Write what was done to a record to the audit trail.
async fn trail(
    state: &AppState,
    identity: &Identity,
    ip: &ClientIp,
    action: Action,
    target: &str,
    detail: String,
) {
    audit::record(
        state,
        Event {
            identity: Some(identity),
            ip,
            action,
            target: Some(target),
            detail: Some(&detail),
        },
    )
    .await;
}

/// Enter somebody by hand.
///
/// A record of its own, known by the ids given: credits a source gives them
/// under one of those ids are filed under it from then on, and what TMDB
/// says of them fills only what is left empty here.
#[utoipa::path(
    post, path = "/people", tag = TAG,
    operation_id = "people_create",
    request_body = EnteredPerson,
    responses(
        (status = 201, body = PersonId),
        (status = 400, description = "No name, or an id, a day or an address that is not one"),
        (status = 403, description = "The caller may not write"),
        (status = 409, description = "Another record holds that TMDB id"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<EnteredPerson>,
) -> AppResult<(StatusCode, Json<PersonId>)> {
    identity.require_write()?;
    let entered = request.checked()?;
    let id = repo::person::create(&state.db, &entered)
        .await?
        .map_err(refused)?;
    after(&state, &[&id], &[]).await;
    trail(
        &state,
        &identity,
        &ip,
        Action::PersonCreated,
        &id,
        format!("entered {}", entered.name),
    )
    .await;
    Ok((StatusCode::CREATED, Json(PersonId { id })))
}

/// Put somebody right by hand.
///
/// Every field as given — one left out is cleared — and the record marked as
/// put right: what TMDB says of them afterwards fills only what is empty.
#[utoipa::path(
    put, path = "/people/{id}", tag = TAG,
    operation_id = "people_update",
    params(("id" = String, Path, description = "The person's record id")),
    request_body = EnteredPerson,
    responses(
        (status = 204, description = "Put right"),
        (status = 400, description = "No name, or an id, a day or an address that is not one"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such record, or one merged into another"),
        (status = 409, description = "Another record holds that TMDB id"),
    ),
)]
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<EnteredPerson>,
) -> AppResult<StatusCode> {
    identity.require_write()?;
    let entered = request.checked()?;
    repo::person::update(&state.db, &id, &entered)
        .await?
        .map_err(refused)?;
    after(&state, &[&id], &[]).await;
    trail(
        &state,
        &identity,
        &ip,
        Action::PersonUpdated,
        &id,
        format!("put right as {}", entered.name),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Into whom a record is merged.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MergePerson {
    /// The record that is kept.
    pub into: String,
}

/// Merge a record into another: two records of one person.
///
/// Every credit filed under it is filed under the other, which goes by its
/// names from then on and takes whatever ids and facts it lacks. The merged
/// record is kept as a pointer: a credit a source gives under one of its ids
/// is filed under the other, and its page leads there. A refresh leaves the
/// credits where they were put.
#[utoipa::path(
    post, path = "/people/{id}/merge", tag = TAG,
    operation_id = "people_merge",
    params(("id" = String, Path, description = "The record merged away")),
    request_body = MergePerson,
    responses(
        (status = 200, description = "The record kept", body = PersonId),
        (status = 400, description = "A record merged into itself"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "Either record does not exist, or was merged already"),
    ),
)]
async fn merge(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<MergePerson>,
) -> AppResult<Json<PersonId>> {
    identity.require_write()?;
    let into = request.into.trim().to_string();
    let works = repo::person::merge(&state.db, &id, &into)
        .await?
        .map_err(refused)?;
    after(&state, &[&id, &into], &works).await;
    trail(
        &state,
        &identity,
        &ip,
        Action::PersonMerged,
        &id,
        format!("merged into {into}"),
    )
    .await;
    Ok(Json(PersonId { id: into }))
}

/// The credits split off, and what the new record is called.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SplitPerson {
    /// The credits, by id — `credit_id` on the person's page — that are
    /// somebody else's.
    pub credits: Vec<String>,
    /// That somebody's name; the person's own when left out.
    #[serde(default)]
    pub name: Option<String>,
}

/// Split credits off a record onto a new one: one name, two people.
///
/// The way out of a homonym taken for somebody else — a credit by name alone
/// filed under the person known by a TMDB id who goes by that name. The new
/// record is made by hand; a refresh leaves the credits where they were put.
#[utoipa::path(
    post, path = "/people/{id}/split", tag = TAG,
    operation_id = "people_split",
    params(("id" = String, Path, description = "The record the credits are taken from")),
    request_body = SplitPerson,
    responses(
        (status = 201, description = "The new record", body = PersonId),
        (status = 400, description = "No credit, or one that is not this person's"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such record, or one merged into another"),
    ),
)]
async fn split(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<SplitPerson>,
) -> AppResult<(StatusCode, Json<PersonId>)> {
    identity.require_write()?;
    let credits = distinct(&request.credits);
    let (new, works) = repo::person::split(&state.db, &id, &credits, request.name.as_deref())
        .await?
        .map_err(refused)?;
    after(&state, &[&id, &new], &works).await;
    trail(
        &state,
        &identity,
        &ip,
        Action::PersonSplit,
        &id,
        format!("{} split off onto {new}", credited(credits.len())),
    )
    .await;
    Ok((StatusCode::CREATED, Json(PersonId { id: new })))
}

/// The credits filed under a person.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttachCredits {
    /// The credits, by id — a work's `credits[].id` — whomever they were
    /// filed under.
    pub credits: Vec<String>,
}

/// File credits under a record, whomever they were filed under.
///
/// How a credit by name alone is given the person it is of — an actor typed
/// in by hand, a guest TheTVDB lists — or one filed under the wrong person
/// moved. A refresh leaves them where they were put.
#[utoipa::path(
    post, path = "/people/{id}/credits", tag = TAG,
    operation_id = "people_attach",
    params(("id" = String, Path, description = "The record the credits are filed under")),
    request_body = AttachCredits,
    responses(
        (status = 204, description = "Filed"),
        (status = 400, description = "No credit, or one that does not exist"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such record, or one merged into another"),
    ),
)]
async fn attach(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<AttachCredits>,
) -> AppResult<StatusCode> {
    identity.require_write()?;
    let credits = distinct(&request.credits);
    if credits.is_empty() {
        return Err(refused(Refused::NotTheirs(String::new())));
    }
    let (was, works) = repo::person::attach(&state.db, &id, &credits)
        .await?
        .map_err(refused)?;
    let mut people: Vec<&str> = was.iter().map(String::as_str).collect();
    people.push(&id);
    after(&state, &people, &works).await;
    trail(
        &state,
        &identity,
        &ip,
        Action::PersonCredited,
        &id,
        format!("{} filed under them", credited(credits.len())),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// How many credits, in words a trail entry reads well with.
fn credited(n: usize) -> String {
    match n {
        1 => "one credit".to_string(),
        n => format!("{n} credits"),
    }
}

/// Each credit once, blanks aside.
fn distinct(credits: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for credit in credits.iter().map(|c| c.trim()).filter(|c| !c.is_empty()) {
        if !seen.iter().any(|s| s == credit) {
            seen.push(credit.to_string());
        }
    }
    seen
}

#[cfg(test)]
mod tests;
