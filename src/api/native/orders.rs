//! The orders a series' episodes come in: TheTVDB's besides the aired one,
//! TMDB's episode groups, and those made here — read by whoever may see the
//! work, made, written, frozen as the aired numbering and forgotten by those
//! who may edit it. And who numbers the series: TheTVDB, or TMDB in its place
//! while TheTVDB does not answer for it, until somebody gives it back.

use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action, order::LOCAL},
    domain::{EpisodeOrder, MediaItem, MediaKind, OrderKind, OrderRecord, PlacedEpisode},
    error::{AppError, AppResult},
    service::{self, orders},
    state::AppState,
};

const TAG: &str = super::items::TAG;

/// The longest name an order is given.
const LABEL_MOST: usize = 80;

/// The longest title a record of an order's own is given, and its overview.
const TITLE_MOST: usize = 300;
const OVERVIEW_MOST: usize = 4000;

/// The most orders made here a work keeps: each is offered to every Sonarr
/// and every Plex that reads the series.
const MADE_MOST: usize = 20;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(orders, create))
        .routes(routes!(save, remove))
        .routes(routes!(freeze, thaw))
        .routes(routes!(restore))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Orders {
    /// The aired order: the work's own episodes, as whoever its `source`
    /// names numbers them. Absent for a film.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired: Option<EpisodeOrder>,
    /// Every other order — TheTVDB's first, the DVDs' at their head, then
    /// TMDB's episode groups, then those made here — each with its places:
    /// an episode the work holds by its id as the work gives it now, or the
    /// record the order carries of one it does not. Empty for a film, or a
    /// series numbered one way only.
    pub orders: Vec<EpisodeOrder>,
    /// Who numbers the series, and what its clients are offered.
    pub numbering: Numbering,
}

/// Who numbers a series, and what its clients are offered.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = OrderNumbering)]
pub struct Numbering {
    /// Whether TMDB numbers a series TheTVDB lists: while TheTVDB does not
    /// answer for it, or until somebody gives the numbering back.
    pub held_by_tmdb: bool,
    /// Since when TMDB numbers it in TheTVDB's place, after TheTVDB left it
    /// unanswered refresh after refresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_since: Option<String>,
    /// How many refreshes in a row, the most part of a day apart, TheTVDB
    /// has not answered for it.
    pub silent: i32,
    /// After how many TMDB numbers a series in TheTVDB's place: the
    /// `numbering.fallbackAfter` setting, `0` when it never does.
    pub fallback_after: i64,
    /// The key of the order frozen as its aired numbering: what Sonarr
    /// asking for the aired order, Sonarr v4, Plex with no order named and
    /// the TheTVDB relay are given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frozen: Option<String>,
    /// The keys of the orders Sonarr v5 and Plex are offered, the aired one
    /// aside: the same order kept by TheTVDB and by TMDB is offered once.
    pub offered: Vec<String>,
}

/// A new order, made here.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NewOrder)]
pub struct NewOrder {
    /// What it is called: what Sonarr's list of orders, Plex's and the
    /// editor show. Its key is made of it — `ams-` and its words.
    pub label: String,
    /// The key of the order its places are copied from: `official` for the
    /// aired order, as the work numbers its episodes now. Empty without.
    #[serde(default)]
    pub from: Option<String>,
}

/// An order made here, written whole.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = OrderWrite)]
pub struct OrderWrite {
    pub label: String,
    /// Its places, in its own sequence: each an episode the work holds, or
    /// a record of the order's own. A season and a number once each.
    pub places: Vec<PlaceWrite>,
}

/// One place of an order made here.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlaceWrite {
    /// The place's id, as the order gave it: what keeps the id a record of
    /// the order's own is served under the same from one save to the next.
    #[serde(default)]
    pub id: Option<String>,
    pub season_number: i32,
    pub episode_number: i32,
    #[serde(default)]
    pub absolute_number: Option<i32>,
    /// An episode the work holds, by its id as the work gives it.
    #[serde(default)]
    pub episode_id: Option<String>,
    /// Its TheTVDB id, as the order gave it: names the episode when the id
    /// above is one a refresh has written again since.
    #[serde(default)]
    pub tvdb_id: Option<i64>,
    /// Its TMDB id, likewise.
    #[serde(default)]
    pub tmdb_id: Option<i64>,
    /// A record of the order's own, for an episode the work does not hold.
    #[serde(default)]
    pub record: Option<OrderRecord>,
}

/// How else a series' episodes are numbered: by TheTVDB, by TMDB's episode
/// groups, by orders made here. The aired order is the work's own and the
/// one every client is served unless it asks for another, or one is frozen
/// as it. As kept: a TMDB group not read yet — its places are read when the
/// series is refreshed, or when a client asks in it — says only how many
/// episodes TMDB lists it with (`announced`).
#[utoipa::path(
    get, path = "/items/{id}/orders", tag = TAG,
    operation_id = "orders_orders",
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, body = Orders),
        (status = 404, description = "No such work"),
    ),
)]
async fn orders(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<Orders>> {
    // As the work's own page decides: switched off, or kept from this caller
    // by the adult policy, it is not here either.
    let item = super::visible_work(&state, &identity, &id).await?;
    // Nothing is asked of TMDB on the way: a page that shows the orders is
    // drawn once, from what is kept, however many groups TMDB lists.
    let all = repo::order::list(&state.db, &item.id).await?;
    Ok(Json(answer(&state, &item, all).await?))
}

/// The orders as they are answered: the aired one apart, each other one's
/// places named as the work names its episodes now.
async fn answer(state: &AppState, item: &MediaItem, all: Vec<EpisodeOrder>) -> AppResult<Orders> {
    let held = repo::order::numbering(&state.db, &item.id).await?;
    let offered = orders::offered(item, &all)
        .into_iter()
        .map(|order| order.key.clone())
        .collect();
    let aired = orders::aired(&all).cloned();
    let held_by_tmdb = item.external_ids.tvdb.is_some()
        && aired.as_ref().is_some_and(|aired| aired.source == "tmdb");
    let frozen = all
        .iter()
        .find(|order| order.is_frozen)
        .map(|order| order.key.clone());
    let others = all
        .iter()
        .filter(|order| !order.is_default)
        .map(|order| orders::resolved(item, order))
        .collect();
    Ok(Orders {
        aired,
        orders: others,
        numbering: Numbering {
            held_by_tmdb,
            fallback_since: held.since,
            silent: held.silent,
            fallback_after: state
                .settings
                .int_at("numbering.fallbackAfter", None, None)
                .unwrap_or(service::gather::FALLBACK_AFTER),
            frozen,
            offered,
        },
    })
}

/// Make an order: empty, or a copy of another — the aired order as the
/// work numbers its episodes now, one of TheTVDB's, a TMDB group, another
/// made here — to change as one likes. It is offered to Sonarr v5 and Plex
/// at once, under `ams-` and its name.
#[utoipa::path(
    post, path = "/items/{id}/orders", tag = TAG,
    operation_id = "orders_create",
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = NewOrder,
    responses(
        (status = 201, description = "Made", body = EpisodeOrder),
        (status = 400, description = "No name, or an order to copy the work does not have"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 409, description = "A film, or a series with as many orders made here as it may keep"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(new): Json<NewOrder>,
) -> AppResult<(StatusCode, Json<EpisodeOrder>)> {
    identity.require_write()?;
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if item.kind != MediaKind::Series {
        return Err(AppError::Conflict(
            "a film has one order: there is nothing to number otherwise".into(),
        ));
    }
    let label = label_of(&new.label)?;

    let all = match new.from.as_deref() {
        Some(from) => orders::for_clients(&state, &item, Some(from)).await?,
        None => repo::order::list(&state.db, &item.id).await?,
    };
    if all.iter().filter(|order| order.source == LOCAL).count() >= MADE_MOST {
        return Err(AppError::Conflict(format!(
            "a series keeps {MADE_MOST} orders made here at the most: forget one first"
        )));
    }
    let places = match new.from.as_deref().map(str::trim) {
        None | Some("") => Vec::new(),
        Some(from) if from.eq_ignore_ascii_case(orders::AIRED) => orders::places_from(&item, None),
        Some(from) => {
            let source = orders::named(&all, from).ok_or_else(|| {
                AppError::BadRequest(format!("the work has no order {from:?} to copy"))
            })?;
            orders::places_from(&item, Some(source))
        }
    };

    let taken = repo::order::keys(&state.db, &item.id).await?;
    let mut keys_taken = taken;
    // TheTVDB's own and the aired one are never one made here.
    keys_taken.push(orders::AIRED.to_string());
    let made = EpisodeOrder {
        key: orders::key_for(&label, &keys_taken),
        kind: OrderKind::Custom,
        source: LOCAL.to_string(),
        source_ref: None,
        label,
        is_default: false,
        is_frozen: false,
        announced: None,
        updated_at: crate::db::now(),
        episodes: places,
    };
    repo::order::create(&state.db, &item.id, &made).await?;
    state.caches.touched(&item.id).await;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OrderCreated,
            target: Some(&target(&item.id, &made.key)),
            detail: Some(&format!(
                "“{}”: order “{}” made{}",
                item.title,
                made.label,
                new.from
                    .as_deref()
                    .map(|from| format!(" from {from}"))
                    .unwrap_or_default()
            )),
        },
    )
    .await;

    let kept = repo::order::get(&state.db, &item.id, &made.key)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok((StatusCode::CREATED, Json(orders::resolved(&item, &kept))))
}

/// Write an order made here, whole: its name and its places, each an
/// episode the work holds or a record of its own, a season and a number once
/// each. An order read from TheTVDB or TMDB is read again at every refresh
/// and is not written here: make an order from it to change it.
#[utoipa::path(
    put, path = "/items/{id}/orders/{key}", tag = TAG,
    operation_id = "orders_save",
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("key" = String, Path, description = "The order's key, `ams-…`"),
    ),
    request_body = OrderWrite,
    responses(
        (status = 200, description = "Written", body = EpisodeOrder),
        (status = 400, description = "A place that names no episode the work holds and carries no record, one given twice, a number out of range"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, or no such order"),
        (status = 409, description = "An order read from a source, which is not written here"),
    ),
)]
async fn save(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, key)): Path<(String, String)>,
    Json(write): Json<OrderWrite>,
) -> AppResult<Json<EpisodeOrder>> {
    identity.require_write()?;
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let label = label_of(&write.label)?;
    let held = repo::order::get(&state.db, &item.id, &key)
        .await?
        .ok_or(AppError::NotFound)?;
    if held.source != LOCAL {
        return Err(AppError::Conflict(format!(
            "“{}” is read from {} at every refresh and is not written here: make an order from it \
             to change it",
            held.label,
            source_name(&held.source)
        )));
    }
    let places = places_of(&item, &write.places)?;
    if !repo::order::save(&state.db, &item.id, &key, &label, &places).await? {
        return Err(AppError::NotFound);
    }
    state.caches.touched(&item.id).await;
    if held.is_frozen {
        // What the aired order answers with moved.
        crate::arr::push::offer(&state, &item.id, crate::arr::push::Reason::Edit);
    }

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OrderUpdated,
            target: Some(&target(&item.id, &key)),
            detail: Some(&format!(
                "“{}”: order “{label}” written, {} places",
                item.title,
                places.len()
            )),
        },
    )
    .await;

    let kept = repo::order::get(&state.db, &item.id, &key)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(orders::resolved(&item, &kept)))
}

/// Forget an order made here. Frozen as the aired numbering, the aired order
/// answers as the work numbers its episodes again.
#[utoipa::path(
    delete, path = "/items/{id}/orders/{key}", tag = TAG,
    operation_id = "orders_remove",
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("key" = String, Path, description = "The order's key, `ams-…`"),
    ),
    responses(
        (status = 204, description = "Forgotten"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, or no such order"),
        (status = 409, description = "An order read from a source, which comes back at the next refresh"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, key)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    identity.require_write()?;
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let held = repo::order::get(&state.db, &item.id, &key)
        .await?
        .ok_or(AppError::NotFound)?;
    if held.source != LOCAL {
        return Err(AppError::Conflict(format!(
            "“{}” is read from {} and comes back at the next refresh",
            held.label,
            source_name(&held.source)
        )));
    }
    if !repo::order::delete(&state.db, &item.id, &key).await? {
        return Err(AppError::NotFound);
    }
    state.caches.touched(&item.id).await;
    if held.is_frozen {
        crate::arr::push::offer(&state, &item.id, crate::arr::push::Reason::Edit);
    }

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OrderDeleted,
            target: Some(&target(&item.id, &key)),
            detail: Some(&format!(
                "“{}”: order “{}” forgotten",
                item.title, held.label
            )),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Freeze an order as the aired numbering: what Sonarr asking for the aired
/// order (`official`) is given from now on, and Sonarr v4, Plex with no
/// order named, the TheTVDB relay and the `.nfo` documents with it. Sonarr
/// renumbers its episodes at its next refresh, and files it matched by the
/// old numbers stand for other episodes; XEM's mappings, made for TheTVDB's
/// aired numbers, no longer fit. One order at the most is frozen: freezing
/// one thaws any other. Undone by thawing it.
#[utoipa::path(
    post, path = "/items/{id}/orders/{key}/freeze", tag = TAG,
    operation_id = "orders_freeze",
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("key" = String, Path, description = "The order's key"),
    ),
    responses(
        (status = 200, description = "Frozen: the orders as they stand", body = Orders),
        (status = 400, description = "The aired order itself, which a frozen order stands in for"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, or no such order"),
        (status = 409, description = "An order that places no episode"),
    ),
)]
async fn freeze(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, key)): Path<(String, String)>,
) -> AppResult<Json<Orders>> {
    identity.require_write()?;
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if key.eq_ignore_ascii_case(orders::AIRED) {
        return Err(AppError::BadRequest(
            "the aired order is what a frozen order stands in for: make an order from it, and \
             freeze that"
                .into(),
        ));
    }
    let all = orders::for_clients(&state, &item, Some(&key)).await?;
    let order = orders::named(&all, &key).ok_or(AppError::NotFound)?;
    if orders::placements(&item, order).is_empty() {
        return Err(AppError::Conflict(format!(
            "“{}” places no episode: there would be nothing to number",
            order.label
        )));
    }
    let label = order.label.clone();
    let key = order.key.clone();
    if !repo::order::freeze(&state.db, &item.id, Some(&key)).await? {
        return Err(AppError::NotFound);
    }
    changed_numbering(&state, &item.id).await;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OrderFrozen,
            target: Some(&target(&item.id, &key)),
            detail: Some(&format!(
                "“{}”: order “{label}” frozen as the aired numbering",
                item.title
            )),
        },
    )
    .await;

    let all = repo::order::list(&state.db, &item.id).await?;
    Ok(Json(answer(&state, &item, all).await?))
}

/// Thaw the order frozen as the aired numbering: the aired order answers as
/// the work numbers its episodes again, and Sonarr renumbers at its next
/// refresh.
#[utoipa::path(
    delete, path = "/items/{id}/orders/{key}/freeze", tag = TAG,
    operation_id = "orders_thaw",
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("key" = String, Path, description = "The key of the order frozen"),
    ),
    responses(
        (status = 200, description = "Thawed: the orders as they stand", body = Orders),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, or that order is not the one frozen"),
    ),
)]
async fn thaw(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, key)): Path<(String, String)>,
) -> AppResult<Json<Orders>> {
    identity.require_write()?;
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    let frozen = repo::order::frozen(&state.db, &item.id).await?;
    if !frozen.is_some_and(|frozen| frozen.eq_ignore_ascii_case(&key)) {
        return Err(AppError::NotFound);
    }
    repo::order::freeze(&state.db, &item.id, None).await?;
    changed_numbering(&state, &item.id).await;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OrderThawed,
            target: Some(&target(&item.id, &key)),
            detail: Some(&format!(
                "“{}”: the aired numbering is the work's own again",
                item.title
            )),
        },
    )
    .await;

    let all = repo::order::list(&state.db, &item.id).await?;
    Ok(Json(answer(&state, &item, all).await?))
}

/// Give the numbering back to TheTVDB: the series is refreshed at once, and
/// numbered by TheTVDB again when it answers — even with an order frozen,
/// which a return of TheTVDB's own accord leaves with TMDB. When it still
/// does not answer, TMDB keeps the numbering, as before.
#[utoipa::path(
    post, path = "/items/{id}/numbering/restore", tag = TAG,
    operation_id = "orders_restore",
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, description = "Refreshed: the orders as they stand, and who numbers the series", body = Orders),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work"),
        (status = 409, description = "A series TMDB does not number, or one with no TheTVDB id"),
        (status = 502, description = "The refresh failed altogether"),
    ),
)]
async fn restore(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Json<Orders>> {
    identity.require_write()?;
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if item.kind != MediaKind::Series || item.external_ids.tvdb.is_none() {
        return Err(AppError::Conflict(
            "only a series TheTVDB lists is numbered by it".into(),
        ));
    }
    let held = repo::order::numbering(&state.db, &item.id).await?;
    let before = repo::order::list(&state.db, &item.id).await?;
    let by_tmdb = orders::aired(&before).is_some_and(|aired| aired.source == "tmdb");
    if held.since.is_none() && !by_tmdb {
        return Err(AppError::Conflict(
            "TheTVDB numbers this series already".into(),
        ));
    }

    repo::order::set_numbering(&state.db, &item.id, &repo::order::Numbering::default()).await?;
    let refreshed = crate::jobs::refresh::refresh_one(&state, &item)
        .await
        .map_err(AppError::UpstreamUnavailable)?
        .unwrap_or(item);

    let after = repo::order::list(&state.db, &refreshed.id).await?;
    let back = orders::aired(&after).is_some_and(|aired| aired.source == "tvdb");
    if !back && held.since.is_some() {
        // TheTVDB did not answer: TMDB numbers it as it did, since then.
        let mut still = repo::order::numbering(&state.db, &refreshed.id).await?;
        still.since = held.since.clone();
        repo::order::set_numbering(&state.db, &refreshed.id, &still).await?;
    }
    state.caches.touched(&refreshed.id).await;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::ItemNumberingRestored,
            target: Some(&refreshed.id),
            detail: Some(&if back {
                format!("“{}”: the numbering given back to TheTVDB", refreshed.title)
            } else {
                format!(
                    "“{}”: the numbering given back to TheTVDB, which did not answer; TMDB keeps it",
                    refreshed.title
                )
            }),
        },
    )
    .await;

    Ok(Json(answer(&state, &refreshed, after).await?))
}

/// What the audit trail names an order by: its work, then its key.
fn target(id: &str, key: &str) -> String {
    format!("{id}#orders/{key}")
}

fn source_name(source: &str) -> &str {
    match source {
        "tvdb" => "TheTVDB",
        "tmdb" => "TMDB",
        other => other,
    }
}

/// The work's numbering moved: its copies are dropped, and the operator's
/// Sonarr told, when telling is on.
async fn changed_numbering(state: &AppState, id: &str) {
    state.caches.touched(id).await;
    crate::arr::push::offer(state, id, crate::arr::push::Reason::Edit);
}

/// A name, as an order is given one: trimmed, on one line, and not too long.
fn label_of(text: &str) -> AppResult<String> {
    let label: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty() {
        return Err(AppError::BadRequest("an order is given a name".into()));
    }
    if label.chars().count() > LABEL_MOST {
        return Err(AppError::BadRequest(format!(
            "an order's name is {LABEL_MOST} characters at the most"
        )));
    }
    Ok(label)
}

/// The places written, checked and named as they are kept: an episode the
/// work holds by every id that names it, a record of the order's own as it
/// was written.
fn places_of(item: &MediaItem, written: &[PlaceWrite]) -> AppResult<Vec<PlacedEpisode>> {
    if written.len() > orders::PLACES_MOST {
        return Err(AppError::BadRequest(format!(
            "an order holds {} places at the most",
            orders::PLACES_MOST
        )));
    }
    let mut seen = HashSet::new();
    let mut places = Vec::with_capacity(written.len());
    for (at, place) in written.iter().enumerate() {
        let position = at + 1;
        if !(0..=9999).contains(&place.season_number)
            || !(0..=99_999).contains(&place.episode_number)
            || place.absolute_number.is_some_and(|n| n < 0)
        {
            return Err(AppError::BadRequest(format!(
                "place {position}: a season is numbered 0 to 9999, an episode 0 to 99999"
            )));
        }
        if !seen.insert((place.season_number, place.episode_number)) {
            return Err(AppError::BadRequest(format!(
                "place {position}: S{:02}E{:02} is given twice",
                place.season_number, place.episode_number
            )));
        }
        let id = place
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.len() <= 64)
            .map(str::to_string)
            .unwrap_or_else(crate::db::new_id);

        let held = place
            .episode_id
            .as_deref()
            .and_then(|row| item.episodes.iter().find(|e| e.id == row))
            .or_else(|| {
                place
                    .tvdb_id
                    .and_then(|id| item.episodes.iter().find(|e| e.tvdb_id == Some(id)))
            })
            .or_else(|| {
                place
                    .tmdb_id
                    .and_then(|id| item.episodes.iter().find(|e| e.tmdb_id == Some(id)))
            });
        let made = match (held, &place.record) {
            (Some(episode), _) => {
                let mut made = orders::place_of(
                    episode,
                    place.season_number,
                    place.episode_number,
                    place.absolute_number,
                );
                made.id = id;
                made
            }
            (None, Some(record)) => PlacedEpisode {
                id,
                season_number: place.season_number,
                episode_number: place.episode_number,
                absolute_number: place.absolute_number,
                episode_id: None,
                tvdb_id: place.tvdb_id,
                tmdb_id: place.tmdb_id,
                record: Some(record_of(record, position)?),
            },
            (None, None) => {
                return Err(AppError::BadRequest(format!(
                    "place {position}: it names no episode the work holds and carries no record of \
                     its own"
                )));
            }
        };
        places.push(made);
    }
    Ok(places)
}

/// A record of an order's own, as written: a title, and what else is said of
/// the episode in the shapes it is kept in.
fn record_of(record: &OrderRecord, position: usize) -> AppResult<OrderRecord> {
    let title = record
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() || title.chars().count() > TITLE_MOST {
        return Err(AppError::BadRequest(format!(
            "place {position}: a record of the order's own is given a title, of \
             {TITLE_MOST} characters at the most"
        )));
    }
    let overview = record
        .overview
        .as_deref()
        .map(str::trim)
        .filter(|o| !o.is_empty());
    if overview.is_some_and(|o| o.chars().count() > OVERVIEW_MOST) {
        return Err(AppError::BadRequest(format!(
            "place {position}: an overview is {OVERVIEW_MOST} characters at the most"
        )));
    }
    let aired = record
        .aired
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    if aired.is_some_and(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_err()) {
        return Err(AppError::BadRequest(format!(
            "place {position}: a day is written YYYY-MM-DD"
        )));
    }
    if record.runtime.is_some_and(|r| !(0..=1440).contains(&r)) {
        return Err(AppError::BadRequest(format!(
            "place {position}: a runtime is 0 to 1440 minutes"
        )));
    }
    let image = record
        .image
        .as_deref()
        .map(str::trim)
        .filter(|i| !i.is_empty());
    if image
        .is_some_and(|i| !(i.starts_with("https://") || i.starts_with("http://")) || i.len() > 2048)
    {
        return Err(AppError::BadRequest(format!(
            "place {position}: a still is an http or https address"
        )));
    }
    Ok(OrderRecord {
        title,
        overview: overview.map(str::to_string),
        aired: aired.map(str::to_string),
        runtime: record.runtime.filter(|r| *r > 0),
        image: image.map(str::to_string),
        finale_type: None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        api::native::testing::server,
        db::repo::{
            child::{blank_episode, blank_season},
            item::ItemWrite,
        },
        domain::ExternalIds,
    };

    /// *Firefly* as its sources gave it: three episodes, a DVD order that
    /// swaps the first two, and the record TheTVDB gives of one the aired
    /// list lacks.
    async fn firefly(state: &AppState) -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = "Firefly".into();
        work.slug = "firefly-2002".into();
        work.external_ids = ExternalIds {
            tvdb: Some(78874),
            ..Default::default()
        };
        let mut season = blank_season(1);
        season.is_manual = false;
        work.seasons.push(season);
        for (number, tvdb_id) in [(1, 297989), (2, 297999), (3, 298000)] {
            let mut episode = blank_episode(1, number);
            episode.is_manual = false;
            episode.title = format!("Episode {tvdb_id}");
            episode.tvdb_id = Some(tvdb_id);
            episode.air_date = Some(format!("2002-09-{:02}", 19 + number));
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
        let mut extra = PlacedEpisode::of_tvdb(300000, 0, 1);
        extra.record = Some(OrderRecord {
            title: "Here's How It Was".into(),
            overview: None,
            aired: None,
            runtime: Some(30),
            image: None,
            finale_type: None,
        });
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
                    PlacedEpisode::of_tvdb(297999, 1, 1),
                    PlacedEpisode::of_tvdb(297989, 1, 2),
                    extra,
                ],
            }],
        )
        .await
        .expect("the order kept");
        work
    }

    fn as_json(value: &impl Serialize) -> Value {
        serde_json::to_value(value).expect("serialised")
    }

    async fn listed(state: &AppState, work: &MediaItem) -> Value {
        let Json(listed) = orders(
            State(state.clone()),
            Extension(Identity::Anonymous),
            Path(work.id.clone()),
        )
        .await
        .expect("listed");
        as_json(&listed)
    }

    async fn made(state: &AppState, work: &MediaItem, new: Value) -> AppResult<Value> {
        let (status, Json(order)) = create(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(work.id.clone()),
            Json(serde_json::from_value(new).expect("a new order")),
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED);
        Ok(as_json(&order))
    }

    async fn saved(
        state: &AppState,
        work: &MediaItem,
        key: &str,
        write: Value,
    ) -> AppResult<Value> {
        let Json(order) = save(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path((work.id.clone(), key.to_string())),
            Json(serde_json::from_value(write).expect("an order written")),
        )
        .await?;
        Ok(as_json(&order))
    }

    fn says(result: AppResult<impl Sized>) -> (StatusCode, String) {
        let error = result.err().expect("refused");
        let status = axum::response::IntoResponse::into_response(match &error {
            AppError::BadRequest(m) => AppError::BadRequest(m.clone()),
            AppError::Conflict(m) => AppError::Conflict(m.clone()),
            AppError::NotFound => AppError::NotFound,
            other => panic!("{other:?}"),
        })
        .status();
        (status, format!("{error:?}"))
    }

    #[tokio::test]
    async fn an_order_is_made_from_another_written_frozen_thawed_and_forgotten() {
        let state = server().await;
        let work = firefly(&state).await;
        let anyone = || Extension(Identity::Anonymous);
        let both = |key: &str| Path((work.id.clone(), key.to_string()));

        let first_listed = listed(&state, &work).await;
        assert_eq!(first_listed["aired"]["key"], "official");
        assert_eq!(first_listed["aired"]["source"], "tvdb");
        assert_eq!(first_listed["orders"][0]["key"], "dvd");
        assert_eq!(first_listed["orders"][0]["label"], "DVD Order");
        // A place the work holds is named by the episode's id as the work
        // gives it; the one it does not, by its record.
        let places = first_listed["orders"][0]["episodes"].as_array().unwrap();
        assert!(places[0]["episodeId"].is_string(), "{first_listed}");
        assert!(places[0].get("record").is_none(), "{first_listed}");
        assert_eq!(places[2]["record"]["title"], "Here's How It Was");
        assert_eq!(first_listed["numbering"]["offered"], json!(["dvd"]));
        assert_eq!(first_listed["numbering"]["heldByTmdb"], false);
        assert_eq!(first_listed["numbering"]["fallbackAfter"], 3);

        // Made from the DVD order: its places, the record among them.
        let mine = made(
            &state,
            &work,
            json!({ "label": "  My  DVD order ", "from": "dvd" }),
        )
        .await
        .expect("made");
        assert_eq!(mine["key"], "ams-my-dvd-order");
        assert_eq!(mine["label"], "My DVD order");
        assert_eq!(mine["kind"], "custom");
        assert_eq!(mine["source"], "local");
        assert_eq!(mine["episodes"].as_array().unwrap().len(), 3);
        let key = mine["key"].as_str().unwrap().to_string();

        // Written: the first two swapped back, a record of its own added,
        // one place named by its TheTVDB id alone.
        let first = &mine["episodes"][0];
        let second = &mine["episodes"][1];
        let written = saved(
            &state,
            &work,
            &key,
            json!({
                "label": "My order",
                "places": [
                    { "seasonNumber": 1, "episodeNumber": 1, "episodeId": second["episodeId"] },
                    { "seasonNumber": 1, "episodeNumber": 2, "episodeId": first["episodeId"] },
                    { "seasonNumber": 1, "episodeNumber": 3, "tvdbId": 298000 },
                    { "seasonNumber": 0, "episodeNumber": 1, "record": { "title": "The Message (Extended)", "aired": "2003-07-15", "runtime": 50 } }
                ]
            }),
        )
        .await
        .expect("written");
        assert_eq!(written["label"], "My order");
        assert_eq!(
            written["episodes"][3]["record"]["title"],
            "The Message (Extended)"
        );
        assert_eq!(written["episodes"][2]["tvdbId"], 298000);
        assert_eq!(written["episodes"][0]["tvdbId"], 297989);

        // Places that cannot be kept are refused, and say which.
        for (places, word) in [
            (
                json!([{ "seasonNumber": 1, "episodeNumber": 1, "episodeId": "nobody" }]),
                "place 1",
            ),
            (
                json!([
                    { "seasonNumber": 1, "episodeNumber": 1, "tvdbId": 297989 },
                    { "seasonNumber": 1, "episodeNumber": 1, "tvdbId": 297999 }
                ]),
                "given twice",
            ),
            (
                json!([{ "seasonNumber": 1, "episodeNumber": 1, "record": { "title": "x", "aired": "15/07/2003" } }]),
                "YYYY-MM-DD",
            ),
        ] {
            let (status, said) = says(
                saved(
                    &state,
                    &work,
                    &key,
                    json!({ "label": "My order", "places": places }),
                )
                .await,
            );
            assert_eq!(status, StatusCode::BAD_REQUEST, "{said}");
            assert!(said.contains(word), "{said}");
        }
        // TheTVDB's order is read at every refresh, and not written here.
        let (status, _) = says(
            saved(
                &state,
                &work,
                "dvd",
                json!({ "label": "Mine", "places": [] }),
            )
            .await,
        );
        assert_eq!(status, StatusCode::CONFLICT);

        // Frozen: the aired order answers with it.
        let Json(frozen) = freeze(State(state.clone()), anyone(), ClientIp::NONE, both(&key))
            .await
            .expect("frozen");
        assert_eq!(frozen.numbering.frozen.as_deref(), Some(key.as_str()));
        let (status, _) = says(
            freeze(
                State(state.clone()),
                anyone(),
                ClientIp::NONE,
                both("official"),
            )
            .await,
        );
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Thawed by the wrong key: nothing; by its own: the work's own again.
        let (status, _) =
            says(thaw(State(state.clone()), anyone(), ClientIp::NONE, both("dvd")).await);
        assert_eq!(status, StatusCode::NOT_FOUND);
        let Json(thawed) = thaw(State(state.clone()), anyone(), ClientIp::NONE, both(&key))
            .await
            .expect("thawed");
        assert_eq!(thawed.numbering.frozen, None);

        // Forgotten; TheTVDB's own is not.
        let (status, _) =
            says(remove(State(state.clone()), anyone(), ClientIp::NONE, both("dvd")).await);
        assert_eq!(status, StatusCode::CONFLICT);
        let status = remove(State(state.clone()), anyone(), ClientIp::NONE, both(&key))
            .await
            .expect("forgotten");
        assert_eq!(status, StatusCode::NO_CONTENT);
        let after = listed(&state, &work).await;
        assert_eq!(after["orders"].as_array().unwrap().len(), 1);

        // The trail says each.
        let trail = repo::audit::list(
            &state.db,
            &repo::audit::Query {
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .expect("the trail");
        let actions: Vec<&str> = trail.iter().map(|e| e.action.as_str()).collect();
        for action in [
            "order.created",
            "order.updated",
            "order.frozen",
            "order.thawed",
            "order.deleted",
        ] {
            assert!(actions.contains(&action), "{action} in {actions:?}");
        }
    }

    #[tokio::test]
    async fn an_order_made_empty_takes_the_next_free_key_and_places_nothing_to_freeze() {
        let state = server().await;
        let work = firefly(&state).await;

        for expected in ["ams-mine", "ams-mine-2"] {
            let mine = made(&state, &work, json!({ "label": "Mine" }))
                .await
                .expect("made");
            assert_eq!(mine["key"], expected);
            assert!(mine["episodes"].as_array().unwrap().is_empty());
        }
        for refused in [
            json!({ "label": "  " }),
            json!({ "label": "x", "from": "nonsense" }),
        ] {
            let (status, _) = says(made(&state, &work, refused).await);
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        // An empty order places nothing, and is not frozen.
        let (status, _) = says(
            freeze(
                State(state.clone()),
                Extension(Identity::Anonymous),
                ClientIp::NONE,
                Path((work.id.clone(), "ams-mine".to_string())),
            )
            .await,
        );
        assert_eq!(status, StatusCode::CONFLICT);
        // Nor is a numbering TheTVDB holds given back to it.
        let (status, _) = says(
            restore(
                State(state.clone()),
                Extension(Identity::Anonymous),
                ClientIp::NONE,
                Path(work.id.clone()),
            )
            .await,
        );
        assert_eq!(status, StatusCode::CONFLICT);
        // And a reader writes nothing.
        let reader = create(
            State(state.clone()),
            Extension(Identity::Visitor),
            ClientIp::NONE,
            Path(work.id.clone()),
            Json(NewOrder {
                label: "Theirs".into(),
                from: None,
            }),
        )
        .await;
        assert!(matches!(reader, Err(AppError::Forbidden)));
    }
}
