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
use utoipa::ToSchema;

use crate::db::{Db, RowExt, new_id, now};

/// The actions, their names and the order the trail's filter offers them in,
/// from one list: an action added here is offered, and none can be offered
/// without being added. A list kept beside the enum by hand had four of them
/// missing from the filter.
macro_rules! actions {
    ($($action:ident => $name:literal,)*) => {
        /// What happened. A closed set, so the UI can filter on it and a log
        /// shipper can alert on it without pattern-matching prose.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Action {
            $($action,)*
        }

        impl Action {
            /// Every action, in the order the trail's filter offers them.
            pub const ALL: &'static [Action] = &[$(Self::$action,)*];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$action => $name,)*
                }
            }
        }
    };
}

actions! {
    ItemCreated => "item.created",
    ItemImported => "item.imported",
    ItemUpdated => "item.updated",
    ItemRefreshed => "item.refreshed",
    ItemSynced => "item.synced",
    ItemRenumbered => "item.renumbered",
    ItemNumberingFallback => "item.numbering_fallback",
    ItemNumberingRestored => "item.numbering_restored",
    OrderCreated => "order.created",
    OrderUpdated => "order.updated",
    OrderDeleted => "order.deleted",
    OrderFrozen => "order.frozen",
    OrderThawed => "order.thawed",
    MediaUploaded => "media.uploaded",
    MediaRemoved => "media.removed",
    CertificateRenewed => "tls.renewed",
    CacheFlushed => "cache.flushed",
    ItemDeleted => "item.deleted",
    ItemMerged => "item.merged",
    ItemUnmerged => "item.unmerged",
    DuplicateDismissed => "duplicate.dismissed",
    PersonCreated => "person.created",
    PersonUpdated => "person.updated",
    PersonMerged => "person.merged",
    PersonSplit => "person.split",
    PersonCredited => "person.credited",
    OverrideSet => "override.set",
    OverrideRemoved => "override.removed",
    OverridesCleared => "override.cleared",
    LocksImported => "override.imported",
    ClientCreated => "client.created",
    ClientUpdated => "client.updated",
    ClientRevoked => "client.revoked",
    ClientRotated => "client.rotated",
    UserCreated => "user.created",
    UserUpdated => "user.updated",
    UserDeleted => "user.deleted",
    UserPasswordReset => "user.password_reset",
    SessionsRevoked => "auth.sessions_revoked",
    ProfileUpdated => "account.updated",
    UserRegistered => "user.registered",
    InvitationCreated => "invitation.created",
    InvitationRevoked => "invitation.revoked",
    UserLinked => "user.linked",
    UserUnlinked => "user.unlinked",
    OidcConfigured => "oidc.configured",
    TaskStarted => "task.started",
    TaskStopped => "task.stopped",
    TaskScheduled => "task.scheduled",
    ListCreated => "list.created",
    ListUpdated => "list.updated",
    ListDeleted => "list.deleted",
    ImportRuleCreated => "import_rule.created",
    ImportRuleUpdated => "import_rule.updated",
    ImportRuleDeleted => "import_rule.deleted",
    EditorialUpdated => "editorial.updated",
    NetworkRuleAdded => "network.rule_added",
    NetworkRuleRemoved => "network.rule_removed",
    SettingChanged => "setting.changed",
    NotificationCreated => "notification.created",
    NotificationUpdated => "notification.updated",
    NotificationDeleted => "notification.deleted",
    SignedIn => "auth.signed_in",
    SignInFailed => "auth.sign_in_failed",
    SignedOut => "auth.signed_out",
    PasswordChanged => "auth.password_changed",
    CacheCleared => "cache.cleared",
    NfoExported => "export.nfo",
    DatasetImported => "dataset.imported",
    BackupDeleted => "backup.deleted",
    BackupRestored => "backup.restored",
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = AuditEntry)]
pub struct Entry {
    pub id: String,
    pub at: String,
    #[schema(required)]
    pub actor: Option<String>,
    pub action: String,
    /// What was acted on: an item id, a client name, a field path.
    #[schema(required)]
    pub target: Option<String>,
    /// A short human-readable note. Never carries a secret.
    #[schema(required)]
    pub detail: Option<String>,
    #[schema(required)]
    pub ip: Option<String>,
    /// The work the target names, when it names one still held: its title,
    /// for a reader who does not know works by their ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::db::repo::item::WorkRef>,
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

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub target: Option<String>,
    /// Only entries at or after this RFC 3339 instant.
    pub since: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// The conditions a query puts on the trail, and what they are bound to.
fn filtered(q: &Query) -> Result<(String, sqlx::any::AnyArguments)> {
    use sqlx::{Arguments, any::AnyArguments};

    let mut sql = String::from(" WHERE 1 = 1");
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

    Ok((sql, args))
}

/// An entry as a row holds it, before its work is named.
fn read(row: &sqlx::any::AnyRow) -> Result<Entry> {
    Ok(Entry {
        id: row.text("id")?,
        at: row.text("at")?,
        actor: row.opt_text("actor")?,
        action: row.text("action")?,
        target: row.opt_text("target")?,
        detail: row.opt_text("detail")?,
        ip: row.opt_text("ip")?,
        work: None,
    })
}

pub async fn list(db: &Db, q: &Query) -> Result<Vec<Entry>> {
    use sqlx::Arguments;

    let (conditions, mut args) = filtered(q)?;
    let mut sql =
        format!("SELECT id, at, actor, action, target, detail, ip FROM audit_log{conditions}");

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

    rows.iter().map(read).collect()
}

/// The entries a query's conditions match after `after` — the instant and the
/// id of the last one read — newest first, `limit` at the most: what an export
/// walks the whole trail with, a page at a time, however long it has grown
/// and whatever is written to it meanwhile. The query's own page is passed
/// over.
pub async fn page_after(
    db: &Db,
    q: &Query,
    after: Option<(&str, &str)>,
    limit: i64,
) -> Result<Vec<Entry>> {
    use sqlx::Arguments;

    let (conditions, mut args) = filtered(q)?;
    let mut sql =
        format!("SELECT id, at, actor, action, target, detail, ip FROM audit_log{conditions}");
    if let Some((at, id)) = after {
        sql.push_str(" AND (at < ? OR (at = ? AND id < ?))");
        for value in [at, at, id] {
            args.add(value.to_string())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    sql.push_str(" ORDER BY at DESC, id DESC LIMIT ?");
    args.add(limit.clamp(1, 1000))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    rows.iter().map(read).collect()
}

/// How many entries a query's conditions match, however many pages of them.
pub async fn count_matching(db: &Db, q: &Query) -> Result<i64> {
    let (conditions, args) = filtered(q)?;
    let row = sqlx::query_with(
        db.sql(&format!("SELECT COUNT(*) AS n FROM audit_log{conditions}")),
        args,
    )
    .fetch_one(db.pool())
    .await?;

    Ok(row.big("n")?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::db;

    #[tokio::test]
    async fn the_total_is_what_the_filters_match_and_the_pages_walk_it() {
        let db = db().await;
        for (actor, action) in [
            ("admin", Action::SignedIn),
            ("admin", Action::OverrideSet),
            ("admin", Action::OverrideSet),
            ("other", Action::OverrideSet),
            ("admin", Action::SignedOut),
        ] {
            record(
                &db,
                Record {
                    actor: Some(actor),
                    action,
                    target: None,
                    detail: None,
                    ip: None,
                },
            )
            .await
            .unwrap();
        }

        let query = |offset| Query {
            action: Some("override.set".into()),
            actor: Some("admin".into()),
            limit: 1,
            offset,
            ..Default::default()
        };
        assert_eq!(count_matching(&db, &query(0)).await.unwrap(), 2);
        assert_eq!(list(&db, &query(0)).await.unwrap().len(), 1);
        assert_eq!(list(&db, &query(1)).await.unwrap().len(), 1);
        assert!(list(&db, &query(2)).await.unwrap().is_empty());
        assert_eq!(count_matching(&db, &Query::default()).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn an_export_walks_every_entry_the_filters_match_a_page_at_a_time() {
        let db = db().await;
        for n in 0..7 {
            record(
                &db,
                Record {
                    actor: Some(if n % 2 == 0 { "admin" } else { "other" }),
                    action: Action::OverrideSet,
                    target: Some(&format!("target {n}")),
                    detail: None,
                    ip: None,
                },
            )
            .await
            .unwrap();
        }

        let mine = Query {
            actor: Some("admin".into()),
            // A page of the list is no page of the export.
            limit: 1,
            offset: 3,
            ..Default::default()
        };
        let mut read = Vec::new();
        let mut after: Option<(String, String)> = None;
        loop {
            let page = page_after(
                &db,
                &mine,
                after.as_ref().map(|(at, id)| (at.as_str(), id.as_str())),
                2,
            )
            .await
            .unwrap();
            let Some(last) = page.last() else { break };
            after = Some((last.at.clone(), last.id.clone()));
            read.extend(
                page.into_iter()
                    .map(|entry| entry.target.unwrap_or_default()),
            );
        }
        // Newest first, every one of the actor's, once.
        assert_eq!(read, ["target 6", "target 4", "target 2", "target 0"]);
    }
}
