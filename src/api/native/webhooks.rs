//! Webhooks and notifications, from the administration: the events a
//! destination can be told of, the destinations themselves, a test of each,
//! and what was told to each — with a way to tell it again.

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
    db::repo::{
        self,
        audit::Action,
        webhook::{Counts, Delivery, Destination, NewDestination, Status},
    },
    error::{AppError, AppResult},
    service::webhook::{self, Fact, Kind, NextSecret, Outcome, Told},
    state::AppState,
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(events))
        .routes(routes!(list, create))
        .routes(routes!(change, remove))
        .routes(routes!(test))
        .routes(routes!(rotate))
        .routes(routes!(log))
        .routes(routes!(replay))
}

/// Every name a destination can be told of, besides `*`.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEvents {
    /// What happens on its own: a refresh failed, a medium given up on, a
    /// newer release out.
    pub facts: Vec<&'static str>,
    /// The actions of the audit trail, in the order its filter offers them.
    pub actions: Vec<&'static str>,
    /// What a new destination is told of, until other events are ticked.
    pub defaults: Vec<&'static str>,
    /// The kinds of destination, in the order they are offered.
    pub kinds: Vec<Kind>,
}

/// The events a destination can be told of, for the Notifications page to
/// offer.
#[utoipa::path(
    get, path = "/admin/webhooks/events", tag = TAG,
    operation_id = "webhooks_events",
    responses(
        (status = 200, body = WebhookEvents),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn events(Extension(identity): Extension<Identity>) -> AppResult<Json<WebhookEvents>> {
    identity.require_admin()?;
    Ok(Json(WebhookEvents {
        facts: Fact::ALL.iter().map(|f| f.as_str()).collect(),
        actions: Action::ALL.iter().map(|a| a.as_str()).collect(),
        defaults: webhook::DEFAULT_EVENTS.split(',').collect(),
        kinds: Kind::ALL.to_vec(),
    }))
}

// ─── destinations ────────────────────────────────────────────────────────────

/// A destination, as the administration sees it: never its secret, and its
/// address with what follows the host masked.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WebhookDestination)]
pub struct DestinationView {
    pub id: String,
    pub name: String,
    /// A kind this server does not know — a destination a later version
    /// made — is sent as it was stored, and nothing is posted to it.
    // Kept as the text stored rather than read into a `Kind`, which could
    // not hold one it does not know: the page names that one as it is.
    #[schema(value_type = Kind)]
    pub kind: String,
    /// Its address: the scheme, host and port shown, the rest masked — for
    /// most services the token is in the path.
    pub address: String,
    /// Whether a secret is kept: a standard destination's signing secret,
    /// an ntfy or Gotify one's token. Never sent back.
    pub secret_set: bool,
    /// The signing secret just made, in the answer that made it and in no
    /// other: it cannot be read again, only made anew.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// The events it is told of; `*` for every one.
    pub events: Vec<String>,
    pub enabled: bool,
    /// Whether failed refreshes are told to it together: a few minutes'
    /// worth in one message, counted by cause, rather than one per work.
    pub grouped: bool,
    /// Why the server switched it off, when it did: an address that
    /// answered 410 Gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// Since when it is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// What its log holds.
    pub deliveries: Counts,
}

fn view(destination: Destination, counts: Counts, secret: Option<String>) -> DestinationView {
    DestinationView {
        address: webhook::masked(&destination.url),
        secret_set: destination.secret.is_some(),
        secret,
        id: destination.id,
        name: destination.name,
        kind: destination.kind,
        events: destination.events,
        enabled: destination.enabled,
        grouped: destination.grouped,
        disabled_reason: destination.disabled_reason,
        disabled_at: destination.disabled_at,
        created_at: destination.created_at,
        updated_at: destination.updated_at,
        deliveries: counts,
    }
}

/// Every destination, in the order they were added.
#[utoipa::path(
    get, path = "/admin/notifications", tag = TAG,
    operation_id = "webhooks_list",
    responses(
        (status = 200, body = Vec<DestinationView>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<DestinationView>>> {
    identity.require_admin()?;
    let mut counts = repo::webhook::counts(&state.db).await?;
    let destinations = repo::webhook::all(&state.db).await?;
    Ok(Json(
        destinations
            .into_iter()
            .map(|d| {
                let of = counts.remove(&d.id).unwrap_or_default();
                view(d, of, None)
            })
            .collect(),
    ))
}

/// A destination to make, or what to change of one: each field left out
/// stays as it is.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WebhookDestinationInput)]
pub struct DestinationInput {
    pub name: Option<String>,
    /// Needed for a new one; left out on a change, the kind stays. Another
    /// word is refused with a 400 that names the kinds there are.
    // Read as text and checked by `kind_of`: read into a `Kind`, another
    // word would be axum's 422, in plain text.
    #[schema(value_type = Option<Kind>)]
    pub kind: Option<String>,
    /// Where it is posted: a webhook's address, an ntfy topic's, a Gotify
    /// server's, an Apprise API's `/notify/{key}`. Left out or empty on a
    /// change, the address kept stays.
    pub address: Option<String>,
    /// The token an ntfy topic or a Gotify application was given: left out,
    /// the one kept stays — while the destination stays the same kind at the
    /// same origin, its address's scheme, host and port; moved to another
    /// kind or origin, it is dropped unless given again. Empty, it is
    /// removed. Never sent back. A standard destination's signing secret is
    /// made here instead, and made anew at another origin.
    pub token: Option<String>,
    /// The events it is told of; `*` for every one. Left out on a new one,
    /// what enters and leaves the catalogue.
    pub events: Option<Vec<String>>,
    pub enabled: Option<bool>,
    /// Whether failed refreshes are told to it together: a few minutes'
    /// worth in one message, counted by cause. Left out on a new one, on for
    /// the kinds people read — discord, slack, ntfy, gotify, apprise — and
    /// off for standard and json, read by programs that want an event per
    /// work; left out on a change, it stays as it is.
    pub grouped: Option<bool>,
}

/// The kind asked for, or why it is not one.
fn kind_of(kind: &str) -> AppResult<Kind> {
    Kind::parse(kind.trim()).ok_or_else(|| {
        AppError::BadRequest(format!(
            "{:?} is not a kind of destination: {}",
            crate::providers::clip(kind, 40),
            Kind::ALL
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

/// What [`webhook::next_secret`] said, made: the secret to keep, and the
/// signing secret to show once if one was made.
fn secret_to_keep(next: NextSecret) -> AppResult<(Option<String>, Option<String>)> {
    match next {
        NextSecret::Store(secret) => Ok((secret, None)),
        NextSecret::Make => {
            let made = webhook::new_secret()?;
            Ok((Some(made.clone()), Some(made)))
        }
    }
}

/// Add a destination. A standard one's signing secret is made here and
/// shown in this answer, and only in this one.
#[utoipa::path(
    post, path = "/admin/notifications", tag = TAG,
    operation_id = "webhooks_create",
    request_body = DestinationInput,
    responses(
        (status = 201, body = DestinationView),
        (status = 400, description = "A field was refused; the message says which"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(input): Json<DestinationInput>,
) -> AppResult<(StatusCode, Json<DestinationView>)> {
    identity.require_admin()?;

    let bad = AppError::BadRequest;
    let kind = kind_of(input.kind.as_deref().unwrap_or_default())?;
    let name = webhook::check_name(input.name.as_deref().unwrap_or_default()).map_err(bad)?;
    let url =
        webhook::check_address(kind, input.address.as_deref().unwrap_or_default()).map_err(bad)?;
    let events = match &input.events {
        Some(events) => webhook::check_events(events).map_err(bad)?,
        None => webhook::names_in(webhook::DEFAULT_EVENTS),
    };
    let next = webhook::next_secret(None, kind, false, input.token.as_deref()).map_err(bad)?;
    let (secret, shown) = secret_to_keep(next)?;

    let made = repo::webhook::create(
        &state.db,
        &NewDestination {
            name: &name,
            kind: kind.as_str(),
            url: &url,
            secret: secret.as_deref(),
            events: &events,
            enabled: input.enabled.unwrap_or(true),
            grouped: input.grouped.unwrap_or(kind.read_by_people()),
        },
    )
    .await?;
    state.webhooks.reload(&state.db).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::NotificationCreated,
            target: Some(&made.name),
            detail: Some(kind.as_str()),
        },
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(view(made, Counts::default(), shown)),
    ))
}

/// The destination, or a 404.
async fn found(state: &AppState, id: &str) -> AppResult<Destination> {
    repo::webhook::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)
}

/// Change a destination: each field left out stays. Switched off, what
/// waits for it is failed — it is told nothing while it is off — and can be
/// replayed once it is on again. Made `standard`, or moved to another
/// origin while it is, it is given a signing secret, shown in this answer
/// only. A token kept is not carried to another kind or origin: it is given
/// again, or dropped — and a Gotify destination, which needs one, refused.
#[utoipa::path(
    patch, path = "/admin/notifications/{id}", tag = TAG,
    operation_id = "webhooks_change",
    params(("id" = String, Path, description = "The destination's identifier")),
    request_body = DestinationInput,
    responses(
        (status = 200, body = DestinationView),
        (status = 400, description = "A field was refused; the message says which"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such destination"),
    ),
)]
async fn change(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(input): Json<DestinationInput>,
) -> AppResult<Json<DestinationView>> {
    identity.require_admin()?;
    let before = found(&state, &id).await?;
    let was = Kind::parse(&before.kind);

    let bad = AppError::BadRequest;
    let kind = match &input.kind {
        Some(kind) => kind_of(kind)?,
        // A kind this server no longer knows is changed by naming another.
        None => was.ok_or_else(|| bad("name the destination's kind".to_string()))?,
    };
    let mut after = before.clone();
    let mut changed: Vec<&str> = Vec::new();

    if let Some(name) = &input.name {
        after.name = webhook::check_name(name).map_err(bad)?;
    }
    after.kind = kind.as_str().to_string();
    match input.address.as_deref().map(str::trim) {
        Some(address) if !address.is_empty() => {
            after.url = webhook::check_address(kind, address).map_err(bad)?;
        }
        // Kept as it is, but it has to suit the kind it is now.
        _ if Some(kind) != was => {
            after.url = webhook::check_address(kind, &before.url).map_err(bad)?;
        }
        _ => {}
    }
    if let Some(events) = &input.events {
        after.events = webhook::check_events(events).map_err(bad)?;
    }
    // A secret kept goes with the service it was kept for: another origin
    // — another scheme, host or port — is another server, which is given
    // no token kept for the old one, and a receiver of its own secret. An
    // address left as it was has moved nowhere, whatever it reads as.
    let moved = after.url != before.url && !crate::outbound::same_origin(&before.url, &after.url);
    let next = webhook::next_secret(
        was.map(|was| (was, before.secret.as_deref())),
        kind,
        moved,
        input.token.as_deref(),
    )
    .map_err(bad)?;
    let (secret, shown) = secret_to_keep(next)?;
    after.secret = secret;

    if let Some(grouped) = input.grouped {
        after.grouped = grouped;
    }

    if let Some(enabled) = input.enabled
        && enabled != before.enabled
    {
        after.enabled = enabled;
        after.disabled_at = (!enabled).then(crate::db::now);
        // Switched on again, the reason it was off is past; off by an
        // administrator, there is no reason to say.
        after.disabled_reason = None;
    }

    for (what, differs) in [
        ("name", after.name != before.name),
        ("kind", after.kind != before.kind),
        ("address", after.url != before.url),
        ("secret", after.secret != before.secret),
        ("events", after.events != before.events),
        ("grouping", after.grouped != before.grouped),
    ] {
        if differs {
            changed.push(what);
        }
    }
    if after.enabled != before.enabled {
        changed.push(if after.enabled {
            "switched on"
        } else {
            "switched off"
        });
    }

    if !repo::webhook::save(&state.db, &after).await? {
        return Err(AppError::NotFound);
    }
    state.webhooks.reload(&state.db).await?;

    if !changed.is_empty() {
        // What changed, by name: never the address or the token.
        audit::record(
            &state,
            Event {
                identity: Some(&identity),
                ip: &ip,
                action: Action::NotificationUpdated,
                target: Some(&after.name),
                detail: Some(&changed.join(", ")),
            },
        )
        .await;
    }

    let saved = found(&state, &id).await?;
    let counts = repo::webhook::counts(&state.db)
        .await?
        .remove(&id)
        .unwrap_or_default();
    Ok(Json(view(saved, counts, shown)))
}

/// Delete a destination, and its log with it.
#[utoipa::path(
    delete, path = "/admin/notifications/{id}", tag = TAG,
    operation_id = "webhooks_remove",
    params(("id" = String, Path, description = "The destination's identifier")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such destination"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;
    let removed = found(&state, &id).await?;
    if !repo::webhook::remove(&state.db, &id).await? {
        return Err(AppError::NotFound);
    }
    state.webhooks.reload(&state.db).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::NotificationDeleted,
            target: Some(&removed.name),
            detail: Some(&removed.kind),
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Make a standard destination's signing secret anew: the receiver needs the
/// new one from now on. Shown in this answer only.
#[utoipa::path(
    post, path = "/admin/notifications/{id}/secret", tag = TAG,
    operation_id = "webhooks_rotate",
    params(("id" = String, Path, description = "The destination's identifier")),
    responses(
        (status = 200, body = DestinationView),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such destination"),
        (status = 409, description = "Not a standard destination: it signs nothing"),
    ),
)]
async fn rotate(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Json<DestinationView>> {
    identity.require_admin()?;
    let mut destination = found(&state, &id).await?;
    if Kind::parse(&destination.kind) != Some(Kind::Standard) {
        return Err(AppError::Conflict(
            "only a standard destination is signed, and has a secret to make".into(),
        ));
    }
    let made = webhook::new_secret()?;
    destination.secret = Some(made.clone());
    if !repo::webhook::save(&state.db, &destination).await? {
        return Err(AppError::NotFound);
    }
    state.webhooks.reload(&state.db).await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::NotificationUpdated,
            target: Some(&destination.name),
            detail: Some("signing secret made anew"),
        },
    )
    .await;

    let saved = found(&state, &id).await?;
    let counts = repo::webhook::counts(&state.db)
        .await?
        .remove(&id)
        .unwrap_or_default();
    Ok(Json(view(saved, counts, Some(made))))
}

// ─── a test ──────────────────────────────────────────────────────────────────

/// Why a test was not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
// Named explicitly: utoipa keys schemas on the leaf name alone, and a
// `Refused` could be anything's.
#[schema(as = WebhookRefusal)]
pub enum Refused {
    /// Not an http(s) address, or one that names this machine.
    NotWeb,
    /// A name that resolves to an address only reachable from here.
    Internal,
    /// The post could not be made: a standard destination without its
    /// secret.
    Unsent,
}

impl From<webhook::Refusal> for Refused {
    fn from(refusal: webhook::Refusal) -> Self {
        match refusal {
            webhook::Refusal::NotWeb => Self::NotWeb,
            webhook::Refusal::Internal => Self::Internal,
        }
    }
}

/// What a test of a destination came to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookTest {
    /// Whether the address took it: it answered, with a success.
    pub delivered: bool,
    /// What it answered with, when it answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Why it was not reached, or what it said when it refused — in words
    /// that name no address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why nothing was sent: `not_web`, not an http(s) address or one that
    /// names this machine; `internal`, a name that resolves to an address
    /// only reachable from here; `unsent`, the post could not be made — a
    /// standard destination without its secret — as `error` says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<Refused>,
    /// How long it took, in milliseconds.
    pub millis: u64,
}

/// Post `webhook.test` to a destination, whatever it is told of and
/// whether it is on, and say what came of it. Nothing is written: neither
/// in the audit trail nor in its log.
#[utoipa::path(
    post, path = "/admin/notifications/{id}/test", tag = TAG,
    operation_id = "webhooks_test",
    params(("id" = String, Path, description = "The destination's identifier")),
    responses(
        (status = 200, body = WebhookTest),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such destination"),
    ),
)]
async fn test(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<WebhookTest>> {
    identity.require_admin()?;
    let destination = found(&state, &id).await?;
    let by = identity.label();
    let outcome = webhook::test(&state, &destination, &by).await;

    let millis = |took: std::time::Duration| u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
    let tried = match outcome {
        Outcome::Answered {
            status, took, said, ..
        } => WebhookTest {
            delivered: status.is_success(),
            status: Some(status.as_u16()),
            error: said,
            refused: None,
            millis: millis(took),
        },
        Outcome::Unreached { why, took } => WebhookTest {
            delivered: false,
            status: None,
            error: Some(why),
            refused: None,
            millis: millis(took),
        },
        Outcome::Refused(why) => WebhookTest {
            delivered: false,
            status: None,
            error: None,
            refused: Some(why.into()),
            millis: 0,
        },
        Outcome::Unsent { why } => WebhookTest {
            delivered: false,
            status: None,
            error: Some(why),
            refused: Some(Refused::Unsent),
            millis: 0,
        },
    };
    tracing::info!(
        by,
        destination = %destination.name,
        delivered = tried.delivered,
        status = tried.status,
        "a destination was sent a test"
    );
    Ok(Json(tried))
}

// ─── the log ─────────────────────────────────────────────────────────────────

/// One event owed to a destination, and how its tries went.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WebhookDelivery)]
pub struct DeliveryView {
    pub id: String,
    pub event: String,
    pub status: Status,
    /// The tries made, of the eight it gets.
    pub attempts: i32,
    /// When it is tried next, while it waits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<String>,
    /// What the address answered the last try with, when it answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<i32>,
    /// Why the last try was not taken, in words that name no address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// How long the last try took, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// The delivery this one sends again, when it is a replay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_of: Option<String>,
    /// What it tells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub told: Option<Told>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
}

impl From<Delivery> for DeliveryView {
    fn from(delivery: Delivery) -> Self {
        Self {
            told: serde_json::from_str(&delivery.payload).ok(),
            next_attempt_at: (delivery.status == Status::Pending)
                .then_some(delivery.next_attempt_at)
                .flatten(),
            id: delivery.id,
            event: delivery.event,
            status: delivery.status,
            attempts: delivery.attempts,
            last_status: delivery.last_status,
            last_error: delivery.last_error,
            duration_ms: delivery.duration_ms,
            replay_of: delivery.replay_of,
            created_at: delivery.created_at,
            updated_at: delivery.updated_at,
            delivered_at: delivery.delivered_at,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogQuery {
    /// How many, fifty unless said: two hundred at the most.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub offset: Option<i64>,
}

/// A page of a destination's log.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WebhookDeliveries)]
pub struct DeliveriesPage {
    pub deliveries: Vec<DeliveryView>,
    /// How many the log holds: thirty days of it, the newest thousand.
    pub total: i64,
}

/// What was told to a destination, the newest first: kept thirty days, the
/// newest thousand of it.
#[utoipa::path(
    get, path = "/admin/notifications/{id}/deliveries", tag = TAG,
    operation_id = "webhooks_log",
    params(("id" = String, Path, description = "The destination's identifier"), LogQuery),
    responses(
        (status = 200, body = DeliveriesPage),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such destination"),
    ),
)]
async fn log(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(page): Query<LogQuery>,
) -> AppResult<Json<DeliveriesPage>> {
    identity.require_admin()?;
    found(&state, &id).await?;
    let (deliveries, total) = repo::webhook::log(
        &state.db,
        &id,
        page.limit.unwrap_or(50),
        page.offset.unwrap_or(0),
    )
    .await?;
    Ok(Json(DeliveriesPage {
        deliveries: deliveries.into_iter().map(DeliveryView::from).collect(),
        total,
    }))
}

/// Tell a delivery's event again, as it was told — under the same message
/// id, so a receiver that keeps them knows it for the same. A line of its
/// own in the log, due at once.
#[utoipa::path(
    post, path = "/admin/notifications/{id}/deliveries/{delivery}/replay", tag = TAG,
    operation_id = "webhooks_replay",
    params(
        ("id" = String, Path, description = "The destination's identifier"),
        ("delivery" = String, Path, description = "The delivery's identifier"),
    ),
    responses(
        (status = 201, body = DeliveryView),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such destination or delivery"),
        (status = 409, description = "The destination is off, or the delivery still waits to be sent"),
    ),
)]
async fn replay(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((id, delivery)): Path<(String, String)>,
) -> AppResult<(StatusCode, Json<DeliveryView>)> {
    identity.require_admin()?;
    let destination = found(&state, &id).await?;
    if !destination.enabled {
        return Err(AppError::Conflict(
            "the destination is switched off: switch it on to tell it anything".into(),
        ));
    }
    let original = repo::webhook::find_delivery(&state.db, &id, &delivery)
        .await?
        .ok_or(AppError::NotFound)?;
    if original.status == Status::Pending {
        return Err(AppError::Conflict(
            "this delivery still waits to be sent".into(),
        ));
    }
    let again = repo::webhook::replay(&state.db, &original).await?;
    state.webhooks.wake();
    tracing::info!(
        by = %identity.label(),
        destination = %destination.name,
        event = %again.event,
        "a delivery was replayed"
    );
    Ok((StatusCode::CREATED, Json(DeliveryView::from(again))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Authentication switched off: an administrator, as far as a guard asks.
    fn admin() -> Extension<Identity> {
        Extension(Identity::Anonymous)
    }

    /// The address of a request that came from nowhere in particular, as
    /// the extractor reads it.
    async fn ip(state: &AppState) -> ClientIp {
        use axum::extract::FromRequestParts as _;
        let (mut parts, ()) = axum::http::Request::new(()).into_parts();
        ClientIp::from_request_parts(&mut parts, state)
            .await
            .expect("an address")
    }

    fn input(kind: &str, name: &str, address: &str) -> DestinationInput {
        DestinationInput {
            name: Some(name.into()),
            kind: Some(kind.into()),
            address: Some(address.into()),
            token: None,
            events: None,
            enabled: None,
            grouped: None,
        }
    }

    fn nothing() -> DestinationInput {
        DestinationInput {
            name: None,
            kind: None,
            address: None,
            token: None,
            events: None,
            enabled: None,
            grouped: None,
        }
    }

    #[tokio::test]
    async fn the_events_offered_are_the_facts_and_every_action() {
        let Json(offered) = events(admin()).await.expect("offered");
        assert_eq!(
            offered.facts,
            [
                "refresh.failed",
                "media.failed",
                "update.available",
                "numbering.fallback",
                "backup.completed",
                "backup.failed"
            ]
        );
        assert_eq!(offered.actions.len(), Action::ALL.len());
        assert_eq!(
            offered.defaults,
            [
                "item.imported",
                "item.created",
                "item.deleted",
                "dataset.imported"
            ]
        );
        assert_eq!(
            serde_json::to_value(&offered.kinds).unwrap(),
            serde_json::json!([
                "standard", "discord", "slack", "ntfy", "gotify", "apprise", "json"
            ])
        );
        assert!(matches!(
            events(Extension(Identity::Visitor)).await,
            Err(AppError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn a_standard_destination_is_given_its_secret_once() {
        let state = crate::api::native::testing::server().await;
        let (status, Json(made)) = create(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Json(input(
                "standard",
                " Receiver ",
                "https://hooks.example.com/in/abc",
            )),
        )
        .await
        .expect("made");
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(made.name, "Receiver");
        assert!(made.secret_set);
        let secret = made.secret.expect("shown once");
        assert!(secret.starts_with("whsec_"));
        assert_eq!(made.address, "https://hooks.example.com/••••••••");
        assert_eq!(made.events, webhook::names_in(webhook::DEFAULT_EVENTS));

        // Listed, it is never shown again.
        let Json(listed) = list(State(state.clone()), admin()).await.expect("listed");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].secret.is_none());
        assert!(listed[0].secret_set);
        let body = serde_json::to_string(&listed).unwrap();
        assert!(!body.contains(&secret));
        assert!(!body.contains("/in/abc"));

        // Made anew, a different one, shown once again.
        let Json(rotated) = rotate(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(made.id.clone()),
        )
        .await
        .expect("rotated");
        assert_ne!(rotated.secret.as_deref(), Some(secret.as_str()));

        // Changed into a Discord webhook, it keeps no secret at all.
        let Json(discord) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(made.id.clone()),
            Json(DestinationInput {
                kind: Some("discord".into()),
                ..nothing()
            }),
        )
        .await
        .expect("changed");
        assert!(!discord.secret_set);
        assert!(matches!(
            rotate(
                State(state.clone()),
                admin(),
                ip(&state).await,
                Path(made.id.clone())
            )
            .await,
            Err(AppError::Conflict(_))
        ));

        // The audit trail has it all, and never the address.
        let trail = repo::audit::list(
            &state.db,
            &repo::audit::Query {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let actions: Vec<&str> = trail.iter().map(|e| e.action.as_str()).collect();
        assert!(actions.contains(&"notification.created"));
        assert!(actions.contains(&"notification.updated"));
        assert!(trail.iter().all(|e| {
            !e.detail
                .as_deref()
                .unwrap_or_default()
                .contains("hooks.example.com")
        }));
    }

    #[tokio::test]
    async fn a_destination_is_refused_what_it_cannot_be() {
        let state = crate::api::native::testing::server().await;
        for (kind, address) in [
            ("smtp", "https://hooks.example.com/in"),
            ("json", "http://127.0.0.1:9/hook"),
            ("json", "ftp://example.com/x"),
            ("ntfy", "https://ntfy.sh/"),
            // Gotify without its token.
            ("gotify", "https://push.example.de"),
        ] {
            assert!(
                matches!(
                    create(
                        State(state.clone()),
                        admin(),
                        ip(&state).await,
                        Json(input(kind, "x", address))
                    )
                    .await,
                    Err(AppError::BadRequest(_))
                ),
                "{kind} {address}"
            );
        }
        assert!(matches!(
            create(
                State(state.clone()),
                admin(),
                ip(&state).await,
                Json(input("json", "  ", "https://hooks.example.com/in"))
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            create(
                State(state.clone()),
                Extension(Identity::Visitor),
                ip(&state).await,
                Json(input("json", "x", "https://hooks.example.com/in"))
            )
            .await,
            Err(AppError::Forbidden)
        ));
        assert!(matches!(
            remove(
                State(state.clone()),
                admin(),
                ip(&state).await,
                Path("nothing".into())
            )
            .await,
            Err(AppError::NotFound)
        ));
    }

    #[tokio::test]
    async fn failed_refreshes_are_grouped_for_people_and_not_for_programs_unless_asked() {
        let state = crate::api::native::testing::server().await;
        let made = |kind: &'static str| {
            let state = state.clone();
            async move {
                let (_, Json(made)) = create(
                    State(state.clone()),
                    admin(),
                    ip(&state).await,
                    Json(input(kind, kind, "https://hooks.example.com/in")),
                )
                .await
                .expect("made");
                made
            }
        };
        // Read by people: grouped. Read by a program: an event per work.
        assert!(made("discord").await.grouped);
        let json = made("json").await;
        assert!(!json.grouped);

        // Asked for, it is; left out on a change, it stays.
        let Json(changed) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(json.id.clone()),
            Json(DestinationInput {
                grouped: Some(true),
                ..nothing()
            }),
        )
        .await
        .expect("changed");
        assert!(changed.grouped);
        let Json(renamed) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(json.id.clone()),
            Json(DestinationInput {
                name: Some("Renamed".into()),
                ..nothing()
            }),
        )
        .await
        .expect("changed");
        assert!(renamed.grouped);
    }

    #[tokio::test]
    async fn a_token_is_kept_unseen_and_the_address_stays_when_left_out() {
        let state = crate::api::native::testing::server().await;
        let (_, Json(made)) = create(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Json(DestinationInput {
                token: Some("AbCdEf123".into()),
                events: Some(vec!["*".into(), "item.created".into()]),
                ..input("gotify", "Phone", "https://push.example.de")
            }),
        )
        .await
        .expect("made");
        assert!(made.secret_set);
        assert!(made.secret.is_none());
        assert_eq!(made.events, ["*"]);
        assert_eq!(made.address, "https://push.example.de");

        // Renamed and switched off: the address and the token stay.
        let Json(changed) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(made.id.clone()),
            Json(DestinationInput {
                name: Some("Phone, quiet".into()),
                address: Some(String::new()),
                enabled: Some(false),
                ..nothing()
            }),
        )
        .await
        .expect("changed");
        assert_eq!(changed.name, "Phone, quiet");
        assert!(!changed.enabled);
        assert!(changed.disabled_at.is_some());
        let kept = repo::webhook::find(&state.db, &made.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kept.url, "https://push.example.de");
        assert_eq!(kept.secret.as_deref(), Some("AbCdEf123"));

        // Its token emptied, a Gotify destination is refused.
        assert!(matches!(
            change(
                State(state.clone()),
                admin(),
                ip(&state).await,
                Path(made.id.clone()),
                Json(DestinationInput {
                    token: Some(String::new()),
                    ..nothing()
                }),
            )
            .await,
            Err(AppError::BadRequest(_))
        ));

        // Deleted, with its log.
        assert_eq!(
            remove(
                State(state.clone()),
                admin(),
                ip(&state).await,
                Path(made.id.clone())
            )
            .await
            .expect("deleted"),
            StatusCode::NO_CONTENT
        );
        assert!(
            repo::webhook::find(&state.db, &made.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_test_says_why_an_address_was_not_posted_to() {
        let state = crate::api::native::testing::server().await;
        // As an earlier version's setting could have held it: never checked.
        let made = repo::webhook::create(
            &state.db,
            &NewDestination {
                name: "Webhook",
                kind: "json",
                url: "http://127.0.0.1:9/hook",
                secret: None,
                events: &["*".to_string()],
                enabled: true,
                grouped: false,
            },
        )
        .await
        .unwrap();
        let Json(tried) = test(State(state.clone()), admin(), Path(made.id.clone()))
            .await
            .expect("tried");
        assert!(!tried.delivered);
        assert_eq!(tried.refused, Some(Refused::NotWeb));
        assert_eq!(serde_json::to_value(&tried).unwrap()["refused"], "not_web");
        assert_eq!(tried.status, None);
        // Nothing of it in the log.
        assert_eq!(
            repo::webhook::log(&state.db, &made.id, 10, 0)
                .await
                .unwrap()
                .1,
            0
        );

        assert!(matches!(
            test(State(state.clone()), admin(), Path("nothing".into())).await,
            Err(AppError::NotFound)
        ));
        assert!(matches!(
            test(State(state), Extension(Identity::Visitor), Path(made.id)).await,
            Err(AppError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn the_log_is_read_by_page_and_what_was_told_is_told_again() {
        let state = crate::api::native::testing::server().await;
        let (_, Json(made)) = create(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Json(DestinationInput {
                events: Some(vec!["notification.updated".into()]),
                ..input("json", "Self", "https://hooks.example.com/in")
            }),
        )
        .await
        .expect("made");

        // Changing it is an event it wants: told to itself.
        for name in ["Self, 2", "Self, 3"] {
            let Json(_) = change(
                State(state.clone()),
                admin(),
                ip(&state).await,
                Path(made.id.clone()),
                Json(DestinationInput {
                    name: Some(name.into()),
                    ..nothing()
                }),
            )
            .await
            .expect("changed");
        }
        let Json(page) = log(
            State(state.clone()),
            admin(),
            Path(made.id.clone()),
            Query(LogQuery {
                limit: Some(1),
                offset: None,
            }),
        )
        .await
        .expect("a page");
        assert_eq!(page.total, 2);
        assert_eq!(page.deliveries.len(), 1);
        let newest = &page.deliveries[0];
        assert_eq!(newest.event, "notification.updated");
        assert_eq!(newest.status, Status::Pending);
        assert_eq!(
            newest.told.as_ref().and_then(|t| t.target.as_deref()),
            Some("Self, 3")
        );

        // Still waiting: not replayed.
        assert!(matches!(
            replay(
                State(state.clone()),
                admin(),
                Path((made.id.clone(), newest.id.clone()))
            )
            .await,
            Err(AppError::Conflict(_))
        ));

        // Given up on, it is told again: a line of its own, the same event.
        let original = repo::webhook::find_delivery(&state.db, &made.id, &newest.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            repo::webhook::claim(
                &state.db,
                &original.id,
                "me",
                webhook::MOST_ATTEMPTS,
                &crate::db::now(),
                &crate::db::now()
            )
            .await
            .unwrap()
        );
        repo::webhook::settle(
            &state.db,
            &original.id,
            "me",
            &repo::webhook::Settled {
                status: Status::Failed,
                next_attempt_at: None,
                last_status: Some(500),
                last_error: None,
                duration_ms: Some(12),
            },
        )
        .await
        .unwrap();
        let (status, Json(again)) = replay(
            State(state.clone()),
            admin(),
            Path((made.id.clone(), original.id.clone())),
        )
        .await
        .expect("replayed");
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(again.replay_of.as_deref(), Some(original.id.as_str()));
        assert_eq!(again.status, Status::Pending);
        assert_eq!(again.event, "notification.updated");

        // Off, it is told nothing — not even again.
        let Json(off) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(made.id.clone()),
            Json(DestinationInput {
                enabled: Some(false),
                ..nothing()
            }),
        )
        .await
        .expect("off");
        assert!(!off.enabled);
        assert!(matches!(
            replay(
                State(state.clone()),
                admin(),
                Path((made.id.clone(), original.id))
            )
            .await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            log(
                State(state),
                admin(),
                Path("nothing".into()),
                Query(LogQuery {
                    limit: None,
                    offset: None
                })
            )
            .await,
            Err(AppError::NotFound)
        ));
    }

    #[tokio::test]
    async fn a_secret_kept_goes_to_no_other_server() {
        let state = crate::api::native::testing::server().await;
        let stored = |id: String| {
            let state = state.clone();
            async move {
                repo::webhook::find(&state.db, &id)
                    .await
                    .unwrap()
                    .expect("kept")
            }
        };
        let moved_to = |address: &str, token: Option<&str>| DestinationInput {
            address: Some(address.into()),
            token: token.map(str::to_string),
            ..nothing()
        };

        // A Gotify server's token, moved with its address to another server
        // without it: refused, saying why, and nothing written.
        let (_, Json(gotify)) = create(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Json(DestinationInput {
                token: Some("AbCdEf123".into()),
                ..input("gotify", "Phone", "https://push.example.de")
            }),
        )
        .await
        .expect("made");
        let refused = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(gotify.id.clone()),
            Json(moved_to("https://push.attacker.example", None)),
        )
        .await;
        assert!(
            matches!(&refused, Err(AppError::BadRequest(why)) if why.contains("another server")),
            "{:?}",
            refused.err()
        );
        let kept = stored(gotify.id.clone()).await;
        assert_eq!(kept.url, "https://push.example.de");
        assert_eq!(kept.secret.as_deref(), Some("AbCdEf123"));
        // Another path at the same origin is the same server: the token stays.
        let Json(_) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(gotify.id.clone()),
            Json(moved_to("https://push.example.de:443/gotify", None)),
        )
        .await
        .expect("changed");
        assert_eq!(
            stored(gotify.id.clone()).await.secret.as_deref(),
            Some("AbCdEf123")
        );
        // Given again with the new address, it goes with it.
        let Json(_) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(gotify.id.clone()),
            Json(moved_to("https://push.elsewhere.example", Some("Zz9Yy8"))),
        )
        .await
        .expect("moved");
        assert_eq!(
            stored(gotify.id.clone()).await.secret.as_deref(),
            Some("Zz9Yy8")
        );

        // An ntfy topic's token is dropped at another server, and is not
        // handed to a Gotify one either.
        let (_, Json(ntfy)) = create(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Json(DestinationInput {
                token: Some("tk_secret".into()),
                ..input("ntfy", "Topic", "https://ntfy.sh/ams-alerts")
            }),
        )
        .await
        .expect("made");
        assert!(matches!(
            change(
                State(state.clone()),
                admin(),
                ip(&state).await,
                Path(ntfy.id.clone()),
                Json(DestinationInput {
                    kind: Some("gotify".into()),
                    address: Some("https://push.example.de".into()),
                    ..nothing()
                }),
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        let Json(moved) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(ntfy.id.clone()),
            Json(moved_to("https://ntfy.example.org/ams-alerts", None)),
        )
        .await
        .expect("moved");
        assert!(!moved.secret_set);
        assert_eq!(stored(ntfy.id.clone()).await.secret, None);

        // A standard destination's receiver at another origin is given a
        // secret of its own, shown once; the same receiver keeps its own.
        let (_, Json(standard)) = create(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Json(input(
                "standard",
                "Receiver",
                "https://hooks.example.com/in",
            )),
        )
        .await
        .expect("made");
        let first = standard.secret.expect("shown once");
        let Json(same) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(standard.id.clone()),
            Json(moved_to("https://hooks.example.com/other", None)),
        )
        .await
        .expect("changed");
        assert!(same.secret.is_none());
        assert_eq!(
            stored(standard.id.clone()).await.secret.as_deref(),
            Some(first.as_str())
        );
        let Json(elsewhere) = change(
            State(state.clone()),
            admin(),
            ip(&state).await,
            Path(standard.id.clone()),
            Json(moved_to("https://receiver.example.net/in", None)),
        )
        .await
        .expect("moved");
        let second = elsewhere.secret.expect("made anew, and shown once");
        assert_ne!(second, first);
        assert_eq!(
            stored(standard.id).await.secret.as_deref(),
            Some(second.as_str())
        );
    }
}
