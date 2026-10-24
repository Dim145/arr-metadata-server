//! Plex: this server as a metadata provider that Plex Media Server asks.
//!
//! Plex Media Server 1.43 and later take a library's metadata from any HTTP
//! API a person adds by its address — Settings › Metadata Agents › Add
//! Provider, then an agent that lists the provider — and Plex removes its
//! legacy agents in 2026. Plex reads the manifest at the address it was
//! given, then calls the paths the manifest names under it: a match for what
//! it scanned, the metadata of what it matched, a series' seasons and
//! episodes, an item's pictures. The shapes are Plex's: see [`crate::wire::plex`].
//!
//! The answers are the catalogue's, as every surface gives them: locks
//! honoured, in the language Plex asks in, and a work Plex asks for by its
//! id fetched and kept when the catalogue does not hold it yet — as asking
//! Sonarr's surface for a series does. A match by title fetches nothing to
//! keep: the providers' search lists what Plex may choose, and only the work
//! it then asks for is fetched. A match by an id Plex gives — a few of them
//! at most, each once — reaches the work that id names: fetched and kept for
//! a caller who may see works for adults, as an id is on every surface; for
//! one who may not, looked for in the catalogue alone, since what a fetch
//! brought back could be a work they may not see.
//!
//! Plex sends no credential to a provider yet, so this surface is guarded by
//! the caller's address, like Sonarr's; it is not for the open internet.

use std::collections::HashSet;

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, header},
};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::native::feeds::{self, Peer},
    auth::Identity,
    db::repo,
    domain::{EpisodeOrder, ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service::{self, ids, language, movie, orders, series},
    state::AppState,
    wire::plex::{
        self, Drawn, Hint, Key, Laid, Layout, PlexImagesResponse, PlexMatchRequest, PlexMetadata,
        PlexMetadataResponse, PlexProviderResponse, Series, Work,
    },
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Plex provider";

/// How many items a page holds when Plex asks for no size: the first 20, as
/// Plex asks of a provider (`docs/API Endpoints.md`, Paging).
const PAGE: usize = 20;

/// The most a page may hold, whatever Plex asks: a season of an anime in its
/// absolute order runs past a thousand episodes, and that is the size an
/// answer is allowed to reach.
const MOST_PER_PAGE: usize = 5_000;

/// How many works of the catalogue a title is looked for among: enough that
/// the one named exactly so is in it, when a popular word gives dozens.
const HELD_CANDIDATES: i64 = 50;

/// How many of the ids Plex gives for what it asks to match are tried, each
/// once. Plex sends a handful — this provider's own, IMDb's, TMDB's,
/// TheTVDB's, for the item and for its parents — and each one tried may be a
/// work fetched from the providers: a request naming thousands of ids would
/// otherwise be as many calls upstream.
const MOST_HINTS: usize = 10;

/// What every path of the provider begins with.
const BASE: &str = "/plex/";

/// The one fixed segment under `/plex`: any other first segment there is the
/// API key of an address that carries one.
const LIBRARY: &str = "library";

pub fn router() -> OpenApiRouter<AppState> {
    // `/matches` is a fixed segment, so it is matched before the
    // `{rating_key}` beside it whatever the order here.
    OpenApiRouter::new()
        .routes(routes!(provider))
        .routes(routes!(matches))
        .routes(routes!(metadata))
        .routes(routes!(children))
        .routes(routes!(grandchildren))
        .routes(routes!(images))
}

/// The same provider under an address that carries an API key,
/// `/plex/<key>`: what Plex is given when the provider asks for a key
/// (`AMS_PLEX_AUTH=apikey`). Plex sends no credential to a provider and keeps
/// no query, but calls the paths of the manifest under the address it was
/// given — so the key rides in the path, the guard reads it there
/// ([`key_in_path`]), and the handlers are the ones [`router`] documents.
pub fn keyed_router() -> axum::Router<AppState> {
    use axum::routing::{get, post};

    axum::Router::new().nest(
        "/plex/{key}",
        axum::Router::new()
            .route("/", get(provider))
            .route("/library/metadata/matches", post(matches))
            .route("/library/metadata/{rating_key}", get(metadata))
            .route("/library/metadata/{rating_key}/children", get(children))
            .route(
                "/library/metadata/{rating_key}/grandchildren",
                get(grandchildren),
            )
            .route("/library/metadata/{rating_key}/images", get(images)),
    )
}

/// The API key an address of the provider carries — `/plex/<key>/…` — and
/// nothing for `/plex` and the paths under it. What the guard reads a key
/// from when the provider asks for one, and what a log masks.
pub fn key_in_path(path: &str) -> Option<&str> {
    let segment = path.strip_prefix(BASE)?.split('/').next()?;
    (!segment.is_empty() && segment != LIBRARY).then_some(segment)
}

/// An item's path segment, `{rating_key}`: read by name, so that the key an
/// address may carry before it is left aside.
#[derive(Deserialize)]
struct Item {
    rating_key: String,
}

/// What Plex adds to a request besides its path. The `X-Plex-*` values may
/// come as headers instead, and a header wins, as the reference provider
/// reads them (`src/routes/tvRoutes.ts`).
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct PlexQuery {
    /// `1` for a series' seasons, or a season's episodes, as `Children`.
    #[serde(rename = "includeChildren")]
    include_children: Option<String>,
    /// The order to number a series' episodes by: one of its `SeasonType`
    /// ids — `official` for the aired order, `dvd`, `absolute`…
    #[serde(rename = "episodeOrder")]
    episode_order: Option<String>,
    /// The language to answer in, `fr-FR`.
    #[serde(rename = "X-Plex-Language")]
    language: Option<String>,
    /// The country whose rating, and whose release day, a work is given:
    /// `FR`.
    #[serde(rename = "X-Plex-Country")]
    country: Option<String>,
    /// The first item of the page, counted from 1.
    #[serde(rename = "X-Plex-Container-Start")]
    start: Option<String>,
    /// How many items the page holds: 20 when left out.
    #[serde(rename = "X-Plex-Container-Size")]
    size: Option<String>,
}

impl PlexQuery {
    fn include_children(&self) -> bool {
        self.include_children
            .as_deref()
            .and_then(plex::flag)
            .unwrap_or(false)
    }
}

/// The manifest: what the provider describes, and where it answers.
///
/// What a person adds to Plex is this address — under Settings › Metadata
/// Agents › Add Provider. Plex reads the paths of its two features under it.
#[utoipa::path(
    get, path = "/plex", tag = TAG,
    operation_id = "plex_provider",
    responses(
        (status = 200, body = PlexProviderResponse),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 503, description = "The Plex provider is switched off"),
    ),
    security(),
)]
async fn provider() -> Json<PlexProviderResponse> {
    Json(plex::provider())
}

/// What a file Plex scanned is: a film, a series, a season or an episode of
/// this catalogue.
///
/// By an id Plex gives, first — one of this provider's own, or IMDb's,
/// TMDB's, TheTVDB's — then by title and year: among the works the
/// catalogue holds, one named exactly so from a year either side; else the
/// search every client's lookup goes through, which asks the providers and
/// keeps nothing. The best match alone, or with `manual` every likely one,
/// best first. A season is matched by its series and number, an episode by
/// its series and its place — or the day it aired.
#[utoipa::path(
    post, path = "/plex/library/metadata/matches", tag = TAG,
    operation_id = "plex_matches",
    params(PlexQuery),
    request_body = PlexMatchRequest,
    responses(
        (status = 200, body = PlexMetadataResponse),
        (status = 400, description = "Not a match request: no JSON body, no type, or a film or a series with neither a title nor an id"),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 415, description = "The body is not sent as `application/json`"),
        (status = 503, description = "The Plex provider is switched off"),
    ),
    security(),
)]
async fn matches(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Query(query): Query<PlexQuery>,
    body: Bytes,
) -> AppResult<Json<PlexMetadataResponse>> {
    // JSON, said so. A web page may send another site a body of a few kinds
    // without asking it first — text, a form — and one on the local network
    // could otherwise have this server look up whatever it listed; a body
    // that says it is JSON is one a browser asks leave for first, and is not
    // given it here.
    if !says_json(&headers) {
        return Err(AppError::UnsupportedMediaType(
            "a match is asked with a JSON body, sent as application/json".into(),
        ));
    }
    let asked: PlexMatchRequest = serde_json::from_slice(&body)
        .map_err(|e| AppError::BadRequest(format!("a match is asked with a JSON body: {e}")))?;
    let asking = Asking::of(&state, &identity, &headers, &query, peer);
    let matcher = Matcher {
        state: &state,
        asked: &asked,
        asking: &asking,
        adult: state.adult_for(
            identity.client_id(),
            identity.peer_id(),
            asked.include_adult,
        ),
    };

    let found = match asked.kind {
        Some(1) => matcher.films().await?,
        Some(2) => matcher.shows().await?,
        Some(3) => matcher.season().await?,
        Some(4) => matcher.episode().await?,
        _ => {
            return Err(AppError::BadRequest(
                "`type` is 1 for a film, 2 for a series, 3 for a season, 4 for an episode".into(),
            ));
        }
    };
    let total = found.len();
    Ok(Json(plex::container(found, 0, total)))
}

/// An item by its ratingKey: a film, a series, a season or an episode.
///
/// A series' seasons, and a season's episodes, come with it as `Children`
/// when `includeChildren` is `1`; a series' seasons as the order
/// `episodeOrder` names lays them out — none for an order it has not.
#[utoipa::path(
    get, path = "/plex/library/metadata/{rating_key}", tag = TAG,
    operation_id = "plex_metadata",
    params(
        ("rating_key" = String, Path, description = "`movie-tmdb-603`, `show-tvdb-81189`, `season-tvdb-81189-s1`, `episode-tvdb-81189-s1e3`; a season's or an episode's in another order ends with it, `-dvd`"),
        PlexQuery,
    ),
    responses(
        (status = 200, body = PlexMetadataResponse),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No such item"),
        (status = 503, description = "The Plex provider is switched off"),
    ),
    security(),
)]
async fn metadata(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Path(Item { rating_key }): Path<Item>,
    Query(query): Query<PlexQuery>,
) -> AppResult<Json<PlexMetadataResponse>> {
    let key = Key::parse(&rating_key).ok_or(AppError::NotFound)?;
    let asking = Asking::of(&state, &identity, &headers, &query, peer);
    let (item, orders) = served(&state, &key, &asking, asked_order(&key, &query)).await?;
    let drawn = asking.drawn();
    let with_children = query.include_children();

    let answer = match &key {
        Key::Movie(work) => plex::movie(&item, work, &drawn),
        Key::Show(work) => {
            let layout = Layout::asked(&item, &orders, query.episode_order.as_deref());
            Series::new(&item, work, &layout, &drawn).show(&orders, with_children)
        }
        Key::Season {
            show,
            number,
            order,
        } => {
            let layout = Layout::asked(&item, &orders, order.as_deref());
            let laid = layout.season(*number).ok_or(AppError::NotFound)?;
            Series::new(&item, show, &layout, &drawn).season(laid, with_children)
        }
        Key::Episode {
            show,
            season,
            number,
            order,
        } => {
            let layout = Layout::asked(&item, &orders, order.as_deref());
            let laid = layout.season(*season).ok_or(AppError::NotFound)?;
            let episode = laid.episode(*number).ok_or(AppError::NotFound)?;
            Series::new(&item, show, &layout, &drawn).episode(laid, *number, episode)
        }
    };
    Ok(Json(plex::container(vec![answer], 0, 1)))
}

/// A series' seasons, or a season's episodes, a page at a time.
///
/// `X-Plex-Container-Start` names the page's first item, counted from 1;
/// `X-Plex-Container-Size` how many it holds, 20 when left out. A film and
/// an episode have none.
#[utoipa::path(
    get, path = "/plex/library/metadata/{rating_key}/children", tag = TAG,
    operation_id = "plex_children",
    params(
        ("rating_key" = String, Path, description = "A series' or a season's ratingKey"),
        PlexQuery,
    ),
    responses(
        (status = 200, body = PlexMetadataResponse),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No such item"),
        (status = 503, description = "The Plex provider is switched off"),
    ),
    security(),
)]
async fn children(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Path(Item { rating_key }): Path<Item>,
    Query(query): Query<PlexQuery>,
) -> AppResult<Json<PlexMetadataResponse>> {
    let key = Key::parse(&rating_key).ok_or(AppError::NotFound)?;
    let asking = Asking::of(&state, &identity, &headers, &query, peer);
    let (item, orders) = served(&state, &key, &asking, asked_order(&key, &query)).await?;
    let drawn = asking.drawn();
    let page = Page::of(&headers, &query);

    let (drawn_page, total) = match &key {
        Key::Show(work) => {
            let layout = Layout::asked(&item, &orders, query.episode_order.as_deref());
            let series = Series::new(&item, work, &layout, &drawn);
            let total = series.season_count();
            (series.seasons(page.range(total)), total)
        }
        Key::Season {
            show,
            number,
            order,
        } => {
            let layout = Layout::asked(&item, &orders, order.as_deref());
            let laid = layout.season(*number).ok_or(AppError::NotFound)?;
            let series = Series::new(&item, show, &layout, &drawn);
            let total = laid.episodes.len();
            (series.episodes_of(laid, page.range(total)), total)
        }
        Key::Movie(_) | Key::Episode { .. } => (Vec::new(), 0),
    };
    Ok(Json(plex::container(
        drawn_page,
        page.offset.min(total),
        total,
    )))
}

/// A series' episodes, every season's, a page at a time — paged as
/// `/children` is.
#[utoipa::path(
    get, path = "/plex/library/metadata/{rating_key}/grandchildren", tag = TAG,
    operation_id = "plex_grandchildren",
    params(
        ("rating_key" = String, Path, description = "A series' ratingKey"),
        PlexQuery,
    ),
    responses(
        (status = 200, body = PlexMetadataResponse),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No such item"),
        (status = 503, description = "The Plex provider is switched off"),
    ),
    security(),
)]
async fn grandchildren(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Path(Item { rating_key }): Path<Item>,
    Query(query): Query<PlexQuery>,
) -> AppResult<Json<PlexMetadataResponse>> {
    let key = Key::parse(&rating_key).ok_or(AppError::NotFound)?;
    let asking = Asking::of(&state, &identity, &headers, &query, peer);
    let (item, orders) = served(&state, &key, &asking, asked_order(&key, &query)).await?;
    let drawn = asking.drawn();
    let page = Page::of(&headers, &query);

    let (drawn_page, total) = match &key {
        Key::Show(work) => {
            let layout = Layout::asked(&item, &orders, query.episode_order.as_deref());
            let series = Series::new(&item, work, &layout, &drawn);
            let total = series.episode_count();
            (series.episodes(page.range(total)), total)
        }
        _ => (Vec::new(), 0),
    };
    Ok(Json(plex::container(
        drawn_page,
        page.offset.min(total),
        total,
    )))
}

/// An item's pictures: a work's posters, backgrounds and logos, the one it
/// leads with first of each kind; a season's posters; an episode's still.
#[utoipa::path(
    get, path = "/plex/library/metadata/{rating_key}/images", tag = TAG,
    operation_id = "plex_images",
    params(
        ("rating_key" = String, Path, description = "Any item's ratingKey"),
        PlexQuery,
    ),
    responses(
        (status = 200, body = PlexImagesResponse),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No such item"),
        (status = 503, description = "The Plex provider is switched off"),
    ),
    security(),
)]
async fn images(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    peer: Peer,
    headers: HeaderMap,
    Path(Item { rating_key }): Path<Item>,
    Query(query): Query<PlexQuery>,
) -> AppResult<Json<PlexImagesResponse>> {
    let key = Key::parse(&rating_key).ok_or(AppError::NotFound)?;
    let asking = Asking::of(&state, &identity, &headers, &query, peer);
    // Pictures need no text: the work as it is held, in no other language.
    let item = work(&state, &key).await?.ok_or(AppError::NotFound)?;
    let drawn = asking.drawn();

    let listed = match &key {
        Key::Movie(_) | Key::Show(_) => {
            plex::images_of(&item.images, &item.primary_images, &item.title, &drawn)
        }
        Key::Season { number, order, .. } => {
            let orders = orders::for_clients(&state, &item, order.as_deref()).await?;
            let layout = Layout::asked(&item, &orders, order.as_deref());
            let laid = layout.season(*number).ok_or(AppError::NotFound)?;
            laid.season
                .map(|season| {
                    plex::images_of(&season.images, &season.primary_images, &item.title, &drawn)
                })
                .unwrap_or_default()
        }
        Key::Episode {
            season,
            number,
            order,
            ..
        } => {
            let orders = orders::for_clients(&state, &item, order.as_deref()).await?;
            let layout = Layout::asked(&item, &orders, order.as_deref());
            let episode = layout
                .season(*season)
                .and_then(|laid| laid.episode(*number))
                .ok_or(AppError::NotFound)?;
            plex::snapshot(episode, &episode.title, &drawn)
        }
    };
    Ok(Json(plex::image_container(listed)))
}

// ─── what a request asks besides its item ───────────────────────────────────

/// What a request asks of the answer besides the item: its language, its
/// country, and where this server is reached.
struct Asking {
    /// The language to answer in, as a tag: Plex's, else the caller's own.
    language: String,
    /// The caller's own: `tmdb.language` on their address's rule, or the
    /// server's. What the work is served in when it is not named in Plex's,
    /// before English, then the language it is stored in — the chain
    /// Sonarr v5 is answered by.
    own: String,
    /// Three letters, for the words this server writes itself.
    words: String,
    /// ISO 3166-1 alpha-2.
    country: Option<String>,
    origin: String,
    stored_language: bool,
}

impl Asking {
    fn of(
        state: &AppState,
        identity: &Identity,
        headers: &HeaderMap,
        query: &PlexQuery,
        peer: Peer,
    ) -> Self {
        let own = state.language(identity.client_id(), identity.peer_id());
        // A tag, or nothing: what is no language is taken for none asked,
        // not refused — Plex sends what its person chose, and a 400 would
        // fail the refresh.
        let language = sent(headers, "x-plex-language", query.language.as_deref())
            .and_then(|raw| language::tag(&raw))
            .unwrap_or_else(|| own.clone());
        let country = sent(headers, "x-plex-country", query.country.as_deref())
            .map(|raw| raw.trim().to_ascii_uppercase())
            .filter(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase()))
            .or_else(|| state.watch_region(identity.client_id(), identity.peer_id()));
        let stored = language::normalize(&state.language(None, None));

        Self {
            words: language::three_letter(&language),
            stored_language: language::normalize(&language) == stored,
            origin: feeds::origin_of(state, headers, peer),
            language,
            own,
            country,
        }
    }

    fn drawn(&self) -> Drawn<'_> {
        Drawn {
            origin: &self.origin,
            language: &self.words,
            country: self.country.as_deref(),
            stored_language: self.stored_language,
        }
    }
}

/// Whether a request says its body is JSON: `application/json`, whatever
/// its case and its parameters — `; charset=utf-8`.
fn says_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json"))
}

/// One of the values Plex may send as a header or in the query: the header
/// first.
fn sent(headers: &HeaderMap, name: &str, query: Option<&str>) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .or(query)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// A page, as Plex asks for one (`docs/API Endpoints.md`, Paging): from the
/// item `X-Plex-Container-Start` names — counted from 1, as the reference
/// provider counts, a 0 taken for the first item rather than for none — and
/// `X-Plex-Container-Size` of them; the first 20 when it names none.
struct Page {
    /// Where the page starts, from 0: what the answer's `offset` says.
    offset: usize,
    size: usize,
}

impl Page {
    fn of(headers: &HeaderMap, query: &PlexQuery) -> Self {
        let number = |name: &str, asked: Option<&str>| {
            sent(headers, name, asked).and_then(|value| value.parse::<usize>().ok())
        };
        let start = number("x-plex-container-start", query.start.as_deref()).unwrap_or(1);
        let size = number("x-plex-container-size", query.size.as_deref()).unwrap_or(PAGE);
        Self {
            offset: start.max(1) - 1,
            size: size.min(MOST_PER_PAGE),
        }
    }

    fn range(&self, total: usize) -> std::ops::Range<usize> {
        let from = self.offset.min(total);
        from..from.saturating_add(self.size).min(total)
    }
}

// ─── the work ────────────────────────────────────────────────────────────────

/// The work a key belongs to, as every surface resolves one: held, or due
/// and fetched again, or fetched and kept when the catalogue does not hold
/// it yet. `None` when nothing has it, or what it names is not of the kind
/// the key says.
async fn work(state: &AppState, key: &Key) -> AppResult<Option<MediaItem>> {
    let film = matches!(key, Key::Movie(_));
    let item = match (film, key.work()) {
        (_, Work::Held(id)) => service::load(state, id).await?,
        (true, Work::Tmdb(id)) => movie::by_tmdb_id(state, *id).await?,
        (true, Work::Imdb(id)) => movie::by_imdb_id(state, id).await?,
        (true, Work::Tvdb(_) | Work::Fankai(_)) => None,
        (false, Work::Tvdb(id)) => series::by_tvdb_id(state, *id).await?,
        (false, Work::Tmdb(id)) => series::by_tmdb_id(state, *id).await?,
        (false, Work::Fankai(id)) => series::by_fankai_id(state, *id).await?,
        (false, Work::Imdb(id)) => series::by_imdb_id(state, id).await?,
    };
    let kind = if film {
        MediaKind::Movie
    } else {
        MediaKind::Series
    };
    Ok(item.filter(|item| item.kind == kind))
}

/// The order an item is asked in: the one its key names, for a season or an
/// episode; the one Plex names, for a series.
fn asked_order<'k>(key: &'k Key, query: &'k PlexQuery) -> Option<&'k str> {
    match key {
        Key::Season { order, .. } | Key::Episode { order, .. } => order.as_deref(),
        _ => query.episode_order.as_deref(),
    }
}

/// The work, in the language asked — its title and overview, a series'
/// episodes too — with the orders the catalogue keeps for a series, the one
/// asked for read from its source first when it is still to be.
async fn served(
    state: &AppState,
    key: &Key,
    asking: &Asking,
    asked_order: Option<&str>,
) -> AppResult<(MediaItem, Vec<EpisodeOrder>)> {
    let mut item = work(state, key).await?.ok_or(AppError::NotFound)?;
    language::apply_with_fallback(state, &mut item, &asking.language, &asking.own).await?;
    let orders = match item.kind {
        MediaKind::Series => orders::for_clients(state, &item, asked_order).await?,
        MediaKind::Movie => Vec::new(),
    };
    Ok((item, orders))
}

/// The work as the catalogue holds it, whole, when it holds it: what a
/// match draws a series' seasons from. A search result it does not hold yet
/// stays as it was found, since a match fetches nothing.
async fn held_whole(state: &AppState, item: &MediaItem) -> AppResult<Option<MediaItem>> {
    held_as(state, item.kind, &Work::of(item)).await
}

/// The work of `kind` the catalogue holds under the id `work` names, whole,
/// with its locks: read from the store alone, and nothing fetched — not even
/// when it is due again.
async fn held_as(state: &AppState, kind: MediaKind, work: &Work) -> AppResult<Option<MediaItem>> {
    // The work holding the id, or the one a merge folded the holder into:
    // Plex keeps the key it was handed, and is answered under it.
    let by = |source: ExternalSource, value: String| async move {
        repo::alias::work_for_external(&state.db, source, &value).await
    };
    let id = match work {
        Work::Held(id) => Some(id.clone()),
        Work::Tvdb(id) => by(ExternalSource::TvdbSeries, id.to_string()).await?,
        Work::Tmdb(id) => by(ExternalSource::tmdb_for(kind), id.to_string()).await?,
        Work::Imdb(id) => by(ExternalSource::Imdb, id.clone()).await?,
        Work::Fankai(id) => by(ExternalSource::Fankai, id.to_string()).await?,
    };
    match id {
        Some(id) => Ok(service::load(state, &id)
            .await?
            .filter(|whole| whole.kind == kind)),
        None => Ok(None),
    }
}

// ─── matching ────────────────────────────────────────────────────────────────

struct Matcher<'a> {
    state: &'a AppState,
    asked: &'a PlexMatchRequest,
    asking: &'a Asking,
    /// Whether this caller may be matched to a work for adults: as the
    /// server, the caller's rule and Plex's `includeAdult` decide.
    adult: bool,
}

impl Matcher<'_> {
    fn manual(&self) -> bool {
        self.asked.manual.unwrap_or(false)
    }

    fn may_see(&self, item: &MediaItem) -> bool {
        self.adult || !item.is_adult
    }

    /// The work a key names, as a match may reach it. For a caller who may
    /// see works for adults, as every surface resolves one: held, or fetched
    /// and kept. For one who may not, only a work the catalogue holds already
    /// and may show them, told by the flag it is held with — before anything
    /// is asked of a provider, a refresh included. One not held yet is not
    /// fetched for them: what came back could be a work they may not see,
    /// kept for nobody. Plex asks for the work it chose by its key afterwards,
    /// and that one is fetched as any other is.
    async fn reach(&self, key: &Key) -> AppResult<Option<MediaItem>> {
        if !self.adult {
            let kind = match key {
                Key::Movie(_) => MediaKind::Movie,
                _ => MediaKind::Series,
            };
            match held_as(self.state, kind, key.work()).await? {
                Some(held) if self.may_see(&held) => {}
                _ => return Ok(None),
            }
        }
        // A refresh due may yet find it is for adults.
        Ok(work(self.state, key)
            .await?
            .filter(|item| self.may_see(item)))
    }

    /// The year Plex gives, when it is a year.
    fn year(&self) -> Option<i32> {
        self.asked
            .year
            .and_then(|year| i32::try_from(year).ok())
            .filter(|year| (1800..=3000).contains(year))
    }

    /// A film: by an id Plex gave, else by its title and year.
    async fn films(&self) -> AppResult<Vec<PlexMetadata>> {
        let hints = hints(own_or_named(self.asked));
        for hint in &hints {
            let key = match hint {
                Hint::Own(key @ Key::Movie(_)) => key.clone(),
                Hint::Tmdb(id) => Key::Movie(Work::Tmdb(*id)),
                Hint::Imdb(id) => Key::Movie(Work::Imdb(id.clone())),
                _ => continue,
            };
            if let Some(item) = self.reach(&key).await? {
                // This provider's own key is kept as Plex holds it.
                let work = match hint {
                    Hint::Own(key) => key.work().clone(),
                    _ => Work::of(&item),
                };
                return Ok(vec![self.film(item, &work)]);
            }
        }

        let Some(title) = self.asked.title.as_deref() else {
            return self.nothing_named(&hints, "a film is matched by its title or an id");
        };
        let found = self
            .by_title(MediaKind::Movie, title, self.year(), self.manual())
            .await?;
        Ok(found
            .into_iter()
            .map(|item| {
                let work = Work::of(&item);
                self.film(item, &work)
            })
            .collect())
    }

    fn film(&self, mut item: MediaItem, work: &Work) -> PlexMetadata {
        language::apply_shallow_with_fallback(
            self.state,
            &mut item,
            &self.asking.language,
            &self.asking.own,
        );
        plex::movie(&item, work, &self.asking.drawn())
    }

    /// A series: by an id Plex gave, else by its title and year. The one
    /// match comes with its seasons when Plex asks for them and the
    /// catalogue holds the series.
    async fn shows(&self) -> AppResult<Vec<PlexMetadata>> {
        let hints = hints(own_or_named(self.asked));
        if let Some((item, work)) = self.series_by(&hints).await? {
            return Ok(vec![self.show(item, &work, self.wants_children()).await?]);
        }

        let Some(title) = self.asked.title.as_deref() else {
            return self.nothing_named(&hints, "a series is matched by its title or an id");
        };
        let found = self
            .by_title(MediaKind::Series, title, self.year(), self.manual())
            .await?;
        let alone = found.len() == 1 && self.wants_children();
        let mut drawn = Vec::with_capacity(found.len());
        for item in found {
            let work = Work::of(&item);
            drawn.push(self.show(item, &work, alone).await?);
        }
        Ok(drawn)
    }

    fn wants_children(&self) -> bool {
        !self.manual() && self.asked.include_children.unwrap_or(false)
    }

    /// A series as a match: its seasons laid out in the order asked, as
    /// `Children`, for the one match of a series the catalogue holds. A
    /// series it does not hold yet is fetched when Plex asks for it, not
    /// here.
    async fn show(
        &self,
        mut item: MediaItem,
        work: &Work,
        with_children: bool,
    ) -> AppResult<PlexMetadata> {
        let drawn = self.asking.drawn();
        let whole = if with_children {
            held_whole(self.state, &item).await?
        } else {
            None
        };
        let Some(mut whole) = whole else {
            language::apply_shallow_with_fallback(
                self.state,
                &mut item,
                &self.asking.language,
                &self.asking.own,
            );
            let layout = Layout::asked(&item, &[], None);
            return Ok(Series::new(&item, work, &layout, &drawn).show(&[], false));
        };

        language::apply_shallow_with_fallback(
            self.state,
            &mut whole,
            &self.asking.language,
            &self.asking.own,
        );
        let orders =
            orders::for_clients(self.state, &whole, self.asked.episode_order.as_deref()).await?;
        let layout = Layout::asked(&whole, &orders, self.asked.episode_order.as_deref());
        Ok(Series::new(&whole, work, &layout, &drawn).show(&orders, true))
    }

    /// A season, by its series and its number — or this provider's own key
    /// for it.
    async fn season(&self) -> AppResult<Vec<PlexMetadata>> {
        let hints = hints(
            own_or_named(self.asked)
                .into_iter()
                .filter(|guid| Key::from_guid(guid).is_some())
                .chain(self.asked.parent_guid.iter().map(String::as_str))
                .chain(self.asked.parent_guids.iter().map(String::as_str)),
        );
        let own = own_place(&hints);
        let number = match own {
            Some(Key::Season { number, .. }) => Some(*number),
            _ => self.asked.index.and_then(|n| i32::try_from(n).ok()),
        };
        let Some(number) = number else {
            return Ok(Vec::new());
        };
        let Some((mut item, work)) = self
            .series_of(&hints, self.asked.parent_title.as_deref())
            .await?
        else {
            return Ok(Vec::new());
        };

        language::apply_with_fallback(
            self.state,
            &mut item,
            &self.asking.language,
            &self.asking.own,
        )
        .await?;
        let orders = orders::for_clients(self.state, &item, self.order_of(own)).await?;
        let layout = Layout::asked(&item, &orders, self.order_of(own));
        let drawn = self.asking.drawn();
        let series = Series::new(&item, &work, &layout, &drawn);
        Ok(layout
            .season(number)
            .map(|laid| series.season(laid, self.asked.include_children.unwrap_or(false)))
            .into_iter()
            .collect())
    }

    /// An episode, by its series and its place — its season and number in
    /// the order asked — or the day it aired; or this provider's own key.
    async fn episode(&self) -> AppResult<Vec<PlexMetadata>> {
        // Plex's ids for an episode, and for its season, are theirs and no
        // series': only this provider's own keys are read among them.
        let own_keys = own_or_named(self.asked)
            .into_iter()
            .chain(self.asked.parent_guid.iter().map(String::as_str))
            .chain(self.asked.parent_guids.iter().map(String::as_str))
            .filter(|guid| Key::from_guid(guid).is_some());
        let hints = hints(
            own_keys
                .chain(self.asked.grandparent_guid.iter().map(String::as_str))
                .chain(self.asked.grandparent_guids.iter().map(String::as_str)),
        );
        let own = own_place(&hints);
        let Some((mut item, work)) = self
            .series_of(&hints, self.asked.grandparent_title.as_deref())
            .await?
        else {
            return Ok(Vec::new());
        };

        language::apply_with_fallback(
            self.state,
            &mut item,
            &self.asking.language,
            &self.asking.own,
        )
        .await?;
        let orders = orders::for_clients(self.state, &item, self.order_of(own)).await?;
        let layout = Layout::asked(&item, &orders, self.order_of(own));
        let place = match own {
            Some(Key::Episode { season, number, .. }) => Some((*season, *number)),
            _ => match (self.asked.parent_index, self.asked.index) {
                (Some(season), Some(number)) => {
                    i32::try_from(season).ok().zip(i32::try_from(number).ok())
                }
                _ => None,
            },
        };
        let found = match place {
            Some((season, number)) => layout
                .season(season)
                .and_then(|laid| laid.episode(number).map(|episode| (laid, number, episode))),
            None => self
                .asked
                .date
                .as_deref()
                .and_then(|date| aired_on(&layout, date)),
        };

        let drawn = self.asking.drawn();
        let series = Series::new(&item, &work, &layout, &drawn);
        Ok(found
            .map(|(laid, number, episode)| series.episode(laid, number, episode))
            .into_iter()
            .collect())
    }

    /// The order a season or an episode is numbered by: the one this
    /// provider's own key for it names, else the one Plex asks in.
    fn order_of<'k>(&'k self, own: Option<&'k Key>) -> Option<&'k str> {
        match own {
            Some(Key::Season { order, .. } | Key::Episode { order, .. }) => order.as_deref(),
            _ => self.asked.episode_order.as_deref(),
        }
    }

    /// The series a season or an episode belongs to, whole: by the key or
    /// the ids Plex gave for it, else by its title — fetched and kept if the
    /// catalogue does not hold it yet, as one asked for by id is, since its
    /// seasons are what is matched; for a caller who may not see works for
    /// adults, only one the catalogue holds ([`Self::reach`]).
    async fn series_of(
        &self,
        hints: &[Hint],
        title: Option<&str>,
    ) -> AppResult<Option<(MediaItem, Work)>> {
        if let Some(found) = self.series_by(hints).await? {
            return Ok(Some(found));
        }
        let Some(title) = title else {
            return Ok(None);
        };
        // A season's year is not its series': the title alone.
        let Some(found) = self
            .by_title(MediaKind::Series, title, None, false)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let named = Work::of(&found);
        Ok(self
            .reach(&Key::Show(named.clone()))
            .await?
            .map(|item| (item, named)))
    }

    /// A series by the first of Plex's ids that names one.
    async fn series_by(&self, hints: &[Hint]) -> AppResult<Option<(MediaItem, Work)>> {
        for hint in hints {
            let named = match hint {
                Hint::Own(key) if !matches!(key, Key::Movie(_)) => key.work().clone(),
                Hint::Tvdb(id) => Work::Tvdb(*id),
                Hint::Tmdb(id) => Work::Tmdb(*id),
                Hint::Imdb(id) => Work::Imdb(id.clone()),
                Hint::Own(_) => continue,
            };
            if let Some(item) = self.reach(&Key::Show(named.clone())).await? {
                // This provider's own key is kept as Plex holds it.
                let named = match hint {
                    Hint::Own(_) => named,
                    _ => Work::of(&item),
                };
                return Ok(Some((item, named)));
            }
        }
        Ok(None)
    }

    /// Works by title, and year where Plex knows it: the catalogue's first,
    /// where one named exactly so — by any title it goes by here, folded of
    /// case and accents — from a year either side of Plex's is the match;
    /// else the search every client's lookup goes through. The best alone,
    /// or with `manual` every likely one, best first. Nothing is fetched or
    /// kept for any of them: the one Plex then asks for is.
    async fn by_title(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
        manual: bool,
    ) -> AppResult<Vec<MediaItem>> {
        let wanted = slug::slugify(title);
        let mut held = self.catalogue(kind, title).await?;
        if !manual && let Some(at) = best_named(&held, &wanted, year) {
            return Ok(vec![held.swap_remove(at)]);
        }

        // A title that names an id — `tmdb:603` — is a lookup, which fetches
        // and keeps the work it names: for a caller who may not see works for
        // adults it is not made, as an id Plex gives is not followed for them
        // ([`Self::reach`]).
        let names_an_id = !matches!(ids::classify(title), ids::TermLookup::Text(_));
        let searched = if names_an_id && !self.adult {
            Vec::new()
        } else {
            match kind {
                MediaKind::Movie => movie::search(self.state, title, year).await?,
                MediaKind::Series => series::search(self.state, title).await?,
            }
        };
        let mut searched: Vec<MediaItem> = searched
            .into_iter()
            .filter(|item| item.kind == kind && self.may_see(item))
            .collect();

        if !manual {
            // The search ranks what it found by itself: its first of a year
            // that fits, where none is named exactly so.
            let at = best_named(&searched, &wanted, year)
                .or_else(|| searched.iter().position(|item| year_fits(year, item.year)));
            return Ok(at.map(|at| searched.swap_remove(at)).into_iter().collect());
        }

        // Every likely one, each once: named exactly so and of a year that
        // fits first, then named so, then the rest — the catalogue's before
        // the search's within each.
        let mut listed: Vec<MediaItem> = held.into_iter().chain(searched).collect();
        listed.sort_by_key(|item| {
            let named = named(item, &wanted);
            std::cmp::Reverse(u8::from(named) * 2 + u8::from(named && year_fits(year, item.year)))
        });
        let mut seen = HashSet::new();
        listed.retain(|item| seen.insert(Work::of(item)));
        listed.truncate(self.state.search_limit());
        Ok(listed)
    }

    /// The works of the catalogue a title may name: those it is found in by
    /// the catalogue's own search, switched on, with what a card draws of
    /// them and the locks laid over it.
    async fn catalogue(&self, kind: MediaKind, title: &str) -> AppResult<Vec<MediaItem>> {
        let query = repo::item::Query {
            term: Some(title.to_string()),
            kind: Some(kind),
            include_adult: self.adult,
            // The best named first, by their letters alone: a work that only
            // resembles a file's name is no match for it.
            sort: repo::item::Sort::Relevance,
            tolerance: Some(Vec::new()),
            limit: HELD_CANDIDATES,
            ..Default::default()
        };
        let mut items = repo::item::search(&self.state.db, &query).await?;
        repo::item::load_artwork(&self.state.db, &mut items).await?;
        service::apply_overrides(self.state, &mut items).await?;
        for item in &mut items {
            self.state.media.localize(item);
        }
        Ok(items)
    }

    /// Nothing to match by: an empty answer when Plex gave ids that named
    /// nothing here, a refusal when it gave nothing at all.
    fn nothing_named(&self, hints: &[Hint], why: &str) -> AppResult<Vec<PlexMetadata>> {
        let gave_ids = self.asked.guid.is_some() || !self.asked.guids.is_empty();
        if gave_ids || !hints.is_empty() {
            Ok(Vec::new())
        } else {
            Err(AppError::BadRequest(why.to_string()))
        }
    }
}

/// The GUIDs Plex gave for the item itself.
fn own_or_named(asked: &PlexMatchRequest) -> Vec<&str> {
    asked
        .guid
        .iter()
        .chain(&asked.guids)
        .map(String::as_str)
        .collect()
}

/// The ids Plex gave that this provider reads, in the order given: each
/// once, and no more than [`MOST_HINTS`] of them.
fn hints<'a>(guids: impl IntoIterator<Item = &'a str>) -> Vec<Hint> {
    let mut kept: Vec<Hint> = Vec::with_capacity(MOST_HINTS);
    for hint in guids.into_iter().filter_map(Hint::parse) {
        if kept.len() == MOST_HINTS {
            break;
        }
        if !kept.contains(&hint) {
            kept.push(hint);
        }
    }
    kept
}

/// This provider's own key for a season or an episode, among Plex's ids.
fn own_place(hints: &[Hint]) -> Option<&Key> {
    hints.iter().find_map(|hint| match hint {
        Hint::Own(key @ (Key::Season { .. } | Key::Episode { .. })) => Some(key),
        _ => None,
    })
}

/// The first episode of the layout that aired on `date`.
fn aired_on<'l, 'a>(
    layout: &'l Layout<'a>,
    date: &str,
) -> Option<(&'l Laid<'a>, i32, &'l crate::domain::Episode)> {
    let day = crate::wire::sonarr::sonarr_day(Some(date))?;
    layout.seasons.iter().find_map(|laid| {
        laid.episodes.iter().find_map(|(number, episode)| {
            (crate::wire::sonarr::sonarr_day(episode.air_date.as_deref()).as_deref()
                == Some(day.as_str()))
            .then_some((laid, *number, episode.as_ref()))
        })
    })
}

/// The one named exactly so, of a year that fits: the one of the very
/// year first, then the first listed.
fn best_named(items: &[MediaItem], wanted: &str, year: Option<i32>) -> Option<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| named(item, wanted) && year_fits(year, item.year))
        .min_by_key(|(at, item)| (year.is_none() || item.year != year, *at))
        .map(|(at, _)| at)
}

/// Whether a work goes by the title asked, in any language it is held in.
fn named(item: &MediaItem, wanted: &str) -> bool {
    if wanted.is_empty() {
        return false;
    }
    std::iter::once(item.title.as_str())
        .chain(item.original_title.as_deref())
        .chain(item.sort_title.as_deref())
        .chain(item.translations.iter().filter_map(|t| t.title.as_deref()))
        .chain(item.alternative_titles.iter().map(|t| t.title.as_str()))
        .any(|title| slug::slugify(title) == wanted)
}

/// Whether a work's year fits the one Plex gives: a year either side, since
/// a film's year is its première's for one source and its release's for
/// another. Either unknown, it fits.
fn year_fits(asked: Option<i32>, held: Option<i32>) -> bool {
    match (asked, held) {
        (Some(asked), Some(held)) => (asked - held).abs() <= 1,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::{
        Router,
        body::Body,
        extract::ConnectInfo,
        http::{Request, StatusCode},
        middleware::from_fn_with_state,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt as _;

    use super::*;
    use crate::{
        config,
        db::repo::{
            child::{blank_episode, blank_season},
            item::ItemWrite,
        },
        domain::{CoverType, ExternalIds, Image, OrderKind, PlacedEpisode, Translation},
    };

    /// The whole server on a database of its own, stored in English, with no
    /// provider to ask: what a match falls back to finds nothing upstream.
    async fn server() -> AppState {
        server_asking(config::SurfacePolicy::Allowlist).await
    }

    /// The same, the provider asking `policy` of its callers.
    async fn server_asking(policy: config::SurfacePolicy) -> AppState {
        let mut config = config::Config::from_env().expect("a configuration");
        config.security.plex_policy = policy;
        config.security.auth_disabled = false;
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
        config.skyhook.fallback = false;
        config.skyhook.enrich = false;
        config.radarr_metadata.fallback = false;
        config.radarr_metadata.enrich = false;
        config.server.public_url = Some("http://ams.lan:8479".into());

        AppState::bootstrap(config).await.expect("a server")
    }

    /// The provider's routes behind its own guard, as the server mounts them.
    fn door(state: &AppState) -> Router {
        let (routes, _) = router().split_for_parts();
        routes
            .merge(keyed_router())
            .layer(from_fn_with_state(
                state.clone(),
                crate::auth::middleware::guard_plex,
            ))
            .with_state(state.clone())
    }

    /// Ask the provider as Plex would, from `from`.
    async fn ask_from(
        state: &AppState,
        from: [u8; 4],
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut request = request
            .body(match body {
                Some(body) => Body::from(body.to_string()),
                None => Body::empty(),
            })
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((from, 32400))));
        let response = door(state).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// From the machine itself, which the allowlist lets in by default.
    async fn ask(state: &AppState, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
        ask_from(state, [127, 0, 0, 1], "GET", uri, headers, None).await
    }

    async fn matched(state: &AppState, body: Value) -> Value {
        let (status, answer) = ask_from(
            state,
            [127, 0, 0, 1],
            "POST",
            "/plex/library/metadata/matches",
            &[("content-type", "application/json")],
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        answer["MediaContainer"].clone()
    }

    async fn store(state: &AppState, work: &MediaItem) {
        repo::item::upsert(
            &state.db,
            ItemWrite {
                item: work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
    }

    /// *The Matrix*, held, named in French besides, its poster kept here.
    async fn the_matrix(state: &AppState) -> MediaItem {
        let mut film = MediaItem::empty(MediaKind::Movie);
        film.title = "The Matrix".into();
        film.slug = "the-matrix-1999".into();
        film.year = Some(1999);
        film.in_cinemas = Some("1999-03-31".into());
        film.overview = Some("A hacker learns what the world is.".into());
        film.external_ids = ExternalIds {
            tmdb: Some(603),
            imdb: Some("tt0133093".into()),
            ..Default::default()
        };
        film.translations = vec![Translation {
            language: "fra".into(),
            title: Some("Matrix".into()),
            overview: Some("Un pirate apprend ce qu'est le monde.".into()),
            tagline: None,
            is_manual: false,
        }];
        film.images = vec![Image {
            id: "poster".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "/media/matrix.jpg".into(),
            language: None,
            sort_order: 0,
            source: None,
            // As a provider gave it: what is added by hand is kept apart,
            // and not written with the work.
            is_manual: false,
        }];
        store(state, &film).await;
        film
    }

    /// *Rurouni Kenshin* (2023), held: two seasons, three episodes, named in
    /// French besides, and a DVD order.
    async fn kenshin(state: &AppState) -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = "Rurouni Kenshin".into();
        work.slug = "rurouni-kenshin-2023".into();
        work.year = Some(2023);
        work.first_aired = Some("2023-07-07".into());
        work.external_ids = ExternalIds {
            tvdb: Some(416802),
            tmdb: Some(209867),
            ..Default::default()
        };
        work.translations = vec![Translation {
            language: "fra".into(),
            title: Some("Kenshin le vagabond".into()),
            overview: None,
            tagline: None,
            is_manual: false,
        }];
        for (number, title) in [(1, "東京編"), (2, "Season 2")] {
            let mut season = blank_season(number);
            season.is_manual = false;
            season.title = Some(title.into());
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
        store(state, &work).await;
        let place = PlacedEpisode::of_tvdb;
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
                episodes: vec![place(1002, 1, 1), place(1001, 1, 2), place(2001, 2, 1)],
            }],
        )
        .await
        .expect("the order kept");
        work
    }

    #[tokio::test]
    async fn the_manifest_names_films_and_series_and_both_features() {
        let state = server().await;
        let (status, answer) = ask(&state, "/plex", &[]).await;
        assert_eq!(status, StatusCode::OK);
        let provider = &answer["MediaProvider"];
        assert_eq!(provider["identifier"], plex::IDENTIFIER);
        let types: Vec<i64> = provider["Types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["type"].as_i64().unwrap())
            .collect();
        assert_eq!(types, [1, 2, 3, 4]);
        let features: Vec<(&str, &str)> = provider["Feature"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["type"].as_str().unwrap(), f["key"].as_str().unwrap()))
            .collect();
        assert_eq!(
            features,
            [
                ("metadata", "/library/metadata"),
                ("match", "/library/metadata/matches")
            ]
        );
    }

    #[tokio::test]
    async fn an_address_the_allowlist_does_not_hold_is_refused() {
        let state = server().await;
        let (status, _) = ask_from(&state, [203, 0, 113, 9], "GET", "/plex", &[], None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = ask_from(
            &state,
            [203, 0, 113, 9],
            "POST",
            "/plex/library/metadata/matches",
            &[("content-type", "application/json")],
            Some(json!({ "type": 1, "title": "The Matrix" })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// A key issued here, read only, as one is made for Plex.
    async fn issued(state: &AppState) -> String {
        let key = crate::auth::secrets::generate_api_key().expect("a key");
        repo::client::create(
            &state.db,
            repo::client::NewClient {
                name: "Plex",
                key_prefix: &key.prefix,
                key_hash: &key.hash,
                scopes: &["read".to_string()],
                expires_at: None,
                note: None,
                owner_id: None,
            },
        )
        .await
        .expect("the key is stored");
        key.plaintext
    }

    #[test]
    fn the_key_an_address_carries_is_the_first_segment_under_plex() {
        assert_eq!(key_in_path("/plex/ams_abc"), Some("ams_abc"));
        assert_eq!(key_in_path("/plex/ams_abc/"), Some("ams_abc"));
        assert_eq!(
            key_in_path("/plex/ams_abc/library/metadata/movie-tmdb-603"),
            Some("ams_abc")
        );
        // The provider's own paths, and nothing outside it.
        for path in [
            "/plex",
            "/plex/",
            "/plex/library/metadata/matches",
            "/plexus/ams_abc",
            "/api/v1/items",
        ] {
            assert_eq!(key_in_path(path), None, "{path}");
        }
    }

    #[tokio::test]
    async fn asked_for_a_key_the_provider_reads_the_one_its_address_carries() {
        let state = server_asking(config::SurfacePolicy::ApiKey).await;
        the_matrix(&state).await;
        let key = issued(&state).await;

        // The manifest and the provider's paths, under the address with the
        // key — from anywhere: the key is what lets Plex in, not its address.
        let (status, manifest) = ask_from(
            &state,
            [203, 0, 113, 9],
            "GET",
            &format!("/plex/{key}"),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{manifest}");
        assert_eq!(
            manifest["MediaProvider"]["identifier"],
            plex::provider().media_provider.identifier
        );
        let (status, answer) = ask_from(
            &state,
            [203, 0, 113, 9],
            "POST",
            &format!("/plex/{key}/library/metadata/matches"),
            &[("content-type", "application/json")],
            Some(json!({ "type": 1, "guid": "tmdb://603" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(
            answer["MediaContainer"]["Metadata"][0]["ratingKey"],
            "movie-tmdb-603"
        );
        let (status, film) = ask_from(
            &state,
            [203, 0, 113, 9],
            "GET",
            &format!("/plex/{key}/library/metadata/movie-tmdb-603"),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{film}");
        assert_eq!(film["MediaContainer"]["Metadata"][0]["title"], "The Matrix");

        // Without a key, or with one never issued, the provider answers
        // nothing — not even the machine itself.
        let (status, _) = ask(&state, "/plex", &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = ask(
            &state,
            "/plex/ams_neverissued/library/metadata/movie-tmdb-603",
            &[],
        )
        .await;
        assert!(
            status.is_client_error() && status != StatusCode::NOT_FOUND,
            "{status}"
        );

        // A key in a header still does, for whoever can send one.
        let (status, _) = ask(&state, "/plex", &[("x-api-key", &key)]).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_address_with_a_key_is_refused_where_the_provider_asks_for_none() {
        // By address: the list of allowed addresses keeps callers out, and a
        // key in the address would only look as though it did.
        let state = server().await;
        let (status, refusal) =
            ask(&state, "/plex/ams_whatever/library/metadata/matches", &[]).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(refusal["error"], "plex_key_unused");
        let (status, _) = ask(&state, "/plex", &[]).await;
        assert_eq!(status, StatusCode::OK);

        // Open: everything is answered, that address with the rest.
        let state = server_asking(config::SurfacePolicy::Open).await;
        let (status, _) = ask_from(
            &state,
            [203, 0, 113, 9],
            "GET",
            "/plex/ams_whatever",
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// Plex keeps the rating key it was handed: a work merged into another
    /// answers under it — by its TMDB id, its TheTVDB id or its own — with
    /// the survivor; a match by the merged work's id finds the survivor, under
    /// the survivor's own key.
    #[tokio::test]
    async fn a_work_merged_into_another_answers_under_the_key_plex_kept() {
        let state = server().await;
        let matrix = the_matrix(&state).await;
        let held = kenshin(&state).await;
        let mut copy = MediaItem::empty(MediaKind::Movie);
        copy.title = "Matrix, The".into();
        copy.slug = "matrix-the".into();
        copy.year = Some(1999);
        copy.external_ids = ExternalIds {
            tmdb: Some(604_000),
            ..Default::default()
        };
        let mut twin = MediaItem::empty(MediaKind::Series);
        twin.title = "Kenshin".into();
        twin.slug = "kenshin".into();
        twin.external_ids = ExternalIds {
            tvdb: Some(999_002),
            ..Default::default()
        };
        for work in [&copy, &twin] {
            store(&state, work).await;
        }
        for (merged, into) in [(&copy.id, &matrix.id), (&twin.id, &held.id)] {
            crate::service::merge::merge(
                &state,
                &crate::service::merge::Asker { by: None, ip: None },
                merged,
                into,
                Default::default(),
            )
            .await
            .expect("asked")
            .expect("merged");
        }

        for key in [
            "movie-tmdb-604000".to_string(),
            format!("movie-id-{}", copy.id),
        ] {
            let (status, answer) = ask(&state, &format!("/plex/library/metadata/{key}"), &[]).await;
            assert_eq!(status, StatusCode::OK, "{key}: {answer}");
            let item = &answer["MediaContainer"]["Metadata"][0];
            assert_eq!(item["ratingKey"], key.as_str());
            assert_eq!(item["title"], "The Matrix");
        }
        let (status, answer) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-999002/children",
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(
            answer["MediaContainer"]["totalSize"], 2,
            "the survivor's seasons"
        );

        let found = matched(&state, json!({ "type": 1, "guid": "tmdb://604000" })).await;
        assert_eq!(found["size"], 1, "{found}");
        assert_eq!(found["Metadata"][0]["ratingKey"], "movie-tmdb-603");
    }

    #[tokio::test]
    async fn a_film_is_matched_by_its_id_then_by_its_title_and_year() {
        let state = server().await;
        the_matrix(&state).await;

        // An id Plex has for it, in either of the ways Plex writes one.
        for guid in [
            "imdb://tt0133093",
            "com.plexapp.agents.themoviedb://603?lang=en",
        ] {
            let found = matched(&state, json!({ "type": 1, "guid": guid })).await;
            assert_eq!(found["size"], 1, "{guid}");
            assert_eq!(found["Metadata"][0]["ratingKey"], "movie-tmdb-603");
            assert_eq!(
                found["Metadata"][0]["guid"],
                "tv.plex.agents.custom.arrmetadataserver://movie/movie-tmdb-603"
            );
        }

        // Its title, folded of case, and a year either side of its own.
        let found = matched(
            &state,
            json!({ "type": 1, "title": "the matrix", "year": 2000 }),
        )
        .await;
        assert_eq!(found["size"], 1, "{found}");
        assert_eq!(found["Metadata"][0]["ratingKey"], "movie-tmdb-603");
        assert_eq!(found["Metadata"][0]["year"], 1999);
        // The poster kept here, at the address this server is reached at.
        assert_eq!(
            found["Metadata"][0]["thumb"],
            "http://ams.lan:8479/media/matrix.jpg"
        );

        // Its French title is a title it goes by, and it is answered in it.
        let (status, answer) = ask_from(
            &state,
            [127, 0, 0, 1],
            "POST",
            "/plex/library/metadata/matches?X-Plex-Language=fr-FR",
            &[("content-type", "application/json")],
            Some(json!({ "type": 1, "title": "Matrix", "year": 1999 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["MediaContainer"]["Metadata"][0]["title"], "Matrix");

        // Twenty years off is another film, which nothing here knows.
        let found = matched(
            &state,
            json!({ "type": 1, "title": "The Matrix", "year": 2021 }),
        )
        .await;
        assert_eq!(found["size"], 0);

        // Nothing to match by at all.
        let (status, _) = ask_from(
            &state,
            [127, 0, 0, 1],
            "POST",
            "/plex/library/metadata/matches",
            &[("content-type", "application/json")],
            Some(json!({ "type": 1 })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_match_is_asked_in_json_said_so_and_nothing_else() {
        let state = server().await;
        the_matrix(&state).await;
        let asked = json!({ "type": 1, "guid": "tmdb://603" });

        // What a web page may send without asking first — no type, text, a
        // form — is refused before it is read.
        for headers in [
            vec![],
            vec![("content-type", "text/plain")],
            vec![("content-type", "application/x-www-form-urlencoded")],
            vec![("content-type", "multipart/form-data; boundary=x")],
            vec![("content-type", "application/jsonp")],
        ] {
            let (status, refusal) = ask_from(
                &state,
                [127, 0, 0, 1],
                "POST",
                "/plex/library/metadata/matches",
                &headers,
                Some(asked.clone()),
            )
            .await;
            assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{headers:?}");
            assert_eq!(refusal["error"], "unsupported_media_type");
        }

        // JSON, whatever its case and parameters.
        let (status, answer) = ask_from(
            &state,
            [127, 0, 0, 1],
            "POST",
            "/plex/library/metadata/matches",
            &[("content-type", "Application/JSON; charset=utf-8")],
            Some(asked),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["MediaContainer"]["size"], 1);
    }

    #[test]
    fn the_ids_tried_are_each_tried_once_and_a_few_at_most() {
        let many: Vec<String> = (1..=5_000).map(|n| format!("tmdb://{n}")).collect();
        let tried = hints(
            [
                "imdb://tt0133093",
                "tmdb://603",
                "tmdb://603",
                "plex://movie/5d77",
            ]
            .into_iter()
            .chain(many.iter().map(String::as_str)),
        );
        assert_eq!(tried.len(), MOST_HINTS);
        assert_eq!(tried[0], Hint::Imdb("tt0133093".into()));
        // Once, however often it is given; what names nothing here is no id.
        assert_eq!(tried[1], Hint::Tmdb(603));
        assert_eq!(tried[2], Hint::Tmdb(1));
        assert_eq!(
            tried
                .iter()
                .filter(|hint| **hint == Hint::Tmdb(603))
                .count(),
            1
        );
    }

    /// TMDB as a stand-in answers it: a film for adults, 999, and nothing
    /// else.
    fn tmdb_with_a_film_for_adults(
        asked: &crate::api::relay::testing::Asked,
    ) -> (StatusCode, Value) {
        if asked.path == "/3/movie/999" {
            let mut film: Value =
                serde_json::from_str(include_str!("../providers/tmdb/fixtures/movie.json"))
                    .expect("the fixture");
            film["id"] = json!(999);
            film["adult"] = json!(true);
            film["imdb_id"] = json!("tt0999999");
            film["title"] = json!("Not for everyone");
            return (StatusCode::OK, film);
        }
        (StatusCode::NOT_FOUND, json!({ "status_code": 34 }))
    }

    #[tokio::test]
    async fn a_work_for_adults_is_neither_fetched_nor_kept_for_a_caller_who_may_not_see_it() {
        use crate::api::relay::testing::{self, StandIn};

        let stand_in = StandIn::start(tmdb_with_a_film_for_adults).await;
        let state = testing::server(&stand_in).await;
        let held = |tmdb: &'static str| {
            let state = state.clone();
            async move {
                repo::item::find_id_by_external(&state.db, ExternalSource::TmdbMovie, tmdb)
                    .await
                    .unwrap()
            }
        };

        // Adult titles hidden: an id Plex gives for a work not held is not
        // followed — TMDB is asked nothing, and nothing is kept.
        let found = matched(&state, json!({ "type": 1, "guid": "tmdb://999" })).await;
        assert_eq!(found["size"], 0, "{found}");
        assert!(stand_in.asked_at("/3/movie/999").is_empty());
        assert!(held("999").await.is_none());

        // Held already, and for adults: passed over on the flag it is held
        // with, never refreshed for this caller.
        let mut kept = MediaItem::empty(MediaKind::Movie);
        kept.title = "Held for adults".into();
        kept.slug = "held-for-adults".into();
        kept.is_adult = true;
        kept.external_ids.tmdb = Some(998);
        store(&state, &kept).await;
        let found = matched(&state, json!({ "type": 1, "guid": "tmdb://998" })).await;
        assert_eq!(found["size"], 0, "{found}");
        assert!(stand_in.asked_at("/3/movie/998").is_empty());

        // A caller who may see them follows the id, which is fetched and kept
        // as every surface fetches one.
        state
            .settings
            .set(
                crate::settings::Scope::Server,
                "",
                "adult.mode",
                "visible",
                None,
            )
            .await
            .expect("set");
        let found = matched(
            &state,
            json!({ "type": 1, "guid": "tmdb://999", "includeAdult": 1 }),
        )
        .await;
        assert_eq!(found["size"], 1, "{found}");
        assert_eq!(stand_in.asked_at("/3/movie/999").len(), 1);
        assert!(held("999").await.is_some());
    }

    #[tokio::test]
    async fn a_film_is_described_in_the_language_asked() {
        let state = server().await;
        the_matrix(&state).await;

        let (status, answer) = ask(
            &state,
            "/plex/library/metadata/movie-tmdb-603",
            &[("X-Plex-Language", "fr-FR"), ("X-Plex-Country", "US")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let film = &answer["MediaContainer"]["Metadata"][0];
        assert_eq!(answer["MediaContainer"]["totalSize"], 1);
        assert_eq!(film["type"], "movie");
        assert_eq!(film["key"], "/library/metadata/movie-tmdb-603");
        assert_eq!(film["title"], "Matrix");
        assert_eq!(film["summary"], "Un pirate apprend ce qu'est le monde.");
        assert_eq!(film["originallyAvailableAt"], "1999-03-31");
        assert_eq!(film["Guid"][0]["id"], "imdb://tt0133093");

        // A language it is not named in: the caller's own, which is the one
        // it is stored in.
        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/movie-tmdb-603",
            &[("X-Plex-Language", "de-DE")],
        )
        .await;
        assert_eq!(
            answer["MediaContainer"]["Metadata"][0]["title"],
            "The Matrix"
        );
    }

    #[tokio::test]
    async fn a_series_is_matched_and_described_with_its_seasons_and_episodes() {
        let state = server().await;
        kenshin(&state).await;

        let found = matched(
            &state,
            json!({ "type": 2, "title": "Rurouni Kenshin", "year": 2023, "includeChildren": 1 }),
        )
        .await;
        assert_eq!(found["size"], 1);
        let show = &found["Metadata"][0];
        assert_eq!(show["ratingKey"], "show-tvdb-416802");
        assert_eq!(show["Children"]["size"], 2);

        let (status, answer) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-416802?includeChildren=1",
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let show = &answer["MediaContainer"]["Metadata"][0];
        assert_eq!(show["type"], "show");
        assert_eq!(show["key"], "/library/metadata/show-tvdb-416802/children");
        assert_eq!(show["SeasonType"][1]["id"], "dvd");
        assert_eq!(show["Children"]["Metadata"][0]["title"], "東京編");
        assert_eq!(show["Children"]["Metadata"][1]["title"], "Season 2");

        let (_, answer) = ask(&state, "/plex/library/metadata/season-tvdb-416802-s1", &[]).await;
        let season = &answer["MediaContainer"]["Metadata"][0];
        assert_eq!(season["index"], 1);
        assert_eq!(season["parentRatingKey"], "show-tvdb-416802");
        assert_eq!(season["originallyAvailableAt"], "2023-07-07");

        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/episode-tvdb-416802-s1e2",
            &[],
        )
        .await;
        let episode = &answer["MediaContainer"]["Metadata"][0];
        assert_eq!(episode["title"], "Episode 1002");
        assert_eq!(episode["index"], 2);
        assert_eq!(episode["parentIndex"], 1);
        assert_eq!(episode["grandparentTitle"], "Rurouni Kenshin");
        assert_eq!(episode["Guid"][0]["id"], "tvdb://1002");

        // A season and an episode are matched within their series.
        let found = matched(
            &state,
            json!({ "type": 4, "grandparentTitle": "Rurouni Kenshin", "parentIndex": 2, "index": 1 }),
        )
        .await;
        assert_eq!(
            found["Metadata"][0]["ratingKey"],
            "episode-tvdb-416802-s2e1"
        );
        let found = matched(
            &state,
            json!({ "type": 4, "grandparentGuid": "tvdb://416802", "date": "2023-07-14" }),
        )
        .await;
        assert_eq!(
            found["Metadata"][0]["ratingKey"],
            "episode-tvdb-416802-s1e2"
        );
        let found = matched(
            &state,
            json!({ "type": 3, "parentGuid": "tvdb://416802", "index": 1, "includeChildren": 1 }),
        )
        .await;
        assert_eq!(found["Metadata"][0]["Children"]["size"], 2);

        // In French, the series' own name, and the words for a season with
        // none.
        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-416802?includeChildren=1",
            &[("X-Plex-Language", "fr-FR")],
        )
        .await;
        let show = &answer["MediaContainer"]["Metadata"][0];
        assert_eq!(show["title"], "Kenshin le vagabond");
        assert_eq!(show["Children"]["Metadata"][1]["title"], "Saison 2");
    }

    #[tokio::test]
    async fn children_come_a_page_at_a_time() {
        let state = server().await;
        kenshin(&state).await;

        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-416802/children?X-Plex-Container-Start=2&X-Plex-Container-Size=1",
            &[],
        )
        .await;
        let page = &answer["MediaContainer"];
        assert_eq!(page["offset"], 1);
        assert_eq!(page["totalSize"], 2);
        assert_eq!(page["size"], 1);
        assert_eq!(page["Metadata"][0]["ratingKey"], "season-tvdb-416802-s2");

        // As headers, and from the first when the start is 0.
        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/season-tvdb-416802-s1/children",
            &[
                ("X-Plex-Container-Start", "0"),
                ("X-Plex-Container-Size", "1"),
            ],
        )
        .await;
        let page = &answer["MediaContainer"];
        assert_eq!(page["totalSize"], 2);
        assert_eq!(page["Metadata"][0]["ratingKey"], "episode-tvdb-416802-s1e1");

        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-416802/grandchildren?X-Plex-Container-Start=2&X-Plex-Container-Size=5",
            &[],
        )
        .await;
        let page = &answer["MediaContainer"];
        assert_eq!(page["totalSize"], 3);
        let keys: Vec<&str> = page["Metadata"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["ratingKey"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            ["episode-tvdb-416802-s1e2", "episode-tvdb-416802-s2e1"]
        );
    }

    #[tokio::test]
    async fn an_episode_order_numbers_the_episodes_and_one_the_series_lacks_none() {
        let state = server().await;
        kenshin(&state).await;

        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-416802/children?episodeOrder=dvd",
            &[],
        )
        .await;
        assert_eq!(
            answer["MediaContainer"]["Metadata"][0]["ratingKey"],
            "season-tvdb-416802-s1-dvd"
        );

        let (status, answer) = ask(
            &state,
            "/plex/library/metadata/season-tvdb-416802-s1-dvd?includeChildren=1",
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let episodes: Vec<(&str, &str)> =
            answer["MediaContainer"]["Metadata"][0]["Children"]["Metadata"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e["ratingKey"].as_str().unwrap(),
                        e["title"].as_str().unwrap(),
                    )
                })
                .collect();
        assert_eq!(
            episodes,
            [
                ("episode-tvdb-416802-s1e1-dvd", "Episode 1002"),
                ("episode-tvdb-416802-s1e2-dvd", "Episode 1001")
            ]
        );

        // An order the series has not: no season, as Plex asks.
        let (_, answer) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-416802/children?episodeOrder=absolute",
            &[],
        )
        .await;
        assert_eq!(answer["MediaContainer"]["totalSize"], 0);
        let (status, _) = ask(
            &state,
            "/plex/library/metadata/season-tvdb-416802-s1-absolute",
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// An order made here, a record of its own among its places, is offered
    /// under its key and asked for by it — its hyphens written as
    /// underscores in the keys of its seasons and episodes; frozen, it is
    /// what the series answers when no order is named.
    #[tokio::test]
    async fn an_order_made_here_is_offered_asked_for_and_frozen_as_the_aired_one() {
        let state = server().await;
        let work = kenshin(&state).await;
        let mut own = PlacedEpisode::of_tvdb(0, 1, 3);
        own.tvdb_id = None;
        own.record = Some(crate::domain::OrderRecord {
            title: "A night at the cinema".into(),
            overview: None,
            aired: Some("2023-08-01".into()),
            runtime: Some(24),
            image: None,
            finale_type: None,
        });
        repo::order::create(
            &state.db,
            &work.id,
            &EpisodeOrder {
                key: "ams-my-order".into(),
                kind: OrderKind::Custom,
                source: "local".into(),
                source_ref: None,
                label: "My order".into(),
                is_default: false,
                is_frozen: false,
                announced: None,
                updated_at: crate::db::now(),
                episodes: vec![
                    PlacedEpisode::of_tvdb(1002, 1, 1),
                    PlacedEpisode::of_tvdb(1001, 1, 2),
                    own,
                ],
            },
        )
        .await
        .expect("made");

        let (_, show) = ask(&state, "/plex/library/metadata/show-tvdb-416802", &[]).await;
        let offered: Vec<(&str, &str)> = show["MediaContainer"]["Metadata"][0]["SeasonType"]
            .as_array()
            .expect("the orders offered")
            .iter()
            .map(|t| (t["id"].as_str().unwrap(), t["title"].as_str().unwrap()))
            .collect();
        assert!(
            offered.contains(&("ams-my-order", "My order")),
            "{offered:?}"
        );
        assert!(offered.contains(&("dvd", "DVD Order")), "{offered:?}");

        let (_, seasons) = ask(
            &state,
            "/plex/library/metadata/show-tvdb-416802/children?episodeOrder=ams-my-order",
            &[],
        )
        .await;
        assert_eq!(
            seasons["MediaContainer"]["Metadata"][0]["ratingKey"],
            "season-tvdb-416802-s1-ams_my_order"
        );
        let (status, season) = ask(
            &state,
            "/plex/library/metadata/season-tvdb-416802-s1-ams_my_order?includeChildren=1",
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let titles: Vec<&str> = season["MediaContainer"]["Metadata"][0]["Children"]["Metadata"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Episode 1002", "Episode 1001", "A night at the cinema"]
        );

        // Frozen: the series' own seasons are the order's.
        assert!(
            repo::order::freeze(&state.db, &work.id, Some("ams-my-order"))
                .await
                .expect("frozen")
        );
        state.caches.touched(&work.id).await;
        let (_, season) = ask(
            &state,
            "/plex/library/metadata/season-tvdb-416802-s1?includeChildren=1",
            &[],
        )
        .await;
        let titles: Vec<&str> = season["MediaContainer"]["Metadata"][0]["Children"]["Metadata"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Episode 1002", "Episode 1001", "A night at the cinema"]
        );
    }

    #[tokio::test]
    async fn what_names_nothing_here_is_not_found() {
        let state = server().await;
        kenshin(&state).await;
        for path in [
            "/plex/library/metadata/show-id-0192f8a4-1234-7abc-8def-0123456789ab",
            "/plex/library/metadata/season-tvdb-416802-s9",
            "/plex/library/metadata/episode-tvdb-416802-s1e9",
            "/plex/library/metadata/5d7768244de0ee001fcc7fed",
        ] {
            let (status, _) = ask(&state, path, &[]).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
    }
}
