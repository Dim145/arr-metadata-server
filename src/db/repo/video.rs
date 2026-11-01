//! A work's videos: every one its sources list — trailers, teasers, clips,
//! featurettes — where each is and what it is.
//!
//! Wholly what the sources said: a refresh that gives a list writes it again
//! whole, and nothing is entered by hand. The trailer a work is sent to
//! clients with stays the one chosen of them (`trailer_youtube_id`), which a
//! person may lock.

use anyhow::{Context, Result};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};

use crate::{
    db::{Db, RowExt, from_bool, new_id, now},
    domain::Video,
};

/// Videos written in one statement, at fifteen values each.
const CHUNK: usize = 100;

/// One work's videos, in their order.
pub async fn of_work(db: &Db, media_id: &str) -> Result<Vec<Video>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, site, video_key, name, video_type, language, country, official,
                published_at, size, tmdb_id, source, sort_order
           FROM media_video WHERE media_id = ? ORDER BY sort_order",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(Video {
                id: row.text("id")?,
                site: row.text("site")?,
                key: row.text("video_key")?,
                name: row.opt_text("name")?,
                video_type: row.opt_text("video_type")?,
                language: row.opt_text("language")?,
                country: row.opt_text("country")?,
                official: row.flag("official")?,
                published_at: row.opt_text("published_at")?,
                size: row.opt_int("size")?,
                tmdb_id: row.opt_text("tmdb_id")?,
                source: row.opt_text("source")?,
                sort_order: row.int("sort_order")?,
            })
        })
        .collect()
}

/// Write a work's videos in place of those it had, in the work's own write.
/// One listed twice — the same site and key — is written once.
pub async fn replace(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_id: &str,
    videos: &[Video],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_video WHERE media_id = ?"))
        .bind(media_id)
        .execute(&mut **tx)
        .await?;

    let created = now();
    for chunk in videos.chunks(CHUNK) {
        let sql = format!(
            "INSERT INTO media_video
                 (id, media_id, site, video_key, name, video_type, language, country, official,
                  published_at, size, tmdb_id, source, sort_order, created_at)
             VALUES {}
             ON CONFLICT (media_id, site, video_key) DO NOTHING",
            vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for video in chunk {
            add(
                &mut args,
                if video.id.is_empty() {
                    new_id()
                } else {
                    video.id.clone()
                },
            )?;
            add(&mut args, media_id.to_string())?;
            add(&mut args, video.site.clone())?;
            add(&mut args, video.key.clone())?;
            add(&mut args, video.name.clone())?;
            add(&mut args, video.video_type.clone())?;
            add(&mut args, video.language.clone())?;
            add(&mut args, video.country.clone())?;
            add(&mut args, from_bool(video.official))?;
            add(&mut args, video.published_at.clone())?;
            add(&mut args, video.size)?;
            add(&mut args, video.tmdb_id.clone())?;
            add(&mut args, video.source.clone())?;
            add(&mut args, video.sort_order)?;
            add(&mut args, created.clone())?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to write a work's videos")?;
    }
    Ok(())
}

/// Bind one more value to a statement being built.
fn add<'t>(
    args: &mut AnyArguments,
    value: impl sqlx::Encode<'t, Any> + sqlx::Type<Any>,
) -> Result<()> {
    args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
}
