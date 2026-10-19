//! The `backup` table: one row an archive this database made of itself.

use anyhow::Result;

use crate::db::{Db, RowExt, from_bool};

/// An archive, as its row describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub reason: String,
    pub place: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub engine: String,
    pub server_version: String,
    pub migration: i64,
    pub includes_media: bool,
    pub media_files: i64,
    pub encrypted: bool,
    pub created_by: Option<String>,
    pub job_id: Option<String>,
}

const COLUMNS: &str = "id, name, created_at, reason, place, size_bytes, sha256, engine, \
                       server_version, migration, includes_media, media_files, encrypted, \
                       created_by, job_id";

pub async fn insert(db: &Db, row: &Row) -> Result<()> {
    sqlx::query(db.sql(&format!(
        "INSERT INTO backup ({COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )))
    .bind(&row.id)
    .bind(&row.name)
    .bind(&row.created_at)
    .bind(&row.reason)
    .bind(&row.place)
    .bind(row.size_bytes)
    .bind(&row.sha256)
    .bind(&row.engine)
    .bind(&row.server_version)
    .bind(row.migration)
    .bind(from_bool(row.includes_media))
    .bind(row.media_files)
    .bind(from_bool(row.encrypted))
    .bind(row.created_by.as_deref())
    .bind(row.job_id.as_deref())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Every row, newest first.
pub async fn all(db: &Db) -> Result<Vec<Row>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM backup ORDER BY created_at DESC, name DESC"
    )))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(read).collect()
}

pub async fn by_name(db: &Db, name: &str) -> Result<Option<Row>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM backup WHERE name = ?")))
        .bind(name)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(read).transpose()
}

/// Forget an archive; whether there was a row to forget.
pub async fn remove(db: &Db, name: &str) -> Result<bool> {
    let done = sqlx::query(db.sql("DELETE FROM backup WHERE name = ?"))
        .bind(name)
        .execute(db.pool())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// When the newest archive was made, if one was.
pub async fn newest_at(db: &Db) -> Result<Option<String>> {
    let row = sqlx::query(db.sql("SELECT MAX(created_at) AS at FROM backup"))
        .fetch_one(db.pool())
        .await?;
    Ok(row.opt_text("at")?)
}

fn read(row: &sqlx::any::AnyRow) -> Result<Row> {
    Ok(Row {
        id: row.text("id")?,
        name: row.text("name")?,
        created_at: row.text("created_at")?,
        reason: row.text("reason")?,
        place: row.text("place")?,
        size_bytes: row.big("size_bytes")?,
        sha256: row.text("sha256")?,
        engine: row.text("engine")?,
        server_version: row.text("server_version")?,
        migration: row.big("migration")?,
        includes_media: row.flag("includes_media")?,
        media_files: row.big("media_files")?,
        encrypted: row.flag("encrypted")?,
        created_by: row.opt_text("created_by")?,
        job_id: row.opt_text("job_id")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn row(name: &str, at: &str) -> Row {
        Row {
            id: crate::db::new_id(),
            name: name.into(),
            created_at: at.into(),
            reason: "scheduled".into(),
            place: "directory".into(),
            size_bytes: 4_294_967_296,
            sha256: "0".repeat(64),
            engine: "postgres".into(),
            server_version: "0.9.0".into(),
            migration: 36,
            includes_media: true,
            media_files: 12,
            encrypted: true,
            created_by: None,
            job_id: Some("job".into()),
        }
    }

    #[tokio::test]
    async fn a_row_reads_back_as_written_and_the_newest_comes_first() {
        let db = crate::db::testing::db().await;
        assert_eq!(newest_at(&db).await.unwrap(), None);

        let older = row(
            "ams-20261012-020000-scheduled.tar.zst.age",
            "2026-10-12T02:00:00.000Z",
        );
        let newer = Row {
            created_by: Some("admin:alice".into()),
            includes_media: false,
            encrypted: false,
            ..row(
                "ams-20261019-020000-manual.tar.zst",
                "2026-10-19T02:00:00.000Z",
            )
        };
        insert(&db, &older).await.unwrap();
        insert(&db, &newer).await.unwrap();

        assert_eq!(all(&db).await.unwrap(), [newer.clone(), older.clone()]);
        assert_eq!(
            by_name(&db, &older.name).await.unwrap().as_ref(),
            Some(&older)
        );
        assert_eq!(
            newest_at(&db).await.unwrap().as_deref(),
            Some("2026-10-19T02:00:00.000Z")
        );

        assert!(remove(&db, &newer.name).await.unwrap());
        assert!(!remove(&db, &newer.name).await.unwrap());
        assert_eq!(all(&db).await.unwrap(), [older]);
    }
}
