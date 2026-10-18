//! The refresh's bookkeeping: where a work's values came from, which works
//! are due, what a refresh of everything works through, and how the last
//! refresh of a work went.

use anyhow::Result;

use crate::{
    db::{Db, RowExt, now},
    domain::MediaKind,
};

/// Who gave a work its values, when a merge has said.
pub async fn provenance(db: &Db, id: &str) -> Result<Option<crate::merge::provenance::Provenance>> {
    let row = sqlx::query(db.sql("SELECT provenance FROM media_item WHERE id = ?"))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    Ok(row
        .and_then(|row| row.opt_text("provenance").ok().flatten())
        .and_then(|json| serde_json::from_str(&json).ok()))
}

/// What a refresh of everything works through: the enabled works that have
/// an identifier elsewhere and are not entries made by hand, in id order,
/// after `after` — a cursor, so the run is one pass that ends, and nothing
/// about the works' own schedule is touched to make it.
pub async fn refresh_candidates(
    db: &Db,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<(String, MediaKind)>> {
    const ELIGIBLE: &str = "SELECT id, kind FROM media_item
         WHERE is_enabled = 1 AND is_manual = 0
           AND EXISTS (SELECT 1 FROM media_external_id e WHERE e.media_id = media_item.id)";

    let rows = match after {
        Some(after) => {
            sqlx::query(db.sql(&format!("{ELIGIBLE} AND id > ? ORDER BY id LIMIT ?")))
                .bind(after)
                .bind(limit.clamp(1, 500))
                .fetch_all(db.pool())
                .await?
        }
        None => {
            sqlx::query(db.sql(&format!("{ELIGIBLE} ORDER BY id LIMIT ?")))
                .bind(limit.clamp(1, 500))
                .fetch_all(db.pool())
                .await?
        }
    };

    rows.iter()
        .map(|row| Ok((row.text("id")?, row.text("kind")?.parse()?)))
        .collect()
}

/// How many works a refresh of everything has before it.
pub async fn count_refresh_candidates(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS n FROM media_item
         WHERE is_enabled = 1 AND is_manual = 0
           AND EXISTS (SELECT 1 FROM media_external_id e WHERE e.media_id = media_item.id)",
    ))
    .fetch_one(db.pool())
    .await?;

    Ok(row.big("n")?)
}

/// Works whose `refresh_after` has passed, oldest first.
///
/// Manual-only entries are excluded: with no external id there is nothing to
/// refresh from.
pub async fn due_for_refresh(db: &Db, limit: i64) -> Result<Vec<(String, MediaKind)>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, kind FROM media_item
         WHERE is_enabled = 1
           AND refresh_after IS NOT NULL
           AND refresh_after <= ?
           AND EXISTS (SELECT 1 FROM media_external_id e WHERE e.media_id = media_item.id)
         ORDER BY refresh_after ASC
         LIMIT ?",
    ))
    .bind(now())
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| Ok((row.text("id")?, row.text("kind")?.parse()?)))
        .collect()
}

pub async fn mark_refreshed(
    db: &Db,
    id: &str,
    next_refresh: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    // A refresh that failed changed nothing: `updated_at` stays, so that
    // `/v1/movie/changed` does not name the film to Radarr for nothing.
    let stamp = now();
    if error.is_some() {
        sqlx::query(db.sql(
            "UPDATE media_item
             SET refreshed_at = ?, refresh_after = ?, refresh_error = ?
             WHERE id = ?",
        ))
        .bind(&stamp)
        .bind(next_refresh)
        .bind(error)
        .bind(id)
        .execute(db.pool())
        .await?;
    } else {
        sqlx::query(db.sql(
            "UPDATE media_item
             SET refreshed_at = ?, refresh_after = ?, refresh_error = NULL, updated_at = ?
             WHERE id = ?",
        ))
        .bind(&stamp)
        .bind(next_refresh)
        .bind(&stamp)
        .bind(id)
        .execute(db.pool())
        .await?;
    }

    Ok(())
}
