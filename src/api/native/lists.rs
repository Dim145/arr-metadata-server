//! Curated lists: the catalogue's own selections, composed by whoever
//! maintains it, and what Sonarr and Radarr import from them.
//!
//! A list is read as its page is — open under public browsing, otherwise with
//! a credential — and served in more shapes: Sonarr's custom list (`title`,
//! `tvdbId`), Radarr's custom list (TMDB's result shape, `id` being the TMDB
//! id), the StevenLu shape Radarr also reads (`title`, `imdb_id`), a text
//! file of identifiers for Kometa's "Text File" builder, and an Atom feed
//! (see [`super::feeds`]). Point an import list at one and the selection
//! becomes what that client adds on its own.

use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{
        self,
        audit::Action,
        list::{CuratedList, ListFields, ListFilter, ListKind, ListMode},
    },
    domain::{MediaItem, MediaKind, ids::normalize_imdb_id, make_slug},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

pub const TAG: &str = "Lists";

/// The most works a list resolves to, whichever way it is composed.
const MAX_ITEMS: i64 = 500;

/// The longest a name, a description, and a field of a filter may be.
const NAME_LIMIT: usize = 120;
const DESCRIPTION_LIMIT: usize = 2000;
const FIELD_LIMIT: usize = 200;
/// The longest address a cover may have: more than any provider's picture
/// has, and no more than a row should be asked to hold for one.
const COVER_LIMIT: usize = 2048;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(detail, update, delete))
        .routes(routes!(set_items))
        .routes(routes!(sonarr_json))
        .routes(routes!(radarr_json))
        .routes(routes!(stevenlu_json))
        .routes(routes!(ids_txt))
        .routes(routes!(holding))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CuratedLists {
    pub lists: Vec<CuratedList>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CuratedListPage {
    pub list: CuratedList,
    /// The works, drawn as a list is: artwork and scores, no episodes. For a
    /// reader, those the reader may see; for whoever maintains the list,
    /// every member, so one that is switched off or of the other kind can
    /// be taken out.
    pub items: Vec<MediaItem>,
    pub total: i64,
}

/// What a list is made of. Every field is optional: on creation the absent
/// ones take their defaults, on a change they are left as they are — a
/// change that says nothing about visibility does not publish a draft.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CuratedListRequest {
    /// Required on creation.
    pub name: Option<String>,
    pub description: Option<String>,
    pub kind: Option<ListKind>,
    pub mode: Option<ListMode>,
    pub filter: Option<ListFilter>,
    pub is_public: Option<bool>,
    /// The picture the selection is shown with, by its address — a
    /// provider's, which the catalogue holds a copy of or not, or any
    /// `https://` address: the site's pages load no picture over `http://`.
    /// Empty, it is taken away; left out, it stays.
    pub cover: Option<String>,
    /// The members of a hand-made list, in order; left as they are when
    /// absent.
    pub items: Option<Vec<String>>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CuratedListItems {
    /// The members, in order.
    pub items: Vec<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// Titles in this language, where a translation is held.
    pub language: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct IdsQuery {
    /// `movie` or `series`: only the works of that kind. A Kometa library
    /// holds one kind, so a list of both is asked for once for each; left
    /// out, every work of the list is written.
    pub kind: Option<String>,
}

// ─── reading ─────────────────────────────────────────────────────────────────

/// Every list — the public ones, for a reader who does not maintain them.
#[utoipa::path(
    get, path = "/lists", tag = TAG,
    responses((status = 200, body = CuratedLists)),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<CuratedLists>> {
    let mut lists = repo::list::list(&state.db, !identity.can_write()).await?;
    counted_for(&state, &identity, &mut lists).await?;
    shown(&state, &mut lists);
    Ok(Json(CuratedLists { lists }))
}

/// One list with its works, by slug or by id.
#[utoipa::path(
    get, path = "/lists/{key}", tag = TAG,
    params(("key" = String, Path, description = "The list's slug, or its id"), PageQuery),
    responses(
        (status = 200, body = CuratedListPage),
        (status = 404, description = "No such list, or none this caller may see"),
    ),
)]
async fn detail(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(key): Path<String>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<CuratedListPage>> {
    let language = crate::api::extract::language(query.language.as_deref())?;
    let mut list = visible(&state, &identity, &key).await?;
    counted_for(&state, &identity, std::slice::from_mut(&mut list)).await?;
    let audience = if identity.can_write() {
        Audience::Maintainer
    } else {
        Audience::Reader
    };
    let (items, total) = resolve(&state, &identity, &list, language.as_deref(), audience).await?;
    shown(&state, std::slice::from_mut(&mut list));
    Ok(Json(CuratedListPage { list, items, total }))
}

/// The hand-made lists a work is in — the public ones, for a reader who does
/// not maintain them, and none at all for a work the reader may not see.
#[utoipa::path(
    get, path = "/items/{id}/lists", tag = TAG,
    params(("id" = String, Path, description = "The work's id")),
    responses(
        (status = 200, body = CuratedLists),
        (status = 404, description = "No such work, or none this caller may see"),
    ),
)]
async fn holding(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<CuratedLists>> {
    // As the work's own page decides it.
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let hidden = !item.is_enabled
        || (item.is_adult
            && !state.adult_for(identity.client_id(), identity.peer_id(), Some(true)));
    if hidden && !identity.can_write() {
        return Err(AppError::NotFound);
    }
    let mut lists = repo::list::holding(&state.db, &item.id, !identity.can_write()).await?;
    counted_for(&state, &identity, &mut lists).await?;
    shown(&state, &mut lists);
    Ok(Json(CuratedLists { lists }))
}

// ─── the shapes the clients import ───────────────────────────────────────────

/// The list as Sonarr's "Custom List" import list reads it: the series, by
/// TheTVDB id.
#[utoipa::path(
    get, path = "/lists/{key}/sonarr.json", tag = TAG,
    params(("key" = String, Path, description = "The list's slug, or its id")),
    responses((status = 200, description = "`[{ \"title\", \"tvdbId\" }]`")),
)]
async fn sonarr_json(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(key): Path<String>,
) -> AppResult<Json<Vec<Value>>> {
    let list = visible(&state, &identity, &key).await?;
    let (items, _) = resolve(&state, &identity, &list, None, Audience::Reader).await?;
    Ok(Json(sonarr_list(&items)))
}

/// The list as Radarr's "Custom Lists" import list reads it: the films, in
/// TMDB's result shape, `id` being the TMDB id.
#[utoipa::path(
    get, path = "/lists/{key}/radarr.json", tag = TAG,
    params(("key" = String, Path, description = "The list's slug, or its id")),
    responses((status = 200, description = "`[{ \"id\", \"title\" }]`")),
)]
async fn radarr_json(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(key): Path<String>,
) -> AppResult<Json<Vec<Value>>> {
    let list = visible(&state, &identity, &key).await?;
    let (items, _) = resolve(&state, &identity, &list, None, Audience::Reader).await?;
    Ok(Json(radarr_list(&items)))
}

/// The list in the shape of the StevenLu lists Radarr reads: the films, by
/// IMDb id.
#[utoipa::path(
    get, path = "/lists/{key}/stevenlu.json", tag = TAG,
    params(("key" = String, Path, description = "The list's slug, or its id")),
    responses((status = 200, description = "`[{ \"title\", \"imdb_id\" }]`")),
)]
async fn stevenlu_json(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(key): Path<String>,
) -> AppResult<Json<Vec<Value>>> {
    let list = visible(&state, &identity, &key).await?;
    let (items, _) = resolve(&state, &identity, &list, None, Audience::Reader).await?;
    Ok(Json(stevenlu_list(&items)))
}

/// The list as a text file of identifiers, as Kometa's "Text File" builder
/// reads one: a work a line, `tmdb:` for a film's TMDB id, `tvdb:` for a
/// series' TheTVDB id, a bare `tt…` for an IMDb id, and after a `#` the
/// title, for whoever reads the file. In the list's order, which the builder
/// keeps for a collection ordered as the source is.
#[utoipa::path(
    get, path = "/lists/{key}/ids.txt", tag = TAG,
    params(("key" = String, Path, description = "The list's slug, or its id"), IdsQuery),
    responses(
        (status = 200, description = "A work a line: `tmdb:949  # Heat (1995)`", content_type = "text/plain"),
        (status = 400, description = "`kind` is neither movie nor series"),
        (status = 404, description = "No such list, or none this caller may see"),
    ),
)]
async fn ids_txt(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(key): Path<String>,
    Query(query): Query<IdsQuery>,
) -> AppResult<Response> {
    let kind = match query.kind.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("movie") => Some(MediaKind::Movie),
        Some("series") => Some(MediaKind::Series),
        Some(_) => {
            return Err(AppError::BadRequest(
                "kind is movie or series: a Kometa library holds one of them".into(),
            ));
        }
    };
    let list = visible(&state, &identity, &key).await?;
    let (items, _) = resolve(&state, &identity, &list, None, Audience::Reader).await?;

    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )],
        kometa_ids(&list, &items, kind),
    )
        .into_response())
}

fn sonarr_list(items: &[MediaItem]) -> Vec<Value> {
    items
        .iter()
        .filter(|w| w.kind == MediaKind::Series)
        .filter_map(|w| Some(json!({ "title": w.title, "tvdbId": w.external_ids.tvdb? })))
        .collect()
}

fn radarr_list(items: &[MediaItem]) -> Vec<Value> {
    items
        .iter()
        .filter(|w| w.kind == MediaKind::Movie)
        .filter_map(|w| Some(json!({ "id": w.external_ids.tmdb?, "title": w.title })))
        .collect()
}

fn stevenlu_list(items: &[MediaItem]) -> Vec<Value> {
    items
        .iter()
        .filter(|w| w.kind == MediaKind::Movie)
        .filter_map(|w| {
            Some(json!({ "title": w.title, "imdb_id": w.external_ids.imdb.as_deref()? }))
        })
        .collect()
}

/// The identifier Kometa is given for a work, written so that it cannot be
/// taken for another kind's: Kometa reads a bare number as the TMDB id of a
/// film in a movie library and the TheTVDB id of a series in a show library —
/// the two kinds' numbers are not each other's — so each line says which id it
/// is, and a list of both kinds is asked for once for each library.
///
/// A film is named by TMDB, then IMDb. A series is named by TheTVDB, which a
/// show library reads it by, then by IMDb, which no film shares, and last by
/// TMDB. Nothing, for a work none of these name.
fn kometa_id(work: &MediaItem) -> Option<String> {
    let ids = &work.external_ids;
    let imdb = || ids.imdb.as_deref().and_then(normalize_imdb_id);
    let tmdb = || ids.tmdb.map(|id| format!("tmdb:{id}"));

    match work.kind {
        MediaKind::Movie => tmdb().or_else(imdb),
        MediaKind::Series => ids
            .tvdb
            .map(|id| format!("tvdb:{id}"))
            .or_else(imdb)
            .or_else(tmdb),
    }
}

/// A text as one line of a comment: its whitespace and control characters
/// run into single spaces, so that nothing in a title can start a new line.
fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text file of a list for Kometa's "Text File" builder: a header in
/// comments, which the builder skips, then a work a line, titled after a `#`.
/// `kind` keeps the works of one kind, as a library of that kind reads them.
fn kometa_ids(list: &CuratedList, items: &[MediaItem], kind: Option<MediaKind>) -> String {
    let lines: Vec<(String, &MediaItem)> = items
        .iter()
        .filter(|work| kind.is_none_or(|kind| work.kind == kind))
        .filter_map(|work| Some((kometa_id(work)?, work)))
        .collect();

    let mut out = format!("# {}\n", one_line(&list.name));
    out.push_str(
        "# For Kometa's Text File builder: tmdb: is a film's TMDB id, tvdb: a series' TheTVDB id, tt… an IMDb id.\n",
    );
    if kind.is_none() && list.kind == ListKind::Mixed {
        out.push_str(
            "# This selection holds films and series. A library holds one kind: ask for ?kind=movie or ?kind=series.\n",
        );
    }
    for (id, work) in lines {
        let title = match work.year {
            Some(year) => format!("{} ({year})", work.title),
            None => work.title.clone(),
        };
        out.push_str(&format!("{id}  # {}\n", one_line(&title)));
    }
    out
}

// ─── composing ───────────────────────────────────────────────────────────────

/// Create a list. Nothing is written until everything in the request has
/// been checked, the members included.
#[utoipa::path(
    post, path = "/lists", tag = TAG,
    request_body = CuratedListRequest,
    responses(
        (status = 201, body = CuratedList),
        (status = 400, description = "A name is missing or too long, the filter is not one, the cover is not a picture's address, or a member is not in the catalogue"),
        (status = 403, description = "The caller may not write"),
        (status = 409, description = "Another list took that name at the same moment"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(mut request): Json<CuratedListRequest>,
) -> AppResult<(StatusCode, Json<CuratedList>)> {
    require_write(&identity)?;
    unkept(&state, &mut request);
    let checked = check(&request, None)?;
    let members = match &request.items {
        Some(ids) if checked.mode == ListMode::Manual => Some(existing(&state, ids).await?),
        _ => None,
    };
    let slug = free_slug(&state, &checked.name).await?;

    // The list and its members together, or neither.
    let mut list = repo::list::create(&state.db, checked.fields(&slug), members.as_deref())
        .await
        .map_err(|e| conflict_or_internal(e, &checked.name))?;

    tracing::info!(list = %list.name, actor = %identity.label(), "created a list");
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ListCreated,
            target: Some(&list.id),
            detail: Some(&list.name),
        },
    )
    .await;

    shown(&state, std::slice::from_mut(&mut list));
    Ok((StatusCode::CREATED, Json(list)))
}

/// Change a list, by id or by slug. A field left out stays as it is; the
/// slug stays whatever the name becomes, since the clients hold the address.
#[utoipa::path(
    put, path = "/lists/{key}", tag = TAG,
    params(("key" = String, Path, description = "The list's id, or its slug")),
    request_body = CuratedListRequest,
    responses(
        (status = 200, body = CuratedList),
        (status = 400, description = "A name is too long, the filter is not one, the cover is not a picture's address, or a member is not in the catalogue"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such list"),
    ),
)]
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(key): Path<String>,
    Json(mut request): Json<CuratedListRequest>,
) -> AppResult<Json<CuratedList>> {
    require_write(&identity)?;
    unkept(&state, &mut request);
    let current = find(&state, &key).await?.ok_or(AppError::NotFound)?;
    let checked = check(&request, Some(&current))?;
    // A list composed by a filter has no members of its own: any it had are
    // let go, so a work's page does not name a list that may not hold it.
    let members = match (checked.mode, &request.items) {
        (ListMode::Filter, _) => Some(Vec::new()),
        (ListMode::Manual, Some(ids)) => Some(existing(&state, ids).await?),
        (ListMode::Manual, None) => None,
    };

    // What the list is made of and its members together, or neither.
    let written = repo::list::update(
        &state.db,
        &current.id,
        checked.fields(&current.slug),
        members.as_deref(),
    )
    .await
    .map_err(|e| conflict_or_internal(e, &checked.name))?;
    if !written {
        return Err(AppError::NotFound);
    }
    let mut list = repo::list::get(&state.db, &current.id)
        .await?
        .ok_or(AppError::NotFound)?;

    tracing::info!(list = %list.name, actor = %identity.label(), "changed a list");
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ListUpdated,
            target: Some(&list.id),
            detail: Some(&list.name),
        },
    )
    .await;

    shown(&state, std::slice::from_mut(&mut list));
    Ok(Json(list))
}

/// Replace the members of a hand-made list, in order.
#[utoipa::path(
    put, path = "/lists/{key}/items", tag = TAG,
    params(("key" = String, Path, description = "The list's id, or its slug")),
    request_body = CuratedListItems,
    responses(
        (status = 200, body = CuratedList),
        (status = 400, description = "A work named is not in the catalogue, there are too many, or the list is composed by a filter"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such list"),
    ),
)]
async fn set_items(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(key): Path<String>,
    Json(request): Json<CuratedListItems>,
) -> AppResult<Json<CuratedList>> {
    require_write(&identity)?;
    let current = find(&state, &key).await?.ok_or(AppError::NotFound)?;
    if current.mode != ListMode::Manual {
        return Err(AppError::BadRequest(
            "a list composed by a filter has no members of its own".into(),
        ));
    }
    let members = existing(&state, &request.items).await?;
    repo::list::set_members(&state.db, &current.id, &members).await?;
    let mut list = repo::list::get(&state.db, &current.id)
        .await?
        .ok_or(AppError::NotFound)?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ListUpdated,
            target: Some(&list.id),
            detail: Some(&list.name),
        },
    )
    .await;

    shown(&state, std::slice::from_mut(&mut list));
    Ok(Json(list))
}

/// Delete a list. The works stay.
#[utoipa::path(
    delete, path = "/lists/{key}", tag = TAG,
    params(("key" = String, Path, description = "The list's id, or its slug")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such list"),
    ),
)]
async fn delete(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(key): Path<String>,
) -> AppResult<StatusCode> {
    require_write(&identity)?;
    let current = find(&state, &key).await?.ok_or(AppError::NotFound)?;
    if !repo::list::delete(&state.db, &current.id).await? {
        return Err(AppError::NotFound);
    }

    tracing::info!(list = %current.name, actor = %identity.label(), "deleted a list");
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ListDeleted,
            target: Some(&current.id),
            detail: Some(&current.name),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── the pieces ──────────────────────────────────────────────────────────────

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}

/// A list by id, or failing that by slug.
pub(super) async fn find(state: &AppState, key: &str) -> AppResult<Option<CuratedList>> {
    if let Some(list) = repo::list::get(&state.db, key).await? {
        return Ok(Some(list));
    }
    Ok(repo::list::by_slug(&state.db, key).await?)
}

/// The covers of some lists as their readers are given them: the copy this
/// server keeps of the picture where there is one — a path the site follows
/// whatever name it was reached by — else the address it was given by. An
/// upload that is no longer kept has none a reader could follow, and so no
/// cover.
fn shown(state: &AppState, lists: &mut [CuratedList]) {
    for list in lists {
        if let Some(cover) = list.cover.take() {
            let served = state.media.localized(&cover);
            list.cover = (!served.starts_with("upload:")).then_some(served);
        }
    }
}

/// A cover that came back as it was given — a path to a copy kept, which is
/// what the site shows a cover by — put back to the address the copy was
/// fetched from. The list keeps the address, not the path: the copy may be
/// forgotten, or the picture kept somewhere else, and the address is still
/// the picture's.
fn unkept(state: &AppState, request: &mut CuratedListRequest) {
    if let Some(cover) = request.cover.as_mut() {
        *cover = state.media.unlocalize(cover.trim());
    }
}

/// The hand-made lists' counts as this reader is shown them: of the members
/// switched on and — for a reader the adult policy keeps them from — not for
/// adults. Whoever maintains the lists is told every member switched on.
async fn counted_for(
    state: &AppState,
    identity: &Identity,
    lists: &mut [CuratedList],
) -> AppResult<()> {
    if !identity.can_write() && !state.adult_for(identity.client_id(), identity.peer_id(), None) {
        repo::list::count_without_adult(&state.db, lists).await?;
    }
    Ok(())
}

/// A list this caller may read: any, for one who maintains them — who may
/// write, or reads with the key of an editor or an administrator, which an
/// import list or a feed reader is given in its address and which need not
/// be able to write; a public one otherwise, a private one being no list at
/// all to a reader.
pub(super) async fn visible(
    state: &AppState,
    identity: &Identity,
    key: &str,
) -> AppResult<CuratedList> {
    let list = find(state, key).await?.ok_or(AppError::NotFound)?;
    if !list.is_public && !identity.can_read_private() {
        return Err(AppError::NotFound);
    }
    Ok(list)
}

/// The unique index on the slug is the only constraint a caller can trip:
/// two lists named alike, composed at the same moment.
fn conflict_or_internal(e: anyhow::Error, name: &str) -> AppError {
    let text = e.to_string();
    if text.contains("UNIQUE") || text.contains("duplicate key") {
        AppError::Conflict(format!(
            "a list named like {name:?} was created at the same moment; try again"
        ))
    } else {
        AppError::Internal(e)
    }
}

/// A request, checked: the name trimmed and bounded, the filter one the store
/// understands, and what was left out taken from the list as it is.
struct Checked {
    name: String,
    description: Option<String>,
    kind: ListKind,
    mode: ListMode,
    filter: Option<ListFilter>,
    is_public: bool,
    cover: Option<String>,
}

impl Checked {
    fn fields<'a>(&'a self, slug: &'a str) -> ListFields<'a> {
        ListFields {
            slug,
            name: &self.name,
            description: self.description.as_deref(),
            kind: self.kind,
            mode: self.mode,
            filter: self.filter.as_ref(),
            is_public: self.is_public,
            cover: self.cover.as_deref(),
        }
    }
}

/// A cover as a list may keep it: the address of a picture a browser loads,
/// `https://`, or an upload's `upload:` origin, which the copy kept is
/// served for. Nothing a browser would run — `javascript:`, a `data:` page —
/// and no address that is not one: no space, no control character.
///
/// Not `http://`: the site's policy lets a page load a picture from its own
/// origin and over https, and from nowhere else (`img-src`, in `web.rs`), so a
/// cover at an http address would be kept, offered and shown as a gap on every
/// page that carries it.
fn check_cover(text: &str) -> AppResult<Option<String>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.starts_with("http://") {
        return Err(AppError::BadRequest(
            "a cover's address is an https:// one: the site's pages load no picture over http://"
                .into(),
        ));
    }
    if !(text.starts_with("https://") || text.starts_with("upload:")) {
        return Err(AppError::BadRequest(
            "a cover is a picture's address: https://".into(),
        ));
    }
    if text.chars().count() > COVER_LIMIT
        || text.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AppError::BadRequest(format!(
            "a cover's address is at most {COVER_LIMIT} characters, with no space in it"
        )));
    }
    Ok(Some(text.to_string()))
}

fn check(request: &CuratedListRequest, current: Option<&CuratedList>) -> AppResult<Checked> {
    let name = match (&request.name, current) {
        (Some(name), _) => name.trim().to_string(),
        (None, Some(current)) => current.name.clone(),
        (None, None) => return Err(AppError::BadRequest("a list needs a name".into())),
    };
    if name.is_empty() {
        return Err(AppError::BadRequest("a list needs a name".into()));
    }
    if name.chars().count() > NAME_LIMIT {
        return Err(AppError::BadRequest(format!(
            "a name is at most {NAME_LIMIT} characters"
        )));
    }
    let description = match &request.description {
        Some(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        None => current.and_then(|c| c.description.clone()),
    };
    if description
        .as_deref()
        .is_some_and(|d| d.chars().count() > DESCRIPTION_LIMIT)
    {
        return Err(AppError::BadRequest(format!(
            "a description is at most {DESCRIPTION_LIMIT} characters"
        )));
    }
    let kind = request.kind.or(current.map(|c| c.kind)).unwrap_or_default();
    let mode = request.mode.or(current.map(|c| c.mode)).unwrap_or_default();
    let is_public = request
        .is_public
        .or(current.map(|c| c.is_public))
        .unwrap_or(true);
    // Said, it is checked — and an empty one takes it away; left out, it stays.
    let cover = match &request.cover {
        Some(text) => check_cover(text)?,
        None => current.and_then(|c| c.cover.clone()),
    };
    let filter = match mode {
        ListMode::Filter => {
            let filter = request
                .filter
                .clone()
                .or_else(|| current.and_then(|c| c.filter.clone()))
                .ok_or_else(|| {
                    AppError::BadRequest("a list composed by a filter needs one".into())
                })?;
            // Tried on for size now, so a list that cannot be read is not
            // kept: the same checks its page would make.
            filter_query(&filter, false, kind)?;
            Some(filter)
        }
        ListMode::Manual => None,
    };
    if let Some(items) = &request.items {
        if items.len() as i64 > MAX_ITEMS {
            return Err(AppError::BadRequest(format!(
                "a list holds at most {MAX_ITEMS} works"
            )));
        }
        if mode == ListMode::Filter && !items.is_empty() {
            return Err(AppError::BadRequest(
                "a list composed by a filter has no members of its own".into(),
            ));
        }
    }
    Ok(Checked {
        name,
        description,
        kind,
        mode,
        filter,
        is_public,
        cover,
    })
}

/// The slug a name gives, or the first of `-2`, `-3`… that no other list has.
/// A name that reads as an id is set apart from the ids, which a list is
/// also found by.
async fn free_slug(state: &AppState, name: &str) -> AppResult<String> {
    let mut base = make_slug(name, None);
    if uuid::Uuid::parse_str(&base).is_ok() {
        base = format!("list-{base}");
    }
    for attempt in 0..50u32 {
        let candidate = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{}", attempt + 1)
        };
        if repo::list::by_slug(&state.db, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(AppError::Conflict(format!(
        "too many lists are already named like {name:?}"
    )))
}

/// The ids named, every one of which is a work in the catalogue, in the
/// order given.
async fn existing(state: &AppState, ids: &[String]) -> AppResult<Vec<String>> {
    if ids.len() as i64 > MAX_ITEMS {
        return Err(AppError::BadRequest(format!(
            "a list holds at most {MAX_ITEMS} works"
        )));
    }
    let ids: Vec<String> = ids.iter().map(|id| id.trim().to_string()).collect();
    let found: HashSet<String> = repo::item::by_ids(&state.db, &ids)
        .await?
        .into_iter()
        .map(|w| w.id)
        .collect();
    if let Some(missing) = ids.iter().find(|id| !found.contains(*id)) {
        return Err(AppError::BadRequest(format!(
            "{missing:?} is not a work in the catalogue"
        )));
    }
    Ok(ids)
}

/// A filter as the store takes it, checked, under this reader's adult policy
/// and the list's kind.
fn filter_query(filter: &ListFilter, adult: bool, kind: ListKind) -> AppResult<repo::item::Query> {
    let sort = filter
        .sort
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::parse::<repo::item::Sort>)
        .transpose()
        .map_err(AppError::BadRequest)?
        .unwrap_or_default();
    if filter
        .min_rating
        .is_some_and(|m| !(0.0..=10.0).contains(&m))
    {
        return Err(AppError::BadRequest(
            "minRating is a score out of ten".into(),
        ));
    }
    if filter.limit.is_some_and(|l| !(1..=MAX_ITEMS).contains(&l)) {
        return Err(AppError::BadRequest(format!(
            "limit is between 1 and {MAX_ITEMS}"
        )));
    }
    if filter.genres.len() > 20 {
        return Err(AppError::BadRequest("at most twenty genres".into()));
    }
    // Short, every one of them: a pattern the size of a book is not a
    // filter, and the engine would refuse it in the reader's face.
    let short = |text: &Option<String>, what: &str| -> AppResult<Option<String>> {
        let text = text.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if text.is_some_and(|t| t.chars().count() > FIELD_LIMIT) {
            return Err(AppError::BadRequest(format!(
                "{what} is at most {FIELD_LIMIT} characters"
            )));
        }
        Ok(text.map(str::to_string))
    };
    let genres: Vec<String> = filter
        .genres
        .iter()
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect();
    if genres.iter().any(|g| g.chars().count() > FIELD_LIMIT) {
        return Err(AppError::BadRequest(format!(
            "a genre is at most {FIELD_LIMIT} characters"
        )));
    }
    Ok(repo::item::Query {
        // A list's genres are a narrowing: all of them, as ever.
        genre_any: false,
        term: short(&filter.term, "term")?,
        kind: match kind {
            ListKind::Series => Some(MediaKind::Series),
            ListKind::Movie => Some(MediaKind::Movie),
            ListKind::Mixed => None,
        },
        year: None,
        manual_only: false,
        include_disabled: false,
        include_adult: adult,
        genres,
        keyword: short(&filter.keyword, "keyword")?,
        year_from: filter.year_from,
        year_to: filter.year_to,
        status: short(&filter.status, "status")?,
        original_language: short(&filter.original_language, "originalLanguage")?,
        network: short(&filter.network, "network")?,
        collection: filter.collection,
        min_rating: filter.min_rating,
        refresh_failed: false,
        missing: Vec::new(),
        stored_language: None,
        sort,
        descending: filter.descending,
        limit: filter.limit.unwrap_or(100),
        offset: 0,
    })
}

/// Who a list is resolved for: a reader, shown what a reader may see, or
/// whoever maintains it, shown every member.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Audience {
    Reader,
    Maintainer,
}

/// The works of a list as the store holds them — no artwork, no overrides —
/// in the list's own order, for a reader who may be shown adult works or
/// not (`adult`), or whoever maintains the list.
async fn members(
    state: &AppState,
    list: &CuratedList,
    adult: bool,
    audience: Audience,
) -> AppResult<Vec<MediaItem>> {
    let mut items = match list.mode {
        ListMode::Manual => {
            let ids = repo::list::member_ids(&state.db, &list.id).await?;
            let mut works = repo::item::by_ids(&state.db, &ids).await?;
            // In the list's own order — and for a reader, only what a reader
            // may see, the rule the catalogue's lists follow.
            let rank: HashMap<&str, usize> = ids
                .iter()
                .enumerate()
                .map(|(index, id)| (id.as_str(), index))
                .collect();
            if audience == Audience::Reader {
                works.retain(|w| w.is_enabled && (adult || !w.is_adult));
            }
            works.sort_by_key(|w| rank.get(w.id.as_str()).copied().unwrap_or(usize::MAX));
            works
        }
        ListMode::Filter => {
            let filter = list.filter.clone().unwrap_or_default();
            repo::item::search(&state.db, &filter_query(&filter, adult, list.kind)?).await?
        }
    };
    // The kind a list is for narrows a hand-made one too, for a reader; the
    // maintainer is shown a member of the other kind, to take it out.
    if audience == Audience::Reader {
        match list.kind {
            ListKind::Series => items.retain(|w| w.kind == MediaKind::Series),
            ListKind::Movie => items.retain(|w| w.kind == MediaKind::Movie),
            ListKind::Mixed => {}
        }
    }
    Ok(items)
}

/// The works of a list an export of the whole catalogue would hold: those
/// switched on and not for adults, in the list's order. What the `.nfo`
/// documents of one selection are written for.
pub(super) async fn exportable(state: &AppState, list: &CuratedList) -> AppResult<Vec<MediaItem>> {
    members(state, list, false, Audience::Reader).await
}

/// The works of a list, drawn as a list is.
pub(super) async fn resolve(
    state: &AppState,
    identity: &Identity,
    list: &CuratedList,
    language: Option<&str>,
    audience: Audience,
) -> AppResult<(Vec<MediaItem>, i64)> {
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
    let mut items = members(state, list, adult, audience).await?;

    service::apply_overrides(state, &mut items).await?;
    repo::item::load_artwork(&state.db, &mut items).await?;
    service::overlay_imdb_many(state, &mut items).await;
    if let Some(language) = language.map(str::trim).filter(|l| !l.is_empty()) {
        for item in &mut items {
            service::language::apply_shallow(state, item, language);
        }
    }
    for item in &mut items {
        state.media.localize(item);
        service::as_card(item);
    }
    service::redact_for_reader(identity, &mut items);

    let total = items.len() as i64;
    Ok((items, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(kind: MediaKind, title: &str) -> MediaItem {
        let mut w = MediaItem::empty(kind);
        w.id = title.to_lowercase();
        w.title = title.to_string();
        w
    }

    fn request(
        name: Option<&str>,
        mode: Option<ListMode>,
        filter: Option<ListFilter>,
    ) -> CuratedListRequest {
        CuratedListRequest {
            name: name.map(str::to_string),
            description: None,
            kind: None,
            mode,
            filter,
            is_public: None,
            cover: None,
            items: None,
        }
    }

    fn a_list() -> CuratedList {
        CuratedList {
            id: "01a0d073-6ce8-760c-a2cb-113d4cf78503".into(),
            slug: "picks".into(),
            name: "Picks".into(),
            description: Some("A few.".into()),
            kind: ListKind::Series,
            mode: ListMode::Filter,
            filter: Some(ListFilter {
                sort: Some("rating".into()),
                ..Default::default()
            }),
            is_public: false,
            cover: None,
            item_count: 0,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn each_client_is_given_its_own_shape_and_only_what_it_can_use() {
        let mut dark = work(MediaKind::Series, "Dark");
        dark.external_ids.tvdb = Some(348_303);
        let unknown = work(MediaKind::Series, "Unknown");
        let mut heat = work(MediaKind::Movie, "Heat");
        heat.external_ids.tmdb = Some(949);
        heat.external_ids.imdb = Some("tt0113277".into());
        let mut orphan = work(MediaKind::Movie, "Orphan");
        orphan.external_ids.imdb = Some("tt0000001".into());
        let items = [dark, unknown, heat, orphan];

        assert_eq!(
            sonarr_list(&items),
            vec![json!({ "title": "Dark", "tvdbId": 348_303 })]
        );
        assert_eq!(
            radarr_list(&items),
            vec![json!({ "id": 949, "title": "Heat" })]
        );
        assert_eq!(
            stevenlu_list(&items),
            vec![
                json!({ "title": "Heat", "imdb_id": "tt0113277" }),
                json!({ "title": "Orphan", "imdb_id": "tt0000001" }),
            ]
        );
    }

    #[test]
    fn a_request_is_checked_before_anything_is_kept() {
        assert!(check(&request(Some("  "), None, None), None).is_err());
        assert!(check(&request(None, None, None), None).is_err());
        assert!(check(&request(Some(&"x".repeat(121)), None, None), None).is_err());
        assert!(check(&request(Some("Autumn"), Some(ListMode::Filter), None), None).is_err());
        let bad_sort = ListFilter {
            sort: Some("sideways".into()),
            ..Default::default()
        };
        assert!(
            check(
                &request(Some("Autumn"), Some(ListMode::Filter), Some(bad_sort)),
                None
            )
            .is_err()
        );
        let too_many = ListFilter {
            limit: Some(501),
            ..Default::default()
        };
        assert!(
            check(
                &request(Some("Autumn"), Some(ListMode::Filter), Some(too_many)),
                None
            )
            .is_err()
        );
        let too_long = ListFilter {
            term: Some("x".repeat(201)),
            ..Default::default()
        };
        assert!(
            check(
                &request(Some("Autumn"), Some(ListMode::Filter), Some(too_long)),
                None
            )
            .is_err()
        );

        let fine = ListFilter {
            genres: vec!["Animation".into()],
            year_from: Some(2026),
            sort: Some("release".into()),
            ..Default::default()
        };
        let autumn = request(Some(" Autumn 2026 "), Some(ListMode::Filter), Some(fine));
        let checked = check(&autumn, None).unwrap();
        assert_eq!(checked.name, "Autumn 2026");
        assert!(checked.filter.is_some());
        assert!(checked.is_public && checked.kind == ListKind::Mixed);

        // A hand-made list keeps no filter, whatever was sent.
        let picks = request(
            Some("Picks"),
            Some(ListMode::Manual),
            Some(ListFilter::default()),
        );
        assert!(check(&picks, None).unwrap().filter.is_none());

        // A filter list may not be given members of its own.
        let mut with_members = request(
            Some("Autumn"),
            Some(ListMode::Filter),
            Some(ListFilter::default()),
        );
        with_members.items = Some(vec!["w1".into()]);
        assert!(check(&with_members, None).is_err());
    }

    #[test]
    fn a_change_that_says_nothing_leaves_things_as_they_are() {
        let current = a_list();
        // Nothing said: the private filter list stays a private filter list.
        let checked = check(&request(None, None, None), Some(&current)).unwrap();
        assert_eq!(checked.name, "Picks");
        assert_eq!(checked.description.as_deref(), Some("A few."));
        assert_eq!(
            (checked.kind, checked.mode),
            (ListKind::Series, ListMode::Filter)
        );
        assert!(!checked.is_public);
        assert_eq!(checked.filter, current.filter);
        // A new name alone.
        let checked = check(&request(Some("Better picks"), None, None), Some(&current)).unwrap();
        assert_eq!(checked.name, "Better picks");
        assert!(!checked.is_public);
        // An empty description takes it away; a mode change to hand-made
        // drops the filter.
        let mut cleared = request(None, Some(ListMode::Manual), None);
        cleared.description = Some("  ".into());
        let checked = check(&cleared, Some(&current)).unwrap();
        assert!(checked.description.is_none() && checked.filter.is_none());
    }

    #[test]
    fn a_filter_becomes_the_stores_query_under_the_lists_kind() {
        let filter = ListFilter {
            genres: vec![" Animation ".into(), String::new()],
            min_rating: Some(7.5),
            limit: Some(40),
            ..Default::default()
        };
        let query = filter_query(&filter, false, ListKind::Series).unwrap();
        assert_eq!(query.kind, Some(MediaKind::Series));
        assert_eq!(query.genres, vec!["Animation".to_string()]);
        assert_eq!(query.limit, 40);
        assert!(!query.include_adult && !query.include_disabled);
        let query = filter_query(&ListFilter::default(), true, ListKind::Mixed).unwrap();
        assert_eq!(query.kind, None);
        assert_eq!(query.limit, 100);
        assert!(query.include_adult);
    }

    #[test]
    fn a_filter_written_out_leaves_out_what_it_does_not_say() {
        let filter = ListFilter {
            sort: Some("rating".into()),
            limit: Some(24),
            ..Default::default()
        };
        let json = serde_json::to_value(&filter).unwrap();
        assert_eq!(json, json!({ "genres": [], "sort": "rating", "limit": 24 }));
        assert!(json.get("descending").is_none());
    }

    #[test]
    fn kometa_is_given_each_work_by_the_id_its_kind_is_read_by() {
        let mut heat = work(MediaKind::Movie, "Heat");
        heat.year = Some(1995);
        heat.external_ids.tmdb = Some(949);
        heat.external_ids.tvdb = Some(1);
        heat.external_ids.imdb = Some("tt0113277".into());
        // A film without a TMDB id is named by IMDb, never by a TheTVDB id.
        let mut ronin = work(MediaKind::Movie, "Ronin");
        ronin.external_ids.tvdb = Some(55);
        ronin.external_ids.imdb = Some("0207992".into());
        let mut dark = work(MediaKind::Series, "Dark");
        dark.external_ids.tvdb = Some(348_303);
        dark.external_ids.tmdb = Some(70_523);
        // A series: TheTVDB, then IMDb, which no film shares, then TMDB.
        let mut only_imdb = work(MediaKind::Series, "Imdb only");
        only_imdb.external_ids.imdb = Some("tt0000002".into());
        only_imdb.external_ids.tmdb = Some(5);
        let mut only_tmdb = work(MediaKind::Series, "Tmdb only");
        only_tmdb.external_ids.tmdb = Some(6);
        let nameless = work(MediaKind::Series, "Nameless");

        assert_eq!(kometa_id(&heat).as_deref(), Some("tmdb:949"));
        assert_eq!(kometa_id(&ronin).as_deref(), Some("tt0207992"));
        assert_eq!(kometa_id(&dark).as_deref(), Some("tvdb:348303"));
        assert_eq!(kometa_id(&only_imdb).as_deref(), Some("tt0000002"));
        assert_eq!(kometa_id(&only_tmdb).as_deref(), Some("tmdb:6"));
        assert_eq!(kometa_id(&nameless), None);

        let mut list = a_list();
        list.name = "Autumn\n2026".into();
        list.kind = ListKind::Mixed;
        let items = [heat, dark, nameless, ronin];

        // Both kinds, in the list's order, the nameless one left out; and a
        // line that says a library holds one kind.
        let all = kometa_ids(&list, &items, None);
        assert!(all.starts_with("# Autumn 2026\n# For Kometa's Text File builder"));
        assert!(all.contains("ask for ?kind=movie or ?kind=series"));
        assert_eq!(
            entries(&all),
            [
                "tmdb:949  # Heat (1995)",
                "tvdb:348303  # Dark",
                "tt0207992  # Ronin"
            ]
        );

        // One kind: what a library of that kind reads, and no advice.
        let films = kometa_ids(&list, &items, Some(MediaKind::Movie));
        assert_eq!(
            entries(&films),
            ["tmdb:949  # Heat (1995)", "tt0207992  # Ronin"]
        );
        assert!(!films.contains("?kind="));
        let shows = kometa_ids(&list, &items, Some(MediaKind::Series));
        assert_eq!(entries(&shows), ["tvdb:348303  # Dark"]);
        // A list of one kind needs no advice either.
        list.kind = ListKind::Series;
        assert!(!kometa_ids(&list, &items, None).contains("?kind="));
    }

    /// The lines of a text file that name a work: not the comments above them.
    fn entries(text: &str) -> Vec<&str> {
        text.lines().filter(|l| !l.starts_with('#')).collect()
    }

    #[test]
    fn a_title_cannot_start_a_line_of_the_text_file() {
        let mut odd = work(MediaKind::Movie, "Two\nlines # and a hash\r\u{7}");
        odd.external_ids.tmdb = Some(1);
        let text = kometa_ids(&a_list(), &[odd], None);
        let line = text.lines().last().unwrap();
        assert_eq!(line, "tmdb:1  # Two lines # and a hash");
        // Every line is a comment or an identifier and its comment.
        for line in text.lines() {
            assert!(line.starts_with('#') || line.starts_with("tmdb:"), "{line}");
        }
    }

    #[test]
    fn a_cover_is_the_address_of_a_picture_a_browser_loads() {
        let mut said = request(Some("Autumn"), None, None);
        let check_with = |cover: &str, current: Option<&CuratedList>| {
            let mut said = request(Some("Autumn"), Some(ListMode::Manual), None);
            said.cover = Some(cover.to_string());
            check(&said, current)
        };

        // Left out, it is not touched; said empty, it is taken away.
        let mut current = a_list();
        current.cover = Some("https://image.tmdb.org/t/p/original/a.jpg".into());
        let kept = check(&said, Some(&current)).unwrap();
        assert_eq!(kept.cover.as_deref(), current.cover.as_deref());
        said.cover = Some("  ".into());
        assert_eq!(check(&said, Some(&current)).unwrap().cover, None);
        assert_eq!(check(&said, None).unwrap().cover, None);

        for fine in [
            " https://image.tmdb.org/t/p/w1280/b.jpg ",
            "https://films.example:8443/b.png",
            "upload:0f3a",
        ] {
            let checked = check_with(fine, None).unwrap();
            assert_eq!(checked.cover.as_deref(), Some(fine.trim()));
        }
        for bad in [
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "//films.example/b.png",
            "/media/0f3a.jpg",
            "https://films.example/a b.png",
            "https://films.example/\n.png",
            &format!("https://films.example/{}", "x".repeat(COVER_LIMIT)),
        ] {
            assert!(check_with(bad, None).is_err(), "{bad:?}");
        }
    }

    /// The site's policy loads a picture over https and from its own origin,
    /// and a browser blocks the rest: a cover at an http address is refused
    /// where it is set, saying why — not kept, to be shown as a gap.
    #[test]
    fn a_cover_over_http_is_refused_for_the_policy_would_block_it() {
        let mut said = request(Some("Autumn"), Some(ListMode::Manual), None);
        for http in [
            "http://films.example/b.png",
            " http://image.tmdb.org/t/p/original/a.jpg ",
        ] {
            said.cover = Some(http.to_string());
            let Err(AppError::BadRequest(why)) = check(&said, None) else {
                panic!("{http:?} was kept");
            };
            assert!(
                why.contains("https://") && why.contains("http://"),
                "it says which address is wanted and which is not: {why}"
            );
        }

        // A list that holds one already keeps it until a cover is said: only
        // what is set is checked.
        let mut current = a_list();
        current.cover = Some("http://films.example/old.png".into());
        said.cover = None;
        assert_eq!(
            check(&said, Some(&current)).unwrap().cover.as_deref(),
            Some("http://films.example/old.png")
        );
        // And can be taken away, or replaced by one that loads.
        said.cover = Some(String::new());
        assert_eq!(check(&said, Some(&current)).unwrap().cover, None);
        said.cover = Some("https://films.example/new.png".into());
        assert_eq!(
            check(&said, Some(&current)).unwrap().cover.as_deref(),
            Some("https://films.example/new.png")
        );
    }

    /// The body of an answer, read as text.
    async fn read(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read");
        String::from_utf8(bytes.to_vec()).expect("text")
    }

    /// A work, stored.
    async fn stored(state: &AppState, mut work: MediaItem) -> MediaItem {
        work.slug = make_slug(&work.title, None);
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

    #[tokio::test]
    async fn the_text_file_is_served_as_a_list_is_read() {
        let state = super::super::testing::server().await;
        let mut heat = work(MediaKind::Movie, "Heat");
        heat.external_ids.tmdb = Some(949);
        let heat = stored(&state, heat).await;
        let mut dark = work(MediaKind::Series, "Dark");
        dark.external_ids.tvdb = Some(348_303);
        let dark = stored(&state, dark).await;

        let list = repo::list::create(
            &state.db,
            ListFields {
                slug: "picks",
                name: "Picks",
                description: None,
                kind: ListKind::Mixed,
                mode: ListMode::Manual,
                filter: None,
                is_public: true,
                cover: None,
            },
            Some(&[dark.id.clone(), heat.id.clone()]),
        )
        .await
        .expect("created");

        let ask = |kind: Option<&str>| {
            ids_txt(
                State(state.clone()),
                Extension(Identity::Visitor),
                Path(list.slug.clone()),
                Query(IdsQuery {
                    kind: kind.map(str::to_string),
                }),
            )
        };

        let response = ask(None).await.expect("answered");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        let body = read(response).await;
        // In the list's own order, which is not the catalogue's.
        assert_eq!(entries(&body), ["tvdb:348303  # Dark", "tmdb:949  # Heat"]);

        let films = read(ask(Some("movie")).await.expect("answered")).await;
        assert_eq!(entries(&films), ["tmdb:949  # Heat"]);
        assert!(matches!(
            ask(Some("anime")).await,
            Err(AppError::BadRequest(_))
        ));

        // A private list is a reader's no list at all, whatever the shape.
        repo::list::create(
            &state.db,
            ListFields {
                slug: "private",
                name: "Private",
                description: None,
                kind: ListKind::Mixed,
                mode: ListMode::Manual,
                filter: None,
                is_public: false,
                cover: None,
            },
            None,
        )
        .await
        .expect("created");
        let refused = ids_txt(
            State(state.clone()),
            Extension(Identity::Visitor),
            Path("private".into()),
            Query(IdsQuery { kind: None }),
        )
        .await;
        assert!(matches!(refused, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn a_private_list_is_read_by_a_key_that_only_reads_when_its_owner_could_write() {
        use crate::{auth::testing::key, db::repo::user::Role};

        let state = super::super::testing::server().await;
        let mut heat = work(MediaKind::Movie, "Heat");
        heat.external_ids.tmdb = Some(949);
        let heat = stored(&state, heat).await;
        repo::list::create(
            &state.db,
            ListFields {
                slug: "private",
                name: "Private",
                description: None,
                kind: ListKind::Movie,
                mode: ListMode::Manual,
                filter: None,
                is_public: false,
                cover: None,
            },
            Some(std::slice::from_ref(&heat.id)),
        )
        .await
        .expect("created");

        let ask = |who: Identity| {
            ids_txt(
                State(state.clone()),
                Extension(who),
                Path("private".into()),
                Query(IdsQuery { kind: None }),
            )
        };

        // What an import list or Kometa carries in its address: the key of an
        // editor or of an administrator, limited to reading.
        for role in [Role::Editor, Role::Admin] {
            let reads = key(&["read"], Some(role));
            assert!(!reads.can_write(), "{role}");
            let body = read(ask(reads).await.expect("answered")).await;
            assert_eq!(entries(&body), ["tmdb:949  # Heat"], "{role}");
        }

        // A member's key reads what a visitor does, which is no such list;
        // nor does a key of the server's that only reads, with no owner whose
        // role it could read by.
        for refused in [key(&["read"], Some(Role::Member)), key(&["read"], None)] {
            assert!(matches!(ask(refused).await, Err(AppError::NotFound)));
        }

        // And a key that may write reads it, as it always did.
        assert!(ask(key(&["read", "write"], None)).await.is_ok());
    }

    #[tokio::test]
    async fn a_work_keeps_the_day_it_joined_through_every_save() {
        let state = super::super::testing::server().await;
        let first = stored(&state, work(MediaKind::Movie, "First")).await;
        let second = stored(&state, work(MediaKind::Movie, "Second")).await;
        let third = stored(&state, work(MediaKind::Movie, "Third")).await;

        let list = repo::list::create(
            &state.db,
            ListFields {
                slug: "joined",
                name: "Joined",
                description: None,
                kind: ListKind::Movie,
                mode: ListMode::Manual,
                filter: None,
                is_public: true,
                cover: Some("https://image.tmdb.org/t/p/original/c.jpg"),
            },
            Some(&[first.id.clone(), second.id.clone()]),
        )
        .await
        .expect("created");
        assert_eq!(
            list.cover.as_deref(),
            Some("https://image.tmdb.org/t/p/original/c.jpg")
        );
        let before: HashMap<String, String> = repo::list::members_joined(&state.db, &list.id)
            .await
            .unwrap()
            .into_iter()
            .collect();

        // A third work joins, the others are moved: each of the first two
        // is dated as it was, and only the third by the save.
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        repo::list::set_members(
            &state.db,
            &list.id,
            &[third.id.clone(), second.id.clone(), first.id.clone()],
        )
        .await
        .unwrap();
        let after = repo::list::members_joined(&state.db, &list.id)
            .await
            .unwrap();

        assert_eq!(
            after.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            [third.id.as_str(), second.id.as_str(), first.id.as_str()]
        );
        for (id, at) in &after {
            match before.get(id) {
                Some(was) => assert_eq!(at, was, "{id} kept the day it joined"),
                None => assert!(
                    at > before.get(&first.id).unwrap(),
                    "the work that joined is dated by the save"
                ),
            }
        }
    }
}
