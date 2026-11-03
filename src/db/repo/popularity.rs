//! What the popularity of the works held here is worked out from, and the
//! figure it comes to (wave P, P6): see `service::popularity`.
//!
//! The figure is the column `media_item.local_popularity`, which the lists
//! sort by where TMDB gives a work none; what it was worked out from is
//! `media_popularity`, a row a work, written whole each time.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;
use sqlx::{Arguments as _, any::AnyArguments};

use crate::{
    db::{Db, Double, RowExt},
    domain::ExternalSource,
};

/// Works one statement reads or writes.
const CHUNK: usize = 200;

/// TVmaze's answers read at a time: a long series' runs to megabytes.
const ANSWERS: usize = 16;

fn add<'t>(
    args: &mut AnyArguments,
    value: impl sqlx::Encode<'t, sqlx::Any> + sqlx::Type<sqlx::Any>,
) -> Result<()> {
    args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
}

fn holes(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// A work held, as its popularity is worked out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Held {
    pub id: String,
    /// `series` or `movie`.
    pub kind: String,
    /// TMDB's popularity of it.
    pub popularity: Option<f64>,
    /// The popularity last worked out here.
    pub local: Option<f64>,
    pub tvmaze: Option<i64>,
    pub tvdb: Option<i64>,
    pub imdb: Option<String>,
}

/// Every work held — none merged into another — with its ids at TVmaze,
/// TheTVDB and IMDb.
pub async fn held(db: &Db) -> Result<Vec<Held>> {
    let mut works: HashMap<String, Held> = HashMap::new();
    for row in sqlx::query(db.sql(
        "SELECT id, kind, popularity, local_popularity FROM media_item
         WHERE merged_into IS NULL",
    ))
    .fetch_all(db.pool())
    .await?
    {
        let id = row.text("id")?;
        works.insert(
            id.clone(),
            Held {
                id,
                kind: row.text("kind")?,
                popularity: row.opt_real("popularity")?,
                local: row.opt_real("local_popularity")?,
                ..Held::default()
            },
        );
    }
    let (tvmaze, tvdb, imdb) = (
        ExternalSource::TvMaze.as_str(),
        ExternalSource::TvdbSeries.as_str(),
        ExternalSource::Imdb.as_str(),
    );
    for row in sqlx::query(
        db.sql("SELECT media_id, source, value FROM media_external_id WHERE source IN (?, ?, ?)"),
    )
    .bind(tvmaze)
    .bind(tvdb)
    .bind(imdb)
    .fetch_all(db.pool())
    .await?
    {
        let Some(work) = works.get_mut(&row.text("media_id")?) else {
            continue;
        };
        let value = row.text("value")?;
        let source = row.text("source")?;
        if source == tvmaze {
            work.tvmaze = value.trim().parse().ok();
        } else if source == tvdb {
            work.tvdb = value.trim().parse().ok();
        } else {
            work.imdb = Some(value);
        }
    }
    let mut works: Vec<Held> = works.into_values().collect();
    works.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    Ok(works)
}

/// Where a work's weight was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightFrom {
    /// TVmaze's index, as this server keeps it.
    Index,
    /// TVmaze's own answer for the series, kept with the work.
    Answer,
}

impl WeightFrom {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Answer => "answer",
        }
    }
}

/// What a work's popularity was worked out from, as `media_popularity` keeps
/// it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worked {
    pub media_id: String,
    /// The weight the figure was worked out from, and where it was read.
    pub weight: Option<(i32, WeightFrom)>,
    /// The weight TVmaze's own answer for the series gave, and when that
    /// answer was fetched: read again once a newer one is.
    pub answer_weight: Option<i32>,
    pub answer_at: Option<String>,
    pub votes_per_day: Option<f64>,
}

/// What each work's popularity was last worked out from, by work.
pub async fn worked(db: &Db) -> Result<HashMap<String, Worked>> {
    let rows = sqlx::query(db.sql(
        "SELECT media_id, weight, weight_from, answer_weight, answer_at, votes_per_day
         FROM media_popularity",
    ))
    .fetch_all(db.pool())
    .await?;
    let mut kept = HashMap::with_capacity(rows.len());
    for row in &rows {
        let from = match row.opt_text("weight_from")?.as_deref() {
            Some("index") => Some(WeightFrom::Index),
            Some("answer") => Some(WeightFrom::Answer),
            _ => None,
        };
        let weight = row
            .opt_big("weight")?
            .zip(from)
            .map(|(weight, from)| (weight.clamp(0, 100) as i32, from));
        let media_id = row.text("media_id")?;
        kept.insert(
            media_id.clone(),
            Worked {
                media_id,
                weight,
                answer_weight: row
                    .opt_big("answer_weight")?
                    .map(|w| w.clamp(0, 100) as i32),
                answer_at: row.opt_text("answer_at")?,
                votes_per_day: row.opt_real("votes_per_day")?,
            },
        );
    }
    Ok(kept)
}

/// When TVmaze's answer kept with each work was fetched, by work: without the
/// answers themselves.
pub async fn answered_at(db: &Db) -> Result<HashMap<String, String>> {
    let rows = sqlx::query(
        db.sql("SELECT media_id, fetched_at FROM media_provider_snapshot WHERE provider = ?"),
    )
    .bind(crate::providers::names::TVMAZE)
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| Ok((row.text("media_id")?, row.text("fetched_at")?)))
        .collect()
}

/// The weight in a TVmaze answer kept with a work: `{"show": {...}, ...}`,
/// the episodes beside it passed over unread.
#[derive(Deserialize)]
struct Answer {
    show: Option<AnswerShow>,
}

#[derive(Deserialize)]
struct AnswerShow {
    weight: Option<f64>,
}

/// The weight TVmaze's answer kept with each of these works gives, and when
/// that answer was fetched, by work: nothing within for an answer without
/// one, or one that does not read.
pub async fn answer_weights(
    db: &Db,
    ids: &[String],
) -> Result<HashMap<String, (Option<i32>, String)>> {
    let mut found = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(ANSWERS) {
        let sql = format!(
            "SELECT media_id, payload, fetched_at FROM media_provider_snapshot
             WHERE provider = ? AND media_id IN ({})",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        add(&mut args, crate::providers::names::TVMAZE.to_string())?;
        for id in chunk {
            add(&mut args, id.clone())?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            let weight = serde_json::from_str::<Answer>(&row.text("payload")?)
                .ok()
                .and_then(|answer| answer.show?.weight)
                .filter(|weight| weight.is_finite())
                .map(|weight| weight.round().clamp(0.0, 100.0) as i32);
            found.insert(row.text("media_id")?, (weight, row.text("fetched_at")?));
        }
    }
    Ok(found)
}

/// Keep what each work's popularity was worked out from, in place of what
/// was kept, and give the works in `moved` their new figure — nothing for one
/// that has none now — in one transaction. A work whose figure moved and that
/// has none of TMDB's is marked for the search index, whose rows carry the
/// figure in its place. How many figures moved.
pub async fn write(
    db: &Db,
    worked: &[Worked],
    moved: &[(String, Option<f64>)],
    at: &str,
) -> Result<usize> {
    let mut tx = db.begin_write().await?;
    sqlx::query(db.sql("DELETE FROM media_popularity"))
        .execute(&mut *tx)
        .await?;
    for chunk in worked.chunks(CHUNK) {
        let sql = format!(
            "INSERT INTO media_popularity (media_id, weight, weight_from, answer_weight,
                                           answer_at, votes_per_day, worked_at)
             VALUES {}",
            vec!["(?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for work in chunk {
            add(&mut args, work.media_id.clone())?;
            add(&mut args, work.weight.map(|(weight, _)| i64::from(weight)))?;
            add(
                &mut args,
                work.weight.map(|(_, from)| from.as_str().to_string()),
            )?;
            add(&mut args, work.answer_weight.map(i64::from))?;
            add(&mut args, work.answer_at.clone())?;
            add(&mut args, Double(work.votes_per_day))?;
            add(&mut args, at.to_string())?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut *tx)
            .await?;
    }
    for (id, figure) in moved {
        sqlx::query(db.sql(
            "UPDATE media_item SET local_popularity = ?,
                    search_stale = CASE WHEN popularity IS NULL THEN 1 ELSE search_stale END
              WHERE id = ?",
        ))
        .bind(Double(*figure))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(moved.len())
}

/// The popularity worked out here of each of these works that has one, by
/// work.
pub async fn local_of(db: &Db, ids: &[String]) -> Result<HashMap<String, f64>> {
    let mut found = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT id, local_popularity FROM media_item
              WHERE id IN ({}) AND local_popularity IS NOT NULL",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            add(&mut args, id.clone())?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            if let Some(figure) = row.opt_real("local_popularity")? {
                found.insert(row.text("id")?, figure);
            }
        }
    }
    Ok(found)
}
