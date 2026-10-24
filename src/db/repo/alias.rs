//! What a merged work was known by, and the work that answers for it now.
//!
//! A merge keeps one of two records of a work and leaves the other as a
//! tombstone (`repo::item::merge`). Every client kept an id of the one that
//! went — Sonarr TheTVDB's, Radarr TMDB's, Plex its rating key, a reader the
//! site's address — so each of them is filed here against the survivor: the
//! tombstone's own id (`id`), its slug (`slug`), and each identifier it had
//! elsewhere, under the source `media_external_id` files it under. They are
//! kept for ever; only the undo of their merge takes them back.
//!
//! An alias names the work that survived its merge, which may have been
//! merged in its turn: whoever reads one follows the survivors until a work
//! that is still one of its own.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Any, Transaction};
use utoipa::ToSchema;

use crate::{
    db::{Db, RowExt, now},
    domain::ExternalSource,
};

/// The source a tombstone's own id is filed under.
pub const ID: &str = "id";

/// The source a tombstone's address is filed under.
pub const SLUG: &str = "slug";

/// How many survivors one address is followed through, at the most: far more
/// than merges of merges ever chain, few enough that a cycle written by hand
/// in the table ends.
const MOST_HOPS: usize = 16;

/// One name a merged work went by, as a reader of the survivor is told it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = WorkAlias)]
pub struct Alias {
    /// `id`, `slug`, or the source of an identifier elsewhere: `tvdb_series`,
    /// `tmdb_movie`, `imdb`…
    pub source: String,
    pub value: String,
    /// The work that answers for it now.
    pub media_id: String,
    /// The work it named, kept as a tombstone.
    pub merged_id: String,
    pub created_at: String,
}

/// The work an alias leads to now, or none: the survivor it names, and that
/// one's survivor in turn when it was merged since.
pub async fn survivor(db: &Db, source: &str, value: &str) -> Result<Option<String>> {
    let Some(first) = lookup(db, source, value).await? else {
        return Ok(None);
    };
    follow(db, first).await.map(Some)
}

/// The work an id the site or the API was asked by answers with, when a merge
/// folded it into another: the tombstone's own id, or its address. None for
/// an id that is a work of its own, or no work at all. A work's id is a UUID,
/// which a slug never is: one look, not two.
pub async fn for_address(db: &Db, address: &str) -> Result<Option<String>> {
    let source = match uuid::Uuid::try_parse(address) {
        Ok(_) => ID,
        Err(_) => SLUG,
    };
    survivor(db, source, address).await
}

/// The work an identifier elsewhere leads to through an alias, when no work
/// holds it: what a lookup by that identifier answers with once the work
/// holding it was merged into another. The caller has looked for a holder.
pub async fn for_external(db: &Db, source: ExternalSource, value: &str) -> Result<Option<String>> {
    survivor(db, source.as_str(), value).await
}

/// The work an identifier elsewhere names: the one holding it — or, should
/// that be a tombstone, the work it was merged into — else the survivor of
/// the one that held it.
pub async fn work_for_external(
    db: &Db,
    source: ExternalSource,
    value: &str,
) -> Result<Option<String>> {
    match crate::db::repo::item::find_id_by_external(db, source, value).await? {
        Some(holder) => Ok(Some(survivor(db, ID, &holder).await?.unwrap_or(holder))),
        None => for_external(db, source, value).await,
    }
}

/// Follow a work through the merges it went into since, to one that is a
/// work of its own.
async fn follow(db: &Db, mut at: String) -> Result<String> {
    for _ in 0..MOST_HOPS {
        match lookup(db, ID, &at).await? {
            Some(next) if next != at => at = next,
            _ => return Ok(at),
        }
    }
    Ok(at)
}

async fn lookup(db: &Db, source: &str, value: &str) -> Result<Option<String>> {
    let row =
        sqlx::query(db.sql("SELECT media_id FROM media_alias WHERE source = ? AND value = ?"))
            .bind(source)
            .bind(value)
            .fetch_optional(db.pool())
            .await?;
    row.map(|row| row.text("media_id"))
        .transpose()
        .map_err(Into::into)
}

/// Every name the works merged into one went by, newest merge first: what the
/// survivor answers to besides its own.
pub async fn of_work(db: &Db, media_id: &str) -> Result<Vec<Alias>> {
    let rows = sqlx::query(db.sql(
        "SELECT source, value, media_id, merged_id, created_at FROM media_alias
          WHERE media_id = ? ORDER BY created_at DESC, merged_id, source, value",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(Alias {
                source: row.text("source")?,
                value: row.text("value")?,
                media_id: row.text("media_id")?,
                merged_id: row.text("merged_id")?,
                created_at: row.text("created_at")?,
            })
        })
        .collect()
}

/// File what a tombstone was known by against its survivor, in the merge's
/// own transaction. A name already filed — the address of another tombstone
/// of the other kind, which may share it — keeps the work it named first.
pub async fn write(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    merged_id: &str,
    media_id: &str,
    names: &[(String, String)],
) -> Result<()> {
    let at = now();
    for (source, value) in names {
        sqlx::query(db.sql(
            "INSERT INTO media_alias (source, value, media_id, merged_id, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (source, value) DO NOTHING",
        ))
        .bind(source)
        .bind(value)
        .bind(media_id)
        .bind(merged_id)
        .bind(&at)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("failed to file {source} {value} against the survivor"))?;
    }
    Ok(())
}

/// Take back what one merge filed, in the undo's own transaction. Returns how
/// many names went.
pub async fn remove(db: &Db, tx: &mut Transaction<'_, Any>, merged_id: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM media_alias WHERE merged_id = ?"))
        .bind(merged_id)
        .execute(&mut **tx)
        .await
        .context("failed to take back the names a merge filed")?;
    Ok(result.rows_affected())
}
