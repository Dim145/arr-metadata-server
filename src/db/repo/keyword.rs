//! Keywords, each kept once for every work it is given, and which works
//! carry which.
//!
//! TMDB's, by its id: the work's own list of keywords — every source's
//! texts, AniList's tags among them — stays what it was, and these double
//! TMDB's part of it with the ids its pages and filters go by. A work's list
//! is written again whole by a refresh that gives one; a keyword's name is
//! TMDB's latest.

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};

use crate::{
    db::{Db, RowExt, new_id, now},
    domain::Keyword,
};

/// Keywords read or written in one statement: well inside both engines'
/// ceilings on bound values.
const CHUNK: usize = 200;

fn map(row: &sqlx::any::AnyRow) -> Result<Keyword> {
    Ok(Keyword {
        id: row.text("id")?,
        tmdb_id: row.opt_big("tmdb_id")?,
        name: row.text("name")?,
    })
}

/// One keyword, by this server's id.
pub async fn get(db: &Db, id: &str) -> Result<Option<Keyword>> {
    let row = sqlx::query(db.sql("SELECT id, tmdb_id, name FROM keyword WHERE id = ?"))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

/// One keyword, by TMDB's id for it.
#[cfg(test)]
pub async fn by_tmdb(db: &Db, tmdb_id: i64) -> Result<Option<Keyword>> {
    let row = sqlx::query(db.sql("SELECT id, tmdb_id, name FROM keyword WHERE tmdb_id = ?"))
        .bind(tmdb_id)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

/// The keywords of one work, in their source's order.
pub async fn of_work(db: &Db, media_id: &str) -> Result<Vec<Keyword>> {
    let rows = sqlx::query(db.sql(
        "SELECT k.id, k.tmdb_id, k.name
           FROM media_keyword l JOIN keyword k ON k.id = l.keyword_id
          WHERE l.media_id = ?
          ORDER BY l.sort_order",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

/// Write a work's keywords in place of those it had, each keyword found
/// again by TMDB's id or added, its name TMDB's latest. One without an id
/// is found by its name alone. In the work's own write.
pub async fn replace(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_id: &str,
    keywords: &[Keyword],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_keyword WHERE media_id = ?"))
        .bind(media_id)
        .execute(&mut **tx)
        .await?;

    let given: Vec<&Keyword> = keywords
        .iter()
        .filter(|k| !k.name.trim().is_empty())
        .collect();
    if given.is_empty() {
        return Ok(());
    }

    // Every keyword with an id: added where new — two writes adding one at
    // once meet on its id, and the second takes the first's — renamed where
    // TMDB renamed it, then read back by id.
    let at = now();
    let with_id: Vec<(i64, &str)> = given
        .iter()
        .filter_map(|k| Some((k.tmdb_id?, k.name.trim())))
        .collect();
    for chunk in with_id.chunks(CHUNK) {
        let sql = format!(
            "INSERT INTO keyword (id, tmdb_id, name, created_at, updated_at) VALUES {}
             ON CONFLICT DO NOTHING",
            vec!["(?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for (tmdb_id, name) in chunk {
            add(&mut args, new_id())?;
            add(&mut args, *tmdb_id)?;
            add(&mut args, name.to_string())?;
            add(&mut args, at.clone())?;
            add(&mut args, at.clone())?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to add keywords")?;
    }
    let mut held: HashMap<i64, (String, String)> = HashMap::new();
    for chunk in with_id.chunks(CHUNK) {
        let sql = format!(
            "SELECT id, tmdb_id, name FROM keyword WHERE tmdb_id IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for (tmdb_id, _) in chunk {
            add(&mut args, *tmdb_id)?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(&mut **tx)
            .await?
        {
            held.insert(row.big("tmdb_id")?, (row.text("id")?, row.text("name")?));
        }
    }
    for (tmdb_id, name) in &with_id {
        if let Some((id, kept)) = held.get(tmdb_id)
            && kept != name
        {
            sqlx::query(db.sql("UPDATE keyword SET name = ?, updated_at = ? WHERE id = ?"))
                .bind(*name)
                .bind(&at)
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
    }

    let mut ids: Vec<String> = Vec::with_capacity(given.len());
    for keyword in &given {
        let id = match keyword.tmdb_id {
            Some(tmdb_id) => held.get(&tmdb_id).map(|(id, _)| id.clone()),
            None => Some(by_name(db, tx, keyword.name.trim(), &at).await?),
        };
        if let Some(id) = id.filter(|id| !ids.contains(id)) {
            ids.push(id);
        }
    }

    for (chunk_at, chunk) in ids.chunks(CHUNK).enumerate() {
        let sql = format!(
            "INSERT INTO media_keyword (media_id, keyword_id, sort_order, source) VALUES {}
             ON CONFLICT (media_id, keyword_id) DO NOTHING",
            vec!["(?, ?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for (at, id) in chunk.iter().enumerate() {
            add(&mut args, media_id.to_string())?;
            add(&mut args, id.clone())?;
            add(
                &mut args,
                i32::try_from(chunk_at * CHUNK + at).unwrap_or(i32::MAX),
            )?;
            add(&mut args, crate::providers::names::TMDB.to_string())?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to write a work's keywords")?;
    }
    Ok(())
}

/// A keyword with no id, found by its name among those with none, or added.
async fn by_name(db: &Db, tx: &mut Transaction<'_, Any>, name: &str, at: &str) -> Result<String> {
    let found: Option<String> = sqlx::query_scalar(db.sql(
        "SELECT id FROM keyword WHERE tmdb_id IS NULL AND name = ? ORDER BY created_at LIMIT 1",
    ))
    .bind(name)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(found) = found {
        return Ok(found);
    }
    let id = new_id();
    sqlx::query(db.sql(
        "INSERT INTO keyword (id, tmdb_id, name, created_at, updated_at)
         VALUES (?, NULL, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(name)
    .bind(at)
    .bind(at)
    .execute(&mut **tx)
    .await
    .context("failed to add a keyword")?;
    Ok(id)
}

/// Bind one more value to a statement being built.
fn add<'t>(
    args: &mut AnyArguments,
    value: impl sqlx::Encode<'t, Any> + sqlx::Type<Any>,
) -> Result<()> {
    args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
}
