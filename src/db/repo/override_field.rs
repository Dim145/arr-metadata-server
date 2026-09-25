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

/// Every override in the catalogue, with the work each is on, in a fixed
/// order: what an export is made of.
pub async fn all(db: &Db) -> Result<Vec<(String, Override)>> {
    let rows = sqlx::query(db.sql(
        "SELECT media_id, scope, field, value, updated_at, updated_by
         FROM media_override ORDER BY media_id, scope, field",
    ))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            let raw: Option<String> = row.opt_text("value")?;
            Ok((
                row.text("media_id")?,
                Override {
                    scope: row.text("scope")?,
                    field: row.text("field")?,
                    value: raw.and_then(|s| serde_json::from_str(&s).ok()),
                    updated_at: row.text("updated_at")?,
                    updated_by: row.opt_text("updated_by")?,
                },
            ))
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
    // The lock and the change it makes to how the work is listed, together:
    // written apart, a failure between the two left the lock unlisted, and
    // nothing afterwards would ever list it.
    let mut tx = db.begin_write().await?;

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
    .execute(&mut *tx)
    .await?;

    // A lock on the work itself changes what it is listed by: its genre, its
    // year, its title's place in the order.
    if scope == Scope::Item {
        super::item::mark_changed_in(db, &mut tx, media_id).await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Remove an override, handing the field back to provider data.
pub async fn unset(db: &Db, media_id: &str, scope: Scope, field: &str) -> Result<bool> {
    let mut tx = db.begin_write().await?;

    let result = sqlx::query(
        db.sql("DELETE FROM media_override WHERE media_id = ? AND scope = ? AND field = ?"),
    )
    .bind(media_id)
    .bind(scope.to_string())
    .bind(field)
    .execute(&mut *tx)
    .await?;

    let removed = result.rows_affected() > 0;
    if removed && scope == Scope::Item {
        super::item::mark_changed_in(db, &mut tx, media_id).await?;
    }

    tx.commit().await?;
    Ok(removed)
}

/// Unlock every field of a work at once.
pub async fn clear(db: &Db, media_id: &str) -> Result<u64> {
    let mut tx = db.begin_write().await?;

    let result = sqlx::query(db.sql("DELETE FROM media_override WHERE media_id = ?"))
        .bind(media_id)
        .execute(&mut *tx)
        .await?;

    if result.rows_affected() > 0 {
        super::item::mark_changed_in(db, &mut tx, media_id).await?;
    }

    tx.commit().await?;
    Ok(result.rows_affected())
}

/// Overrides for several works at once, keyed by media id.
///
/// A list view needs every item's edits applied; doing that one query per row
/// turns a page of fifty into fifty round trips.
pub async fn list_for_many(
    db: &Db,
    media_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<Override>>> {
    use std::collections::HashMap;

    let mut out: HashMap<String, Vec<Override>> = HashMap::new();

    if media_ids.is_empty() {
        return Ok(out);
    }

    // Chunked to stay well inside every engine's bind-parameter limit.
    for chunk in media_ids.chunks(200) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT media_id, scope, field, value, updated_at, updated_by
             FROM media_override WHERE media_id IN ({placeholders})
             ORDER BY media_id, scope, field"
        );

        let mut query = sqlx::query(db.sql(&sql));
        for id in chunk {
            query = query.bind(id);
        }

        for row in query.fetch_all(db.pool()).await? {
            let raw: Option<String> = row.opt_text("value")?;
            out.entry(row.text("media_id")?)
                .or_default()
                .push(Override {
                    scope: row.text("scope")?,
                    field: row.text("field")?,
                    value: raw.and_then(|s| serde_json::from_str(&s).ok()),
                    updated_at: row.text("updated_at")?,
                    updated_by: row.opt_text("updated_by")?,
                });
        }
    }

    Ok(out)
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_override"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}
