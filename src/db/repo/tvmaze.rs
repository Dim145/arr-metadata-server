//! TVmaze's index of every series it lists, as this instance keeps it
//! (`tvmaze.index`): written by `jobs::tvmaze_index`, searched by
//! `service::tvmaze_index`, and read for each series' weight by the
//! popularity worked out here.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::{Arguments as _, any::AnyArguments};

use crate::{
    db::{Db, Dialect, RowExt, from_bool},
    providers::tvmaze::IndexShow,
};

/// What a series is read back with.
const COLUMNS: &str = "id, name, kind, language, status, premiered, ended, network, country, \
                       web, tvdb_id, imdb_id, tvrage_id, weight, updated";

/// Series one statement writes, at sixteen values each.
const ROWS: usize = 50;

/// Ids one statement reads or deletes.
const CHUNK: usize = 500;

fn add<'t>(
    args: &mut AnyArguments,
    value: impl sqlx::Encode<'t, sqlx::Any> + sqlx::Type<sqlx::Any>,
) -> Result<()> {
    args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
}

fn holes(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// Write these series, each in place of the one kept under its id, in one
/// transaction. A series given twice is written as it is given last.
pub async fn write(db: &Db, shows: &[IndexShow]) -> Result<()> {
    let mut last: HashMap<i64, &IndexShow> = HashMap::with_capacity(shows.len());
    for show in shows {
        last.insert(show.id, show);
    }
    let mut shows: Vec<&IndexShow> = last.into_values().collect();
    if shows.is_empty() {
        return Ok(());
    }
    shows.sort_unstable_by_key(|show| show.id);

    let mut tx = db.begin_write().await?;
    for chunk in shows.chunks(ROWS) {
        let sql = format!(
            "INSERT INTO tvmaze_show (id, name, norm, kind, language, status, premiered, ended,
                                      network, country, web, tvdb_id, imdb_id, tvrage_id,
                                      weight, updated)
             VALUES {}
             ON CONFLICT (id) DO UPDATE SET
                 name = excluded.name, norm = excluded.norm, kind = excluded.kind,
                 language = excluded.language, status = excluded.status,
                 premiered = excluded.premiered, ended = excluded.ended,
                 network = excluded.network, country = excluded.country, web = excluded.web,
                 tvdb_id = excluded.tvdb_id, imdb_id = excluded.imdb_id,
                 tvrage_id = excluded.tvrage_id, weight = excluded.weight,
                 updated = excluded.updated",
            vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for show in chunk {
            add(&mut args, show.id)?;
            add(&mut args, show.name.clone())?;
            add(&mut args, crate::search::norm(&show.name))?;
            add(&mut args, show.kind.clone())?;
            add(&mut args, show.language.clone())?;
            add(&mut args, show.status.clone())?;
            add(&mut args, show.premiered.clone())?;
            add(&mut args, show.ended.clone())?;
            add(&mut args, show.network.clone())?;
            add(&mut args, show.country.clone())?;
            add(&mut args, from_bool(show.web))?;
            add(&mut args, show.tvdb_id)?;
            add(&mut args, show.imdb_id.clone())?;
            add(&mut args, show.tvrage_id)?;
            add(&mut args, show.weight.map(i64::from))?;
            add(&mut args, show.updated)?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Forget these series, in one transaction: TVmaze no longer lists them.
pub async fn forget(db: &Db, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = db.begin_write().await?;
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "DELETE FROM tvmaze_show WHERE id IN ({})",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            add(&mut args, *id)?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// How many series are kept.
pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM tvmaze_show"))
        .fetch_one(db.pool())
        .await?;
    Ok(row.big("n")?)
}

/// The ids of every series kept.
pub async fn ids(db: &Db) -> Result<Vec<i64>> {
    let rows = sqlx::query(db.sql("SELECT id FROM tvmaze_show ORDER BY id"))
        .fetch_all(db.pool())
        .await?;
    rows.iter().map(|row| Ok(row.big("id")?)).collect()
}

/// When TVmaze last changed each of these series, as it was kept, for those
/// that are: `None` within for one kept without a date.
pub async fn updated_of(db: &Db, ids: &[i64]) -> Result<HashMap<i64, Option<i64>>> {
    let mut kept = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT id, updated FROM tvmaze_show WHERE id IN ({})",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            add(&mut args, *id)?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            kept.insert(row.big("id")?, row.opt_big("updated")?);
        }
    }
    Ok(kept)
}

fn show(row: &sqlx::any::AnyRow) -> Result<IndexShow> {
    Ok(IndexShow {
        id: row.big("id")?,
        name: row.text("name")?,
        kind: row.opt_text("kind")?,
        language: row.opt_text("language")?,
        status: row.opt_text("status")?,
        premiered: row.opt_text("premiered")?,
        ended: row.opt_text("ended")?,
        network: row.opt_text("network")?,
        country: row.opt_text("country")?,
        web: row.flag("web")?,
        tvdb_id: row.opt_big("tvdb_id")?,
        imdb_id: row.opt_text("imdb_id")?,
        tvrage_id: row.opt_big("tvrage_id")?,
        weight: row.opt_big("weight")?.map(|w| w.clamp(0, 100) as i32),
        updated: row.opt_big("updated")?,
    })
}

/// Text as a `LIKE` pattern matches it, its wildcards escaped: a folded term
/// holds none, but a pattern is never trusted to have been folded.
fn like_escaped(text: &str) -> String {
    text.replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

/// The series whose folded name starts with `prefix`, folded as well: the
/// best known first, `limit` at most. Read in the order of the name's index —
/// a range on SQLite, `LIKE` on PostgreSQL, whose index is built for it.
pub async fn starting(db: &Db, prefix: &str, limit: i64) -> Result<Vec<IndexShow>> {
    if prefix.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = AnyArguments::default();
    let condition = match db.dialect() {
        Dialect::Sqlite => {
            add(&mut args, prefix.to_string())?;
            // No character sorts after U+10FFFF.
            add(&mut args, format!("{prefix}\u{10FFFF}"))?;
            "norm >= ? AND norm < ?"
        }
        Dialect::Postgres => {
            add(&mut args, format!("{}%", like_escaped(prefix)))?;
            r"norm LIKE ? ESCAPE '\'"
        }
    };
    add(&mut args, limit)?;
    let sql = format!(
        "SELECT {COLUMNS} FROM tvmaze_show WHERE {condition}
         ORDER BY weight DESC NULLS LAST, id LIMIT ?"
    );
    sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?
        .iter()
        .map(show)
        .collect()
}

/// The series with a word of their folded name, past its first, starting with
/// `word`: the best known first, `limit` at most. Every name is read for it —
/// ninety thousand short texts, a few milliseconds — which the search only
/// does when the providers did not answer.
pub async fn with_word(db: &Db, word: &str, limit: i64) -> Result<Vec<IndexShow>> {
    if word.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = AnyArguments::default();
    add(&mut args, format!("% {}%", like_escaped(word)))?;
    add(&mut args, limit)?;
    let sql = format!(
        r"SELECT {COLUMNS} FROM tvmaze_show WHERE norm LIKE ? ESCAPE '\'
          ORDER BY weight DESC NULLS LAST, id LIMIT ?"
    );
    sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?
        .iter()
        .map(show)
        .collect()
}

/// Which of the index's ids a series is filed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tvmaze,
    Tvdb,
}

impl Key {
    const fn column(self) -> &'static str {
        match self {
            Self::Tvmaze => "id",
            Self::Tvdb => "tvdb_id",
        }
    }
}

/// The series kept under these ids, TVmaze's or TheTVDB's, by the id asked
/// for. Two series TVmaze files under one TheTVDB id: the better known.
pub async fn by(db: &Db, key: Key, ids: &[i64]) -> Result<HashMap<i64, IndexShow>> {
    let mut found: HashMap<i64, IndexShow> = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT {COLUMNS} FROM tvmaze_show WHERE {} IN ({})
             ORDER BY weight ASC NULLS FIRST, id DESC",
            key.column(),
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for id in chunk {
            add(&mut args, *id)?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            let show = show(&row)?;
            let id = match key {
                Key::Tvmaze => Some(show.id),
                Key::Tvdb => show.tvdb_id,
            };
            // In the order read, the better known last: it is the one kept.
            if let Some(id) = id {
                found.insert(id, show);
            }
        }
    }
    Ok(found)
}

/// The series kept under these IMDb ids, by id; of two, the better known.
pub async fn by_imdb(db: &Db, tconsts: &[String]) -> Result<HashMap<String, IndexShow>> {
    let mut found: HashMap<String, IndexShow> = HashMap::with_capacity(tconsts.len());
    for chunk in tconsts.chunks(CHUNK) {
        let sql = format!(
            "SELECT {COLUMNS} FROM tvmaze_show WHERE imdb_id IN ({})
             ORDER BY weight ASC NULLS FIRST, id DESC",
            holes(chunk.len())
        );
        let mut args = AnyArguments::default();
        for tconst in chunk {
            add(&mut args, tconst.clone())?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            let show = show(&row)?;
            if let Some(tconst) = show.imdb_id.clone() {
                found.insert(tconst, show);
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(id: i64, name: &str, weight: i32) -> IndexShow {
        IndexShow {
            id,
            name: name.into(),
            weight: Some(weight),
            updated: Some(1_700_000_000 + id),
            ..IndexShow::default()
        }
    }

    #[tokio::test]
    async fn the_index_is_written_over_and_read_by_the_start_of_a_name_or_of_a_word() {
        let db = crate::db::testing::db().await;
        write(
            &db,
            &[
                listed(1, "Breaking Bad", 99),
                listed(2, "Breaking Point", 40),
                listed(3, "Bad Banks", 60),
                listed(4, "Le Bureau des Légendes", 70),
                // Given twice: written as it is given last.
                listed(5, "Old Name", 10),
                listed(5, "The Office", 95),
            ],
        )
        .await
        .unwrap();
        assert_eq!(count(&db).await.unwrap(), 5);

        let names = |shows: Vec<IndexShow>| shows.into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(
            names(starting(&db, "breaking", 10).await.unwrap()),
            ["Breaking Bad", "Breaking Point"]
        );
        assert_eq!(
            names(starting(&db, "breaking", 1).await.unwrap()),
            ["Breaking Bad"]
        );
        assert_eq!(
            names(with_word(&db, "bad", 10).await.unwrap()),
            ["Breaking Bad"]
        );
        // Folded as the search index folds names: accents off, lower case.
        assert_eq!(
            names(with_word(&db, "legendes", 10).await.unwrap()),
            ["Le Bureau des Légendes"]
        );
        assert_eq!(
            names(with_word(&db, "office", 10).await.unwrap()),
            ["The Office"]
        );
        // A wildcard is a letter like any other.
        assert!(starting(&db, "%", 10).await.unwrap().is_empty());

        // Written over, and forgotten.
        write(&db, &[listed(2, "Breaking Point", 45)])
            .await
            .unwrap();
        let kept = updated_of(&db, &[1, 2, 9]).await.unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[&2], Some(1_700_000_002));
        forget(&db, &[1, 9]).await.unwrap();
        assert_eq!(ids(&db).await.unwrap(), [2, 3, 4, 5]);
        assert_eq!(
            by(&db, Key::Tvmaze, &[2]).await.unwrap()[&2].weight,
            Some(45)
        );
    }

    #[tokio::test]
    async fn a_series_is_found_by_its_ids_elsewhere_the_better_known_of_two() {
        let db = crate::db::testing::db().await;
        let with_ids = |id: i64, weight: i32, tvdb: i64, imdb: &str| IndexShow {
            tvdb_id: Some(tvdb),
            imdb_id: Some(imdb.into()),
            ..listed(id, &format!("Series {id}"), weight)
        };
        write(
            &db,
            &[
                with_ids(10, 20, 81189, "tt0903747"),
                with_ids(11, 80, 81189, "tt0903747"),
                with_ids(12, 50, 73739, "tt0411008"),
            ],
        )
        .await
        .unwrap();

        let by_tvdb = by(&db, Key::Tvdb, &[81189, 73739, 1]).await.unwrap();
        assert_eq!(by_tvdb[&81189].id, 11);
        assert_eq!(by_tvdb[&73739].id, 12);
        assert_eq!(by_tvdb.len(), 2);
        let by_imdb = by_imdb(&db, &["tt0903747".into()]).await.unwrap();
        assert_eq!(by_imdb["tt0903747"].id, 11);
    }
}
