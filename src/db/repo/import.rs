//! When each downloaded list was last brought in.

use anyhow::Result;

use crate::db::{Db, RowExt};

/// The last import of one list: when it landed, and how many rows it kept.
#[derive(Clone, Debug)]
pub struct Import {
    pub imported_at: String,
    pub row_count: i64,
}

pub async fn get(db: &Db, name: &str) -> Result<Option<Import>> {
    let row = sqlx::query(db.sql("SELECT imported_at, row_count FROM data_import WHERE name = ?"))
        .bind(name)
        .fetch_optional(db.pool())
        .await?;

    row.map(|r| {
        Ok(Import {
            imported_at: r.text("imported_at")?,
            row_count: r.big("row_count")?,
        })
    })
    .transpose()
}

/// Record an import as of `imported_at`: when it *started*, so that anything
/// added while it ran counts as added after it.
pub async fn record(db: &Db, name: &str, row_count: i64, imported_at: &str) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO data_import (name, imported_at, row_count) VALUES (?, ?, ?)
         ON CONFLICT (name) DO UPDATE SET
             imported_at = excluded.imported_at,
             row_count = excluded.row_count",
    ))
    .bind(name)
    .bind(imported_at)
    .bind(row_count)
    .execute(db.pool())
    .await?;

    Ok(())
}
