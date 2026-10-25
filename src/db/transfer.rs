//! Copying a database to another engine.
//!
//! Switching `AMS_DATABASE_URL` from SQLite to PostgreSQL gives you an empty
//! PostgreSQL, not your catalogue. This moves the rows.
//!
//! The copy is generic: every table is read column by column and written back
//! with the same names, so a migration that adds a column does not also require
//! editing this file. Values are decoded by trying the types the schema uses —
//! there are only three — rather than consulting the engine's own type system,
//! which `Any` does not expose.
//!
//! It is also how a backup is made of a PostgreSQL — copied into an SQLite
//! file, the archive's one engine-neutral payload — and how any archive is
//! restored: copied out of that file into the database it is restored to,
//! whose rows are emptied first in the same transaction (see
//! [`Options::replace`]).

use anyhow::{Context, Result, bail};
use futures::TryStreamExt as _;
use sqlx::{Column, Row, ValueRef};

use crate::db::{Db, Dialect};

/// Every table, parents before children.
///
/// Foreign keys are enforced on SQLite and on PostgreSQL, so the order is not
/// cosmetic. `_sqlx_migrations` is deliberately absent: the target applies its
/// own, and copying another engine's checksums would break it.
///
/// Every other table the migrations create has to be here. Four were not, and
/// the copy still reported success: the allowlist arrived empty, which either
/// refused every Sonarr and Radarr call or — worse — was re-seeded from the
/// `AMS_ALLOWLIST` variable the operator had long since stopped maintaining,
/// with fresh rule ids that orphaned every peer-scoped setting hanging off
/// them. [`tests::every_table_in_the_schema_is_copied`] is what stops it
/// happening again.
const TABLES: &[&str] = &[
    "media_item",
    "media_external_id",
    "media_provider_snapshot",
    "media_override",
    "media_season",
    "media_episode",
    "media_image",
    // The people the credits are of, before the credits.
    "person",
    "person_alias",
    "media_credit",
    "media_relation",
    // An order before its places; who numbers a series beside them.
    "media_order",
    "media_order_episode",
    "media_numbering",
    "media_alternative_title",
    "media_rating",
    "media_translation",
    "media_release",
    "media_episode_translation",
    "media_language_fetch",
    // What changed in each work, after the works.
    "media_change",
    // What a merged work was known by, and the pairs that look like one work.
    "media_alias",
    "media_duplicate",
    // The search index, after the works it names: carried rather than built
    // again, its rows numbered by what they are and not by a counter.
    "search_doc",
    // People before their keys: a key names its owner.
    "admin_user",
    "admin_session",
    "api_client",
    // What each person keeps of their own way of working, after them.
    "account_preference",
    // After the accounts, whose administrators made them.
    "user_invitation",
    "job_run",
    // What each run did to each work, after the runs.
    "job_entry",
    "audit_log",
    // What was told to the operator's Sonarr and Radarr lately.
    "arr_push",
    // Where what happens is told, then what is owed and was told to each:
    // what still waits is sent from the new database.
    "webhook_destination",
    "webhook_delivery",
    // The media kept: its rows only — the files stay in the store they
    // were put in, which both databases can be pointed at.
    "media_asset",
    "media_variant",
    "network_rule",
    "network_caller",
    "setting",
    // What every instance holds the same of: the cookie key, the authority
    // the clients trust. Carried over, so the clients need not trust anew.
    "keystore",
    "search_cache",
    "anime_mapping",
    "imdb_rating",
    // IMDb's titles by region and its episodes, for the works held here.
    "imdb_title",
    "imdb_episode",
    "data_import",
    "curated_list",
    "curated_list_item",
    // The genres, their names after them; the programmer's notes.
    "genre",
    "genre_name",
    "editorial",
    // When the tasks run, where somebody said; what is imported on a
    // schedule, after the selections it fills.
    "task_schedule",
    "import_rule",
    // The archives made of the database: carried to another engine like the
    // rest, and kept as they are by a restore (see `backup::restore`).
    "backup",
];

/// The tables a copy carries, parents before children.
pub fn tables() -> &'static [&'static str] {
    TABLES
}

/// Rows carried per statement. Large enough to be quick, small enough to stay
/// inside every engine's bind-parameter ceiling for a wide table.
const CHUNK: usize = 200;

#[derive(Debug)]
pub struct Report {
    pub copied: Vec<(String, u64)>,
}

impl Report {
    pub fn total(&self) -> u64 {
        self.copied.iter().map(|(_, n)| n).sum()
    }
}

/// What is told of each table as its copy starts: its name, how many come
/// before it, and how many there are.
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Send + Sync);

/// How a copy goes, besides from where to where.
#[derive(Default)]
pub struct Options<'a> {
    /// Add to a target that already holds rows, rather than refusing it.
    pub force: bool,
    /// Empty the target's tables first, in the transaction the copy is made
    /// in: what a restore does, once it has made its safety copy. A copy that
    /// fails part of the way leaves the target as it was, emptied rows and
    /// all.
    pub replace: bool,
    /// Tables neither emptied nor copied: what the target keeps of its own.
    pub keep: &'a [&'a str],
    /// Told of each table as its copy starts.
    pub progress: Option<Progress<'a>>,
    /// Copy the tables the source holds — and the target too — at whatever
    /// version of the schema the two stand, rather than this binary's: an
    /// archive made before an upgrade's migrations holds the tables they
    /// will reshape or drop. Into SQLite, whose foreign keys are then
    /// checked once the copy is whole: where an older table goes among the
    /// others is not known here.
    pub as_found: bool,
}

/// Copy everything from `source` into `target`.
///
/// The target must already be migrated and must be empty, unless `force` is set.
/// Overwriting is not offered: the two databases have no shared notion of which
/// row is newer, so a merge would be guesswork.
pub async fn run(source: &Db, target: &Db, force: bool) -> Result<Report> {
    if source.dialect() == target.dialect() {
        tracing::warn!(
            "source and target use the same engine; this will still copy, but a file \
             copy is usually what you want"
        );
    }
    run_with(
        source,
        target,
        Options {
            force,
            ..Options::default()
        },
    )
    .await
}

/// Copy everything from `source` into `target`, as `options` say: the
/// backups' own way in, which copies SQLite into SQLite on purpose.
pub async fn run_with(source: &Db, target: &Db, options: Options<'_>) -> Result<Report> {
    let listed: Vec<String> = if options.as_found {
        tables_as_found(source, target).await?
    } else {
        TABLES.iter().map(|table| (*table).to_string()).collect()
    };
    let carried: Vec<&str> = listed
        .iter()
        .map(String::as_str)
        .filter(|table| !options.keep.contains(table))
        .collect();

    if !options.force && !options.replace {
        for table in &carried {
            let existing = count(target, table).await?;
            if existing > 0 {
                bail!(
                    "the target already holds {existing} rows in {table}; \
                     transfer into an empty database, or pass --force to add to it"
                );
            }
        }
    }

    let mut report = Report { copied: Vec::new() };

    // The source read as of one moment, every table of it: read table by
    // table through the pool, a server writing meanwhile — the one a backup
    // is made of while it runs — could hand over an episode whose season was
    // read before it existed, and the target refuses the orphan. A read
    // transaction on SQLite holds its snapshot from its first read; on
    // PostgreSQL it has to be asked for.
    let mut snapshot = match source.dialect() {
        Dialect::Sqlite => source.pool().begin().await,
        Dialect::Postgres => {
            source
                .pool()
                .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .await
        }
    }
    .context("failed to open a read of the source")?;

    // One transaction for the whole copy. Chunk by chunk and table by table, a
    // failure part of the way through left the target holding everything before
    // it — which the emptiness check above then refuses to let you retry, while
    // `--force` would re-insert the tables that did land and duplicate every row
    // in the ones with no key to conflict on. All of it, or none of it.
    let mut tx = target
        .begin_write()
        .await
        .context("failed to open the transfer transaction")?;

    if options.replace {
        empty(target, &mut tx, &carried).await?;
    }
    if options.as_found && target.dialect() == Dialect::Sqlite {
        sqlx::query(target.sql("PRAGMA defer_foreign_keys = ON"))
            .execute(&mut *tx)
            .await
            .context("failed to defer the target's foreign keys")?;
    }

    for (index, table) in carried.iter().enumerate() {
        if let Some(progress) = options.progress {
            progress(table, index, carried.len());
        }
        let rows = copy_table(source, &mut snapshot, target, &mut tx, table)
            .await
            .with_context(|| format!("while copying {table}"))?;

        tracing::info!(table, rows, "copied");
        report.copied.push(((*table).to_string(), rows));
    }

    tx.commit().await.context("failed to commit the transfer")?;
    // Nothing was written through it; ended rather than dropped, so the
    // connection goes back to the pool at once.
    snapshot
        .commit()
        .await
        .context("failed to end the read of the source")?;

    Ok(report)
}

/// Empty the tables a restore replaces, children before their parents, in
/// the copy's own transaction. Another instance on the same PostgreSQL may
/// go on reading the rows as they were until the copy commits, and waits to
/// write: nothing it writes meanwhile can land between the emptying and the
/// copy. SQLite's write transaction keeps every other writer out already.
async fn empty(
    target: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    tables: &[&str],
) -> Result<()> {
    if target.dialect() == Dialect::Postgres {
        let list = tables.join(", ");
        sqlx::query(target.sql(&format!("LOCK TABLE {list} IN EXCLUSIVE MODE")))
            .execute(&mut **tx)
            .await
            .context("failed to hold the tables for the restore")?;
    }
    for table in tables.iter().rev() {
        sqlx::query(target.sql(&format!("DELETE FROM {table}")))
            .execute(&mut **tx)
            .await
            .with_context(|| format!("while emptying {table}"))?;
    }
    Ok(())
}

/// The tables `source` holds that `target` holds too: this binary's first,
/// in their order, then the others by name. Neither engine's own tables, the
/// migrations' record, nor SQLite's search index and the tables behind it,
/// which its triggers fill as the rows arrive.
async fn tables_as_found(source: &Db, target: &Db) -> Result<Vec<String>> {
    let in_source = tables_of(source).await?;
    let in_target = tables_of(target).await?;
    let mut both: Vec<String> = in_source
        .into_iter()
        .filter(|table| in_target.contains(table))
        .collect();
    both.sort_by_key(|table| {
        (
            TABLES
                .iter()
                .position(|known| known == table)
                .unwrap_or(usize::MAX),
            table.clone(),
        )
    });
    Ok(both)
}

/// The tables of a database, as its engine lists them.
async fn tables_of(db: &Db) -> Result<Vec<String>> {
    let rows = match db.dialect() {
        Dialect::Postgres => {
            sqlx::query(db.sql(
                "SELECT CAST(table_name AS TEXT) AS name FROM information_schema.tables \
                 WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'",
            ))
            .fetch_all(db.pool())
            .await?
        }
        Dialect::Sqlite => {
            sqlx::query(db.sql(
                "SELECT name, CASE WHEN sql LIKE 'CREATE VIRTUAL TABLE%' THEN 1 ELSE 0 END \
                 AS virtual FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            ))
            .fetch_all(db.pool())
            .await?
        }
    };
    let mut names = Vec::new();
    let mut virtual_tables = Vec::new();
    for row in &rows {
        let name: String = row.try_get("name")?;
        let is_virtual = db.dialect() == Dialect::Sqlite && row.try_get::<i64, _>("virtual")? != 0;
        if is_virtual {
            virtual_tables.push(name);
        } else {
            names.push(name);
        }
    }
    Ok(names
        .into_iter()
        .filter(|name| name != "_sqlx_migrations")
        .filter(|name| {
            !virtual_tables
                .iter()
                .any(|table| name.starts_with(&format!("{table}_")))
        })
        .collect())
}

async fn count(db: &Db, table: &str) -> Result<i64> {
    let row = sqlx::query(db.sql(&format!("SELECT COUNT(*) AS n FROM {table}")))
        .fetch_one(db.pool())
        .await?;

    Ok(row.try_get::<i64, _>("n")?)
}

/// One column's value, in the only three shapes this schema uses.
enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

/// Read a column without asking the engine what type it is.
///
/// `Any` has no runtime type reflection, so this tries each type the schema can
/// hold. The order matters: an integer decodes as a float too, so integers are
/// tried first and the value keeps its kind.
fn read(row: &sqlx::any::AnyRow, index: usize) -> Result<Value> {
    if row.try_get_raw(index)?.is_null() {
        return Ok(Value::Null);
    }

    if let Ok(v) = row.try_get::<i64, _>(index) {
        return Ok(Value::Int(v));
    }
    if let Ok(v) = row.try_get::<f64, _>(index) {
        return Ok(Value::Real(v));
    }
    if let Ok(v) = row.try_get::<String, _>(index) {
        return Ok(Value::Text(v));
    }

    let column = row.column(index).name().to_string();
    bail!("column {column} holds a type this transfer does not handle")
}

/// Copy one table, a chunk at a time as its rows are read: a table of a
/// hundred thousand episodes is never held whole.
async fn copy_table(
    source: &Db,
    snapshot: &mut sqlx::Transaction<'_, sqlx::Any>,
    target: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    table: &str,
) -> Result<u64> {
    let mut rows =
        sqlx::query(source.sql(&format!("SELECT * FROM {table}"))).fetch(&mut **snapshot);

    let mut columns: Vec<String> = Vec::new();
    let mut chunk: Vec<Vec<Value>> = Vec::with_capacity(CHUNK);
    let mut written = 0u64;

    while let Some(row) = rows.try_next().await? {
        if columns.is_empty() {
            columns = row.columns().iter().map(|c| c.name().to_string()).collect();
        }
        chunk.push(
            (0..columns.len())
                .map(|i| read(&row, i))
                .collect::<Result<_>>()?,
        );
        if chunk.len() == CHUNK {
            written += insert(target, tx, table, &columns, &chunk).await?;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        written += insert(target, tx, table, &columns, &chunk).await?;
    }

    Ok(written)
}

/// Write one chunk of a table's rows, as they were read.
async fn insert(
    target: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    table: &str,
    columns: &[String],
    decoded: &[Vec<Value>],
) -> Result<u64> {
    let column_list = columns.join(", ");

    // A NULL goes in as a literal rather than a bound parameter. Binding
    // `None::<String>` sends a typed null, and PostgreSQL rejects a text
    // null for an integer column — "column is of type integer but
    // expression is of type text". A bare NULL takes the column's own type.
    let values = decoded
        .iter()
        .map(|row| {
            let cells: Vec<&str> = row
                .iter()
                .map(|v| {
                    if matches!(v, Value::Null) {
                        "NULL"
                    } else {
                        "?"
                    }
                })
                .collect();
            format!("({})", cells.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!("INSERT INTO {table} ({column_list}) VALUES {values}");
    let mut query = sqlx::query(target.sql(&sql));

    for row in decoded {
        for value in row {
            query = match value {
                Value::Null => continue,
                Value::Int(v) => query.bind(*v),
                Value::Real(v) => query.bind(*v),
                Value::Text(v) => query.bind(v.clone()),
            };
        }
    }

    Ok(query.execute(&mut **tx).await?.rows_affected())
}

#[cfg(test)]
mod tests {
    /// Every table the schema creates is one the transfer carries.
    ///
    /// Read off the migrations rather than listed a second time here, because a
    /// second list is a second thing to forget. A table added in a later
    /// migration and not added to `TABLES` is silently skipped by a copy that
    /// still reports success — which is how the allowlist came to be lost.
    #[test]
    fn every_table_in_the_schema_is_copied() {
        let mut in_schema: Vec<String> = Vec::new();

        let mut files: Vec<_> = std::fs::read_dir("migrations/sqlite")
            .expect("the migrations are beside the crate")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        files.sort();

        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a readable migration");

            for line in sql.lines() {
                let lowered = line.trim().to_ascii_lowercase();
                let first_word = |text: &str| {
                    text.split([' ', '(', ';'])
                        .next()
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                };
                // A table built again around a new check is made under one
                // name and renamed: the name it was made under is gone, and
                // the one it took is there.
                if let Some(rest) = lowered.strip_prefix("alter table ")
                    && let Some((old, new)) = rest.split_once(" rename to ")
                {
                    in_schema.retain(|name| name != old.trim());
                    in_schema.extend(first_word(new.trim()));
                    continue;
                }
                // A table dropped is gone too: the one a rebuild replaces,
                // and the one made for the length of a migration — 0031
                // holds the overrides aside while it builds their table
                // again — which no transfer will ever meet.
                if let Some(rest) = lowered.strip_prefix("drop table ") {
                    let rest = rest.strip_prefix("if exists ").unwrap_or(rest);
                    if let Some(dropped) = first_word(rest) {
                        in_schema.retain(|name| name != &dropped);
                    }
                    continue;
                }
                let Some(rest) = lowered
                    .strip_prefix("create table ")
                    .or_else(|| lowered.strip_prefix("create temp table "))
                    .or_else(|| lowered.strip_prefix("create temporary table "))
                else {
                    continue;
                };
                let rest = rest.strip_prefix("if not exists ").unwrap_or(rest);
                in_schema.extend(first_word(rest));
            }
        }

        assert!(
            in_schema.len() > 10,
            "the migrations were not read; found {in_schema:?}"
        );

        let missing: Vec<&String> = in_schema
            .iter()
            .filter(|name| name.as_str() != "_sqlx_migrations")
            .filter(|name| !TABLES.contains(&name.as_str()))
            .collect();

        assert!(
            missing.is_empty(),
            "these tables would be silently dropped by a transfer: {missing:?}"
        );
    }

    use super::*;
    use crate::{
        config,
        db::repo::item::{self, ItemWrite},
        domain::{ExternalIds, MediaItem, MediaKind},
    };

    async fn sqlite() -> Db {
        let db = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        db.migrate().await.unwrap();
        db
    }

    fn work(title: &str, tmdb: i64) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = title.into();
        item.slug = format!("{}-2026", title.to_lowercase().replace(' ', "-"));
        item.year = Some(2026);
        item.popularity = Some(3.25);
        item.overview = Some("Has an ampersand & a quote \"like this\".".into());
        item.external_ids = ExternalIds {
            tmdb: Some(tmdb),
            ..Default::default()
        };
        item
    }

    #[tokio::test]
    async fn a_populated_database_copies_into_an_empty_one() {
        let source = sqlite().await;
        let target = sqlite().await;

        for (i, title) in ["First Film", "Second Film"].iter().enumerate() {
            item::upsert(
                &source,
                ItemWrite {
                    item: &work(title, 100 + i as i64),
                    replace_children: false,
                },
            )
            .await
            .unwrap();
        }

        let report = run(&source, &target, false).await.unwrap();

        assert!(report.total() >= 4, "two works and their external ids");
        assert_eq!(count(&target, "media_item").await.unwrap(), 2);
        assert_eq!(count(&target, "media_external_id").await.unwrap(), 2);

        // Values must survive, not merely rows.
        let items = item::search(
            &target,
            &item::Query {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let first = items.iter().find(|i| i.title == "First Film").unwrap();
        assert_eq!(first.year, Some(2026));
        assert_eq!(first.popularity, Some(3.25));
        assert_eq!(
            first.overview.as_deref(),
            Some("Has an ampersand & a quote \"like this\".")
        );
        assert_eq!(first.external_ids.tmdb, Some(100));
    }

    #[tokio::test]
    async fn a_target_that_already_holds_rows_is_refused() {
        let source = sqlite().await;
        let target = sqlite().await;

        item::upsert(
            &source,
            ItemWrite {
                item: &work("A", 1),
                replace_children: false,
            },
        )
        .await
        .unwrap();
        item::upsert(
            &target,
            ItemWrite {
                item: &work("B", 2),
                replace_children: false,
            },
        )
        .await
        .unwrap();

        let error = run(&source, &target, false).await.unwrap_err().to_string();
        assert!(error.contains("already holds"), "{error}");
        assert!(
            error.contains("--force"),
            "the message should say how to proceed"
        );

        // Nothing was written.
        assert_eq!(count(&target, "media_item").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn force_adds_to_what_is_already_there() {
        let source = sqlite().await;
        let target = sqlite().await;

        item::upsert(
            &source,
            ItemWrite {
                item: &work("A", 1),
                replace_children: false,
            },
        )
        .await
        .unwrap();
        item::upsert(
            &target,
            ItemWrite {
                item: &work("B", 2),
                replace_children: false,
            },
        )
        .await
        .unwrap();

        run(&source, &target, true).await.unwrap();
        assert_eq!(count(&target, "media_item").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn an_empty_source_copies_nothing_and_does_not_fail() {
        let source = sqlite().await;
        let target = sqlite().await;

        let report = run(&source, &target, false).await.unwrap();
        assert_eq!(report.total(), 0);
        assert_eq!(report.copied.len(), TABLES.len());
    }
}
