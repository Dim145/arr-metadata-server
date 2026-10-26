//! A work entered by hand, as each client finds it: made through the
//! administration's own routes, then searched for and asked for by Sonarr —
//! both editions — Radarr and Plex, through the routes and guards they call.
//! The server's whole surface, on a database of its own, every source
//! switched off: a work entered by hand is the catalogue's alone.

use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::{
    config,
    service::ids::{self, OWN_OFFSET},
    state::AppState,
};

/// The whole server with authentication switched off — the administration
/// open to whoever calls, as `AMS_AUTH_DISABLED` leaves it — and no source
/// to ask.
async fn server() -> AppState {
    let mut config = config::Config::from_env().expect("a configuration");
    config.mode = config::Mode::Single;
    config.database = crate::db::testing::database().await;
    config.security.bootstrap_admin = None;
    config.security.auth_disabled = true;
    config.cache.redis_url = None;
    config.media.storage = config::MediaStorage::Off;
    config.clients = None;
    config.tmdb.api_key = None;
    config.tmdb.language = "en-US".into();
    config.tvdb.api_key = None;
    config.tvdb.enabled = false;
    config.skyhook.enrich = false;
    config.skyhook.fallback = false;
    config.radarr_metadata.enrich = false;
    config.radarr_metadata.fallback = false;
    config.tvmaze.enabled = false;
    config.anilist.enabled = false;
    config.mal.enabled = false;
    config.server.public_url = Some("http://ams.lan:8479".into());

    AppState::bootstrap(config).await.expect("a server")
}

/// Every surface, under its own guard, as the server mounts them.
fn surfaces(state: &AppState) -> Router {
    let (router, _) = super::build(state.clone());
    router.with_state(state.clone())
}

/// One request from this very machine, with a JSON body when there is one;
/// the answer's status and its body, read as JSON.
async fn call(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let mut request = request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .expect("a request");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    let response = router.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(router: &Router, uri: &str) -> (StatusCode, Value) {
    call(router, "GET", uri, None).await
}

/// Made as the editor's *New entry* makes it, with a season of two episodes
/// for a series: its catalogue id.
async fn entered_by_hand(router: &Router, work: Value) -> String {
    let series = work["kind"] == "series";
    let (status, made) = call(router, "POST", "/api/v1/items", Some(work)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["isManual"], true);
    let id = made["id"].as_str().expect("an id").to_string();
    if series {
        let (status, _) = call(
            router,
            "POST",
            &format!("/api/v1/items/{id}/seasons"),
            Some(json!({ "seasonNumber": 1 })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        for number in [1, 2] {
            let (status, _) = call(
                router,
                "POST",
                &format!("/api/v1/items/{id}/episodes"),
                Some(json!({
                    "seasonNumber": 1,
                    "episodeNumber": number,
                    "title": format!("Typed in, {number}"),
                    "airDate": format!("2024-0{number}-10"),
                })),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
        }
    }
    id
}

fn vicineko() -> Value {
    json!({
        "kind": "series",
        "title": "Vicineko",
        "year": 2024,
        "overview": "A series no source lists, entered by hand.",
        "status": "continuing",
        "firstAired": "2024-01-10",
        "network": "Hand TV",
        "genres": ["Animation"],
    })
}

/// An id of the range that no work entered by hand is handed: the range's
/// first, unless the work at hand happens to be handed it.
fn named_by_none(handed: i64) -> i64 {
    if handed == OWN_OFFSET {
        OWN_OFFSET + 1
    } else {
        OWN_OFFSET
    }
}

/// The user's case: a series entered by hand — no TheTVDB, TMDB or Fankai id
/// — was left out of Sonarr's search, since Sonarr keys a series by its
/// TheTVDB id and it had none. It is listed now under a number made of its
/// catalogue id, the same on both editions and at every answer, which Sonarr
/// adds it by and refreshes it by.
#[tokio::test]
async fn sonarr_finds_adds_and_refreshes_a_series_entered_by_hand() {
    let state = server().await;
    let router = surfaces(&state);
    let id = entered_by_hand(&router, vicineko()).await;
    let handed = ids::own(&id);
    assert!(ids::is_own(handed), "{handed}");

    // Sonarr v4's search, as it sends it: lower-cased.
    let (status, found) = get(&router, "/v1/tvdb/search/en?term=vicineko").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let found = found.as_array().expect("a list");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["tvdbId"], handed);
    assert_eq!(found[0]["title"], "Vicineko");

    // Its add, and every refresh after: the series by that id.
    let (status, show) = get(&router, &format!("/v1/tvdb/shows/en/{handed}")).await;
    assert_eq!(status, StatusCode::OK, "{show}");
    assert_eq!(show["tvdbId"], handed);
    assert_eq!(show["title"], "Vicineko");
    assert_eq!(show["firstAired"], "2024-01-10");
    let episodes = show["episodes"].as_array().expect("episodes");
    assert_eq!(episodes.len(), 2);
    assert!(episodes.iter().all(|e| e["tvdbShowId"] == handed));
    assert_eq!(episodes[0]["title"], "Typed in, 1");

    // Sonarr v5's, the same series under the same id.
    let (status, found) = get(&router, "/v2/tvdb/search?term=vicineko&language=eng").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found[0]["tvdbId"], handed);
    assert_eq!(found[0]["metadataSource"], "local");
    let (status, show) = get(&router, &format!("/v2/tvdb/shows/eng/{handed}/official")).await;
    assert_eq!(status, StatusCode::OK, "{show}");
    assert_eq!(show["tvdbId"], handed);
    assert_eq!(show["episodes"].as_array().map(Vec::len), Some(2));

    // `tvdb:` and the id, as Sonarr resolves such a term itself.
    let (status, found) = get(&router, &format!("/v1/tvdb/search/en?term=tvdb:{handed}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found[0]["tvdbId"], handed);

    // An edit reaches the next refresh, under the same id.
    let (status, _) = call(
        &router,
        "PUT",
        &format!("/api/v1/items/{id}/overrides"),
        Some(json!({ "field": "title", "value": "Vicineko, au propre" })),
    )
    .await;
    assert!(status.is_success(), "{status}");
    let (_, show) = get(&router, &format!("/v1/tvdb/shows/en/{handed}")).await;
    assert_eq!(show["title"], "Vicineko, au propre");
    assert_eq!(show["tvdbId"], handed);

    // Switched off, it is still Sonarr's: a 404 would have Sonarr take it
    // for deleted.
    let (status, _) = call(
        &router,
        "PATCH",
        &format!("/api/v1/items/{id}"),
        Some(json!({ "isEnabled": false })),
    )
    .await;
    assert!(status.is_success(), "{status}");
    let (status, _) = get(&router, &format!("/v1/tvdb/shows/en/{handed}")).await;
    assert_eq!(status, StatusCode::OK);

    // A number of the range no work is handed names nothing — at once, with
    // no source to wait for — and neither does the work's once it is gone.
    let (status, _) = get(
        &router,
        &format!("/v1/tvdb/shows/en/{}", named_by_none(handed)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&router, "DELETE", &format!("/api/v1/items/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = get(&router, &format!("/v1/tvdb/shows/en/{handed}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&router, &format!("/v2/tvdb/shows/eng/{handed}/official")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A film entered by hand reached Radarr's search with a TMDB id of nought,
/// which Radarr refuses to add, and every such film under the same one. It
/// is listed under a number of its own, which Radarr adds, refreshes and is
/// told of a change by.
#[tokio::test]
async fn radarr_finds_adds_and_refreshes_a_film_entered_by_hand() {
    let state = server().await;
    let router = surfaces(&state);
    let id = entered_by_hand(
        &router,
        json!({
            "kind": "movie",
            "title": "Sans Source",
            "year": 2031,
            "overview": "A film no source lists, entered by hand.",
            "status": "released",
            "inCinemas": "2031-05-01",
            "studio": "Hand Films",
        }),
    )
    .await;
    let handed = ids::own(&id);

    let (status, found) = get(&router, "/v1/search?q=sans%20source&year=").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let found = found.as_array().expect("a list");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["TmdbId"], handed);
    assert_eq!(found[0]["Title"], "Sans Source");

    let (status, film) = get(&router, &format!("/v1/movie/{handed}")).await;
    assert_eq!(status, StatusCode::OK, "{film}");
    assert_eq!(film["TmdbId"], handed);
    assert_eq!(film["Title"], "Sans Source");
    assert_eq!(film["Studio"], "Hand Films");

    let (status, films) = call(&router, "POST", "/v1/movie/bulk", Some(json!([handed]))).await;
    assert_eq!(status, StatusCode::OK, "{films}");
    assert_eq!(films[0]["TmdbId"], handed);

    let (status, found) = get(&router, &format!("/v1/search?q=tmdb:{handed}&year=")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found[0]["TmdbId"], handed);

    // Named among the films changed, so that Radarr's scheduled refresh
    // reads it again.
    let (status, changed) = get(&router, "/v1/movie/changed?since=2020-01-01").await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert!(
        changed
            .as_array()
            .expect("ids")
            .iter()
            .any(|changed| *changed == handed),
        "{changed}"
    );

    let (status, _) = get(&router, &format!("/v1/movie/{}", named_by_none(handed))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Plex is given a work entered by hand under the catalogue's own id —
/// `show-id-…`, `movie-id-…` — which a real Plex Media Server takes: a match
/// by its title, then the record, its seasons and episodes, and its
/// pictures. No `Guid` names it elsewhere, since nothing does.
#[tokio::test]
async fn plex_matches_a_work_entered_by_hand_and_is_given_its_record_and_pictures() {
    let state = server().await;
    let router = surfaces(&state);
    let id = entered_by_hand(&router, vicineko()).await;
    let (status, _) = call(
        &router,
        "POST",
        &format!("/api/v1/items/{id}/images"),
        Some(json!({
            "coverType": "poster",
            "url": "https://images.example/vicineko.jpg",
            "sortOrder": 0,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let key = format!("show-id-{id}");

    for manual in [false, true] {
        let (status, matched) = call(
            &router,
            "POST",
            "/plex/library/metadata/matches",
            Some(json!({ "type": 2, "title": "Vicineko", "year": 2024, "manual": manual })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{matched}");
        let first = &matched["MediaContainer"]["Metadata"][0];
        assert_eq!(first["ratingKey"], key.as_str(), "{matched}");
        assert_eq!(
            first["guid"],
            format!("tv.plex.agents.custom.arrmetadataserver://show/{key}")
        );
        assert!(first.get("Guid").is_none_or(|g| g == &json!([])), "{first}");
    }

    let (status, show) = get(&router, &format!("/plex/library/metadata/{key}")).await;
    assert_eq!(status, StatusCode::OK, "{show}");
    let show = &show["MediaContainer"]["Metadata"][0];
    assert_eq!(show["title"], "Vicineko");
    assert_eq!(
        show["summary"],
        "A series no source lists, entered by hand."
    );
    assert_eq!(show["thumb"], "https://images.example/vicineko.jpg");
    assert_eq!(show["SeasonType"][0]["source"], "local");

    let (status, images) = get(&router, &format!("/plex/library/metadata/{key}/images")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(images["MediaContainer"]["Image"][0]["type"], "coverPoster");

    let (status, seasons) = get(&router, &format!("/plex/library/metadata/{key}/children")).await;
    assert_eq!(status, StatusCode::OK);
    let season = &seasons["MediaContainer"]["Metadata"][0];
    assert_eq!(season["ratingKey"], format!("season-id-{id}-s1"));
    let (status, episodes) = get(
        &router,
        &format!("/plex/library/metadata/season-id-{id}-s1/children"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let episodes = episodes["MediaContainer"]["Metadata"]
        .as_array()
        .expect("episodes");
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[1]["ratingKey"], format!("episode-id-{id}-s1e2"));
    assert_eq!(episodes[1]["title"], "Typed in, 2");

    // A film, the same way.
    let film = entered_by_hand(
        &router,
        json!({ "kind": "movie", "title": "Sans Source", "year": 2031 }),
    )
    .await;
    let (status, matched) = call(
        &router,
        "POST",
        "/plex/library/metadata/matches",
        Some(json!({ "type": 1, "title": "Sans Source", "year": 2031 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{matched}");
    assert_eq!(
        matched["MediaContainer"]["Metadata"][0]["ratingKey"],
        format!("movie-id-{film}")
    );
    let (status, _) = get(&router, &format!("/plex/library/metadata/movie-id-{film}")).await;
    assert_eq!(status, StatusCode::OK);
}
