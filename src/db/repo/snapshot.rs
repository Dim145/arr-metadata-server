//! Raw provider payloads.
//!
//! A snapshot is what a provider actually returned, kept verbatim. Refreshes
//! replace it; the canonical entity is re-derived from the set of snapshots by
//! [`crate::merge`]. Keeping the raw form means a mapping bug can be fixed and
//! replayed without re-fetching everything.

use anyhow::Result;
use serde_json::Value;

use crate::db::{Db, RowExt, now};

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub provider: String,
    pub payload: Value,
    pub fetched_at: String,
    pub etag: Option<String>,
}

/// Keep what `provider` answered for a work: `payload` is the document as
/// JSON text, which is what is written.
pub async fn put(
    db: &Db,
    media_id: &str,
    provider: &str,
    payload: &str,
    etag: Option<&str>,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO media_provider_snapshot (media_id, provider, payload, fetched_at, etag)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (media_id, provider) DO UPDATE SET
             payload = excluded.payload,
             fetched_at = excluded.fetched_at,
             etag = excluded.etag",
    ))
    .bind(media_id)
    .bind(provider)
    .bind(payload)
    .bind(now())
    .bind(etag)
    .execute(db.pool())
    .await?;

    Ok(())
}

/// Every snapshot for a work, newest first.
pub async fn list(db: &Db, media_id: &str) -> Result<Vec<Snapshot>> {
    let rows = sqlx::query(db.sql(
        "SELECT provider, payload, fetched_at, etag
         FROM media_provider_snapshot WHERE media_id = ? ORDER BY fetched_at DESC",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(map).collect()
}

/// Which providers have answered for a work, and when, without their
/// documents: a long series' run to megabytes.
pub async fn fetched(db: &Db, media_id: &str) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        db.sql("SELECT provider, fetched_at FROM media_provider_snapshot WHERE media_id = ?"),
    )
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| Ok((row.text("provider")?, row.text("fetched_at")?)))
        .collect()
}

/// What some providers answered for a work, by provider, as the text it was
/// kept as: for a reader after a few of its fields, which parses those alone
/// rather than the whole document — a film's from TMDB runs to a few hundred
/// kilobytes.
pub async fn texts(db: &Db, media_id: &str, providers: &[&str]) -> Result<Vec<(String, String)>> {
    if providers.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT provider, payload FROM media_provider_snapshot
         WHERE media_id = ? AND provider IN ({})",
        vec!["?"; providers.len()].join(", ")
    );
    let mut query = sqlx::query(db.sql(&sql)).bind(media_id);
    for provider in providers {
        query = query.bind(*provider);
    }

    query
        .fetch_all(db.pool())
        .await?
        .iter()
        .map(|row| Ok((row.text("provider")?, row.text("payload")?)))
        .collect()
}

fn map(row: &sqlx::any::AnyRow) -> Result<Snapshot> {
    Ok(Snapshot {
        provider: row.text("provider")?,
        // A payload that no longer parses is surfaced as JSON null rather than
        // failing the read; the next refresh overwrites it.
        payload: serde_json::from_str(&row.text("payload")?).unwrap_or(Value::Null),
        fetched_at: row.text("fetched_at")?,
        etag: row.opt_text("etag")?,
    })
}
