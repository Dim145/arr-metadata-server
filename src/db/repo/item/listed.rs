//! What a work is listed by: the values the catalogue's lists are narrowed
//! and ordered by, kept on the work's row, and the count of its changes that
//! says when they are behind and it must be listed again.

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};

use super::read::map_rating;
use crate::{
    db::{Db, RowExt, from_bool, now, text_list},
    domain::{MediaItem, Rating},
};

/// What a work is listed by: its values as the catalogue shows it, kept beside
/// the providers' own so a list can be narrowed and ordered by them. Written
/// by `service::listing` from the work as it is read — its locks applied, IMDb's
/// list laid over its scores — and here, on a work's first write, from the
/// work as given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    /// The key a list is put in title order by: the sort title if there is
    /// one, folded to plain lower-case letters, so an accent does not send
    /// *Étoile* past *Zodiac* the way the stored letters did.
    pub title: String,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub keywords: Vec<String>,
    pub network: Option<String>,
    pub studio: Option<String>,
    pub status: Option<String>,
    pub language: Option<String>,
    /// When it first reached the public, as far as it is known: a date, or the
    /// year alone.
    pub release: Option<String>,
    /// The score its card leads with; see [`MediaItem::headline_rating`].
    pub score: Option<f64>,
}

impl Listed {
    pub fn of(item: &MediaItem) -> Self {
        // PostgreSQL will not store a NUL in text, and a lock can hold one: a
        // value that passes as JSON. Written as it came, one such work failed
        // every batch it was in.
        let clean = |value: &str| value.replace('\0', "");
        let given = |value: &Option<String>| {
            value
                .as_deref()
                .map(clean)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let each = |values: &[String]| values.iter().map(|v| clean(v)).collect::<Vec<_>>();

        let named = clean(
            item.sort_title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(&item.title),
        );
        let folded = slug::slugify(&named);
        let title = if folded.is_empty() {
            named.to_lowercase()
        } else {
            folded
        };

        let release = [
            &item.first_aired,
            &item.in_cinemas,
            &item.digital_release,
            &item.physical_release,
        ]
        .into_iter()
        .find_map(given)
        .map(|date| date.get(..10).map_or(date.clone(), String::from))
        .or_else(|| item.year.map(|year| format!("{year:04}")));

        Self {
            title,
            year: item.year,
            // Each once, in the order the work gives them; see `genre_parts`.
            genres: {
                let mut seen = std::collections::HashSet::new();
                item.genres
                    .iter()
                    .flat_map(|g| crate::domain::genre_parts(&clean(g)))
                    .filter(|g| seen.insert(g.clone()))
                    .collect()
            },
            keywords: each(&item.keywords),
            network: given(&item.network),
            studio: given(&item.studio),
            status: given(&item.status),
            language: given(&item.original_language),
            release,
            score: item.headline_rating().and_then(|r| r.value),
        }
    }
}

/// Writes that touch many works' listed values at once take turns.
///
/// On PostgreSQL two of them locking the same rows in different orders
/// deadlock: a batch of the listing job and the mark IMDb's import leaves on
/// every work it rescored did, each time they met. Single-row writes need no
/// turn; they cannot close a cycle.
static LISTING: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Which change to each work is the latest, read before the work itself: a
/// change that lands between the two is then not mistaken for one listed.
pub async fn listed_changes(db: &Db, ids: &[String]) -> Result<HashMap<String, i64>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let sql = format!(
        "SELECT id, listed_change FROM media_item WHERE id IN ({})",
        vec!["?"; ids.len()].join(", ")
    );
    let mut args = AnyArguments::default();
    for id in ids {
        args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|row| Ok((row.text("id")?, row.big("listed_change")?)))
        .collect()
}

/// One work's listed values, as of the change they were read at.
#[derive(Clone, Debug, PartialEq)]
pub struct Relisted {
    pub id: String,
    /// The change the work was read at; see [`listed_changes`].
    pub change: i64,
    pub values: Listed,
    /// The adult flag a lock on `isAdult` gives the work, where its row says
    /// otherwise: written to the row with the values. Every list, the
    /// calendar, the feeds and the clients' lookups filter on the row, so a
    /// lock the row did not follow — set before the row was written with it,
    /// or by a write that does not pin the locked identity — left an adult
    /// work in every one of them.
    pub adult: Option<bool>,
}

/// Store what works are listed by, each as of the change it was read at.
///
/// A work that has changed again since is left alone — still to be listed,
/// from what it holds now — rather than written with what it held then.
/// Returns how many were written.
pub async fn write_listed(db: &Db, listed: &[Relisted], imdb: bool, version: i64) -> Result<u64> {
    let _turn = LISTING.lock().await;
    let mut tx = db.begin_write().await?;
    let at = now();
    let mut written = 0;

    for Relisted {
        id,
        change,
        values,
        adult,
    } in listed
    {
        let result = sqlx::query(db.sql(
            "UPDATE media_item SET
                 listed_title = ?, listed_year = ?, listed_genres = ?, listed_keywords = ?,
                 listed_network = ?, listed_studio = ?, listed_status = ?, listed_language = ?,
                 listed_release = ?, listed_score = ?, listed_imdb = ?, listed_version = ?,
                 listed_seen = ?, listed_at = ?
             WHERE id = ? AND listed_change = ?",
        ))
        .bind(&values.title)
        .bind(values.year)
        .bind(text_list(&values.genres))
        .bind(text_list(&values.keywords))
        .bind(&values.network)
        .bind(&values.studio)
        .bind(&values.status)
        .bind(&values.language)
        .bind(&values.release)
        .bind(values.score)
        .bind(from_bool(imdb))
        .bind(version)
        .bind(*change)
        .bind(&at)
        .bind(id)
        .bind(*change)
        .execute(&mut *tx)
        .await?;

        // As of the same change: a lock lifted meanwhile is not written back.
        if result.rows_affected() > 0
            && let Some(adult) = adult
        {
            sqlx::query(db.sql("UPDATE media_item SET is_adult = ?, updated_at = ? WHERE id = ?"))
                .bind(from_bool(*adult))
                .bind(&at)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }

        written += result.rows_affected();
    }

    tx.commit()
        .await
        .context("failed to commit listed values")?;
    Ok(written)
}

/// Works whose listed values are behind: changed since they were listed,
/// listed another way than `version`, or with IMDb's list taken into account
/// where the switch now says `imdb` otherwise.
///
/// In id order, after `after`: a pass walks them from one end to the other, so
/// a work that cannot be written is passed over rather than met at the head of
/// every pass, holding back every work behind it.
pub async fn stale_ids(
    db: &Db,
    version: i64,
    imdb: bool,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql(
        "SELECT id FROM media_item
         WHERE (listed_seen IS NULL OR listed_seen <> listed_change
                OR listed_version IS NULL OR listed_version <> ?
                OR listed_imdb <> ?)
           AND id > ?
         ORDER BY id
         LIMIT ?",
    ))
    .bind(version)
    .bind(from_bool(imdb))
    .bind(after.unwrap_or(""))
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .iter()
        .map(|row| row.text("id"))
        .collect::<Result<_, _>>()?)
}

/// Count a change to what a work is listed by, so that it is listed again.
pub async fn mark_changed(db: &Db, id: &str) -> Result<()> {
    sqlx::query(db.sql("UPDATE media_item SET listed_change = listed_change + 1 WHERE id = ?"))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(())
}

/// [`mark_changed`], inside the transaction that made the change: a change
/// written without its mark would never be listed.
pub async fn mark_changed_in(db: &Db, tx: &mut Transaction<'_, Any>, id: &str) -> Result<()> {
    sqlx::query(db.sql("UPDATE media_item SET listed_change = listed_change + 1 WHERE id = ?"))
        .bind(id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

/// Count a change to the works IMDb's list rescored: those whose IMDb id is
/// one of `tconsts`.
pub async fn mark_changed_by_imdb(db: &Db, tconsts: &[String]) -> Result<u64> {
    let mut marked = 0;

    for chunk in tconsts.chunks(500) {
        let _turn = LISTING.lock().await;
        let sql = format!(
            "UPDATE media_item SET listed_change = listed_change + 1
             WHERE id IN (SELECT media_id FROM media_external_id
                          WHERE source = 'imdb' AND value IN ({}))",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for tconst in chunk {
            args.add(tconst.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        marked += sqlx::query_with(db.sql(&sql), args)
            .execute(db.pool())
            .await?
            .rows_affected();
    }

    Ok(marked)
}

/// The ratings of a batch of works, which is all a score needs of what hangs
/// off them.
pub async fn attach_ratings(db: &Db, items: &mut [MediaItem]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let sql = format!(
        "SELECT media_id, source, value, votes, rating_type
         FROM media_rating WHERE media_id IN ({})",
        vec!["?"; items.len()].join(", ")
    );
    let mut args = AnyArguments::default();
    for item in items.iter() {
        args.add(item.id.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let mut ratings: HashMap<String, Vec<Rating>> = HashMap::new();
    for row in &rows {
        ratings
            .entry(row.text("media_id")?)
            .or_default()
            .push(map_rating(row)?);
    }

    for item in items {
        item.ratings = ratings.remove(&item.id).unwrap_or_default();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MediaKind;

    #[test]
    fn genres_are_listed_each_once_whichever_way_they_came() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.genres = vec![
            "Sci-Fi & Fantasy".into(),
            "Fantasy".into(),
            "Science Fiction".into(),
            "Drama".into(),
        ];
        assert_eq!(
            Listed::of(&item).genres,
            ["Science Fiction", "Fantasy", "Drama"]
        );
    }
}
