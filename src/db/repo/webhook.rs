//! Where what happens is told, and what was told to each.
//!
//! A destination is an address and the way it is posted to; a delivery is
//! one event owed to one destination, from the moment it happens until it is
//! taken, given up on, or forgotten with the rest of the log. The worker that
//! sends them is [`crate::service::webhook`]'s: this module only reads and
//! writes the rows, each change in one statement, so that two instances
//! asking at once are answered once.

use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;
use sqlx::any::AnyRow;
use utoipa::ToSchema;

use crate::db::{Db, RowExt, from_bool, new_id, now, text_list};

/// Where a delivery stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: an account, an asset and the answer of a Sonarr or a
// Radarr each have a `Status` too, and utoipa keys schemas on the leaf name
// alone.
#[schema(as = WebhookDeliveryStatus)]
pub enum Status {
    /// Not sent yet, or to be tried again.
    Pending,
    /// Taken: the address answered with a success.
    Delivered,
    /// Given up on: its tries spent, the address gone, or its destination
    /// switched off before it could be sent.
    Failed,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "delivered" => Self::Delivered,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// One destination, as stored. Its secret is here for the sender; the API
/// says only whether there is one.
#[derive(Clone, Debug)]
pub struct Destination {
    pub id: String,
    pub name: String,
    /// `standard`, `json`, `discord`, `slack`, `ntfy`, `gotify` or `apprise`.
    pub kind: String,
    pub url: String,
    pub secret: Option<String>,
    /// The names of the events it is told of; `*` for every one.
    pub events: Vec<String>,
    pub enabled: bool,
    /// Whether failed refreshes are told to it together, a few minutes'
    /// worth in one message, rather than one message per work.
    pub grouped: bool,
    /// Why the server switched it off, when it did.
    pub disabled_reason: Option<String>,
    pub disabled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

const DESTINATION_COLUMNS: &str = "id, name, kind, url, secret, events, enabled, grouped, \
     disabled_reason, disabled_at, created_at, updated_at";

fn destination(row: &AnyRow) -> Result<Destination, sqlx::Error> {
    Ok(Destination {
        id: row.text("id")?,
        name: row.text("name")?,
        kind: row.text("kind")?,
        url: row.text("url")?,
        secret: row.opt_text("secret")?,
        events: row.text_list("events")?,
        enabled: row.flag("enabled")?,
        grouped: row.flag("grouped")?,
        disabled_reason: row.opt_text("disabled_reason")?,
        disabled_at: row.opt_text("disabled_at")?,
        created_at: row.text("created_at")?,
        updated_at: row.text("updated_at")?,
    })
}

/// A destination to make.
pub struct NewDestination<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub url: &'a str,
    pub secret: Option<&'a str>,
    pub events: &'a [String],
    pub enabled: bool,
    pub grouped: bool,
}

pub async fn create(db: &Db, new: &NewDestination<'_>) -> Result<Destination> {
    let id = new_id();
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO webhook_destination
             (id, name, kind, url, secret, events, enabled, grouped, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(new.name)
    .bind(new.kind)
    .bind(new.url)
    .bind(new.secret)
    .bind(text_list(new.events))
    .bind(from_bool(new.enabled))
    .bind(from_bool(new.grouped))
    .bind(&at)
    .bind(&at)
    .execute(db.pool())
    .await?;

    Ok(Destination {
        id,
        name: new.name.to_string(),
        kind: new.kind.to_string(),
        url: new.url.to_string(),
        secret: new.secret.map(str::to_string),
        events: new.events.to_vec(),
        enabled: new.enabled,
        grouped: new.grouped,
        disabled_reason: None,
        disabled_at: None,
        created_at: at.clone(),
        updated_at: at,
    })
}

/// Every destination, the oldest first: the order they were added in.
pub async fn all(db: &Db) -> Result<Vec<Destination>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {DESTINATION_COLUMNS} FROM webhook_destination ORDER BY created_at, id"
    )))
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(destination).collect::<Result<_, _>>()?)
}

/// The destinations switched on: those an event can be owed to.
pub async fn enabled(db: &Db) -> Result<Vec<Destination>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {DESTINATION_COLUMNS} FROM webhook_destination WHERE enabled = 1
         ORDER BY created_at, id"
    )))
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(destination).collect::<Result<_, _>>()?)
}

pub async fn find(db: &Db, id: &str) -> Result<Option<Destination>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {DESTINATION_COLUMNS} FROM webhook_destination WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.as_ref().map(destination).transpose()?)
}

/// How many destinations there are, switched on or not.
pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM webhook_destination"))
        .fetch_one(db.pool())
        .await?;
    Ok(row.big("n")?)
}

/// What a delivery left owed is noted as when its destination is switched
/// off: nothing was sent, and nothing will be unless it is replayed.
pub const SWITCHED_OFF: &str = "not sent: the destination was switched off";

/// Write a destination as it now stands. One switched off here — by an
/// administrator, or by the server after a 410 — owes nothing any more:
/// what was waiting for it is failed, for the log to show and a replay to
/// send again. `false` when there is no such destination.
pub async fn save(db: &Db, destination: &Destination) -> Result<bool> {
    let mut tx = db.begin_write().await?;
    let written = sqlx::query(db.sql(
        "UPDATE webhook_destination
         SET name = ?, kind = ?, url = ?, secret = ?, events = ?, enabled = ?, grouped = ?,
             disabled_reason = ?, disabled_at = ?, updated_at = ?
         WHERE id = ?",
    ))
    .bind(&destination.name)
    .bind(&destination.kind)
    .bind(&destination.url)
    .bind(destination.secret.as_deref())
    .bind(text_list(&destination.events))
    .bind(from_bool(destination.enabled))
    .bind(from_bool(destination.grouped))
    .bind(destination.disabled_reason.as_deref())
    .bind(destination.disabled_at.as_deref())
    .bind(now())
    .bind(&destination.id)
    .execute(&mut *tx)
    .await?;
    if written.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(false);
    }

    if !destination.enabled {
        fail_pending(&mut tx, db, &destination.id).await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Switch a destination off, the server's doing: why, and since when.
pub async fn switch_off(db: &Db, id: &str, reason: &str) -> Result<()> {
    let at = now();
    let mut tx = db.begin_write().await?;
    sqlx::query(db.sql(
        "UPDATE webhook_destination
         SET enabled = 0, disabled_reason = ?, disabled_at = ?, updated_at = ?
         WHERE id = ?",
    ))
    .bind(reason)
    .bind(&at)
    .bind(&at)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    fail_pending(&mut tx, db, id).await?;
    tx.commit().await?;
    Ok(())
}

/// The deliveries waiting for a destination, failed as switched off. Not one
/// a worker holds: it is settled by its worker, which knows how it went.
async fn fail_pending(
    tx: &mut sqlx::Transaction<'static, sqlx::Any>,
    db: &Db,
    destination_id: &str,
) -> Result<()> {
    sqlx::query(db.sql(
        "UPDATE webhook_delivery
         SET status = 'failed', next_attempt_at = NULL, last_error = ?, updated_at = ?
         WHERE destination_id = ? AND status = 'pending' AND claimed_by IS NULL",
    ))
    .bind(SWITCHED_OFF)
    .bind(now())
    .bind(destination_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Delete a destination, and its log with it. `false` when there was none.
pub async fn remove(db: &Db, id: &str) -> Result<bool> {
    let mut tx = db.begin_write().await?;
    sqlx::query(db.sql("DELETE FROM webhook_delivery WHERE destination_id = ?"))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let removed = sqlx::query(db.sql("DELETE FROM webhook_destination WHERE id = ?"))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(removed.rows_affected() > 0)
}

// ─── deliveries ──────────────────────────────────────────────────────────────

/// One event owed to one destination, and how its tries went.
#[derive(Clone, Debug)]
pub struct Delivery {
    pub id: String,
    pub destination_id: String,
    pub message_id: String,
    pub event: String,
    /// What is told, as JSON: see [`crate::service::webhook::Told`].
    pub payload: String,
    pub status: Status,
    pub attempts: i32,
    pub next_attempt_at: Option<String>,
    pub last_status: Option<i32>,
    pub last_error: Option<String>,
    pub duration_ms: Option<i64>,
    pub replay_of: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub delivered_at: Option<String>,
}

const DELIVERY_COLUMNS: &str = "id, destination_id, message_id, event, payload, status, attempts, \
     next_attempt_at, last_status, last_error, duration_ms, replay_of, created_at, updated_at, \
     delivered_at";

fn delivery(row: &AnyRow) -> Result<Delivery, sqlx::Error> {
    Ok(Delivery {
        id: row.text("id")?,
        destination_id: row.text("destination_id")?,
        message_id: row.text("message_id")?,
        event: row.text("event")?,
        payload: row.text("payload")?,
        status: Status::parse(&row.text("status")?),
        attempts: row.int("attempts")?,
        next_attempt_at: row.opt_text("next_attempt_at")?,
        last_status: row.opt_int("last_status")?,
        last_error: row.opt_text("last_error")?,
        duration_ms: row.opt_big("duration_ms")?,
        replay_of: row.opt_text("replay_of")?,
        created_at: row.text("created_at")?,
        updated_at: row.text("updated_at")?,
        delivered_at: row.opt_text("delivered_at")?,
    })
}

/// Owe an event to a destination: due at once. Not while that destination
/// already has `most_pending` waiting — an address that has stopped
/// answering is not queued for without end. Whether it was put in line.
pub async fn enqueue(
    db: &Db,
    destination_id: &str,
    message_id: &str,
    event: &str,
    payload: &str,
    most_pending: i64,
) -> Result<bool> {
    let at = now();
    let done = sqlx::query(db.sql(
        "INSERT INTO webhook_delivery
             (id, destination_id, message_id, event, payload, status, attempts,
              next_attempt_at, created_at, updated_at)
         SELECT ?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?
         WHERE (SELECT COUNT(*) FROM webhook_delivery
                WHERE destination_id = ? AND status = 'pending') < ?",
    ))
    .bind(new_id())
    .bind(destination_id)
    .bind(message_id)
    .bind(event)
    .bind(payload)
    .bind(&at)
    .bind(&at)
    .bind(&at)
    .bind(destination_id)
    .bind(most_pending)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// The deliveries to send now: waiting, due, of a destination switched on,
/// with a try left, and not held lately by a worker — this instance's or
/// another's. The longest waiting first.
pub async fn due(
    db: &Db,
    most_attempts: i32,
    claim_cutoff: &str,
    limit: i64,
) -> Result<Vec<Delivery>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM webhook_delivery
         WHERE status = 'pending' AND attempts < ?
           AND next_attempt_at <= ?
           AND (claimed_at IS NULL OR claimed_at < ?)
           AND destination_id IN (SELECT id FROM webhook_destination WHERE enabled = 1)
         ORDER BY next_attempt_at, id
         LIMIT ?"
    )))
    .bind(most_attempts)
    .bind(now())
    .bind(claim_cutoff)
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(delivery).collect::<Result<_, _>>()?)
}

/// The deliveries of one destination to send now, as [`due`] takes them and
/// in the same order, at most `limit`: what is put off, all of it, once its
/// address has been found not to answer.
pub async fn due_of(
    db: &Db,
    destination_id: &str,
    most_attempts: i32,
    claim_cutoff: &str,
    limit: i64,
) -> Result<Vec<Delivery>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM webhook_delivery
         WHERE destination_id = ? AND status = 'pending' AND attempts < ?
           AND next_attempt_at <= ?
           AND (claimed_at IS NULL OR claimed_at < ?)
         ORDER BY next_attempt_at, id
         LIMIT ?"
    )))
    .bind(destination_id)
    .bind(most_attempts)
    .bind(now())
    .bind(claim_cutoff)
    .bind(limit.max(1))
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(delivery).collect::<Result<_, _>>()?)
}

/// When the next delivery falls due, of those [`due`] would take once their
/// time comes: what the worker sleeps until.
pub async fn next_due(db: &Db, most_attempts: i32, claim_cutoff: &str) -> Result<Option<String>> {
    let row = sqlx::query(db.sql(
        "SELECT MIN(next_attempt_at) AS next FROM webhook_delivery
         WHERE status = 'pending' AND attempts < ?
           AND (claimed_at IS NULL OR claimed_at < ?)
           AND destination_id IN (SELECT id FROM webhook_destination WHERE enabled = 1)",
    ))
    .bind(most_attempts)
    .bind(claim_cutoff)
    .fetch_one(db.pool())
    .await?;
    Ok(row.opt_text("next")?)
}

/// What a delivery whose last try never came back is failed with.
pub const CUT_SHORT: &str = "the last try was cut short: the server stopped while sending it";

/// Fail the deliveries whose every try was taken and whose last one never
/// came back — the server stopped while sending it, and its claim has gone
/// stale. How many.
pub async fn give_up_spent(db: &Db, most_attempts: i32, claim_cutoff: &str) -> Result<u64> {
    let done = sqlx::query(db.sql(
        "UPDATE webhook_delivery
         SET status = 'failed', next_attempt_at = NULL, claimed_by = NULL, claimed_at = NULL,
             last_error = ?, updated_at = ?
         WHERE status = 'pending' AND attempts >= ?
           AND (claimed_at IS NULL OR claimed_at < ?)",
    ))
    .bind(CUT_SHORT)
    .bind(now())
    .bind(most_attempts)
    .bind(claim_cutoff)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected())
}

/// Take a delivery to send it: whether it was this worker's to take — still
/// waiting and due, with a try left, and not held lately by another. One
/// statement, so two workers asking at once are answered yes once.
///
/// The try is counted here, before anything is sent, and the delivery is
/// put off until `fallback_at`: a server that stops mid-send finds it due
/// again then, its try spent, rather than at once and forever.
pub async fn claim(
    db: &Db,
    id: &str,
    by: &str,
    most_attempts: i32,
    claim_cutoff: &str,
    fallback_at: &str,
) -> Result<bool> {
    let at = now();
    let done = sqlx::query(db.sql(
        "UPDATE webhook_delivery
         SET claimed_by = ?, claimed_at = ?, attempts = attempts + 1, next_attempt_at = ?,
             updated_at = ?
         WHERE id = ? AND status = 'pending' AND attempts < ?
           AND next_attempt_at <= ?
           AND (claimed_at IS NULL OR claimed_at < ?)",
    ))
    .bind(by)
    .bind(&at)
    .bind(fallback_at)
    .bind(&at)
    .bind(id)
    .bind(most_attempts)
    .bind(&at)
    .bind(claim_cutoff)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// How a try went, to write on its delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub status: Status,
    /// When to try again, while it is still waiting.
    pub next_attempt_at: Option<String>,
    /// What the address answered with, when it answered.
    pub last_status: Option<i32>,
    /// Why it was not taken, in words that name no address.
    pub last_error: Option<String>,
    pub duration_ms: Option<i64>,
}

/// Write how a try went, and let go of the delivery. Only while `by` still
/// holds it: one taken over by another worker is that worker's to settle.
///
/// A try that was under way when its destination was switched off came back
/// to a destination that owes nothing any more: one that is to be tried
/// again is failed instead, as what waited for it was when it was switched
/// off.
pub async fn settle(db: &Db, id: &str, by: &str, settled: &Settled) -> Result<bool> {
    let written = write_settled(db, id, by, settled).await?;
    if written && settled.status == Status::Pending {
        sqlx::query(db.sql(
            "UPDATE webhook_delivery
             SET status = 'failed', next_attempt_at = NULL, last_error = ?, updated_at = ?
             WHERE id = ? AND status = 'pending' AND claimed_by IS NULL
               AND destination_id IN (SELECT id FROM webhook_destination WHERE enabled = 0)",
        ))
        .bind(SWITCHED_OFF)
        .bind(now())
        .bind(id)
        .execute(db.pool())
        .await?;
    }
    Ok(written)
}

async fn write_settled(db: &Db, id: &str, by: &str, settled: &Settled) -> Result<bool> {
    let at = now();
    let delivered_at = (settled.status == Status::Delivered).then(|| at.clone());
    let done = sqlx::query(db.sql(
        "UPDATE webhook_delivery
         SET status = ?, next_attempt_at = ?, last_status = ?, last_error = ?, duration_ms = ?,
             claimed_by = NULL, claimed_at = NULL, updated_at = ?,
             delivered_at = COALESCE(?, delivered_at)
         WHERE id = ? AND claimed_by = ?",
    ))
    .bind(settled.status.as_str())
    .bind(settled.next_attempt_at.as_deref())
    .bind(settled.last_status)
    .bind(settled.last_error.as_deref())
    .bind(settled.duration_ms)
    .bind(&at)
    .bind(delivered_at.as_deref())
    .bind(id)
    .bind(by)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// A page of a destination's log, the newest first, and how many it holds.
pub async fn log(
    db: &Db,
    destination_id: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Delivery>, i64)> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM webhook_delivery WHERE destination_id = ?
         ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
    )))
    .bind(destination_id)
    .bind(limit.clamp(1, 200))
    .bind(offset.max(0))
    .fetch_all(db.pool())
    .await?;
    let total =
        sqlx::query(db.sql("SELECT COUNT(*) AS n FROM webhook_delivery WHERE destination_id = ?"))
            .bind(destination_id)
            .fetch_one(db.pool())
            .await?
            .big("n")?;
    Ok((rows.iter().map(delivery).collect::<Result<_, _>>()?, total))
}

/// A destination's log after a delivery — newest first, past the one made
/// at `after`'s instant with its id — `limit` at the most, and how many the
/// whole log holds: what a page read by cursor holds, whatever was told
/// meanwhile.
pub async fn log_after(
    db: &Db,
    destination_id: &str,
    after: (&str, &str),
    limit: i64,
) -> Result<(Vec<Delivery>, i64)> {
    let (at, id) = after;
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM webhook_delivery WHERE destination_id = ?
            AND (created_at < ? OR (created_at = ? AND id < ?))
         ORDER BY created_at DESC, id DESC LIMIT ?"
    )))
    .bind(destination_id)
    .bind(at)
    .bind(at)
    .bind(id)
    .bind(limit.clamp(1, 201))
    .fetch_all(db.pool())
    .await?;
    let total =
        sqlx::query(db.sql("SELECT COUNT(*) AS n FROM webhook_delivery WHERE destination_id = ?"))
            .bind(destination_id)
            .fetch_one(db.pool())
            .await?
            .big("n")?;
    Ok((rows.iter().map(delivery).collect::<Result<_, _>>()?, total))
}

/// One delivery of one destination.
pub async fn find_delivery(db: &Db, destination_id: &str, id: &str) -> Result<Option<Delivery>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM webhook_delivery WHERE id = ? AND destination_id = ?"
    )))
    .bind(id)
    .bind(destination_id)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.as_ref().map(delivery).transpose()?)
}

/// Owe a delivery's event again, as it was told: a line of its own, due at
/// once, under the same message id — the receiver is told the same event,
/// and knows it for one.
pub async fn replay(db: &Db, original: &Delivery) -> Result<Delivery> {
    let id = new_id();
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO webhook_delivery
             (id, destination_id, message_id, event, payload, status, attempts,
              next_attempt_at, replay_of, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(&original.destination_id)
    .bind(&original.message_id)
    .bind(&original.event)
    .bind(&original.payload)
    .bind(&at)
    .bind(&original.id)
    .bind(&at)
    .bind(&at)
    .execute(db.pool())
    .await?;

    Ok(Delivery {
        id,
        destination_id: original.destination_id.clone(),
        message_id: original.message_id.clone(),
        event: original.event.clone(),
        payload: original.payload.clone(),
        status: Status::Pending,
        attempts: 0,
        next_attempt_at: Some(at.clone()),
        last_status: None,
        last_error: None,
        duration_ms: None,
        replay_of: Some(original.id.clone()),
        created_at: at.clone(),
        updated_at: at,
        delivered_at: None,
    })
}

/// How many deliveries of each standing a destination's log holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: the accounts and the media are counted in a `Counts` too,
// and utoipa keys schemas on the leaf name alone.
#[schema(as = WebhookDeliveryCounts)]
pub struct Counts {
    pub pending: i64,
    pub delivered: i64,
    pub failed: i64,
}

/// Every destination's counts, by its id. One with no delivery is absent.
pub async fn counts(db: &Db) -> Result<HashMap<String, Counts>> {
    let rows = sqlx::query(db.sql(
        "SELECT destination_id, status, COUNT(*) AS n FROM webhook_delivery
         GROUP BY destination_id, status",
    ))
    .fetch_all(db.pool())
    .await?;
    let mut counts: HashMap<String, Counts> = HashMap::new();
    for row in &rows {
        let of = counts.entry(row.text("destination_id")?).or_default();
        let n = row.big("n")?;
        match Status::parse(&row.text("status")?) {
            Status::Pending => of.pending += n,
            Status::Delivered => of.delivered += n,
            Status::Failed => of.failed += n,
        }
    }
    Ok(counts)
}

/// Forget the log past `cutoff`, and past the newest `most_kept` of each
/// destination — what is still waiting excepted, as long as it is younger
/// than the cutoff. How many were forgotten.
pub async fn prune(db: &Db, cutoff: &str, most_kept: i64) -> Result<u64> {
    let mut removed = sqlx::query(db.sql("DELETE FROM webhook_delivery WHERE created_at < ?"))
        .bind(cutoff)
        .execute(db.pool())
        .await?
        .rows_affected();

    for destination in all(db).await? {
        removed += sqlx::query(db.sql(
            "DELETE FROM webhook_delivery
             WHERE destination_id = ? AND status <> 'pending'
               AND id NOT IN (SELECT id FROM webhook_delivery
                              WHERE destination_id = ? AND status <> 'pending'
                              ORDER BY created_at DESC, id DESC LIMIT ?)",
        ))
        .bind(&destination.id)
        .bind(&destination.id)
        .bind(most_kept)
        .execute(db.pool())
        .await?
        .rows_affected();
    }
    Ok(removed)
}
