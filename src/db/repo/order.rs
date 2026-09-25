//! The other orders a series' episodes come in, kept beside the aired one.
//!
//! TheTVDB numbers many series more than one way: as they aired, as the DVDs
//! put them, straight through from the first, or in an order a network or a
//! country chose. The aired order is the work's own — its episodes, what every
//! client is served — and the rest are placings of those same episodes, by
//! TVDB id, read here only when a reader asks for them.

use anyhow::Result;

use crate::{
    db::{Db, RowExt, new_id, now},
    domain::{EpisodeOrder, PlacedEpisode},
};

/// The orders in the order they are offered: the DVDs first, since that is
/// the one a reader most often knows, then straight numbering, then the rest.
const OFFERED: &[&str] = &["dvd", "absolute", "alternate", "regional", "altdvd"];

/// Replace what is kept for a work with these orders, whole.
pub async fn replace(db: &Db, media_id: &str, orders: &[EpisodeOrder]) -> Result<()> {
    let mut tx = db.begin_write().await?;

    sqlx::query(db.sql("DELETE FROM media_episode_order WHERE media_id = ?"))
        .bind(media_id)
        .execute(&mut *tx)
        .await?;

    let created = now();
    for order in orders {
        for episode in &order.episodes {
            sqlx::query(db.sql(
                "INSERT INTO media_episode_order
                     (id, media_id, order_type, tvdb_episode_id, season_number, episode_number,
                      absolute_number, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            ))
            .bind(new_id())
            .bind(media_id)
            .bind(&order.kind)
            .bind(episode.tvdb_id)
            .bind(episode.season_number)
            .bind(episode.episode_number)
            .bind(episode.absolute_number)
            .bind(&created)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

/// Every order kept for a work, each with its episodes in its own order.
pub async fn list(db: &Db, media_id: &str) -> Result<Vec<EpisodeOrder>> {
    let rows = sqlx::query(db.sql(
        "SELECT order_type, tvdb_episode_id, season_number, episode_number, absolute_number
           FROM media_episode_order WHERE media_id = ?
          ORDER BY order_type, season_number, episode_number",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let mut orders: Vec<EpisodeOrder> = Vec::new();
    for row in &rows {
        let kind = row.text("order_type")?;
        let placed = PlacedEpisode {
            tvdb_id: row.big("tvdb_episode_id")?,
            season_number: row.int("season_number")?,
            episode_number: row.int("episode_number")?,
            absolute_number: row.opt_int("absolute_number")?,
        };
        match orders.iter_mut().find(|o| o.kind == kind) {
            Some(order) => order.episodes.push(placed),
            None => orders.push(EpisodeOrder {
                kind,
                episodes: vec![placed],
            }),
        }
    }

    orders.sort_by_key(|o| rank(&o.kind));
    Ok(orders)
}

/// Where an order stands among those offered; an unknown one last, by name.
fn rank(kind: &str) -> (usize, String) {
    (
        OFFERED
            .iter()
            .position(|k| *k == kind)
            .unwrap_or(OFFERED.len()),
        kind.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        db::repo::item::{self, ItemWrite},
        domain::{MediaItem, MediaKind},
    };

    async fn db() -> Db {
        let db = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .expect("in-memory database");
        db.migrate().await.expect("migrations");
        db
    }

    fn placed(tvdb_id: i64, season: i32, number: i32) -> PlacedEpisode {
        PlacedEpisode {
            tvdb_id,
            season_number: season,
            episode_number: number,
            absolute_number: None,
        }
    }

    #[tokio::test]
    async fn the_orders_are_kept_whole_and_offered_dvd_first() {
        let db = db().await;
        let mut work = MediaItem::empty(MediaKind::Series);
        work.title = "Firefly".into();
        work.slug = "firefly-2002".into();
        item::upsert(
            &db,
            ItemWrite {
                item: &work,
                replace_children: false,
            },
        )
        .await
        .expect("stored");

        let orders = vec![
            EpisodeOrder {
                kind: "absolute".into(),
                episodes: vec![placed(297999, 1, 1), placed(297989, 1, 2)],
            },
            EpisodeOrder {
                kind: "dvd".into(),
                episodes: vec![placed(297989, 1, 1), placed(297999, 1, 2)],
            },
        ];
        replace(&db, &work.id, &orders).await.expect("written");

        let read = list(&db, &work.id).await.expect("read");
        assert_eq!(
            read.iter().map(|o| o.kind.as_str()).collect::<Vec<_>>(),
            ["dvd", "absolute"]
        );
        assert_eq!(
            read[0]
                .episodes
                .iter()
                .map(|e| (e.tvdb_id, e.episode_number))
                .collect::<Vec<_>>(),
            [(297989, 1), (297999, 2)]
        );

        // Replaced whole: a series that lost its DVD order loses it here too.
        replace(&db, &work.id, &orders[..1])
            .await
            .expect("rewritten");
        let read = list(&db, &work.id).await.expect("read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].kind, "absolute");

        // And nothing for a work nothing was kept for.
        assert!(list(&db, "nobody").await.expect("read").is_empty());
    }
}
