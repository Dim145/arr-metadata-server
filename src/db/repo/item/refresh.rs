//! The refresh's bookkeeping: where a work's values came from, which works
//! are due, what a refresh of everything works through, and how the last
//! refresh of a work went.

use anyhow::Result;

use crate::{
    db::{Db, RowExt, now},
    domain::MediaKind,
};

/// Who gave a work its values, when a merge has said.
pub async fn provenance(db: &Db, id: &str) -> Result<Option<crate::merge::provenance::Provenance>> {
    let row = sqlx::query(db.sql("SELECT provenance FROM media_item WHERE id = ?"))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    Ok(row
        .and_then(|row| row.opt_text("provenance").ok().flatten())
        .and_then(|json| serde_json::from_str(&json).ok()))
}

/// What a refresh of everything works through: the enabled works that have
/// an identifier elsewhere and are not entries made by hand, in id order,
/// after `after` — a cursor, so the run is one pass that ends, and nothing
/// about the works' own schedule is touched to make it.
pub async fn refresh_candidates(
    db: &Db,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<(String, MediaKind)>> {
    const ELIGIBLE: &str = "SELECT id, kind FROM media_item
         WHERE is_enabled = 1 AND is_manual = 0
           AND EXISTS (SELECT 1 FROM media_external_id e WHERE e.media_id = media_item.id)";

    let rows = match after {
        Some(after) => {
            sqlx::query(db.sql(&format!("{ELIGIBLE} AND id > ? ORDER BY id LIMIT ?")))
                .bind(after)
                .bind(limit.clamp(1, 500))
                .fetch_all(db.pool())
                .await?
        }
        None => {
            sqlx::query(db.sql(&format!("{ELIGIBLE} ORDER BY id LIMIT ?")))
                .bind(limit.clamp(1, 500))
                .fetch_all(db.pool())
                .await?
        }
    };

    rows.iter()
        .map(|row| Ok((row.text("id")?, row.text("kind")?.parse()?)))
        .collect()
}

/// How many works a refresh of everything has before it.
pub async fn count_refresh_candidates(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS n FROM media_item
         WHERE is_enabled = 1 AND is_manual = 0
           AND EXISTS (SELECT 1 FROM media_external_id e WHERE e.media_id = media_item.id)",
    ))
    .fetch_one(db.pool())
    .await?;

    Ok(row.big("n")?)
}

/// Works whose `refresh_after` has passed, oldest first.
///
/// Manual-only entries are excluded: with no external id there is nothing to
/// refresh from.
pub async fn due_for_refresh(db: &Db, limit: i64) -> Result<Vec<(String, MediaKind)>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, kind FROM media_item
         WHERE is_enabled = 1
           AND refresh_after IS NOT NULL
           AND refresh_after <= ?
           AND EXISTS (SELECT 1 FROM media_external_id e WHERE e.media_id = media_item.id)
         ORDER BY refresh_after ASC
         LIMIT ?",
    ))
    .bind(now())
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| Ok((row.text("id")?, row.text("kind")?.parse()?)))
        .collect()
}

/// Works whose copy of TMDB's answer is older than `before` — the six months
/// its terms allow — and whose own refresh is not due, oldest first: what
/// the sweep gives a light refresh, from TMDB alone, after the works that
/// are due. Only those with a TMDB id to be asked by; entries made by hand
/// and works switched off are passed by, as [`due_for_refresh`] passes them
/// by. A work never scheduled is taken too: it has a copy to be too old.
pub async fn due_for_light_refresh(
    db: &Db,
    before: &str,
    limit: i64,
) -> Result<Vec<(String, MediaKind)>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, kind FROM media_item
         WHERE is_enabled = 1 AND is_manual = 0
           AND refreshed_at IS NOT NULL
           AND refreshed_at <= ?
           AND (refresh_after IS NULL OR refresh_after > ?)
           AND EXISTS (SELECT 1 FROM media_external_id e
                       WHERE e.media_id = media_item.id
                         AND e.source IN ('tmdb_tv', 'tmdb_movie'))
         ORDER BY refreshed_at ASC
         LIMIT ?",
    ))
    .bind(before)
    .bind(now())
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| Ok((row.text("id")?, row.text("kind")?.parse()?)))
        .collect()
}

/// A light refresh came back: the copy of TMDB's answer is of now. The
/// work's own schedule stands — `refresh_after` is its interval's — and the
/// sync that wrote it stamped `updated_at` as any write does.
pub async fn mark_lightly_refreshed(db: &Db, id: &str) -> Result<()> {
    sqlx::query(db.sql("UPDATE media_item SET refreshed_at = ? WHERE id = ?"))
        .bind(now())
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(())
}

pub async fn mark_refreshed(
    db: &Db,
    id: &str,
    next_refresh: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    // A refresh that failed changed nothing: `updated_at` stays, so that
    // `/v1/movie/changed` does not name the film to Radarr for nothing.
    let stamp = now();
    if error.is_some() {
        sqlx::query(db.sql(
            "UPDATE media_item
             SET refreshed_at = ?, refresh_after = ?, refresh_error = ?
             WHERE id = ?",
        ))
        .bind(&stamp)
        .bind(next_refresh)
        .bind(error)
        .bind(id)
        .execute(db.pool())
        .await?;
    } else {
        sqlx::query(db.sql(
            "UPDATE media_item
             SET refreshed_at = ?, refresh_after = ?, refresh_error = NULL, updated_at = ?
             WHERE id = ?",
        ))
        .bind(&stamp)
        .bind(next_refresh)
        .bind(&stamp)
        .bind(id)
        .execute(db.pool())
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};

    use super::*;
    use crate::{
        db::{testing::db, to_rfc3339},
        domain::{ExternalIds, MediaItem},
    };

    /// A film with TMDB id `tmdb`, refreshed `days_ago` days ago and due
    /// again in `due_in` days — never scheduled with none — changed by
    /// `adjust`, stored.
    async fn aged(
        db: &Db,
        title: &str,
        tmdb: i64,
        days_ago: i64,
        due_in: Option<i64>,
        adjust: impl FnOnce(&mut MediaItem),
    ) -> MediaItem {
        super::super::testing::stored(db, |item| {
            item.title = title.into();
            item.external_ids = ExternalIds {
                tmdb: Some(tmdb),
                ..Default::default()
            };
            item.refreshed_at = Some(to_rfc3339(Utc::now() - TimeDelta::days(days_ago)));
            item.refresh_after = due_in.map(|days| to_rfc3339(Utc::now() + TimeDelta::days(days)));
            adjust(item);
        })
        .await
    }

    fn ids(listed: &[(String, MediaKind)]) -> Vec<&str> {
        listed.iter().map(|(id, _)| id.as_str()).collect()
    }

    #[tokio::test]
    async fn works_whose_copy_of_tmdb_is_six_months_old_are_listed_oldest_first_and_only_those() {
        let db = db().await;
        let before = crate::service::six_months_before(Utc::now());

        let aged_work = aged(&db, "Aged", 1, 200, Some(30), |_| {}).await;
        let older = aged(&db, "Older", 2, 300, Some(30), |_| {}).await;
        let unscheduled = aged(&db, "Unscheduled", 3, 250, None, |_| {}).await;
        // Not listed: refreshed lately...
        aged(&db, "Fresh", 4, 10, Some(30), |_| {}).await;
        // ...due by its own interval, which the sweep takes first...
        let due = aged(&db, "Due", 5, 200, Some(-1), |_| {}).await;
        // ...entered by hand, switched off, or with no TMDB id to be asked by.
        aged(&db, "By hand", 6, 200, Some(30), |item| {
            item.is_manual = true
        })
        .await;
        aged(&db, "Off", 7, 200, Some(30), |item| item.is_enabled = false).await;
        aged(&db, "Elsewhere", 8, 200, Some(30), |item| {
            item.external_ids = ExternalIds {
                imdb: Some("tt0000008".into()),
                ..Default::default()
            };
        })
        .await;

        let listed = due_for_light_refresh(&db, &before, 10)
            .await
            .expect("listed");
        assert_eq!(
            ids(&listed),
            [
                older.id.as_str(),
                unscheduled.id.as_str(),
                aged_work.id.as_str()
            ]
        );
        let due_now = due_for_refresh(&db, 10).await.expect("listed");
        assert_eq!(ids(&due_now), [due.id.as_str()]);

        // Within the room the sweep has left.
        let one = due_for_light_refresh(&db, &before, 1)
            .await
            .expect("listed");
        assert_eq!(ids(&one), [older.id.as_str()]);

        // Asked again: its copy is of now, and its own schedule stands.
        mark_lightly_refreshed(&db, &older.id)
            .await
            .expect("marked");
        let listed = due_for_light_refresh(&db, &before, 10)
            .await
            .expect("listed");
        assert_eq!(
            ids(&listed),
            [unscheduled.id.as_str(), aged_work.id.as_str()]
        );
        let held = super::super::get(&db, &older.id)
            .await
            .expect("read")
            .expect("held");
        assert_eq!(held.refresh_after, older.refresh_after);
        assert!(
            held.refreshed_at
                .as_deref()
                .is_some_and(|at| at > before.as_str())
        );
    }
}
