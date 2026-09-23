//! Per-language text.
//!
//! A work's own title and overview live in `media_translation`; its episodes'
//! live in `media_episode_translation`. They are separate because they are
//! fetched separately — see the migration — and because there are far more of
//! the second kind.

use anyhow::Result;
use std::collections::HashMap;

use crate::db::{Db, RowExt, now};

/// One episode's text in one language.
#[derive(Clone, Debug)]
pub struct EpisodeText {
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
}

/// Episode text for one language, keyed by `(season, episode)`.
pub async fn for_episodes(
    db: &Db,
    media_id: &str,
    language: &str,
) -> Result<HashMap<(i32, i32), EpisodeText>> {
    let rows = sqlx::query(db.sql(
        "SELECT season_number, episode_number, title, overview
         FROM media_episode_translation WHERE media_id = ? AND language = ?",
    ))
    .bind(media_id)
    .bind(language)
    .fetch_all(db.pool())
    .await?;

    let mut out = HashMap::with_capacity(rows.len());

    for row in &rows {
        let text = EpisodeText {
            season_number: row.int("season_number")?,
            episode_number: row.int("episode_number")?,
            title: row.opt_text("title")?,
            overview: row.opt_text("overview")?,
        };
        out.insert((text.season_number, text.episode_number), text);
    }

    Ok(out)
}

/// Replace the stored episode text for one language.
pub async fn put_episodes(
    db: &Db,
    media_id: &str,
    language: &str,
    episodes: &[EpisodeText],
) -> Result<()> {
    let mut tx = db.begin_write().await?;

    sqlx::query(
        db.sql("DELETE FROM media_episode_translation WHERE media_id = ? AND language = ?"),
    )
    .bind(media_id)
    .bind(language)
    .execute(&mut *tx)
    .await?;

    let at = now();

    for episode in episodes {
        // Nothing to say in this language is not worth a row.
        if episode.title.is_none() && episode.overview.is_none() {
            continue;
        }

        sqlx::query(db.sql(
            "INSERT INTO media_episode_translation
                 (media_id, season_number, episode_number, language, title, overview, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (media_id, season_number, episode_number, language) DO UPDATE SET
                 title = excluded.title,
                 overview = excluded.overview,
                 fetched_at = excluded.fetched_at",
        ))
        .bind(media_id)
        .bind(episode.season_number)
        .bind(episode.episode_number)
        .bind(language)
        .bind(&episode.title)
        .bind(&episode.overview)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Record that a language has been fetched for a work.
///
/// Without this a language the provider genuinely has nothing for would be
/// refetched on every single request.
pub async fn mark_fetched(db: &Db, media_id: &str, language: &str) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO media_language_fetch (media_id, language, fetched_at)
         VALUES (?, ?, ?)
         ON CONFLICT (media_id, language) DO UPDATE SET fetched_at = excluded.fetched_at",
    ))
    .bind(media_id)
    .bind(language)
    .bind(now())
    .execute(db.pool())
    .await?;

    Ok(())
}

pub async fn was_fetched(db: &Db, media_id: &str, language: &str) -> Result<bool> {
    let row = sqlx::query(
        db.sql("SELECT 1 AS present FROM media_language_fetch WHERE media_id = ? AND language = ?"),
    )
    .bind(media_id)
    .bind(language)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.is_some())
}

/// Forget which languages were fetched, so a refresh picks them up again.
pub async fn clear_fetched(db: &Db, media_id: &str) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_language_fetch WHERE media_id = ?"))
        .bind(media_id)
        .execute(db.pool())
        .await?;

    Ok(())
}
