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
    /// Works crediting somebody: by their TMDB id, given as digits, or else
    /// by their name, whatever its case and accents.
    pub person: Option<String>,
    /// Works that run at least, or at most, this many minutes: a film's
    /// length, a series' episodes'.
    pub runtime_min: Option<i32>,
    pub runtime_max: Option<i32>,
    /// Works made in this country, ISO 3166-1 alpha-2: `FR`.
    pub country: Option<String>,
    /// Works rated so in [`Self::region`], as that country writes its
    /// ratings: `12`, `PG-13`. Without a region, the rating a work carries
    /// as its own.
    pub certification: Option<String>,
    /// The country a rating is read in: the reader's.
    pub region: Option<String>,
    /// The works a misspelt term was taken for, beside those its letters
    /// find: `None` until [`tolerate`] has looked, so that a list and its
    /// count, or its facets, answer for the same works. A caller that wants
    /// the term's letters alone settles it as none, an empty list.
    pub tolerance: Option<Vec<String>>,
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
    /// How well each work answers the term, best first: see `crate::search`.
    /// Without a term, the same as popularity.
    Relevance,
    Rating,
    Release,
    Title,
    Added,
    Refreshed,
    /// A film's length, a series' episodes'.
    Runtime,
    /// When a series last aired; a film has no such date and comes last.
    LastAired,
    /// How many episodes a series holds, specials aside.
    Episodes,
}

impl std::str::FromStr for Sort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "popularity" => Ok(Self::Popularity),
            "relevance" => Ok(Self::Relevance),
            "rating" => Ok(Self::Rating),
            "release" => Ok(Self::Release),
            "title" => Ok(Self::Title),
            "added" => Ok(Self::Added),
            "refreshed" => Ok(Self::Refreshed),
            "runtime" => Ok(Self::Runtime),
            "lastAired" => Ok(Self::LastAired),
            "episodes" => Ok(Self::Episodes),
            other => Err(format!(
                "unknown sort {other:?}: expected popularity, relevance, rating, release, title, \
                 added, refreshed, runtime, lastAired or episodes"
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
    /// A lock a source moved under since it was set, and which says
    /// something else than the lock now: see `repo::change::DIVERGING_LOCK`.
    DivergingLock,
}

impl Missing {
    pub const ALL: [Missing; 5] = [
        Self::Poster,
        Self::Overview,
        Self::Imdb,
        Self::Refresh,
        Self::DivergingLock,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Overview => "overview",
            Self::Imdb => "imdb",
            Self::Refresh => "refresh",
            Self::DivergingLock => "divergingLock",
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
            // The dashboard's count is the same words, so the two agree.
            Self::DivergingLock => crate::db::repo::change::DIVERGING_LOCK,
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
            .ok_or_else(|| {
                format!(
                    "unknown gap {s:?}: expected poster, overview, imdb, refresh or divergingLock"
                )
            })
    }
}

/// The most rows one call will return, whatever it was asked for.
///
/// A caller that wants more pages through `offset`. It is public because a
/// caller that means to read everything needs to know where the ceiling is —
/// the NFO export asked for ten thousand, was quietly given five hundred, and
/// reported a complete run over a fifth of the library.
pub const MAX_PAGE: i64 = 500;

/// The works a query lists, a page of them.
///
/// With a term, and in no other order than how well each answers it
/// ([`Sort::Relevance`]), the works are found and ranked by `crate::search`:
/// a few hundred names found by the term in the search index, put in order,
/// the works they name read under the query's other filters. Any further
/// down the list come in popularity order, so that every work the term
/// matches is still reachable a page at a time. Otherwise the term narrows
/// the list like any filter, and the list is in the order asked for.
pub async fn search(db: &Db, q: &Query) -> Result<Vec<MediaItem>> {
    match relevance_term(q) {
        Some(term) => {
            let finds = find_works(db, q, &term).await?;
            ranked(db, q, &term, &finds).await
        }
        None => listed(db, q).await,
    }
}

/// A page of the works a query lists, and how many it lists in all: one look
/// in the search index for both, so that the count is of the works the page
/// was drawn from.
pub async fn search_counted(db: &Db, q: &Query) -> Result<(Vec<MediaItem>, i64)> {
    let Some(term) = term_of(q) else {
        return Ok((listed(db, q).await?, count_matching(db, q).await?));
    };
    let mut q = q.clone();
    let items = if q.sort == Sort::Relevance {
        let finds = find_works(db, &q, &term).await?;
        if q.tolerance.is_none() {
            q.tolerance = Some(works_of(&finds.resembling));
        }
        ranked(db, &q, &term, &finds).await?
    } else {
        tolerate(db, &mut q).await?;
        listed(db, &q).await?
    };
    let total = count_matching(db, &q).await?;
    Ok((items, total))
}

/// Settle which works a misspelt term is taken for, once, before a list, its
/// count or its facets are read with the query: none for a query without a
/// term, or one already settled. The search index is glanced at, not read
/// through: a term its letters find in many works forgives nothing, and is
/// known to at the cost of a few rows.
pub async fn tolerate(db: &Db, q: &mut Query) -> Result<()> {
    if q.tolerance.is_some() {
        return Ok(());
    }
    if let Some(term) = term_of(q) {
        let rows = crate::search::forgiven(db, &term, &works_scope(q)).await?;
        q.tolerance = Some(works_of(&rows));
    }
    Ok(())
}

/// The term a query searches by, folded, where it has one.
fn term_of(q: &Query) -> Option<crate::search::Term> {
    q.term
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(crate::search::Term::parse)
}

/// The term, where the list is to be in the order of how well each work
/// answers it.
fn relevance_term(q: &Query) -> Option<crate::search::Term> {
    (q.sort == Sort::Relevance).then(|| term_of(q)).flatten()
}

/// The most names each look in the search index brings back: enough that
/// the work a reader means is among them, few enough to rank at once.
const POOL: i64 = 200;

/// Where the search index is looked in for a query's term: works, of the
/// query's kind, as far as it may show them.
fn works_scope(q: &Query) -> crate::search::Scope {
    crate::search::Scope {
        kind: crate::search::Kind::Work,
        media_kind: q.kind,
        include_disabled: q.include_disabled,
        include_adult: q.include_adult,
        limit: POOL,
        // Settled already — as none, by a caller that wants the letters
        // alone — it is not looked for again.
        forgive: q.tolerance.is_none(),
    }
}

/// What the search index finds of works for a query's term.
async fn find_works(
    db: &Db,
    q: &Query,
    term: &crate::search::Term,
) -> Result<crate::search::Finds> {
    crate::search::find(db, term, &works_scope(q)).await
}

/// The works some rows name, each once, in the rows' order.
fn works_of(rows: &[crate::search::Found]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for row in rows {
        let id = crate::search::subject(row);
        if !ids.iter().any(|seen| seen == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// A page of the works found for a term, in the order of how well each
/// answers it: the works the search index brought, read under the query's
/// other filters and ranked, then — past them — the rest of what the term
/// matches, most popular first.
async fn ranked(
    db: &Db,
    q: &Query,
    term: &crate::search::Term,
    finds: &crate::search::Finds,
) -> Result<Vec<MediaItem>> {
    let mut pool: Vec<String> = Vec::new();
    for row in finds.all() {
        let id = crate::search::subject(row);
        if !pool.iter().any(|seen| seen == id) {
            pool.push(id.to_string());
        }
    }

    // The works found, as the rest of the query narrows them.
    let mut found: Vec<MediaItem> = Vec::new();
    if !pool.is_empty() {
        let others = Query {
            term: None,
            tolerance: None,
            ..q.clone()
        };
        let mut sql = format!(
            "SELECT {ITEM_COLUMNS} FROM media_item WHERE id IN ({})",
            vec!["?"; pool.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for id in &pool {
            args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        narrowed(db.dialect(), &others, true, &mut sql, &mut args)?;
        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?;
        for row in &rows {
            found.push(map_item(row)?);
        }
    }

    // Each work by the best of its names. The title a work is shown under
    // counts for a little more than its other names, and what it is known by
    // is its row's, not the copy the index holds.
    let by_id: HashMap<String, (Option<i32>, Option<f64>, String)> = found
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                (item.year, item.popularity, crate::search::norm(&item.title)),
            )
        })
        .collect();
    let order = crate::search::best_first(
        term,
        finds
            .all()
            .filter(|row| by_id.contains_key(crate::search::subject(row))),
        |row| {
            let (year, popularity, title) = &by_id[crate::search::subject(row)];
            (*year, *popularity, &row.norm == title)
        },
    );
    let rank: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(at, (id, _))| (id.as_str(), at))
        .collect();
    found.retain(|item| rank.contains_key(item.id.as_str()));
    found.sort_by_key(|item| rank[item.id.as_str()]);

    let limit = q.limit.clamp(1, MAX_PAGE) as usize;
    let offset = q.offset.max(0) as usize;
    let ranked_count = found.len();
    let mut page: Vec<MediaItem> = found.into_iter().skip(offset).take(limit).collect();

    // Past the works ranked: the rest of what the term matches, which the
    // index was not asked for, most popular first — when there is a rest:
    // reads that stopped short of their limit brought every name it matches.
    if page.len() < limit && !finds.complete {
        let mut sql = format!("SELECT {ITEM_COLUMNS} FROM media_item WHERE 1 = 1");
        let mut args = AnyArguments::default();
        let settled = Query {
            tolerance: Some(
                q.tolerance
                    .clone()
                    .unwrap_or_else(|| works_of(&finds.resembling)),
            ),
            ..q.clone()
        };
        narrow(db.dialect(), &settled, &mut sql, &mut args)?;
        let ranked_ids: Vec<&str> = rank.keys().copied().collect();
        if !ranked_ids.is_empty() {
            sql.push_str(&format!(
                " AND id NOT IN ({})",
                vec!["?"; ranked_ids.len()].join(", ")
            ));
            for id in &ranked_ids {
                args.add(id.to_string())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
        sql.push_str(" ORDER BY (popularity + 0) DESC NULLS LAST, title ASC LIMIT ? OFFSET ?");
        args.add((limit - page.len()) as i64)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        args.add(offset.saturating_sub(ranked_count) as i64)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?;
        for row in &rows {
            page.push(map_item(row)?);
        }
    }

    attach_external_ids(db, &mut page).await?;
    Ok(page)
}

/// A page of the works a query lists, in the order it asks for.
async fn listed(db: &Db, q: &Query) -> Result<Vec<MediaItem>> {
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

/// Whether a query searches text.
fn searches_text(q: &Query) -> bool {
    q.term.as_deref().is_some_and(|t| !t.trim().is_empty())
}

/// The table to read.
///
/// A text search was once a `LIKE '%term%'` over every row, and read the
/// table straight through (`NOT INDEXED`) rather than let SQLite walk an
/// index of the filter columns for it. The term is now a set of works the
/// search index names — `id IN (…)` — which is best read by the key, as
/// either engine does unprompted.
pub(super) fn from_clause(_db: &Db, _q: &Query) -> &'static str {
    "media_item"
}

/// The order every list is in.
///
/// For a text search, by an expression rather than the column: `popularity + 0`
/// sorts the same way, and no index can produce it. Otherwise PostgreSQL walks
/// `ix_media_item_popular` in order, betting that enough rows match early to
/// stop after thirty-six — a bet that loses badly for a term matching nothing,
/// 38 ms becoming 59 at fifty thousand works. With nothing to walk it reads
/// what the term matched and sorts it, which is what a text search costs anyway.
///
/// Anything but popularity is sorted rather than walked: nothing is indexed
/// by score or date, and for a catalogue's worth of rows that costs little.
/// Missing values go last whichever way the list runs, so a work with no date
/// is not presented as the oldest, nor a film among the series that ran
/// longest.
fn order_clause(q: &Query) -> String {
    let (expression, descending) = match q.sort {
        Sort::Popularity | Sort::Relevance if searches_text(q) => ("(popularity + 0)", true),
        Sort::Popularity | Sort::Relevance => ("popularity", true),
        Sort::Rating => ("listed_score", true),
        Sort::Release => ("listed_release", true),
        Sort::Title => ("listed_title", false),
        Sort::Added => ("created_at", true),
        Sort::Refreshed => ("refreshed_at", true),
        Sort::Runtime => ("runtime", true),
        Sort::LastAired => ("last_aired", true),
        // Counted off `ix_episode_media`, one work at a time: the specials
        // are not what "how many episodes" asks about.
        Sort::Episodes => (
            "(SELECT COUNT(*) FROM media_episode e
               WHERE e.media_id = media_item.id AND e.season_number > 0)",
            true,
        ),
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

/// The countries the works a query lists were made in, with how many works
/// each — counted as if the query named no country, as the other facets
/// count a choice that replaces the one made — most common first.
pub async fn country_facets(db: &Db, q: &Query) -> Result<Vec<super::Facet>> {
    let others = Query {
        country: None,
        ..q.clone()
    };
    let mut sql = format!(
        "SELECT production_countries, original_country FROM {} WHERE 1 = 1",
        from_clause(db, &others)
    );
    let mut args = AnyArguments::default();
    narrow(db.dialect(), &others, &mut sql, &mut args)?;
    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let mut counts: HashMap<String, i64> = HashMap::new();
    for row in &rows {
        let mut made: Vec<String> = row.text_list("production_countries")?;
        made.extend(row.opt_text("original_country")?);
        let mut seen = std::collections::HashSet::new();
        for country in made {
            let country = country.trim().to_ascii_uppercase();
            if country.len() == 2 && seen.insert(country.clone()) {
                *counts.entry(country).or_default() += 1;
            }
        }
    }
    let mut facets: Vec<super::Facet> = counts
        .into_iter()
        .map(|(value, count)| super::Facet { value, count })
        .collect();
    facets.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    Ok(facets)
}

/// The ratings the works a query lists carry in its region — or as their own,
/// without one — with how many works each, counted as if the query named no
/// rating. In the order the region's ratings run where they are ages, `12`
/// before `16`; most common first otherwise.
pub async fn certification_facets(db: &Db, q: &Query) -> Result<Vec<super::Facet>> {
    let others = Query {
        certification: None,
        ..q.clone()
    };
    // The works the list holds without its rating, read once: a common
    // table, kept for the statement, where a region names it twice — a
    // term's match in the search index is not run once for each.
    let mut listed = format!("SELECT id FROM {} WHERE 1 = 1", from_clause(db, &others));
    let mut args = AnyArguments::default();
    narrow(db.dialect(), &others, &mut listed, &mut args)?;

    let region = q.region.as_deref().map(str::trim).filter(|r| !r.is_empty());
    let sql = match region {
        Some(region) => {
            for _ in 0..2 {
                args.add(region.to_string())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            format!(
                "WITH listed AS MATERIALIZED ({listed})
                 SELECT media_id AS id, certification AS rating FROM media_release
                  WHERE country = ? AND certification IS NOT NULL AND certification <> ''
                    AND media_id IN (SELECT id FROM listed)
                 UNION
                 SELECT id, content_rating AS rating FROM media_item
                  WHERE content_rating_country = ? AND content_rating IS NOT NULL
                    AND content_rating <> '' AND id IN (SELECT id FROM listed)"
            )
        }
        None => format!(
            "SELECT id, content_rating AS rating FROM media_item
              WHERE content_rating IS NOT NULL AND content_rating <> '' AND id IN ({listed})"
        ),
    };
    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let mut counts: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    for row in &rows {
        let rating = row.text("rating")?.trim().to_string();
        if !rating.is_empty() {
            counts.entry(rating).or_default().insert(row.text("id")?);
        }
    }
    let mut facets: Vec<super::Facet> = counts
        .into_iter()
        .map(|(value, works)| super::Facet {
            value,
            count: works.len() as i64,
        })
        .collect();
    let ages = facets.iter().all(|f| age(&f.value).is_some());
    facets.sort_by(|a, b| {
        if ages {
            age(&a.value)
                .cmp(&age(&b.value))
                .then_with(|| a.value.cmp(&b.value))
        } else {
            b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value))
        }
    });
    Ok(facets)
}

/// The age a rating names, where it is one: `12`, `16+`, `-12`, `FSK 16`.
fn age(rating: &str) -> Option<u32> {
    let digits: String = rating.chars().filter(char::is_ascii_digit).collect();
    let rest = rating
        .chars()
        .filter(|c| !c.is_ascii_digit() && !matches!(c, '+' | '-' | ' '))
        .count();
    (rest <= 3 && !digits.is_empty())
        .then(|| digits.parse().ok())
        .flatten()
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
    let named = term_of(q).is_some();
    narrowed(dialect, q, named, sql, args)
}

/// [`narrow`], told whether the statement's works are named already — by a
/// term's match in the search index, or a list of ids the statement holds.
///
/// Named, SQLite is kept from reading them through an index of their kind or
/// of what is shown, by a unary `+` before those columns. With no figures on
/// what the columns hold, its planner takes `kind = 'series'` for a handful
/// of rows, and read the twenty thousand series of a fifty-thousand-work
/// catalogue to keep the few a term named: 40 ms for a count the works' own
/// keys answer in one, every page and every facet of a search of series.
/// PostgreSQL keeps figures, and has no `+` for text.
fn narrowed(
    dialect: crate::db::Dialect,
    q: &Query,
    named: bool,
    sql: &mut String,
    args: &mut AnyArguments,
) -> Result<()> {
    let plain = if named && dialect == crate::db::Dialect::Sqlite {
        "+"
    } else {
        ""
    };

    if !q.include_disabled {
        sql.push_str(&format!(" AND {plain}is_enabled = 1"));
    }

    if let Some(kind) = q.kind {
        sql.push_str(&format!(" AND {plain}kind = ?"));
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

    // Somebody by their TMDB id, or by their name as the search index folds
    // it — so `wim wenders` and `Wim Wenders` are one person, and so is
    // `Pedro Almodovar` without his accent — on either engine alike. The
    // index's people's rows say which ids and which names that is.
    if let Some(person) = q.person.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        if let Ok(tmdb_id) = person.parse::<i64>() {
            sql.push_str(" AND id IN (SELECT media_id FROM media_credit WHERE tmdb_person_id = ?)");
            args.add(tmdb_id).map_err(|e| anyhow::anyhow!("{e}"))?;
        } else {
            sql.push_str(
                " AND id IN (SELECT c.media_id FROM media_credit c
                             WHERE c.tmdb_person_id IN (
                                       SELECT CAST(substr(d.ref_id, 6) AS BIGINT) FROM search_doc d
                                        WHERE d.kind = 'person' AND d.norm = ?
                                          AND substr(d.ref_id, 1, 5) = 'tmdb:')
                                OR (c.tmdb_person_id IS NULL AND c.person_name IN (
                                       SELECT substr(d.ref_id, 6) FROM search_doc d
                                        WHERE d.kind = 'person' AND d.norm = ?
                                          AND substr(d.ref_id, 1, 5) = 'name:')))",
            );
            let folded = crate::search::norm(person);
            for _ in 0..2 {
                args.add(folded.clone())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
    }

    if let Some(least) = q.runtime_min {
        sql.push_str(" AND runtime >= ?");
        args.add(i64::from(least))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    if let Some(most) = q.runtime_max {
        sql.push_str(" AND runtime <= ?");
        args.add(i64::from(most))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    // Among the countries it was made in, as the genres are matched: whole,
    // inside the JSON list. A series filed under its country of origin alone
    // is found by that.
    if let Some(country) = q
        .country
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        let country = country.to_ascii_uppercase();
        sql.push_str(
            " AND (REPLACE(production_countries, ?, '') <> production_countries
                   OR original_country = ?)",
        );
        args.add(json_element(&country))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        args.add(country).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    // The rating a country gives the work: one of its releases there, or the
    // rating the work carries as its own when that is the country's. Without
    // a country, the work's own.
    if let Some(rating) = q
        .certification
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        match q.region.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            Some(region) => {
                sql.push_str(
                    " AND (id IN (SELECT media_id FROM media_release
                                  WHERE country = ? AND certification = ?)
                           OR (content_rating = ? AND content_rating_country = ?))",
                );
                for value in [region, rating, rating, region] {
                    args.add(value.to_string())
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
            None => {
                sql.push_str(" AND content_rating = ?");
                args.add(rating.to_string())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
    }

    if let Some(min) = q.min_rating.filter(|m| *m > 0.0) {
        sql.push_str(" AND listed_score >= ?");
        args.add(min).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if q.refresh_failed {
        sql.push_str(" AND refresh_error IS NOT NULL");
    }

    // Each gap narrows further: asked for a poster and an overview, the list
    // is of the works lacking both.
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
        sql.push_str(&format!(" AND {plain}is_adult = 0"));
    }

    // The works the search index finds by the term's letters — or, read as a
    // title and a year, by the title's — and those a misspelt term was taken
    // for, once that is settled (see `tolerate`).
    if let Some(term) = q.term.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let term = crate::search::Term::parse(term);
        sql.push_str(" AND (");
        crate::search::candidates::push_works(dialect, &term.forms(), sql, args)?;
        if let Some(taken) = q.tolerance.as_ref().filter(|ids| !ids.is_empty()) {
            sql.push_str(&format!(
                " OR id IN ({})",
                vec!["?"; taken.len()].join(", ")
            ));
            for id in taken {
                args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
        sql.push(')');
    }

    Ok(())
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
    async fn a_list_is_read_in_index_order_and_a_text_search_from_the_search_index() {
        // The whole point of migration 0007, pinned: a page of one kind comes
        // straight off `ix_media_item_browse` with no sort, a page of every kind
        // off `ix_media_item_popular` — and a text search reads neither, since
        // walking either for it measured four times slower on a term that
        // matched nothing: it reads the works the search index names, by
        // their key.
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
            text.contains("VIRTUAL TABLE") && text.contains("(id=?)"),
            "a text search did not start from the search index: {text}"
        );
        assert!(
            !text.contains("ix_media_item_browse") && !text.contains("ix_media_item_popular"),
            "a text search walked an index of the list: {text}"
        );

        // Of one kind too. Told nothing of what `kind` holds, SQLite took
        // `kind = 'series'` for a handful of rows, and read every series
        // through an index of their kind to keep the few the term named.
        let text_of_a_kind = plan(
            &db,
            &Query {
                term: Some("breaking".into()),
                kind: Some(MediaKind::Series),
                limit: 36,
                ..Default::default()
            },
        )
        .await;
        assert!(
            text_of_a_kind.contains("VIRTUAL TABLE") && text_of_a_kind.contains("(id=?)"),
            "a text search of series did not start from the search index: {text_of_a_kind}"
        );
        assert!(
            !text_of_a_kind.contains("(kind=?"),
            "a text search of series read the series by their kind: {text_of_a_kind}"
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
        // The lock marks the work, and the server writes its rows of the
        // index again within seconds; here, at once.
        crate::db::repo::search::catch_up(&db, 10).await.unwrap();

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

    #[tokio::test]
    async fn a_lock_its_sources_moved_under_is_a_gap_the_register_lists() {
        let db = db().await;
        let locked = stored(&db, |i| i.title = "Locked".into()).await;
        stored(&db, |i| i.title = "Untouched".into()).await;
        crate::db::repo::override_field::set(
            &db,
            &locked.id,
            crate::domain::fields::Scope::Item,
            "overview",
            Some(&serde_json::json!("Written by hand.")),
            None,
        )
        .await
        .expect("locked");
        // A refresh since the lock, whose source says something else now.
        sqlx::query(db.sql(
            "INSERT INTO media_change
                 (id, media_id, at, cause, scope, field, language, value_before, value_after,
                  was_locked)
             VALUES (?, ?, '2999-01-01T00:00:00.000Z', 'refresh', 'item', 'overview', '',
                     ?, ?, 1)",
        ))
        .bind(crate::db::new_id())
        .bind(&locked.id)
        .bind(r#""As it was.""#)
        .bind(r#""As the source has it now.""#)
        .execute(db.pool())
        .await
        .expect("journaled");

        let query = Query {
            missing: vec!["divergingLock".parse().expect("a gap")],
            include_disabled: true,
            include_adult: true,
            ..Default::default()
        };
        assert_eq!(titles(&db, query.clone()).await, ["Locked"]);
        assert_eq!(
            count_matching(&db, &query).await.expect("counted"),
            crate::db::repo::change::count_diverging(&db, true)
                .await
                .expect("counted"),
            "the register and the dashboard count the same works"
        );
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

        // A term is folded before it is looked for, its punctuation a space:
        // `%` and `_` are no wildcards, and a term of punctuation alone is
        // nothing to look for — it finds nothing, where it once found every
        // work.
        assert_eq!(found("%").await, Vec::<String>::new());
        assert_eq!(found("_").await, Vec::<String>::new());
        assert_eq!(found("%%").await, Vec::<String>::new());
        assert_eq!(found(r"\").await, Vec::<String>::new());
        assert_eq!(found("!").await, Vec::<String>::new());
        assert_eq!(found("a_c").await, Vec::<String>::new());
        // Written as it is stored, a title is found.
        assert_eq!(found("50%").await, ["50% Off"]);
        assert_eq!(found("snake_case").await, ["snake_case"]);
        // An ordinary short term still matches as it did.
        assert_eq!(found("ab").await, ["Abc"]);
    }
}
