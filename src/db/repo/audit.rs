//! The audit trail.
//!
//! Every action that changes what this server serves is recorded here: who did
//! it, to what, and when. Reads are not recorded — they would bury the entries
//! that matter under one row per metadata request.
//!
//! Failed sign-ins *are* recorded, because a run of them is the one thing in
//! this log worth an alert.

use anyhow::Result;
use serde::Serialize;

use crate::db::{Db, RowExt, new_id, now};

/// What happened. A closed set, so the UI can filter on it and a log shipper can
/// alert on it without pattern-matching prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ItemCreated,
    ItemUpdated,
    ItemDeleted,
    ItemRefreshed,
    OverrideSet,
    OverrideRemoved,
    OverridesCleared,
    ClientCreated,
    ClientUpdated,
    ClientRevoked,
    SignedIn,
    SignInFailed,
    SignedOut,
    PasswordChanged,
    CacheCleared,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ItemCreated => "item.created",
            Self::ItemUpdated => "item.updated",
            Self::ItemDeleted => "item.deleted",
            Self::ItemRefreshed => "item.refreshed",
            Self::OverrideSet => "override.set",
            Self::OverrideRemoved => "override.removed",
            Self::OverridesCleared => "override.cleared",
            Self::ClientCreated => "client.created",
            Self::ClientUpdated => "client.updated",
            Self::ClientRevoked => "client.revoked",
            Self::SignedIn => "auth.signed_in",
            Self::SignInFailed => "auth.sign_in_failed",
            Self::SignedOut => "auth.signed_out",
            Self::PasswordChanged => "auth.password_changed",
            Self::CacheCleared => "cache.cleared",
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub at: String,
    pub actor: Option<String>,
    pub action: String,
    /// What was acted on: an item id, a client name, a field path.
    pub target: Option<String>,
    /// A short human-readable note. Never carries a secret.
    pub detail: Option<String>,
    pub ip: Option<String>,
}

/// One entry to record.
pub struct Record<'a> {
    pub actor: Option<&'a str>,
    pub action: Action,
    pub target: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub ip: Option<&'a str>,
}

/// Write an entry.
///
/// Prefer [`crate::api::audit`] over calling this directly: it swallows the
/// error, which is what a handler wants — failing to record an action must not
/// fail the action.
pub async fn record(db: &Db, entry: Record<'_>) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO audit_log (id, at, actor, action, target, detail, ip)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(new_id())
    .bind(now())
    .bind(entry.actor)
    .bind(entry.action.as_str())
    .bind(entry.target)
    .bind(entry.detail)
    .bind(entry.ip)
    .execute(db.pool())
    .await?;

    Ok(())
}

#[derive(Debug, Default)]
pub struct Query {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub target: Option<String>,
    /// Only entries at or after this RFC 3339 instant.
    pub since: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

pub async fn list(db: &Db, q: &Query) -> Result<Vec<Entry>> {
    use sqlx::{Arguments, any::AnyArguments};

    let mut sql =
        String::from("SELECT id, at, actor, action, target, detail, ip FROM audit_log WHERE 1 = 1");
    let mut args = AnyArguments::default();

    // AnyArguments::add returns a boxed dyn Error, which `?` cannot widen into
    // anyhow; this keeps the four call sites below to one line each.
    let bind = |args: &mut AnyArguments, value: String| -> Result<()> {
        args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
    };

    if let Some(action) = &q.action {
        sql.push_str(" AND action = ?");
        bind(&mut args, action.clone())?;
    }
    if let Some(actor) = &q.actor {
        sql.push_str(" AND actor = ?");
        bind(&mut args, actor.clone())?;
    }
    if let Some(target) = &q.target {
        sql.push_str(" AND target = ?");
        bind(&mut args, target.clone())?;
    }
    if let Some(since) = &q.since {
        sql.push_str(" AND at >= ?");
        bind(&mut args, since.clone())?;
    }

    // Ids are UUIDv7, so they order by creation time too: a correct tiebreaker
    // when several entries land in the same millisecond.
    sql.push_str(" ORDER BY at DESC, id DESC LIMIT ? OFFSET ?");
    args.add(q.limit.clamp(1, 500))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    args.add(q.offset.max(0))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|row| {
            Ok(Entry {
                id: row.text("id")?,
                at: row.text("at")?,
                actor: row.opt_text("actor")?,
                action: row.text("action")?,
                target: row.opt_text("target")?,
                detail: row.opt_text("detail")?,
                ip: row.opt_text("ip")?,
            })
        })
        .collect()
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM audit_log"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

/// Delete entries older than `cutoff` (RFC 3339). Returns how many went.
pub async fn prune(db: &Db, cutoff: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM audit_log WHERE at < ?"))
        .bind(cutoff)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected())
}
