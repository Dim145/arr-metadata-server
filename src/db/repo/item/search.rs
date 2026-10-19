//! Search and listing: the one predicate every list, count and search of the
//! catalogue is narrowed by, the page it reads and what its cards show, and
//! the figures counted over the catalogue.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::{Arguments, any::AnyArguments};

use super::read::{HELD_ADULT, ITEM_COLUMNS, held, map_image, map_item, map_rating};
use crate::{
    db::{Db, RowExt},
    domain::{ExternalIds, ExternalSource, Image, Kin, MediaItem, MediaKind, Rating, Translation},
};

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub term: Option<String>,
    pub kind: Option<MediaKind>,
    pub year: Option<i32>,
    pub manual_only: bool,
    pub include_disabled: bool,
    /// Whether adult titles may appear. Defaulting to `false` means a caller
    /// that forgets to decide gets the safe answer rather than the open one.
    pub include_adult: bool,
    /// Works carrying every one of these genres.
    pub genres: Vec<String>,
    /// Or any one of them, where asked: a mood rather than a narrowing.
    pub genre_any: bool,
    pub keyword: Option<String>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub status: Option<String>,
    pub original_language: Option<String>,
    /// A network or a studio, by name.
    pub network: Option<String>,
    pub collection: Option<i64>,
    /// Works whose score — the one their card leads with — is at least this.
    pub min_rating: Option<f64>,
    /// Works whose last refresh failed.
    pub refresh_failed: bool,
    /// Works lacking every one of these: the gaps the administration's
    /// register is narrowed to, and its to-do list counts.
    pub missing: Vec<Missing>,
    /// The language the catalogue keeps its works in, as a lock is filed
    /// under it (`fra`): a work's overview is the one locked in that
    /// language, else the one locked in every language, as it is read. What
    /// the overview gap goes by; left out, a lock counts only in every
    /// language.
    pub stored_language: Option<String>,
    pub sort: Sort,
    /// `None` is the sort's own direction: A to Z for titles, highest,
    /// newest or most popular first for the rest.
    pub descending: Option<bool>,
    pub limit: i64,
    pub offset: i64,
}

/// What a list is ordered by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Popularity,
    Rating,
    Release,
    Title,
    Added,
    Refreshed,
}

impl std::str::FromStr for Sort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "popularity" => Ok(Self::Popularity),
            "rating" => Ok(Self::Rating),
            "release" => Ok(Self::Release),
            "title" => Ok(Self::Title),
            "added" => Ok(Self::Added),
            "refreshed" => Ok(Self::Refreshed),
            other => Err(format!(
                "unknown sort {other:?}: expected popularity, rating, release, title, added or refreshed"
            )),
        }
    }
}

/// A gap in a work's record, as the administration's register narrows to it
/// and its to-do list counts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Missing {
    /// No poster of its own: none from a source, none chosen by hand.
    Poster,
    /// No overview in the language it is stored in — none given, or one
    /// cleared by hand, in that language or in every one.
    Overview,
    /// No IMDb identifier, which IMDb's rating and the clients' links need.
    Imdb,
    /// Never refreshed, although it has an identifier to be refreshed by. A
    /// work entered by hand is never refreshed, and is not one of these.
    Refresh,
}

impl Missing {
    pub const ALL: [Missing; 4] = [Self::Poster, Self::Overview, Self::Imdb, Self::Refresh];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Overview => "overview",
            Self::Imdb => "imdb",
            Self::Refresh => "refresh",
        }
    }

    /// What a row of `media_item` lacking it meets, and how many times the
    /// language the catalogue is stored in is bound in it — the overview's
    /// alone; the words are the same on both engines. A lock in every
    /// language counts, and one in the stored language, which is the text the
    /// work is read with and over the other; one in another language is its
    /// readers' alone, and the stored record is what everybody else is given.
    fn condition(self) -> (&'static str, usize) {
        let words = match self {
            // A poster of the work's own — a season's is the season's — or one
            // chosen by hand, which its lock brings back when the sources no
            // longer list it.
            Self::Poster => {
                r"(NOT EXISTS (SELECT 1 FROM media_image i
                               WHERE i.media_id = media_item.id AND i.cover_type = 'poster'
                                 AND i.season_number IS NULL)
                   AND NOT EXISTS (SELECT 1 FROM media_override o
                                   WHERE o.media_id = media_item.id AND o.scope = 'item'
                                     AND o.field = 'primaryPoster' AND o.language = ''
                                     AND o.value IS NOT NULL AND o.value <> 'null'))"
            }
            // The overview served in the stored language: the lock made in it
            // where there is one, else the one made in every language —
            // cleared, either is an empty one — and the sources' otherwise.
            Self::Overview => {
                return (
                    r#"(EXISTS (SELECT 1 FROM media_override o
                               WHERE o.media_id = media_item.id AND o.scope = 'item'
                                 AND o.field = 'overview'
                                 AND (o.language = ?
                                      OR (o.language = ''
                                          AND NOT EXISTS (
                                              SELECT 1 FROM media_override l
                                              WHERE l.media_id = media_item.id
                                                AND l.scope = 'item' AND l.field = 'overview'
                                                AND l.language = ?)))
                                 AND (o.value IS NULL OR o.value IN ('null', '""')))
                       OR (COALESCE(TRIM(media_item.overview), '') = ''
                           AND NOT EXISTS (SELECT 1 FROM media_override o
                                           WHERE o.media_id = media_item.id AND o.scope = 'item'
                                             AND o.field = 'overview'
                                             AND o.language IN ('', ?))))"#,
                    3,
                );
            }
            Self::Imdb => {
                r"NOT EXISTS (SELECT 1 FROM media_external_id e
                              WHERE e.media_id = media_item.id AND e.source = 'imdb')"
            }
            Self::Refresh => {
                r"(media_item.refreshed_at IS NULL AND media_item.is_manual = 0
                   AND EXISTS (SELECT 1 FROM media_external_id e
                               WHERE e.media_id = media_item.id))"
            }
        };
        (words, 0)
    }
}

impl std::str::FromStr for Missing {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|gap| gap.as_str() == s)
            .ok_or_else(|| format!("unknown gap {s:?}: expected poster, overview, imdb or refresh"))
    }
}

/// The most rows one call will return, whatever it was asked for.
///
/// A caller that wants more pages through `offset`. It is public because a
/// caller that means to read everything needs to know where the ceiling is —
/// the NFO export asked for ten thousand, was quietly given five hundred, and
/// reported a complete run over a fifth of the library.
pub const MAX_PAGE: i64 = 500;

/// Shallow search over the canonical store.
///
/// Matching is a case-insensitive substring over the title, the sort title and
/// every alternative title, which is what clients searching by a localised name
/// need. Anything more (ranking, typo tolerance) belongs in a later FTS index.
pub async fn search(db: &Db, q: &Query) -> Result<Vec<MediaItem>> {
    let mut sql = format!(
        "SELECT {ITEM_COLUMNS} FROM {} WHERE 1 = 1",
        from_clause(db, q)
    );
    let mut args = AnyArguments::default();
    narrow(db.dialect(), q, &mut sql, &mut args)?;

    sql.push_str(" ORDER BY ");
    sql.push_str(&order_clause(q));
    sql.push_str(" LIMIT ? OFFSET ?");
    args.add(q.limit.clamp(1, MAX_PAGE))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    args.add(q.offset.max(0))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(map_item(row)?);
    }

    attach_external_ids(db, &mut items).await?;

    Ok(items)
}

/// Fill in every result's identifiers, in one query rather than one each.
///
/// The same shape as [`load_artwork`] below and for the same reason: a page of
/// five hundred was five hundred and one round trips, against a pool of eight
/// connections, for every catalogue list and every local search.
pub(super) async fn attach_external_ids(db: &Db, items: &mut [MediaItem]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let holes = vec!["?"; ids.len()].join(", ");

    let mut args = AnyArguments::default();
    for id in &ids {
        args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let sql = format!(
        "SELECT media_id, source, value FROM media_external_id
         WHERE media_id IN ({holes}) ORDER BY source"
    );

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let mut by_item: std::collections::HashMap<String, ExternalIds> =
        std::collections::HashMap::new();

    for row in &rows {
        let raw = row.text("source")?;
        match raw.parse::<ExternalSource>() {
            Ok(source) => by_item
                .entry(row.text("media_id")?)
                .or_default()
                .apply(source, &row.text("value")?),
            // The CHECK constraint makes this unreachable today; if a future
            // migration adds a source this build does not know, skip it.
            Err(e) => tracing::warn!(error = %e, "ignoring unknown external id source"),
        }
    }

    for item in items {
        if let Some(ids) = by_item.remove(&item.id) {
            item.external_ids = ids;
        }
    }

    Ok(())
}

/// Whether a query searches text, which no index can help with.
fn searches_text(q: &Query) -> bool {
    q.term.as_deref().is_some_and(|t| !t.trim().is_empty())
}

/// The table to read, and — for a text search on SQLite — the instruction to
/// read it straight through.
///
/// `LIKE '%term%'` has to look at every row, so a text search is a full read
/// whatever happens. But `ix_media_item_browse` carries `is_enabled` and
/// `is_adult`, and SQLite's planner prefers filtering through an index that
/// holds the filter columns to reading the table: it walks the index, then
/// fetches nearly every row from the table one at a time, because nearly every
/// row passes. Measured at fifty thousand works, a search for a term that
/// matched nothing went from 22 ms to 97 ms that way — and a search for
/// something not yet stored is exactly what every Sonarr and Radarr search does
/// first. `NOT INDEXED` is SQLite's own spelling of "don't"; PostgreSQL's cost
/// model gets there by itself once [`order_clause`] stops it walking an index
/// for the order.
pub(super) fn from_clause(db: &Db, q: &Query) -> &'static str {
    match (db.dialect(), searches_text(q)) {
        (crate::db::Dialect::Sqlite, true) => "media_item NOT INDEXED",
        _ => "media_item",
    }
}

/// The order every list is in.
///
/// For a text search, by an expression rather than the column: `popularity + 0`
/// sorts the same way, and no index can produce it. Otherwise PostgreSQL walks
/// `ix_media_item_popular` in order, betting that enough rows match early to
/// stop after thirty-six — a bet that loses badly for a term matching nothing,
/// 38 ms becoming 59 at fifty thousand works. With nothing to walk it reads
/// the table and sorts what matched, which is what a text search costs anyway.
///
/// Anything but popularity is sorted rather than walked: nothing is indexed
/// by score or date, and for a catalogue's worth of rows that costs little.
/// Missing values go last whichever way the list runs, so a work with no date
/// is not presented as the oldest.
fn order_clause(q: &Query) -> String {
    let (expression, descending) = match q.sort {
        Sort::Popularity if searches_text(q) => ("(popularity + 0)", true),
        Sort::Popularity => ("popularity", true),
        Sort::Rating => ("listed_score", true),
        Sort::Release => ("listed_release", true),
        Sort::Title => ("listed_title", false),
        Sort::Added => ("created_at", true),
        Sort::Refreshed => ("refreshed_at", true),
    };

    let direction = if q.descending.unwrap_or(descending) {
        "DESC"
    } else {
        "ASC"
    };

    format!("{expression} {direction} NULLS LAST, title ASC")
}

/// `value` as one element of a JSON array of strings reads in the column that
/// stores genres and keywords: encoded the way the column was, so its quotes
/// delimit it and `"Drama"` is not found inside `"Docudrama"`.
///
/// Looked for with `REPLACE(column, ?, '') <> column`, a substring test both
/// engines make the same way. `LIKE` did not: SQLite folds ASCII case in it and
/// PostgreSQL does not, so `drama` found two works on one and none on the other.
fn json_element(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// Attach the artwork, scores and translations a list of works needs.
///
/// [`search`] returns the rows and nothing hanging off them, which is right for
/// a table and useless for a grid of posters. Loading every child for fifty
/// works would mean loading One Piece's twelve hundred episodes to draw one
/// thumbnail, so this fetches only what a card puts on screen, batched into two
/// statements rather than two per work.
pub async fn load_artwork(db: &Db, items: &mut [MediaItem]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let holes = vec!["?"; ids.len()].join(", ");

    let mut image_args = AnyArguments::default();
    for id in &ids {
        image_args
            .add(id.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    // Season artwork belongs to a season, and a card shows the work.
    let image_sql = format!(
        "SELECT media_id, id, season_number, cover_type, url, language, sort_order, source, is_manual
         FROM media_image
         WHERE media_id IN ({holes}) AND season_number IS NULL
         ORDER BY cover_type, sort_order"
    );

    let image_rows = sqlx::query_with(db.sql(&image_sql), image_args)
        .fetch_all(db.pool())
        .await?;

    let mut rating_args = AnyArguments::default();
    for id in &ids {
        rating_args
            .add(id.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let rating_sql = format!(
        "SELECT media_id, source, value, votes, rating_type
         FROM media_rating WHERE media_id IN ({holes})"
    );

    let rating_rows = sqlx::query_with(db.sql(&rating_sql), rating_args)
        .fetch_all(db.pool())
        .await?;

    let mut translation_args = AnyArguments::default();
    for id in &ids {
        translation_args
            .add(id.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    // Without these a list asked for in French would come back in English while
    // the work's own page came back translated.
    let translation_sql = format!(
        "SELECT media_id, language, title, overview, tagline, is_manual
         FROM media_translation WHERE media_id IN ({holes})"
    );

    let translation_rows = sqlx::query_with(db.sql(&translation_sql), translation_args)
        .fetch_all(db.pool())
        .await?;

    let mut images: HashMap<String, Vec<Image>> = HashMap::new();
    for row in &image_rows {
        images
            .entry(row.text("media_id")?)
            .or_default()
            .push(map_image(row)?);
    }

    let mut ratings: HashMap<String, Vec<Rating>> = HashMap::new();
    for row in &rating_rows {
        ratings
            .entry(row.text("media_id")?)
            .or_default()
            .push(map_rating(row)?);
    }

    let mut translations: HashMap<String, Vec<Translation>> = HashMap::new();
    for row in &translation_rows {
        translations
            .entry(row.text("media_id")?)
            .or_default()
            .push(Translation {
                language: row.text("language")?,
                title: row.opt_text("title")?,
                overview: row.opt_text("overview")?,
                tagline: row.opt_text("tagline")?,
                is_manual: row.flag("is_manual")?,
            });
    }

    for item in items {
        let mut own = images.remove(&item.id).unwrap_or_default();
        // SQL ordered by the cover type's name; a card wants the poster first.
        own.sort_by_key(|i| (i.cover_type.priority(), i.sort_order));
        // A picture a source gave and a person hid is none of the work's: as
        // its locks have it, applied before this as every list applies them.
        own.retain(|i| {
            i.is_manual
                || !item
                    .locked_fields
                    .contains(&format!("image:{}/hidden", i.url))
        });

        item.images = own;
        item.ratings = ratings.remove(&item.id).unwrap_or_default();
        item.translations = translations.remove(&item.id).unwrap_or_default();
    }

    Ok(())
}

/// The most collections one listing carries.
pub const MOST_COLLECTIONS: usize = 200;

/// The collections the catalogue's films belong to, by TMDB id, with how
/// many of each it holds — most films first. Only films switched on, and
/// adult ones only where asked.
pub async fn collections(db: &Db, include_adult: bool) -> Result<Vec<(i64, i64)>> {
    let sql = format!(
        "SELECT collection_tmdb_id AS id, COUNT(*) AS n FROM media_item
          WHERE kind = 'movie' AND is_enabled = 1 AND collection_tmdb_id IS NOT NULL{}
          GROUP BY collection_tmdb_id
          ORDER BY n DESC, id
          LIMIT {MOST_COLLECTIONS}",
        if include_adult {
            ""
        } else {
            " AND is_adult = 0"
        }
    );
    let rows = sqlx::query(db.sql(&sql)).fetch_all(db.pool()).await?;
    rows.iter()
        .map(|row| Ok((row.big("id")?, row.big("n")?)))
        .collect()
}

/// Works by decade of release, earliest first: switched on, adult only
/// where asked.
pub async fn decades(db: &Db, include_adult: bool) -> Result<Vec<(i32, i64)>> {
    let sql = format!(
        "SELECT (listed_year / 10) * 10 AS decade, COUNT(*) AS n FROM media_item
          WHERE is_enabled = 1 AND listed_year IS NOT NULL{}
          GROUP BY decade ORDER BY decade",
        if include_adult {
            ""
        } else {
            " AND is_adult = 0"
        }
    );
    let rows = sqlx::query(db.sql(&sql)).fetch_all(db.pool()).await?;
    rows.iter()
        .map(|row| Ok((row.int("decade")?, row.big("n")?)))
        .collect()
}

/// Works by the whole part of the score their card leads with, lowest first.
///
/// `CAST` to an integer truncates on SQLite and rounds on PostgreSQL, so a
/// seven point nine would be counted as seven on one and eight on the other.
/// The `CASE` brings a rounded-up value back down: both count it as seven.
/// (`FLOOR` would say it in a word, but the SQLite this is built with does
/// not carry the math functions.)
pub async fn score_buckets(db: &Db, include_adult: bool) -> Result<Vec<(i64, i64)>> {
    let sql = format!(
        "SELECT CASE WHEN CAST(listed_score AS INTEGER) > listed_score
                     THEN CAST(listed_score AS INTEGER) - 1
                     ELSE CAST(listed_score AS INTEGER) END AS bucket,
                COUNT(*) AS n
           FROM media_item
          WHERE is_enabled = 1 AND listed_score IS NOT NULL{}
          GROUP BY bucket ORDER BY bucket",
        if include_adult {
            ""
        } else {
            " AND is_adult = 0"
        }
    );
    let rows = sqlx::query(db.sql(&sql)).fetch_all(db.pool()).await?;
    rows.iter()
        .map(|row| Ok((row.big("bucket")?, row.big("n")?)))
        .collect()
}

/// The episodes of the works: of those switched on, unless the ones switched
/// off are asked for too.
pub async fn episode_count(db: &Db, include_adult: bool, include_disabled: bool) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) AS n FROM media_episode e
           JOIN media_item m ON m.id = e.media_id
          WHERE 1 = 1{}{}",
        if include_disabled {
            ""
        } else {
            " AND m.is_enabled = 1"
        },
        if include_adult {
            ""
        } else {
            " AND m.is_adult = 0"
        }
    );
    let row = sqlx::query(db.sql(&sql)).fetch_one(db.pool()).await?;
    Ok(row.big("n")?)
}

/// Works added since an instant, RFC 3339.
pub async fn added_since(db: &Db, since: &str, include_adult: bool) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) AS n FROM media_item WHERE is_enabled = 1 AND created_at >= ?{}",
        if include_adult {
            ""
        } else {
            " AND is_adult = 0"
        }
    );
    let row = sqlx::query(db.sql(&sql))
        .bind(since)
        .fetch_one(db.pool())
        .await?;
    Ok(row.big("n")?)
}

/// What each of these works is the sequel of, where a provider filed a
/// prequel: the nearest one — the latest, where years are known — with the
/// work here that it is, when the catalogue holds it. A work with none filed
/// is absent; so is one whose prequels are all for adults, unless adult works
/// are asked for, and a held work for adults is then not led to either.
pub async fn prequels_of(
    db: &Db,
    ids: &[String],
    include_adult: bool,
) -> Result<std::collections::HashMap<String, Kin>> {
    let mut kin = std::collections::HashMap::new();
    let held = held(include_adult);
    // For adults by AniList's flag, or by the work held: neither is named.
    let adult = if include_adult {
        String::new()
    } else {
        format!(" AND r.is_adult = 0 AND COALESCE({HELD_ADULT}, 0) = 0")
    };
    for chunk in ids.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT r.media_id, r.title, {held} AS work_id
               FROM media_relation r
              WHERE r.relation_type = 'PREQUEL'{adult} AND r.media_id IN ({marks})
              ORDER BY r.media_id, r.year DESC NULLS LAST, r.sort_order"
        );
        let mut query = sqlx::query(db.sql(&sql));
        for id in chunk {
            query = query.bind(id);
        }
        for row in query.fetch_all(db.pool()).await? {
            let media_id = row.text("media_id")?;
            let work_id = row.opt_text("work_id")?;
            // Its own entry, or one already chosen: see `load_relations`.
            if work_id.as_deref() == Some(media_id.as_str()) || kin.contains_key(&media_id) {
                continue;
            }
            kin.insert(
                media_id,
                Kin {
                    title: row.text("title")?,
                    work_id,
                },
            );
        }
    }
    Ok(kin)
}

/// How many works match, ignoring the page.
///
/// The same predicate as [`search`], because a result count that counted
/// something else would be worse than no count at all.
pub async fn count_matching(db: &Db, q: &Query) -> Result<i64> {
    let mut sql = format!(
        "SELECT COUNT(*) AS n FROM {} WHERE 1 = 1",
        from_clause(db, q)
    );
    let mut args = AnyArguments::default();
    narrow(db.dialect(), q, &mut sql, &mut args)?;

    let row = sqlx::query_with(db.sql(&sql), args)
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

/// Append the filters a [`Query`] asks for to a statement being built.
pub(super) fn narrow(
    dialect: crate::db::Dialect,
    q: &Query,
    sql: &mut String,
    args: &mut AnyArguments,
) -> Result<()> {
    if !q.include_disabled {
        sql.push_str(" AND is_enabled = 1");
    }

    if let Some(kind) = q.kind {
        sql.push_str(" AND kind = ?");
        args.add(kind.as_str().to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(year) = q.year {
        // The providers' year, not the one shown: Radarr's search passes the
        // year it was given, and a lock must not change what it is answered.
        sql.push_str(" AND year = ?");
        args.add(i64::from(year))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if q.manual_only {
        sql.push_str(" AND is_manual = 1");
    }

    // A combined genre asked for — from a series' own page — is its parts,
    // as the works are listed by them: "Action & Adventure" is a work that is
    // both. Or the combined name itself, as a work not yet listed again since
    // the change still holds it. Each part once, however often it is asked.
    let mut asked = std::collections::HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for genre in q.genres.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
        let parts: Vec<String> = crate::domain::genre_parts(genre)
            .into_iter()
            // All of them: a part asked twice is asked once. Any of them:
            // each genre stands on its own, parts shared or not.
            .filter(|p| q.genre_any || asked.insert(p.clone()))
            .collect();
        if parts.is_empty() {
            continue;
        }
        let each =
            vec!["REPLACE(listed_genres, ?, '') <> listed_genres"; parts.len()].join(" AND ");
        if parts.len() > 1 {
            clauses.push(format!(
                "(REPLACE(listed_genres, ?, '') <> listed_genres OR ({each}))"
            ));
            args.add(json_element(genre))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        } else {
            clauses.push(each);
        }
        for part in &parts {
            args.add(json_element(part))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    if !clauses.is_empty() {
        let joined = clauses.join(if q.genre_any { " OR " } else { " AND " });
        sql.push_str(&format!(" AND ({joined})"));
    }

    if let Some(keyword) = q
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        sql.push_str(" AND REPLACE(listed_keywords, ?, '') <> listed_keywords");
        args.add(json_element(keyword))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(from) = q.year_from {
        sql.push_str(" AND listed_year >= ?");
        args.add(i64::from(from))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(to) = q.year_to {
        sql.push_str(" AND listed_year <= ?");
        args.add(i64::from(to))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        sql.push_str(" AND listed_status = ?");
        args.add(status.to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    // One language, or several the catalogue spells differently for the same
    // tongue — `ja` from TMDB, `jpn` from TheTVDB — asked for together.
    let languages: Vec<&str> = q
        .original_language
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if !languages.is_empty() {
        sql.push_str(" AND listed_language IN (");
        for (i, language) in languages.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push('?');
            args.add(language.to_string())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        sql.push(')');
    }

    // Both engines fold with their own `LOWER`, on both sides. They agree for
    // ASCII, which is what a link from a work's page or the filter list sends
    // back exactly anyway; for an accented capital typed by hand only
    // PostgreSQL folds, and SQLite asks for the letters as given.
    if let Some(network) = q
        .network
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        sql.push_str(" AND (LOWER(listed_network) = LOWER(?) OR LOWER(listed_studio) = LOWER(?))");
        for _ in 0..2 {
            args.add(network.to_string())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }

    if let Some(collection) = q.collection {
        sql.push_str(" AND collection_tmdb_id = ?");
        args.add(collection).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(min) = q.min_rating.filter(|m| *m > 0.0) {
        sql.push_str(" AND listed_score >= ?");
        args.add(min).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if q.refresh_failed {
        sql.push_str(" AND refresh_error IS NOT NULL");
    }

    // Each gap narrows further: asked for a poster and an overview, the list
    // is of the works lacking both. Before the text search, which ends the
    // statement early on SQLite.
    for gap in &q.missing {
        let (condition, binds) = gap.condition();
        sql.push_str(" AND ");
        sql.push_str(condition);
        // Empty, it matches the locks in every language alone.
        let stored = q.stored_language.clone().unwrap_or_default();
        for _ in 0..binds {
            args.add(stored.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }

    if !q.include_adult {
        sql.push_str(" AND is_adult = 0");
    }

    if let Some(term) = q.term.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        // SQLite: the trigram index over every title the work goes by, folded
        // of case and accents — a substring match like `LIKE`'s, read off an
        // index. A trigram needs three characters; shorter, `LIKE` it is.
        // PostgreSQL runs the `LIKE`s below, which pg_trgm indexes as they
        // are written.
        if dialect == crate::db::Dialect::Sqlite && term.chars().count() >= 3 {
            sql.push_str(
                " AND id IN (SELECT media_id FROM media_search WHERE media_search MATCH ?)",
            );
            args.add(fts_phrase(term))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            return Ok(());
        }
        // Matched as typed: `%` and `_` are characters a title can hold, not
        // wildcards. `_` found every work, and `a_c` found `abc`.
        let pattern = format!("%{}%", like_escaped(&term.to_lowercase()));

        // The stored title is the provider's. Someone who renamed a work will
        // look for it by the name they gave it, so overrides are searched too:
        // the value is JSON, but a substring match over the encoded string finds
        // it either way.
        // The slug as well, and it is the one that does the real work for
        // anything not written in ASCII. `LOWER` is the engines' own, and they
        // disagree: PostgreSQL folds `Été` to `été`, SQLite folds only ASCII and
        // leaves it as `Été` — so a search for `été` found the work on one
        // engine and not the other. The slug was transliterated when the work
        // was stored, so `ete` matches `Été` on both. A term with no letter or
        // digit in it has no slug, and is not looked for in one: the slug of
        // nothing was `untitled`, and found every work named so.
        let slugged = slug::slugify(term);
        sql.push_str(
            r" AND (LOWER(title) LIKE ? ESCAPE '\'
                   OR LOWER(COALESCE(sort_title, '')) LIKE ? ESCAPE '\'
                   OR id IN (SELECT media_id FROM media_alternative_title
                             WHERE LOWER(title) LIKE ? ESCAPE '\')
                   OR id IN (SELECT media_id FROM media_override
                             WHERE field IN ('title', 'sortTitle', 'originalTitle')
                               AND LOWER(COALESCE(value, '')) LIKE ? ESCAPE '\')",
        );
        for _ in 0..4 {
            args.add(pattern.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        if !slugged.is_empty() {
            sql.push_str(r" OR slug LIKE ? ESCAPE '\'");
            args.add(format!("%{}%", like_escaped(&slugged)))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        sql.push(')');
    }

    Ok(())
}

/// Text as a `LIKE` pattern matches it, whatever it holds: the wildcards
/// `%` and `_`, and `\` that escapes them (`ESCAPE '\'`), each escaped.
fn like_escaped(text: &str) -> String {
    text.replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

/// A term as one FTS5 phrase: quoted, so nothing in it is read as syntax,
/// and matched whole — every trigram of it, in order.
fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

pub async fn count(db: &Db, kind: Option<MediaKind>) -> Result<i64> {
    let row = match kind {
        Some(k) => {
            sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_item WHERE kind = ?"))
                .bind(k.as_str())
                .fetch_one(db.pool())
                .await?
        }
        None => {
            sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_item"))
                .fetch_one(db.pool())
                .await?
        }
    };

    Ok(row.big("n")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        repo::item::{
            ItemWrite, facets,
            testing::{sample, stored},
            upsert,
        },
        testing::{db, sqlite},
    };

    /// The plan SQLite chooses for a list query, as `search` would build it.
    async fn plan(db: &Db, q: &Query) -> String {
        let mut sql = format!(
            "EXPLAIN QUERY PLAN SELECT id FROM {} WHERE 1 = 1",
            from_clause(db, q)
        );
        let mut args = AnyArguments::default();
        narrow(db.dialect(), q, &mut sql, &mut args).expect("narrowed");
        sql.push_str(" ORDER BY ");
        sql.push_str(&order_clause(q));
        sql.push_str(" LIMIT 36");

        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await
            .expect("explained");

        rows.iter()
            .map(|r| r.text("detail").expect("a plan line"))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[tokio::test]
    async fn a_list_is_read_in_index_order_and_a_text_search_is_not() {
        // The whole point of migration 0007, pinned: a page of one kind comes
        // straight off `ix_media_item_browse` with no sort, a page of every kind
        // off `ix_media_item_popular` — and a text search reads the table,
        // because walking either index for it measured four times slower on a
        // term that matched nothing.
        //
        // SQLite's, whatever engine the other tests run on: the plan is read
        // with its own `EXPLAIN QUERY PLAN`, and means nothing to PostgreSQL's.
        let db = sqlite().await;

        let one_kind = plan(
            &db,
            &Query {
                kind: Some(MediaKind::Movie),
                limit: 36,
                ..Default::default()
            },
        )
        .await;
        assert!(one_kind.contains("ix_media_item_browse"), "{one_kind}");
        assert!(
            !one_kind.contains("TEMP B-TREE"),
            "sorted anyway: {one_kind}"
        );

        let every_kind = plan(
            &db,
            &Query {
                include_disabled: true,
                limit: 60,
                ..Default::default()
            },
        )
        .await;
        assert!(every_kind.contains("ix_media_item_popular"), "{every_kind}");

        let text = plan(
            &db,
            &Query {
                term: Some("breaking".into()),
                limit: 36,
                ..Default::default()
            },
        )
        .await;
        assert!(
            !text.contains("USING INDEX"),
            "a text search walked an index: {text}"
        );
    }

    #[tokio::test]
    async fn an_accented_title_is_found_by_typing_it_either_way() {
        // `LOWER` is the engine's own and the two disagree: PostgreSQL folds
        // `É` to `é`, SQLite folds ASCII only. The slug was transliterated when
        // the work was stored, so it answers the same on both.
        let db = db().await;

        let mut item = sample();
        item.id = crate::db::new_id();
        item.title = "Été 85".into();
        item.slug = crate::domain::make_slug(&item.title, item.year);

        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: false,
            },
        )
        .await
        .expect("stored");

        for term in ["Été", "été", "ete", "ETE"] {
            let found = search(
                &db,
                &Query {
                    term: Some(term.to_string()),
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .expect("searched");

            assert_eq!(found.len(), 1, "{term:?} should have found it");
        }
    }

    #[tokio::test]
    async fn a_work_is_found_by_the_name_someone_gave_it() {
        // Renaming a work and then not being able to find it is the kind of gap
        // that makes an editor distrust the whole thing.
        let db = db().await;
        let item = sample();

        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: false,
            },
        )
        .await
        .unwrap();

        crate::db::repo::override_field::set(
            &db,
            &item.id,
            crate::domain::fields::Scope::Item,
            "title",
            Some(&serde_json::json!("Renamed By Hand")),
            None,
        )
        .await
        .unwrap();

        let found = |term: &str| {
            let db = &db;
            let term = term.to_string();
            async move {
                search(
                    db,
                    &Query {
                        term: Some(term),
                        limit: 10,
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
                .len()
            }
        };

        assert_eq!(
            found("Round Trip").await,
            1,
            "the provider's title still matches"
        );
        assert_eq!(
            found("Renamed By Hand").await,
            1,
            "and so does the edited one"
        );
        assert_eq!(found("renamed by hand").await, 1, "case does not matter");
        assert_eq!(found("Something Else").await, 0);
    }

    async fn titles(db: &Db, q: Query) -> Vec<String> {
        search(db, &Query { limit: 50, ..q })
            .await
            .expect("searched")
            .into_iter()
            .map(|i| i.title)
            .collect()
    }

    fn rated(source: &str, value: f64, votes: i64) -> Rating {
        Rating {
            source: source.into(),
            value: Some(value),
            votes: Some(votes),
            rating_type: Some("user".into()),
        }
    }

    #[tokio::test]
    async fn every_genre_asked_for_must_be_there_and_whole() {
        let db = db().await;
        stored(&db, |i| {
            i.title = "Both".into();
            i.genres = vec!["Drama".into(), "Crime".into()];
        })
        .await;
        stored(&db, |i| {
            i.title = "Docudrama".into();
            i.genres = vec!["Docudrama".into()];
        })
        .await;
        stored(&db, |i| {
            i.title = "Drama only".into();
            i.genres = vec!["Drama".into()];
        })
        .await;
        stored(&db, |i| {
            i.title = "Crime only".into();
            i.genres = vec!["Crime".into()];
        })
        .await;

        let mut drama = titles(
            &db,
            Query {
                genres: vec!["Drama".into()],
                sort: Sort::Title,
                ..Default::default()
            },
        )
        .await;
        drama.sort();
        assert_eq!(
            drama,
            ["Both", "Drama only"],
            "a genre is matched whole, not as part of another"
        );

        // As PostgreSQL has always answered: SQLite's LIKE folded the case and
        // found both, so the two engines disagreed about the same link.
        assert!(
            titles(
                &db,
                Query {
                    genres: vec!["drama".into()],
                    ..Default::default()
                },
            )
            .await
            .is_empty(),
            "a genre is matched as the list offers it, case and all"
        );

        let both = titles(
            &db,
            Query {
                genres: vec!["Drama".into(), "Crime".into()],
                ..Default::default()
            },
        )
        .await;
        assert_eq!(both, ["Both"]);

        // Any of them: what carries one or the other, the way a mood browses.
        let mut either = titles(
            &db,
            Query {
                genres: vec!["Drama".into(), "Crime".into()],
                genre_any: true,
                ..Default::default()
            },
        )
        .await;
        either.sort();
        assert_eq!(either, ["Both", "Crime only", "Drama only"]);
    }

    #[tokio::test]
    async fn a_network_is_found_whatever_its_case_and_a_studio_with_it() {
        let db = db().await;
        stored(&db, |i| {
            i.title = "On AMC".into();
            i.network = Some("AMC".into());
        })
        .await;
        stored(&db, |i| {
            i.title = "By Wit".into();
            i.studio = Some("Wit Studio".into());
        })
        .await;

        assert_eq!(
            titles(
                &db,
                Query {
                    network: Some("amc".into()),
                    ..Default::default()
                }
            )
            .await,
            ["On AMC"]
        );
        assert_eq!(
            titles(
                &db,
                Query {
                    network: Some("WIT STUDIO".into()),
                    ..Default::default()
                }
            )
            .await,
            ["By Wit"]
        );
    }

    #[tokio::test]
    async fn years_status_and_language_narrow_the_list() {
        let db = db().await;
        for (title, year, status, language) in [
            ("Old", 1999, "ended", "en"),
            ("Mid", 2008, "continuing", "ja"),
            ("New", 2020, "ended", "en"),
        ] {
            stored(&db, |i| {
                i.title = title.into();
                i.year = Some(year);
                i.status = Some(status.into());
                i.original_language = Some(language.into());
            })
            .await;
        }

        assert_eq!(
            titles(
                &db,
                Query {
                    year_from: Some(2000),
                    year_to: Some(2010),
                    ..Default::default()
                }
            )
            .await,
            ["Mid"]
        );
        assert_eq!(
            titles(
                &db,
                Query {
                    status: Some("ended".into()),
                    sort: Sort::Title,
                    ..Default::default()
                }
            )
            .await,
            ["New", "Old"]
        );
        assert_eq!(
            titles(
                &db,
                Query {
                    original_language: Some("ja".into()),
                    ..Default::default()
                }
            )
            .await,
            ["Mid"]
        );
    }

    #[tokio::test]
    async fn the_score_is_imdbs_or_else_the_most_voted_one() {
        let db = db().await;
        stored(&db, |i| {
            i.title = "With IMDb".into();
            i.ratings = vec![rated("tmdb", 7.0, 50_000), rated("imdb", 9.1, 700_000)];
        })
        .await;
        stored(&db, |i| {
            i.title = "Without".into();
            // MyAnimeList has more votes, so its 6.0 is the score, not TMDB's 8.0.
            i.ratings = vec![rated("tmdb", 8.0, 100), rated("mal", 6.0, 1_000)];
        })
        .await;
        stored(&db, |i| {
            i.title = "Unrated".into();
            // Not a mark out of ten: TheTVDB's old popularity figure.
            i.ratings = vec![rated("tvdb", 3_776_757.0, 0)];
        })
        .await;

        assert_eq!(
            titles(
                &db,
                Query {
                    min_rating: Some(7.0),
                    ..Default::default()
                }
            )
            .await,
            ["With IMDb"]
        );

        let best_first = titles(
            &db,
            Query {
                sort: Sort::Rating,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            best_first,
            ["With IMDb", "Without", "Unrated"],
            "no score goes last"
        );

        let worst_first = titles(
            &db,
            Query {
                sort: Sort::Rating,
                descending: Some(false),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            worst_first,
            ["Without", "With IMDb", "Unrated"],
            "and last either way"
        );
    }

    #[tokio::test]
    async fn titles_sort_without_regard_to_case_and_dates_newest_first() {
        let db = db().await;
        for (title, date) in [
            ("beta", "2001-05-01"),
            ("Alpha", "2019-01-01"),
            ("Gamma", "1994-09-22"),
        ] {
            stored(&db, |i| {
                i.title = title.into();
                i.in_cinemas = Some(date.into());
                i.year = date.get(..4).and_then(|y| y.parse().ok());
            })
            .await;
        }

        assert_eq!(
            titles(
                &db,
                Query {
                    sort: Sort::Title,
                    ..Default::default()
                }
            )
            .await,
            ["Alpha", "beta", "Gamma"]
        );
        assert_eq!(
            titles(
                &db,
                Query {
                    sort: Sort::Release,
                    ..Default::default()
                }
            )
            .await,
            ["Alpha", "beta", "Gamma"]
        );
        assert_eq!(
            titles(
                &db,
                Query {
                    sort: Sort::Release,
                    descending: Some(false),
                    ..Default::default()
                }
            )
            .await,
            ["Gamma", "beta", "Alpha"]
        );
    }

    #[tokio::test]
    async fn a_series_genre_and_the_film_genres_it_stands_for_are_one_list() {
        let db = db().await;
        stored(&db, |i| {
            i.kind = MediaKind::Series;
            i.title = "Series".into();
            i.genres = vec!["Action & Adventure".into(), "Drama".into()];
        })
        .await;
        stored(&db, |i| {
            i.kind = MediaKind::Movie;
            i.title = "Action film".into();
            i.genres = vec!["Action".into()];
        })
        .await;
        stored(&db, |i| {
            i.kind = MediaKind::Movie;
            i.title = "Adventure film".into();
            i.genres = vec!["Action".into(), "Adventure".into()];
        })
        .await;

        let by = |genres: &[&str]| Query {
            genres: genres.iter().map(|g| g.to_string()).collect(),
            sort: Sort::Title,
            ..Default::default()
        };
        assert_eq!(
            titles(&db, by(&["Action"])).await,
            ["Action film", "Adventure film", "Series"]
        );
        // Asked for by its own name, from the series' page: works that are both.
        assert_eq!(
            titles(&db, by(&["Action & Adventure"])).await,
            ["Adventure film", "Series"]
        );

        let facets = facets(&db, &Query::default()).await.expect("counted");
        let named: Vec<&str> = facets.genres.iter().map(|f| f.value.as_str()).collect();
        assert!(named.contains(&"Adventure") && !named.contains(&"Action & Adventure"));
    }

    #[tokio::test]
    async fn a_combined_genre_is_found_on_a_work_not_yet_listed_again() {
        let db = db().await;
        let old = stored(&db, |i| {
            i.kind = MediaKind::Series;
            i.title = "Listed before the change".into();
            i.genres = vec!["Action & Adventure".into()];
        })
        .await;
        // As the previous version listed it: the combined name, whole.
        sqlx::query(db.sql("UPDATE media_item SET listed_genres = ? WHERE id = ?"))
            .bind(r#"["Action & Adventure"]"#)
            .bind(&old.id)
            .execute(db.pool())
            .await
            .unwrap();

        let query = Query {
            genres: vec!["Action & Adventure".into(), "Action".into()],
            ..Default::default()
        };
        // Asked for by its own name — and for a part it names as well, which
        // is one condition, not two.
        assert_eq!(titles(&db, query).await, ["Listed before the change"]);
    }

    #[tokio::test]
    async fn a_language_spelt_two_ways_is_asked_for_by_both() {
        let db = db().await;
        stored(&db, |i| {
            i.title = "Short".into();
            i.original_language = Some("ja".into());
        })
        .await;
        stored(&db, |i| {
            i.title = "Long".into();
            i.original_language = Some("jpn".into());
        })
        .await;
        stored(&db, |i| {
            i.title = "Other".into();
            i.original_language = Some("en".into());
        })
        .await;

        let mut both = titles(
            &db,
            Query {
                original_language: Some("ja, jpn".into()),
                ..Default::default()
            },
        )
        .await;
        both.sort_unstable();
        assert_eq!(both, ["Long", "Short"], "either spelling lists the work");

        assert_eq!(
            titles(
                &db,
                Query {
                    original_language: Some("jpn".into()),
                    ..Default::default()
                }
            )
            .await,
            ["Long"]
        );
    }

    #[tokio::test]
    async fn scores_are_bucketed_by_their_whole_part() {
        // A seven point nine is a seven, whichever engine counts it: the
        // CAST alone would round it to eight on PostgreSQL.
        let db = db().await;
        for (n, score) in [(1, 7.9), (2, 7.2), (3, 8.0), (4, 6.5)] {
            let mut item = sample();
            item.id = crate::db::new_id();
            item.slug = format!("scored-{n}");
            item.external_ids = ExternalIds {
                tmdb: Some(n),
                ..Default::default()
            };
            upsert(
                &db,
                ItemWrite {
                    item: &item,
                    replace_children: false,
                },
            )
            .await
            .expect("stored");
            sqlx::query(db.sql("UPDATE media_item SET listed_score = ? WHERE id = ?"))
                .bind(score)
                .bind(&item.id)
                .execute(db.pool())
                .await
                .expect("scored");
        }

        let buckets = score_buckets(&db, true).await.expect("counted");
        assert_eq!(buckets, vec![(6, 1), (7, 2), (8, 1)]);
    }

    #[tokio::test]
    async fn the_episodes_of_a_work_switched_off_are_counted_only_when_asked() {
        let db = db().await;
        let item = sample();
        let episodes = item.episodes.len() as i64;
        assert!(episodes > 0, "the sample carries episodes");
        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        assert_eq!(episode_count(&db, true, false).await.unwrap(), episodes);

        sqlx::query(db.sql("UPDATE media_item SET is_enabled = 0 WHERE id = ?"))
            .bind(&item.id)
            .execute(db.pool())
            .await
            .expect("switched off");
        assert_eq!(episode_count(&db, true, false).await.unwrap(), 0);
        assert_eq!(episode_count(&db, true, true).await.unwrap(), episodes);
    }

    #[test]
    fn a_term_is_escaped_into_a_pattern_that_matches_it_as_typed() {
        assert_eq!(like_escaped("100%"), r"100\%");
        assert_eq!(like_escaped("a_c"), r"a\_c");
        assert_eq!(like_escaped(r"back\slash"), r"back\\slash");
        assert_eq!(like_escaped(r"\%_"), r"\\\%\_");
        assert_eq!(like_escaped("plain"), "plain");
    }

    #[tokio::test]
    async fn a_wildcard_in_a_term_is_a_character_not_a_wildcard() {
        let db = db().await;
        stored(&db, |i| i.title = "50% Off".into()).await;
        stored(&db, |i| i.title = "snake_case".into()).await;
        stored(&db, |i| i.title = "Abc".into()).await;
        stored(&db, |i| i.title = "Untitled".into()).await;

        let found = |term: &str| {
            titles(
                &db,
                Query {
                    term: Some(term.into()),
                    ..Default::default()
                },
            )
        };

        // Terms shorter than a trigram take the `LIKE` path on SQLite, as
        // every term does on PostgreSQL. `%` and `_` found every work.
        assert_eq!(found("%").await, ["50% Off"]);
        assert_eq!(found("_").await, ["snake_case"]);
        assert_eq!(found("%%").await, Vec::<String>::new());
        assert_eq!(found(r"\").await, Vec::<String>::new());
        // A term with nothing to make a slug of is not looked for in the
        // slugs: the slug of nothing was `untitled`.
        assert_eq!(found("!").await, Vec::<String>::new());
        // An ordinary short term still matches as it did.
        assert_eq!(found("ab").await, ["Abc"]);
    }
}
