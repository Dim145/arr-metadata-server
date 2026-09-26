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
    /// SQLite takes `?` verbatim. PostgreSQL needs `$1`, `$2`, … A `?` is a
    /// placeholder only where one could go: not inside a single-quoted literal
    /// (doubled quotes `''` are an escape, not a close), not inside a
    /// double-quoted identifier, and not inside a comment.
    ///
    /// The comment case is the one worth the code. Renumbering starts at the
    /// first `?` the scanner sees, so a `?` in a `-- why?` eats `$1` and every
    /// real placeholder after it shifts by one — binding each value to the
    /// wrong column, with no error at all when the types happen to line up.
    /// Nothing in this crate writes a comment into a query string today; the
    /// point is that the first one will not be a silent data corruption.
    pub fn rewrite<'a>(&self, sql: &'a str) -> Cow<'a, str> {
        if *self == Self::Sqlite || !sql.contains('?') {
            return Cow::Borrowed(sql);
        }

        /// Where the scanner is, which decides whether a `?` means anything.
        enum In {
            Sql,
            String,
            Identifier,
            LineComment,
            BlockComment,
        }

        let mut out = String::with_capacity(sql.len() + 8);
        let mut index = 0usize;
        let mut state = In::Sql;
        let mut chars = sql.chars().peekable();

        while let Some(c) = chars.next() {
            match state {
                In::String => {
                    out.push(c);
                    if c == '\'' {
                        // Doubled: an escaped quote, still inside the literal.
                        if chars.peek() == Some(&'\'') {
                            out.push(chars.next().expect("peeked"));
                        } else {
                            state = In::Sql;
                        }
                    }
                }
                In::Identifier => {
                    out.push(c);
                    if c == '"' {
                        if chars.peek() == Some(&'"') {
                            out.push(chars.next().expect("peeked"));
                        } else {
                            state = In::Sql;
                        }
                    }
                }
                In::LineComment => {
                    out.push(c);
                    if c == '\n' {
                        state = In::Sql;
                    }
                }
                In::BlockComment => {
                    out.push(c);
                    if c == '*' && chars.peek() == Some(&'/') {
                        out.push(chars.next().expect("peeked"));
                        state = In::Sql;
                    }
                }
                In::Sql => match c {
                    '\'' => {
                        state = In::String;
                        out.push(c);
                    }
                    '"' => {
                        state = In::Identifier;
                        out.push(c);
                    }
                    '-' if chars.peek() == Some(&'-') => {
                        state = In::LineComment;
                        out.push(c);
                        out.push(chars.next().expect("peeked"));
                    }
                    '/' if chars.peek() == Some(&'*') => {
                        state = In::BlockComment;
                        out.push(c);
                        out.push(chars.next().expect("peeked"));
                    }
                    '?' => {
                        index += 1;
                        out.push('$');
                        out.push_str(&index.to_string());
                    }
                    other => out.push(other),
                },
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

/// A PostgreSQL address with its statement cache switched off, unless the
/// operator sized the cache themselves.
///
/// sqlx 0.9's `Any` driver binds a missing double as a single-precision null
/// — `sqlx-core`'s `any/arguments.rs` has `Real` and `Double` the wrong way
/// round — and PostgreSQL fixes a cached statement's parameter types the first
/// time it is prepared. A statement first run with a missing rating is
/// prepared for a four-byte float and refuses the eight bytes of the next real
/// one: "incorrect binary data format", and the work is not stored. A Fan-Kai
/// has no rating on any episode, so the series stored after one on the same
/// connection failed. An unnamed statement is typed by each run's own values,
/// which a null of either width satisfies.
fn uncached(url: &str) -> String {
    if url.contains("statement-cache-capacity=") {
        return url.to_string();
    }

    let joiner = if url.contains('?') { '&' } else { '?' };
    format!("{url}{joiner}statement-cache-capacity=0")
}

impl Db {
    pub async fn connect(cfg: &config::Database) -> Result<Self> {
        sqlx::any::install_default_drivers();

        let dialect = Dialect::from_url(&cfg.url)?;

        if dialect == Dialect::Sqlite {
            ensure_sqlite_parent_dir(&cfg.url)?;
        }

        let url = match dialect {
            Dialect::Postgres => uncached(&cfg.url),
            Dialect::Sqlite => cfg.url.clone(),
        };

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
            .connect(&url)
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

    /// Open a transaction that is going to write.
    ///
    /// On SQLite, `BEGIN IMMEDIATE` — the write lock is taken now, and anyone
    /// else wanting it waits out the busy timeout for it. A plain `BEGIN` takes
    /// nothing until the first write, so a transaction that reads first (every
    /// work write checks its slug before inserting) holds a *snapshot*, and if
    /// another writer commits before it gets to its own write, SQLite refuses
    /// the upgrade on the spot: `database is locked`, without waiting at all,
    /// because no amount of waiting makes a stale snapshot current. Sixteen
    /// concurrent imports lost thirteen of their writes that way.
    ///
    /// PostgreSQL has row locks and no such upgrade, so it begins as usual.
    pub async fn begin_write(&self) -> Result<sqlx::Transaction<'static, sqlx::Any>> {
        let tx = match self.dialect {
            Dialect::Sqlite => self.pool.begin_with("BEGIN IMMEDIATE").await,
            Dialect::Postgres => self.pool.begin().await,
        };

        tx.context("failed to open a write transaction")
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
    #[test]
    fn a_postgres_address_is_opened_without_a_statement_cache() {
        use super::uncached;

        assert_eq!(
            uncached("postgres://ams@db/ams"),
            "postgres://ams@db/ams?statement-cache-capacity=0"
        );
        assert_eq!(
            uncached("postgres://ams@db/ams?sslmode=disable"),
            "postgres://ams@db/ams?sslmode=disable&statement-cache-capacity=0"
        );
        // An operator who sized it keeps their size.
        assert_eq!(
            uncached("postgres://ams@db/ams?statement-cache-capacity=50"),
            "postgres://ams@db/ams?statement-cache-capacity=50"
        );
    }

    #[test]
    fn a_question_mark_that_is_not_a_placeholder_is_left_alone() {
        let pg = Dialect::Postgres;

        // A comment. This is the dangerous one: the `?` in it used to take
        // `$1`, shifting every real placeholder by one and binding each value
        // to the wrong column — silently, whenever the types happened to fit.
        assert_eq!(
            pg.rewrite("SELECT a -- why?\nFROM t WHERE b = ?"),
            "SELECT a -- why?\nFROM t WHERE b = $1"
        );
        assert_eq!(
            pg.rewrite("SELECT a /* is it? */ FROM t WHERE b = ?"),
            "SELECT a /* is it? */ FROM t WHERE b = $1"
        );

        // A quoted identifier.
        assert_eq!(
            pg.rewrite("SELECT \"we?ird\" FROM t WHERE id = ?"),
            "SELECT \"we?ird\" FROM t WHERE id = $1"
        );

        // The jsonb "does this key exist" operator, which this schema will
        // reach for the day a payload column stops being TEXT.
        assert_eq!(
            pg.rewrite("SELECT * FROM t WHERE payload ? 'k' AND id = ?"),
            "SELECT * FROM t WHERE payload $1 'k' AND id = $2",
            "still wrong, but it is the one case a comment cannot rescue — \
             left here so the next person sees it before writing it"
        );

        // And the cases that already worked, unchanged.
        assert_eq!(
            pg.rewrite("SELECT * FROM t WHERE s = 'it''s ?' AND id = ?"),
            "SELECT * FROM t WHERE s = 'it''s ?' AND id = $1"
        );
        assert_eq!(
            pg.rewrite("SELECT x::text FROM t WHERE id = ?"),
            "SELECT x::text FROM t WHERE id = $1"
        );
    }

    #[test]
    fn no_query_in_this_crate_carries_a_comment_or_a_jsonb_operator() {
        // The rewriter handles comments now. The jsonb containment operators it
        // cannot tell from a bind, so this is the guard that says so before one
        // is written rather than after it has bound the wrong column.
        //
        // The needles are built rather than written, or this test would be the
        // first thing it found.
        let q = '?';
        let needles = [format!("{q}|"), format!("{q}&")];

        let mut offenders: Vec<String> = Vec::new();

        for file in walk("src") {
            let text = std::fs::read_to_string(&file).expect("a readable source file");

            for (number, line) in text.lines().enumerate() {
                if needles.iter().any(|needle| line.contains(needle.as_str())) {
                    offenders.push(format!("{}:{}", file.display(), number + 1));
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "a jsonb key operator the placeholder rewriter cannot tell from a \
             bind: {offenders:?}"
        );
    }

    fn walk(dir: &str) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return found;
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walk(&path.to_string_lossy()));
            } else if path.extension().is_some_and(|e| e == "rs") {
                found.push(path);
            }
        }

        found
    }

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
