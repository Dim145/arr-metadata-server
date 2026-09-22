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

use anyhow::{Context, Result, bail};
use sqlx::{Column, Row, ValueRef};

use crate::db::Db;

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
    "media_credit",
    "media_alternative_title",
    "media_rating",
    "media_translation",
    "media_episode_translation",
    "media_language_fetch",
    "api_client",
    "admin_user",
    "admin_session",
    "job_run",
    "audit_log",
    "network_rule",
    "network_caller",
    "setting",
    "search_cache",
];

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

    if !force {
        for table in TABLES {
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

    // One transaction for the whole copy. Chunk by chunk and table by table, a
    // failure part of the way through left the target holding everything before
    // it — which the emptiness check above then refuses to let you retry, while
    // `--force` would re-insert the tables that did land and duplicate every row
    // in the ones with no key to conflict on. All of it, or none of it.
    let mut tx = target
        .pool()
        .begin()
        .await
        .context("failed to open the transfer transaction")?;

    for table in TABLES {
        let rows = copy_table(source, target, &mut tx, table)
            .await
            .with_context(|| format!("while copying {table}"))?;

        tracing::info!(table, rows, "copied");
        report.copied.push(((*table).to_string(), rows));
    }

    tx.commit().await.context("failed to commit the transfer")?;

    Ok(report)
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

async fn copy_table(
    source: &Db,
    target: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    table: &str,
) -> Result<u64> {
    let rows = sqlx::query(source.sql(&format!("SELECT * FROM {table}")))
        .fetch_all(source.pool())
        .await?;

    if rows.is_empty() {
        return Ok(0);
    }

    let columns: Vec<String> = rows[0]
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();

    let column_list = columns.join(", ");
    let mut written = 0u64;

    for chunk in rows.chunks(CHUNK) {
        // Read the chunk first: the SQL depends on which values are null.
        let decoded: Vec<Vec<Value>> = chunk
            .iter()
            .map(|row| (0..columns.len()).map(|i| read(row, i)).collect())
            .collect::<Result<_>>()?;

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

        for row in &decoded {
            for value in row {
                query = match value {
                    Value::Null => continue,
                    Value::Int(v) => query.bind(*v),
                    Value::Real(v) => query.bind(*v),
                    Value::Text(v) => query.bind(v.clone()),
                };
            }
        }

        written += query.execute(&mut **tx).await?.rows_affected();
    }

    Ok(written)
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
                let Some(rest) = lowered.strip_prefix("create table ") else {
                    continue;
                };
                let rest = rest.strip_prefix("if not exists ").unwrap_or(rest);

                if let Some(name) = rest.split([' ', '(']).next().filter(|n| !n.is_empty()) {
                    in_schema.push(name.to_string());
                }
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
