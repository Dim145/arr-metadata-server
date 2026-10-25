//! What IMDb's datasets say of the works this catalogue holds: their ratings,
//! the titles they go by in each region, and the episodes of each series.
//!
//! Filled from IMDb's `title.ratings`, `title.akas` and `title.episode`
//! datasets by [`crate::jobs::datasets`]. The ratings are read back when a
//! work is loaded; the titles when Radarr asks for a film; the episodes when
//! an episode's page, or its `.nfo` document, asks which of IMDb's it is.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::{Arguments as _, any::AnyArguments};

use crate::db::{Db, RowExt, from_bool};

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

/// Every figure the list holds now of a work held here, by title: what an
/// import compares its new figures with, to tell which works it rescored.
/// The episodes' figures are left out: no list is sorted by them, and they
/// are ten times as many.
pub async fn values(db: &Db) -> Result<std::collections::HashMap<String, f64>> {
    let rows = sqlx::query(db.sql(
        "SELECT tconst, rating FROM imdb_rating
         WHERE tconst IN (SELECT value FROM media_external_id WHERE source = 'imdb')",
    ))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|r| Ok((r.text("tconst")?, r.opt_real("rating")?.unwrap_or_default())))
        .collect()
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

/// The IMDb id of every series this catalogue holds: whose episodes the
/// episodes' import keeps.
pub async fn series(db: &Db) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql(
        "SELECT e.value FROM media_external_id e
         JOIN media_item m ON m.id = e.media_id
         WHERE e.source = 'imdb' AND m.kind = 'series'",
    ))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|r| r.text("value").map_err(Into::into))
        .collect()
}

/// The IMDb id of every episode IMDb lists of a series held here: what the
/// ratings' import keeps beside the works', while the episodes are read.
pub async fn episode_ids(db: &Db) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql("SELECT tconst FROM imdb_episode"))
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|r| r.text("tconst").map_err(Into::into))
        .collect()
}

// ─── the titles by region ────────────────────────────────────────────────────

/// One title IMDb gives a work, as `title.akas` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Title {
    pub tconst: String,
    /// IMDb's number for the row, unique within the work.
    pub ordering: i64,
    pub title: String,
    /// As IMDb writes it: ISO 3166-1 (`FR`), or a code of its own (`XWW`).
    pub region: Option<String>,
    pub language: Option<String>,
    /// Which of `imdbDisplay`, `original` and `alternative` it is, comma
    /// separated.
    pub types: String,
    pub is_original: bool,
}

/// Write some works' titles, in one short transaction: those already kept
/// of the works in `fresh` are deleted first, then `titles` are added.
///
/// A run writes a batch of works at a time rather than every work at once:
/// replacing every title in one transaction held SQLite's only writer for
/// as long as hundreds of thousands of rows took, and every other write of
/// the server waited on it. `fresh` names the works this run writes for the
/// first time; a work met again in a later batch — which a list sorted by
/// its ids never does — is added to rather than replaced.
pub async fn write_titles(db: &Db, fresh: &[String], titles: &[Title]) -> Result<()> {
    let mut tx = db.begin_write().await?;

    for chunk in fresh.chunks(CHUNK) {
        let sql = format!(
            "DELETE FROM imdb_title WHERE tconst IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for tconst in chunk {
            args.add(tconst.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut *tx)
            .await?;
    }

    for chunk in titles.chunks(CHUNK) {
        let sql = format!(
            "INSERT INTO imdb_title (tconst, ordering, title, region, language, types, is_original)
             VALUES {}
             ON CONFLICT (tconst, ordering) DO UPDATE SET
                 title = excluded.title, region = excluded.region,
                 language = excluded.language, types = excluded.types,
                 is_original = excluded.is_original",
            vec!["(?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );

        let mut args = AnyArguments::default();
        for t in chunk {
            for result in [
                args.add(t.tconst.clone()),
                args.add(t.ordering),
                args.add(t.title.clone()),
                args.add(t.region.clone()),
                args.add(t.language.clone()),
                args.add(t.types.clone()),
                args.add(from_bool(t.is_original)),
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

/// Every work some titles are kept for, by IMDb id: what a run that has read
/// the whole list compares with the works it wrote, to forget the others.
pub async fn titled(db: &Db) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql("SELECT DISTINCT tconst FROM imdb_title"))
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|r| r.text("tconst").map_err(Into::into))
        .collect()
}

/// Forget the titles of these works: no longer held, or no longer listed.
pub async fn forget_titles(db: &Db, tconsts: &[String]) -> Result<u64> {
    forget(db, "DELETE FROM imdb_title WHERE tconst IN", tconsts).await
}

/// The titles IMDb gives these works in these regions, each work's in IMDb's
/// order.
pub async fn titles_in(db: &Db, tconsts: &[String], regions: &[String]) -> Result<Vec<Title>> {
    if tconsts.is_empty() || regions.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    // Regions are a handful; the works, as many as Radarr asks for at once.
    for chunk in tconsts.chunks(CHUNK) {
        let sql = format!(
            "SELECT tconst, ordering, title, region, language, types, is_original
             FROM imdb_title
             WHERE tconst IN ({}) AND region IN ({})
             ORDER BY tconst, ordering",
            vec!["?"; chunk.len()].join(", "),
            vec!["?"; regions.len()].join(", ")
        );

        let mut args = AnyArguments::default();
        for value in chunk.iter().chain(regions) {
            args.add(value.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?;

        for r in &rows {
            out.push(Title {
                tconst: r.text("tconst")?,
                ordering: r.big("ordering")?,
                title: r.text("title")?,
                region: r.opt_text("region")?,
                language: r.opt_text("language")?,
                types: r.text("types")?,
                is_original: r.flag("is_original")?,
            });
        }
    }

    Ok(out)
}

// ─── the episodes ────────────────────────────────────────────────────────────

/// One episode IMDb lists of a series, by the numbers IMDb gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Episode {
    pub tconst: String,
    /// The series'.
    pub parent: String,
    pub season_number: i32,
    pub episode_number: i32,
}

/// Write some series' episodes, in one short transaction: those already
/// kept of the series in `fresh` are deleted first, then `episodes` are
/// added. A batch of series at a time, for the reason [`write_titles`] gives.
pub async fn write_episodes(db: &Db, fresh: &[String], episodes: &[Episode]) -> Result<()> {
    let mut tx = db.begin_write().await?;

    for chunk in fresh.chunks(CHUNK) {
        let sql = format!(
            "DELETE FROM imdb_episode WHERE parent_tconst IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for tconst in chunk {
            args.add(tconst.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut *tx)
            .await?;
    }

    for chunk in episodes.chunks(CHUNK) {
        // An episode IMDb moved to another series since the last run is
        // the other series' now.
        let sql = format!(
            "INSERT INTO imdb_episode (tconst, parent_tconst, season_number, episode_number)
             VALUES {}
             ON CONFLICT (tconst) DO UPDATE SET
                 parent_tconst = excluded.parent_tconst,
                 season_number = excluded.season_number,
                 episode_number = excluded.episode_number",
            vec!["(?, ?, ?, ?)"; chunk.len()].join(", ")
        );

        let mut args = AnyArguments::default();
        for e in chunk {
            for result in [
                args.add(e.tconst.clone()),
                args.add(e.parent.clone()),
                args.add(e.season_number),
                args.add(e.episode_number),
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

/// Every series some episodes are kept for, by IMDb id.
pub async fn listed_series(db: &Db) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql("SELECT DISTINCT parent_tconst FROM imdb_episode"))
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|r| r.text("parent_tconst").map_err(Into::into))
        .collect()
}

/// Forget the episodes of these series.
pub async fn forget_series(db: &Db, parents: &[String]) -> Result<u64> {
    forget(
        db,
        "DELETE FROM imdb_episode WHERE parent_tconst IN",
        parents,
    )
    .await
}

/// The IMDb episode one of a work's episodes is, matched by the season and
/// the number it is stored under — the official order's — against those
/// IMDb gives, with IMDb's rating of it where the ratings' list holds one.
#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeMatch {
    pub episode_id: String,
    pub season_number: i32,
    pub episode_number: i32,
    pub tconst: String,
    pub rating: Option<f64>,
    pub votes: Option<i64>,
}

/// Each episode of a work that IMDb numbers the same, of one season or of
/// all, in their order. The specials are left out: IMDb's season 0, where
/// it keeps one, has nothing to do with TheTVDB's.
///
/// Nothing is matched where the numbers are known to part, since a wrong
/// IMDb id in a media library is worse than none: a number IMDb gives two
/// of its episodes, and a season IMDb counts otherwise (see
/// [`counted_otherwise`]).
pub async fn episodes_of(
    db: &Db,
    media_id: &str,
    season: Option<i32>,
) -> Result<Vec<EpisodeMatch>> {
    let mut sql = String::from(
        "SELECT e.id AS episode_id, e.season_number, e.episode_number, i.tconst,
                r.rating, r.votes
         FROM media_episode e
         JOIN media_external_id x ON x.media_id = e.media_id AND x.source = 'imdb'
         JOIN imdb_episode i ON i.parent_tconst = x.value
                            AND i.season_number = e.season_number
                            AND i.episode_number = e.episode_number
         LEFT JOIN imdb_rating r ON r.tconst = i.tconst
         WHERE e.media_id = ? AND e.season_number > 0",
    );
    if season.is_some() {
        sql.push_str(" AND e.season_number = ?");
    }
    sql.push_str(" ORDER BY e.season_number, e.episode_number, i.tconst");

    let mut query = sqlx::query(db.sql(&sql)).bind(media_id);
    if let Some(season) = season {
        query = query.bind(season);
    }
    let rows = query.fetch_all(db.pool()).await?;

    let mut matches: Vec<EpisodeMatch> = Vec::with_capacity(rows.len());
    let mut twice: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in &rows {
        let found = EpisodeMatch {
            episode_id: r.text("episode_id")?,
            season_number: r.int("season_number")?,
            episode_number: r.int("episode_number")?,
            tconst: r.text("tconst")?,
            rating: r.opt_real("rating")?,
            votes: r.opt_big("votes")?,
        };
        match matches.last() {
            Some(last) if last.episode_id == found.episode_id => {
                if last.tconst != found.tconst {
                    twice.insert(found.episode_id);
                }
            }
            _ => matches.push(found),
        }
    }
    let otherwise = if matches.is_empty() {
        std::collections::HashSet::new()
    } else {
        counted_otherwise(db, media_id).await?
    };
    matches.retain(|m| !twice.contains(&m.episode_id) && !otherwise.contains(&m.season_number));

    Ok(matches)
}

/// The seasons of a work IMDb counts otherwise: those, but the last, in
/// which IMDb numbers fewer episodes than are held here.
///
/// Where TheTVDB splits an hour-long episode in two and IMDb does not, the
/// season is shorter on IMDb, and every number past the first such episode
/// names another episode there: The Office's fourth season is nineteen
/// episodes here and fourteen on IMDb, its second, "Fun Run (2)", IMDb's
/// "Dunder Mifflin Infinity". The last season is let be: IMDb is often a
/// few episodes behind on one that airs, and the episodes it does list are
/// numbered alike. A season IMDb counts more of — announced episodes, an
/// episode 0 — matches as it is, said to be less sure like every match.
async fn counted_otherwise(db: &Db, media_id: &str) -> Result<std::collections::HashSet<i32>> {
    let held = sqlx::query(db.sql(
        "SELECT season_number, COUNT(*) AS n FROM media_episode
         WHERE media_id = ? AND season_number > 0
         GROUP BY season_number",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;
    let listed = sqlx::query(db.sql(
        "SELECT i.season_number, COUNT(*) AS n FROM imdb_episode i
         JOIN media_external_id x ON x.value = i.parent_tconst AND x.source = 'imdb'
         WHERE x.media_id = ? AND i.season_number > 0 AND i.episode_number > 0
         GROUP BY i.season_number",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let count = |rows: &[sqlx::any::AnyRow]| -> Result<HashMap<i32, i64>> {
        rows.iter()
            .map(|r| Ok((r.int("season_number")?, r.big("n")?)))
            .collect()
    };
    let held = count(&held)?;
    let listed = count(&listed)?;
    let last = held.keys().max().copied();

    Ok(held
        .into_iter()
        .filter(|&(season, n)| {
            Some(season) != last && listed.get(&season).is_some_and(|&on_imdb| on_imdb < n)
        })
        .map(|(season, _)| season)
        .collect())
}

/// The IMDb id each of a work's episodes is matched to, by the episode's id.
pub async fn episode_tconsts(db: &Db, media_id: &str) -> Result<HashMap<String, String>> {
    Ok(episodes_of(db, media_id, None)
        .await?
        .into_iter()
        .map(|m| (m.episode_id, m.tconst))
        .collect())
}

/// `DELETE … IN (…)` over a list of any length, a chunk at a time.
async fn forget(db: &Db, head: &str, values: &[String]) -> Result<u64> {
    let mut gone = 0;
    for chunk in values.chunks(CHUNK) {
        let sql = format!("{head} ({})", vec!["?"; chunk.len()].join(", "));
        let mut args = AnyArguments::default();
        for value in chunk {
            args.add(value.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        gone += sqlx::query_with(db.sql(&sql), args)
            .execute(db.pool())
            .await?
            .rows_affected();
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::db;

    #[tokio::test]
    async fn ratings_are_replaced_and_read_back() {
        let db = db().await;

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

    /// A series held under `imdb`, with episodes so numbered.
    async fn series_held(db: &Db, imdb: &str, episodes: &[(i32, i32)]) -> crate::domain::MediaItem {
        let mut item = crate::domain::MediaItem::empty(crate::domain::MediaKind::Series);
        item.id = crate::db::new_id();
        item.title = "The Office".into();
        item.slug = crate::domain::make_slug(&item.title, None);
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        item.external_ids.imdb = Some(imdb.into());
        item.episodes = episodes
            .iter()
            .map(|&(season, number)| {
                let mut episode = crate::db::repo::child::blank_episode(season, number);
                episode.id = crate::db::new_id();
                episode.is_manual = false;
                episode
            })
            .collect();
        crate::db::repo::item::upsert(
            db,
            crate::db::repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();
        item
    }

    fn listed(tconst: &str, season: i32, number: i32) -> Episode {
        Episode {
            tconst: tconst.into(),
            parent: "tt0386676".into(),
            season_number: season,
            episode_number: number,
        }
    }

    #[tokio::test]
    async fn an_episode_is_matched_by_its_numbers_where_imdb_counts_alike() {
        let db = db().await;
        // Season 4 holds four episodes here — TheTVDB splits the hour-long
        // ones — and two on IMDb; season 5 is the last, which IMDb has only
        // begun; season 6, three numbers alike and one IMDb gives twice.
        let held = series_held(
            &db,
            "tt0386676",
            &[
                (0, 1),
                (4, 1),
                (4, 2),
                (4, 3),
                (4, 4),
                (6, 1),
                (6, 2),
                (6, 3),
                (7, 1),
                (7, 2),
            ],
        )
        .await;
        write_episodes(
            &db,
            &[],
            &[
                listed("tt0000101", 0, 1),
                listed("tt1009369", 4, 1),
                listed("tt1031477", 4, 2),
                listed("tt1500001", 6, 1),
                listed("tt1500002", 6, 2),
                listed("tt1500003", 6, 3),
                listed("tt1500004", 6, 3),
                listed("tt1700001", 7, 1),
            ],
        )
        .await
        .unwrap();
        replace_all(
            &db,
            &[Rating {
                tconst: "tt1500001".into(),
                rating: 8.4,
                votes: 5_000,
            }],
        )
        .await
        .unwrap();

        let matched: Vec<(i32, i32, String, Option<f64>)> = episodes_of(&db, &held.id, None)
            .await
            .unwrap()
            .into_iter()
            .map(|m| (m.season_number, m.episode_number, m.tconst, m.rating))
            .collect();
        assert_eq!(
            matched,
            [
                (6, 1, "tt1500001".to_string(), Some(8.4)),
                (6, 2, "tt1500002".to_string(), None),
                (7, 1, "tt1700001".to_string(), None),
            ],
            "no special, nothing of the season IMDb counts otherwise, nothing of a number given twice"
        );
        assert_eq!(
            episodes_of(&db, &held.id, Some(7)).await.unwrap().len(),
            1,
            "a season asked alone"
        );
        assert_eq!(episode_tconsts(&db, &held.id).await.unwrap().len(), 3);
    }
}
