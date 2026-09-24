//! Browsing, creating and refreshing works.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action},
    domain::{ExternalIds, MediaItem, MediaKind, make_slug},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Catalogue";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(detail, update, remove))
        .routes(routes!(refresh))
        .routes(routes!(snapshots))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub term: Option<String>,
    /// Ask for adult titles. Honoured only as far as the server and this
    /// caller's own policy allow, and ignored entirely when the operator has
    /// said the answer is not the client's to give.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub include_adult: Option<bool>,
    pub kind: Option<String>,
    /// Serve the titles in this language, where a translation is held.
    pub language: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub year: Option<i32>,
    #[serde(default)]
    pub manual_only: bool,
    #[serde(default)]
    pub include_disabled: bool,
    /// Genres the works must all carry, comma-separated: `Drama,Crime`.
    pub genre: Option<String>,
    /// A keyword the works must carry.
    pub keyword: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub year_from: Option<i32>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub year_to: Option<i32>,
    /// `continuing`, `ended`, `upcoming`, `released`… as stored.
    pub status: Option<String>,
    /// The language a work was made in, as stored: `ja`, `en`, `fra`…
    pub original_language: Option<String>,
    /// A network or a studio, by name, whatever its case.
    pub network: Option<String>,
    /// The TMDB collection a film belongs to.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub collection: Option<i64>,
    /// The lowest score, out of ten, a work may have to be listed.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub min_rating: Option<f64>,
    /// Only works whose last refresh failed. For those who may edit the
    /// catalogue: it says how the server is doing, not anything about a film,
    /// and anybody else is answered as if it had not been asked.
    #[serde(default)]
    pub refresh_failed: bool,
    /// `popularity` (the default), `rating`, `release`, `title`, `added` or
    /// `refreshed`.
    pub sort: Option<String>,
    /// `asc` or `desc`; each sort has its own default.
    pub order: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub offset: Option<i64>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = ItemListResponse)]
pub struct ListResponse {
    pub items: Vec<MediaItem>,
    pub total: i64,
}

/// The list's filters as the store takes them, checked.
///
/// Shared with the facets, so that what a filter means cannot differ between a
/// list and the counts beside it.
pub(super) fn to_query(
    state: &AppState,
    identity: &Identity,
    query: ListQuery,
) -> AppResult<repo::item::Query> {
    let kind = query
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::parse::<MediaKind>)
        .transpose()
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let sort = query
        .sort
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::parse::<repo::item::Sort>)
        .transpose()
        .map_err(AppError::BadRequest)?
        .unwrap_or_default();

    let descending = match query.order.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("asc") => Some(false),
        Some("desc") => Some(true),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unknown order {other:?}: expected asc or desc"
            )));
        }
    };

    if query.min_rating.is_some_and(|m| !(0.0..=10.0).contains(&m)) {
        return Err(AppError::BadRequest(
            "minRating is a score out of ten".into(),
        ));
    }

    // Which works are switched off, and which failed their last refresh, is for
    // whoever maintains the catalogue — the editors' screens ask for both — and
    // not for a visitor, who is answered as if the flags had not been sent.
    let maintains = identity.can_write();

    Ok(repo::item::Query {
        term: query.term,
        kind,
        year: query.year,
        manual_only: query.manual_only,
        include_disabled: query.include_disabled && maintains,
        include_adult: state.adult_for(
            identity.client_id(),
            identity.peer_id(),
            query.include_adult,
        ),
        genres: query
            .genre
            .as_deref()
            .map(|g| g.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        keyword: query.keyword,
        year_from: query.year_from,
        year_to: query.year_to,
        status: query.status,
        original_language: query.original_language,
        network: query.network,
        collection: query.collection,
        min_rating: query.min_rating,
        refresh_failed: query.refresh_failed && maintains,
        sort,
        descending,
        limit: query.limit.unwrap_or(50),
        offset: query.offset.unwrap_or(0),
    })
}

/// List and search stored works.
///
/// Searches this server's own store only — it does not reach out to a provider.
/// Manual overrides are applied, so what you see here is what clients are served.
#[utoipa::path(
    get, path = "/items", tag = TAG,
    params(ListQuery),
    responses(
        (status = 200, description = "Matching works, newest first", body = ListResponse),
        (status = 401, description = "No valid credential was presented"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    let language = query.language.clone();
    let query_for_count = to_query(&state, &identity, query)?;

    let mut items = repo::item::search(&state.db, &query_for_count).await?;

    // Without this the list would show provider values while the detail view
    // showed edited ones, and a lock would look like it had not taken.
    service::apply_overrides(&state, &mut items).await?;

    // A list is drawn as posters, so it needs artwork and a score. Only those:
    // pulling every child would mean reading a long-running series' whole
    // episode list to draw one thumbnail.
    repo::item::load_artwork(&state.db, &mut items).await?;

    // The same IMDb figure a work's own page leads with, or a card and the
    // page it opens could disagree about the score.
    service::overlay_imdb_many(&state, &mut items).await;

    // Shallow, not the full overlay: a grid shows titles, and fetching every
    // work's episode text to draw fifty posters would be absurd.
    if let Some(language) = language.as_deref().filter(|l| !l.is_empty()) {
        for item in &mut items {
            crate::service::language::apply_shallow(&state, item, language);
        }
    }

    let total = repo::item::count_matching(&state.db, &query_for_count).await?;
    for item in &mut items {
        service::as_card(item);
    }
    service::redact_for_reader(&identity, &mut items);

    Ok(Json(ListResponse { items, total }))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct DetailQuery {
    /// Serve the work in this language, if a translation is held. `fr`, `fr-FR`
    /// and `fra` all mean the same thing.
    pub language: Option<String>,
}

/// One work in full.
///
/// Seasons, episodes, images, credits and ratings are included, with every
/// manual override applied. `lockedFields` lists what a human has claimed.
#[utoipa::path(
    get, path = "/items/{id}", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier"), DetailQuery),
    responses(
        (status = 200, body = MediaItem),
        (status = 404, description = "No such work"),
    ),
)]
async fn detail(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(query): Query<DetailQuery>,
) -> AppResult<Json<MediaItem>> {
    let mut item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Switched off, or kept from this caller by the adult policy even were it
    // to ask: the list would never have shown it to them, and knowing the id
    // is no reason to. Whoever maintains the catalogue still opens it.
    let hidden = !item.is_enabled
        || (item.is_adult
            && !state.adult_for(identity.client_id(), identity.peer_id(), Some(true)));
    if hidden && !identity.can_write() {
        return Err(AppError::NotFound);
    }

    if let Some(language) = query.language.as_deref().filter(|l| !l.is_empty()) {
        crate::service::language::apply(&state, &mut item, language).await?;
    }

    service::redact_for_reader(&identity, std::slice::from_mut(&mut item));

    Ok(Json(item))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = CreateItemRequest)]
pub struct CreateRequest {
    pub kind: String,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub status: Option<String>,
    pub runtime: Option<i32>,
    pub first_aired: Option<String>,
    pub in_cinemas: Option<String>,
    pub network: Option<String>,
    pub studio: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub external_ids: ExternalIds,
}

/// Create an entry by hand.
///
/// The result is marked manual, which exempts it from the refresh scheduler and
/// protects its children from being replaced. If external ids are supplied it
/// still becomes refreshable — a manual entry can be a stub that later fills in
/// from a provider without losing what was typed.
#[utoipa::path(
    post, path = "/items", tag = TAG,
    request_body = CreateRequest,
    responses(
        (status = 201, description = "Created", body = MediaItem),
        (status = 400, description = "The kind or title was not usable"),
        (status = 403, description = "The caller may not write"),
        (status = 409, description = "Another entry already claims one of the external ids"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<CreateRequest>,
) -> AppResult<(StatusCode, Json<MediaItem>)> {
    require_write(&identity)?;

    let kind: MediaKind = request
        .kind
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;

    let title = request.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("title must not be empty".into()));
    }

    let mut item = MediaItem::empty(kind);
    item.title = title.to_string();
    item.year = request.year;
    item.overview = request.overview;
    item.status = request.status;
    item.runtime = request.runtime;
    item.first_aired = request.first_aired;
    item.in_cinemas = request.in_cinemas;
    item.network = request.network;
    item.studio = request.studio;
    item.genres = request.genres;
    item.external_ids = request.external_ids;
    item.is_manual = true;
    item.slug = unique_slug(&state, kind, title, request.year).await?;

    for (source, value) in item.external_ids.rows(kind) {
        if repo::item::find_id_by_external(&state.db, source, &value)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict(format!(
                "another entry already claims {source}={value}"
            )));
        }
    }

    repo::item::upsert(
        &state.db,
        repo::item::ItemWrite {
            item: &item,
            replace_children: false,
        },
    )
    .await?;
    service::listing::after_write(&state, &item.id).await;

    tracing::info!(id = %item.id, actor = %identity.label(), "created a manual entry");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ItemCreated,
            target: Some(&item.id),
            detail: Some(&format!("{} \"{}\"", item.kind, item.title)),
        },
    )
    .await;

    let stored = service::load(&state, &item.id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("entry vanished after being created")))?;

    Ok((StatusCode::CREATED, Json(stored)))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = UpdateItemRequest)]
pub struct UpdateRequest {
    pub is_enabled: Option<bool>,
}

/// Enable or disable an entry.
///
/// A disabled entry stays in the database and stops being served. Editing a
/// *field* is a different operation — see the override endpoints.
#[utoipa::path(
    patch, path = "/items/{id}", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = UpdateRequest,
    responses(
        (status = 200, body = MediaItem),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
    ),
)]
async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> AppResult<Json<MediaItem>> {
    require_write(&identity)?;

    if let Some(enabled) = request.is_enabled {
        if !repo::item::set_enabled(&state.db, &id, enabled).await? {
            return Err(AppError::NotFound);
        }
        state.caches.items.invalidate(&format!("item:{id}")).await;

        audit::record(
            &state,
            Event {
                identity: Some(&identity),
                ip: &ip,
                action: Action::ItemUpdated,
                target: Some(&id),
                detail: Some(if enabled { "enabled" } else { "disabled" }),
            },
        )
        .await;
    }

    service::load(&state, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// Delete an entry and everything attached to it, overrides included.
#[utoipa::path(
    delete, path = "/items/{id}", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_write(&identity)?;

    // Read the title before it goes, so the trail says what was deleted rather
    // than only which id.
    let title = service::load(&state, &id).await?.map(|i| i.title);

    if !repo::item::delete(&state.db, &id).await? {
        return Err(AppError::NotFound);
    }

    state.caches.items.invalidate(&format!("item:{id}")).await;
    tracing::info!(%id, actor = %identity.label(), "deleted an entry");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ItemDeleted,
            target: Some(&id),
            detail: title.as_deref(),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Refetch from providers now, ignoring the refresh schedule.
///
/// Provider data and provider-sourced children are replaced. Manual overrides
/// are not touched — that is the point of them.
#[utoipa::path(
    post, path = "/items/{id}/refresh", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, description = "The work as it now stands", body = MediaItem),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 502, description = "The provider could not be reached"),
    ),
)]
async fn refresh(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Json<MediaItem>> {
    require_write(&identity)?;

    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Someone is waiting on this one, so it earns a row of its own rather than
    // being folded into a sweep summary.
    let record = repo::job::start(&state.db, repo::job::kinds::REFRESH_ITEM, Some(&id))
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "could not open a job run"))
        .ok();

    let outcome = crate::jobs::refresh::refresh_one(&state, &item).await;

    if let Some(record) = record {
        let closed = match &outcome {
            Ok(Some(_)) => {
                repo::job::finish(&state.db, &record, Some("refreshed from a provider"), None).await
            }
            Ok(None) => {
                repo::job::finish(
                    &state.db,
                    &record,
                    Some("no provider could resolve it"),
                    None,
                )
                .await
            }
            Err(e) => repo::job::finish(&state.db, &record, None, Some(&e.to_string())).await,
        };

        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the job run");
        }
    }

    let refreshed = outcome.map_err(AppError::UpstreamUnavailable)?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ItemRefreshed,
            target: Some(&id),
            detail: Some(if refreshed.is_some() {
                "refreshed from a provider"
            } else {
                "no provider could resolve this entry"
            }),
        },
    )
    .await;

    Ok(Json(refreshed.unwrap_or(item)))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub provider: String,
    pub fetched_at: String,
    pub etag: Option<String>,
    /// Omitted when `payload=false` was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotQuery {
    /// Include each provider's raw document. On by default; a series with a
    /// thousand episodes carries megabytes of it, which a caller that only
    /// wants to know *who* answered does not need.
    #[serde(default = "yes")]
    pub payload: bool,
    /// Only this provider's snapshot: `tmdb`, `tvdb`, `skyhook`…
    pub provider: Option<String>,
}

fn yes() -> bool {
    true
}

/// The raw provider documents behind an entry.
///
/// Useful for diagnosing a mapping that produced the wrong canonical value.
#[utoipa::path(
    get, path = "/items/{id}/snapshots", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier"), SnapshotQuery),
    responses((status = 200, body = Vec<SnapshotSummary>)),
)]
async fn snapshots(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SnapshotQuery>,
) -> AppResult<Json<Vec<SnapshotSummary>>> {
    let snapshots = repo::snapshot::list(&state.db, &id).await?;

    let wanted = query
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());

    Ok(Json(
        snapshots
            .into_iter()
            .filter(|s| wanted.is_none_or(|p| s.provider == p))
            .map(|s| SnapshotSummary {
                provider: s.provider,
                fetched_at: s.fetched_at,
                etag: s.etag,
                payload: query.payload.then_some(s.payload),
            })
            .collect(),
    ))
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}

/// A slug not already taken by another work of the same kind.
///
/// Two films can share a title and a year; the slug is a primary key here, so
/// the second one gets a suffix rather than failing the create.
async fn unique_slug(
    state: &AppState,
    kind: MediaKind,
    title: &str,
    year: Option<i32>,
) -> AppResult<String> {
    let base = make_slug(title, year);

    if repo::item::find_id_by_slug(&state.db, kind, &base)
        .await?
        .is_none()
    {
        return Ok(base);
    }

    for suffix in 2..=50 {
        let candidate = format!("{base}-{suffix}");
        if repo::item::find_id_by_slug(&state.db, kind, &candidate)
            .await?
            .is_none()
        {
            return Ok(candidate);
        }
    }

    Err(AppError::Conflict(format!(
        "too many entries already share the slug {base:?}"
    )))
}
