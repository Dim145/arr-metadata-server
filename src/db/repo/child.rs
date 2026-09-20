//! Children added by hand.
//!
//! The refresh path in [`super::item`] deliberately never writes a row flagged
//! manual: credits, images and alternative titles have no natural key, so
//! re-inserting one read back from the database would duplicate it on every
//! refresh. This module is the other half — the only way a manual child is
//! created, and the only way one is removed.
//!
//! Editing a child's *fields* is a different operation and goes through
//! `media_override`, which works the same whether the row came from a provider
//! or from a person.

use anyhow::Result;

use crate::{
    db::{Db, RowExt, new_id, now},
    domain::{AlternativeTitle, CoverType, Credit, CreditType, Episode, Image, Season},
};

/// Whether a row belongs to a person rather than a provider.
///
/// Only manual rows may be deleted: removing a provider row would simply bring
/// it back on the next refresh, which looks like the delete silently failed.
async fn is_manual(db: &Db, table: &str, media_id: &str, id: &str) -> Result<bool> {
    let sql = format!("SELECT is_manual FROM {table} WHERE id = ? AND media_id = ?");

    let row = sqlx::query(db.sql(&sql))
        .bind(id)
        .bind(media_id)
        .fetch_optional(db.pool())
        .await?;

    match row {
        Some(row) => Ok(row.flag("is_manual")?),
        None => Ok(false),
    }
}

// ─── seasons ─────────────────────────────────────────────────────────────────

pub async fn add_season(db: &Db, media_id: &str, season: &Season) -> Result<String> {
    let id = new_id();
    let at = now();

    sqlx::query(db.sql(
        "INSERT INTO media_season
             (id, media_id, season_number, title, overview, air_date, tmdb_id,
              tvdb_id, is_manual, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
    ))
    .bind(&id)
    .bind(media_id)
    .bind(season.season_number)
    .bind(&season.title)
    .bind(&season.overview)
    .bind(&season.air_date)
    .bind(season.tmdb_id)
    .bind(season.tvdb_id)
    .bind(&at)
    .bind(&at)
    .execute(db.pool())
    .await?;

    Ok(id)
}

/// Remove a manual season. Returns false when it is not there or not manual.
pub async fn remove_season(db: &Db, media_id: &str, season_number: i32) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "DELETE FROM media_season
         WHERE media_id = ? AND season_number = ? AND is_manual = 1",
    ))
    .bind(media_id)
    .bind(season_number)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

// ─── episodes ────────────────────────────────────────────────────────────────

pub async fn add_episode(db: &Db, media_id: &str, episode: &Episode) -> Result<String> {
    let id = new_id();
    let at = now();

    sqlx::query(db.sql(
        "INSERT INTO media_episode
             (id, media_id, season_number, episode_number, absolute_episode_number,
              aired_after_season_number, aired_before_season_number,
              aired_before_episode_number, title, overview, air_date, air_date_utc,
              runtime, finale_type, image, tvdb_id, tmdb_id, rating_value,
              rating_count, is_manual, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
    ))
    .bind(&id)
    .bind(media_id)
    .bind(episode.season_number)
    .bind(episode.episode_number)
    .bind(episode.absolute_episode_number)
    .bind(episode.aired_after_season_number)
    .bind(episode.aired_before_season_number)
    .bind(episode.aired_before_episode_number)
    .bind(&episode.title)
    .bind(&episode.overview)
    .bind(&episode.air_date)
    .bind(&episode.air_date_utc)
    .bind(episode.runtime)
    .bind(&episode.finale_type)
    .bind(&episode.image)
    .bind(episode.tvdb_id)
    .bind(episode.tmdb_id)
    .bind(episode.rating.map(|r| r.value))
    .bind(episode.rating.map(|r| r.votes))
    .bind(&at)
    .bind(&at)
    .execute(db.pool())
    .await?;

    Ok(id)
}

pub async fn remove_episode(db: &Db, media_id: &str, season: i32, episode: i32) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "DELETE FROM media_episode
         WHERE media_id = ? AND season_number = ? AND episode_number = ? AND is_manual = 1",
    ))
    .bind(media_id)
    .bind(season)
    .bind(episode)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

// ─── images ──────────────────────────────────────────────────────────────────

pub async fn add_image(db: &Db, media_id: &str, image: &Image) -> Result<String> {
    let id = new_id();

    sqlx::query(db.sql(
        "INSERT INTO media_image
             (id, media_id, season_number, cover_type, url, language, sort_order,
              source, is_manual, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
    ))
    .bind(&id)
    .bind(media_id)
    .bind(image.season_number)
    .bind(image.cover_type.as_str())
    .bind(&image.url)
    .bind(&image.language)
    .bind(image.sort_order)
    .bind(image.source.as_deref().unwrap_or("manual"))
    .bind(now())
    .execute(db.pool())
    .await?;

    Ok(id)
}

pub async fn remove_image(db: &Db, media_id: &str, image_id: &str) -> Result<bool> {
    if !is_manual(db, "media_image", media_id, image_id).await? {
        return Ok(false);
    }

    let result = sqlx::query(db.sql("DELETE FROM media_image WHERE id = ? AND media_id = ?"))
        .bind(image_id)
        .bind(media_id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

// ─── credits ─────────────────────────────────────────────────────────────────

pub async fn add_credit(db: &Db, media_id: &str, credit: &Credit) -> Result<String> {
    let id = new_id();

    sqlx::query(db.sql(
        "INSERT INTO media_credit
             (id, media_id, credit_type, person_name, character_name, image,
              tmdb_person_id, credit_tmdb_id, sort_order, is_manual, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
    ))
    .bind(&id)
    .bind(media_id)
    .bind(credit.credit_type.as_str())
    .bind(&credit.person_name)
    .bind(&credit.character_name)
    .bind(&credit.image)
    .bind(credit.tmdb_person_id)
    .bind(&credit.credit_tmdb_id)
    .bind(credit.sort_order)
    .bind(now())
    .execute(db.pool())
    .await?;

    Ok(id)
}

pub async fn remove_credit(db: &Db, media_id: &str, credit_id: &str) -> Result<bool> {
    if !is_manual(db, "media_credit", media_id, credit_id).await? {
        return Ok(false);
    }

    let result = sqlx::query(db.sql("DELETE FROM media_credit WHERE id = ? AND media_id = ?"))
        .bind(credit_id)
        .bind(media_id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

// ─── alternative titles ──────────────────────────────────────────────────────

pub async fn add_alternative_title(
    db: &Db,
    media_id: &str,
    title: &AlternativeTitle,
) -> Result<String> {
    let id = new_id();

    sqlx::query(db.sql(
        "INSERT INTO media_alternative_title
             (id, media_id, title, title_type, language, is_manual, created_at)
         VALUES (?, ?, ?, ?, ?, 1, ?)",
    ))
    .bind(&id)
    .bind(media_id)
    .bind(&title.title)
    .bind(&title.title_type)
    .bind(&title.language)
    .bind(now())
    .execute(db.pool())
    .await?;

    Ok(id)
}

pub async fn remove_alternative_title(db: &Db, media_id: &str, title_id: &str) -> Result<bool> {
    if !is_manual(db, "media_alternative_title", media_id, title_id).await? {
        return Ok(false);
    }

    let result =
        sqlx::query(db.sql("DELETE FROM media_alternative_title WHERE id = ? AND media_id = ?"))
            .bind(title_id)
            .bind(media_id)
            .execute(db.pool())
            .await?;

    Ok(result.rows_affected() > 0)
}

// ─── shared helpers for the API layer ────────────────────────────────────────

/// A blank manual season, ready for a caller to fill in.
pub fn blank_season(season_number: i32) -> Season {
    Season {
        id: String::new(),
        season_number,
        title: None,
        overview: None,
        air_date: None,
        tmdb_id: None,
        tvdb_id: None,
        is_manual: true,
        images: Vec::new(),
    }
}

pub fn blank_episode(season_number: i32, episode_number: i32) -> Episode {
    Episode {
        id: String::new(),
        season_number,
        episode_number,
        absolute_episode_number: None,
        aired_after_season_number: None,
        aired_before_season_number: None,
        aired_before_episode_number: None,
        title: String::new(),
        overview: None,
        air_date: None,
        air_date_utc: None,
        runtime: None,
        finale_type: None,
        image: None,
        tvdb_id: None,
        tmdb_id: None,
        rating: None,
        is_manual: true,
    }
}

pub fn blank_image(cover_type: CoverType, url: String) -> Image {
    Image {
        id: String::new(),
        season_number: None,
        cover_type,
        url,
        language: None,
        sort_order: 0,
        source: Some("manual".to_string()),
        is_manual: true,
    }
}

pub fn blank_credit(credit_type: CreditType, person_name: String) -> Credit {
    Credit {
        id: String::new(),
        credit_type,
        person_name,
        character_name: None,
        image: None,
        tmdb_person_id: None,
        credit_tmdb_id: None,
        sort_order: 0,
        is_manual: true,
    }
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
        .unwrap();
        db.migrate().await.unwrap();
        db
    }

    /// A series holding one provider season and one provider episode.
    async fn series(db: &Db) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Host".into();
        item.slug = "host-2026".into();
        item.seasons = vec![Season {
            id: String::new(),
            season_number: 1,
            title: None,
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: Vec::new(),
        }];
        item.episodes = vec![blank_episode(1, 1)];
        item.episodes[0].is_manual = false;
        item.episodes[0].title = "From The Provider".into();
        item.credits = vec![{
            let mut c = blank_credit(CreditType::Actor, "Provider Person".into());
            c.is_manual = false;
            c.credit_tmdb_id = Some("abc".into());
            c
        }];

        item::upsert(
            db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();
        item
    }

    async fn reload(db: &Db, id: &str) -> MediaItem {
        let mut item = item::get(db, id).await.unwrap().unwrap();
        item::load_children(db, &mut item).await.unwrap();
        item
    }

    #[tokio::test]
    async fn a_manual_season_survives_a_refresh() {
        let db = db().await;
        let item = series(&db).await;

        let mut season = blank_season(2);
        season.title = Some("Added By Hand".into());
        add_season(&db, &item.id, &season).await.unwrap();

        // A refresh carries only the provider's children.
        item::upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        let read = reload(&db, &item.id).await;
        let numbers: Vec<i32> = read.seasons.iter().map(|s| s.season_number).collect();

        assert_eq!(numbers, vec![1, 2]);
        assert!(read.seasons[1].is_manual);
        assert_eq!(read.seasons[1].title.as_deref(), Some("Added By Hand"));
    }

    #[tokio::test]
    async fn a_manual_episode_survives_a_refresh() {
        let db = db().await;
        let item = series(&db).await;

        let mut episode = blank_episode(1, 2);
        episode.title = "Added By Hand".into();
        add_episode(&db, &item.id, &episode).await.unwrap();

        item::upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        let read = reload(&db, &item.id).await;
        assert_eq!(read.episodes.len(), 2);
        assert_eq!(read.episodes[0].title, "From The Provider");
        assert_eq!(read.episodes[1].title, "Added By Hand");
        assert!(read.episodes[1].is_manual);
    }

    #[tokio::test]
    async fn manual_images_and_credits_survive_a_refresh() {
        let db = db().await;
        let item = series(&db).await;

        add_image(
            &db,
            &item.id,
            &blank_image(CoverType::Poster, "https://example.invalid/mine.jpg".into()),
        )
        .await
        .unwrap();
        add_credit(
            &db,
            &item.id,
            &blank_credit(CreditType::Actor, "Added By Hand".into()),
        )
        .await
        .unwrap();

        item::upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        let read = reload(&db, &item.id).await;

        assert_eq!(read.images.len(), 1);
        assert!(read.images[0].is_manual);

        let names: Vec<&str> = read
            .credits
            .iter()
            .map(|c| c.person_name.as_str())
            .collect();
        assert!(names.contains(&"Provider Person"));
        assert!(names.contains(&"Added By Hand"));
        assert_eq!(read.credits.len(), 2, "no duplicates");
    }

    #[tokio::test]
    async fn a_provider_row_cannot_be_removed() {
        // It would come back on the next refresh, which reads as the delete
        // having silently failed.
        let db = db().await;
        let item = series(&db).await;
        let read = reload(&db, &item.id).await;

        assert!(!remove_season(&db, &item.id, 1).await.unwrap());
        assert!(!remove_episode(&db, &item.id, 1, 1).await.unwrap());
        assert!(
            !remove_credit(&db, &item.id, &read.credits[0].id)
                .await
                .unwrap()
        );

        let after = reload(&db, &item.id).await;
        assert_eq!(after.seasons.len(), 1);
        assert_eq!(after.episodes.len(), 1);
        assert_eq!(after.credits.len(), 1);
    }

    #[tokio::test]
    async fn a_manual_row_can_be_removed() {
        let db = db().await;
        let item = series(&db).await;

        add_season(&db, &item.id, &blank_season(2)).await.unwrap();
        add_episode(&db, &item.id, &blank_episode(1, 2))
            .await
            .unwrap();
        let image = add_image(
            &db,
            &item.id,
            &blank_image(CoverType::Poster, "https://example.invalid/m.jpg".into()),
        )
        .await
        .unwrap();
        let credit = add_credit(
            &db,
            &item.id,
            &blank_credit(CreditType::Actor, "Gone Soon".into()),
        )
        .await
        .unwrap();

        assert!(remove_season(&db, &item.id, 2).await.unwrap());
        assert!(remove_episode(&db, &item.id, 1, 2).await.unwrap());
        assert!(remove_image(&db, &item.id, &image).await.unwrap());
        assert!(remove_credit(&db, &item.id, &credit).await.unwrap());

        let read = reload(&db, &item.id).await;
        assert_eq!(read.seasons.len(), 1);
        assert_eq!(read.episodes.len(), 1);
        assert_eq!(read.images.len(), 0);
        assert_eq!(read.credits.len(), 1);
    }

    #[tokio::test]
    async fn a_row_belonging_to_another_work_is_not_removable() {
        let db = db().await;
        let a = series(&db).await;

        let mut other = MediaItem::empty(MediaKind::Series);
        other.title = "Other".into();
        other.slug = "other-2026".into();
        item::upsert(
            &db,
            ItemWrite {
                item: &other,
                replace_children: false,
            },
        )
        .await
        .unwrap();

        let credit = add_credit(
            &db,
            &a.id,
            &blank_credit(CreditType::Actor, "Belongs To A".into()),
        )
        .await
        .unwrap();

        assert!(!remove_credit(&db, &other.id, &credit).await.unwrap());
        assert_eq!(reload(&db, &a.id).await.credits.len(), 2);
    }
}
