//! Manual overrides — the locking layer.
//!
//! Nothing outside an explicit user action ever writes here. The refresh path
//! must not touch this table; that invariant is what makes an edit permanent.

use anyhow::Result;
use serde_json::Value;

use crate::{
    db::{Db, RowExt, now},
    domain::fields::{Override, Scope},
};

pub async fn list(db: &Db, media_id: &str) -> Result<Vec<Override>> {
    let rows = sqlx::query(db.sql(
        "SELECT scope, field, value, updated_at, updated_by
         FROM media_override WHERE media_id = ? ORDER BY scope, field",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            let raw: Option<String> = row.opt_text("value")?;
            Ok(Override {
                scope: row.text("scope")?,
                field: row.text("field")?,
                value: raw.and_then(|s| serde_json::from_str(&s).ok()),
                updated_at: row.text("updated_at")?,
                updated_by: row.opt_text("updated_by")?,
            })
        })
        .collect()
}

/// Record an edit. `value = None` stores an explicit "cleared" marker, which is
/// different from deleting the override.
pub async fn set(
    db: &Db,
    media_id: &str,
    scope: Scope,
    field: &str,
    value: Option<&Value>,
    by: Option<&str>,
) -> Result<()> {
    let encoded = value.map(serde_json::to_string).transpose()?;
    let at = now();

    sqlx::query(db.sql(
        "INSERT INTO media_override (media_id, scope, field, value, created_at, updated_at, updated_by)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (media_id, scope, field) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at,
             updated_by = excluded.updated_by",
    ))
    .bind(media_id)
    .bind(scope.to_string())
    .bind(field)
    .bind(encoded)
    .bind(&at)
    .bind(&at)
    .bind(by)
    .execute(db.pool())
    .await?;

    Ok(())
}

/// Remove an override, handing the field back to provider data.
pub async fn unset(db: &Db, media_id: &str, scope: Scope, field: &str) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "DELETE FROM media_override WHERE media_id = ? AND scope = ? AND field = ?",
    ))
    .bind(media_id)
    .bind(scope.to_string())
    .bind(field)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Unlock every field of a work at once.
pub async fn clear(db: &Db, media_id: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM media_override WHERE media_id = ?"))
        .bind(media_id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected())
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_override"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}
