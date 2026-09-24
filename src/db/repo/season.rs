//! What a season — a quarter of the calendar — brought to the catalogue.
//!
//! A series is counted by its seasons, the way a season chart lists them: one
//! whose first episode falls in the quarter is new, whether it opens the
//! series or returns it; one that began before and still airs in it carries
//! on. A film is counted by the day it first reached the public, as it is
//! listed (see [`super::item::Listed`]).
//!
//! Episode dates are read as a work's page shows them: an air date somebody
//! corrected is where the season starts or ends, not the date a provider gave.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use sqlx::{Arguments as _, any::AnyArguments};

use crate::{
    db::{Db, RowExt},
    domain::fields::Scope,
};

/// A season's episodes: each one's number, and its day where it has one.
type Numbered = Vec<(i32, Option<String>)>;

/// One season of a series, as it runs across a quarter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub media_id: String,
    pub season_number: i32,
    /// Whether it is the series' first season, which makes a new series of it.
    pub opens_series: bool,
    /// The first and last dated episodes, `YYYY-MM-DD`.
    pub starts: String,
    pub ends: String,
    /// Its episodes, dated or not.
    pub episodes: i64,
    /// Those dated before `today`.
    pub aired: i64,
    /// The next to air, from `today` on: its number and date.
    pub next: Option<(i32, String)>,
}

/// Every season of an enabled series with an episode dated in `[from, to]`,
/// both `YYYY-MM-DD`.
pub async fn runs(
    db: &Db,
    from: &str,
    to: &str,
    today: &str,
    include_adult: bool,
) -> Result<Vec<Run>> {
    let adult = if include_adult {
        ""
    } else {
        " AND m.is_adult = 0"
    };

    // The works first: an episode dated in the quarter as a provider gave it…
    let dated_sql = format!(
        "SELECT DISTINCT e.media_id AS id FROM media_episode e
         JOIN media_item m ON m.id = e.media_id
         WHERE e.season_number > 0 AND e.air_date >= ? AND e.air_date <= ?
           AND m.is_enabled = 1 AND m.kind = 'series'{adult}"
    );
    let mut candidates: Vec<String> = sqlx::query(db.sql(&dated_sql))
        .bind(from)
        .bind(to)
        .fetch_all(db.pool())
        .await?
        .iter()
        .map(|row| row.text("id"))
        .collect::<Result<_, _>>()?;

    // …or as somebody corrected it into the quarter. A correction that moves
    // an episode out, or clears its date, only matters to a work already
    // found above. Read here rather than compared in SQL: the value is JSON,
    // and corrections are few.
    let corrected_sql = format!(
        "SELECT o.media_id, o.value FROM media_override o
         JOIN media_item m ON m.id = o.media_id
         WHERE o.field = 'airDate' AND o.scope LIKE 'episode:%'
           AND m.is_enabled = 1 AND m.kind = 'series'{adult}"
    );
    let mut known: HashSet<String> = candidates.iter().cloned().collect();
    for row in sqlx::query(db.sql(&corrected_sql))
        .fetch_all(db.pool())
        .await?
    {
        let inside = corrected_day(row.opt_text("value")?.as_deref())
            .is_some_and(|d| d.as_str() >= from && d.as_str() <= to);
        let id = row.text("media_id")?;
        if inside && known.insert(id.clone()) {
            candidates.push(id);
        }
    }

    let mut runs = Vec::new();

    for chunk in candidates.chunks(400) {
        let seasons = dated_seasons(db, chunk).await?;

        let firsts: HashMap<&str, i32> =
            seasons
                .keys()
                .fold(HashMap::new(), |mut firsts, (media_id, season)| {
                    let first = firsts.entry(media_id.as_str()).or_insert(*season);
                    *first = (*first).min(*season);
                    firsts
                });

        for ((media_id, season), episodes) in &seasons {
            let mut dated: Vec<(i32, &str)> = episodes
                .iter()
                .filter_map(|(n, d)| d.as_deref().map(|d| (*n, d)))
                .collect();
            if !dated.iter().any(|(_, d)| *d >= from && *d <= to) {
                continue;
            }
            dated.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(&b.0)));

            let (Some(first), Some(last)) = (dated.first(), dated.last()) else {
                continue;
            };

            runs.push(Run {
                media_id: media_id.clone(),
                season_number: *season,
                opens_series: firsts.get(media_id.as_str()) == Some(season),
                starts: first.1.to_string(),
                ends: last.1.to_string(),
                episodes: episodes.len() as i64,
                aired: dated.iter().filter(|(_, d)| *d < today).count() as i64,
                next: dated
                    .iter()
                    .find(|(_, d)| *d >= today)
                    .map(|(n, d)| (*n, d.to_string())),
            });
        }
    }

    runs.sort_by(|a, b| {
        a.starts
            .cmp(&b.starts)
            .then_with(|| a.media_id.cmp(&b.media_id))
            .then(a.season_number.cmp(&b.season_number))
    });
    Ok(runs)
}

/// The regular seasons of some works, each episode with the day it airs as its
/// page shows it: a corrected date in place of the provider's, a cleared one
/// as none.
async fn dated_seasons(db: &Db, ids: &[String]) -> Result<HashMap<(String, i32), Numbered>> {
    let mut seasons: HashMap<(String, i32), Numbered> = HashMap::new();
    if ids.is_empty() {
        return Ok(seasons);
    }
    let holes = vec!["?"; ids.len()].join(", ");

    let mut args = AnyArguments::default();
    for id in ids {
        args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let episodes = sqlx::query_with(
        db.sql(&format!(
            "SELECT media_id, season_number, episode_number, air_date FROM media_episode
             WHERE season_number > 0 AND media_id IN ({holes})"
        )),
        args,
    )
    .fetch_all(db.pool())
    .await?;

    let mut args = AnyArguments::default();
    for id in ids {
        args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let corrected = sqlx::query_with(
        db.sql(&format!(
            "SELECT media_id, scope, value FROM media_override
             WHERE field = 'airDate' AND scope LIKE 'episode:%' AND media_id IN ({holes})"
        )),
        args,
    )
    .fetch_all(db.pool())
    .await?;

    let mut moved: HashMap<(String, i32, i32), Option<String>> = HashMap::new();
    for row in &corrected {
        let Ok(Scope::Episode { season, episode }) = row.text("scope")?.parse::<Scope>() else {
            continue;
        };
        moved.insert(
            (row.text("media_id")?, season, episode),
            corrected_day(row.opt_text("value")?.as_deref()),
        );
    }

    for row in &episodes {
        let media_id = row.text("media_id")?;
        let season = row.int("season_number")?;
        let number = row.int("episode_number")?;
        let date = match moved.get(&(media_id.clone(), season, number)) {
            Some(corrected) => corrected.clone(),
            None => row.opt_text("air_date")?.and_then(|d| day(&d)),
        };
        seasons
            .entry((media_id, season))
            .or_default()
            .push((number, date));
    }
    Ok(seasons)
}

/// The day a stored correction gives, if it gives one: its value is JSON, a
/// date or `null` for one cleared.
fn corrected_day(value: Option<&str>) -> Option<String> {
    value
        .and_then(|v| serde_json::from_str::<Option<String>>(v).ok())
        .flatten()
        .and_then(|d| day(&d))
}

/// Enabled films first released in `[from, to]`, and series announced for it
/// with no dated episode yet: each with the day it is listed under.
pub async fn premieres(
    db: &Db,
    from: &str,
    to: &str,
    include_adult: bool,
) -> Result<Vec<(String, crate::domain::MediaKind, String)>> {
    let adult = if include_adult {
        ""
    } else {
        " AND m.is_adult = 0"
    };

    // `listed_release` is a date, or a year alone; a year is before every day
    // of it as text, so it never falls inside a quarter it cannot be placed in.
    let sql = format!(
        "SELECT m.id, m.kind, m.listed_release FROM media_item m
         WHERE m.is_enabled = 1{adult}
           AND m.listed_release >= ? AND m.listed_release <= ?
         ORDER BY m.listed_release, m.id"
    );

    let rows = sqlx::query(db.sql(&sql))
        .bind(from)
        .bind(to)
        .fetch_all(db.pool())
        .await?;

    let mut found: Vec<(String, crate::domain::MediaKind, String)> = rows
        .iter()
        .map(|row| {
            Ok((
                row.text("id")?,
                row.text("kind")?.parse()?,
                row.text("listed_release")?,
            ))
        })
        .collect::<Result<_>>()?;

    // A series with a dated episode is placed by its seasons, in whichever
    // quarter they air — dated as its page dates them, so one whose dates were
    // all cleared is announced again, and one given a date is not listed twice.
    let series: Vec<String> = found
        .iter()
        .filter(|(_, kind, _)| *kind == crate::domain::MediaKind::Series)
        .map(|(id, _, _)| id.clone())
        .collect();
    let mut dated: HashSet<String> = HashSet::new();
    for chunk in series.chunks(400) {
        for ((media_id, _), episodes) in dated_seasons(db, chunk).await? {
            if episodes.iter().any(|(_, d)| d.is_some()) {
                dated.insert(media_id);
            }
        }
    }
    found.retain(|(id, _, _)| !dated.contains(id));

    Ok(found)
}

/// `YYYY-MM-DD` out of a date, a timestamp, or nothing usable.
fn day(value: &str) -> Option<String> {
    let day = value.trim().get(..10)?;
    let bytes = day.as_bytes();
    let shaped = bytes.iter().enumerate().all(|(i, b)| match i {
        4 | 7 => *b == b'-',
        _ => b.is_ascii_digit(),
    });
    shaped.then(|| day.to_string())
}

/// The works a set of runs and premieres names, once each.
pub fn works(
    runs: &[Run],
    premieres: &[(String, crate::domain::MediaKind, String)],
) -> Vec<String> {
    let mut seen = HashSet::new();
    runs.iter()
        .map(|r| r.media_id.clone())
        .chain(premieres.iter().map(|(id, _, _)| id.clone()))
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        db::repo::{self, child::blank_episode},
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

    async fn stored(db: &Db, adjust: impl FnOnce(&mut MediaItem)) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.id = crate::db::new_id();
        item.title = "Untitled".into();
        item.created_at = crate::db::now();
        item.updated_at = crate::db::now();
        adjust(&mut item);
        item.slug = crate::domain::make_slug(&item.title, item.year);

        repo::item::upsert(
            db,
            repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        item
    }

    fn episode(season: i32, number: i32, date: &str) -> crate::domain::Episode {
        let mut e = blank_episode(season, number);
        e.is_manual = false;
        e.air_date = Some(date.into());
        e
    }

    const AUTUMN: (&str, &str) = ("2026-10-01", "2026-12-31");

    #[tokio::test]
    async fn a_season_is_new_when_it_starts_in_the_quarter_and_carries_on_when_it_began_before() {
        let db = db().await;
        let fresh = stored(&db, |i| {
            i.title = "Fresh".into();
            i.episodes = vec![episode(1, 1, "2026-10-05"), episode(1, 2, "2026-10-12")];
        })
        .await;
        let returning = stored(&db, |i| {
            i.title = "Returning".into();
            i.episodes = vec![
                episode(1, 1, "2024-01-01"),
                episode(2, 1, "2026-11-02"),
                episode(2, 2, "2026-11-09"),
            ];
        })
        .await;
        let running = stored(&db, |i| {
            i.title = "Running".into();
            i.episodes = vec![episode(1, 1, "2026-09-01"), episode(1, 2, "2026-10-08")];
        })
        .await;
        stored(&db, |i| {
            i.title = "Elsewhere".into();
            i.episodes = vec![episode(1, 1, "2026-06-01")];
        })
        .await;

        let runs = runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-10", false)
            .await
            .unwrap();
        let by = |id: &str| runs.iter().find(|r| r.media_id == id).cloned().unwrap();

        assert_eq!(
            runs.len(),
            3,
            "the series airing in summer only is left out"
        );

        let fresh = by(&fresh.id);
        assert!(fresh.opens_series);
        assert_eq!((fresh.starts.as_str(), fresh.aired), ("2026-10-05", 1));
        assert_eq!(fresh.next, Some((2, "2026-10-12".into())));

        let returning = by(&returning.id);
        assert_eq!(returning.season_number, 2);
        assert!(
            !returning.opens_series,
            "a second season returns the series"
        );

        let running = by(&running.id);
        assert!(running.starts.as_str() < AUTUMN.0, "it began in the summer");
    }

    #[tokio::test]
    async fn a_corrected_air_date_is_where_the_season_starts() {
        let db = db().await;
        let moved = stored(&db, |i| {
            i.title = "Brought forward".into();
            i.episodes = vec![episode(1, 1, "2027-01-15")];
        })
        .await;

        assert!(
            runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-01", false)
                .await
                .unwrap()
                .is_empty()
        );

        repo::override_field::set(
            &db,
            &moved.id,
            Scope::Episode {
                season: 1,
                episode: 1,
            },
            "airDate",
            Some(&serde_json::json!("2026-12-18")),
            None,
        )
        .await
        .unwrap();

        let runs = runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-01", false)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].starts, "2026-12-18");
    }

    #[tokio::test]
    async fn films_are_placed_by_their_release_and_announced_series_by_their_first_date() {
        let db = db().await;
        let film = stored(&db, |i| {
            i.kind = MediaKind::Movie;
            i.title = "Film".into();
            i.in_cinemas = Some("2026-11-20".into());
        })
        .await;
        stored(&db, |i| {
            i.kind = MediaKind::Movie;
            i.title = "Only a year".into();
            i.year = Some(2026);
        })
        .await;
        let announced = stored(&db, |i| {
            i.title = "Announced".into();
            i.first_aired = Some("2026-10-30".into());
        })
        .await;
        // Dated episodes place a series by its runs, never here as well.
        stored(&db, |i| {
            i.title = "Has episodes".into();
            i.first_aired = Some("2026-10-30".into());
            i.episodes = vec![episode(1, 1, "2026-10-30")];
        })
        .await;

        let found = premieres(&db, AUTUMN.0, AUTUMN.1, false).await.unwrap();
        let ids: Vec<&str> = found.iter().map(|(id, _, _)| id.as_str()).collect();

        assert_eq!(ids, [announced.id.as_str(), film.id.as_str()]);
        assert_eq!(found[1].2, "2026-11-20");
    }

    #[tokio::test]
    async fn an_episode_dated_today_is_the_next_one_not_one_aired() {
        let db = db().await;
        stored(&db, |i| {
            i.episodes = vec![
                episode(1, 1, "2026-10-05"),
                episode(1, 2, "2026-10-12"),
                episode(1, 3, "2026-10-19"),
            ];
        })
        .await;

        let runs = runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-12", false)
            .await
            .unwrap();
        assert_eq!(runs[0].aired, 1);
        assert_eq!(runs[0].next, Some((2, "2026-10-12".into())));
    }

    #[tokio::test]
    async fn a_date_corrected_out_of_the_quarter_or_cleared_takes_the_season_with_it() {
        let db = db().await;
        let postponed = stored(&db, |i| {
            i.title = "Postponed".into();
            i.episodes = vec![episode(1, 1, "2026-11-01")];
        })
        .await;
        let unknown = stored(&db, |i| {
            i.title = "Date unknown after all".into();
            i.episodes = vec![episode(1, 1, "2026-11-01")];
        })
        .await;
        let episode_one = Scope::Episode {
            season: 1,
            episode: 1,
        };
        repo::override_field::set(
            &db,
            &postponed.id,
            episode_one,
            "airDate",
            Some(&serde_json::json!("2027-02-01")),
            None,
        )
        .await
        .unwrap();
        repo::override_field::set(
            &db,
            &unknown.id,
            episode_one,
            "airDate",
            Some(&serde_json::Value::Null),
            None,
        )
        .await
        .unwrap();

        assert!(
            runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-01", false)
                .await
                .unwrap()
                .is_empty()
        );
        let winter = runs(&db, "2027-01-01", "2027-03-31", "2026-10-01", false)
            .await
            .unwrap();
        assert_eq!(winter.len(), 1);
        assert_eq!(winter[0].media_id, postponed.id);
    }

    #[tokio::test]
    async fn a_season_airing_either_side_of_the_quarter_but_not_in_it_is_not_in_it() {
        let db = db().await;
        stored(&db, |i| {
            i.episodes = vec![episode(1, 1, "2026-09-20"), episode(1, 2, "2027-01-10")];
        })
        .await;

        assert!(
            runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-01", false)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn adult_and_disabled_works_are_left_out_unless_adult_ones_are_asked_for() {
        let db = db().await;
        let adult = stored(&db, |i| {
            i.is_adult = true;
            i.episodes = vec![episode(1, 1, "2026-10-05")];
        })
        .await;
        stored(&db, |i| {
            i.is_enabled = false;
            i.episodes = vec![episode(1, 1, "2026-10-05")];
        })
        .await;
        stored(&db, |i| {
            i.kind = MediaKind::Movie;
            i.is_enabled = false;
            i.in_cinemas = Some("2026-11-20".into());
        })
        .await;

        assert!(
            runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-01", false)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            premieres(&db, AUTUMN.0, AUTUMN.1, true)
                .await
                .unwrap()
                .is_empty()
        );

        let with_adult = runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-01", true)
            .await
            .unwrap();
        assert_eq!(with_adult.len(), 1);
        assert_eq!(with_adult[0].media_id, adult.id);
    }

    #[tokio::test]
    async fn an_announced_series_is_announced_until_it_has_a_date_and_again_once_it_has_none() {
        let db = db().await;
        let dated = stored(&db, |i| {
            i.title = "Given a date".into();
            i.first_aired = Some("2026-10-30".into());
            // Announced with its episodes, none of them dated yet.
            let mut undated = blank_episode(1, 1);
            undated.is_manual = false;
            i.episodes = vec![undated];
        })
        .await;
        let cleared = stored(&db, |i| {
            i.title = "Dates cleared".into();
            i.first_aired = Some("2026-10-30".into());
            i.episodes = vec![episode(1, 1, "2026-10-30")];
        })
        .await;

        let episode_one = Scope::Episode {
            season: 1,
            episode: 1,
        };
        repo::override_field::set(
            &db,
            &dated.id,
            episode_one,
            "airDate",
            Some(&serde_json::json!("2026-10-30")),
            None,
        )
        .await
        .unwrap();
        repo::override_field::set(
            &db,
            &cleared.id,
            episode_one,
            "airDate",
            Some(&serde_json::Value::Null),
            None,
        )
        .await
        .unwrap();

        let announced = premieres(&db, AUTUMN.0, AUTUMN.1, false).await.unwrap();
        let ids: Vec<&str> = announced.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, [cleared.id.as_str()]);

        let running = runs(&db, AUTUMN.0, AUTUMN.1, "2026-10-01", false)
            .await
            .unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].media_id, dated.id);
    }

    #[test]
    fn a_day_is_read_out_of_a_date_or_a_timestamp_and_nothing_else() {
        assert_eq!(day("2026-10-05").as_deref(), Some("2026-10-05"));
        assert_eq!(day("2026-10-05T21:00:00Z").as_deref(), Some("2026-10-05"));
        assert_eq!(day("2026"), None);
        assert_eq!(day("TBA"), None);
        assert_eq!(day("05/10/2026"), None);
    }
}
