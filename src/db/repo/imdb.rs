//! IMDb's ratings, for the works this catalogue holds.
//!
//! Filled wholesale from IMDb's `title.ratings` dataset by
//! [`crate::jobs::datasets`], and read back when a work is loaded.

use anyhow::Result;
use sqlx::{Arguments as _, any::AnyArguments};

use crate::db::{Db, RowExt};

/// One title's rating, as IMDb publishes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Rating {
    pub tconst: String,
    pub rating: f64,
    pub votes: i64,
}

/// Rows per statement: well inside every engine's bind-parameter ceiling.
const CHUNK: usize = 300;

/// Replace every stored rating with `ratings`, in one transaction.
pub async fn replace_all(db: &Db, ratings: &[Rating]) -> Result<()> {
    let mut tx = db.begin_write().await?;

    sqlx::query(db.sql("DELETE FROM imdb_rating"))
        .execute(&mut *tx)
        .await?;

    for chunk in ratings.chunks(CHUNK) {
        let sql = format!(
            "INSERT INTO imdb_rating (tconst, rating, votes) VALUES {}",
            vec!["(?, ?, ?)"; chunk.len()].join(", ")
        );

        let mut args = AnyArguments::default();
        for r in chunk {
            for result in [
                args.add(r.tconst.clone()),
                args.add(r.rating),
                args.add(r.votes),
            ] {
                result.map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }

        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// The rating IMDb gives one title, if it gives one.
pub async fn get(db: &Db, tconst: &str) -> Result<Option<Rating>> {
    let row = sqlx::query(db.sql("SELECT tconst, rating, votes FROM imdb_rating WHERE tconst = ?"))
        .bind(tconst)
        .fetch_optional(db.pool())
        .await?;

    row.map(|r| {
        Ok(Rating {
            tconst: r.text("tconst")?,
            rating: r.opt_real("rating")?.unwrap_or_default(),
            votes: r.big("votes")?,
        })
    })
    .transpose()
}

/// The ratings IMDb gives several titles, by title, in one query.
pub async fn get_many(
    db: &Db,
    tconsts: &[String],
) -> Result<std::collections::HashMap<String, Rating>> {
    if tconsts.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let sql = format!(
        "SELECT tconst, rating, votes FROM imdb_rating WHERE tconst IN ({})",
        vec!["?"; tconsts.len()].join(", ")
    );

    let mut args = AnyArguments::default();
    for tconst in tconsts {
        args.add(tconst.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|r| {
            let rating = Rating {
                tconst: r.text("tconst")?,
                rating: r.opt_real("rating")?.unwrap_or_default(),
                votes: r.big("votes")?,
            };
            Ok((rating.tconst.clone(), rating))
        })
        .collect()
}

/// Whether a work with an IMDb id was first stored after `since`.
///
/// By the work's creation, which a refresh preserves, and not by its ids' —
/// those are rewritten on every refresh, which would make every sweep look
/// like new works.
pub async fn added_since(db: &Db, since: &str) -> Result<bool> {
    let row = sqlx::query(db.sql(
        "SELECT 1 AS found FROM media_item m
         JOIN media_external_id e ON e.media_id = m.id
         WHERE e.source = 'imdb' AND m.created_at > ?
         LIMIT 1",
    ))
    .bind(since)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.is_some())
}

/// Every IMDb id this catalogue holds a work for: what an import keeps.
pub async fn wanted(db: &Db) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql("SELECT value FROM media_external_id WHERE source = 'imdb'"))
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|r| r.text("value").map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[tokio::test]
    async fn ratings_are_replaced_and_read_back() {
        let db = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        db.migrate().await.unwrap();

        let rating = |tconst: &str, rating: f64, votes: i64| Rating {
            tconst: tconst.into(),
            rating,
            votes,
        };

        replace_all(&db, &[rating("tt0000001", 5.7, 2100)])
            .await
            .unwrap();
        replace_all(&db, &[rating("tt0903747", 9.5, 2_679_470)])
            .await
            .unwrap();

        assert_eq!(get(&db, "tt0000001").await.unwrap(), None);
        assert_eq!(
            get(&db, "tt0903747").await.unwrap(),
            Some(rating("tt0903747", 9.5, 2_679_470))
        );
        assert!(wanted(&db).await.unwrap().is_empty());
    }
}
