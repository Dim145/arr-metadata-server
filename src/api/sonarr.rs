//! Sonarr compatibility: the Skyhook surface.
//!
//! Sonarr builds these URLs from
//! `https://skyhook.sonarr.tv/v1/tvdb/{route}/{language}/` — Sonarr v5 from
//! `https://skyhook.sonarr.tv/v2/tvdb/{route}/` — and cannot be pointed
//! elsewhere, so reaching this server means overriding DNS for that host. See
//! `docs/integration.md`. Both editions are answered here from the one
//! catalogue: `/v1/` as Sonarr v4 reads it, `/v2/` as v5 does.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo,
    domain::MediaItem,
    error::{AppError, AppResult},
    service::{language, scene, series},
    state::AppState,
    wire::{
        sonarr::{ShowResource, from_item},
        sonarr_v2::{self, Asked, OwnText, ShowResourceV2},
    },
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Sonarr compatibility";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show))
        .routes(routes!(search))
        .routes(routes!(show_v2))
        .routes(routes!(search_v2))
}

#[derive(Deserialize)]
struct ShowPath {
    language: String,
    tvdb_id: i64,
}

#[derive(Deserialize)]
struct SearchPath {
    language: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    term: Option<String>,
}

#[derive(Deserialize)]
struct ShowPathV2 {
    language: String,
    tvdb_id: i64,
    season_type: String,
}

#[derive(Deserialize)]
struct SearchQueryV2 {
    language: Option<String>,
    term: Option<String>,
}

/// A series by TVDB id, in Skyhook's format.
///
/// The id may also be one this server synthesised for a work TMDB indexes and
/// TVDB does not; the response echoes back whichever id was asked for, because
/// Sonarr has already stored it.
#[utoipa::path(
    get, path = "/v1/tvdb/shows/{language}/{tvdb_id}", tag = TAG,
    params(
        ("language" = String, Path, description = "Two-letter language code, e.g. `en`"),
        ("tvdb_id" = i64, Path, description = "TVDB series id, or a synthesised one"),
    ),
    responses(
        (status = 200, body = ShowResource),
        (status = 400, description = "The language is not a language tag"),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No provider could resolve this id"),
    ),
    security(),
)]
async fn show(
    State(state): State<AppState>,
    Path(ShowPath { language, tvdb_id }): Path<ShowPath>,
) -> AppResult<Json<ShowResource>> {
    let language = asked_in(&language)?;
    let mut item = series::by_client_id(&state, tvdb_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Sonarr puts the language in the URL, which is the only place in this
    // protocol it appears. Episode text for a language nobody has asked for yet
    // is fetched here, once, and stored.
    language::apply(&state, &mut item, &language).await?;
    name_for_sonarr(&state, &mut item, tvdb_id).await;

    // Echo back the id the client asked for: Sonarr has already stored it, and
    // a different one in the response would orphan the series.
    state.media.for_clients(&mut item);
    Ok(Json(from_item(&item, tvdb_id, &language)))
}

/// Search series, in Skyhook's format.
///
/// `term` may be free text or a provider lookup: `tvdb:`, `tmdb:`, `imdb:`,
/// `mal:` or `anilist:`. Results Sonarr could not address are omitted — showing
/// one that fails on add is worse than showing nothing.
#[utoipa::path(
    get, path = "/v1/tvdb/search/{language}", tag = TAG,
    params(
        ("language" = String, Path, description = "Two-letter language code, e.g. `en`"),
        ("term" = Option<String>, Query, description = "Free text, or `prefix:id`"),
    ),
    responses(
        (status = 200, body = Vec<ShowResource>),
        (status = 400, description = "The language is not a language tag"),
        (status = 403, description = "The caller's address is not in the allowlist"),
    ),
    security(),
)]
async fn search(
    State(state): State<AppState>,
    Path(SearchPath { language }): Path<SearchPath>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<ShowResource>>> {
    let language = asked_in(&language)?;
    let term = query.term.unwrap_or_default();

    let mut items = series::search(&state, &term).await?;

    // Only the work's own title here: fetching a season of episode text for
    // each of ten search results would turn one search into dozens of calls.
    for item in &mut items {
        language::apply_shallow(&state, item, &language);
        if let Some(id) = series::client_id(item) {
            name_for_sonarr(&state, item, id).await;
        }
        state.media.for_clients(item);
    }

    // A result Sonarr cannot address is worse than no result: it would show in
    // the list and then fail on add.
    let shows = items
        .iter()
        .filter_map(|item| series::client_id(item).map(|id| from_item(item, id, &language)))
        .collect();

    Ok(Json(shows))
}

/// A series by TVDB id, as Sonarr v5 reads it.
///
/// What `/v1/` answers, and what v5 reads besides: every language the
/// catalogue holds the series in, the orders its episodes come in, a
/// season's title and year. The language is three letters, as Sonarr writes
/// it — `eng`, `fra` — and the series is answered in it when the catalogue
/// holds it so, else in the caller's own language (`tmdb.language` for their
/// key or their address's rule), then in English, then as it is stored; a
/// code that names no language is read as `eng`. The season type is
/// `official` or one of the orders `seasonTypes` lists, and the episodes are
/// numbered by it; any other is answered with the aired order, as Skyhook
/// answers it.
#[utoipa::path(
    get, path = "/v2/tvdb/shows/{language}/{tvdb_id}/{season_type}", tag = TAG,
    params(
        ("language" = String, Path, description = "Three-letter language code, as Sonarr writes it: `eng`, `fra`"),
        ("tvdb_id" = i64, Path, description = "TVDB series id, or a synthesised one"),
        ("season_type" = String, Path, description = "`official`, or an order of `seasonTypes`: `dvd`, `absolute`…"),
    ),
    responses(
        (status = 200, body = ShowResourceV2),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No provider could resolve this id"),
    ),
    security(),
)]
async fn show_v2(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(ShowPathV2 {
        language,
        tvdb_id,
        season_type,
    }): Path<ShowPathV2>,
) -> AppResult<Json<ShowResourceV2>> {
    let language = language::three_letter(&language);
    let mut item = series::by_client_id(&state, tvdb_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Named before a language is laid over it: the names the series goes by
    // in its other languages are told from a homonym's too, and the one laid
    // over carries what tells it.
    name_for_sonarr(&state, &mut item, tvdb_id).await;
    let own = OwnText::of(&item, &language::normalize(&state.language(None, None)));
    language::apply_with_fallback(
        &state,
        &mut item,
        &language,
        &state.language(identity.client_id(), identity.peer_id()),
    )
    .await?;
    let orders = repo::order::list(&state.db, &item.id).await?;

    state.media.for_clients(&mut item);
    Ok(Json(sonarr_v2::from_item(
        &item,
        &Asked {
            client_id: tvdb_id,
            language: &language,
            season_type: &season_type,
            own: &own,
            orders: &orders,
            with_episodes: true,
        },
    )))
}

/// Search series, as Sonarr v5 reads them: without their episodes, as the
/// real service answers a search. `term` is as for `/v1/`; Sonarr resolves
/// `tvdb:` itself and sends `imdb:`, `tmdb:`, `mal:` and `anilist:` here.
#[utoipa::path(
    get, path = "/v2/tvdb/search", tag = TAG,
    params(
        ("language" = Option<String>, Query, description = "Three-letter language code, as Sonarr writes it; English when left out"),
        ("term" = Option<String>, Query, description = "Free text, or `prefix:id`"),
    ),
    responses(
        (status = 200, body = Vec<ShowResourceV2>),
        (status = 403, description = "The caller's address is not in the allowlist"),
    ),
    security(),
)]
async fn search_v2(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(SearchQueryV2 { language, term }): Query<SearchQueryV2>,
) -> AppResult<Json<Vec<ShowResourceV2>>> {
    let language = language::three_letter(language.as_deref().unwrap_or_default());
    let term = term.unwrap_or_default();
    let stored = language::normalize(&state.language(None, None));
    let callers = state.language(identity.client_id(), identity.peer_id());

    let mut items = series::search(&state, &term).await?;

    let mut shows = Vec::with_capacity(items.len());
    for item in &mut items {
        // A result Sonarr cannot address is worse than no result: it would
        // show in the list and then fail on add.
        let Some(id) = series::client_id(item) else {
            continue;
        };
        name_for_sonarr(&state, item, id).await;
        let own = OwnText::of(item, &stored);
        // Only the work's own title here, as for `/v1/`: fetching a season
        // of episode text for each of ten search results would turn one
        // search into dozens of calls.
        language::apply_shallow_with_fallback(&state, item, &language, &callers);
        let orders = repo::order::list(&state.db, &item.id).await?;
        state.media.for_clients(item);
        shows.push(sonarr_v2::from_item(
            item,
            &Asked {
                client_id: id,
                language: &language,
                season_type: sonarr_v2::OFFICIAL,
                own: &own,
                orders: &orders,
                with_episodes: false,
            },
        ));
    }

    Ok(Json(shows))
}

/// The language in the URL, as a tag, or a 400.
///
/// Sonarr always sends one — `en` — so only a caller that is not Sonarr is
/// refused. Not taken as it came: it became the key episode text is filed
/// under, a segment of the path TheTVDB was asked on with the operator's
/// token, and a line in the log, so a path or a query in its place
/// reached TheTVDB, and every new string was a season's worth of calls.
fn asked_in(language: &str) -> AppResult<String> {
    crate::api::extract::language(Some(language))?
        .ok_or_else(|| AppError::BadRequest("the address names no language".into()))
}

/// The title Sonarr is given: told apart from a homonym the way Skyhook tells
/// it, a locked one as it was locked (see [`series::sonarr_title`]). The
/// names the series goes by in its other languages, which Sonarr v5 reads,
/// are told apart the same way: a homonym is one in every language.
async fn name_for_sonarr(state: &AppState, item: &mut MediaItem, tvdb_id: i64) {
    let locked = item.locked_fields.iter().any(|f| f == "item/title");
    let homonym_year = if !locked
        && item.title_qualifier.is_none()
        && item.external_ids.tvdb.is_none()
        && scene::has_homonym(state, tvdb_id, &item.title).await
    {
        item.year
    } else {
        None
    };
    let qualifier = item.title_qualifier.clone();
    item.title = series::sonarr_title(&item.title, locked, qualifier.as_deref(), homonym_year);
    // A name locked in a language, or in every one, is given as it was
    // locked there too.
    let locked_in: Vec<bool> = item
        .translations
        .iter()
        .map(|t| {
            crate::domain::fields::text_lock(
                item,
                "item",
                "title",
                &language::normalize(&t.language),
            )
            .is_some()
        })
        .collect();
    for (translation, locked) in item.translations.iter_mut().zip(locked_in) {
        if let Some(title) = translation.title.as_deref() {
            translation.title = Some(series::sonarr_title(
                title,
                locked,
                qualifier.as_deref(),
                homonym_year,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        db::repo::{
            child::{blank_episode, blank_season},
            item::ItemWrite,
        },
        domain::{EpisodeOrder, ExternalIds, MediaKind, PlacedEpisode, Translation},
    };

    /// The whole server on a database in memory, as `native::testing::server`
    /// builds it, stored in English: what the tests here count on when they
    /// ask in another language.
    async fn server() -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        };
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tmdb.include_adult = false;
        config.tmdb.language = "en-US".into();
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;

        AppState::bootstrap(config).await.expect("a server")
    }

    fn translation(language: &str, title: &str) -> Translation {
        Translation {
            language: language.into(),
            title: Some(title.into()),
            overview: None,
            is_manual: false,
        }
    }

    fn placed(tvdb_id: i64, season: i32, number: i32) -> PlacedEpisode {
        PlacedEpisode {
            tvdb_id,
            season_number: season,
            episode_number: number,
            absolute_number: None,
        }
    }

    /// *Rurouni Kenshin* (2023), stored: two seasons, three episodes, named
    /// in French and Japanese besides, and a DVD order.
    async fn stored(state: &AppState) -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = "Rurouni Kenshin".into();
        work.slug = "rurouni-kenshin-2023".into();
        work.original_title = Some("るろうに剣心 －明治剣客浪漫譚－".into());
        work.original_language = Some("jpn".into());
        work.external_ids = ExternalIds {
            tvdb: Some(416802),
            tmdb: Some(209867),
            ..Default::default()
        };
        work.translations = vec![
            translation("fra", "Kenshin le vagabond"),
            translation("jpn", "るろうに剣心 －明治剣客浪漫譚－"),
        ];
        for (number, title, aired) in [(1, "東京編", "2023-07-07"), (2, "京都編", "2024-10-04")]
        {
            // As a provider gave them: what is added by hand is kept apart,
            // and not written with the work.
            let mut season = blank_season(number);
            season.is_manual = false;
            season.title = Some(title.into());
            season.air_date = Some(aired.into());
            work.seasons.push(season);
        }
        for (season, number, tvdb_id, aired) in [
            (1, 1, 1001, "2023-07-07"),
            (1, 2, 1002, "2023-07-14"),
            (2, 1, 2001, "2024-10-04"),
        ] {
            let mut episode = blank_episode(season, number);
            episode.is_manual = false;
            episode.title = format!("Episode {tvdb_id}");
            episode.tvdb_id = Some(tvdb_id);
            episode.air_date = Some(aired.into());
            work.episodes.push(episode);
        }
        repo::item::upsert(
            &state.db,
            ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        repo::order::replace(
            &state.db,
            &work.id,
            &[EpisodeOrder {
                kind: "dvd".into(),
                episodes: vec![placed(1002, 1, 1), placed(1001, 1, 2), placed(2001, 2, 1)],
            }],
        )
        .await
        .expect("the order kept");
        work
    }

    async fn asked(
        state: &AppState,
        language: &str,
        tvdb_id: i64,
        season_type: &str,
    ) -> AppResult<Json<ShowResourceV2>> {
        show_v2(
            State(state.clone()),
            Extension(Identity::Network(None)),
            Path(ShowPathV2 {
                language: language.into(),
                tvdb_id,
                season_type: season_type.into(),
            }),
        )
        .await
    }

    #[tokio::test]
    async fn sonarr_v5_is_answered_in_the_language_of_its_address_with_what_v4_lacks() {
        let state = server().await;
        stored(&state).await;

        let Json(show) = asked(&state, "fra", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(show.title, "Kenshin le vagabond");
        assert_eq!(show.language, "fra");
        assert_eq!(show.original_title, "るろうに剣心 －明治剣客浪漫譚－");
        assert_eq!(show.original_language.as_deref(), Some("jpn"));
        assert_eq!(
            show.translations
                .iter()
                .map(|t| t.language.as_str())
                .collect::<Vec<_>>(),
            ["eng", "fra", "jpn"]
        );
        assert_eq!(
            show.season_types
                .iter()
                .map(|t| t.kind.as_str())
                .collect::<Vec<_>>(),
            ["official", "dvd"]
        );
        let seasons: Vec<(i32, Option<&str>, Option<i32>)> = show
            .seasons
            .iter()
            .map(|s| (s.season_number, s.title.as_deref(), s.year))
            .collect();
        assert_eq!(
            seasons,
            [
                (1, Some("東京編"), Some(2023)),
                (2, Some("京都編"), Some(2024))
            ]
        );

        let sent = serde_json::to_value(&show).unwrap();
        assert_eq!(sent["episodes"][0]["tvdbShowId"], 416802);
        assert_eq!(sent["episodes"][0]["metadataShowId"], 416802);
        assert_eq!(sent["episodes"][0]["tvdbId"], 1001);
        assert_eq!(sent["episodes"][0]["metadataId"], 1001);
        assert_eq!(sent["metadataSource"], "tvdb");
        assert_eq!(sent["seasonTypes"][1]["type"], "dvd");
        assert_eq!(sent["seasonTypes"][1]["name"], "DVD Order");
    }

    #[tokio::test]
    async fn a_series_asked_in_an_order_it_has_is_numbered_by_it_and_otherwise_as_aired() {
        let state = server().await;
        stored(&state).await;

        let Json(show) = asked(&state, "eng", 416802, "dvd").await.expect("answered");
        let numbered: Vec<(i32, i32, Option<i64>)> = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.tvdb_id))
            .collect();
        assert_eq!(
            numbered,
            [(1, 1, Some(1002)), (1, 2, Some(1001)), (2, 1, Some(2001))]
        );
        assert!(show.seasons.iter().all(|s| s.title.is_none()));

        // An order the series has not: the aired one, and no error — Sonarr
        // asks in the type a person chose, and a refusal fails its refresh.
        let Json(show) = asked(&state, "eng", 416802, "absolute")
            .await
            .expect("answered");
        let numbered: Vec<(i32, i32, Option<i64>)> = show
            .episodes
            .as_deref()
            .unwrap()
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.tvdb_id))
            .collect();
        assert_eq!(
            numbered,
            [(1, 1, Some(1001)), (1, 2, Some(1002)), (2, 1, Some(2001))]
        );
        assert_eq!(show.seasons[0].title.as_deref(), Some("東京編"));
    }

    #[tokio::test]
    async fn a_language_the_series_is_not_named_in_is_answered_as_stored() {
        let state = server().await;
        stored(&state).await;

        // German, which nothing names the series in: the caller's own
        // language is the one it is stored in, so the stored text; the
        // German is still listed as what was asked.
        let Json(show) = asked(&state, "deu", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(show.title, "Rurouni Kenshin");
        assert_eq!(show.language, "deu");

        // A code that names no language is read as English rather than
        // refused.
        let Json(show) = asked(&state, "xx", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(show.language, "eng");
        assert_eq!(show.title, "Rurouni Kenshin");
    }

    #[tokio::test]
    async fn the_search_answers_sonarr_v5_without_episodes() {
        let state = server().await;
        stored(&state).await;

        let Json(shows) = search_v2(
            State(state.clone()),
            Extension(Identity::Network(None)),
            Query(SearchQueryV2 {
                language: Some("fra".into()),
                term: Some("tvdb:416802".into()),
            }),
        )
        .await
        .expect("answered");
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].title, "Kenshin le vagabond");
        assert_eq!(shows[0].season_types.len(), 2);
        let sent = serde_json::to_value(&shows[0]).unwrap();
        assert!(sent.get("episodes").is_none(), "{sent}");
        assert_eq!(sent["seasons"].as_array().unwrap().len(), 2);

        // No language at all: English.
        let Json(shows) = search_v2(
            State(state.clone()),
            Extension(Identity::Network(None)),
            Query(SearchQueryV2 {
                language: None,
                term: Some("tvdb:416802".into()),
            }),
        )
        .await
        .expect("answered");
        assert_eq!(shows[0].language, "eng");
        assert_eq!(shows[0].title, "Rurouni Kenshin");
    }

    #[tokio::test]
    async fn a_series_nobody_holds_is_not_found() {
        let state = server().await;
        let refused = asked(&state, "eng", 1, "official").await;
        assert!(matches!(refused, Err(AppError::NotFound)));
    }

    /// Locks for the series, in a language or in every one, read afresh.
    async fn locked(state: &AppState, id: &str, locks: &[(&str, &str, &str, &str)]) {
        for (scope, field, language, value) in locks {
            repo::override_field::set_in(
                &state.db,
                id,
                scope,
                field,
                language,
                Some(&serde_json::json!(value)),
                None,
            )
            .await
            .expect("locked");
        }
        state.caches.touched(id).await;
    }

    /// A title locked in French alone is the series' name for a Sonarr that
    /// asks in French — its title, its French translation, an episode's —
    /// and for nobody else; an overview locked in every language beats every
    /// translation; and the original title stays the one it was made under.
    #[tokio::test]
    async fn a_text_locked_in_one_language_is_sonarrs_in_that_one_alone() {
        let state = server().await;
        let work = stored(&state).await;
        locked(
            &state,
            &work.id,
            &[
                ("item", "title", "fra", "Kenshin, en français"),
                ("item", "overview", "", "Partout pareil."),
                ("episode:1x1", "title", "fra", "Le premier"),
            ],
        )
        .await;

        // Sonarr v5, in French.
        let Json(french) = asked(&state, "fra", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(french.title, "Kenshin, en français");
        assert_eq!(french.overview.as_deref(), Some("Partout pareil."));
        assert_eq!(french.original_title, "るろうに剣心 －明治剣客浪漫譚－");
        let named = |language: &str| {
            french
                .translations
                .iter()
                .find(|t| t.language == language)
                .cloned()
                .expect("held")
        };
        assert_eq!(named("fra").title, "Kenshin, en français");
        assert_eq!(named("fra").overview.as_deref(), Some("Partout pareil."));
        assert_eq!(named("jpn").title, "るろうに剣心 －明治剣客浪漫譚－");
        assert_eq!(
            named("jpn").overview.as_deref(),
            Some("Partout pareil."),
            "a lock in every language beats every translation"
        );
        assert_eq!(named("eng").title, "Rurouni Kenshin");
        let episodes = french.episodes.as_deref().expect("its episodes");
        assert_eq!(episodes[0].title, "Le premier");
        assert_eq!(episodes[1].title, "Episode 1002");

        // In English, the language it is stored in: none of French's.
        let Json(english) = asked(&state, "eng", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(english.title, "Rurouni Kenshin");
        assert_eq!(english.overview.as_deref(), Some("Partout pareil."));
        assert_eq!(
            english.episodes.as_deref().expect("its episodes")[0].title,
            "Episode 1001"
        );

        // Sonarr v4 asking in French: the same as v5.
        let Json(v4) = show(
            State(state.clone()),
            Path(ShowPath {
                language: "fr".into(),
                tvdb_id: 416802,
            }),
        )
        .await
        .expect("answered");
        assert_eq!(v4.title, "Kenshin, en français");
        assert_eq!(v4.overview.as_deref(), Some("Partout pareil."));
        assert_eq!(v4.episodes[0].title, "Le premier");
    }

    /// Locked in French and in every language, the title is French's own in
    /// French and the one of every language anywhere else — over the
    /// translation each language holds, and over the stored title — while
    /// the original title Sonarr is given stays the one it was made under.
    #[tokio::test]
    async fn a_lock_in_a_language_beats_one_in_every_language_there_and_both_beat_translations() {
        let state = server().await;
        let work = stored(&state).await;
        locked(
            &state,
            &work.id,
            &[
                ("item", "title", "", "Kenshin"),
                ("item", "title", "fra", "Kenshin, en français"),
            ],
        )
        .await;

        let Json(french) = asked(&state, "fra", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(french.title, "Kenshin, en français");

        let Json(japanese) = asked(&state, "jpn", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(japanese.title, "Kenshin", "the translation is beaten");
        assert_eq!(japanese.original_title, "るろうに剣心 －明治剣客浪漫譚－");
        let title_in = |language: &str| {
            japanese
                .translations
                .iter()
                .find(|t| t.language == language)
                .map(|t| t.title.clone())
        };
        assert_eq!(title_in("fra").as_deref(), Some("Kenshin, en français"));
        assert_eq!(title_in("jpn").as_deref(), Some("Kenshin"));

        let Json(english) = asked(&state, "eng", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(english.title, "Kenshin");
    }
}
