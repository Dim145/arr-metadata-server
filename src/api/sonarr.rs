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
    service::{language, orders, scene, series},
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
///
/// A series the catalogue holds is answered from the catalogue, as it was,
/// when the sources do not answer. An id nobody holds is a 404 only when every
/// source that could resolve it answered that there is no such series; when
/// one did not answer — a timeout, a 5xx, a refused connection — it is a 502,
/// never a 404: Sonarr takes a series for deleted on a 404, and keeps what it
/// has on anything else.
#[utoipa::path(
    get, path = "/v1/tvdb/shows/{language}/{tvdb_id}", tag = TAG,
    operation_id = "sonarr_show",
    params(
        ("language" = String, Path, description = "Two-letter language code, e.g. `en`"),
        ("tvdb_id" = i64, Path, description = "TVDB series id, or a synthesised one"),
    ),
    responses(
        (status = 200, body = ShowResource),
        (status = 400, description = "The language is not a language tag"),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "Every source that could resolve this id answered that there is no such series"),
        (status = 502, description = "A source that could have resolved this id did not answer, and nothing is held to answer with"),
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
    // Sonarr v4 knows the aired order alone: numbered by the order frozen as
    // it, when one is.
    let orders = repo::order::list(&state.db, &item.id).await?;
    let own = orders::number_as_official(&mut item, &orders);
    // It reads a TheTVDB id on every episode: one of the frozen order's own
    // is given TheTVDB's when its record carries one, and the id Sonarr v5
    // is given otherwise, so that a series moved to v5 keeps its episodes.
    for episode in &mut item.episodes {
        if episode.tvdb_id.is_none() && own.contains(&episode.id) {
            episode.tvdb_id = Some(orders::own_episode_id(episode));
        }
    }

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
///
/// A search nothing answered while a source that could have did not is a 502
/// rather than an empty list, which Sonarr reads as "no such series"; what the
/// catalogue holds by that name is answered whether or not the sources do.
#[utoipa::path(
    get, path = "/v1/tvdb/search/{language}", tag = TAG,
    operation_id = "sonarr_search",
    params(
        ("language" = String, Path, description = "Two-letter language code, e.g. `en`"),
        ("term" = Option<String>, Query, description = "Free text, or `prefix:id`"),
    ),
    responses(
        (status = 200, body = Vec<ShowResource>),
        (status = 400, description = "The language is not a language tag"),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 502, description = "No source that could answer this search did, and the catalogue holds nothing by that name"),
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
///
/// Not found and not answered are told apart as `/v1/` tells them: a 404
/// only when every source that could resolve the id answered that there is
/// no such series, a 502 when one did not answer, and the catalogue's copy
/// whenever it holds one.
#[utoipa::path(
    get, path = "/v2/tvdb/shows/{language}/{tvdb_id}/{season_type}", tag = TAG,
    operation_id = "sonarr_show_v2",
    params(
        ("language" = String, Path, description = "Three-letter language code, as Sonarr writes it: `eng`, `fra`"),
        ("tvdb_id" = i64, Path, description = "TVDB series id, or a synthesised one"),
        ("season_type" = String, Path, description = "`official`, or an order of `seasonTypes`: `dvd`, `absolute`, `altdvd`, `alttwo`…"),
    ),
    responses(
        (status = 200, body = ShowResourceV2),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "Every source that could resolve this id answered that there is no such series"),
        (status = 502, description = "A source that could have resolved this id did not answer, and nothing is held to answer with"),
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
    // The order asked in, read from its source first when it is a TMDB
    // group not read yet.
    let orders = orders::for_clients(&state, &item, Some(&season_type)).await?;

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
/// `tvdb:` itself and sends `imdb:`, `tmdb:`, `mal:` and `anilist:` here. A
/// search nothing answered while a source did not is a 502, as for `/v1/`,
/// rather than an empty list.
#[utoipa::path(
    get, path = "/v2/tvdb/search", tag = TAG,
    operation_id = "sonarr_search_v2",
    params(
        ("language" = Option<String>, Query, description = "Three-letter language code, as Sonarr writes it; English when left out"),
        ("term" = Option<String>, Query, description = "Free text, or `prefix:id`"),
    ),
    responses(
        (status = 200, body = Vec<ShowResourceV2>),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 502, description = "No source that could answer this search did, and the catalogue holds nothing by that name"),
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
    use std::sync::Arc;

    use axum::{http::StatusCode, response::IntoResponse};
    use serde_json::json;

    use super::*;
    use crate::{
        config,
        db::repo::{
            child::{blank_episode, blank_season},
            item::ItemWrite,
        },
        domain::{
            EpisodeOrder, ExternalIds, MediaKind, OrderKind, OrderRecord, PlacedEpisode,
            Translation,
        },
    };

    /// What the tests' server is made from: a database in memory, as
    /// `native::testing::server` builds it, stored in English — what the
    /// tests here count on when they ask in another language — and every
    /// source off, so that nothing here reaches past this process.
    async fn configured() -> config::Config {
        let mut config = config::Config::from_env().expect("a configuration");
        config.mode = config::Mode::Single;
        config.database = crate::db::testing::database().await;
        config.security.bootstrap_admin = None;
        config.cache.redis_url = None;
        config.media.storage = config::MediaStorage::Off;
        config.clients = None;
        config.tmdb.api_key = None;
        config.tmdb.include_adult = false;
        config.tmdb.language = "en-US".into();
        config.tvdb.api_key = None;
        config.tvdb.enabled = false;
        config.skyhook.enrich = false;
        config.skyhook.fallback = false;
        config.fanart.api_key = None;
        config.tvmaze.enabled = false;
        config.anilist.enabled = false;
        config.mal.enabled = false;
        config
    }

    /// The whole server, with no source to ask.
    async fn server() -> AppState {
        AppState::bootstrap(configured().await)
            .await
            .expect("a server")
    }

    /// The whole server with TheTVDB keyed and Skyhook asked, both at `at` —
    /// the providers of [`providers`] — and TMDB keyed there too when asked.
    async fn server_at(at: &str, tmdb: bool) -> AppState {
        let mut config = configured().await;
        config.tmdb.api_key = tmdb.then(|| "key".to_string());
        config.tmdb.upstream = at.into();
        config.tvdb.api_key = Some("key".into());
        config.tvdb.enabled = true;
        config.tvdb.upstream = at.into();
        config.skyhook.upstream = at.into();
        config.skyhook.enrich = true;
        config.skyhook.fallback = true;
        AppState::bootstrap(config).await.expect("a server")
    }

    /// What the providers answer, by path — the query left aside — as a
    /// status and a body, 404 for a path not listed; and the paths asked,
    /// in order.
    type Routes = Vec<(&'static str, u16, serde_json::Value)>;

    /// Every provider at one address, answering what `routes` says.
    async fn providers(routes: Routes) -> (String, Arc<parking_lot::Mutex<Vec<String>>>) {
        let asked = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let app = axum::Router::new().fallback({
            let asked = asked.clone();
            move |uri: axum::http::Uri| {
                let path = uri.path().to_string();
                asked.lock().push(path.clone());
                let found = routes
                    .iter()
                    .find(|(at, _, _)| *at == path)
                    .map(|(_, status, body)| (*status, body.clone()));
                async move {
                    match found {
                        Some((status, body)) => (
                            StatusCode::from_u16(status).expect("a status"),
                            axum::Json(body),
                        )
                            .into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let at = format!("http://{}", listener.local_addr().expect("its address"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (at, asked)
    }

    /// TheTVDB signing this server in, and holding the series `routes` say.
    fn thetvdb(routes: Routes) -> Routes {
        let mut all = vec![("/login", 200, json!({ "data": { "token": "t" } }))];
        all.extend(routes);
        all
    }

    /// *Rurouni Kenshin* as TheTVDB answers for it, the least of it.
    fn thetvdbs_kenshin() -> Routes {
        thetvdb(vec![
            (
                "/series/416802/extended",
                200,
                json!({ "data": {
                    "id": 416802, "name": "Rurouni Kenshin", "slug": "rurouni-kenshin-2023",
                    "seasons": [{ "id": 1, "number": 1, "type": { "type": "official" } }]
                } }),
            ),
            (
                "/series/416802/episodes/official/eng",
                200,
                json!({ "data": { "episodes": [
                    { "id": 1001, "seasonNumber": 1, "number": 1, "name": "東京編", "aired": "2023-07-07" }
                ] }, "links": { "next": null } }),
            ),
        ])
    }

    /// The stored series made due a refresh, so that a request for it asks
    /// the sources again.
    async fn made_due(state: &AppState, work: &MediaItem) {
        repo::item::mark_refreshed(&state.db, &work.id, Some("2000-01-01T00:00:00Z"), None)
            .await
            .expect("due");
        state.caches.touched(&work.id).await;
    }

    fn translation(language: &str, title: &str) -> Translation {
        Translation {
            language: language.into(),
            title: Some(title.into()),
            overview: None,
            tagline: None,
            is_manual: false,
        }
    }

    fn placed(tvdb_id: i64, season: i32, number: i32) -> PlacedEpisode {
        PlacedEpisode::of_tvdb(tvdb_id, season, number)
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
        repo::order::replace_source(
            &state.db,
            &work.id,
            "tvdb",
            &[EpisodeOrder {
                key: "dvd".into(),
                kind: OrderKind::Dvd,
                source: "tvdb".into(),
                source_ref: None,
                label: "DVD Order".into(),
                is_default: false,
                is_frozen: false,
                announced: None,
                updated_at: crate::db::now(),
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

    /// An order frozen as the aired numbering is what both editions are
    /// given for the aired order: Sonarr v5 asking for `official`, and Sonarr
    /// v4, which knows no other. Thawed, the work's own numbering is back.
    #[tokio::test]
    async fn a_frozen_order_numbers_the_series_for_both_editions_until_it_is_thawed() {
        let state = server().await;
        let work = stored(&state).await;
        let numbers = |episodes: Vec<(i32, i32, Option<i64>)>| episodes;

        assert!(
            repo::order::freeze(&state.db, &work.id, Some("dvd"))
                .await
                .expect("frozen")
        );
        state.caches.touched(&work.id).await;

        let Json(v5) = asked(&state, "eng", 416802, "official")
            .await
            .expect("answered");
        let v5_numbers = numbers(
            v5.episodes
                .as_deref()
                .unwrap()
                .iter()
                .map(|e| (e.season_number, e.episode_number, e.tvdb_id))
                .collect(),
        );
        assert_eq!(
            v5_numbers,
            [(1, 1, Some(1002)), (1, 2, Some(1001)), (2, 1, Some(2001))]
        );
        assert_eq!(v5.season_types[0].kind, "official");

        let Json(v4) = show(
            State(state.clone()),
            Path(ShowPath {
                language: "en".into(),
                tvdb_id: 416802,
            }),
        )
        .await
        .expect("answered");
        let v4_numbers = numbers(
            v4.episodes
                .iter()
                .map(|e| (e.season_number, e.episode_number, e.tvdb_id))
                .collect(),
        );
        assert_eq!(v4_numbers, v5_numbers);

        assert!(
            repo::order::freeze(&state.db, &work.id, None)
                .await
                .expect("thawed")
        );
        let Json(v4) = show(
            State(state.clone()),
            Path(ShowPath {
                language: "en".into(),
                tvdb_id: 416802,
            }),
        )
        .await
        .expect("answered");
        assert_eq!(v4.episodes[0].tvdb_id, Some(1001));
    }

    /// An episode a frozen order carries as its own, which the work does not
    /// hold: Sonarr v4, which reads a TheTVDB id on every episode, is given
    /// the one Sonarr v5 is — or TheTVDB's, when the record carries one.
    #[tokio::test]
    async fn an_episode_a_frozen_order_carries_as_its_own_has_one_id_for_both_editions() {
        let state = server().await;
        let work = stored(&state).await;
        let own = |tvdb_id: Option<i64>, number: i32, title: &str| PlacedEpisode {
            tvdb_id,
            record: Some(OrderRecord {
                title: title.into(),
                overview: None,
                aired: Some("2023-12-01".into()),
                runtime: Some(24),
                image: None,
                finale_type: None,
            }),
            ..placed(0, 0, number)
        };
        repo::order::replace_source(
            &state.db,
            &work.id,
            "tvdb",
            &[EpisodeOrder {
                key: "dvd".into(),
                kind: OrderKind::Dvd,
                source: "tvdb".into(),
                source_ref: None,
                label: "DVD Order".into(),
                is_default: false,
                is_frozen: false,
                announced: None,
                updated_at: crate::db::now(),
                episodes: vec![
                    placed(1002, 1, 1),
                    placed(1001, 1, 2),
                    placed(2001, 2, 1),
                    own(None, 9, "Recap: the Tokyo arc"),
                    own(Some(7007), 10, "Director's cut"),
                ],
            }],
        )
        .await
        .expect("the order kept");
        assert!(
            repo::order::freeze(&state.db, &work.id, Some("dvd"))
                .await
                .expect("frozen")
        );
        state.caches.touched(&work.id).await;

        let Json(v5) = asked(&state, "eng", 416802, "official")
            .await
            .expect("answered");
        let id_in_v5 = |number: i32| {
            v5.episodes
                .as_deref()
                .unwrap()
                .iter()
                .find(|e| e.season_number == 0 && e.episode_number == number)
                .and_then(|e| e.tvdb_id)
        };
        let made = id_in_v5(9).expect("an id made for it");
        assert!(made < 0, "never one of TheTVDB's: {made}");
        assert_eq!(id_in_v5(10), Some(7007));

        let Json(v4) = show(
            State(state.clone()),
            Path(ShowPath {
                language: "en".into(),
                tvdb_id: 416802,
            }),
        )
        .await
        .expect("answered");
        let id_in_v4 = |number: i32| {
            v4.episodes
                .iter()
                .find(|e| e.season_number == 0 && e.episode_number == number)
                .and_then(|e| e.tvdb_id)
        };
        assert_eq!(id_in_v4(9), Some(made));
        assert_eq!(id_in_v4(10), Some(7007));
        // Every episode carries one.
        assert!(v4.episodes.iter().all(|e| e.tvdb_id.is_some()));
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

    /// TheTVDB and Skyhook both answering that there is no series under the
    /// id, nothing held: not found — the one answer Sonarr deletes a series
    /// on, given only when every source said so.
    #[tokio::test]
    async fn a_series_nobody_holds_is_not_found() {
        let (at, _) = providers(thetvdb(Vec::new())).await;
        let state = server_at(&at, false).await;
        let Err(refused) = asked(&state, "eng", 1, "official").await else {
            panic!("answered");
        };
        assert!(matches!(refused, AppError::NotFound), "{refused}");
    }

    /// TheTVDB broken — a 500, as a timeout or a refused connection would
    /// be — and nothing held: a 502, which Sonarr keeps the series on, never
    /// the 404 it would delete it on. On both editions of the surface.
    #[tokio::test]
    async fn a_source_that_did_not_answer_is_a_502_never_a_404() {
        let (at, _) = providers(thetvdb(vec![(
            "/series/1/extended",
            500,
            json!({ "message": "down" }),
        )]))
        .await;
        let state = server_at(&at, false).await;

        let Err(refused) = asked(&state, "eng", 1, "official").await else {
            panic!("answered");
        };
        assert!(
            matches!(refused, AppError::UpstreamUnavailable(_)),
            "{refused}"
        );

        let Err(refused) = show(
            State(state.clone()),
            Path(ShowPath {
                language: "en".into(),
                tvdb_id: 1,
            }),
        )
        .await
        else {
            panic!("answered");
        };
        assert!(
            matches!(refused, AppError::UpstreamUnavailable(_)),
            "{refused}"
        );
    }

    /// A series the catalogue holds, due a refresh, while TheTVDB is broken:
    /// answered from the catalogue as it was, whatever the sources say — and
    /// the failed refresh recorded, so that the next request is answered at
    /// once and asks nobody.
    #[tokio::test]
    async fn a_series_the_catalogue_holds_is_answered_from_it_whatever_the_sources_say() {
        let (at, paths) = providers(thetvdb(vec![(
            "/series/416802/extended",
            500,
            json!({ "message": "down" }),
        )]))
        .await;
        let state = server_at(&at, false).await;
        let work = stored(&state).await;
        made_due(&state, &work).await;

        let Json(show) = asked(&state, "eng", 416802, "official")
            .await
            .expect("answered from the catalogue");
        assert_eq!(show.title, "Rurouni Kenshin");
        assert!(paths.lock().iter().any(|p| p == "/series/416802/extended"));

        let asked_before = paths.lock().len();
        let Json(again) = asked(&state, "eng", 416802, "official")
            .await
            .expect("answered");
        assert_eq!(again.title, "Rurouni Kenshin");
        assert_eq!(paths.lock().len(), asked_before, "{:?}", paths.lock());
    }

    /// A search no source could answer — TMDB broken, nothing held by that
    /// name — is a 502, not the empty list Sonarr reads as "no such series";
    /// a name the catalogue holds is answered with what it holds.
    #[tokio::test]
    async fn a_search_no_source_could_answer_is_a_502_not_an_empty_list() {
        let (at, _) = providers(thetvdb(vec![(
            "/3/search/tv",
            500,
            json!({ "status_message": "down" }),
        )]))
        .await;
        let state = server_at(&at, true).await;
        let searched = |term: &str| {
            search_v2(
                State(state.clone()),
                Extension(Identity::Network(None)),
                Query(SearchQueryV2 {
                    language: None,
                    term: Some(term.into()),
                }),
            )
        };

        let Err(refused) = searched("breaking bad").await else {
            panic!("answered");
        };
        assert!(
            matches!(refused, AppError::UpstreamUnavailable(_)),
            "{refused}"
        );
        let Err(refused) = search(
            State(state.clone()),
            Path(SearchPath {
                language: "en".into(),
            }),
            Query(SearchQuery {
                term: Some("breaking bad".into()),
            }),
        )
        .await
        else {
            panic!("answered");
        };
        assert!(
            matches!(refused, AppError::UpstreamUnavailable(_)),
            "{refused}"
        );

        stored(&state).await;
        let Json(shows) = searched("Rurouni Kenshin").await.expect("answered");
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].tvdb_id, 416802);
    }

    /// Two requests for one series due a refresh, together — a library's
    /// refresh after the upgrade to Sonarr v5 asks for every series at once
    /// — fetch it once: the second waits for the first and is answered with
    /// what it stored.
    #[tokio::test]
    async fn two_requests_for_the_same_stale_series_fetch_it_once() {
        let (at, paths) = providers(thetvdbs_kenshin()).await;
        let state = server_at(&at, false).await;
        let work = stored(&state).await;
        made_due(&state, &work).await;

        let (first, second) = tokio::join!(
            asked(&state, "eng", 416802, "official"),
            asked(&state, "eng", 416802, "official")
        );
        let Json(first) = first.expect("answered");
        let Json(second) = second.expect("answered");
        assert_eq!(first.title, "Rurouni Kenshin");
        assert_eq!(second.title, "Rurouni Kenshin");

        let fetched = paths
            .lock()
            .iter()
            .filter(|p| p.as_str() == "/series/416802/episodes/official/eng")
            .count();
        assert_eq!(fetched, 1, "{:?}", paths.lock());
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
