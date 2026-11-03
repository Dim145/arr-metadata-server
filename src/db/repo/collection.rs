//! The collections the catalogue's films belong to, kept: TMDB's record of
//! each, by its id — its name and synopsis in the catalogue's language and in
//! the others TMDB has them in, its pictures, its films — written whole each
//! time TMDB is asked about it again (`service::collection`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::{Arguments, any::AnyArguments};

use crate::{
    db::{Db, Double, RowExt, new_id, now},
    domain::{Collection, CollectionPart, Translation},
};

const COLUMNS: &str = "id, tmdb_id, name, overview, poster, backdrop, language, refreshed_at";

fn map(row: &sqlx::any::AnyRow) -> Result<Collection> {
    Ok(Collection {
        id: row.text("id")?,
        tmdb_id: row.big("tmdb_id")?,
        name: row.text("name")?,
        overview: row.opt_text("overview")?,
        poster: row.opt_text("poster")?,
        backdrop: row.opt_text("backdrop")?,
        language: row.opt_text("language")?,
        translations: Vec::new(),
        parts: Vec::new(),
        refreshed_at: row.text("refreshed_at")?,
    })
}

/// One collection, by TMDB's id for it, whole: its translations and its
/// films with it.
pub async fn by_tmdb(db: &Db, tmdb_id: i64) -> Result<Option<Collection>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM collection WHERE tmdb_id = ?"
    )))
    .bind(tmdb_id)
    .fetch_optional(db.pool())
    .await?;
    let Some(mut collection) = row.as_ref().map(map).transpose()? else {
        return Ok(None);
    };
    collection.translations = translations(db, std::slice::from_ref(&collection.id))
        .await?
        .remove(&collection.id)
        .unwrap_or_default();
    collection.parts = parts(db, &collection.id).await?;
    Ok(Some(collection))
}

/// Several collections, by TMDB's ids for them, each with its translations
/// but not its films: what a list of them, or of films beside their
/// collections, is drawn from. Those not kept are not there.
pub async fn by_tmdb_ids(db: &Db, tmdb_ids: &[i64]) -> Result<HashMap<i64, Collection>> {
    let mut out: HashMap<i64, Collection> = HashMap::new();
    for chunk in tmdb_ids.chunks(200) {
        let mut args = AnyArguments::default();
        for id in chunk {
            args.add(*id).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let sql = format!(
            "SELECT {COLUMNS} FROM collection WHERE tmdb_id IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            let collection = map(&row)?;
            out.insert(collection.tmdb_id, collection);
        }
    }
    let ids: Vec<String> = out.values().map(|c| c.id.clone()).collect();
    let mut translated = translations(db, &ids).await?;
    for collection in out.values_mut() {
        collection.translations = translated.remove(&collection.id).unwrap_or_default();
    }
    Ok(out)
}

/// The translations of these collections, by their ids here.
async fn translations(db: &Db, ids: &[String]) -> Result<HashMap<String, Vec<Translation>>> {
    let mut out: HashMap<String, Vec<Translation>> = HashMap::new();
    for chunk in ids.chunks(200) {
        let mut args = AnyArguments::default();
        for id in chunk {
            args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let sql = format!(
            "SELECT collection_id, language, name, overview FROM collection_translation
              WHERE collection_id IN ({}) ORDER BY language",
            vec!["?"; chunk.len()].join(", ")
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            out.entry(row.text("collection_id")?)
                .or_default()
                .push(Translation {
                    language: row.text("language")?,
                    title: row.opt_text("name")?,
                    overview: row.opt_text("overview")?,
                    tagline: None,
                    is_manual: false,
                });
        }
    }
    Ok(out)
}

/// A collection's films, in TMDB's order.
async fn parts(db: &Db, collection_id: &str) -> Result<Vec<CollectionPart>> {
    let rows = sqlx::query(db.sql(
        "SELECT tmdb_id, title, original_title, overview, release_date, original_language,
                poster, backdrop, popularity, vote_average, vote_count, is_adult, sort_order
           FROM collection_part WHERE collection_id = ?
          ORDER BY sort_order, tmdb_id",
    ))
    .bind(collection_id)
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok(CollectionPart {
                tmdb_id: row.big("tmdb_id")?,
                title: row.text("title")?,
                original_title: row.opt_text("original_title")?,
                overview: row.opt_text("overview")?,
                release_date: row.opt_text("release_date")?,
                original_language: row.opt_text("original_language")?,
                poster: row.opt_text("poster")?,
                backdrop: row.opt_text("backdrop")?,
                popularity: row.opt_real("popularity")?,
                vote_average: row.opt_real("vote_average")?,
                vote_count: row.opt_big("vote_count")?,
                is_adult: row.flag("is_adult")?,
                sort_order: row.int("sort_order")?,
            })
        })
        .collect()
}

/// When TMDB was last asked about a collection, where it is kept.
pub async fn refreshed_at(db: &Db, tmdb_id: i64) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar(db.sql("SELECT refreshed_at FROM collection WHERE tmdb_id = ?"))
            .bind(tmdb_id)
            .fetch_optional(db.pool())
            .await?,
    )
}

/// TMDB's own answer about a collection, as it was kept with it: what a
/// document of TMDB's shape is written from where the columns keep nothing
/// of it. None where none was kept, or it does not read.
pub async fn document(db: &Db, tmdb_id: i64) -> Result<Option<serde_json::Value>> {
    let text: Option<String> =
        sqlx::query_scalar(db.sql("SELECT document FROM collection WHERE tmdb_id = ?"))
            .bind(tmdb_id)
            .fetch_optional(db.pool())
            .await?
            .flatten();
    Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
}

/// [`write_with`], keeping no answer of TMDB's beside it: what a test keeps
/// a collection with.
#[cfg(test)]
pub async fn write(db: &Db, collection: &Collection) -> Result<String> {
    write_with(db, collection, None).await
}

/// Keep a collection as TMDB gave it, in place of what was kept of it: its
/// row, kept under the same id, TMDB's answer itself (`document`), its
/// translations and its films. This server's id for it.
pub async fn write_with(
    db: &Db,
    collection: &Collection,
    document: Option<&serde_json::Value>,
) -> Result<String> {
    let mut tx = db.begin_write().await?;
    let at = now();
    let refreshed = match collection.refreshed_at.trim() {
        "" => at.clone(),
        given => given.to_string(),
    };
    sqlx::query(db.sql(
        "INSERT INTO collection
             (id, tmdb_id, name, overview, poster, backdrop, language, document,
              refreshed_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (tmdb_id) DO UPDATE SET
             name = excluded.name,
             overview = excluded.overview,
             poster = excluded.poster,
             backdrop = excluded.backdrop,
             language = excluded.language,
             document = excluded.document,
             refreshed_at = excluded.refreshed_at,
             updated_at = excluded.updated_at",
    ))
    .bind(new_id())
    .bind(collection.tmdb_id)
    .bind(collection.name.trim())
    .bind(&collection.overview)
    .bind(&collection.poster)
    .bind(&collection.backdrop)
    .bind(&collection.language)
    .bind(document.map(serde_json::Value::to_string))
    .bind(&refreshed)
    .bind(&at)
    .bind(&at)
    .execute(&mut *tx)
    .await
    .context("failed to keep a collection")?;
    let id: String = sqlx::query_scalar(db.sql("SELECT id FROM collection WHERE tmdb_id = ?"))
        .bind(collection.tmdb_id)
        .fetch_one(&mut *tx)
        .await?;

    for table in ["collection_translation", "collection_part"] {
        sqlx::query(db.sql(&format!("DELETE FROM {table} WHERE collection_id = ?")))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    for translation in &collection.translations {
        if translation.title.is_none() && translation.overview.is_none() {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO collection_translation (collection_id, language, name, overview)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (collection_id, language) DO NOTHING",
        ))
        .bind(&id)
        .bind(&translation.language)
        .bind(&translation.title)
        .bind(&translation.overview)
        .execute(&mut *tx)
        .await
        .context("failed to keep a collection's translation")?;
    }
    for part in &collection.parts {
        sqlx::query(db.sql(
            "INSERT INTO collection_part
                 (collection_id, tmdb_id, title, original_title, overview, release_date,
                  original_language, poster, backdrop, popularity, vote_average, vote_count,
                  is_adult, sort_order)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (collection_id, tmdb_id) DO NOTHING",
        ))
        .bind(&id)
        .bind(part.tmdb_id)
        .bind(&part.title)
        .bind(&part.original_title)
        .bind(&part.overview)
        .bind(&part.release_date)
        .bind(&part.original_language)
        .bind(&part.poster)
        .bind(&part.backdrop)
        .bind(Double(part.popularity))
        .bind(Double(part.vote_average))
        .bind(part.vote_count)
        .bind(crate::db::from_bool(part.is_adult))
        .bind(part.sort_order)
        .execute(&mut *tx)
        .await
        .context("failed to keep a collection's film")?;
    }
    tx.commit()
        .await
        .context("failed to commit a collection kept")?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saga(tmdb_id: i64, name: &str) -> Collection {
        Collection {
            id: String::new(),
            tmdb_id,
            name: name.into(),
            overview: Some("Neo, and the machines.".into()),
            poster: Some("https://image.tmdb.org/t/p/original/saga.jpg".into()),
            backdrop: None,
            language: Some("eng".into()),
            translations: vec![Translation {
                language: "fra".into(),
                title: Some(format!("{name} (fr)")),
                overview: None,
                tagline: None,
                is_manual: false,
            }],
            parts: vec![
                CollectionPart {
                    tmdb_id: 603,
                    title: "The Matrix".into(),
                    original_title: None,
                    overview: None,
                    release_date: Some("1999-03-31".into()),
                    original_language: Some("en".into()),
                    poster: Some("https://image.tmdb.org/t/p/original/603.jpg".into()),
                    backdrop: None,
                    popularity: Some(81.5),
                    vote_average: None,
                    vote_count: Some(26_000),
                    is_adult: false,
                    sort_order: 0,
                },
                CollectionPart {
                    tmdb_id: 604,
                    title: "The Matrix Reloaded".into(),
                    original_title: None,
                    overview: None,
                    release_date: None,
                    original_language: None,
                    poster: None,
                    backdrop: None,
                    popularity: None,
                    vote_average: Some(7.0),
                    vote_count: None,
                    is_adult: false,
                    sort_order: 1,
                },
            ],
            refreshed_at: String::new(),
        }
    }

    #[tokio::test]
    async fn a_collection_is_kept_whole_and_kept_again_under_the_same_id() {
        let db = crate::db::testing::db().await;
        let id = write(&db, &saga(9_102_344, "Kept Saga"))
            .await
            .expect("kept");

        let kept = by_tmdb(&db, 9_102_344).await.unwrap().expect("found");
        assert_eq!(kept.id, id);
        assert_eq!(kept.name, "Kept Saga");
        assert_eq!(
            kept.texts_in("fra"),
            ("Kept Saga (fr)", Some("Neo, and the machines."))
        );
        assert_eq!(kept.texts_in("eng").0, "Kept Saga");
        assert_eq!(kept.texts_in("deu").0, "Kept Saga", "none in German");
        assert_eq!(
            kept.parts
                .iter()
                .map(|p| (p.tmdb_id, p.year(), p.popularity, p.vote_average))
                .collect::<Vec<_>>(),
            [
                (603, Some(1999), Some(81.5), None),
                (604, None, None, Some(7.0))
            ]
        );
        assert!(!kept.refreshed_at.is_empty());

        let mut again = saga(9_102_344, "Kept Saga, renamed");
        again.parts.truncate(1);
        again.translations.clear();
        assert_eq!(write(&db, &again).await.unwrap(), id, "the same row");
        let kept = by_tmdb(&db, 9_102_344).await.unwrap().unwrap();
        assert_eq!(kept.name, "Kept Saga, renamed");
        assert_eq!(kept.parts.len(), 1);
        assert!(kept.translations.is_empty());

        let many = by_tmdb_ids(&db, &[9_102_344, 1]).await.unwrap();
        assert_eq!(many.len(), 1);
        assert!(many[&9_102_344].parts.is_empty(), "a list's, without films");
        assert!(refreshed_at(&db, 9_102_344).await.unwrap().is_some());
        assert!(refreshed_at(&db, 1).await.unwrap().is_none());
    }
}
