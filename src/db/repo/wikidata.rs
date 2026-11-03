//! Wikidata's links between the sites' ids, as kept (wave P, P5).
//!
//! Written by [`crate::jobs::wikidata`] from what Wikidata's query service
//! answered, and read by [`crate::service::bridge`]: a row a link, an id of a
//! site's on one of Wikidata's items, all of it numbers (migration 0051).
//!
//! A reading is not written whole over the last one. What differs between
//! the two — a few thousand links a week of a million — is written instead,
//! in one transaction, so a reader sees the links of one reading or of the
//! next, never half of either. A million links written at once, as the first
//! reading writes them, held SQLite's one writer two to seven seconds on a
//! fast disk in November 2026, by how they were written: on a slower one,
//! longer than the ten its other writers wait. So on SQLite a change larger
//! than [`BATCH`] is written in several transactions, the new links first;
//! PostgreSQL, whose writers do not wait on one another, takes any change in
//! one.

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use futures::TryStreamExt as _;
use sqlx::{Arguments as _, any::AnyArguments};

use crate::db::{Db, Dialect, RowExt};

/// One link: an id of a site's, by the property Wikidata files that site's
/// ids under, on one of Wikidata's items. Ordered as the table's key is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Link {
    /// The property: 4947 for TMDB's films, 345 for IMDb… (migration 0051).
    pub property: u16,
    /// The id, as a number: IMDb's without its `tt`.
    pub value: u32,
    /// Wikidata's item, without its `Q`.
    pub item: u32,
}

/// What a reading changes of the links kept.
#[derive(Debug, Default)]
pub struct Change {
    /// Kept, and no longer listed.
    pub gone: Vec<Link>,
    /// Listed, and not kept yet.
    pub added: Vec<Link>,
    /// How many links of each property were kept before: what a reading is
    /// weighed against.
    pub before: BTreeMap<u16, usize>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.gone.is_empty() && self.added.is_empty()
    }
}

/// Rows per statement: three columns each, well inside every engine's
/// bind-parameter ceiling.
const CHUNK: usize = 1_000;

/// The most rows one transaction writes on SQLite, where a transaction holds
/// the database's one writer: some 60 to 170 ms of it on a fast disk. A
/// week's change fits in one.
pub const BATCH: usize = 25_000;

/// The pause between two of those transactions, for the writers that waited
/// on one to go before the next.
const BETWEEN_BATCHES: Duration = Duration::from_millis(50);

/// Every link of the items an id is on — the id's own among them — by item,
/// then property, then value.
pub async fn of_id(db: &Db, property: u16, value: u32) -> Result<Vec<Link>> {
    let rows = sqlx::query(db.sql(
        "SELECT other.property, other.value, other.item
         FROM wikidata_link asked
         JOIN wikidata_link other ON other.item = asked.item
         WHERE asked.property = ? AND asked.value = ?
         ORDER BY other.item, other.property, other.value",
    ))
    .bind(i64::from(property))
    .bind(i64::from(value))
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(read).collect()
}

/// Every link of one item, by property, then value.
pub async fn of_item(db: &Db, item: u32) -> Result<Vec<Link>> {
    let rows = sqlx::query(db.sql(
        "SELECT property, value, item FROM wikidata_link WHERE item = ?
         ORDER BY property, value",
    ))
    .bind(i64::from(item))
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(read).collect()
}

/// What `links` — every link a reading listed, in order and each once —
/// changes of the links kept. The links kept are read through in the
/// table's order beside them, never held whole.
pub async fn compare(db: &Db, links: &[Link]) -> Result<Change> {
    if !links.windows(2).all(|pair| pair[0] < pair[1]) {
        bail!("the links to compare are not in order, or not each once");
    }

    let mut change = Change::default();
    let mut listed = links.iter().copied().peekable();
    let mut rows = sqlx::query(
        db.sql("SELECT property, value, item FROM wikidata_link ORDER BY property, value, item"),
    )
    .fetch(db.pool());

    while let Some(row) = rows.try_next().await? {
        let kept = read(&row)?;
        *change.before.entry(kept.property).or_default() += 1;
        // The listed links that come before this one are new; this one is
        // gone unless it is listed too.
        loop {
            match listed.peek() {
                Some(&next) if next < kept => {
                    change.added.push(next);
                    listed.next();
                }
                Some(&next) if next == kept => {
                    listed.next();
                    break;
                }
                _ => {
                    change.gone.push(kept);
                    break;
                }
            }
        }
    }
    change.added.extend(listed);

    Ok(change)
}

/// Write what a reading changes: in one transaction, but for a change larger
/// than [`BATCH`] on SQLite, written in several, the new links first — a
/// reader meanwhile finds every link of the last reading, and the new ones as
/// they land, until the gone ones go.
pub async fn apply(db: &Db, change: &Change) -> Result<()> {
    if change.is_empty() {
        return Ok(());
    }

    if db.dialect() == Dialect::Postgres || change.added.len() + change.gone.len() <= BATCH {
        let mut tx = db.begin_write().await?;
        insert(db, &mut tx, &change.added).await?;
        delete(db, &mut tx, &change.gone).await?;
        tx.commit()
            .await
            .context("the links read could not be kept")?;
        return Ok(());
    }

    let batches = change
        .added
        .chunks(BATCH)
        .map(|batch| (batch, true))
        .chain(change.gone.chunks(BATCH).map(|batch| (batch, false)));
    for (n, (batch, new)) in batches.enumerate() {
        if n > 0 {
            tokio::time::sleep(BETWEEN_BATCHES).await;
        }
        let mut tx = db.begin_write().await?;
        if new {
            insert(db, &mut tx, batch).await?;
        } else {
            delete(db, &mut tx, batch).await?;
        }
        tx.commit()
            .await
            .context("the links read could not be kept")?;
    }
    Ok(())
}

async fn insert(
    db: &Db,
    tx: &mut sqlx::Transaction<'static, sqlx::Any>,
    links: &[Link],
) -> Result<()> {
    for chunk in links.chunks(CHUNK) {
        let sql = format!(
            "INSERT INTO wikidata_link (property, value, item) VALUES {} ON CONFLICT DO NOTHING",
            vec!["(?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for link in chunk {
            for result in [
                args.add(i64::from(link.property)),
                args.add(i64::from(link.value)),
                args.add(i64::from(link.item)),
            ] {
                result.map_err(|e| anyhow!("{e}"))?;
            }
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn delete(
    db: &Db,
    tx: &mut sqlx::Transaction<'static, sqlx::Any>,
    links: &[Link],
) -> Result<()> {
    for link in links {
        sqlx::query(
            db.sql("DELETE FROM wikidata_link WHERE property = ? AND value = ? AND item = ?"),
        )
        .bind(i64::from(link.property))
        .bind(i64::from(link.value))
        .bind(i64::from(link.item))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn read(row: &sqlx::any::AnyRow) -> Result<Link> {
    let number = |column: &str| -> Result<i64> { Ok(row.big(column)?) };
    let property = number("property")?;
    let value = number("value")?;
    let item = number("item")?;
    Ok(Link {
        property: u16::try_from(property)
            .with_context(|| format!("a link kept names property {property}"))?,
        value: u32::try_from(value).with_context(|| format!("a link kept holds id {value}"))?,
        item: u32::try_from(item).with_context(|| format!("a link kept names item {item}"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(property: u16, value: u32, item: u32) -> Link {
        Link {
            property,
            value,
            item,
        }
    }

    async fn kept(db: &Db) -> Vec<Link> {
        let rows =
            sqlx::query(db.sql(
                "SELECT property, value, item FROM wikidata_link ORDER BY property, value, item",
            ))
            .fetch_all(db.pool())
            .await
            .expect("the links");
        rows.iter().map(|row| read(row).expect("a link")).collect()
    }

    #[tokio::test]
    async fn a_reading_writes_only_what_changed_and_an_id_leads_to_its_items_other_ids() {
        let db = crate::db::testing::db().await;

        // Breaking Bad: TMDB's series, TheTVDB's, IMDb's; a film beside it.
        let first = vec![
            link(345, 903_747, 1079),
            link(345, 1_375_666, 25188),
            link(4835, 81189, 1079),
            link(4947, 27205, 25188),
            link(4983, 1396, 1079),
        ];
        let change = compare(&db, &first).await.expect("compared");
        assert_eq!(change.added, first);
        assert!(change.gone.is_empty() && change.before.is_empty());
        apply(&db, &change).await.expect("applied");
        assert_eq!(kept(&db).await, first);

        // The film's IMDb id is gone from Wikidata, TVmaze's id of the
        // series has come: those two rows, and nothing else, are written.
        let second = vec![
            link(345, 903_747, 1079),
            link(4835, 81189, 1079),
            link(4947, 27205, 25188),
            link(4983, 1396, 1079),
            link(8600, 169, 1079),
        ];
        let change = compare(&db, &second).await.expect("compared");
        assert_eq!(change.gone, vec![link(345, 1_375_666, 25188)]);
        assert_eq!(change.added, vec![link(8600, 169, 1079)]);
        assert_eq!(
            change.before,
            BTreeMap::from([(345, 2), (4835, 1), (4947, 1), (4983, 1)])
        );
        apply(&db, &change).await.expect("applied");
        assert_eq!(kept(&db).await, second);

        // Nothing changed, nothing written.
        let change = compare(&db, &second).await.expect("compared");
        assert!(change.is_empty());

        // TMDB's id of the series leads to the whole item, its own among
        // them; the item itself to the same.
        let series = vec![
            link(345, 903_747, 1079),
            link(4835, 81189, 1079),
            link(4983, 1396, 1079),
            link(8600, 169, 1079),
        ];
        assert_eq!(of_id(&db, 4983, 1396).await.expect("read"), series);
        assert_eq!(of_item(&db, 1079).await.expect("read"), series);
        assert!(of_id(&db, 4983, 1397).await.expect("read").is_empty());
        assert!(of_item(&db, 1080).await.expect("read").is_empty());

        // A reading out of order is refused before anything is read.
        let backwards = vec![link(4983, 1396, 1079), link(345, 903_747, 1079)];
        assert!(compare(&db, &backwards).await.is_err());
        let twice = vec![link(345, 903_747, 1079), link(345, 903_747, 1079)];
        assert!(compare(&db, &twice).await.is_err());
    }

    #[tokio::test]
    async fn a_change_larger_than_a_batch_is_written_whole_all_the_same() {
        let db = crate::db::testing::db().await;
        let many: Vec<Link> = (1..=(BATCH as u32 + 5_000))
            .map(|n| link(4947, n, n))
            .collect();
        let change = compare(&db, &many).await.expect("compared");
        apply(&db, &change).await.expect("applied");
        assert_eq!(kept(&db).await, many);

        let fewer = &many[..1_000];
        let change = compare(&db, fewer).await.expect("compared");
        assert_eq!(change.gone.len(), BATCH + 4_000);
        apply(&db, &change).await.expect("applied");
        assert_eq!(kept(&db).await, fewer);
    }

    #[tokio::test]
    async fn an_id_on_two_items_leads_to_both() {
        let db = crate::db::testing::db().await;
        // Wikidata's duplicates: one TMDB film on two items, each with
        // another IMDb id.
        let links = vec![
            link(345, 1_259_014, 2_063_364),
            link(345, 1_259_015, 138_841_612),
            link(4947, 13635, 2_063_364),
            link(4947, 13635, 138_841_612),
        ];
        let change = compare(&db, &links).await.expect("compared");
        apply(&db, &change).await.expect("applied");

        assert_eq!(
            of_id(&db, 4947, 13635).await.expect("read"),
            vec![
                link(345, 1_259_014, 2_063_364),
                link(4947, 13635, 2_063_364),
                link(345, 1_259_015, 138_841_612),
                link(4947, 13635, 138_841_612),
            ]
        );
    }
}
