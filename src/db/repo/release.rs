//! A work's releases and ratings, country by country.
//!
//! Wholly what the providers said, like its ratings: a refresh replaces them,
//! and nothing is entered by hand. A row a release — the country, how the
//! work came out there, the day, the rating it was given — and a row for a
//! rating given on its own, a series' or Radarr's, with neither kind nor day.

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::{Any, Transaction};

use crate::{
    db::{Db, RowExt, new_id},
    domain::{CountryRelease, ReleaseKind},
};

/// The most a work keeps. TMDB lists a hundred and more for a film shown
/// everywhere, and every one is a row; the answer comes from an address an
/// operator can point elsewhere.
pub const MAX_RELEASES: usize = 1000;

/// The most films one window of the schedule finds by their country's days.
const MAX_FOUND: i64 = 500;

fn map(row: &sqlx::any::AnyRow) -> Result<CountryRelease> {
    Ok(CountryRelease {
        country: row.text("country")?,
        // A kind this version does not know is a release of no known kind,
        // not a failed read.
        release_type: row
            .opt_text("release_type")?
            .and_then(|kind| kind.parse::<ReleaseKind>().ok()),
        date: row.opt_text("release_date")?,
        certification: row.opt_text("certification")?,
        note: row.opt_text("note")?,
        source: row.opt_text("source")?,
    })
}

/// One work's, in the order they were given.
pub async fn load(db: &Db, media_id: &str) -> Result<Vec<CountryRelease>> {
    let rows = sqlx::query(db.sql(
        "SELECT country, release_type, release_date, certification, note, source
         FROM media_release WHERE media_id = ? ORDER BY sort_order",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(map).collect()
}

/// Several works' at once, by work: the films of a window of the schedule.
pub async fn load_many(
    db: &Db,
    media_ids: &[String],
) -> Result<HashMap<String, Vec<CountryRelease>>> {
    let mut out: HashMap<String, Vec<CountryRelease>> = HashMap::new();

    // Within every engine's limit on bound values.
    for chunk in media_ids.chunks(400) {
        let sql = format!(
            "SELECT media_id, country, release_type, release_date, certification, note, source
             FROM media_release WHERE media_id IN ({})
             ORDER BY media_id, sort_order",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut query = sqlx::query(db.sql(&sql));
        for id in chunk {
            query = query.bind(id);
        }

        for row in query.fetch_all(db.pool()).await? {
            out.entry(row.text("media_id")?)
                .or_default()
                .push(map(&row)?);
        }
    }

    Ok(out)
}

/// Put a work's in place of what it held, in the write that stores it.
pub async fn replace(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_id: &str,
    releases: &[CountryRelease],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_release WHERE media_id = ?"))
        .bind(media_id)
        .execute(&mut **tx)
        .await
        .context("failed to clear the releases")?;

    for (order, release) in releases.iter().take(MAX_RELEASES).enumerate() {
        sqlx::query(db.sql(
            "INSERT INTO media_release
                 (id, media_id, country, release_type, release_date, certification,
                  note, source, sort_order)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        ))
        .bind(new_id())
        .bind(media_id)
        .bind(release.country.to_ascii_uppercase())
        .bind(release.release_type.map(|kind| kind.as_str().to_string()))
        .bind(&release.date)
        .bind(&release.certification)
        .bind(&release.note)
        .bind(&release.source)
        .bind(i32::try_from(order).unwrap_or(i32::MAX))
        .execute(&mut **tx)
        .await
        .context("failed to write a release")?;
    }

    Ok(())
}

/// The films that come out in a country within a window — in its cinemas,
/// digitally or on disc there — whatever their own dates say: what the
/// schedule adds to the films it finds by those, for a reader of that
/// country. A release is read by its day at midnight UTC, as everywhere
/// else; `from` and `to` are instants written `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Any release of the window finds its film, a re-release included: the
/// caller lays each film's first days over its own and keeps the releases
/// they leave in the window.
pub async fn releasing_in(
    db: &Db,
    country: &str,
    from: &str,
    to: &str,
    include_adult: bool,
) -> Result<Vec<String>> {
    let adult = if include_adult {
        ""
    } else {
        " AND m.is_adult = 0"
    };

    let rows = sqlx::query(db.sql(&format!(
        "SELECT DISTINCT r.media_id FROM media_release r
         JOIN media_item m ON m.id = r.media_id
         WHERE r.country = ?
           AND r.release_type IN ('limited', 'theatrical', 'digital', 'physical')
           AND SUBSTR(r.release_date, 1, 10) || 'T00:00:00Z' >= ?
           AND SUBSTR(r.release_date, 1, 10) || 'T00:00:00Z' < ?
           AND m.kind = 'movie' AND m.is_enabled = 1{adult}
         LIMIT ?"
    )))
    .bind(country.to_ascii_uppercase())
    .bind(from)
    .bind(to)
    .bind(MAX_FOUND)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| row.text("media_id").map_err(Into::into))
        .collect()
}
