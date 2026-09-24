//! Which AniList and MyAnimeList entries a series or a film is.
//!
//! Filled wholesale from the Fribb anime-lists project by
//! [`crate::jobs::datasets`], and only ever read here. See migration 0008 for
//! why one TheTVDB series is several rows.

use anyhow::Result;
use sqlx::{Arguments as _, any::AnyArguments};

use crate::db::{Db, RowExt};

/// One entry of the list: one AniList or MyAnimeList entry, and where it sits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub mal_id: Option<i64>,
    pub anilist_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    /// The TheTVDB season this entry is, when it is one. Absent for an entry
    /// that spans the whole series — *One Piece* is one entry, not twenty.
    pub tvdb_season: Option<i32>,
    /// How many TheTVDB episodes into that season this entry starts.
    pub tvdb_offset: Option<i32>,
    pub tmdb_movie: Option<i64>,
    /// `TV`, `MOVIE`, `OVA`, `ONA`, `SPECIAL`…
    pub kind: Option<String>,
}

/// Columns per row, and so bind parameters per row.
const COLUMNS: usize = 7;

/// Rows per statement: well inside every engine's bind-parameter ceiling.
const CHUNK: usize = 250;

/// Replace the whole list with `entries`, in one transaction.
///
/// All or nothing: a reader never sees half a list, and a download that fails
/// halfway through leaves the previous one in place.
pub async fn replace_all(db: &Db, entries: &[Entry]) -> Result<()> {
    let mut tx = db.begin_write().await?;

    sqlx::query(db.sql("DELETE FROM anime_mapping"))
        .execute(&mut *tx)
        .await?;

    for chunk in entries.chunks(CHUNK) {
        let row = format!("({})", ["?"; COLUMNS].join(", "));
        let sql = format!(
            "INSERT INTO anime_mapping
                 (mal_id, anilist_id, tvdb_id, tvdb_season, tvdb_offset, tmdb_movie, kind)
             VALUES {}",
            vec![row; chunk.len()].join(", ")
        );

        let mut args = AnyArguments::default();
        for e in chunk {
            let bound = [
                args.add(e.mal_id),
                args.add(e.anilist_id),
                args.add(e.tvdb_id),
                args.add(e.tvdb_season),
                args.add(e.tvdb_offset),
                args.add(e.tmdb_movie),
                args.add(e.kind.clone()),
            ];
            for result in bound {
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

/// Every entry filed under a TheTVDB series: its seasons, and the films and
/// specials TheTVDB keeps in season zero.
pub async fn for_series(db: &Db, tvdb_id: i64) -> Result<Vec<Entry>> {
    let rows = sqlx::query(db.sql(&format!("{SELECT} WHERE tvdb_id = ?")))
        .bind(tvdb_id)
        .fetch_all(db.pool())
        .await?;

    rows.iter().map(map).collect()
}

/// Every entry that is a TMDB film.
pub async fn for_movie(db: &Db, tmdb_id: i64) -> Result<Vec<Entry>> {
    let rows = sqlx::query(db.sql(&format!("{SELECT} WHERE tmdb_movie = ?")))
        .bind(tmdb_id)
        .fetch_all(db.pool())
        .await?;

    rows.iter().map(map).collect()
}

/// The TheTVDB series a MyAnimeList entry belongs to.
pub async fn tvdb_for_mal(db: &Db, mal_id: i64) -> Result<Option<i64>> {
    tvdb_for(
        db,
        "SELECT tvdb_id FROM anime_mapping WHERE mal_id = ? AND tvdb_id IS NOT NULL",
        mal_id,
    )
    .await
}

/// The TheTVDB series an AniList entry belongs to.
pub async fn tvdb_for_anilist(db: &Db, anilist_id: i64) -> Result<Option<i64>> {
    tvdb_for(
        db,
        "SELECT tvdb_id FROM anime_mapping WHERE anilist_id = ? AND tvdb_id IS NOT NULL",
        anilist_id,
    )
    .await
}

async fn tvdb_for(db: &Db, sql: &str, id: i64) -> Result<Option<i64>> {
    let row = sqlx::query(db.sql(sql))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    Ok(row.map(|r| r.big("tvdb_id")).transpose()?)
}

const SELECT: &str = "SELECT mal_id, anilist_id, tvdb_id, tvdb_season, tvdb_offset, \
                      tmdb_movie, kind FROM anime_mapping";

fn map(row: &sqlx::any::AnyRow) -> Result<Entry> {
    Ok(Entry {
        mal_id: row.opt_big("mal_id")?,
        anilist_id: row.opt_big("anilist_id")?,
        tvdb_id: row.opt_big("tvdb_id")?,
        tvdb_season: row.opt_int("tvdb_season")?,
        tvdb_offset: row.opt_int("tvdb_offset")?,
        tmdb_movie: row.opt_big("tmdb_movie")?,
        kind: row.opt_text("kind")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    async fn db() -> Db {
        let db = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        db.migrate().await.unwrap();
        db
    }

    fn entry(mal: i64, tvdb: Option<i64>, movie: Option<i64>) -> Entry {
        Entry {
            mal_id: Some(mal),
            anilist_id: Some(mal + 1),
            tvdb_id: tvdb,
            tmdb_movie: movie,
            kind: Some("TV".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_import_replaces_the_list_whole() {
        let db = db().await;

        replace_all(&db, &[entry(1, Some(10), None), entry(2, Some(10), None)])
            .await
            .unwrap();
        // More rows than one statement carries, to cross a chunk boundary.
        let next: Vec<Entry> = (100..100 + CHUNK as i64 + 3)
            .map(|mal| entry(mal, Some(20), None))
            .chain([entry(9, None, Some(372058))])
            .collect();
        replace_all(&db, &next).await.unwrap();

        assert!(
            for_series(&db, 10).await.unwrap().is_empty(),
            "the old list is gone"
        );
        assert_eq!(for_series(&db, 20).await.unwrap().len(), CHUNK + 3);
        assert_eq!(for_movie(&db, 372058).await.unwrap()[0].mal_id, Some(9));
    }

    #[tokio::test]
    async fn an_entry_leads_back_to_its_series() {
        let db = db().await;
        replace_all(
            &db,
            &[
                entry(16498, Some(267440), None),
                entry(32281, None, Some(372058)),
            ],
        )
        .await
        .unwrap();

        assert_eq!(tvdb_for_mal(&db, 16498).await.unwrap(), Some(267440));
        assert_eq!(tvdb_for_anilist(&db, 16499).await.unwrap(), Some(267440));
        // A film filed under no series leads nowhere, rather than to a null.
        assert_eq!(tvdb_for_mal(&db, 32281).await.unwrap(), None);
        assert_eq!(tvdb_for_mal(&db, 1).await.unwrap(), None);
    }
}
