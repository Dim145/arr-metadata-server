//! When the periodic tasks run, where an administrator said otherwise than
//! each task's own cadence; see `jobs::schedule`, which reads and checks
//! what is kept here.

use anyhow::Result;

use crate::db::{Db, RowExt, now};

/// A schedule as it is kept, unread: `jobs::schedule` checks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub task: String,
    /// `window` or `cron`.
    pub kind: String,
    pub window_start: Option<String>,
    pub window_end: Option<String>,
    pub cron: Option<String>,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

const COLUMNS: &str = "task, kind, window_start, window_end, cron, updated_by, updated_at";

fn map(row: &sqlx::any::AnyRow) -> Result<Row> {
    Ok(Row {
        task: row.text("task")?,
        kind: row.text("kind")?,
        window_start: row.opt_text("window_start")?,
        window_end: row.opt_text("window_end")?,
        cron: row.opt_text("cron")?,
        updated_by: row.opt_text("updated_by")?,
        updated_at: row.text("updated_at")?,
    })
}

pub async fn get(db: &Db, task: &str) -> Result<Option<Row>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM task_schedule WHERE task = ?"
    )))
    .bind(task)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref().map(map).transpose()
}

pub async fn all(db: &Db) -> Result<Vec<Row>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM task_schedule ORDER BY task"
    )))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

/// What a schedule is set to: a window's two ends, or an expression.
pub struct Set<'a> {
    pub kind: &'a str,
    pub window_start: Option<&'a str>,
    pub window_end: Option<&'a str>,
    pub cron: Option<&'a str>,
    pub by: Option<&'a str>,
}

/// Set a task's schedule, replacing the one it had. When it was set is now.
pub async fn set(db: &Db, task: &str, schedule: Set<'_>) -> Result<Row> {
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO task_schedule (task, kind, window_start, window_end, cron, updated_by, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (task) DO UPDATE SET
             kind = excluded.kind,
             window_start = excluded.window_start,
             window_end = excluded.window_end,
             cron = excluded.cron,
             updated_by = excluded.updated_by,
             updated_at = excluded.updated_at",
    ))
    .bind(task)
    .bind(schedule.kind)
    .bind(schedule.window_start)
    .bind(schedule.window_end)
    .bind(schedule.cron)
    .bind(schedule.by)
    .bind(&at)
    .execute(db.pool())
    .await?;

    Ok(Row {
        task: task.to_string(),
        kind: schedule.kind.to_string(),
        window_start: schedule.window_start.map(str::to_string),
        window_end: schedule.window_end.map(str::to_string),
        cron: schedule.cron.map(str::to_string),
        updated_by: schedule.by.map(str::to_string),
        updated_at: at,
    })
}

/// Leave a task to its own cadence again. Whether it had a schedule.
pub async fn clear(db: &Db, task: &str) -> Result<bool> {
    let done = sqlx::query(db.sql("DELETE FROM task_schedule WHERE task = ?"))
        .bind(task)
        .execute(db.pool())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// When any schedule last changed: what tells a page following the tasks
/// that one moved, without reading them all.
pub async fn changed_at(db: &Db) -> Result<String> {
    let row = sqlx::query(db.sql("SELECT MAX(updated_at) AS at, COUNT(*) AS n FROM task_schedule"))
        .fetch_one(db.pool())
        .await?;
    // A schedule taken away changes the count, not the latest date.
    Ok(format!(
        "{}#{}",
        row.opt_text("at")?.unwrap_or_default(),
        row.big("n")?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::db;

    #[tokio::test]
    async fn a_schedule_is_set_replaced_and_taken_away() {
        let db = db().await;
        assert!(get(&db, "refresh.sweep").await.unwrap().is_none());
        let before = changed_at(&db).await.unwrap();

        set(
            &db,
            "refresh.sweep",
            Set {
                kind: "window",
                window_start: Some("01:00"),
                window_end: Some("06:00"),
                cron: None,
                by: Some("admin:someone"),
            },
        )
        .await
        .unwrap();
        let read = get(&db, "refresh.sweep").await.unwrap().unwrap();
        assert_eq!(read.kind, "window");
        assert_eq!(read.window_start.as_deref(), Some("01:00"));
        assert_eq!(read.updated_by.as_deref(), Some("admin:someone"));
        assert_ne!(changed_at(&db).await.unwrap(), before);

        // Replaced whole: the window's ends go with it.
        set(
            &db,
            "refresh.sweep",
            Set {
                kind: "cron",
                window_start: None,
                window_end: None,
                cron: Some("0 4 * * 0"),
                by: None,
            },
        )
        .await
        .unwrap();
        let read = get(&db, "refresh.sweep").await.unwrap().unwrap();
        assert_eq!(read.kind, "cron");
        assert_eq!(read.window_start, None);
        assert_eq!(read.cron.as_deref(), Some("0 4 * * 0"));
        assert_eq!(all(&db).await.unwrap().len(), 1);

        assert!(clear(&db, "refresh.sweep").await.unwrap());
        assert!(!clear(&db, "refresh.sweep").await.unwrap());
        assert!(all(&db).await.unwrap().is_empty());
    }
}
