//! A database for a test to run against.
//!
//! What is worth testing in a repository is mostly its SQL: a column added to
//! a list of thirty-five without its placeholder compiles cleanly and fails
//! when the statement runs, and only a round trip through a real engine says
//! so. The engine matters too, because the two disagree where nothing in the
//! Rust type-checks — SQLite ignores what a bind is typed as and PostgreSQL
//! does not (a missing double bound as a single-precision null, a `SUM` that
//! comes back `NUMERIC`), and both of those reached a running server because
//! every test ran on SQLite alone.
//!
//! [`db`] gives an empty database with the real migrations applied, of
//! whichever engine the environment picks. With nothing set it is SQLite in
//! memory, which needs nothing installed. With `AMS_TEST_DATABASE_URL` naming
//! a PostgreSQL it is a database of that server's, made for the one test and
//! dropped with it:
//!
//! ```text
//! AMS_TEST_DATABASE_URL=postgres://ams:ams@127.0.0.1:5432/ams cargo test db::
//! ```
//!
//! The user in the address needs to be allowed to create databases.
//!
//! **A database for each test, not a schema.** Both keep tests from seeing
//! one another's rows, and a schema would be the lighter of the two, but it
//! does not give the test the server's own layout: that is one database with
//! one `public` schema, and everything the migrations do assumes it. A schema
//! per test means a `search_path` in the connection options, and then the
//! extension migration 0022 creates — `pg_trgm` belongs to the database, not
//! to a schema — is made by the first test in its schema, found by the second
//! as already there and missing from its own path, and taken away with the
//! first one's schema when that is dropped. A database of its own is dropped
//! with `FORCE`, which does not wait for a connection a failed test left
//! open, and costs about what the migrations cost either way.
//!
//! The database is dropped when the last handle to the [`Db`] is, which is
//! when the test ends however it ends — a failed assertion unwinds and drops
//! it like anything else. A run that is killed leaves its databases behind;
//! the next one takes away any of ours that are over an hour old.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use sqlx::{AnyConnection, AssertSqlSafe, Connection, Row};
use tokio::sync::OnceCell;

use super::Db;
use crate::config;

/// The variable that names a PostgreSQL for the tests to make databases on.
pub const VARIABLE: &str = "AMS_TEST_DATABASE_URL";

/// What every database made here starts with, so that a leftover can be told
/// from anybody else's.
const PREFIX: &str = "ams_test_";

/// How old one of ours has to be before the next run takes it for a leftover
/// of a run that was killed. A suite takes minutes, not hours.
const LEFTOVER_AFTER: Duration = Duration::from_secs(60 * 60);

/// An empty, migrated database: PostgreSQL's when the environment names one,
/// SQLite in memory when it does not.
pub async fn db() -> Db {
    match postgres().await {
        Some(db) => db,
        None => sqlite().await,
    }
}

/// SQLite in memory, whatever the environment says: for a test that is about
/// SQLite itself — a query plan, the search index, writers waiting on a lock —
/// and has nothing to say about the other engine.
pub async fn sqlite() -> Db {
    let db = Db::connect(&config::Database {
        url: "sqlite::memory:".into(),
        // An in-memory database belongs to its connection: a second one
        // would be a second, empty, database.
        max_connections: 1,
        acquire_timeout: Duration::from_secs(5),
    })
    .await
    .expect("an in-memory database");

    db.migrate().await.expect("the SQLite migrations");
    db
}

/// A database of the PostgreSQL the environment names, made for the caller
/// and dropped with the last handle to it; `None` when it names none.
pub async fn postgres() -> Option<Db> {
    let server = server()?;
    Some(fresh(&server).await.0)
}

/// The address of the PostgreSQL the tests may use, if there is one.
pub fn server() -> Option<String> {
    let given = std::env::var(VARIABLE).ok()?;
    let given = given.trim();
    if given.is_empty() {
        return None;
    }

    // Said before anything is tried: a SQLite address here would be one file
    // shared by every test, which is exactly what this is not for.
    assert!(
        url::Url::parse(given).is_ok_and(|url| matches!(url.scheme(), "postgres" | "postgresql")),
        "{VARIABLE} names a PostgreSQL, as postgres://user:password@host/database; \
         the SQLite the tests use otherwise needs no variable"
    );
    Some(given.to_string())
}

/// A new database on `server`, migrated, and the name it was given.
async fn fresh(server: &str) -> (Db, String) {
    sqlx::any::install_default_drivers();
    take_away_leftovers(server).await;

    let name = format!(
        "{PREFIX}{}_{}",
        unix_seconds(),
        uuid::Uuid::new_v4().simple()
    );

    let mut admin = AnyConnection::connect(server)
        .await
        .unwrap_or_else(|e| panic!("could not reach the PostgreSQL {VARIABLE} names: {e}"));
    sqlx::query(AssertSqlSafe(format!(r#"CREATE DATABASE "{name}""#)))
        .execute(&mut admin)
        .await
        .unwrap_or_else(|e| {
            panic!("could not make a database on the PostgreSQL {VARIABLE} names: {e}")
        });
    let _ = admin.close().await;

    // Held from here, so that a failure on the way to a usable database
    // takes it away again rather than leaving it.
    let lease = Arc::new(Lease {
        server: server.to_string(),
        name: name.clone(),
    });

    let mut db = Db::connect(&config::Database {
        url: address_of(server, &name),
        max_connections: 4,
        acquire_timeout: Duration::from_secs(10),
    })
    .await
    .expect("a connection to the database just made");
    db.lease = Some(lease);

    db.migrate().await.expect("the PostgreSQL migrations");
    (db, name)
}

/// `server`'s address with the database in its path replaced by `name`.
fn address_of(server: &str, name: &str) -> String {
    let mut url = url::Url::parse(server).expect("an address checked already");
    url.set_path(&format!("/{name}"));
    url.to_string()
}

/// What takes a test's database away: held by every clone of its [`Db`], run
/// when the last of them is dropped.
pub struct Lease {
    server: String,
    name: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let server = std::mem::take(&mut self.server);
        let name = std::mem::take(&mut self.name);

        // On a thread of its own, and waited for. A `Db` is let go of from the
        // test's own runtime, which cannot start a second one on the spot; and
        // a process that ends before the drop has been served leaves the
        // database where it was.
        let _ = std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(drop_database(&server, &name));
            }
        })
        .join();
    }
}

/// `DROP DATABASE`, with `FORCE`: it ends whatever connections the test left
/// instead of waiting for them. A failure is said and no more — a database
/// left behind is the next run's to take away.
async fn drop_database(server: &str, name: &str) {
    let outcome = async {
        let mut admin = AnyConnection::connect(server).await?;
        sqlx::query(AssertSqlSafe(format!(
            r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#
        )))
        .execute(&mut admin)
        .await?;
        admin.close().await
    }
    .await;

    if let Err(e) = outcome {
        eprintln!("could not drop the test database {name}: {e}");
    }
}

/// Once a process, the databases of ours that a run killed in the middle left
/// behind. Their names say when they were made: `ams_test_<seconds>_<id>`.
async fn take_away_leftovers(server: &str) {
    static DONE: OnceCell<()> = OnceCell::const_new();

    DONE.get_or_init(|| async {
        let Ok(mut admin) = AnyConnection::connect(server).await else {
            return;
        };
        let found = sqlx::query(
            "SELECT datname::text AS name FROM pg_database WHERE starts_with(datname::text, $1)",
        )
        .bind(PREFIX)
        .fetch_all(&mut admin)
        .await;
        let _ = admin.close().await;

        let now = unix_seconds();
        for row in found.unwrap_or_default() {
            let Ok(name) = row.try_get::<String, _>("name") else {
                continue;
            };
            let made = name[PREFIX.len()..]
                .split('_')
                .next()
                .and_then(|seconds| seconds.parse::<u64>().ok());

            if made.is_some_and(|made| now.saturating_sub(made) > LEFTOVER_AFTER.as_secs()) {
                drop_database(server, &name).await;
            }
        }
    })
    .await;
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Dialect;

    #[test]
    fn the_database_is_the_only_part_of_the_address_that_changes() {
        assert_eq!(
            address_of("postgres://ams:s3cret@db:5432/ams", "ams_test_1_x"),
            "postgres://ams:s3cret@db:5432/ams_test_1_x"
        );
        assert_eq!(
            address_of("postgresql://ams@db/ams?sslmode=disable", "ams_test_1_x"),
            "postgresql://ams@db/ams_test_1_x?sslmode=disable"
        );
    }

    #[tokio::test]
    async fn sqlite_is_what_the_tests_get_when_nothing_is_named() {
        let db = sqlite().await;
        assert_eq!(db.dialect(), Dialect::Sqlite);

        let tables = sqlx::query("SELECT COUNT(*) AS n FROM sqlite_master WHERE type = 'table'")
            .fetch_one(db.pool())
            .await
            .expect("the schema can be read");
        assert!(
            tables.try_get::<i64, _>("n").expect("a count") > 10,
            "the migrations were not applied"
        );
    }

    #[tokio::test]
    async fn a_postgres_database_is_its_own_and_is_dropped_with_its_last_handle() {
        let Some(server) = server() else {
            eprintln!("skipped: {VARIABLE} is not set");
            return;
        };

        let (first, name) = fresh(&server).await;
        let (second, _) = fresh(&server).await;
        assert_eq!(first.dialect(), Dialect::Postgres);

        // What one holds, the other does not.
        sqlx::query(
            "INSERT INTO keystore (name, value, created_at, updated_at) VALUES ('a', 'b', 'now', 'now')",
        )
        .execute(first.pool())
        .await
        .expect("a row can be written");
        let rows: i64 = sqlx::query("SELECT COUNT(*) AS n FROM keystore")
            .fetch_one(second.pool())
            .await
            .expect("the other database can be read")
            .try_get("n")
            .expect("a count");
        assert_eq!(rows, 0, "two tests would have shared a database");

        // A handle cloned into a task keeps it alive; the last one lets it go.
        let kept = first.clone();
        drop(first);
        assert!(exists(&server, &name).await, "dropped with a handle left");
        drop(kept);
        assert!(!exists(&server, &name).await, "left behind by the last one");
    }

    async fn exists(server: &str, name: &str) -> bool {
        let mut admin = AnyConnection::connect(server).await.expect("the server");
        let found = sqlx::query("SELECT COUNT(*) AS n FROM pg_database WHERE datname = $1")
            .bind(name.to_string())
            .fetch_one(&mut admin)
            .await
            .expect("the databases can be listed");
        found.try_get::<i64, _>("n").expect("a count") > 0
    }
}
