//! Reads across works: many at once by id, what the works on view can be
//! narrowed by next, and the calendar's window. A person's credits are
//! `repo::person`'s.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Arguments, any::AnyArguments};
use utoipa::ToSchema;

use super::{
    read::{EPISODE_COLUMNS, ITEM_COLUMNS, map_episode, map_item},
    search::{Query, attach_external_ids, from_clause, narrow},
};
use crate::{
    db::{Db, RowExt},
    domain::{Episode, MediaItem},
};

/// Works by id, with their identifiers and nothing hanging off them — the
/// same shape [`search`](super::search::search) returns, in no particular order.
pub async fn by_ids(db: &Db, ids: &[String]) -> Result<Vec<MediaItem>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM media_item WHERE id IN ({})",
        vec!["?"; ids.len()].join(", ")
    );

    let mut args = AnyArguments::default();
    for id in ids {
        args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let mut items = rows.iter().map(map_item).collect::<Result<Vec<_>>>()?;
    attach_external_ids(db, &mut items).await?;
    Ok(items)
}

/// One value a list can be narrowed to, and how many works it would leave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Facet {
    pub value: String,
    pub count: i64,
    /// A genre's slug, the same in every language: what its own page is
    /// addressed by, `/genre/drama`. Only on the genres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

/// What the works on view can be narrowed by.
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Facets {
    pub total: i64,
    /// Most common first.
    pub genres: Vec<Facet>,
    /// Networks and studios together, most common first; the long tail left out.
    pub networks: Vec<Facet>,
    /// The languages works were made in, as stored.
    pub languages: Vec<Facet>,
    pub statuses: Vec<Facet>,
    #[schema(required)]
    pub year_min: Option<i32>,
    #[schema(required)]
    pub year_max: Option<i32>,
}

/// How many networks a facet list offers: enough to find any the catalogue
/// holds more than a handful of, not so many the list stops being a choice.
const TOP_NETWORKS: usize = 40;

/// What the list can be narrowed by next, and how many works each choice
/// would leave.
///
/// Counted under the filters already applied, so that a count is what
/// choosing its value would show. Genres are all required at once, so each is
/// counted among the works listed now; a status, a language or a network
/// replaces the one chosen, so each of those is counted as if its own filter
/// were off, and the years span what the list would hold without its range.
/// One read over the matching rows, and one more for each of those filters
/// that is on, counted here: genres live in a JSON column neither engine can
/// count inside the same way, and a catalogue is a few thousand short rows.
pub async fn facets(db: &Db, q: &Query) -> Result<Facets> {
    use std::collections::HashMap;

    let all = facet_rows(db, q).await?;
    let statuses_over = rows_without(
        db,
        q.status.is_some(),
        Query {
            status: None,
            ..q.clone()
        },
    )
    .await?;
    let languages_over = rows_without(
        db,
        q.original_language.is_some(),
        Query {
            original_language: None,
            ..q.clone()
        },
    )
    .await?;
    let networks_over = rows_without(
        db,
        q.network.is_some(),
        Query {
            network: None,
            ..q.clone()
        },
    )
    .await?;
    let years_over = rows_without(
        db,
        q.year.is_some() || q.year_from.is_some() || q.year_to.is_some(),
        Query {
            year: None,
            year_from: None,
            year_to: None,
            ..q.clone()
        },
    )
    .await?;

    let mut facets = Facets {
        total: all.len() as i64,
        ..Default::default()
    };

    // By genre, whichever of its names or its slug a work is listed under —
    // one listed before genres had slugs still holds names — and each once a
    // work, so that "Drama" and "Drame" are one genre counted once.
    let mut genres: HashMap<String, i64> = HashMap::new();
    let mut held: HashMap<String, String> = HashMap::new();
    for row in &all {
        for (slug, name) in crate::db::repo::genre::of_names(&row.text_list("genres")?) {
            if name != slug {
                held.entry(slug.clone()).or_insert(name);
            }
            *genres.entry(slug).or_default() += 1;
        }
    }

    let mut networks: HashMap<String, i64> = HashMap::new();
    for row in networks_over.as_deref().unwrap_or(&all) {
        let network = row.opt_text("network")?.filter(|n| !n.trim().is_empty());
        let studio = row.opt_text("studio")?.filter(|s| !s.trim().is_empty());
        if let Some(network) = &network {
            *networks.entry(network.clone()).or_default() += 1;
        }
        if let Some(studio) = studio.filter(|s| Some(s) != network.as_ref()) {
            *networks.entry(studio).or_default() += 1;
        }
    }

    let mut languages: HashMap<String, i64> = HashMap::new();
    for row in languages_over.as_deref().unwrap_or(&all) {
        if let Some(language) = row.opt_text("original_language")?.filter(|l| !l.is_empty()) {
            *languages.entry(language).or_default() += 1;
        }
    }

    let mut statuses: HashMap<String, i64> = HashMap::new();
    for row in statuses_over.as_deref().unwrap_or(&all) {
        if let Some(status) = row.opt_text("status")?.filter(|s| !s.is_empty()) {
            *statuses.entry(status).or_default() += 1;
        }
    }

    for row in years_over.as_deref().unwrap_or(&all) {
        if let Some(year) = row.opt_int("year")?.filter(|y| *y > 0) {
            facets.year_min = Some(facets.year_min.map_or(year, |m| m.min(year)));
            facets.year_max = Some(facets.year_max.map_or(year, |m| m.max(year)));
        }
    }

    let ranked = |counts: HashMap<String, i64>| {
        let mut list: Vec<Facet> = counts
            .into_iter()
            .map(|(value, count)| Facet {
                value,
                count,
                slug: None,
            })
            .collect();
        list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        list
    };

    // Each genre by the name a filter asks for it by and the interface
    // translates — its English one where the server knows it, else the one a
    // source gave it by — with its slug beside it.
    let slugs: Vec<String> = genres.keys().cloned().collect();
    let named = crate::db::repo::genre::many(db, &slugs).await?;
    let mut by_genre: Vec<Facet> = genres
        .into_iter()
        .map(|(slug, count)| Facet {
            value: named
                .get(&slug)
                .map(|genre| genre.listed_name())
                .or_else(|| held.get(&slug).cloned())
                .unwrap_or_else(|| slug.clone()),
            count,
            slug: Some(slug),
        })
        .collect();
    by_genre.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));

    facets.genres = by_genre;
    facets.networks = ranked(networks);
    facets.networks.truncate(TOP_NETWORKS);
    facets.languages = ranked(languages);
    facets.statuses = ranked(statuses);

    Ok(facets)
}

/// The columns [`facets`] counts, for the works a query lists.
async fn facet_rows(db: &Db, q: &Query) -> Result<Vec<sqlx::any::AnyRow>> {
    let mut sql = format!(
        "SELECT listed_genres AS genres, listed_network AS network, listed_studio AS studio,
                listed_language AS original_language, listed_status AS status, listed_year AS year
         FROM {} WHERE 1 = 1",
        from_clause(db, q)
    );
    let mut args = AnyArguments::default();
    narrow(db.dialect(), q, &mut sql, &mut args)?;

    Ok(sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?)
}

/// The rows one filter's own values are counted over: read again without that
/// filter when it is on, and otherwise the list's own, which are the same.
async fn rows_without(db: &Db, on: bool, without: Query) -> Result<Option<Vec<sqlx::any::AnyRow>>> {
    if on {
        Ok(Some(facet_rows(db, &without).await?))
    } else {
        Ok(None)
    }
}

/// The names works are listed under as a network or a studio, and how many
/// works each names, among the works switched on — for adults only where
/// `include_adult`. Networks have no record of their own: a network's page,
/// addressed by the slug of its name, finds it among these.
pub async fn network_names(db: &Db, include_adult: bool) -> Result<Vec<(String, i64)>> {
    let adult = if include_adult {
        ""
    } else {
        " AND is_adult = 0"
    };
    let sql = format!(
        "SELECT listed_network AS name, COUNT(*) AS n FROM media_item
          WHERE is_enabled = 1 AND merged_into IS NULL AND listed_network IS NOT NULL{adult}
          GROUP BY listed_network
         UNION ALL
         SELECT listed_studio AS name, COUNT(*) AS n FROM media_item
          WHERE is_enabled = 1 AND merged_into IS NULL AND listed_studio IS NOT NULL
            AND (listed_network IS NULL OR listed_network <> listed_studio){adult}
          GROUP BY listed_studio"
    );
    let rows = sqlx::query(db.sql(&sql)).fetch_all(db.pool()).await?;

    let mut counted: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for row in &rows {
        *counted.entry(row.text("name")?).or_default() += row.big("n")?;
    }
    Ok(counted.into_iter().collect())
}

/// A mark that moves whenever the catalogue does: how many works it holds and
/// when one last changed. For answers read across works and kept a while.
pub async fn catalogue_stamp(db: &Db) -> Result<String> {
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS n, MAX(updated_at) AS at, MAX(listed_at) AS listed FROM media_item",
    ))
    .fetch_one(db.pool())
    .await?;

    // Listed again counts: a lock changes what a work is counted under
    // without changing the work.
    Ok(format!(
        "{}@{}@{}",
        row.big("n")?,
        row.opt_text("at")?.unwrap_or_default(),
        row.opt_text("listed")?.unwrap_or_default()
    ))
}

/// An episode of an enabled series, for a list across works.
#[derive(Clone, Debug)]
pub struct Airing {
    pub media_id: String,
    pub episode: Episode,
}

/// The most episodes one window answers with.
pub const MAX_AIRING: i64 = 500;

/// Episodes of enabled series that air in `[from, to)`, earliest first, and
/// whether the window held more than [`MAX_AIRING`] of them.
///
/// `from` and `to` are instants written as `YYYY-MM-DDTHH:MM:SSZ`, the form
/// every provider's `air_date_utc` is stored in, so the comparison is a string
/// comparison on both engines. An episode with a date and no time is counted
/// at midnight UTC on that date — the moment Sonarr is given for it.
///
/// The episodes whose date somebody corrected come too, wherever their stored
/// date is: the correction is applied after this, and can move an episode
/// into the window as easily as out of it. The caller keeps those it lands in.
pub async fn airing(
    db: &Db,
    from: &str,
    to: &str,
    include_adult: bool,
) -> Result<(Vec<Airing>, bool)> {
    let adult = if include_adult {
        ""
    } else {
        " AND m.is_adult = 0"
    };
    let columns = EPISODE_COLUMNS
        .split(',')
        .map(|c| format!("e.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");

    // The two kinds of episode as two statements, each walked from its own
    // index on the episodes. As one with an OR between them, SQLite walked
    // every episode of every series instead: some seventy milliseconds at
    // half a million episodes, against three. Ties are broken all the way
    // down, so a season dropped at once comes back in its own order.
    let sql = format!(
        "SELECT e.media_id, {columns}, e.air_date_utc AS aired_at, m.title AS work_title
         FROM media_episode e JOIN media_item m ON m.id = e.media_id
         WHERE e.air_date_utc >= ? AND e.air_date_utc < ?
           AND m.is_enabled = 1 AND m.kind = 'series'{adult}
         UNION ALL
         SELECT e.media_id, {columns}, e.air_date || 'T00:00:00Z' AS aired_at, m.title AS work_title
         FROM media_episode e JOIN media_item m ON m.id = e.media_id
         WHERE e.air_date_utc IS NULL AND e.air_date >= ? AND e.air_date <= ?
           AND e.air_date || 'T00:00:00Z' >= ? AND e.air_date || 'T00:00:00Z' < ?
           AND m.is_enabled = 1 AND m.kind = 'series'{adult}
         ORDER BY aired_at, work_title, media_id, season_number, episode_number
         LIMIT ?"
    );

    let day = |instant: &str| instant.get(..10).unwrap_or(instant).to_string();

    let rows = sqlx::query(db.sql(&sql))
        .bind(from)
        .bind(to)
        .bind(day(from))
        .bind(day(to))
        .bind(from)
        .bind(to)
        // One more than is answered, to know whether there was more.
        .bind(MAX_AIRING + 1)
        .fetch_all(db.pool())
        .await?;

    let mut found = rows
        .iter()
        .map(|row| {
            Ok(Airing {
                media_id: row.text("media_id")?,
                episode: map_episode(row)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let truncated = found.len() as i64 > MAX_AIRING;
    found.truncate(MAX_AIRING as usize);

    // A handful of rows, read from the corrections rather than the episodes.
    let moved_sql = format!(
        "SELECT e.media_id, {columns} FROM media_override o
         JOIN media_episode e ON e.media_id = o.media_id
          AND o.scope = 'episode:' || CAST(e.season_number AS TEXT) || 'x' || CAST(e.episode_number AS TEXT)
         JOIN media_item m ON m.id = e.media_id
         WHERE o.field IN ('airDate', 'airDateUtc')
           AND m.is_enabled = 1 AND m.kind = 'series'{adult}
         LIMIT ?"
    );

    let moved = sqlx::query(db.sql(&moved_sql))
        .bind(MAX_AIRING)
        .fetch_all(db.pool())
        .await?;

    let mut seen: std::collections::HashSet<String> =
        found.iter().map(|a| a.episode.id.clone()).collect();
    for row in &moved {
        let airing = Airing {
            media_id: row.text("media_id")?,
            episode: map_episode(row)?,
        };
        if seen.insert(airing.episode.id.clone()) {
            found.push(airing);
        }
    }

    Ok((found, truncated))
}

/// The most film releases one window answers with: a film out in cinemas
/// and digitally on two days is two.
pub const MAX_RELEASING: i64 = 500;

/// Enabled films that come out on a day of `[from, to)` — in cinemas,
/// digitally or on disc — earliest first, and whether the window held more
/// than [`MAX_RELEASING`] such releases.
///
/// A release has a day and no time, and is counted at midnight UTC on it, as
/// an episode with only a date is. `from` and `to` are instants written as
/// `YYYY-MM-DDTHH:MM:SSZ`; a release is stored as a day, or as an instant the
/// way Radarr writes one, and read by its first ten characters either way.
///
/// The films whose release somebody corrected into the window come too,
/// wherever their stored dates are, as the episodes do in [`airing`]; one
/// corrected out of it comes by its stored date. The caller applies the
/// corrections and keeps the releases they leave in the window.
pub async fn releasing(
    db: &Db,
    from: &str,
    to: &str,
    include_adult: bool,
) -> Result<(Vec<String>, bool)> {
    let adult = if include_adult {
        ""
    } else {
        " AND m.is_adult = 0"
    };

    // A statement each kind of release, so each is ordered by its own day; a
    // film is read a second time for a second release in the window. Nothing
    // indexes these dates: films are few beside episodes, and read from the
    // index on their kind.
    let kind = |column: &str| {
        format!(
            "SELECT m.id AS media_id, SUBSTR(m.{column}, 1, 10) AS released_on, m.title AS work_title
             FROM media_item m
             WHERE m.kind = 'movie' AND m.is_enabled = 1{adult}
               AND SUBSTR(m.{column}, 1, 10) || 'T00:00:00Z' >= ?
               AND SUBSTR(m.{column}, 1, 10) || 'T00:00:00Z' < ?"
        )
    };
    let sql = format!(
        "{} UNION ALL {} UNION ALL {}
         ORDER BY released_on, work_title, media_id
         LIMIT ?",
        kind("in_cinemas"),
        kind("digital_release"),
        kind("physical_release"),
    );

    let rows = sqlx::query(db.sql(&sql))
        .bind(from)
        .bind(to)
        .bind(from)
        .bind(to)
        .bind(from)
        .bind(to)
        // One more than is answered, to know whether there was more.
        .bind(MAX_RELEASING + 1)
        .fetch_all(db.pool())
        .await?;

    let truncated = rows.len() as i64 > MAX_RELEASING;
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    for row in rows.iter().take(MAX_RELEASING as usize) {
        let id = row.text("media_id")?;
        if seen.insert(id.clone()) {
            found.push(id);
        }
    }

    // A handful of rows, read from the corrections rather than the films:
    // those whose day is in the window. A correction is held as JSON, so the
    // day starts after the quote — `"2026-09-24"` — and `null`, a date
    // cleared, is no day at all.
    let moved = sqlx::query(db.sql(&format!(
        "SELECT DISTINCT o.media_id FROM media_override o
         JOIN media_item m ON m.id = o.media_id
         WHERE o.scope = 'item' AND o.field IN ('inCinemas', 'digitalRelease', 'physicalRelease')
           AND SUBSTR(o.value, 2, 10) || 'T00:00:00Z' >= ?
           AND SUBSTR(o.value, 2, 10) || 'T00:00:00Z' < ?
           AND m.kind = 'movie' AND m.is_enabled = 1{adult}
         LIMIT ?"
    )))
    .bind(from)
    .bind(to)
    .bind(MAX_RELEASING)
    .fetch_all(db.pool())
    .await?;

    for row in &moved {
        let id = row.text("media_id")?;
        if seen.insert(id.clone()) {
            found.push(id);
        }
    }

    Ok((found, truncated))
}

/// One of a person's credits, on an enabled work.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct PersonCredit {
    pub credit: crate::domain::Credit,
}

/// Every credit filed under the person TMDB knows by this id, on works this
/// catalogue shows, as `repo::person::credits` reads them: what the tests
/// of a work's locks look for on the person's page.
#[cfg(test)]
pub async fn person_credits(
    db: &Db,
    tmdb_person_id: i64,
    include_adult: bool,
) -> Result<Vec<PersonCredit>> {
    let Some(person) = crate::db::repo::person::find(db, &tmdb_person_id.to_string()).await? else {
        return Ok(Vec::new());
    };
    Ok(
        crate::db::repo::person::credits(db, &person.id, include_adult)
            .await?
            .into_iter()
            .map(|credited| PersonCredit {
                credit: credited.credit,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::{repo::item::testing::stored, testing::db},
        domain::MediaKind,
    };

    #[tokio::test]
    async fn facets_count_what_is_on_view() {
        let db = db().await;
        stored(&db, |i| {
            i.title = "One".into();
            i.genres = vec!["Drama".into(), "Crime".into()];
            i.network = Some("AMC".into());
            i.original_language = Some("en".into());
            i.year = Some(2008);
        })
        .await;
        stored(&db, |i| {
            i.title = "Two".into();
            i.genres = vec!["Drama".into()];
            i.studio = Some("Wit Studio".into());
            i.original_language = Some("ja".into());
            i.year = Some(2013);
        })
        .await;
        stored(&db, |i| {
            i.title = "Hidden".into();
            i.genres = vec!["Hentai".into()];
            i.is_adult = true;
        })
        .await;

        let facets = facets(&db, &Query::default()).await.expect("counted");

        assert_eq!(
            facets.total, 2,
            "an adult work is not counted for someone who would not see it"
        );
        assert_eq!(
            facets.genres[0],
            Facet {
                value: "Drama".into(),
                count: 2,
                slug: Some("drama".into()),
            }
        );
        assert!(facets.genres.iter().all(|g| g.value != "Hentai"));
        assert_eq!(facets.networks.len(), 2);
        assert_eq!((facets.year_min, facets.year_max), (Some(2008), Some(2013)));
    }

    #[tokio::test]
    async fn a_genre_named_in_two_languages_is_counted_once_by_its_slug() {
        let db = db().await;
        // Fetched in English, then in French after the server's language
        // changed, and once by a source with its own word for it.
        for (title, genres) in [
            ("English", vec!["Drama", "Science Fiction"]),
            ("French", vec!["Drame", "Science-Fiction"]),
            ("Both", vec!["Drama", "Drame", "Sci-Fi"]),
            ("Learned", vec!["Komödie"]),
        ] {
            stored(&db, |i| {
                i.title = title.into();
                i.genres = genres.into_iter().map(String::from).collect();
            })
            .await;
        }
        crate::service::listing::relist_stale(&db, false, 100)
            .await
            .expect("listed");

        let facets = facets(&db, &Query::default()).await.expect("counted");
        let found: Vec<(&str, i64, Option<&str>)> = facets
            .genres
            .iter()
            .map(|g| (g.value.as_str(), g.count, g.slug.as_deref()))
            .collect();
        assert_eq!(
            found,
            [
                ("Drama", 3, Some("drama")),
                ("Science Fiction", 3, Some("science-fiction")),
                ("Komödie", 1, Some("komodie")),
            ]
        );
        // Asked for by any of its names, or its slug: the same three works.
        for asked in ["Drama", "Drame", "drama"] {
            let narrowed = facets_of(&db, asked).await;
            assert_eq!(narrowed, 3, "{asked}");
        }
        assert_eq!(facets_of(&db, "Komödie").await, 1);
        assert_eq!(facets_of(&db, "komodie").await, 1);
    }

    /// How many works a genre asked for by `name` lists.
    async fn facets_of(db: &Db, name: &str) -> i64 {
        facets(
            db,
            &Query {
                genres: vec![name.into()],
                ..Default::default()
            },
        )
        .await
        .expect("counted")
        .total
    }

    #[tokio::test]
    async fn facets_count_what_each_choice_would_leave() {
        let db = db().await;
        for (title, genres, status) in [
            ("Running drama", vec!["Drama", "Crime"], "continuing"),
            ("Ended drama", vec!["Drama"], "ended"),
            ("Ended comedy", vec!["Comedy"], "ended"),
        ] {
            stored(&db, |i| {
                i.title = title.into();
                i.genres = genres.into_iter().map(String::from).collect();
                i.status = Some(status.into());
            })
            .await;
        }

        let facets = facets(
            &db,
            &Query {
                genres: vec!["Drama".into()],
                status: Some("ended".into()),
                ..Default::default()
            },
        )
        .await
        .expect("counted");

        let count =
            |list: &[Facet], value: &str| list.iter().find(|f| f.value == value).map(|f| f.count);

        assert_eq!(facets.total, 1, "one ended drama");
        // Genres are all required: each is counted among what is listed now.
        assert_eq!(count(&facets.genres, "Drama"), Some(1));
        assert_eq!(
            count(&facets.genres, "Comedy"),
            None,
            "no ended drama is also a comedy"
        );
        // A status replaces the one chosen: counted as if it were not.
        assert_eq!(count(&facets.statuses, "continuing"), Some(1));
        assert_eq!(count(&facets.statuses, "ended"), Some(1));
    }

    #[tokio::test]
    async fn the_calendar_reads_the_window_and_places_dateless_times_at_midnight() {
        let db = db().await;

        let episode = |number: i32, date: &str, utc: Option<&str>| {
            let mut e = crate::db::repo::child::blank_episode(1, number);
            // As a provider gave it: a refresh writes only those.
            e.is_manual = false;
            e.air_date = Some(date.into());
            e.air_date_utc = utc.map(String::from);
            e
        };

        stored(&db, |i| {
            i.kind = MediaKind::Series;
            i.title = "Airing".into();
            i.episodes = vec![
                episode(1, "2026-09-20", Some("2026-09-21T01:30:00Z")),
                // No time known: counted at midnight UTC on its date.
                episode(2, "2026-09-24", None),
                episode(3, "2026-10-30", Some("2026-10-31T01:30:00Z")),
            ];
        })
        .await;
        stored(&db, |i| {
            i.kind = MediaKind::Series;
            i.title = "Switched off".into();
            i.is_enabled = false;
            i.episodes = vec![episode(1, "2026-09-22", Some("2026-09-22T20:00:00Z"))];
        })
        .await;

        let read = async |from: &str, to: &str| {
            let (found, truncated) = airing(&db, from, to, false).await.expect("read");
            assert!(!truncated);
            found
                .iter()
                .map(|a| a.episode.episode_number)
                .collect::<Vec<i32>>()
        };

        assert_eq!(
            read("2026-09-21T00:00:00Z", "2026-09-28T00:00:00Z").await,
            [1, 2],
            "the disabled series and the episode outside are left out"
        );

        // Episode 2 has a date and no time, so it is midnight UTC on that date:
        // a window from one second after misses it, one ending on it misses
        // it, and one ending a second after holds it.
        assert_eq!(
            read("2026-09-24T00:00:01Z", "2026-09-28T00:00:00Z").await,
            [] as [i32; 0]
        );
        assert_eq!(
            read("2026-09-22T00:00:00Z", "2026-09-24T00:00:00Z").await,
            [] as [i32; 0]
        );
        assert_eq!(
            read("2026-09-22T00:00:00Z", "2026-09-24T00:00:01Z").await,
            [2]
        );
    }

    #[tokio::test]
    async fn an_episode_a_correction_moved_is_read_wherever_it_is_stored() {
        let db = db().await;

        let mut postponed = crate::db::repo::child::blank_episode(2, 5);
        postponed.is_manual = false;
        postponed.air_date = Some("2027-01-01".into());
        let work = stored(&db, |i| {
            i.kind = MediaKind::Series;
            i.title = "Postponed".into();
            i.episodes = vec![postponed];
        })
        .await;

        // Its network brought it forward; the provider has not caught up.
        crate::db::repo::override_field::set(
            &db,
            &work.id,
            crate::domain::fields::Scope::Episode {
                season: 2,
                episode: 5,
            },
            "airDate",
            Some(&serde_json::json!("2026-09-25")),
            None,
        )
        .await
        .expect("corrected");

        let (found, _) = airing(&db, "2026-09-21T00:00:00Z", "2026-09-28T00:00:00Z", false)
            .await
            .expect("read");

        // Read with its stored date: the caller applies the correction, and
        // then keeps it in this window.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].episode.air_date.as_deref(), Some("2027-01-01"));
    }

    #[tokio::test]
    async fn the_calendar_reads_the_films_coming_out_in_the_window() {
        let db = db().await;
        let week = ("2026-09-21T00:00:00Z", "2026-09-28T00:00:00Z");

        let cinema = stored(&db, |i| {
            i.title = "In cinemas".into();
            i.in_cinemas = Some("2026-09-23".into());
        })
        .await;
        // Out long ago in cinemas, then digitally and on disc this week — the
        // digital day written as Radarr writes one.
        let at_home = stored(&db, |i| {
            i.title = "At home".into();
            i.in_cinemas = Some("2026-05-01".into());
            i.digital_release = Some("2026-09-22T00:00:00Z".into());
            i.physical_release = Some("2026-09-26".into());
        })
        .await;
        let long_out = stored(&db, |i| {
            i.title = "Long out".into();
            i.in_cinemas = Some("2026-01-05".into());
        })
        .await;
        for (title, enabled, adult) in [("Switched off", false, false), ("For adults", true, true)]
        {
            stored(&db, |i| {
                i.title = title.into();
                i.in_cinemas = Some("2026-09-23".into());
                i.is_enabled = enabled;
                i.is_adult = adult;
            })
            .await;
        }
        // A series is placed by its episodes, whatever else it was given.
        stored(&db, |i| {
            i.kind = MediaKind::Series;
            i.title = "A series".into();
            i.in_cinemas = Some("2026-09-23".into());
        })
        .await;

        let read = async |from: &str, to: &str, adult: bool| {
            let (found, truncated) = releasing(&db, from, to, adult).await.expect("read");
            assert!(!truncated);
            found
        };

        // Each film once, by its first release in the window: the 22nd, then
        // the 23rd. Switched off, for adults, out before: left out.
        assert_eq!(
            read(week.0, week.1, false).await,
            [at_home.id.clone(), cinema.id.clone()]
        );
        assert_eq!(read(week.0, week.1, true).await.len(), 3, "and adults' too");

        // A release is at midnight UTC on its day: a window from one second
        // after misses it, one ending a second after holds it.
        assert!(read("2026-09-26T00:00:01Z", week.1, false).await.is_empty());
        assert_eq!(
            read("2026-09-25T00:00:00Z", "2026-09-26T00:00:01Z", false).await,
            std::slice::from_ref(&at_home.id)
        );

        // A digital release set by hand in the window, and a disc set by hand
        // out of it: the first is read with its stored dates, wherever they
        // are, for the caller to place; the second has nothing to place.
        let corrected = async |work: &MediaItem, field: &str, day: &str| {
            crate::db::repo::override_field::set(
                &db,
                &work.id,
                crate::domain::fields::Scope::Item,
                field,
                Some(&serde_json::json!(day)),
                None,
            )
            .await
            .expect("corrected");
        };
        let elsewhere = stored(&db, |i| {
            i.title = "Out of the week".into();
            i.in_cinemas = Some("2026-02-02".into());
        })
        .await;
        corrected(&long_out, "digitalRelease", "2026-09-24").await;
        corrected(&elsewhere, "physicalRelease", "2027-03-01T00:00:00Z").await;
        assert_eq!(
            read(week.0, week.1, false).await,
            [at_home.id.clone(), cinema.id.clone(), long_out.id.clone()]
        );
    }
}
