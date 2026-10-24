//! Radarr compatibility.
//!
//! Radarr builds these URLs from `https://api.radarr.video/v1/{route}`. As with
//! Sonarr, the host is compiled in, so reaching this server means overriding DNS.

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    domain::MediaItem,
    error::{AppError, AppResult},
    service::{language, movie},
    state::AppState,
    wire::radarr::{
        Beside, CollectionResource, MovieResource, collection_images, from_item, from_item_beside,
    },
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Radarr compatibility";

pub fn router() -> OpenApiRouter<AppState> {
    // Static segments are matched before the `{tmdb_id}` route, so these do not
    // need to be ordered by hand.
    OpenApiRouter::new()
        .routes(routes!(bulk))
        .routes(routes!(changed))
        .routes(routes!(by_imdb))
        .routes(routes!(collection))
        .routes(routes!(by_tmdb))
        .routes(routes!(search))
        .routes(routes!(popular))
        .routes(routes!(trending))
        .routes(routes!(imdb_list))
}

/// Radarr's format, in the language this caller is served in.
///
/// Sonarr puts a language in its URL; Radarr has none to put. So it is whatever
/// the settings say for whoever is calling — which is what `tmdb.language` on an
/// API key or an allowlist rule is for. Shallow: a film has no episode text, so
/// the overlay is the title and the synopsis and costs no upstream call. The
/// works are taken by value because the usual answer is that this caller reads
/// the language they are already stored in, and then nothing is copied at all.
fn served(state: &AppState, identity: &Identity, items: Vec<MediaItem>) -> Vec<MovieResource> {
    served_with(state, identity, items, &[])
}

/// [`served`], each film with what goes beside it: the collection it is part
/// of, which Radarr adds and offers the rest of, and the films recommended
/// with it, which feed its Discover page. These are the routes Radarr
/// refreshes its library through; what goes beside is read from what is held
/// and never fetched — see [`movie::beside`]. A search stays without: its
/// results are shallow, and Radarr asks for the film again once it is added.
async fn served_beside(
    state: &AppState,
    identity: &Identity,
    items: Vec<MediaItem>,
) -> Vec<MovieResource> {
    let language = state.language(identity.client_id(), identity.peer_id());
    let beside = movie::beside(state, &items, &language).await;

    served_with(state, identity, items, &beside)
}

fn served_with(
    state: &AppState,
    identity: &Identity,
    mut items: Vec<MediaItem>,
    beside: &[Beside],
) -> Vec<MovieResource> {
    let language = state.language(identity.client_id(), identity.peer_id());

    for item in &mut items {
        language::apply_shallow(state, item, &language);
        state.media.for_clients(item);
    }

    items
        .iter()
        .enumerate()
        .map(|(at, item)| match beside.get(at) {
            Some(beside) => from_item_beside(item, beside),
            None => from_item(item),
        })
        .collect()
}

/// A movie by TMDB id, in Radarr's format.
#[utoipa::path(
    get, path = "/v1/movie/{tmdb_id}", tag = TAG,
    operation_id = "radarr_by_tmdb",
    params(("tmdb_id" = i64, Path, description = "TMDB movie id")),
    responses(
        (status = 200, body = MovieResource),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "No provider could resolve this id"),
    ),
    security(),
)]
async fn by_tmdb(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(tmdb_id): Path<i64>,
) -> AppResult<Json<MovieResource>> {
    let mut item = movie::by_tmdb_id(&state, tmdb_id)
        .await?
        .ok_or(AppError::NotFound)?;
    // Under the id Radarr asked by, which a film merged into another is
    // answered under: Radarr keeps the film it holds, and reads the
    // survivor's record into it, rather than finding a film it may hold
    // already under the other id.
    item.external_ids.tmdb = Some(tmdb_id);

    let mut answer = served_beside(&state, &identity, vec![item]).await;

    Ok(Json(answer.remove(0)))
}

/// Radarr expects an array here even though it only ever uses the first entry.
/// A movie by IMDb id. Radarr expects an array even though it reads only the first.
#[utoipa::path(
    get, path = "/v1/movie/imdb/{imdb_id}", tag = TAG,
    operation_id = "radarr_by_imdb",
    params(("imdb_id" = String, Path, description = "IMDb id, with or without the `tt` prefix")),
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn by_imdb(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(imdb_id): Path<String>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let found: Vec<MediaItem> = movie::by_imdb_id(&state, &imdb_id)
        .await?
        .into_iter()
        .collect();

    Ok(Json(served_beside(&state, &identity, found).await))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    // Radarr always emits this parameter, empty when the user gave no year.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    year: Option<i32>,
}

/// Search movies, in Radarr's format.
#[utoipa::path(
    get, path = "/v1/search", tag = TAG,
    operation_id = "radarr_search",
    params(
        ("q" = Option<String>, Query, description = "Free text, or `tmdb:`/`imdb:` followed by an id"),
        ("year" = Option<i32>, Query, description = "Narrow to a release year"),
    ),
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn search(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let term = query.q.unwrap_or_default();

    let items = movie::search(&state, &term, query.year).await?;

    Ok(Json(served(&state, &identity, items)))
}

/// Several movies at once.
///
/// A title that cannot be resolved is skipped rather than failing the batch,
/// which is what Radarr expects when one entry has gone away upstream. Capped
/// at 100 ids per request.
#[utoipa::path(
    post, path = "/v1/movie/bulk", tag = TAG,
    operation_id = "radarr_bulk",
    request_body = Vec<i64>,
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn bulk(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(tmdb_ids): Json<Vec<i64>>,
) -> AppResult<Json<Vec<MovieResource>>> {
    // Each under the id it was asked by, as `by_tmdb` answers it.
    let items: Vec<MediaItem> = movie::bulk(&state, &tmdb_ids)
        .await?
        .into_iter()
        .map(|(asked, mut item)| {
            item.external_ids.tmdb = Some(asked);
            item
        })
        .collect();

    Ok(Json(served_beside(&state, &identity, items).await))
}

#[derive(Deserialize)]
struct ChangedQuery {
    since: Option<String>,
}

/// TMDB ids changed since an instant, which Radarr polls to know what to
/// refetch: those TMDB lists, and the films held here that changed since —
/// fetched again, or a lock put on, changed or lifted.
#[utoipa::path(
    get, path = "/v1/movie/changed", tag = TAG,
    operation_id = "radarr_changed",
    params(("since" = String, Query, description = "An instant — `2026-10-09T11:45:00`, UTC unless an offset says otherwise — or a date. TMDB is asked from its date")),
    responses(
        (status = 200, body = Vec<i64>),
        (status = 400, description = "`since` was not supplied, or is no date"),
    ),
    security(),
)]
async fn changed(
    State(state): State<AppState>,
    Query(query): Query<ChangedQuery>,
) -> AppResult<Json<Vec<i64>>> {
    let Some(since) = query.since else {
        return Err(AppError::BadRequest(
            "the `since` parameter is required".into(),
        ));
    };
    let Some(since) = movie::instant(&since) else {
        return Err(AppError::BadRequest(
            "`since` is a date, or an instant such as 2026-10-09T11:45:00".into(),
        ));
    };

    Ok(Json(movie::changed_since(&state, since).await?))
}

/// A collection and the movies in it.
///
/// The parts carry search-level detail only: fetching each in full would be
/// dozens of upstream calls for a list Radarr uses to offer suggestions. Each
/// names the collection, as api.radarr.video's do: Radarr files a film under
/// the collection it names, and lists a collection's films by that — parts
/// sent naming none were filed under none.
#[utoipa::path(
    get, path = "/v1/movie/collection/{tmdb_id}", tag = TAG,
    operation_id = "radarr_collection",
    params(("tmdb_id" = i64, Path, description = "TMDB collection id")),
    responses(
        (status = 200, body = CollectionResource),
        (status = 404, description = "No such collection, or no TMDB key is configured"),
    ),
    security(),
)]
async fn collection(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(tmdb_id): Path<i64>,
) -> AppResult<Json<CollectionResource>> {
    let language = language::tmdb_locale(
        None,
        &state.language(identity.client_id(), identity.peer_id()),
    );
    let found = movie::collection(&state, tmdb_id, &language)
        .await?
        .ok_or(AppError::NotFound)?;

    let named = CollectionResource {
        name: found.name.clone(),
        overview: None,
        tmdb_id,
        images: Vec::new(),
        parts: Vec::new(),
    };
    let mut parts = served(&state, &identity, found.parts);
    for part in &mut parts {
        part.collection = Some(named.clone());
    }

    Ok(Json(CollectionResource {
        name: found.name,
        overview: found.overview,
        tmdb_id,
        images: collection_images(found.poster.as_deref(), found.backdrop.as_deref()),
        parts,
    }))
}

/// TMDB's popular movies.
#[utoipa::path(
    get, path = "/v1/list/tmdb/popular", tag = TAG,
    operation_id = "radarr_popular",
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn popular(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let items = movie::popular(&state).await?;

    Ok(Json(served(&state, &identity, items)))
}

/// One of IMDb's lists, as Radarr's "IMDb Lists" import asks for it: `top250`,
/// `popular`, or a user's ratings by `ur…` id.
///
/// The one route of Radarr's that this server cannot answer from what it
/// holds — the lists are IMDb's, compiled by Radarr's own metadata service —
/// so it is answered from that service and kept a day. Radarr reads only the
/// TMDB ids out of it and then asks for each movie here, where the operator's
/// edits apply. With the service switched off there is no list to give.
#[utoipa::path(
    get, path = "/v1/list/imdb/{id}", tag = TAG,
    operation_id = "radarr_imdb_list",
    params(("id" = String, Path, description = "`top250`, `popular`, or an IMDb user id such as `ur12345678`")),
    responses(
        (status = 200, body = Vec<MovieResource>),
        (status = 400, description = "Not one of IMDb's lists"),
        (status = 403, description = "The caller's address is not in the allowlist"),
        (status = 404, description = "Radarr's metadata service has no such list"),
        (status = 502, description = "Radarr's metadata service could not be asked"),
        (status = 503, description = "Radarr's metadata service is switched off here"),
    ),
    security(),
)]
async fn imdb_list(State(state): State<AppState>, Path(id): Path<String>) -> AppResult<Response> {
    // Only what Radarr can ask for reaches the service: this route must not
    // become a way to fetch arbitrary paths of it through this server.
    if !is_imdb_list(&id) {
        return Err(AppError::BadRequest(
            "an IMDb list is top250, popular, or a user id such as ur12345678".into(),
        ));
    }
    let provider = &state.config.radarr_metadata;
    if !(provider.fallback || provider.enrich) {
        return Err(AppError::ProviderNotConfigured);
    }

    let key = format!("list:imdb:{id}");
    // One download for every Radarr asking at once, run in a task of its own:
    // a caller that gives up — Radarr's patience, or this server's own request
    // timeout — does not take the download with it, and the next to ask finds
    // the list in the cache rather than starting it over.
    let body = state
        .caches
        .lists
        .try_get_with(key.clone(), {
            let state = state.clone();
            async move {
                tokio::spawn(async move { fetch_imdb_list(&state, &key, &id).await })
                    .await
                    .map_err(|e| ListFailure::Upstream(e.into()))?
            }
        })
        .await
        .map_err(|failure| match &*failure {
            ListFailure::Missing => AppError::NotFound,
            ListFailure::Upstream(e) => AppError::UpstreamUnavailable(anyhow::anyhow!("{e:#}")),
        })?;

    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

/// Why a list was not had — kept apart so a missing list stays a 404 through
/// the cache's sharing of one download between callers.
enum ListFailure {
    Missing,
    Upstream(anyhow::Error),
}

/// The list from the service, checked and cached.
async fn fetch_imdb_list(state: &AppState, key: &str, id: &str) -> Result<Bytes, ListFailure> {
    let bytes = state
        .radarr_metadata
        .imdb_list(id)
        .await
        .map_err(ListFailure::Upstream)?
        .ok_or(ListFailure::Missing)?;
    // Checked here to be the shape Radarr parses, once, rather than found out
    // by every Radarr that asks.
    serde_json::from_slice::<Vec<MovieResource>>(&bytes).map_err(|e| {
        ListFailure::Upstream(anyhow::anyhow!(
            "Radarr's metadata service returned a list this server could not interpret: {e}"
        ))
    })?;
    state
        .caches
        .lists
        .insert(key.to_string(), bytes.clone())
        .await;
    Ok(bytes)
}

/// Whether `id` names a list Radarr's metadata service compiles from IMDb.
fn is_imdb_list(id: &str) -> bool {
    match id {
        "top250" | "popular" => true,
        _ => {
            id.len() >= 6
                && id.len() <= 14
                && id.starts_with("ur")
                && id[2..].bytes().all(|b| b.is_ascii_digit())
        }
    }
}

/// TMDB's trending movies for the week.
#[utoipa::path(
    get, path = "/v1/list/tmdb/trending", tag = TAG,
    operation_id = "radarr_trending",
    responses((status = 200, body = Vec<MovieResource>)),
    security(),
)]
async fn trending(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<MovieResource>>> {
    let items = movie::trending(&state).await?;

    Ok(Json(served(&state, &identity, items)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::repo::{self, item::ItemWrite},
        domain::{ExternalIds, MediaKind},
    };

    /// A film merged into another is answered under the TMDB id Radarr asked
    /// by — alone and in bulk — with the survivor's record: Radarr keeps the
    /// film it holds, and matches each answer of a bulk refresh to it.
    #[tokio::test]
    async fn a_film_merged_into_another_is_answered_under_the_id_radarr_asked_by() {
        let state = crate::api::native::testing::server().await;
        let film = |title: &str, tmdb: i64| {
            let mut film = MediaItem::empty(MediaKind::Movie);
            film.title = title.into();
            film.slug = crate::domain::make_slug(title, Some(1995));
            film.year = Some(1995);
            film.external_ids = ExternalIds {
                tmdb: Some(tmdb),
                ..Default::default()
            };
            film
        };
        let (heat, copy) = (film("Heat", 949), film("Heat, a copy", 950));
        for work in [&heat, &copy] {
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
        crate::service::merge::merge(
            &state,
            &crate::service::merge::Asker { by: None, ip: None },
            &copy.id,
            &heat.id,
            Default::default(),
        )
        .await
        .expect("asked")
        .expect("merged");

        let Json(one) = by_tmdb(
            State(state.clone()),
            Extension(Identity::Network(None)),
            Path(950),
        )
        .await
        .expect("answered");
        assert_eq!((one.tmdb_id, one.title.as_str()), (950, "Heat"));

        let Json(many) = bulk(
            State(state.clone()),
            Extension(Identity::Network(None)),
            Json(vec![949, 950]),
        )
        .await
        .expect("answered");
        let said: Vec<(i64, &str)> = many
            .iter()
            .map(|film| (film.tmdb_id, film.title.as_str()))
            .collect();
        assert_eq!(said, [(949, "Heat"), (950, "Heat")]);
    }

    #[test]
    fn only_imdbs_lists_are_asked_of_the_service() {
        for ok in [
            "top250",
            "popular",
            "ur1234",
            "ur12345678",
            "ur123456789012",
        ] {
            assert!(is_imdb_list(ok), "{ok}");
        }
        for bad in [
            "",
            "top",
            "ls012345678",
            "ur",
            "ur12",
            "ur12345678901234",
            "urabc",
            "../movie/1",
            "top250/x",
            "UR12345678",
        ] {
            assert!(!is_imdb_list(bad), "{bad}");
        }
    }
}
