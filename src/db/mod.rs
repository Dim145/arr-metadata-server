//! Database access layer.
//!
//! One code path serves both SQLite (the default) and PostgreSQL. That is done
//! with sqlx's `Any` driver plus two deliberate concessions:
//!
//! * **Two migration sets** (`migrations/sqlite`, `migrations/postgres`) so each
//!   engine gets idiomatic column types instead of a lowest common denominator.
//! * **One placeholder style.** Queries are written with `?` throughout and
//!   [`Dialect::rewrite`] turns them into `$1, $2, …` for PostgreSQL.
//!
//! Values that the `Any` driver cannot carry natively — timestamps, UUIDs, JSON —
//! are stored as `TEXT` in RFC 3339 / canonical-hyphenated / serialized form.
//! RFC 3339 in UTC sorts lexicographically, so range predicates still work.

pub mod repo;
pub mod transfer;

use std::{borrow::Cow, path::Path, time::Duration};

use anyhow::{Context, Result, bail};
// `AnyPool` is re-exported at the crate root, `AnyPoolOptions` under `any`.
use sqlx::{AnyPool, AssertSqlSafe, Executor, Row, SqlStr, any::AnyPoolOptions};

use crate::config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

impl Dialect {
    fn from_url(url: &str) -> Result<Self> {
        let scheme = url.split(':').next().unwrap_or_default();
        match scheme {
            "sqlite" => Ok(Self::Sqlite),
            "postgres" | "postgresql" => Ok(Self::Postgres),
            other => {
                bail!("unsupported database scheme {other:?}; expected sqlite:// or postgres://")
            }
        }
    }

    /// Translate a `?`-placeholder query into this dialect's syntax.
    ///
    /// SQLite takes `?` verbatim. PostgreSQL needs `$1`, `$2`, … Question marks
    /// inside single-quoted string literals are left alone; doubled quotes
    /// (`''`) are handled as SQL escapes rather than as a closing quote.
    pub fn rewrite<'a>(&self, sql: &'a str) -> Cow<'a, str> {
        if *self == Self::Sqlite || !sql.contains('?') {
            return Cow::Borrowed(sql);
        }

        let mut out = String::with_capacity(sql.len() + 8);
        let mut index = 0usize;
        let mut in_string = false;
        let mut chars = sql.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '\'' if in_string && chars.peek() == Some(&'\'') => {
                    // Escaped quote inside a literal: consume both, stay inside.
                    out.push('\'');
                    out.push(chars.next().expect("peeked"));
                }
                '\'' => {
                    in_string = !in_string;
                    out.push('\'');
                }
                '?' if !in_string => {
                    index += 1;
                    out.push('$');
                    out.push_str(&index.to_string());
                }
                other => out.push(other),
            }
        }

        Cow::Owned(out)
    }
}

#[derive(Clone)]
pub struct Db {
    pool: AnyPool,
    dialect: Dialect,
}

impl Db {
    pub async fn connect(cfg: &config::Database) -> Result<Self> {
        sqlx::any::install_default_drivers();

        let dialect = Dialect::from_url(&cfg.url)?;

        if dialect == Dialect::Sqlite {
            ensure_sqlite_parent_dir(&cfg.url)?;
        }

        let pool = AnyPoolOptions::new()
            // SQLite tolerates concurrent readers but a single writer; a wide pool
            // buys nothing and multiplies lock contention.
            .max_connections(match dialect {
                Dialect::Sqlite => cfg.max_connections.min(8),
                Dialect::Postgres => cfg.max_connections,
            })
            .acquire_timeout(cfg.acquire_timeout)
            .after_connect(move |conn, _meta| {
                Box::pin(async move {
                    if dialect == Dialect::Sqlite {
                        // WAL is persistent; the rest are per-connection and must
                        // be reapplied every time the pool opens a handle.
                        for pragma in [
                            "PRAGMA journal_mode = WAL",
                            "PRAGMA synchronous = NORMAL",
                            "PRAGMA foreign_keys = ON",
                            "PRAGMA busy_timeout = 10000",
                            "PRAGMA temp_store = MEMORY",
                        ] {
                            conn.execute(pragma).await?;
                        }
                    }
                    Ok(())
                })
            })
            .connect(&cfg.url)
            .await
            .with_context(|| format!("failed to connect to {}", redact(&cfg.url)))?;

        Ok(Self { pool, dialect })
    }

    /// Apply the migration set belonging to this dialect.
    pub async fn migrate(&self) -> Result<()> {
        match self.dialect {
            Dialect::Sqlite => sqlx::migrate!("migrations/sqlite").run(&self.pool).await,
            Dialect::Postgres => sqlx::migrate!("migrations/postgres").run(&self.pool).await,
        }
        .context("database migration failed")?;

        tracing::info!(dialect = ?self.dialect, "database schema up to date");
        Ok(())
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Rewrite a `?`-placeholder query for this connection's dialect.
    ///
    /// This is the single place where a dynamically-built string is handed to
    /// sqlx, so it is the single place to audit. Two properties make it safe:
    ///
    /// * [`Dialect::rewrite`] only renumbers placeholders. It never splices in a
    ///   value, and it leaves string literals untouched.
    /// * Every caller assembles its query from string literals in this crate.
    ///   Caller-supplied data — search terms, ids, field names — is passed as a
    ///   bind parameter, never concatenated into the query text.
    ///
    /// Adding a caller that interpolates user input here would defeat that, so
    /// don't: use a bind parameter, or `QueryBuilder`.
    pub fn sql(&self, query: &str) -> SqlStr {
        use sqlx::SqlSafeStr as _;

        match self.dialect.rewrite(query) {
            Cow::Borrowed(s) => AssertSqlSafe(s).into_sql_str(),
            Cow::Owned(s) => AssertSqlSafe(s).into_sql_str(),
        }
    }

    pub async fn health(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .context("database health check failed")?;
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// `sqlite://data/ams.db?mode=rwc` only creates the file, never the directory.
fn ensure_sqlite_parent_dir(url: &str) -> Result<()> {
    let path = url
        .trim_start_matches("sqlite://")
        .trim_start_matches("sqlite:")
        .split('?')
        .next()
        .unwrap_or_default();

    if path.is_empty() || path == ":memory:" {
        return Ok(());
    }

    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create database directory {}", parent.display()))?;
    }

    Ok(())
}

/// Strip the password out of a connection URL before it reaches a log line.
fn redact(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((creds, host)) => {
                let user = creds.split(':').next().unwrap_or("");
                format!("{scheme}://{user}:***@{host}")
            }
            None => url.to_string(),
        },
        None => url.to_string(),
    }
}

// ─── row accessors ───────────────────────────────────────────────────────────

/// Typed column access over `AnyRow`.
///
/// `Any` has no date, UUID or JSON types, so those columns come back as `TEXT`
/// and are parsed here. Integers widen across SMALLINT/INTEGER/BIGINT, which is
/// why `i32` columns can safely be read through [`RowExt::int`].
pub trait RowExt {
    fn text(&self, col: &str) -> Result<String, sqlx::Error>;
    fn opt_text(&self, col: &str) -> Result<Option<String>, sqlx::Error>;
    fn big(&self, col: &str) -> Result<i64, sqlx::Error>;
    fn opt_big(&self, col: &str) -> Result<Option<i64>, sqlx::Error>;
    fn int(&self, col: &str) -> Result<i32, sqlx::Error>;
    fn opt_int(&self, col: &str) -> Result<Option<i32>, sqlx::Error>;
    fn opt_real(&self, col: &str) -> Result<Option<f64>, sqlx::Error>;
    /// Boolean stored as 0/1; see the migration header for why it is not BOOLEAN.
    fn flag(&self, col: &str) -> Result<bool, sqlx::Error>;
    /// A `TEXT` column holding a JSON array of strings. A malformed document
    /// yields an empty list rather than failing the whole row.
    fn text_list(&self, col: &str) -> Result<Vec<String>, sqlx::Error>;
}

impl RowExt for sqlx::any::AnyRow {
    fn text(&self, col: &str) -> Result<String, sqlx::Error> {
        self.try_get(col)
    }

    fn opt_text(&self, col: &str) -> Result<Option<String>, sqlx::Error> {
        self.try_get(col)
    }

    fn big(&self, col: &str) -> Result<i64, sqlx::Error> {
        self.try_get(col)
    }

    fn opt_big(&self, col: &str) -> Result<Option<i64>, sqlx::Error> {
        self.try_get(col)
    }

    fn int(&self, col: &str) -> Result<i32, sqlx::Error> {
        self.try_get(col)
    }

    fn opt_int(&self, col: &str) -> Result<Option<i32>, sqlx::Error> {
        self.try_get(col)
    }

    fn opt_real(&self, col: &str) -> Result<Option<f64>, sqlx::Error> {
        self.try_get(col)
    }

    fn flag(&self, col: &str) -> Result<bool, sqlx::Error> {
        Ok(to_bool(self.try_get::<i64, _>(col)?))
    }

    fn text_list(&self, col: &str) -> Result<Vec<String>, sqlx::Error> {
        let raw: Option<String> = self.try_get(col)?;
        Ok(raw
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default())
    }
}

/// Serialize a list of strings for a `TEXT` JSON column.
pub fn text_list(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

// ─── shared value helpers ────────────────────────────────────────────────────

/// Current instant in the canonical storage format.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn to_rfc3339(ts: chrono::DateTime<chrono::Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn parse_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// SQLite has no boolean type; both engines round-trip through 0/1 safely.
pub fn to_bool(v: i64) -> bool {
    v != 0
}

pub fn from_bool(v: bool) -> i64 {
    i64::from(v)
}

#[allow(dead_code)]
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_leaves_placeholders_alone() {
        let sql = "SELECT * FROM t WHERE a = ? AND b = ?";
        assert_eq!(Dialect::Sqlite.rewrite(sql), sql);
    }

    #[test]
    fn postgres_numbers_placeholders() {
        assert_eq!(
            Dialect::Postgres.rewrite("SELECT * FROM t WHERE a = ? AND b = ?"),
            "SELECT * FROM t WHERE a = $1 AND b = $2"
        );
    }

    #[test]
    fn postgres_skips_question_marks_in_literals() {
        assert_eq!(
            Dialect::Postgres.rewrite("SELECT ? WHERE name = 'who? me' AND x = ?"),
            "SELECT $1 WHERE name = 'who? me' AND x = $2"
        );
    }

    #[test]
    fn postgres_handles_escaped_quotes() {
        assert_eq!(
            Dialect::Postgres.rewrite("SELECT ? WHERE s = 'it''s ? here' AND y = ?"),
            "SELECT $1 WHERE s = 'it''s ? here' AND y = $2"
        );
    }

    #[test]
    fn dialect_detection() {
        assert_eq!(
            Dialect::from_url("sqlite://data/a.db").unwrap(),
            Dialect::Sqlite
        );
        assert_eq!(
            Dialect::from_url("postgres://u:p@h/db").unwrap(),
            Dialect::Postgres
        );
        assert_eq!(
            Dialect::from_url("postgresql://u:p@h/db").unwrap(),
            Dialect::Postgres
        );
        assert!(Dialect::from_url("mysql://x").is_err());
    }

    #[test]
    fn redact_hides_password() {
        assert_eq!(
            redact("postgres://ams:s3cret@db:5432/ams"),
            "postgres://ams:***@db:5432/ams"
        );
        assert_eq!(redact("sqlite://data/ams.db"), "sqlite://data/ams.db");
    }
}
