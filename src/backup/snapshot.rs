//! The database as one SQLite file: what an archive carries, whatever engine
//! it was read from, and what a restore reads back.
//!
//! From SQLite, `VACUUM INTO` — a consistent copy, made while the server
//! goes on writing, compacted as it is written, with the path bound as a
//! parameter rather than written into the statement. From PostgreSQL, the
//! `transfer` the command line has always had, into an SQLite file migrated
//! as far as the database is and no further, every table it holds copied:
//! an archive made before an upgrade's migrations keeps the tables they
//! reshape, as they were. Either way the copy is checked whole before it is
//! used: a file cut short by a full disk is found here, not on the day it is
//! restored.

use std::path::Path;

use anyhow::{Context, Result, bail};
use sqlx::{
    ConnectOptions as _, Row as _,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};

use crate::{
    config,
    db::{self, Db, Dialect, transfer},
};

/// What a copy was taken from.
pub struct Taken {
    pub engine: Dialect,
    /// The last migration the database had.
    pub migration: i64,
}

/// Copy `db` into a new SQLite file at `into`, which must not exist.
pub async fn take(db: &Db, into: &Path) -> Result<Taken> {
    let migration = db.last_migration().await?;
    if db.dialect() == Dialect::Sqlite {
        // The path is bound, never written into the statement: a quote in a
        // directory's name cannot end the string it is in.
        sqlx::query(db.sql("VACUUM INTO ?"))
            .bind(into.to_string_lossy().to_string())
            .execute(db.pool())
            .await
            .context("could not copy the database")?;
    }
    // A PostgreSQL is copied row by row. So is an SQLite that `VACUUM INTO`
    // wrote nothing of: one in memory, which sqlx opens in a shared cache
    // the statement does not copy out of.
    if !into.exists() {
        let target = open(into, true).await?;
        let copied = async {
            target.migrate_to(migration).await?;
            transfer::run_with(
                db,
                &target,
                transfer::Options {
                    as_found: true,
                    ..transfer::Options::default()
                },
            )
            .await
        }
        .await;
        target.close().await;
        copied.context("could not copy the database into the archive's SQLite file")?;
    }
    check(into).await?;
    Ok(Taken {
        engine: db.dialect(),
        migration,
    })
}

/// An SQLite file as a database to copy from or into.
pub async fn open(path: &Path, create: bool) -> Result<Db> {
    Db::connect(&config::Database {
        url: db::sqlite_url(path, create),
        // The copy's write and the check that it may begin; nothing else
        // uses it.
        max_connections: 2,
        acquire_timeout: std::time::Duration::from_secs(60),
    })
    .await
    .with_context(|| format!("could not open {}", path.display()))
}

/// Whether the file is a whole SQLite database, and made to stand on its
/// own: its journal kept in the file, not beside it.
pub async fn check(path: &Path) -> Result<()> {
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Delete)
        .connect()
        .await
        .with_context(|| format!("could not open the copy {}", path.display()))?;
    let found: Vec<String> = sqlx::query("PRAGMA integrity_check")
        .fetch_all(&mut conn)
        .await
        .context("could not check the copy")?
        .iter()
        .map(|row| row.try_get::<String, _>(0))
        .collect::<Result<_, _>>()?;
    drop(conn);
    if found != ["ok"] {
        bail!(
            "the copy of the database is not whole: {}",
            found.join("; ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::scratch;
    use crate::db::repo::item::{self, ItemWrite};
    use crate::domain::{MediaItem, MediaKind};

    async fn stored(db: &Db, title: &str) {
        let mut work = MediaItem::empty(MediaKind::Movie);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, Some(2026));
        item::upsert(
            db,
            ItemWrite {
                item: &work,
                replace_children: false,
            },
        )
        .await
        .unwrap();
    }

    async fn titles(db: &Db) -> Vec<String> {
        let mut titles: Vec<String> = sqlx::query("SELECT title FROM media_item")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .iter()
            .map(|row| row.try_get::<String, _>("title").unwrap())
            .collect();
        titles.sort();
        titles
    }

    #[tokio::test]
    async fn an_sqlite_database_is_copied_whole_into_a_file_of_its_own() {
        let dir = scratch();
        let source = open(&dir.join("live.db"), true).await.unwrap();
        source.migrate().await.unwrap();
        stored(&source, "A Film").await;
        stored(&source, "Another, with a 'quote'").await;

        let into = dir.join("with a space").join("copy.sqlite");
        std::fs::create_dir_all(into.parent().unwrap()).unwrap();
        let taken = take(&source, &into).await.unwrap();
        assert_eq!(taken.engine, Dialect::Sqlite);
        assert_eq!(taken.migration, db::latest_migration());
        // Its own journal: nothing beside it to carry along.
        assert!(!into.with_file_name("copy.sqlite-wal").exists());

        let copy = open(&into, false).await.unwrap();
        assert_eq!(titles(&copy).await, ["A Film", "Another, with a 'quote'"]);
        copy.close().await;
        source.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_in_memory_database_is_copied_too() {
        let dir = scratch();
        let source = crate::db::testing::sqlite().await;
        stored(&source, "Held in memory").await;

        let into = dir.join("copy.sqlite");
        take(&source, &into).await.unwrap();
        let copy = open(&into, false).await.unwrap();
        assert_eq!(titles(&copy).await, ["Held in memory"]);
        copy.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_whole_database_is_refused() {
        let dir = scratch();
        let path = dir.join("cut.sqlite");
        std::fs::write(&path, b"SQLite format 3\0 cut short").unwrap();
        assert!(check(&path).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
