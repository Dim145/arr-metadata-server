//! Who uses the database: the servers, which hold it shared for as long as
//! they run, and the commands that have to have it alone — a restore,
//! migrations run by hand — which are refused while any server holds it.
//!
//! sqlx's own migration lock is nothing on SQLite, and nothing a server
//! takes for longer than its migrations: neither keeps a restore from
//! emptying the tables under a server that is reading them. So: on SQLite,
//! a lock on a file beside the database, which every container that mounts
//! the volume sees; on PostgreSQL, an advisory lock, which every instance
//! and every command on that server sees, wherever it runs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sqlx::Connection as _;

use crate::config;

/// The advisory lock servers hold shared and a command holds alone: any
/// fixed number, the same for every instance, and not the start's.
const POSTGRES_KEY: i64 = 7_462_392;

/// A hold on the database, let go of when dropped — and by the system when
/// the process ends, however it ends.
pub struct Hold {
    _held: Held,
}

enum Held {
    File(#[allow(dead_code)] std::fs::File),
    Postgres(#[allow(dead_code)] sqlx::postgres::PgConnection),
    /// A database in memory, which nobody else can open.
    Nothing,
}

/// What a server holds while it runs: shared with the other instances,
/// refused while a command has the database to itself.
pub async fn hold_shared(database: &config::Database) -> Result<Hold> {
    hold(database, false).await.map_err(|e| {
        e.context(
            "a restore or a migration run from the command line has this database to \
             itself; start the server once it is done",
        )
    })
}

/// What a restore or a migration from the command line holds: the database
/// alone, refused while a server holds it — said with `instead`, what to do
/// rather than stop it.
pub async fn hold_alone(database: &config::Database, instead: &str) -> Result<Hold> {
    hold(database, true).await.map_err(|e| {
        e.context(format!(
            "a server is running on this database: stop it first{instead}"
        ))
    })
}

async fn hold(database: &config::Database, alone: bool) -> Result<Hold> {
    let url = database.url.as_str();
    if url.starts_with("postgres:") || url.starts_with("postgresql:") {
        let mut conn = sqlx::postgres::PgConnection::connect(url)
            .await
            .context("could not reach the database")?;
        let ask = if alone {
            "SELECT pg_try_advisory_lock($1)"
        } else {
            "SELECT pg_try_advisory_lock_shared($1)"
        };
        let held: bool = sqlx::query_scalar(ask)
            .bind(POSTGRES_KEY)
            .fetch_one(&mut conn)
            .await
            .context("could not ask for the database's lock")?;
        if !held {
            bail!("the database is held");
        }
        return Ok(Hold {
            _held: Held::Postgres(conn),
        });
    }

    let Some(path) = crate::db::sqlite_path(url) else {
        return Ok(Hold {
            _held: Held::Nothing,
        });
    };
    let lock = lock_file(&path);
    if let Some(parent) = lock.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .with_context(|| format!("could not open {}", lock.display()))?;
    let taken = if alone {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    match taken {
        Ok(()) => Ok(Hold {
            _held: Held::File(file),
        }),
        Err(std::fs::TryLockError::WouldBlock) => bail!("{} is held", lock.display()),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("could not lock {}", lock.display()))
        }
    }
}

/// The file beside an SQLite database that its users lock.
pub fn lock_file(database: &Path) -> PathBuf {
    let mut name = database
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".ams-lock");
    database.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite(path: &Path) -> config::Database {
        config::Database {
            url: crate::db::sqlite_url(path, true),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn a_command_is_refused_the_database_while_a_server_holds_it() {
        let dir = crate::backup::scratch();
        let database = sqlite(&dir.join("ams.db"));

        let first = hold_shared(&database).await.expect("a server");
        let second = hold_shared(&database).await.expect("another instance");
        let refused = hold_alone(&database, "").await.err().expect("refused");
        assert!(
            format!("{refused:#}").contains("a server is running"),
            "{refused:#}"
        );

        drop((first, second));
        let alone = hold_alone(&database, "").await.expect("nobody else");
        let refused = hold_shared(&database).await.err().expect("refused");
        assert!(
            format!("{refused:#}").contains("start the server once it is done"),
            "{refused:#}"
        );
        drop(alone);
        hold_shared(&database).await.expect("free again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_database_in_memory_is_nobody_elses() {
        let memory = config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        };
        let _server = hold_shared(&memory).await.unwrap();
        hold_alone(&memory, "").await.unwrap();
    }

    #[tokio::test]
    async fn on_postgresql_the_hold_is_the_servers_advisory_lock() {
        if crate::db::testing::server().is_none() {
            return;
        }
        // A database of the test's own, which no instance running beside
        // the tests holds: the lock is the database's.
        let database = crate::db::testing::database().await;
        let _kept = crate::db::Db::connect(&database).await.unwrap();
        let held = hold_shared(&database).await.expect("a server");
        assert!(hold_alone(&database, "").await.is_err());
        drop(held);
        drop(hold_alone(&database, "").await.expect("nobody else"));
    }
}
