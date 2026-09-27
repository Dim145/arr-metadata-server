//! Background work, recorded so an operator can see what the scheduler did.
//!
//! One row per *run*, not per item: a sweep touching twenty-five entries is one
//! job with a summary, because twenty-five rows every fifteen minutes would bury
//! the one that failed. A refresh someone asked for by hand gets its own row,
//! since that is a thing a person is waiting on.

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;

use crate::db::{Db, RowExt, new_id, now};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Succeeded,
    Failed,
    /// Asked to stop partway, and did: neither a success nor a failure.
    Stopped,
}

impl Status {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// What kind of work ran.
pub mod kinds {
    /// A scheduled sweep over everything due a refresh.
    pub const REFRESH_SWEEP: &str = "refresh.sweep";
    /// One entry refreshed because someone asked.
    pub const REFRESH_ITEM: &str = "refresh.item";
    /// The anime identifier list, downloaded whole.
    pub const IMPORT_ANIME: &str = "import.anime";
    /// IMDb's ratings, downloaded whole.
    pub const IMPORT_IMDB: &str = "import.imdb";
    /// Every work with an identifier elsewhere, refreshed at once, by hand.
    pub const REFRESH_ALL: &str = "refresh.all";
    /// The `.nfo` documents, written for the whole library.
    pub const EXPORT_NFO: &str = "export.nfo";
    /// Every picture and theme the catalogue points at, fetched and kept.
    pub const MEDIA_STORE: &str = "media.store";
    /// The media nobody points at any more, forgotten and deleted.
    pub const MEDIA_SWEEP: &str = "media.sweep";
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub kind: String,
    /// What it acted on: an item id, or nothing for a sweep.
    pub target: Option<String>,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub detail: Option<String>,
    pub created_at: String,
    /// Who started it, as the journal names them; nothing for the schedule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggered_by: Option<String>,
    /// The work it acted on, while it is held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::db::repo::item::WorkRef>,
}

/// Open a job row the schedule started and return its id. Mark it done with
/// [`finish`].
pub async fn start(db: &Db, kind: &str, target: Option<&str>) -> Result<String> {
    start_by(db, kind, target, None).await
}

/// Open a job row somebody started — `by` as the journal names them.
pub async fn start_by(
    db: &Db,
    kind: &str,
    target: Option<&str>,
    by: Option<&str>,
) -> Result<String> {
    let id = new_id();
    let at = now();

    sqlx::query(db.sql(
        "INSERT INTO job_run (id, kind, target, status, started_at, created_at, triggered_by)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(kind)
    .bind(target)
    .bind(Status::Running.as_str())
    .bind(&at)
    .bind(&at)
    .bind(by)
    .execute(db.pool())
    .await?;

    Ok(id)
}

/// Close a run that was asked to stop, saying how far it got.
pub async fn stop(db: &Db, id: &str, detail: &str) -> Result<()> {
    sqlx::query(db.sql(
        "UPDATE job_run SET status = ?, finished_at = ?, detail = ?, error = NULL WHERE id = ?",
    ))
    .bind(Status::Stopped.as_str())
    .bind(now())
    .bind(detail)
    .bind(id)
    .execute(db.pool())
    .await?;

    Ok(())
}

/// Say how far a run has got, while it runs.
pub async fn progress(db: &Db, id: &str, detail: &str) -> Result<()> {
    sqlx::query(db.sql("UPDATE job_run SET detail = ? WHERE id = ? AND status = 'running'"))
        .bind(detail)
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(())
}

const COLUMNS: &str =
    "id, kind, target, status, started_at, finished_at, error, detail, created_at, triggered_by";

fn map(row: &sqlx::any::AnyRow) -> Result<Job> {
    Ok(Job {
        id: row.text("id")?,
        kind: row.text("kind")?,
        target: row.opt_text("target")?,
        status: row.text("status")?,
        started_at: row.opt_text("started_at")?,
        finished_at: row.opt_text("finished_at")?,
        error: row.opt_text("error")?,
        detail: row.opt_text("detail")?,
        created_at: row.text("created_at")?,
        triggered_by: row.opt_text("triggered_by")?,
        work: None,
    })
}

pub async fn get(db: &Db, id: &str) -> Result<Option<Job>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM job_run WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    row.as_ref().map(map).transpose()
}

/// A kind's latest run, whatever its outcome, and when it last succeeded.
pub async fn latest(db: &Db, kind: &str) -> Result<(Option<Job>, Option<String>)> {
    let last = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM job_run WHERE kind = ? ORDER BY created_at DESC, id DESC LIMIT 1"
    )))
    .bind(kind)
    .fetch_optional(db.pool())
    .await?;

    let succeeded =
        sqlx::query(db.sql(
            "SELECT MAX(finished_at) AS at FROM job_run WHERE kind = ? AND status = 'succeeded'",
        ))
        .bind(kind)
        .fetch_one(db.pool())
        .await?;

    Ok((
        last.as_ref().map(map).transpose()?,
        succeeded.opt_text("at")?,
    ))
}

/// Close a job row. `error` decides whether it succeeded.
pub async fn finish(db: &Db, id: &str, detail: Option<&str>, error: Option<&str>) -> Result<()> {
    let status = if error.is_some() {
        Status::Failed
    } else {
        Status::Succeeded
    };

    sqlx::query(
        db.sql(
            "UPDATE job_run SET status = ?, finished_at = ?, detail = ?, error = ? WHERE id = ?",
        ),
    )
    .bind(status.as_str())
    .bind(now())
    .bind(detail)
    .bind(error)
    .bind(id)
    .execute(db.pool())
    .await?;

    Ok(())
}

#[derive(Debug, Default)]
pub struct Query {
    pub kind: Option<String>,
    pub status: Option<String>,
    /// `schedule`, or `person` for the runs somebody started.
    pub by: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// The conditions a query puts on the runs, and what they are bound to.
fn filtered(q: &Query) -> Result<(String, sqlx::any::AnyArguments)> {
    use sqlx::{Arguments, any::AnyArguments};

    let mut sql = String::from(" WHERE 1 = 1");
    let mut args = AnyArguments::default();

    let bind = |args: &mut AnyArguments, value: String| -> Result<()> {
        args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
    };

    if let Some(kind) = &q.kind {
        sql.push_str(" AND kind = ?");
        bind(&mut args, kind.clone())?;
    }
    if let Some(status) = &q.status {
        sql.push_str(" AND status = ?");
        bind(&mut args, status.clone())?;
    }
    match q.by.as_deref() {
        Some("schedule") => sql.push_str(" AND triggered_by IS NULL"),
        Some("person") => sql.push_str(" AND triggered_by IS NOT NULL"),
        _ => {}
    }

    Ok((sql, args))
}

pub async fn list(db: &Db, q: &Query) -> Result<Vec<Job>> {
    use sqlx::Arguments;

    let (conditions, mut args) = filtered(q)?;
    let mut sql = format!("SELECT {COLUMNS} FROM job_run{conditions}");

    // Ids are UUIDv7, so they break a timestamp tie in creation order.
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?");
    args.add(q.limit.clamp(1, 500))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    args.add(q.offset.max(0))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    rows.iter().map(map).collect()
}

/// How many runs a query's conditions match, however many pages of them.
pub async fn count_matching(db: &Db, q: &Query) -> Result<i64> {
    let (conditions, args) = filtered(q)?;
    let row = sqlx::query_with(
        db.sql(&format!("SELECT COUNT(*) AS n FROM job_run{conditions}")),
        args,
    )
    .fetch_one(db.pool())
    .await?;

    Ok(row.big("n")?)
}

/// How many runs the log holds of each kind, in each state.
pub async fn counts(db: &Db) -> Result<Vec<(String, String, i64)>> {
    let rows = sqlx::query(db.sql(
        "SELECT kind, status, COUNT(*) AS n FROM job_run GROUP BY kind, status ORDER BY kind, status",
    ))
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| Ok((row.text("kind")?, row.text("status")?, row.big("n")?)))
        .collect()
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM job_run"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

/// Drop runs older than `cutoff` (RFC 3339).
pub async fn prune(db: &Db, cutoff: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM job_run WHERE created_at < ?"))
        .bind(cutoff)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected())
}

/// Close any run still marked running.
///
/// A job row is only ever closed by the task that opened it, so a process that
/// is killed mid-sweep leaves one behind. Without this they accumulate as
/// permanently-running phantoms in the UI.
pub async fn fail_orphaned(db: &Db) -> Result<u64> {
    let result = sqlx::query(db.sql(
        "UPDATE job_run
         SET status = 'failed', finished_at = ?, error = 'the server stopped while this was running'
         WHERE status = 'running'",
    ))
    .bind(now())
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_are_counted_by_kind_and_state() {
        let db = crate::db::Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .expect("in-memory database");
        db.migrate().await.expect("migrations");
        assert!(counts(&db).await.unwrap().is_empty());

        let done = start(&db, kinds::REFRESH_ITEM, Some("x")).await.unwrap();
        finish(&db, &done, Some("refreshed"), None).await.unwrap();
        let failed = start(&db, kinds::REFRESH_ITEM, Some("y")).await.unwrap();
        finish(&db, &failed, None, Some("no provider"))
            .await
            .unwrap();
        start(&db, kinds::REFRESH_ITEM, Some("z")).await.unwrap();

        let by = counts(&db).await.unwrap();
        let of = |status: &str| {
            by.iter()
                .find(|(k, s, _)| k == kinds::REFRESH_ITEM && s == status)
                .map(|(_, _, n)| *n)
        };
        assert_eq!(of("succeeded"), Some(1));
        assert_eq!(of("failed"), Some(1));
        assert_eq!(of("running"), Some(1));
    }
    use crate::config;

    async fn db() -> Db {
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

    #[tokio::test]
    async fn a_job_records_its_outcome() {
        let db = db().await;

        let ok = start(&db, kinds::REFRESH_SWEEP, None).await.unwrap();
        finish(&db, &ok, Some("12 refreshed"), None).await.unwrap();

        let bad = start(&db, kinds::REFRESH_ITEM, Some("item-1"))
            .await
            .unwrap();
        finish(&db, &bad, None, Some("the provider timed out"))
            .await
            .unwrap();

        let jobs = list(
            &db,
            &Query {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(jobs.len(), 2);

        let failed = jobs.iter().find(|j| j.status == "failed").unwrap();
        assert_eq!(failed.kind, kinds::REFRESH_ITEM);
        assert_eq!(failed.target.as_deref(), Some("item-1"));
        assert_eq!(failed.error.as_deref(), Some("the provider timed out"));
        assert!(failed.finished_at.is_some());

        let succeeded = jobs.iter().find(|j| j.status == "succeeded").unwrap();
        assert_eq!(succeeded.detail.as_deref(), Some("12 refreshed"));
        assert!(succeeded.error.is_none());
    }

    #[tokio::test]
    async fn jobs_can_be_filtered() {
        let db = db().await;

        finish(
            &db,
            &start(&db, kinds::REFRESH_SWEEP, None).await.unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        start(&db, kinds::REFRESH_ITEM, Some("x")).await.unwrap();

        let sweeps = list(
            &db,
            &Query {
                kind: Some(kinds::REFRESH_SWEEP.into()),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(sweeps.len(), 1);

        let running = list(
            &db,
            &Query {
                status: Some("running".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].kind, kinds::REFRESH_ITEM);
    }

    #[tokio::test]
    async fn a_run_left_open_by_a_crash_is_closed_at_startup() {
        let db = db().await;
        start(&db, kinds::REFRESH_SWEEP, None).await.unwrap();

        assert_eq!(fail_orphaned(&db).await.unwrap(), 1);

        let jobs = list(
            &db,
            &Query {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(jobs[0].status, "failed");
        assert!(jobs[0].error.as_deref().unwrap().contains("stopped"));

        // Running it again must not touch the row it already closed.
        assert_eq!(fail_orphaned(&db).await.unwrap(), 0);
    }
}
