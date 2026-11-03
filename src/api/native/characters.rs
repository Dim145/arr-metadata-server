//! A character's page: the part itself — its name, in its own script too,
//! its picture, the ids its sources file it under — and every work of the
//! catalogue it is in that the reader may see, with what it is there, who
//! plays it and who voices it in each dub.
//!
//! A character is one row for every work it is in (`repo::character`):
//! TheTVDB's record of a part and AniList's character, found again by their
//! ids, so a film and its series share theirs. Who plays it on a work is the
//! person a credit of the work is filed under — said only where the work's
//! credits, as its locks leave them, still name that person: a performer a
//! lock hides is no performer to show, here as on the work's page.

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
    domain::{
        CreditType, MediaItem,
        person::{CharacterRole, CharacterVoice, name_language},
    },
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// Filed with the rest of the catalogue in the documentation.
const TAG: &str = super::items::TAG;

/// The longest id a character's address is looked up by: this server's ids
/// are far shorter, and nothing longer is worth a query.
const ENTITY_ID: usize = 64;

/// The most works a page reads the dubs of: one query each, and a character
/// is in a handful of works, not hundreds.
const VOICED_WORKS: usize = 24;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(page))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CharacterQuery {
    /// The reader's language: `fr`. The works' titles are read in it where
    /// they are written in it.
    pub language: Option<String>,
}

/// Who plays a character on a work: the person a credit of the work is
/// filed under.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = CharacterPerformer)]
pub struct Performer {
    /// Their record's id: their page is `/people/{id}`.
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// Their portrait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// A work a character is in, and what it is there.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = CharacterAppearance)]
pub struct Appearance {
    pub work_id: String,
    /// A lead, one beside them, one in the background — where the source
    /// says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<CharacterRole>,
    /// Who plays it there — an actor on screen, an anime's original voice —
    /// where a credit of the work the reader is shown names them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performer: Option<Performer>,
    /// Who voices it there in each dub, a language at a time — ISO 639-3,
    /// `jpn`, `eng` — its own order within one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voices: Vec<CharacterVoice>,
}

/// A character, and the works of the catalogue it is in.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CharacterPage {
    /// This server's id for it: its page is `/characters/{id}`.
    pub id: String,
    pub name: String,
    /// Its name in the script of its works' own language: フリーレン.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_name: Option<String>,
    /// The language that name is in, as a page's `lang` takes it: `ja`.
    /// None where neither its script nor its works say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_name_lang: Option<String>,
    /// Its picture — drawn, or the actor in the part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// TheTVDB's id for its record of the part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    /// AniList's id for it: `https://anilist.co/character/{id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anilist_id: Option<i64>,
    /// MyAnimeList's id for it: `https://myanimelist.net/character/{id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mal_id: Option<i64>,
    /// Every work it is in that the reader may see, the newest first.
    pub appearances: Vec<Appearance>,
    /// Those works, as cards, in the same order.
    pub works: Vec<MediaItem>,
}

/// A character, and every work of the catalogue it is in that the caller
/// may see: what it is there, who plays it — as the work's credits, locks
/// and all, have it — and who voices it in each dub; the works as cards,
/// the newest first, their titles in the language asked for where one is
/// held.
#[utoipa::path(
    get, path = "/characters/{id}", tag = TAG,
    operation_id = "characters_page",
    params(
        ("id" = String, Path, description = "This server's id for the character: a work's `characters[].character.id`"),
        CharacterQuery,
    ),
    responses(
        (status = 200, body = CharacterPage),
        (status = 400, description = "The language is not one"),
        (status = 404, description = "No character has that id, or none of its works is one the caller may see"),
    ),
)]
async fn page(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(asked): Path<String>,
    Query(query): Query<CharacterQuery>,
) -> AppResult<Json<CharacterPage>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let asked = asked.trim();
    if asked.is_empty() || asked.len() > ENTITY_ID {
        return Err(AppError::NotFound);
    }
    let character = repo::character::get(&state.db, asked)
        .await?
        .ok_or(AppError::NotFound)?;
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let appearances = repo::character::works_of(&state.db, &character.id, adult).await?;
    // In nothing the caller may see: no character to them, as a company
    // credited on nothing they may see is none.
    if appearances.is_empty() {
        return Err(AppError::NotFound);
    }

    let ids: Vec<String> = appearances.iter().map(|a| a.work_id.clone()).collect();
    let mut works = repo::item::by_ids(&state.db, &ids).await?;
    service::apply_overrides(&state, &mut works).await?;
    repo::item::load_artwork(&state.db, &mut works).await?;
    service::overlay_imdb_many(&state, &mut works).await;
    if let Some(language) = language.as_deref() {
        for work in &mut works {
            service::language::apply_shallow(&state, work, language);
        }
    }
    newest_first(&mut works);
    let made_in: Vec<String> = works
        .iter()
        .filter_map(|work| work.original_language.clone())
        .collect();
    for work in &mut works {
        state.media.localize(work);
        service::as_card(work);
    }
    service::redact_for_reader(&identity, &mut works);

    let performers = performers(&state, &appearances, adult).await?;
    let mut voices = voices(&state, &character.id, &works).await?;
    let mut by_work: HashMap<&str, &repo::character::Appearance> = HashMap::new();
    for appearance in &appearances {
        by_work
            .entry(appearance.work_id.as_str())
            .or_insert(appearance);
    }
    let shown = works
        .iter()
        .filter_map(|work| {
            let appearance = by_work.get(work.id.as_str())?;
            let performer = appearance
                .person_id
                .as_ref()
                .and_then(|person| performers.get(&(work.id.clone(), person.clone())))
                .map(|listed| Performer {
                    id: listed.id.clone(),
                    name: listed.name.clone(),
                    tmdb_id: listed.tmdb_id,
                    image: listed
                        .image
                        .as_deref()
                        .map(|url| state.media.localized(url)),
                });
            Some(Appearance {
                work_id: work.id.clone(),
                role: appearance.role,
                performer,
                voices: voices.remove(&work.id).unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(CharacterPage {
        native_name_lang: character
            .native_name
            .as_deref()
            .and_then(|name| name_language(name, &made_in)),
        native_name: character
            .native_name
            .filter(|native| *native != character.name),
        image: character
            .image
            .as_deref()
            .map(|url| state.media.localized(url)),
        id: character.id,
        name: character.name,
        tvdb_id: character.tvdb_id,
        anilist_id: character.anilist_id,
        mal_id: character.mal_id,
        appearances: shown,
        works,
    }))
}

/// The works a character is in as its page and its preview list them: the
/// newest first, the undated last, then by title — whichever engine read
/// them, which do not agree on where a missing year goes.
pub(crate) fn newest_first(works: &mut [MediaItem]) {
    works.sort_by(|a, b| {
        b.year
            .is_some()
            .cmp(&a.year.is_some())
            .then(b.year.cmp(&a.year))
            .then_with(|| a.title.cmp(&b.title))
    });
}

/// Who plays the character on each work, by (work, person): the people its
/// appearances name, each where a credit of that work the caller is shown —
/// on screen, or a guest's — is still filed under them.
async fn performers(
    state: &AppState,
    appearances: &[repo::character::Appearance],
    adult: bool,
) -> AppResult<HashMap<(String, String), repo::person::Listed>> {
    let mut people: Vec<String> = appearances
        .iter()
        .filter_map(|a| a.person_id.clone())
        .collect();
    people.sort();
    people.dedup();
    if people.is_empty() {
        return Ok(HashMap::new());
    }
    let named = repo::person::named(&state.db, &people).await?;

    let wanted: HashSet<(&str, &str)> = appearances
        .iter()
        .filter_map(|a| Some((a.work_id.as_str(), a.person_id.as_deref()?)))
        .collect();
    let mut out = HashMap::new();
    for person in &people {
        let Some(listed) = named.get(person) else {
            continue;
        };
        // The credits as each work's locks leave them: one hidden there is
        // not theirs here.
        for credited in repo::person::credits(&state.db, person, adult).await? {
            let plays = matches!(
                credited.credit.credit_type,
                CreditType::Actor | CreditType::Guest
            );
            if plays && wanted.contains(&(credited.media_id.as_str(), person.as_str())) {
                out.entry((credited.media_id, person.clone()))
                    .or_insert_with(|| listed.clone());
            }
        }
    }
    Ok(out)
}

/// Who voices the character in each dub, on each of the first works shown.
async fn voices(
    state: &AppState,
    character_id: &str,
    works: &[MediaItem],
) -> AppResult<HashMap<String, Vec<CharacterVoice>>> {
    let mut out = HashMap::new();
    for work in works.iter().take(VOICED_WORKS) {
        let found = repo::character::of_work(&state.db, &work.id)
            .await?
            .into_iter()
            .find(|(credit, _)| credit.character.id == character_id)
            .map(|(credit, _)| credit.voices)
            .unwrap_or_default();
        if !found.is_empty() {
            let found = found
                .into_iter()
                .map(|mut voice| {
                    voice.image = voice.image.map(|url| state.media.localized(&url));
                    voice
                })
                .collect();
            out.insert(work.id.clone(), found);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;

    use super::*;
    use crate::domain::{
        Credit, MediaKind,
        person::{Character, CharacterCredit},
    };

    fn credit(name: &str, tmdb: i64) -> Credit {
        Credit {
            id: crate::db::new_id(),
            credit_type: CreditType::Actor,
            person_name: name.into(),
            character_name: None,
            image: None,
            tmdb_person_id: Some(tmdb),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
            department: None,
            job: None,
        }
    }

    fn part(name: &str, anilist: i64) -> CharacterCredit {
        CharacterCredit {
            character: Character {
                id: String::new(),
                name: name.into(),
                native_name: None,
                image: None,
                tvdb_id: None,
                anilist_id: Some(anilist),
                mal_id: None,
            },
            role: Some(CharacterRole::Main),
            sort_order: 0,
            credit_id: None,
            voices: Vec::new(),
            source: Some("anilist".into()),
        }
    }

    fn voice(language: &str, name: &str) -> CharacterVoice {
        CharacterVoice {
            language: language.into(),
            name: name.into(),
            native_name: None,
            image: None,
            anilist_id: None,
            mal_id: None,
            sort_order: 0,
        }
    }

    async fn stored(
        state: &AppState,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
        adjust: impl FnOnce(&mut MediaItem),
    ) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.title = title.into();
        item.year = year;
        item.original_language = Some("jpn".into());
        item.slug = crate::domain::make_slug(title, year);
        adjust(&mut item);
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

    async fn read(state: &AppState, identity: Identity, id: &str) -> AppResult<CharacterPage> {
        let Query(query) = Query::<CharacterQuery>::try_from_uri(
            &"/characters/x".parse::<Uri>().expect("an address"),
        )
        .expect("a query");
        page(
            State(state.clone()),
            Extension(identity),
            Path(id.to_string()),
            Query(query),
        )
        .await
        .map(|Json(page)| page)
    }

    #[tokio::test]
    async fn a_character_is_read_with_the_works_it_is_in_who_plays_it_and_its_dubs() {
        let state = crate::api::native::testing::server().await;
        let series = stored(
            &state,
            MediaKind::Series,
            "Character Page Series",
            Some(2023),
            |item| {
                item.credits = vec![credit("Atsumi Page", 9_400_001)];
                let mut lead = part("Frieren Page", 9_410_001);
                lead.character.native_name = Some("フリーレン".into());
                lead.character.image = Some("https://s4.anilist.co/character/frieren.png".into());
                lead.credit_id = Some(item.credits[0].id.clone());
                lead.voices = vec![voice("jpn", "Atsumi Page"), voice("eng", "Mallorie Page")];
                item.characters = vec![lead];
            },
        )
        .await;
        // A film naming the same character by AniList's id: one row, two
        // works, played by nobody the film credits.
        let film = stored(
            &state,
            MediaKind::Movie,
            "Character Page Film",
            Some(2025),
            |item| {
                let mut lead = part("Frieren Page", 9_410_001);
                lead.role = Some(CharacterRole::Supporting);
                item.characters = vec![lead];
            },
        )
        .await;
        // And one a visitor is not shown.
        stored(
            &state,
            MediaKind::Movie,
            "Character Page Adult",
            Some(2024),
            |item| {
                item.is_adult = true;
                item.characters = vec![part("Frieren Page", 9_410_001)];
            },
        )
        .await;

        let held = repo::item::get(&state.db, &series.id)
            .await
            .unwrap()
            .expect("the series");
        let mut held = held;
        repo::item::load_children(&state.db, &mut held)
            .await
            .unwrap();
        let id = held.characters[0].character.id.clone();
        assert!(!id.is_empty());

        let page = read(&state, Identity::Visitor, &id).await.expect("a page");
        assert_eq!(page.name, "Frieren Page");
        assert_eq!(page.native_name.as_deref(), Some("フリーレン"));
        assert_eq!(page.native_name_lang.as_deref(), Some("ja"));
        assert_eq!(page.anilist_id, Some(9_410_001));
        assert_eq!(
            page.image.as_deref(),
            Some("https://s4.anilist.co/character/frieren.png")
        );
        assert_eq!(
            page.works.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            [film.id.as_str(), series.id.as_str()],
            "the newest first, nothing for adults"
        );
        assert_eq!(
            page.appearances
                .iter()
                .map(|a| a.work_id.as_str())
                .collect::<Vec<_>>(),
            [film.id.as_str(), series.id.as_str()]
        );

        let in_film = &page.appearances[0];
        assert_eq!(in_film.role, Some(CharacterRole::Supporting));
        assert!(in_film.performer.is_none() && in_film.voices.is_empty());

        let in_series = &page.appearances[1];
        assert_eq!(in_series.role, Some(CharacterRole::Main));
        let performer = in_series.performer.as_ref().expect("who plays it");
        assert_eq!(
            (performer.name.as_str(), performer.tmdb_id),
            ("Atsumi Page", Some(9_400_001))
        );
        assert_eq!(
            in_series
                .voices
                .iter()
                .map(|v| (v.language.as_str(), v.name.as_str()))
                .collect::<Vec<_>>(),
            [("eng", "Mallorie Page"), ("jpn", "Atsumi Page")]
        );

        // A lock that hides the performer's credit leaves the part unplayed
        // here too, as on the work's page.
        let hidden = crate::domain::fields::Child::of_credit(&series.credits[0]).to_string();
        repo::override_field::set_in(
            &state.db,
            &series.id,
            &hidden,
            "hidden",
            "",
            Some(&serde_json::Value::Bool(true)),
            None,
        )
        .await
        .expect("hidden");
        let page = read(&state, Identity::Visitor, &id).await.expect("a page");
        assert!(page.appearances[1].performer.is_none());

        // Unknown, or in nothing a visitor may see: nobody.
        assert!(matches!(
            read(&state, Identity::Visitor, "no-such-character").await,
            Err(AppError::NotFound)
        ));
        let only_adult = stored(
            &state,
            MediaKind::Movie,
            "Character Page Adult Only",
            Some(2020),
            |item| {
                item.is_adult = true;
                item.characters = vec![part("Adult Only Page", 9_410_002)];
            },
        )
        .await;
        let mut only_adult = repo::item::get(&state.db, &only_adult.id)
            .await
            .unwrap()
            .expect("the film");
        repo::item::load_children(&state.db, &mut only_adult)
            .await
            .unwrap();
        assert!(matches!(
            read(
                &state,
                Identity::Visitor,
                &only_adult.characters[0].character.id
            )
            .await,
            Err(AppError::NotFound)
        ));
    }
}
