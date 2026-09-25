//! Curated lists: the catalogue's own selections, composed by whoever
//! maintains it, and what Sonarr and Radarr import from them.
//!
//! A list is read as its page is — open under public browsing, otherwise with
//! a credential — and served in three more shapes: Sonarr's custom list
//! (`title`, `tvdbId`), Radarr's custom list (TMDB's result shape, `id` being
//! the TMDB id) and the StevenLu shape Radarr also reads (`title`,
//! `imdb_id`). Point an import list at one and the selection becomes what
//! that client adds on its own.

use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
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
    domain::{MediaItem, MediaKind, make_slug},
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

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(detail, update, delete))
        .routes(routes!(set_items))
        .routes(routes!(sonarr_json))
        .routes(routes!(radarr_json))
        .routes(routes!(stevenlu_json))
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
    let lists = repo::list::list(&state.db, !identity.can_write()).await?;
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
    let list = visible(&state, &identity, &key).await?;
    let audience = if identity.can_write() {
        Audience::Maintainer
    } else {
        Audience::Reader
    };
    let (items, total) = resolve(
        &state,
        &identity,
        &list,
        query.language.as_deref(),
        audience,
    )
    .await?;
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
    let lists = repo::list::holding(&state.db, &item.id, !identity.can_write()).await?;
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

// ─── composing ───────────────────────────────────────────────────────────────

/// Create a list. Nothing is written until everything in the request has
/// been checked, the members included.
#[utoipa::path(
    post, path = "/lists", tag = TAG,
    request_body = CuratedListRequest,
    responses(
        (status = 201, body = CuratedList),
        (status = 400, description = "A name is missing or too long, the filter is not one, or a member is not in the catalogue"),
        (status = 403, description = "The caller may not write"),
        (status = 409, description = "Another list took that name at the same moment"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<CuratedListRequest>,
) -> AppResult<(StatusCode, Json<CuratedList>)> {
    require_write(&identity)?;
    let checked = check(&request, None)?;
    let members = match &request.items {
        Some(ids) if checked.mode == ListMode::Manual => Some(existing(&state, ids).await?),
        _ => None,
    };
    let slug = free_slug(&state, &checked.name).await?;

    let list = repo::list::create(&state.db, checked.fields(&slug))
        .await
        .map_err(|e| conflict_or_internal(e, &checked.name))?;
    if let Some(members) = members {
        repo::list::set_members(&state.db, &list.id, &members).await?;
    }
    let list = repo::list::get(&state.db, &list.id)
        .await?
        .ok_or(AppError::NotFound)?;

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
        (status = 400, description = "A name is too long, the filter is not one, or a member is not in the catalogue"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such list"),
    ),
)]
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(key): Path<String>,
    Json(request): Json<CuratedListRequest>,
) -> AppResult<Json<CuratedList>> {
    require_write(&identity)?;
    let current = find(&state, &key).await?.ok_or(AppError::NotFound)?;
    let checked = check(&request, Some(&current))?;
    // A list composed by a filter has no members of its own: any it had are
    // let go, so a work's page does not name a list that may not hold it.
    let members = match (checked.mode, &request.items) {
        (ListMode::Filter, _) => Some(Vec::new()),
        (ListMode::Manual, Some(ids)) => Some(existing(&state, ids).await?),
        (ListMode::Manual, None) => None,
    };

    let written = repo::list::update(&state.db, &current.id, checked.fields(&current.slug))
        .await
        .map_err(|e| conflict_or_internal(e, &checked.name))?;
    if !written {
        return Err(AppError::NotFound);
    }
    if let Some(members) = members {
        repo::list::set_members(&state.db, &current.id, &members).await?;
    }
    let list = repo::list::get(&state.db, &current.id)
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
    let list = repo::list::get(&state.db, &current.id)
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
async fn find(state: &AppState, key: &str) -> AppResult<Option<CuratedList>> {
    if let Some(list) = repo::list::get(&state.db, key).await? {
        return Ok(Some(list));
    }
    Ok(repo::list::by_slug(&state.db, key).await?)
}

/// A list this caller may read: any, for one who maintains them; a public
/// one otherwise, a private one being no list at all to a reader.
async fn visible(state: &AppState, identity: &Identity, key: &str) -> AppResult<CuratedList> {
    let list = find(state, key).await?.ok_or(AppError::NotFound)?;
    if !list.is_public && !identity.can_write() {
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
        }
    }
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
        sort,
        descending: filter.descending,
        limit: filter.limit.unwrap_or(100),
        offset: 0,
    })
}

/// Who a list is resolved for: a reader, shown what a reader may see, or
/// whoever maintains it, shown every member.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Audience {
    Reader,
    Maintainer,
}

/// The works of a list, drawn as a list is.
async fn resolve(
    state: &AppState,
    identity: &Identity,
    list: &CuratedList,
    language: Option<&str>,
    audience: Audience,
) -> AppResult<(Vec<MediaItem>, i64)> {
    let adult = state.adult_for(identity.client_id(), identity.peer_id(), None);
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

    service::apply_overrides(state, &mut items).await?;
    repo::item::load_artwork(&state.db, &mut items).await?;
    service::overlay_imdb_many(state, &mut items).await;
    if let Some(language) = language.map(str::trim).filter(|l| !l.is_empty()) {
        for item in &mut items {
            service::language::apply_shallow(state, item, language);
        }
    }
    for item in &mut items {
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
}
