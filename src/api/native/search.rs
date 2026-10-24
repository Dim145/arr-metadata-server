//! Searching the catalogue across its kinds: the works, the people credited
//! on them and the episodes, each ranked, for a field that lists what it
//! finds as it is typed — the command palette's.
//!
//! The list of works (`GET /items?term=`) searches the same index; this is the
//! shape for a search box rather than a page of cards.

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo,
    domain::MediaKind,
    error::{AppError, AppResult},
    search::{self, Found, Kind, Scope, Term},
    service,
    state::AppState,
};

/// Filed with the rest of the catalogue in the documentation.
const TAG: &str = super::items::TAG;

/// The hits of each kind when nothing says how many.
const SHOWN: i64 = 6;

/// The most hits of each kind one answer carries.
const MOST: i64 = 20;

/// The longest term looked for, as the list takes it.
const TERM_LENGTH: usize = 200;

/// The most rows each look in the index brings back to be ranked.
const POOL: i64 = 100;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(find))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// What to look for: a title, a name, an episode's title — as typed,
    /// accents, case and punctuation aside. A year at the end of a title,
    /// `dune 2021`, puts that year's work first.
    pub term: String,
    /// Which kinds of hit, comma-separated: `work`, `person`, `episode`.
    /// All three when left out.
    pub kinds: Option<String>,
    /// The most hits of each kind, from 1 to 20; 6 when left out.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    /// Titles in this language, where a translation is held.
    pub language: Option<String>,
    /// Ask for adult titles. Honoured only as far as the server and this
    /// caller's own policy allow, as the list does.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub include_adult: Option<bool>,
    /// Works switched off too. For those who may edit the catalogue; anybody
    /// else is answered as if it had not been asked.
    #[serde(default)]
    pub include_disabled: bool,
}

/// What a search found, kind by kind, the best of each first.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SearchHits)]
pub struct Hits {
    pub works: Vec<WorkHit>,
    pub people: Vec<PersonHit>,
    pub episodes: Vec<EpisodeHit>,
}

/// A work found.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkHit {
    pub id: String,
    pub kind: MediaKind,
    /// Its title, in the language asked for where a translation is held.
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// The other name it was found by, where that is not its title: its
    /// original title, one of its alternative ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
}

/// Somebody the catalogue's works credit.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonHit {
    /// Their record's id, which their page is filed under: `/people/{id}`.
    pub id: String,
    pub name: String,
    /// The other name they were found by, where that is not their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    /// Their TMDB id, where they are known by one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// Their portrait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// What they did on the works anybody may see.
    pub jobs: Vec<crate::domain::person::Job>,
    /// How many works of the catalogue credit them.
    pub works: i64,
}

/// An episode found by its title.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeHit {
    pub work_id: String,
    /// The series' title, in the language asked for where one is held.
    pub work_title: String,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: String,
    /// `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
}

/// Search the works, the people and the episodes at once.
///
/// Each kind ranked on its own — a name that is the term, then one that
/// starts with it, then one with a word that does, then one that holds it,
/// then, when those are few, one a misspelt term resembles; the better known
/// first among those that answer alike. A work is found by any of its titles
/// — original, alternative, translated, locked by hand — and named by the one
/// it is shown under, with the one it was found by beside it. Only what the
/// caller may see: works switched on, adult ones as their policy allows, and
/// people credited on such a work.
#[utoipa::path(
    get, path = "/search", tag = TAG,
    operation_id = "search_find",
    params(SearchQuery),
    responses(
        (status = 200, body = Hits),
        (status = 400, description = "The term is too long, a kind is not one of work, person and episode, or the language is not a language tag"),
    ),
)]
async fn find(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Response> {
    if query.term.chars().count() > TERM_LENGTH {
        return Err(AppError::BadRequest(format!(
            "term is at most {TERM_LENGTH} characters"
        )));
    }
    let kinds = kinds(query.kinds.as_deref())?;
    let language = crate::api::extract::language(query.language.as_deref())?;
    let limit = query.limit.unwrap_or(SHOWN).clamp(1, MOST);
    let adult = state.adult_for(
        identity.client_id(),
        identity.peer_id(),
        query.include_adult,
    );
    let disabled = query.include_disabled && identity.can_write();

    let term = Term::parse(&query.term);
    if term.is_empty() {
        return Ok(Json(Hits::default()).into_response());
    }

    // Asked for at every keystroke that settles, and asked again on the way
    // back: kept under the stamp every write and every settings change moves
    // on, by what it was asked and what the caller may see.
    let key = {
        use sha2::{Digest as _, Sha256};
        let asked = format!(
            "{}|{kinds:?}|{limit}|{language:?}|{adult}|{disabled}",
            term.whole.norm
        );
        let digest: String = Sha256::digest(asked.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("search:{}:{digest}", state.caches.stamp())
    };
    if let Some(cached) = state.caches.searches.get(&key).await {
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            cached,
        )
            .into_response());
    }

    let scope = |kind: Kind| Scope {
        kind,
        media_kind: None,
        include_disabled: disabled,
        include_adult: adult,
        limit: POOL,
        forgive: true,
    };
    let mut hits = Hits::default();
    if kinds.contains(&Kind::Work) {
        let found = search::find(&state.db, &term, &scope(Kind::Work)).await?;
        hits.works = works(&state, &term, &found, limit, language.as_deref()).await?;
    }
    if kinds.contains(&Kind::Person) {
        let found = search::find(&state.db, &term, &scope(Kind::Person)).await?;
        hits.people = people(&state, &term, &found, limit, adult, disabled).await?;
    }
    if kinds.contains(&Kind::Episode) {
        let found = search::find(&state.db, &term, &scope(Kind::Episode)).await?;
        hits.episodes = episodes(&state, &term, &found, limit, language.as_deref()).await?;
    }

    if let Ok(encoded) = serde_json::to_string(&hits) {
        state.caches.searches.insert(key, encoded).await;
    }
    Ok(Json(hits).into_response())
}

/// The kinds asked for, each once; all three for none.
fn kinds(asked: Option<&str>) -> AppResult<Vec<Kind>> {
    let mut kinds: Vec<Kind> = Vec::new();
    for name in asked
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        let kind = name.parse::<Kind>().map_err(|_| {
            AppError::BadRequest(format!(
                "unknown kind {name:?}: expected work, person or episode"
            ))
        })?;
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        kinds = vec![Kind::Work, Kind::Person, Kind::Episode];
    }
    Ok(kinds)
}

/// What each thing found is worth, by its best row, with that row: the
/// best first.
fn best<'a>(
    term: &Term,
    rows: impl IntoIterator<Item = &'a Found>,
    weigh: impl Fn(&Found) -> Option<(Option<i32>, Option<f64>, bool)>,
) -> Vec<(f64, &'a Found)> {
    let mut kept: HashMap<&str, (f64, &Found)> = HashMap::new();
    for row in rows {
        let Some((year, popularity, own)) = weigh(row) else {
            continue;
        };
        let Some(score) = search::rank::score(term, &row.norm, year, popularity, own) else {
            continue;
        };
        let subject = search::subject(row);
        match kept.get(subject) {
            Some((was, _)) if *was >= score => {}
            _ => {
                kept.insert(subject, (score, row));
            }
        }
    }
    let mut ranked: Vec<(f64, &Found)> = kept.into_values().collect();
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| search::subject(a.1).cmp(search::subject(b.1)))
    });
    ranked
}

/// The works found, as a reader is shown them: locks laid over, in their
/// language.
async fn works(
    state: &AppState,
    term: &Term,
    found: &search::Finds,
    limit: i64,
    language: Option<&str>,
) -> AppResult<Vec<WorkHit>> {
    let mut ids: Vec<String> = Vec::new();
    for row in found.all() {
        let id = search::subject(row).to_string();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut items = repo::item::by_ids(&state.db, &ids).await?;
    let held: HashMap<String, (Option<i32>, Option<f64>, String)> = items
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                (item.year, item.popularity, search::norm(&item.title)),
            )
        })
        .collect();
    let ranked = best(term, found.all(), |row| {
        let (year, popularity, title) = held.get(search::subject(row))?;
        Some((*year, *popularity, &row.norm == title))
    });
    let chosen: Vec<(&str, &Found)> = ranked
        .into_iter()
        .take(limit as usize)
        .map(|(_, row)| (search::subject(row), row))
        .collect();

    items.retain(|item| chosen.iter().any(|(id, _)| *id == item.id));
    service::apply_overrides(state, &mut items).await?;
    repo::item::load_artwork(&state.db, &mut items).await?;
    if let Some(language) = language {
        for item in &mut items {
            service::language::apply_shallow(state, item, language);
        }
    }
    let by_id: HashMap<&str, &crate::domain::MediaItem> =
        items.iter().map(|item| (item.id.as_str(), item)).collect();

    Ok(chosen
        .into_iter()
        .filter_map(|(id, row)| {
            let item = by_id.get(id)?;
            // The name it was found by, where the reader would not know it
            // for the title they are shown.
            let matched = (search::norm(&item.title) != row.norm).then(|| row.text.clone());
            Some(WorkHit {
                id: item.id.clone(),
                kind: item.kind,
                title: item.title.clone(),
                year: item.year,
                matched,
            })
        })
        .collect())
}

/// The people found, among those credited on a work the reader may see, each
/// once whatever names of theirs the term found.
async fn people(
    state: &AppState,
    term: &Term,
    found: &search::Finds,
    limit: i64,
    adult: bool,
    disabled: bool,
) -> AppResult<Vec<PersonHit>> {
    let ids: Vec<String> = found.all().map(|row| row.ref_id.clone()).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let shown = repo::search::shown_people(&state.db, &ids, adult, disabled).await?;
    let ranked = best(term, found.all(), |row| {
        shown
            .contains(&row.ref_id)
            .then_some((None, row.popularity, true))
    });
    let chosen: Vec<&Found> = ranked
        .into_iter()
        .take(limit as usize)
        .map(|(_, row)| row)
        .collect();
    let ids: Vec<String> = chosen.iter().map(|row| row.ref_id.clone()).collect();
    let named = repo::person::named(&state.db, &ids).await?;
    Ok(chosen
        .into_iter()
        .filter_map(|row| {
            let person = named.get(&row.ref_id)?;
            Some(PersonHit {
                id: row.ref_id.clone(),
                name: person.name.clone(),
                matched: (search::norm(&person.name) != row.norm).then(|| row.text.clone()),
                tmdb_id: person.tmdb_id,
                image: person
                    .image
                    .as_deref()
                    .map(|url| state.media.localized(url)),
                jobs: person.jobs.clone(),
                works: row.popularity.unwrap_or(0.0) as i64,
            })
        })
        .collect())
}

/// The episodes found, with the series each belongs to as the reader is
/// shown it.
async fn episodes(
    state: &AppState,
    term: &Term,
    found: &search::Finds,
    limit: i64,
    language: Option<&str>,
) -> AppResult<Vec<EpisodeHit>> {
    let ranked = best(term, found.all(), |row| {
        Some((row.year, row.popularity, true))
    });
    let chosen: Vec<&Found> = ranked
        .into_iter()
        .take(limit as usize)
        .map(|(_, row)| row)
        .collect();
    if chosen.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<String> = chosen.iter().map(|row| row.ref_id.clone()).collect();
    let held = repo::search::episodes(&state.db, &ids).await?;
    let mut work_ids: Vec<String> = held.iter().map(|e| e.work_id.clone()).collect();
    work_ids.sort();
    work_ids.dedup();
    let mut works = repo::item::by_ids(&state.db, &work_ids).await?;
    service::apply_overrides(state, &mut works).await?;
    if let Some(language) = language {
        repo::item::load_artwork(&state.db, &mut works).await?;
        for work in &mut works {
            service::language::apply_shallow(state, work, language);
        }
    }
    let titles: HashMap<&str, &str> = works
        .iter()
        .map(|w| (w.id.as_str(), w.title.as_str()))
        .collect();
    let by_id: HashMap<&str, &repo::search::Episode> =
        held.iter().map(|e| (e.id.as_str(), e)).collect();

    Ok(chosen
        .into_iter()
        .filter_map(|row| {
            let episode = by_id.get(row.ref_id.as_str())?;
            Some(EpisodeHit {
                work_id: episode.work_id.clone(),
                work_title: titles.get(episode.work_id.as_str())?.to_string(),
                season_number: episode.season_number,
                episode_number: episode.episode_number,
                // The title it was found by: the one locked by hand where
                // that is what matched.
                title: row.text.clone(),
                air_date: episode.air_date.clone(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;
    use serde_json::Value;

    use super::*;
    use crate::{
        db::repo::item::ItemWrite,
        domain::{Credit, CreditType, Episode, MediaItem},
    };

    async fn stored(
        state: &AppState,
        kind: MediaKind,
        title: &str,
        adjust: impl FnOnce(&mut MediaItem),
    ) -> String {
        let mut item = MediaItem::empty(kind);
        item.title = title.into();
        adjust(&mut item);
        item.slug = crate::domain::make_slug(&item.title, item.year);
        repo::item::upsert(
            &state.db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item.id
    }

    fn credit(name: &str, tmdb_id: i64) -> Credit {
        Credit {
            id: String::new(),
            credit_type: CreditType::Director,
            person_name: name.into(),
            character_name: None,
            image: None,
            tmdb_person_id: Some(tmdb_id),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        }
    }

    /// What a visitor is answered with for `asked`, as JSON.
    async fn asked(state: &AppState, asked: &str) -> AppResult<Value> {
        let uri: Uri = format!("/search?{asked}").parse().expect("an address");
        let Query(query) = Query::<SearchQuery>::try_from_uri(&uri).expect("a query");
        let response = find(
            State(state.clone()),
            Extension(Identity::Visitor),
            Query(query),
        )
        .await?;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a body");
        Ok(serde_json::from_slice(&body).expect("JSON"))
    }

    #[tokio::test]
    async fn each_kind_is_found_and_ranked_apart_and_only_what_a_reader_may_see() {
        let state = crate::api::native::testing::server().await;
        let breaking = stored(&state, MediaKind::Series, "Breaking Bad", |i| {
            i.popularity = Some(300.0);
            i.episodes = vec![Episode {
                id: String::new(),
                season_number: 5,
                episode_number: 14,
                absolute_episode_number: None,
                aired_after_season_number: None,
                aired_before_season_number: None,
                aired_before_episode_number: None,
                title: "Ozymandias".into(),
                overview: None,
                air_date: Some("2013-09-15".into()),
                air_date_utc: None,
                runtime: None,
                finale_type: None,
                image: None,
                tvdb_id: None,
                tmdb_id: None,
                rating: None,
                is_manual: false,
            }];
        })
        .await;
        stored(&state, MediaKind::Movie, "Wings of Desire", |i| {
            i.original_title = Some("Der Himmel über Berlin".into());
            i.credits = vec![credit("Wim Wenders", 2725)];
        })
        .await;
        stored(&state, MediaKind::Movie, "Breaking Point", |i| {
            i.is_adult = true;
            i.credits = vec![credit("Somebody Hidden", 99)];
        })
        .await;

        // Misspelt, the work it misspells; the work for adults is no
        // visitor's to be told of.
        let hits = asked(&state, "term=breking%20bad").await.expect("answered");
        assert_eq!(hits["works"][0]["id"], breaking.as_str());
        assert_eq!(hits["works"][0]["kind"], "series");
        let hits = asked(&state, "term=breaking%20point")
            .await
            .expect("answered");
        assert!(
            !hits["works"]
                .as_array()
                .expect("works")
                .iter()
                .any(|w| w["title"] == "Breaking Point"),
            "{hits}"
        );
        let hits = asked(&state, "term=somebody%20hidden&kinds=person")
            .await
            .expect("answered");
        assert_eq!(
            hits["people"],
            Value::Array(Vec::new()),
            "credited on nothing a visitor sees"
        );

        // A person, with the id their page is filed under.
        let hits = asked(&state, "term=Wenders&kinds=person")
            .await
            .expect("answered");
        assert_eq!(hits["people"][0]["name"], "Wim Wenders");
        assert_eq!(hits["people"][0]["tmdbId"], 2725);
        assert_eq!(hits["people"][0]["works"], 1);
        assert_eq!(
            hits["people"][0]["jobs"],
            serde_json::json!(["directing"]),
            "what they did, for the hit to say"
        );
        assert_eq!(
            hits["works"],
            Value::Array(Vec::new()),
            "only the kinds asked for"
        );

        // An episode, with its series and its place.
        let hits = asked(&state, "term=ozymandias").await.expect("answered");
        let episode = &hits["episodes"][0];
        assert_eq!(episode["workId"], breaking.as_str());
        assert_eq!(episode["workTitle"], "Breaking Bad");
        assert_eq!(
            (
                episode["seasonNumber"].clone(),
                episode["episodeNumber"].clone()
            ),
            (5.into(), 14.into())
        );

        // A work found by another of its titles says which.
        let hits = asked(&state, "term=himmel%20uber").await.expect("answered");
        assert_eq!(hits["works"][0]["title"], "Wings of Desire");
        assert_eq!(hits["works"][0]["matched"], "Der Himmel über Berlin");

        assert!(matches!(
            asked(&state, "term=x&kinds=planet").await,
            Err(AppError::BadRequest(_))
        ));
        let hits = asked(&state, "term=%25%25").await.expect("answered");
        assert_eq!(
            hits["works"],
            Value::Array(Vec::new()),
            "punctuation alone finds nothing"
        );
    }
}
