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

/// Episode text for one language, for several works at once: by work, then
/// by `(season, episode)`. A work with none held in the language is absent.
pub async fn for_episodes_of(
    db: &Db,
    media_ids: &[String],
    language: &str,
) -> Result<HashMap<String, HashMap<(i32, i32), EpisodeText>>> {
    let mut out: HashMap<String, HashMap<(i32, i32), EpisodeText>> = HashMap::new();

    // Within every engine's limit on bound values.
    for chunk in media_ids.chunks(400) {
        let sql = format!(
            "SELECT media_id, season_number, episode_number, title, overview
             FROM media_episode_translation WHERE language = ? AND media_id IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut query = sqlx::query(db.sql(&sql)).bind(language);
        for id in chunk {
            query = query.bind(id);
        }

        for row in query.fetch_all(db.pool()).await? {
            let text = EpisodeText {
                season_number: row.int("season_number")?,
                episode_number: row.int("episode_number")?,
                title: row.opt_text("title")?,
                overview: row.opt_text("overview")?,
            };
            out.entry(row.text("media_id")?)
                .or_default()
                .insert((text.season_number, text.episode_number), text);
        }
    }

    Ok(out)
}

/// A language tried for a work since its last refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tried {
    pub language: String,
    /// When it is to be tried again — it is due once this is past — set by
    /// [`mark_unanswered`]; `None` for one fetched in full.
    pub retry_after: Option<String>,
}

/// Every language tried for a work since its last refresh: whether one is
/// due again, and what bounds how many a work is fetched in
/// (`service::language::MOST_LANGUAGES`).
pub async fn tried(db: &Db, media_id: &str) -> Result<Vec<Tried>> {
    let rows = sqlx::query(db.sql(
        "SELECT language, retry_after FROM media_language_fetch WHERE media_id = ?
         ORDER BY language",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(Tried {
                language: row.text("language")?,
                retry_after: row.opt_text("retry_after")?,
            })
        })
        .collect()
}

/// Store the episode text the providers gave in one language.
///
/// An episode they gave text for has it replaced, field by field: a field
/// they left empty keeps what is held, and an episode they said nothing
/// about keeps all of it. This deleted the language's text and wrote the
/// answer in its place, which made providers out of reach the same as
/// providers with nothing to say: an empty answer erased the language. Only
/// a `whole` answer — every provider asked gave one — replaces the
/// language's text outright, dropping what it no longer names; and an answer
/// with nothing in it changes nothing, whole or not.
pub async fn put_episodes(
    db: &Db,
    media_id: &str,
    language: &str,
    episodes: &[EpisodeText],
    whole: bool,
) -> Result<()> {
    // Nothing to say in this language is not worth a row, nor a reason to
    // drop one.
    let said: Vec<&EpisodeText> = episodes
        .iter()
        .filter(|e| e.title.is_some() || e.overview.is_some())
        .collect();
    if said.is_empty() {
        return Ok(());
    }

    let mut tx = db.begin_write().await?;

    if whole {
        sqlx::query(
            db.sql("DELETE FROM media_episode_translation WHERE media_id = ? AND language = ?"),
        )
        .bind(media_id)
        .bind(language)
        .execute(&mut *tx)
        .await?;
    }

    let at = now();

    for episode in said {
        sqlx::query(db.sql(
            "INSERT INTO media_episode_translation
                 (media_id, season_number, episode_number, language, title, overview, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (media_id, season_number, episode_number, language) DO UPDATE SET
                 title = COALESCE(excluded.title, media_episode_translation.title),
                 overview = COALESCE(excluded.overview, media_episode_translation.overview),
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

/// Record that a language has been fetched in full for a work.
///
/// Without this it would be fetched again on every single request. Whatever
/// wait [`mark_unanswered`] set is over.
pub async fn mark_fetched(db: &Db, media_id: &str, language: &str) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO media_language_fetch (media_id, language, fetched_at, retry_after)
         VALUES (?, ?, ?, NULL)
         ON CONFLICT (media_id, language) DO UPDATE SET
             fetched_at = excluded.fetched_at,
             retry_after = NULL",
    ))
    .bind(media_id)
    .bind(language)
    .bind(now())
    .execute(db.pool())
    .await?;

    Ok(())
}

/// Record that fetching a language for a work came to less than a whole
/// answer, and when it is tried again.
///
/// Not a fetch: the language is due again at `retry_after`, so what the
/// providers did not give is asked for once more. Not before, though — a
/// provider out of reach would otherwise be waited on by every request in
/// the language.
pub async fn mark_unanswered(
    db: &Db,
    media_id: &str,
    language: &str,
    retry_after: &str,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO media_language_fetch (media_id, language, fetched_at, retry_after)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (media_id, language) DO UPDATE SET
             fetched_at = excluded.fetched_at,
             retry_after = excluded.retry_after",
    ))
    .bind(media_id)
    .bind(language)
    .bind(now())
    .bind(retry_after)
    .execute(db.pool())
    .await?;

    Ok(())
}

/// Forget which languages were fetched or tried, so a refresh picks them up
/// again.
pub async fn clear_fetched(db: &Db, media_id: &str) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_language_fetch WHERE media_id = ?"))
        .bind(media_id)
        .execute(db.pool())
        .await?;

    Ok(())
}
