//! The journal of each work: what changed in it, value by value.
//!
//! A refresh replaces the copy each source gave, and the audit trail says
//! that a lock was set, not what it held. This keeps, for each work, every
//! value that changed — of the work, of a season, of an episode, of a child a
//! lock names — what it held and what it took, why, and who: a person, a
//! source, or both when a person asked the sources again. How a refresh's are
//! found is `merge::provenance::changes`; a lock's are written where the lock
//! is (`api::native::history`).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Arguments as _, any::AnyArguments};
use utoipa::ToSchema;

use crate::{
    db::{Db, RowExt, from_bool, new_id, now},
    merge::provenance::FieldChange,
};

/// Why a value changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: the leaf name alone says nothing in the document.
#[schema(as = ChangeCause)]
pub enum Cause {
    /// A refresh: every source asked again, by hand or by the schedule.
    Refresh,
    /// Some sources asked again, as a person chose them.
    Sync,
    /// TMDB numbering a series in TheTVDB's place, or TheTVDB taking it back.
    Fallback,
    /// A lock set, or changed.
    Lock,
    /// A lock lifted.
    Unlock,
    /// A value put back from the journal: a lock set to what a field held,
    /// or a change undone.
    Revert,
    /// An edit of what has no source to be locked against: a work, a season
    /// or an episode entered by hand.
    Edit,
    /// Another record of the same work folded into this one.
    Merge,
    /// Locks brought in from a file.
    Import,
}

impl Cause {
    pub const ALL: &'static [Cause] = &[
        Self::Refresh,
        Self::Sync,
        Self::Fallback,
        Self::Lock,
        Self::Unlock,
        Self::Revert,
        Self::Edit,
        Self::Merge,
        Self::Import,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Sync => "sync",
            Self::Fallback => "fallback",
            Self::Lock => "lock",
            Self::Unlock => "unlock",
            Self::Revert => "revert",
            Self::Edit => "edit",
            Self::Merge => "merge",
            Self::Import => "import",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|cause| cause.as_str() == text)
    }

    /// Whether a source made the change, rather than a person or another
    /// record.
    pub const fn is_a_source(self) -> bool {
        matches!(self, Self::Refresh | Self::Sync | Self::Fallback)
    }
}

/// The causes that are a source's, as the SQL below names them.
const BY_A_SOURCE: &str = "('refresh', 'sync', 'fallback')";

/// One change to write.
#[derive(Clone, Debug, PartialEq)]
pub struct NewChange {
    pub cause: Cause,
    pub scope: String,
    pub field: String,
    /// The language a text was locked in alone, as translations are filed
    /// (`fra`); empty otherwise.
    pub language: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub provider: Option<String>,
    pub was_locked: bool,
}

impl NewChange {
    /// A change a refresh or a sync found, for `cause`.
    pub fn found(change: FieldChange, cause: Cause, was_locked: bool) -> Self {
        Self {
            cause,
            scope: change.scope,
            field: change.field,
            language: String::new(),
            before: change.before,
            after: change.after,
            provider: change.provider,
            was_locked,
        }
    }
}

/// A change, as the journal reads it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare an `Entry`, and utoipa keys
// schemas on the leaf name alone.
#[schema(as = ChangeEntry)]
pub struct Entry {
    pub id: String,
    pub at: String,
    pub cause: Cause,
    /// Who, as the audit trail names them — a person, or a key; nothing for
    /// the schedule's own doing.
    #[schema(required)]
    pub actor: Option<String>,
    /// The source whose answer the new value is, where one is known.
    #[schema(required)]
    pub provider: Option<String>,
    /// `item`, `season:2`, `episode:2x5`, or a child a lock names, as a lock
    /// names it: `credit:actor:tmdb:17419`.
    pub scope: String,
    /// The field, as the field registry names it; for an episode, `number`
    /// when it moved, `episode` when it came or went — `season` for a season.
    pub field: String,
    /// The language a text was locked in alone (`fra`); absent for every
    /// language, and for what is in none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// What it held; `null` for nothing.
    #[schema(required)]
    pub before: Option<Value>,
    /// What it took; `null` for nothing.
    #[schema(required)]
    pub after: Option<Value>,
    /// Whether a lock held the field as it changed: a source's change made
    /// under one reached the record and not what is served.
    pub was_locked: bool,
}

/// Rows per statement: twelve columns each, well inside every engine's
/// bind-parameter ceiling.
const CHUNK: usize = 200;

const COLUMNS: &str = "id, at, cause, actor, provider, scope, field, language, value_before, \
                       value_after, was_locked";

/// Write what changed in a work at once, as `actor` did it — a statement a
/// couple of hundred rows, in one transaction — and nothing at all for no
/// change, which is what most refreshes find.
pub async fn record(
    db: &Db,
    media_id: &str,
    actor: Option<&str>,
    changes: &[NewChange],
) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }

    let at = now();
    let mut tx = db.begin_write().await?;
    for chunk in changes.chunks(CHUNK) {
        let sql = format!(
            "INSERT INTO media_change
                 (id, media_id, at, cause, actor, provider, scope, field, language,
                  value_before, value_after, was_locked)
             VALUES {}",
            vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );

        let mut args = AnyArguments::default();
        for change in chunk {
            let before = change
                .before
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            let after = change
                .after
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            for result in [
                args.add(new_id()),
                args.add(media_id.to_string()),
                args.add(at.clone()),
                args.add(change.cause.as_str()),
                args.add(actor.map(str::to_string)),
                args.add(change.provider.clone()),
                args.add(change.scope.clone()),
                args.add(change.field.clone()),
                args.add(change.language.clone()),
                args.add(before),
                args.add(after),
                args.add(from_bool(change.was_locked)),
            ] {
                result.map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }

        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// What a reading of a work's journal keeps to.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// One field, as the registry names it, wherever it is.
    pub field: Option<String>,
    /// A person or a source: whoever made the change, or whose answer it
    /// was.
    pub actor: Option<String>,
    /// Only changes at or after this RFC 3339 instant.
    pub since: Option<String>,
    /// Only changes before this one.
    pub before: Option<String>,
}

/// One page of a work's journal, newest first, and where the next begins.
#[derive(Debug)]
pub struct Page {
    pub entries: Vec<Entry>,
    /// What to ask for the next page with; nothing after the last.
    pub next: Option<String>,
}

/// A place in the journal: a change's instant and its id. The changes one
/// write made share the instant, and their ids tell them apart.
fn cursor_of(entry: &Entry) -> String {
    format!("{}_{}", entry.at, entry.id)
}

fn parse_cursor(cursor: &str) -> Option<(&str, &str)> {
    cursor.split_once('_')
}

/// A page of a work's journal, newest first — the changes one refresh made
/// in the order the work lists them — `limit` at the most, after `cursor`
/// when one is given. A cursor that names no place reads as none.
pub async fn list(
    db: &Db,
    media_id: &str,
    filter: &Filter,
    cursor: Option<&str>,
    limit: i64,
) -> Result<Page> {
    let limit = limit.clamp(1, 500);
    let mut sql = format!("SELECT {COLUMNS} FROM media_change WHERE media_id = ?");
    let mut args = AnyArguments::default();
    let bind = |args: &mut AnyArguments, value: String| -> Result<()> {
        args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
    };
    bind(&mut args, media_id.to_string())?;

    if let Some(field) = &filter.field {
        sql.push_str(" AND field = ?");
        bind(&mut args, field.clone())?;
    }
    if let Some(actor) = &filter.actor {
        sql.push_str(" AND (actor = ? OR provider = ?)");
        bind(&mut args, actor.clone())?;
        bind(&mut args, actor.clone())?;
    }
    if let Some(since) = &filter.since {
        sql.push_str(" AND at >= ?");
        bind(&mut args, since.clone())?;
    }
    if let Some(before) = &filter.before {
        sql.push_str(" AND at < ?");
        bind(&mut args, before.clone())?;
    }
    if let Some((at, id)) = cursor.and_then(parse_cursor) {
        sql.push_str(" AND (at < ? OR (at = ? AND id > ?))");
        bind(&mut args, at.to_string())?;
        bind(&mut args, at.to_string())?;
        bind(&mut args, id.to_string())?;
    }

    // Newest first, and within one write in the order it was made: ids are
    // UUIDv7, ordered by their making within a process.
    sql.push_str(" ORDER BY at DESC, id ASC LIMIT ?");
    args.add(limit + 1).map_err(|e| anyhow::anyhow!("{e}"))?;

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;
    let mut entries = rows.iter().map(read).collect::<Result<Vec<_>>>()?;

    let more = entries.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    entries.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next = if more {
        entries.last().map(cursor_of)
    } else {
        None
    };
    Ok(Page { entries, next })
}

fn read(row: &sqlx::any::AnyRow) -> Result<Entry> {
    let json = |column: &str| -> Result<Option<Value>> {
        Ok(row
            .opt_text(column)?
            .and_then(|text| serde_json::from_str(&text).ok()))
    };
    let cause = row.text("cause")?;
    Ok(Entry {
        id: row.text("id")?,
        at: row.text("at")?,
        cause: Cause::parse(&cause)
            .ok_or_else(|| anyhow::anyhow!("{cause:?} is no cause of a change"))?,
        actor: row.opt_text("actor")?,
        provider: row.opt_text("provider")?,
        scope: row.text("scope")?,
        field: row.text("field")?,
        language: row.opt_text("language")?.filter(|l| !l.is_empty()),
        before: json("value_before")?,
        after: json("value_after")?,
        was_locked: row.flag("was_locked")?,
    })
}

/// What a work's journal names, for its filters to offer: every field, and
/// everybody — person or source — in it.
pub async fn named(db: &Db, media_id: &str) -> Result<(Vec<String>, Vec<String>)> {
    let fields = sqlx::query(
        db.sql("SELECT DISTINCT field FROM media_change WHERE media_id = ? ORDER BY field"),
    )
    .bind(media_id)
    .fetch_all(db.pool())
    .await?
    .iter()
    .map(|row| row.text("field"))
    .collect::<Result<Vec<_>, _>>()?;

    let who = sqlx::query(db.sql(
        "SELECT actor AS who FROM media_change WHERE media_id = ? AND actor IS NOT NULL
         UNION
         SELECT provider AS who FROM media_change WHERE media_id = ? AND provider IS NOT NULL
         ORDER BY who",
    ))
    .bind(media_id)
    .bind(media_id)
    .fetch_all(db.pool())
    .await?
    .iter()
    .map(|row| row.text("who"))
    .collect::<Result<Vec<_>, _>>()?;

    Ok((fields, who))
}

/// The last change of one value of a work — of `field` at `scope`, in
/// `language` (empty for every one) — whatever made it.
pub async fn latest(
    db: &Db,
    media_id: &str,
    scope: &str,
    field: &str,
    language: &str,
) -> Result<Option<Entry>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_change
          WHERE media_id = ? AND scope = ? AND field = ? AND language = ?
          ORDER BY at DESC, id DESC LIMIT 1"
    )))
    .bind(media_id)
    .bind(scope)
    .bind(field)
    .bind(language)
    .fetch_optional(db.pool())
    .await?;

    row.as_ref().map(read).transpose()
}

/// When a source last changed each value of a work, by scope and field:
/// what tells a lock set before it from one set after.
pub async fn last_by_a_source(
    db: &Db,
    media_id: &str,
) -> Result<std::collections::HashMap<(String, String), String>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT scope, field, MAX(at) AS at FROM media_change
          WHERE media_id = ? AND cause IN {BY_A_SOURCE}
          GROUP BY scope, field"
    )))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| Ok(((row.text("scope")?, row.text("field")?), row.text("at")?)))
        .collect()
}

/// The lock a work holds on one value — of `field` at `scope`, in `language`
/// (empty for every one) — as its value: `Some(None)` for a lock that
/// cleared the value, `None` for no lock at all. What a change written beside
/// a lock is told held before.
pub async fn lock_held(
    db: &Db,
    media_id: &str,
    scope: &str,
    field: &str,
    language: &str,
) -> Result<Option<Option<Value>>> {
    let row = sqlx::query(db.sql(
        "SELECT value FROM media_override
          WHERE media_id = ? AND scope = ? AND field = ? AND language = ?",
    ))
    .bind(media_id)
    .bind(scope)
    .bind(field)
    .bind(language)
    .fetch_optional(db.pool())
    .await?;

    Ok(match row {
        Some(row) => Some(
            row.opt_text("value")?
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .filter(|value| !value.is_null()),
        ),
        None => None,
    })
}

/// A work with a lock its sources moved under since it was set: the last
/// value a source gave the field came after the lock, and is another than
/// the lock's. What the dashboard counts and the catalogue's
/// `missing=divergingLock` lists, written against a row of `media_item`; the
/// words are the same on both engines. A lock in one language alone is no
/// such lock: what the sources give is in the language the work is stored
/// in.
///
/// The values are compared as the JSON they are kept in, which both write
/// the same way for the same value. The identity — adult, the slug, the
/// identifiers — is written to the row as it is locked, so no source is ever
/// read as changing it under its lock.
pub const DIVERGING_LOCK: &str = r"EXISTS (SELECT 1 FROM media_override o
                          JOIN media_change c
                            ON c.media_id = o.media_id AND c.scope = o.scope
                           AND c.field = o.field AND c.language = ''
                         WHERE o.media_id = media_item.id AND o.language = ''
                           AND c.cause IN ('refresh', 'sync', 'fallback')
                           AND c.at > o.updated_at
                           AND c.id = (SELECT l.id FROM media_change l
                                        WHERE l.media_id = o.media_id AND l.scope = o.scope
                                          AND l.field = o.field AND l.language = ''
                                          AND l.cause IN ('refresh', 'sync', 'fallback')
                                        ORDER BY l.at DESC, l.id DESC LIMIT 1)
                           AND COALESCE(c.value_after, 'null') <> COALESCE(o.value, 'null'))";

/// How many works hold a lock their sources moved under since it was set
/// ([`DIVERGING_LOCK`]), switched off or not, adult ones when the reader is
/// shown them: as the catalogue's list counts them.
pub async fn count_diverging(db: &Db, include_adult: bool) -> Result<i64> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT COUNT(*) AS n FROM media_item WHERE {DIVERGING_LOCK}{}",
        if include_adult {
            ""
        } else {
            " AND is_adult = 0"
        }
    )))
    .fetch_one(db.pool())
    .await?;

    Ok(row.big("n")?)
}

/// Delete changes older than `cutoff` (RFC 3339). Returns how many went.
pub async fn prune(db: &Db, cutoff: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM media_change WHERE at < ?"))
        .bind(cutoff)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::{repo, testing::db},
        domain::{MediaItem, MediaKind, fields::Scope},
    };
    use serde_json::json;

    async fn work(db: &Db, title: &str) -> String {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        repo::item::upsert(
            db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item.id
    }

    fn change(cause: Cause, scope: &str, field: &str, before: Value, after: Value) -> NewChange {
        NewChange {
            cause,
            scope: scope.into(),
            field: field.into(),
            language: String::new(),
            before: (!before.is_null()).then_some(before),
            after: (!after.is_null()).then_some(after),
            provider: cause.is_a_source().then(|| "tmdb".to_string()),
            was_locked: false,
        }
    }

    #[tokio::test]
    async fn a_write_is_read_back_whole_and_nothing_is_written_for_no_change() {
        let db = db().await;
        let id = work(&db, "Journal").await;

        record(&db, &id, None, &[]).await.unwrap();
        let empty = list(&db, &id, &Filter::default(), None, 50).await.unwrap();
        assert!(empty.entries.is_empty() && empty.next.is_none());

        record(
            &db,
            &id,
            Some("admin"),
            &[
                change(
                    Cause::Lock,
                    "item",
                    "overview",
                    json!("Old."),
                    json!("Mine."),
                ),
                NewChange {
                    language: "fra".into(),
                    was_locked: true,
                    ..change(Cause::Lock, "item", "title", json!("Avant"), json!("Après"))
                },
                change(
                    Cause::Unlock,
                    "episode:1x2",
                    "title",
                    json!("A"),
                    Value::Null,
                ),
            ],
        )
        .await
        .unwrap();

        let page = list(&db, &id, &Filter::default(), None, 50).await.unwrap();
        // One write, in the order it was made.
        let read: Vec<(&str, &str, Option<&str>)> = page
            .entries
            .iter()
            .map(|e| (e.scope.as_str(), e.field.as_str(), e.language.as_deref()))
            .collect();
        assert_eq!(
            read,
            [
                ("item", "overview", None),
                ("item", "title", Some("fra")),
                ("episode:1x2", "title", None),
            ]
        );
        let title = &page.entries[1];
        assert_eq!(title.actor.as_deref(), Some("admin"));
        assert_eq!(title.provider, None);
        assert_eq!(title.before, Some(json!("Avant")));
        assert_eq!(title.after, Some(json!("Après")));
        assert!(title.was_locked);
        assert_eq!(page.entries[2].after, None);
        assert_eq!(page.entries[2].cause, Cause::Unlock);
    }

    #[tokio::test]
    async fn the_pages_walk_the_journal_newest_first_and_the_filters_narrow_it() {
        let db = db().await;
        let id = work(&db, "Pages").await;
        let other = work(&db, "Another").await;

        for (n, cause) in [Cause::Refresh, Cause::Lock, Cause::Refresh, Cause::Sync]
            .into_iter()
            .enumerate()
        {
            let field = if n % 2 == 0 { "overview" } else { "genres" };
            record(
                &db,
                &id,
                (!cause.is_a_source()).then_some("admin"),
                &[change(cause, "item", field, json!(n), json!(n + 1))],
            )
            .await
            .unwrap();
            // Written a millisecond apart at the least, as two writes are.
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        record(
            &db,
            &other,
            None,
            &[change(
                Cause::Refresh,
                "item",
                "overview",
                json!(1),
                json!(2),
            )],
        )
        .await
        .unwrap();

        let first = list(&db, &id, &Filter::default(), None, 3).await.unwrap();
        assert_eq!(first.entries.len(), 3);
        assert_eq!(first.entries[0].cause, Cause::Sync, "the newest first");
        let rest = list(&db, &id, &Filter::default(), first.next.as_deref(), 3)
            .await
            .unwrap();
        assert_eq!(rest.entries.len(), 1);
        assert_eq!(rest.entries[0].before, Some(json!(0)), "the oldest last");
        assert!(rest.next.is_none());

        let overview = Filter {
            field: Some("overview".into()),
            ..Filter::default()
        };
        assert_eq!(
            list(&db, &id, &overview, None, 50)
                .await
                .unwrap()
                .entries
                .len(),
            2
        );
        // A person or a source, by either name.
        let by = |who: &str| Filter {
            actor: Some(who.into()),
            ..Filter::default()
        };
        assert_eq!(
            list(&db, &id, &by("admin"), None, 50)
                .await
                .unwrap()
                .entries
                .len(),
            1
        );
        assert_eq!(
            list(&db, &id, &by("tmdb"), None, 50)
                .await
                .unwrap()
                .entries
                .len(),
            3
        );

        let all = list(&db, &id, &Filter::default(), None, 50).await.unwrap();
        let middle = &all.entries[1].at;
        let since = Filter {
            since: Some(middle.clone()),
            ..Filter::default()
        };
        assert_eq!(
            list(&db, &id, &since, None, 50)
                .await
                .unwrap()
                .entries
                .len(),
            2
        );
        let before = Filter {
            before: Some(middle.clone()),
            ..Filter::default()
        };
        assert_eq!(
            list(&db, &id, &before, None, 50)
                .await
                .unwrap()
                .entries
                .len(),
            2
        );

        let (fields, who) = named(&db, &id).await.unwrap();
        assert_eq!(fields, ["genres", "overview"]);
        assert_eq!(who, ["admin", "tmdb"]);
    }

    #[tokio::test]
    async fn the_last_change_of_a_value_is_found_whatever_made_it() {
        let db = db().await;
        let id = work(&db, "Latest").await;
        assert!(
            latest(&db, &id, "item", "genres", "")
                .await
                .unwrap()
                .is_none()
        );

        record(
            &db,
            &id,
            Some("admin"),
            &[change(
                Cause::Lock,
                "item",
                "genres",
                json!(["A"]),
                json!(["B"]),
            )],
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        record(
            &db,
            &id,
            None,
            &[change(
                Cause::Refresh,
                "item",
                "genres",
                json!(["A"]),
                json!(["C"]),
            )],
        )
        .await
        .unwrap();

        let last = latest(&db, &id, "item", "genres", "")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last.cause, Cause::Refresh);
        let by_source = last_by_a_source(&db, &id).await.unwrap();
        assert_eq!(
            by_source.get(&("item".to_string(), "genres".to_string())),
            Some(&last.at)
        );
    }

    #[tokio::test]
    async fn a_lock_diverges_once_its_source_moved_under_it_to_another_value() {
        let db = db().await;
        let id = work(&db, "Diverging").await;
        let lock = |value: Value| {
            let db = db.clone();
            let id = id.clone();
            async move {
                repo::override_field::set(&db, &id, Scope::Item, "status", Some(&value), None)
                    .await
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
            }
        };
        let moved = |to: &str| {
            let db = db.clone();
            let id = id.clone();
            let to = to.to_string();
            async move {
                record(
                    &db,
                    &id,
                    None,
                    &[NewChange {
                        was_locked: true,
                        ..change(Cause::Refresh, "item", "status", json!("ended"), json!(to))
                    }],
                )
                .await
                .unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
            }
        };

        // Locked, and the source changed before the lock: nothing to say.
        moved("returning").await;
        lock(json!("canceled")).await;
        assert_eq!(count_diverging(&db, false).await.unwrap(), 0);
        assert_eq!(
            lock_held(&db, &id, "item", "status", "").await.unwrap(),
            Some(Some(json!("canceled")))
        );

        // The source moved under it.
        moved("ended").await;
        assert_eq!(count_diverging(&db, false).await.unwrap(), 1);
        // Adult or not, as the reader is shown them.
        sqlx::query(db.sql("UPDATE media_item SET is_adult = 1 WHERE id = ?"))
            .bind(&id)
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(count_diverging(&db, false).await.unwrap(), 0);
        assert_eq!(count_diverging(&db, true).await.unwrap(), 1);

        // And came round to the lock's value: nothing to say again.
        moved("canceled").await;
        assert_eq!(count_diverging(&db, true).await.unwrap(), 0);

        // Moved again, then the lock set anew, kept as it was: seen.
        moved("ended").await;
        assert_eq!(count_diverging(&db, true).await.unwrap(), 1);
        lock(json!("canceled")).await;
        assert_eq!(count_diverging(&db, true).await.unwrap(), 0);

        // Lifted, it is no lock at all.
        moved("returning").await;
        assert_eq!(count_diverging(&db, true).await.unwrap(), 1);
        repo::override_field::unset(&db, &id, Scope::Item, "status")
            .await
            .unwrap();
        assert_eq!(count_diverging(&db, true).await.unwrap(), 0);
        assert_eq!(
            lock_held(&db, &id, "item", "status", "").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn the_journal_is_pruned_by_age_and_goes_with_its_work() {
        let db = db().await;
        let id = work(&db, "Pruned").await;
        record(
            &db,
            &id,
            None,
            &[change(
                Cause::Refresh,
                "item",
                "overview",
                json!("A"),
                json!("B"),
            )],
        )
        .await
        .unwrap();

        assert_eq!(prune(&db, "2000-01-01T00:00:00.000Z").await.unwrap(), 0);
        assert_eq!(prune(&db, "2999-01-01T00:00:00.000Z").await.unwrap(), 1);

        record(
            &db,
            &id,
            None,
            &[change(
                Cause::Refresh,
                "item",
                "overview",
                json!("B"),
                json!("C"),
            )],
        )
        .await
        .unwrap();
        repo::item::delete(&db, &id).await.unwrap();
        let left = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_change"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(left.big("n").unwrap(), 0);
    }

    #[test]
    fn every_cause_is_named_once_and_read_back() {
        for cause in Cause::ALL {
            assert_eq!(Cause::parse(cause.as_str()), Some(*cause));
            assert_eq!(
                serde_json::to_value(cause).unwrap(),
                json!(cause.as_str()),
                "the API says it as the table keeps it"
            );
        }
        assert_eq!(Cause::parse("rewrite"), None);
        let sources: Vec<&str> = Cause::ALL
            .iter()
            .filter(|c| c.is_a_source())
            .map(|c| c.as_str())
            .collect();
        assert_eq!(
            format!("('{}')", sources.join("', '")),
            BY_A_SOURCE,
            "the SQL names the sources' causes as the enum does"
        );
    }
}
