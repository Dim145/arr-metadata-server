//! The orders a series' episodes come in, kept beside the aired one.
//!
//! TheTVDB numbers many series more than one way: as they aired, as the DVDs
//! put them, straight through from the first, or in an order a network, a
//! country or a streaming service chose. TMDB keeps episode groups that do
//! the same, and a person may make an order of their own. The aired order is
//! the work's own — its episodes, what every client is served unless it asks
//! for another — and each of the others is kept here as an order of its own:
//! a sequence of places, each naming an episode the work holds or carrying
//! the record of one it does not.
//!
//! Who numbers the aired order, and whether TMDB does so in TheTVDB's place,
//! is kept here too: see [`Numbering`].

use anyhow::{Context, Result};
use sqlx::{Any, Transaction};

use crate::{
    db::{Db, RowExt, from_bool, new_id, now},
    domain::{EpisodeOrder, OrderKind, OrderRecord, PlacedEpisode},
    merge::provenance::Provenance,
};

/// What TheTVDB, and Sonarr after it, call the aired order.
pub const AIRED: &str = "official";

/// What the aired order is called, as TheTVDB calls it.
pub const AIRED_LABEL: &str = "Aired Order";

/// Where an order made here comes from.
pub const LOCAL: &str = "local";

/// TheTVDB's orders in the order they are offered: the DVDs first, since that
/// is the one a reader most often knows, then straight numbering, then the
/// series' own.
const TVDB_OFFERED: &[&str] = &[
    "dvd",
    "absolute",
    "alternate",
    "regional",
    "altdvd",
    "alttwo",
];

/// Every order kept for a series, the aired one first — whose places are the
/// work's episodes and are not listed — then TheTVDB's, then TMDB's groups,
/// then those made here, each with its places in its own sequence. Nothing
/// for a film, or a work not held.
pub async fn list(db: &Db, media_id: &str) -> Result<Vec<EpisodeOrder>> {
    let Some(aired) = aired(db, media_id).await? else {
        return Ok(Vec::new());
    };

    let rows = sqlx::query(db.sql(
        "SELECT o.id AS order_id, o.key, o.kind, o.source, o.source_ref, o.label, o.is_default,
                o.is_frozen, o.announced, o.updated_at,
                e.id AS place_id, e.position, e.season_number, e.episode_number,
                e.absolute_number, e.episode_id, e.tvdb_episode_id, e.tmdb_episode_id,
                e.title, e.overview, e.aired, e.runtime, e.image, e.finale_type
           FROM media_order o
           LEFT JOIN media_order_episode e ON e.order_id = o.id
          WHERE o.media_id = ?
          ORDER BY o.key, e.position",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let mut orders: Vec<EpisodeOrder> = Vec::new();
    let mut last: Option<String> = None;
    for row in &rows {
        let id = row.text("order_id")?;
        if last.as_deref() != Some(id.as_str()) {
            orders.push(header(row)?);
            last = Some(id);
        }
        if let Some(place) = place(row)? {
            orders
                .last_mut()
                .expect("a header before its places")
                .episodes
                .push(place);
        }
    }

    orders.sort_by_key(rank);
    orders.insert(0, aired);
    Ok(orders)
}

/// One order kept for a work, by its key — the aired order included, without
/// its places.
pub async fn get(db: &Db, media_id: &str, key: &str) -> Result<Option<EpisodeOrder>> {
    Ok(list(db, media_id)
        .await?
        .into_iter()
        .find(|order| order.key == key))
}

/// The aired order of a series: its source is whoever numbers the work's
/// episodes, as the last merge said — TheTVDB, whether it answered itself or
/// through Skyhook; TMDB, for a series it alone numbers. `None` for a film or
/// a work not held.
async fn aired(db: &Db, media_id: &str) -> Result<Option<EpisodeOrder>> {
    let row = sqlx::query(db.sql(
        "SELECT kind, is_manual, provenance, updated_at,
                (SELECT COUNT(*) FROM media_external_id x
                  WHERE x.media_id = media_item.id AND x.source = 'tvdb_series') AS on_tvdb
           FROM media_item WHERE id = ?",
    ))
    .bind(media_id)
    .fetch_optional(db.pool())
    .await?;
    let Some(row) = row else { return Ok(None) };
    if row.text("kind")? != "series" {
        return Ok(None);
    }

    let numbered_by = row
        .opt_text("provenance")?
        .and_then(|json| serde_json::from_str::<Provenance>(&json).ok())
        .and_then(|provenance| provenance.episodes);
    let source = match numbered_by.as_deref() {
        Some("tvdb" | "skyhook") => "tvdb".to_string(),
        Some(other) => other.to_string(),
        None if row.big("on_tvdb")? > 0 => "tvdb".to_string(),
        None if row.flag("is_manual")? => LOCAL.to_string(),
        None => "tmdb".to_string(),
    };

    Ok(Some(EpisodeOrder {
        key: AIRED.to_string(),
        kind: OrderKind::Aired,
        source,
        source_ref: None,
        label: AIRED_LABEL.to_string(),
        is_default: true,
        is_frozen: false,
        announced: None,
        updated_at: row.text("updated_at")?,
        episodes: Vec::new(),
    }))
}

fn header(row: &sqlx::any::AnyRow) -> Result<EpisodeOrder> {
    Ok(EpisodeOrder {
        key: row.text("key")?,
        kind: row.text("kind")?.parse()?,
        source: row.text("source")?,
        source_ref: row.opt_text("source_ref")?,
        label: row.text("label")?,
        is_default: row.flag("is_default")?,
        is_frozen: row.flag("is_frozen")?,
        announced: row.opt_int("announced")?,
        updated_at: row.text("updated_at")?,
        episodes: Vec::new(),
    })
}

/// A place of the row's order, when the row carries one: a header joined to
/// none carries empty columns.
fn place(row: &sqlx::any::AnyRow) -> Result<Option<PlacedEpisode>> {
    let Some(id) = row.opt_text("place_id")? else {
        return Ok(None);
    };
    let record = match row.opt_text("title")? {
        Some(title) => Some(OrderRecord {
            title,
            overview: row.opt_text("overview")?,
            aired: row.opt_text("aired")?,
            runtime: row.opt_int("runtime")?,
            image: row.opt_text("image")?,
            finale_type: row.opt_text("finale_type")?,
        }),
        None => None,
    };
    Ok(Some(PlacedEpisode {
        id,
        season_number: row.int("season_number")?,
        episode_number: row.int("episode_number")?,
        absolute_number: row.opt_int("absolute_number")?,
        episode_id: row.opt_text("episode_id")?,
        tvdb_id: row.opt_big("tvdb_episode_id")?,
        tmdb_id: row.opt_big("tmdb_episode_id")?,
        record,
    }))
}

/// Where an order stands among those offered: TheTVDB's in the order of
/// [`TVDB_OFFERED`], an unknown type of theirs after, by name; then TMDB's
/// groups by kind and name; then those made here, by name.
fn rank(order: &EpisodeOrder) -> (usize, usize, String) {
    let kind = |kind: OrderKind| {
        [
            OrderKind::Aired,
            OrderKind::Dvd,
            OrderKind::Absolute,
            OrderKind::Digital,
            OrderKind::Production,
            OrderKind::StoryArc,
            OrderKind::Tv,
            OrderKind::TvdbAlt,
            OrderKind::Custom,
        ]
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(usize::MAX)
    };
    match order.source.as_str() {
        "tvdb" => (
            0,
            TVDB_OFFERED
                .iter()
                .position(|k| *k == order.key)
                .unwrap_or(TVDB_OFFERED.len()),
            order.key.clone(),
        ),
        "tmdb" => (1, kind(order.kind), order.label.to_lowercase()),
        _ => (2, 0, order.label.to_lowercase()),
    }
}

/// Replace the orders a source keeps for a work with these, whole: an order
/// it no longer gives is gone, one it still gives keeps its row and whether
/// it is frozen, and its places are written again. What is kept of the other
/// sources, and what was made here, is left alone.
pub async fn replace_source(
    db: &Db,
    media_id: &str,
    source: &str,
    orders: &[EpisodeOrder],
) -> Result<()> {
    let mut tx = db.begin_write().await?;
    let at = now();

    let held = sqlx::query(
        db.sql("SELECT id, key, is_frozen FROM media_order WHERE media_id = ? AND source = ?"),
    )
    .bind(media_id)
    .bind(source)
    .fetch_all(&mut *tx)
    .await?;

    for row in &held {
        let key = row.text("key")?;
        if orders.iter().any(|order| order.key == key) {
            continue;
        }
        if row.flag("is_frozen")? {
            tracing::warn!(
                media_id,
                key,
                "an order frozen as the aired numbering is no longer given by its source; the \
                 aired order answers again"
            );
        }
        sqlx::query(db.sql("DELETE FROM media_order WHERE id = ?"))
            .bind(row.text("id")?)
            .execute(&mut *tx)
            .await?;
    }

    for order in orders {
        let existing = held
            .iter()
            .find(|row| row.text("key").is_ok_and(|key| key == order.key));
        let id = match existing {
            Some(row) => {
                let id = row.text("id")?;
                sqlx::query(db.sql(
                    "UPDATE media_order
                        SET kind = ?, source_ref = ?, label = ?, announced = ?, read_at = ?,
                            updated_at = ?
                      WHERE id = ?",
                ))
                .bind(order.kind.as_str())
                .bind(&order.source_ref)
                .bind(&order.label)
                .bind(order.announced)
                .bind(&at)
                .bind(&at)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
                id
            }
            None => {
                let id = new_id();
                insert_header(db, &mut tx, &id, media_id, order, Some(&at), &at).await?;
                id
            }
        };
        write_places(db, &mut tx, &id, &order.episodes).await?;
    }

    tx.commit().await?;
    Ok(())
}

/// The episode groups TMDB lists for a work, as orders still to be read:
/// a group new here is kept with no places, one whose count of episodes moved
/// is to be read again, and one TMDB no longer lists is gone. Its places are
/// read the first time it is served or shown ([`write_read`]).
pub async fn list_groups(db: &Db, media_id: &str, groups: &[EpisodeOrder]) -> Result<()> {
    let mut tx = db.begin_write().await?;
    let at = now();

    let held = sqlx::query(db.sql(
        "SELECT id, key, announced, is_frozen FROM media_order
          WHERE media_id = ? AND source = 'tmdb'",
    ))
    .bind(media_id)
    .fetch_all(&mut *tx)
    .await?;

    for row in &held {
        let key = row.text("key")?;
        if !groups.iter().any(|group| group.key == key) {
            if row.flag("is_frozen")? {
                tracing::warn!(
                    media_id,
                    key,
                    "a TMDB group frozen as the aired numbering is no longer listed by TMDB; the \
                     aired order answers again"
                );
            }
            sqlx::query(db.sql("DELETE FROM media_order WHERE id = ?"))
                .bind(row.text("id")?)
                .execute(&mut *tx)
                .await?;
        }
    }

    for group in groups {
        match held
            .iter()
            .find(|row| row.text("key").is_ok_and(|key| key == group.key))
        {
            Some(row) => {
                // Its count moved: what it holds changed, and is read again
                // the next time it is asked for. What was read stands until.
                let moved = row.opt_int("announced")? != group.announced;
                sqlx::query(db.sql(if moved {
                    "UPDATE media_order SET kind = ?, label = ?, announced = ?, read_at = NULL,
                            updated_at = ?
                      WHERE id = ?"
                } else {
                    "UPDATE media_order SET kind = ?, label = ?, announced = ?, updated_at = ?
                      WHERE id = ?"
                }))
                .bind(group.kind.as_str())
                .bind(&group.label)
                .bind(group.announced)
                .bind(&at)
                .bind(row.text("id")?)
                .execute(&mut *tx)
                .await?;
            }
            None => {
                insert_header(db, &mut tx, &new_id(), media_id, group, None, &at).await?;
            }
        }
    }

    tx.commit().await?;
    Ok(())
}

/// The keys of the orders kept for a work whose places are still to be read
/// from their source.
pub async fn unread(db: &Db, media_id: &str) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(db.sql(
        "SELECT key, source_ref FROM media_order
          WHERE media_id = ? AND read_at IS NULL AND source_ref IS NOT NULL",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| Ok((row.text("key")?, row.text("source_ref")?)))
        .collect()
}

/// The places read from an order's source, written as its own: the order is
/// read as of now.
pub async fn write_read(
    db: &Db,
    media_id: &str,
    key: &str,
    places: &[PlacedEpisode],
) -> Result<bool> {
    let mut tx = db.begin_write().await?;
    let Some(id) = order_id(db, &mut tx, media_id, key).await? else {
        return Ok(false);
    };
    let at = now();
    sqlx::query(db.sql("UPDATE media_order SET read_at = ?, updated_at = ? WHERE id = ?"))
        .bind(&at)
        .bind(&at)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    write_places(db, &mut tx, &id, places).await?;
    tx.commit().await?;
    Ok(true)
}

/// Keep an order made here: its header and its places.
pub async fn create(db: &Db, media_id: &str, order: &EpisodeOrder) -> Result<()> {
    let mut tx = db.begin_write().await?;
    let id = new_id();
    let at = now();
    insert_header(db, &mut tx, &id, media_id, order, None, &at).await?;
    write_places(db, &mut tx, &id, &order.episodes).await?;
    tx.commit().await?;
    Ok(())
}

/// Write an order made here again: its name and its places, whole. False
/// when the work keeps no order of that key made here.
pub async fn save(
    db: &Db,
    media_id: &str,
    key: &str,
    label: &str,
    places: &[PlacedEpisode],
) -> Result<bool> {
    let mut tx = db.begin_write().await?;
    let row = sqlx::query(
        db.sql("SELECT id FROM media_order WHERE media_id = ? AND key = ? AND source = ?"),
    )
    .bind(media_id)
    .bind(key)
    .bind(LOCAL)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else { return Ok(false) };
    let id = row.text("id")?;
    sqlx::query(db.sql("UPDATE media_order SET label = ?, updated_at = ? WHERE id = ?"))
        .bind(label)
        .bind(now())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    write_places(db, &mut tx, &id, places).await?;
    tx.commit().await?;
    Ok(true)
}

/// Forget an order made here. False when the work keeps none of that key.
pub async fn delete(db: &Db, media_id: &str, key: &str) -> Result<bool> {
    let done = sqlx::query(
        db.sql("DELETE FROM media_order WHERE media_id = ? AND key = ? AND source = ?"),
    )
    .bind(media_id)
    .bind(key)
    .bind(LOCAL)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Freeze one order as the aired numbering, or none: every other order of
/// the work is thawed in the same write, so that one at the most is frozen.
/// False when the work keeps no order of that key.
pub async fn freeze(db: &Db, media_id: &str, key: Option<&str>) -> Result<bool> {
    let mut tx = db.begin_write().await?;
    if let Some(key) = key
        && order_id(db, &mut tx, media_id, key).await?.is_none()
    {
        return Ok(false);
    }
    let at = now();
    sqlx::query(
        db.sql("UPDATE media_order SET is_frozen = 0 WHERE media_id = ? AND is_frozen = 1"),
    )
    .bind(media_id)
    .execute(&mut *tx)
    .await?;
    if let Some(key) = key {
        // The moment it was frozen is what tells an episode aired after it.
        sqlx::query(db.sql(
            "UPDATE media_order SET is_frozen = 1, updated_at = ? WHERE media_id = ? AND key = ?",
        ))
        .bind(&at)
        .bind(media_id)
        .bind(key)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// The key of the order frozen as a work's aired numbering, if one is.
pub async fn frozen(db: &Db, media_id: &str) -> Result<Option<String>> {
    let row =
        sqlx::query(db.sql("SELECT key FROM media_order WHERE media_id = ? AND is_frozen = 1"))
            .bind(media_id)
            .fetch_optional(db.pool())
            .await?;
    row.map(|row| row.text("key"))
        .transpose()
        .map_err(Into::into)
}

/// Every key a work's orders are asked for by.
pub async fn keys(db: &Db, media_id: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql("SELECT key FROM media_order WHERE media_id = ?"))
        .bind(media_id)
        .fetch_all(db.pool())
        .await?;
    rows.iter()
        .map(|row| row.text("key").map_err(Into::into))
        .collect()
}

async fn order_id(
    db: &Db,
    tx: &mut Transaction<'static, Any>,
    media_id: &str,
    key: &str,
) -> Result<Option<String>> {
    let row = sqlx::query(db.sql("SELECT id FROM media_order WHERE media_id = ? AND key = ?"))
        .bind(media_id)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| row.text("id"))
        .transpose()
        .map_err(Into::into)
}

async fn insert_header(
    db: &Db,
    tx: &mut Transaction<'static, Any>,
    id: &str,
    media_id: &str,
    order: &EpisodeOrder,
    read_at: Option<&str>,
    at: &str,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO media_order
             (id, media_id, key, kind, source, source_ref, label, is_default, is_frozen,
              announced, read_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(id)
    .bind(media_id)
    .bind(&order.key)
    .bind(order.kind.as_str())
    .bind(&order.source)
    .bind(&order.source_ref)
    .bind(&order.label)
    .bind(from_bool(order.is_default))
    .bind(from_bool(order.is_frozen))
    .bind(order.announced)
    .bind(read_at)
    .bind(at)
    .bind(at)
    .execute(&mut **tx)
    .await
    .context("failed to write an order")?;
    Ok(())
}

/// An order's places, whole, in the sequence given.
async fn write_places(
    db: &Db,
    tx: &mut Transaction<'static, Any>,
    order_id: &str,
    places: &[PlacedEpisode],
) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_order_episode WHERE order_id = ?"))
        .bind(order_id)
        .execute(&mut **tx)
        .await?;

    for (position, place) in places.iter().enumerate() {
        let record = place.record.as_ref();
        sqlx::query(db.sql(
            "INSERT INTO media_order_episode
                 (id, order_id, position, season_number, episode_number, absolute_number,
                  episode_id, tvdb_episode_id, tmdb_episode_id, title, overview, aired, runtime,
                  image, finale_type)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        ))
        .bind(if place.id.is_empty() {
            new_id()
        } else {
            place.id.clone()
        })
        .bind(order_id)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .bind(place.season_number)
        .bind(place.episode_number)
        .bind(place.absolute_number)
        .bind(&place.episode_id)
        .bind(place.tvdb_id)
        .bind(place.tmdb_id)
        .bind(record.map(|r| r.title.clone()))
        .bind(record.and_then(|r| r.overview.clone()))
        .bind(record.and_then(|r| r.aired.clone()))
        .bind(record.and_then(|r| r.runtime))
        .bind(record.and_then(|r| r.image.clone()))
        .bind(record.and_then(|r| r.finale_type.clone()))
        .execute(&mut **tx)
        .await
        .context("failed to write an order's place")?;
    }
    Ok(())
}

// ─── who numbers the aired order ─────────────────────────────────────────────

/// Who numbers a series while TheTVDB does not answer for it: how many
/// refreshes in a row it has not, a day apart at the least, the last of them
/// counted when; and since when TMDB numbers the series in its place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Numbering {
    pub silent: i32,
    pub silent_at: Option<String>,
    pub since: Option<String>,
}

/// What is kept of a work's numbering: nothing, for one TheTVDB has always
/// answered for.
pub async fn numbering(db: &Db, media_id: &str) -> Result<Numbering> {
    let row = sqlx::query(
        db.sql("SELECT silent, silent_at, since FROM media_numbering WHERE media_id = ?"),
    )
    .bind(media_id)
    .fetch_optional(db.pool())
    .await?;
    Ok(match row {
        Some(row) => Numbering {
            silent: row.int("silent")?,
            silent_at: row.opt_text("silent_at")?,
            since: row.opt_text("since")?,
        },
        None => Numbering::default(),
    })
}

/// Keep what a refresh made of a work's numbering. One with nothing to say
/// — TheTVDB answering, TMDB not numbering — is not kept at all.
pub async fn set_numbering(db: &Db, media_id: &str, numbering: &Numbering) -> Result<()> {
    if *numbering == Numbering::default() {
        sqlx::query(db.sql("DELETE FROM media_numbering WHERE media_id = ?"))
            .bind(media_id)
            .execute(db.pool())
            .await?;
        return Ok(());
    }
    sqlx::query(db.sql(
        "INSERT INTO media_numbering (media_id, silent, silent_at, since, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (media_id) DO UPDATE SET
             silent = excluded.silent,
             silent_at = excluded.silent_at,
             since = excluded.since,
             updated_at = excluded.updated_at",
    ))
    .bind(media_id)
    .bind(numbering.silent)
    .bind(&numbering.silent_at)
    .bind(&numbering.since)
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::{
            repo::item::{self, ItemWrite},
            testing::db,
        },
        domain::{MediaItem, MediaKind},
    };

    fn placed(tvdb_id: i64, season: i32, number: i32) -> PlacedEpisode {
        PlacedEpisode::of_tvdb(tvdb_id, season, number)
    }

    fn tvdb(key: &str, label: &str, episodes: Vec<PlacedEpisode>) -> EpisodeOrder {
        EpisodeOrder {
            key: key.into(),
            kind: OrderKind::of_tvdb(key),
            source: "tvdb".into(),
            source_ref: None,
            label: label.into(),
            is_default: false,
            is_frozen: false,
            announced: None,
            updated_at: String::new(),
            episodes,
        }
    }

    async fn firefly(db: &Db) -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = "Firefly".into();
        work.slug = "firefly-2002".into();
        item::upsert(
            db,
            ItemWrite {
                item: &work,
                replace_children: false,
            },
        )
        .await
        .expect("stored");
        work
    }

    #[tokio::test]
    async fn the_orders_are_kept_whole_and_offered_after_the_aired_one_dvd_first() {
        let db = db().await;
        let work = firefly(&db).await;

        let mut extended = placed(10718410, 1, 3);
        extended.record = Some(OrderRecord {
            title: "Pilot (Extended Cut)".into(),
            overview: None,
            aired: Some("2005-03-24".into()),
            runtime: Some(38),
            image: None,
            finale_type: None,
        });
        let orders = vec![
            tvdb(
                "absolute",
                "Absolute Order",
                vec![placed(297999, 1, 1), placed(297989, 1, 2)],
            ),
            tvdb(
                "dvd",
                "DVD Order",
                vec![placed(297989, 1, 1), placed(297999, 1, 2), extended],
            ),
        ];
        replace_source(&db, &work.id, "tvdb", &orders)
            .await
            .expect("written");

        let read = list(&db, &work.id).await.expect("read");
        assert_eq!(
            read.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(),
            ["official", "dvd", "absolute"]
        );
        assert!(read[0].is_default && read[0].episodes.is_empty());
        assert_eq!(read[0].kind, OrderKind::Aired);
        let dvd = &read[1];
        assert_eq!(dvd.label, "DVD Order");
        assert_eq!(
            dvd.episodes
                .iter()
                .map(|e| (e.tvdb_id, e.episode_number))
                .collect::<Vec<_>>(),
            [(Some(297989), 1), (Some(297999), 2), (Some(10718410), 3)]
        );
        // The record an order carries of an episode the aired list lacks.
        assert_eq!(
            dvd.episodes[2].record.as_ref().map(|r| r.title.as_str()),
            Some("Pilot (Extended Cut)")
        );
        assert_eq!(dvd.episodes[0].record, None);

        // Frozen, and read again from TheTVDB: still frozen. Replaced whole:
        // a series that lost its DVD order loses it here too.
        assert!(freeze(&db, &work.id, Some("absolute")).await.unwrap());
        replace_source(&db, &work.id, "tvdb", &orders[..1])
            .await
            .expect("rewritten");
        let read = list(&db, &work.id).await.expect("read");
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].key, "absolute");
        assert!(read[1].is_frozen);
        assert_eq!(
            frozen(&db, &work.id).await.unwrap().as_deref(),
            Some("absolute")
        );

        // And nothing for a work nothing was kept for.
        assert!(list(&db, "nobody").await.expect("read").is_empty());
    }

    #[tokio::test]
    async fn one_order_at_the_most_is_frozen_and_none_once_thawed() {
        let db = db().await;
        let work = firefly(&db).await;
        let mine = EpisodeOrder {
            key: "ams-mine".into(),
            kind: OrderKind::Custom,
            source: LOCAL.into(),
            label: "Mine".into(),
            ..tvdb("dvd", "DVD Order", vec![placed(297989, 1, 1)])
        };
        replace_source(&db, &work.id, "tvdb", &[tvdb("dvd", "DVD Order", vec![])])
            .await
            .unwrap();
        create(&db, &work.id, &mine).await.unwrap();

        assert!(freeze(&db, &work.id, Some("dvd")).await.unwrap());
        assert!(freeze(&db, &work.id, Some("ams-mine")).await.unwrap());
        let frozen_now: Vec<String> = list(&db, &work.id)
            .await
            .unwrap()
            .into_iter()
            .filter(|o| o.is_frozen)
            .map(|o| o.key)
            .collect();
        assert_eq!(frozen_now, ["ams-mine"]);
        // An order the work does not keep is not frozen, and thaws nothing.
        assert!(!freeze(&db, &work.id, Some("nonsense")).await.unwrap());
        assert_eq!(
            frozen(&db, &work.id).await.unwrap().as_deref(),
            Some("ams-mine")
        );

        assert!(freeze(&db, &work.id, None).await.unwrap());
        assert_eq!(frozen(&db, &work.id).await.unwrap(), None);

        // Only an order made here is written by hand, or forgotten.
        assert!(!save(&db, &work.id, "dvd", "Mine", &[]).await.unwrap());
        assert!(
            save(&db, &work.id, "ams-mine", "Still mine", &[])
                .await
                .unwrap()
        );
        assert!(!delete(&db, &work.id, "dvd").await.unwrap());
        assert!(delete(&db, &work.id, "ams-mine").await.unwrap());
        assert_eq!(keys(&db, &work.id).await.unwrap(), ["dvd"]);
    }

    #[tokio::test]
    async fn a_group_is_listed_unread_and_read_again_when_its_count_moves() {
        let db = db().await;
        let work = firefly(&db).await;
        let group = |announced: i32| EpisodeOrder {
            key: "tmdb-5acf93e60e0a26346d0000ce".into(),
            kind: OrderKind::Dvd,
            source: "tmdb".into(),
            source_ref: Some("5acf93e60e0a26346d0000ce".into()),
            label: "Blu-ray Order".into(),
            announced: Some(announced),
            ..tvdb("dvd", "DVD Order", vec![])
        };

        list_groups(&db, &work.id, &[group(14)]).await.unwrap();
        assert_eq!(
            unread(&db, &work.id).await.unwrap(),
            [(
                "tmdb-5acf93e60e0a26346d0000ce".to_string(),
                "5acf93e60e0a26346d0000ce".to_string()
            )]
        );
        let mut first = placed(297989, 1, 1);
        first.tmdb_id = Some(1001);
        assert!(
            write_read(&db, &work.id, "tmdb-5acf93e60e0a26346d0000ce", &[first])
                .await
                .unwrap()
        );
        assert!(unread(&db, &work.id).await.unwrap().is_empty());

        // Listed again as it was: still read. With another count: to read.
        list_groups(&db, &work.id, &[group(14)]).await.unwrap();
        assert!(unread(&db, &work.id).await.unwrap().is_empty());
        list_groups(&db, &work.id, &[group(15)]).await.unwrap();
        assert_eq!(unread(&db, &work.id).await.unwrap().len(), 1);
        // What was read stands until it is read again.
        let read = get(&db, &work.id, "tmdb-5acf93e60e0a26346d0000ce")
            .await
            .unwrap()
            .expect("kept");
        assert_eq!(read.episodes.len(), 1);
        assert_eq!(read.announced, Some(15));

        // No longer listed by TMDB: gone.
        list_groups(&db, &work.id, &[]).await.unwrap();
        assert!(keys(&db, &work.id).await.unwrap().is_empty());
    }

    /// What 0013 kept — the DVD and the Superfan orders of a series, by
    /// TheTVDB id — is an order of its own each after 0037, named as
    /// TheTVDB names the type until the next refresh reads the series'
    /// own name, its places in the order of their numbers; and that refresh
    /// is brought forward to the upgrade, where a series with no order kept,
    /// or one due already, keeps its schedule.
    #[tokio::test]
    async fn the_orders_kept_before_are_carried_into_orders_of_their_own() {
        let db = Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .expect("a database");
        let migrations = crate::db::migrations(db.dialect());
        for migration in migrations.iter().filter(|m| m.version < 37) {
            sqlx::raw_sql(migration.sql.clone())
                .execute(db.pool())
                .await
                .unwrap_or_else(|e| panic!("migration {}: {e}", migration.version));
        }
        let work = firefly(&db).await;
        // Two series beside it: one with no order kept, one with an order
        // and its refresh due already.
        let another = |title: &str, slug: &str| {
            let mut series = MediaItem::empty(MediaKind::Series);
            series.title = title.into();
            series.slug = slug.into();
            series
        };
        let unordered = another("Dollhouse", "dollhouse-2009");
        let due = another("Angel", "angel-1999");
        for series in [&unordered, &due] {
            item::upsert(
                &db,
                ItemWrite {
                    item: series,
                    replace_children: false,
                },
            )
            .await
            .expect("stored");
        }
        let scheduled = |id: String, at: &'static str| {
            let db = &db;
            async move {
                sqlx::query(db.sql("UPDATE media_item SET refresh_after = ? WHERE id = ?"))
                    .bind(at)
                    .bind(id)
                    .execute(db.pool())
                    .await
                    .expect("scheduled");
            }
        };
        const LATER: &str = "2099-01-01T00:00:00.000Z";
        const EARLIER: &str = "2020-01-01T00:00:00.000Z";
        scheduled(work.id.clone(), LATER).await;
        scheduled(unordered.id.clone(), LATER).await;
        scheduled(due.id.clone(), EARLIER).await;

        for (id, media, kind, tvdb_id, season, number) in [
            ("p1", &work.id, "dvd", 297999, 1, 2),
            ("p2", &work.id, "dvd", 297989, 1, 1),
            ("p3", &work.id, "alttwo", 10718410, 1, 1),
            ("p4", &due.id, "dvd", 400001, 1, 1),
        ] {
            sqlx::query(db.sql(
                "INSERT INTO media_episode_order
                     (id, media_id, order_type, tvdb_episode_id, season_number, episode_number,
                      absolute_number, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, NULL, '2026-10-01T00:00:00.000Z')",
            ))
            .bind(id)
            .bind(media)
            .bind(kind)
            .bind(tvdb_id)
            .bind(season)
            .bind(number)
            .execute(db.pool())
            .await
            .expect("kept as 0013 kept it");
        }
        let orders = migrations
            .iter()
            .find(|m| m.version == 37)
            .expect("the migration of the orders");
        sqlx::raw_sql(orders.sql.clone())
            .execute(db.pool())
            .await
            .expect("migrated");

        let read = list(&db, &work.id).await.expect("read");
        let shapes: Vec<(&str, OrderKind, &str, &str)> = read
            .iter()
            .map(|o| (o.key.as_str(), o.kind, o.source.as_str(), o.label.as_str()))
            .collect();
        assert_eq!(
            shapes,
            [
                // No TheTVDB id, and no source on record for its list.
                ("official", OrderKind::Aired, "tmdb", "Aired Order"),
                ("dvd", OrderKind::Dvd, "tvdb", "DVD Order"),
                ("alttwo", OrderKind::TvdbAlt, "tvdb", "Alternate Order 2"),
            ]
        );
        let dvd = &read[1];
        assert_eq!(
            dvd.episodes
                .iter()
                .map(|e| (e.id.as_str(), e.tvdb_id, e.episode_number))
                .collect::<Vec<_>>(),
            [("p2", Some(297989), 1), ("p1", Some(297999), 2)]
        );
        assert!(dvd.episodes.iter().all(|e| e.record.is_none()));

        // Its refresh is due from the upgrade on, which reads its orders as
        // they are now; the others' schedules stand.
        let next = |id: String| {
            let db = &db;
            async move {
                sqlx::query(db.sql("SELECT refresh_after FROM media_item WHERE id = ?"))
                    .bind(id)
                    .fetch_one(db.pool())
                    .await
                    .expect("read")
                    .text("refresh_after")
                    .expect("scheduled")
            }
        };
        let brought = next(work.id.clone()).await;
        assert!(
            brought.as_str() <= crate::db::now().as_str() && brought.as_str() > EARLIER,
            "due at the upgrade, not {brought}"
        );
        assert_eq!(next(unordered.id.clone()).await, LATER);
        assert_eq!(next(due.id.clone()).await, EARLIER);

        // The table it was kept in is gone.
        assert!(
            sqlx::query(db.sql("SELECT 1 FROM media_episode_order"))
                .fetch_optional(db.pool())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_numbering_with_nothing_to_say_is_not_kept() {
        let db = db().await;
        let work = firefly(&db).await;
        assert_eq!(
            numbering(&db, &work.id).await.unwrap(),
            Numbering::default()
        );
        let held = Numbering {
            silent: 3,
            silent_at: Some("2026-10-19T00:00:00.000Z".into()),
            since: Some("2026-10-19T00:00:00.000Z".into()),
        };
        set_numbering(&db, &work.id, &held).await.unwrap();
        assert_eq!(numbering(&db, &work.id).await.unwrap(), held);
        set_numbering(&db, &work.id, &Numbering::default())
            .await
            .unwrap();
        assert_eq!(
            numbering(&db, &work.id).await.unwrap(),
            Numbering::default()
        );
    }
}
