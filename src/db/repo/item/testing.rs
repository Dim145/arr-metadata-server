//! What the tests of every part store their works with.
//!
//! A real database with the real migrations applied: SQLite in memory, or
//! the PostgreSQL `AMS_TEST_DATABASE_URL` names.
//!
//! Every part's queries are built by hand and bound positionally, so a column
//! added to the list without a matching placeholder compiles cleanly and fails
//! at runtime — which is how `34 values for 35 columns` reached a running
//! server. A round trip through an actual engine is what catches that.

use super::{ItemWrite, upsert};
use crate::{
    db::Db,
    domain::{
        CoverType, Credit, CreditType, Episode, ExternalIds, Image, MediaItem, MediaKind, Rating,
        RatingValue,
    },
};

pub(super) fn sample() -> MediaItem {
    let mut item = MediaItem::empty(MediaKind::Movie);
    item.title = "Round Trip".into();
    item.slug = "round-trip-2026".into();
    item.year = Some(2026);
    item.overview = Some("Everything set, so everything is checked.".into());
    item.status = Some("released".into());
    item.runtime = Some(97);
    item.content_rating = Some("PG-13".into());
    item.content_rating_country = Some("US".into());
    item.in_cinemas = Some("2026-01-05".into());
    item.genres = vec!["Drama".into(), "Mystery".into()];
    item.keywords = vec!["test".into()];
    item.popularity = Some(12.5);
    item.theme_music = Some("https://example.invalid/theme.mp3".into());
    item.external_ids = ExternalIds {
        tmdb: Some(4242),
        imdb: Some("tt4242424".into()),
        ..Default::default()
    };
    item.credits = vec![Credit {
        id: String::new(),
        credit_type: CreditType::Actor,
        person_name: "A Person".into(),
        character_name: Some("A Role".into()),
        image: None,
        tmdb_person_id: Some(7),
        credit_tmdb_id: Some("52fe4726c3a36847f812048b".into()),
        sort_order: 0,
        is_manual: false,
    }];
    item.images = vec![Image {
        id: String::new(),
        season_number: None,
        cover_type: CoverType::Poster,
        url: "https://example.invalid/p.jpg".into(),
        language: None,
        sort_order: 0,
        source: Some("tmdb".into()),
        is_manual: false,
    }];
    item.episodes = vec![Episode {
        id: String::new(),
        season_number: 1,
        episode_number: 1,
        absolute_episode_number: None,
        aired_after_season_number: None,
        aired_before_season_number: None,
        aired_before_episode_number: None,
        title: "One".into(),
        overview: None,
        air_date: Some("2026-01-05".into()),
        air_date_utc: Some("2026-01-05T00:00:00Z".into()),
        runtime: Some(42),
        finale_type: None,
        image: None,
        tvdb_id: None,
        tmdb_id: Some(99),
        rating: Some(RatingValue {
            value: 8.0,
            votes: 10,
        }),
        is_manual: false,
    }];
    item.ratings = vec![Rating {
        source: "tmdb".into(),
        value: Some(7.5),
        votes: Some(100),
        rating_type: Some("user".into()),
    }];
    item
}

/// A work built from [`sample`], changed by `adjust`, stored.
pub(super) async fn stored(db: &Db, adjust: impl FnOnce(&mut MediaItem)) -> MediaItem {
    let mut item = sample();
    item.id = crate::db::new_id();
    // Each its own identity: the sample's ids would make every work the same.
    item.external_ids = ExternalIds::default();
    item.credits.clear();
    item.episodes.clear();
    item.ratings.clear();
    adjust(&mut item);
    item.slug = crate::domain::make_slug(&item.title, item.year);

    upsert(
        db,
        ItemWrite {
            item: &item,
            replace_children: true,
        },
    )
    .await
    .expect("stored");
    item
}
