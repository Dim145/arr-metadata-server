//! Reads and writes for works and their children.
//!
//! Writes distinguish **provider-sourced** rows from **manual** ones: a refresh
//! deletes and re-inserts only `is_manual = 0` children, so anything a human
//! added by hand survives every refresh. Manual *edits* to provider rows live in
//! `media_override` and are applied on read, never written back here.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};
use utoipa::ToSchema;

use crate::{
    db::{Db, RowExt, from_bool, new_id, now, text_list},
    domain::{
        AlternativeTitle, CoverType, Credit, CreditType, Episode, ExternalIds, ExternalSource,
        Image, MediaItem, MediaKind, Rating, RatingValue, Season, Translation,
    },
};

/// Everything written for one work in a single transaction.
pub struct ItemWrite<'a> {
    pub item: &'a MediaItem,
    /// When true, existing provider-sourced children are replaced.
    pub replace_children: bool,
}

// ─── reads ───────────────────────────────────────────────────────────────────

const ITEM_COLUMNS: &str = "
    id, kind, slug, title, sort_title, original_title, overview, status,
    original_language, original_country, runtime, year, first_aired, last_aired,
    in_cinemas, physical_release, digital_release, air_time, network, studio,
    content_rating, content_rating_country, homepage, trailer_youtube_id, popularity, genres, keywords,
    collection_tmdb_id, is_manual, is_enabled, is_adult, created_at, updated_at,
    refreshed_at, refresh_after, refresh_error
";

fn map_item(row: &sqlx::any::AnyRow) -> Result<MediaItem> {
    let kind: MediaKind = row.text("kind")?.parse()?;

    Ok(MediaItem {
        id: row.text("id")?,
        kind,
        slug: row.text("slug")?,
        title: row.text("title")?,
        sort_title: row.opt_text("sort_title")?,
        original_title: row.opt_text("original_title")?,
        overview: row.opt_text("overview")?,
        status: row.opt_text("status")?,
        original_language: row.opt_text("original_language")?,
        original_country: row.opt_text("original_country")?,
        runtime: row.opt_int("runtime")?,
        year: row.opt_int("year")?,
        first_aired: row.opt_text("first_aired")?,
        last_aired: row.opt_text("last_aired")?,
        in_cinemas: row.opt_text("in_cinemas")?,
        physical_release: row.opt_text("physical_release")?,
        digital_release: row.opt_text("digital_release")?,
        air_time: row.opt_text("air_time")?,
        network: row.opt_text("network")?,
        studio: row.opt_text("studio")?,
        content_rating: row.opt_text("content_rating")?,
        content_rating_country: row.opt_text("content_rating_country")?,
        homepage: row.opt_text("homepage")?,
        trailer_youtube_id: row.opt_text("trailer_youtube_id")?,
        popularity: row.opt_real("popularity")?,
        collection_tmdb_id: row.opt_big("collection_tmdb_id")?,
        genres: row.text_list("genres")?,
        keywords: row.text_list("keywords")?,
        external_ids: ExternalIds::default(),
        is_manual: row.flag("is_manual")?,
        is_adult: row.flag("is_adult").unwrap_or(false),
        is_enabled: row.flag("is_enabled")?,
        created_at: row.text("created_at")?,
        updated_at: row.text("updated_at")?,
        refreshed_at: row.opt_text("refreshed_at")?,
        refresh_after: row.opt_text("refresh_after")?,
        refresh_error: row.opt_text("refresh_error")?,
        seasons: Vec::new(),
        episodes: Vec::new(),
        images: Vec::new(),
        credits: Vec::new(),
        alternative_titles: Vec::new(),
        ratings: Vec::new(),
        translations: Vec::new(),
        locked_fields: Vec::new(),
    })
}

/// One work, without children. External ids are always loaded: they are what
/// every compatibility surface keys on.
pub async fn get(db: &Db, id: &str) -> Result<Option<MediaItem>> {
    let sql = format!("SELECT {ITEM_COLUMNS} FROM media_item WHERE id = ?");

    let row = sqlx::query(db.sql(&sql))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    let Some(row) = row else { return Ok(None) };
    let mut item = map_item(&row)?;
    item.external_ids = load_external_ids(db, id).await?;

    Ok(Some(item))
}

pub async fn load_external_ids(db: &Db, media_id: &str) -> Result<ExternalIds> {
    let rows = sqlx::query(
        db.sql("SELECT source, value FROM media_external_id WHERE media_id = ? ORDER BY source"),
    )
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let mut ids = ExternalIds::default();
    for row in &rows {
        let raw = row.text("source")?;
        match raw.parse::<ExternalSource>() {
            Ok(source) => ids.apply(source, &row.text("value")?),
            // The CHECK constraint makes this unreachable today; if a future
            // migration adds a source this build does not know, skip it.
            Err(e) => tracing::warn!(error = %e, "ignoring unknown external id source"),
        }
    }

    Ok(ids)
}

/// Resolve a work by one of its external identifiers.
pub async fn find_id_by_external(
    db: &Db,
    source: ExternalSource,
    value: &str,
) -> Result<Option<String>> {
    let row = sqlx::query(
        db.sql("SELECT media_id FROM media_external_id WHERE source = ? AND value = ?"),
    )
    .bind(source.as_str())
    .bind(value)
    .fetch_optional(db.pool())
    .await?;

    row.map(|r| r.text("media_id"))
        .transpose()
        .map_err(Into::into)
}

pub async fn find_id_by_slug(db: &Db, kind: MediaKind, slug: &str) -> Result<Option<String>> {
    let row = sqlx::query(db.sql("SELECT id FROM media_item WHERE kind = ? AND slug = ?"))
        .bind(kind.as_str())
        .bind(slug)
        .fetch_optional(db.pool())
        .await?;

    row.map(|r| r.text("id")).transpose().map_err(Into::into)
}

/// Load every child collection onto `item`.
pub async fn load_children(db: &Db, item: &mut MediaItem) -> Result<()> {
    let (seasons, episodes, images, credits, alt_titles, ratings, translations) = tokio::try_join!(
        load_seasons(db, &item.id),
        load_episodes(db, &item.id),
        load_images(db, &item.id),
        load_credits(db, &item.id),
        load_alternative_titles(db, &item.id),
        load_ratings(db, &item.id),
        load_translations(db, &item.id),
    )?;

    // Season-scoped images belong on their season, not on the work.
    let (season_images, item_images): (Vec<Image>, Vec<Image>) =
        images.into_iter().partition(|i| i.season_number.is_some());

    let mut seasons = seasons;
    for season in &mut seasons {
        season.images = season_images
            .iter()
            .filter(|i| i.season_number == Some(season.season_number))
            .cloned()
            .collect();
    }

    item.seasons = seasons;
    item.episodes = episodes;
    item.images = item_images;
    item.credits = credits;
    item.alternative_titles = alt_titles;
    item.ratings = ratings;
    item.translations = translations;

    Ok(())
}

async fn load_seasons(db: &Db, media_id: &str) -> Result<Vec<Season>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, season_number, title, overview, air_date, tmdb_id, tvdb_id, is_manual
         FROM media_season WHERE media_id = ? ORDER BY season_number",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(Season {
                id: row.text("id")?,
                season_number: row.int("season_number")?,
                title: row.opt_text("title")?,
                overview: row.opt_text("overview")?,
                air_date: row.opt_text("air_date")?,
                tmdb_id: row.opt_big("tmdb_id")?,
                tvdb_id: row.opt_big("tvdb_id")?,
                is_manual: row.flag("is_manual")?,
                images: Vec::new(),
            })
        })
        .collect()
}

async fn load_episodes(db: &Db, media_id: &str) -> Result<Vec<Episode>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {EPISODE_COLUMNS} FROM media_episode WHERE media_id = ?
         ORDER BY season_number, episode_number"
    )))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(map_episode).collect()
}

/// The columns [`map_episode`] reads, for a query that selects episodes.
const EPISODE_COLUMNS: &str = "id, season_number, episode_number, absolute_episode_number,
    aired_after_season_number, aired_before_season_number, aired_before_episode_number,
    title, overview, air_date, air_date_utc, runtime, finale_type, image, tvdb_id, tmdb_id,
    rating_value, rating_count, is_manual";

fn map_episode(row: &sqlx::any::AnyRow) -> Result<Episode> {
    let rating = match (row.opt_real("rating_value")?, row.opt_big("rating_count")?) {
        (Some(value), votes) => Some(RatingValue {
            value,
            votes: votes.unwrap_or(0),
        }),
        _ => None,
    };

    Ok(Episode {
        id: row.text("id")?,
        season_number: row.int("season_number")?,
        episode_number: row.int("episode_number")?,
        absolute_episode_number: row.opt_int("absolute_episode_number")?,
        aired_after_season_number: row.opt_int("aired_after_season_number")?,
        aired_before_season_number: row.opt_int("aired_before_season_number")?,
        aired_before_episode_number: row.opt_int("aired_before_episode_number")?,
        title: row.text("title")?,
        overview: row.opt_text("overview")?,
        air_date: row.opt_text("air_date")?,
        air_date_utc: row.opt_text("air_date_utc")?,
        runtime: row.opt_int("runtime")?,
        finale_type: row.opt_text("finale_type")?,
        image: row.opt_text("image")?,
        tvdb_id: row.opt_big("tvdb_id")?,
        tmdb_id: row.opt_big("tmdb_id")?,
        rating,
        is_manual: row.flag("is_manual")?,
    })
}

async fn load_images(db: &Db, media_id: &str) -> Result<Vec<Image>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, season_number, cover_type, url, language, sort_order, source, is_manual
         FROM media_image WHERE media_id = ? ORDER BY cover_type, sort_order",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let mut images: Vec<Image> = rows.iter().map(map_image).collect::<Result<_>>()?;

    // SQL orders by the cover type's *name*, which puts clearlogo before poster.
    // Sort by meaning instead.
    images.sort_by_key(|i| (i.cover_type.priority(), i.sort_order));

    Ok(images)
}

async fn load_credits(db: &Db, media_id: &str) -> Result<Vec<Credit>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, credit_type, person_name, character_name, image,
                tmdb_person_id, credit_tmdb_id, sort_order, is_manual
         FROM media_credit WHERE media_id = ? ORDER BY credit_type, sort_order",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(map_credit).collect()
}

fn map_credit(row: &sqlx::any::AnyRow) -> Result<Credit> {
    Ok(Credit {
        id: row.text("id")?,
        credit_type: row
            .text("credit_type")?
            .parse::<CreditType>()
            .unwrap_or(CreditType::Actor),
        person_name: row.text("person_name")?,
        character_name: row.opt_text("character_name")?,
        image: row.opt_text("image")?,
        tmdb_person_id: row.opt_big("tmdb_person_id")?,
        credit_tmdb_id: row.opt_text("credit_tmdb_id")?,
        sort_order: row.int("sort_order")?,
        is_manual: row.flag("is_manual")?,
    })
}

async fn load_alternative_titles(db: &Db, media_id: &str) -> Result<Vec<AlternativeTitle>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, title, title_type, language, is_manual
         FROM media_alternative_title WHERE media_id = ? ORDER BY title",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(AlternativeTitle {
                id: row.text("id")?,
                title: row.text("title")?,
                title_type: row.opt_text("title_type")?,
                language: row.opt_text("language")?,
                is_manual: row.flag("is_manual")?,
            })
        })
        .collect()
}

async fn load_ratings(db: &Db, media_id: &str) -> Result<Vec<Rating>> {
    let rows = sqlx::query(
        db.sql("SELECT source, value, votes, rating_type FROM media_rating WHERE media_id = ?"),
    )
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(map_rating).collect()
}

fn map_image(row: &sqlx::any::AnyRow) -> Result<Image> {
    Ok(Image {
        id: row.text("id")?,
        season_number: row.opt_int("season_number")?,
        cover_type: row
            .text("cover_type")?
            .parse::<CoverType>()
            .unwrap_or(CoverType::Unknown),
        url: row.text("url")?,
        language: row.opt_text("language")?,
        sort_order: row.int("sort_order")?,
        source: row.opt_text("source")?,
        is_manual: row.flag("is_manual")?,
    })
}

fn map_rating(row: &sqlx::any::AnyRow) -> Result<Rating> {
    Ok(Rating {
        source: row.text("source")?,
        value: row.opt_real("value")?,
        votes: row.opt_big("votes")?,
        rating_type: row.opt_text("rating_type")?,
    })
}

async fn load_translations(db: &Db, media_id: &str) -> Result<Vec<Translation>> {
    let rows = sqlx::query(db.sql(
        "SELECT language, title, overview, is_manual FROM media_translation WHERE media_id = ?",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            Ok(Translation {
                language: row.text("language")?,
                title: row.opt_text("title")?,
                overview: row.opt_text("overview")?,
                is_manual: row.flag("is_manual")?,
            })
        })
        .collect()
}

// ─── search & listing ────────────────────────────────────────────────────────

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
    pub keyword: Option<String>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub status: Option<String>,
    pub original_language: Option<String>,
    /// A network or a studio, by name.
    pub network: Option<String>,
    pub collection: Option<i64>,
    /// Works whose score — see [`SCORE`] — is at least this, out of ten.
    pub min_rating: Option<f64>,
    /// Works whose last refresh failed.
    pub refresh_failed: bool,
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

/// A work's score, out of ten, the way the interface and Sonarr read one:
/// IMDb's when there is one, otherwise the rating with the most votes behind
/// it — see `MediaItem::headline_rating`. Figures outside ten are not scores.
const SCORE: &str = "COALESCE(
        (SELECT r.value FROM media_rating r
          WHERE r.media_id = media_item.id AND r.source = 'imdb'
            AND r.value > 0 AND r.value <= 10),
        (SELECT r.value FROM media_rating r
          WHERE r.media_id = media_item.id AND r.value > 0 AND r.value <= 10
          ORDER BY COALESCE(r.votes, 0) DESC LIMIT 1))";

/// When a work first reached the public, as far as it is known: a date, or
/// the year alone.
const RELEASE: &str =
    "COALESCE(first_aired, in_cinemas, digital_release, physical_release, CAST(year AS TEXT))";

/// Shallow search over the canonical store.
///
/// Matching is a case-insensitive substring over the title, the sort title and
/// every alternative title, which is what clients searching by a localised name
/// The most rows one call will return, whatever it was asked for.
///
/// A caller that wants more pages through `offset`. It is public because a
/// caller that means to read everything needs to know where the ceiling is —
/// the NFO export asked for ten thousand, was quietly given five hundred, and
/// reported a complete run over a fifth of the library.
pub const MAX_PAGE: i64 = 500;

/// need. Anything more (ranking, typo tolerance) belongs in a later FTS index.
pub async fn search(db: &Db, q: &Query) -> Result<Vec<MediaItem>> {
    let mut sql = format!(
        "SELECT {ITEM_COLUMNS} FROM {} WHERE 1 = 1",
        from_clause(db, q)
    );
    let mut args = AnyArguments::default();
    narrow(q, &mut sql, &mut args)?;

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
async fn attach_external_ids(db: &Db, items: &mut [MediaItem]) -> Result<()> {
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
fn from_clause(db: &Db, q: &Query) -> &'static str {
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
        Sort::Rating => (SCORE, true),
        Sort::Release => (RELEASE, true),
        Sort::Title => ("LOWER(COALESCE(sort_title, title))", false),
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
        "SELECT media_id, language, title, overview, is_manual
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
                is_manual: row.flag("is_manual")?,
            });
    }

    for item in items {
        let mut own = images.remove(&item.id).unwrap_or_default();
        // SQL ordered by the cover type's name; a card wants the poster first.
        own.sort_by_key(|i| (i.cover_type.priority(), i.sort_order));

        item.images = own;
        item.ratings = ratings.remove(&item.id).unwrap_or_default();
        item.translations = translations.remove(&item.id).unwrap_or_default();
    }

    Ok(())
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
    narrow(q, &mut sql, &mut args)?;

    let row = sqlx::query_with(db.sql(&sql), args)
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

/// Append the filters a [`Query`] asks for to a statement being built.
fn narrow(q: &Query, sql: &mut String, args: &mut AnyArguments) -> Result<()> {
    if !q.include_disabled {
        sql.push_str(" AND is_enabled = 1");
    }

    if let Some(kind) = q.kind {
        sql.push_str(" AND kind = ?");
        args.add(kind.as_str().to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(year) = q.year {
        sql.push_str(" AND year = ?");
        args.add(i64::from(year))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if q.manual_only {
        sql.push_str(" AND is_manual = 1");
    }

    for genre in q.genres.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
        sql.push_str(" AND REPLACE(genres, ?, '') <> genres");
        args.add(json_element(genre))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(keyword) = q
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        sql.push_str(" AND REPLACE(keywords, ?, '') <> keywords");
        args.add(json_element(keyword))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(from) = q.year_from {
        sql.push_str(" AND year >= ?");
        args.add(i64::from(from))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(to) = q.year_to {
        sql.push_str(" AND year <= ?");
        args.add(i64::from(to))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        sql.push_str(" AND status = ?");
        args.add(status.to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if let Some(language) = q.original_language.as_deref().filter(|l| !l.is_empty()) {
        sql.push_str(" AND original_language = ?");
        args.add(language.to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
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
        sql.push_str(" AND (LOWER(network) = LOWER(?) OR LOWER(studio) = LOWER(?))");
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
        sql.push_str(&format!(" AND {SCORE} >= ?"));
        args.add(min).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    if q.refresh_failed {
        sql.push_str(" AND refresh_error IS NOT NULL");
    }

    if !q.include_adult {
        sql.push_str(" AND is_adult = 0");
    }

    if let Some(term) = q.term.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let pattern = format!("%{}%", term.to_lowercase());

        // The stored title is the provider's. Someone who renamed a work will
        // look for it by the name they gave it, so overrides are searched too:
        // the value is JSON, but a substring match over the encoded string finds
        // it either way.
        // The slug as well, and it is the one that does the real work for
        // anything not written in ASCII. `LOWER` is the engines' own, and they
        // disagree: PostgreSQL folds `Été` to `été`, SQLite folds only ASCII and
        // leaves it as `Été` — so a search for `été` found the work on one
        // engine and not the other. The slug was transliterated when the work
        // was stored, so `ete` matches `Été` on both.
        sql.push_str(
            " AND (LOWER(title) LIKE ? OR LOWER(COALESCE(sort_title, '')) LIKE ?
                   OR slug LIKE ?
                   OR id IN (SELECT media_id FROM media_alternative_title
                             WHERE LOWER(title) LIKE ?)
                   OR id IN (SELECT media_id FROM media_override
                             WHERE field IN ('title', 'sortTitle', 'originalTitle')
                               AND LOWER(COALESCE(value, '')) LIKE ?))",
        );

        let slugged = format!("%{}%", crate::domain::make_slug(term, None));

        for pattern in [&pattern, &pattern, &slugged, &pattern, &pattern] {
            args.add(pattern.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
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

// ─── across works ────────────────────────────────────────────────────────────

/// Works by id, with their identifiers and nothing hanging off them — the
/// same shape [`search`] returns, in no particular order.
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
    pub year_min: Option<i32>,
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

    let mut genres: HashMap<String, i64> = HashMap::new();
    for row in &all {
        for genre in row.text_list("genres")? {
            *genres.entry(genre).or_default() += 1;
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
            .map(|(value, count)| Facet { value, count })
            .collect();
        list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        list
    };

    facets.genres = ranked(genres);
    facets.networks = ranked(networks);
    facets.networks.truncate(TOP_NETWORKS);
    facets.languages = ranked(languages);
    facets.statuses = ranked(statuses);

    Ok(facets)
}

/// The columns [`facets`] counts, for the works a query lists.
async fn facet_rows(db: &Db, q: &Query) -> Result<Vec<sqlx::any::AnyRow>> {
    let mut sql = format!(
        "SELECT genres, network, studio, original_language, status, year FROM {} WHERE 1 = 1",
        from_clause(db, q)
    );
    let mut args = AnyArguments::default();
    narrow(q, &mut sql, &mut args)?;

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

/// A mark that moves whenever the catalogue does: how many works it holds and
/// when one last changed. For answers read across works and kept a while.
pub async fn catalogue_stamp(db: &Db) -> Result<String> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n, MAX(updated_at) AS at FROM media_item"))
        .fetch_one(db.pool())
        .await?;

    Ok(format!(
        "{}@{}",
        row.big("n")?,
        row.opt_text("at")?.unwrap_or_default()
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

/// One of a person's credits, on an enabled work.
#[derive(Clone, Debug)]
pub struct PersonCredit {
    pub media_id: String,
    pub credit: Credit,
}

/// Every credit TMDB files under one person, on works this catalogue shows.
pub async fn person_credits(
    db: &Db,
    tmdb_person_id: i64,
    include_adult: bool,
) -> Result<Vec<PersonCredit>> {
    let adult = if include_adult {
        ""
    } else {
        " AND m.is_adult = 0"
    };
    let sql = format!(
        "SELECT c.media_id, c.id, c.credit_type, c.person_name, c.character_name, c.image,
                c.tmdb_person_id, c.credit_tmdb_id, c.sort_order, c.is_manual
         FROM media_credit c JOIN media_item m ON m.id = c.media_id
         WHERE c.tmdb_person_id = ? AND m.is_enabled = 1{adult}
         ORDER BY m.year DESC NULLS LAST, m.title
         LIMIT 500"
    );

    let rows = sqlx::query(db.sql(&sql))
        .bind(tmdb_person_id)
        .fetch_all(db.pool())
        .await?;

    rows.iter()
        .map(|row| {
            Ok(PersonCredit {
                media_id: row.text("media_id")?,
                credit: map_credit(row)?,
            })
        })
        .collect()
}

// ─── writes ──────────────────────────────────────────────────────────────────

/// Insert or update a work and, optionally, its provider-sourced children.
pub async fn upsert(db: &Db, write: ItemWrite<'_>) -> Result<()> {
    let mut tx = db.begin_write().await?;

    upsert_row(db, &mut tx, write.item).await?;
    upsert_external_ids(db, &mut tx, write.item).await?;

    if write.replace_children {
        replace_children(db, &mut tx, write.item).await?;
    }

    tx.commit().await.context("failed to commit item write")?;
    Ok(())
}

/// A slug no other work of this kind is already using.
///
/// Slugs come from the title and year, and two different films genuinely share
/// both — there are several unrelated *Ram (2023)*. The column is unique so a
/// slug addresses one work, which means the collision has to be resolved here
/// rather than rejected: an entry nobody can look up is worth less than one
/// under `ram-2023-2`.
async fn free_slug(db: &Db, tx: &mut Transaction<'_, Any>, item: &MediaItem) -> Result<String> {
    let sql = "SELECT id FROM media_item WHERE kind = ? AND slug = ? LIMIT 1";

    for attempt in 1..=50 {
        let candidate = match attempt {
            1 => item.slug.clone(),
            n => format!("{}-{n}", item.slug),
        };

        let taken: Option<String> = sqlx::query_scalar(db.sql(sql))
            .bind(item.kind.as_str())
            .bind(&candidate)
            .fetch_optional(&mut **tx)
            .await
            .context("failed to check whether a slug was free")?;

        // Ours already, or nobody's.
        if taken.as_deref().is_none_or(|owner| owner == item.id) {
            return Ok(candidate);
        }
    }

    // Fifty works sharing a title and year is not a collision any more.
    Ok(format!("{}-{}", item.slug, &item.id[..8]))
}

async fn upsert_row(db: &Db, tx: &mut Transaction<'_, Any>, item: &MediaItem) -> Result<()> {
    let slug = free_slug(db, tx, item).await?;

    // `created_at` is preserved on conflict; everything else is overwritten.
    let sql = "
        INSERT INTO media_item (
            id, kind, slug, title, sort_title, original_title, overview, status,
            original_language, original_country, runtime, year, first_aired, last_aired,
            in_cinemas, physical_release, digital_release, air_time, network, studio,
            content_rating, content_rating_country, homepage, trailer_youtube_id,
            popularity, genres, keywords, collection_tmdb_id, is_manual, is_enabled, is_adult,
            created_at, updated_at, refreshed_at, refresh_after, refresh_error
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT (id) DO UPDATE SET
            kind = excluded.kind,
            slug = excluded.slug,
            title = excluded.title,
            sort_title = excluded.sort_title,
            original_title = excluded.original_title,
            overview = excluded.overview,
            status = excluded.status,
            original_language = excluded.original_language,
            original_country = excluded.original_country,
            runtime = excluded.runtime,
            year = excluded.year,
            first_aired = excluded.first_aired,
            last_aired = excluded.last_aired,
            in_cinemas = excluded.in_cinemas,
            physical_release = excluded.physical_release,
            digital_release = excluded.digital_release,
            air_time = excluded.air_time,
            network = excluded.network,
            studio = excluded.studio,
            content_rating = excluded.content_rating,
            content_rating_country = excluded.content_rating_country,
            homepage = excluded.homepage,
            trailer_youtube_id = excluded.trailer_youtube_id,
            popularity = excluded.popularity,
            genres = excluded.genres,
            keywords = excluded.keywords,
            collection_tmdb_id = excluded.collection_tmdb_id,
            is_manual = excluded.is_manual,
            is_enabled = excluded.is_enabled,
            is_adult = excluded.is_adult,
            updated_at = excluded.updated_at,
            refreshed_at = excluded.refreshed_at,
            refresh_after = excluded.refresh_after,
            refresh_error = excluded.refresh_error
    ";

    sqlx::query(db.sql(sql))
        .bind(&item.id)
        .bind(item.kind.as_str())
        .bind(&slug)
        .bind(&item.title)
        .bind(&item.sort_title)
        .bind(&item.original_title)
        .bind(&item.overview)
        .bind(&item.status)
        .bind(&item.original_language)
        .bind(&item.original_country)
        .bind(item.runtime)
        .bind(item.year)
        .bind(&item.first_aired)
        .bind(&item.last_aired)
        .bind(&item.in_cinemas)
        .bind(&item.physical_release)
        .bind(&item.digital_release)
        .bind(&item.air_time)
        .bind(&item.network)
        .bind(&item.studio)
        .bind(&item.content_rating)
        .bind(&item.content_rating_country)
        .bind(&item.homepage)
        .bind(&item.trailer_youtube_id)
        .bind(item.popularity)
        .bind(text_list(&item.genres))
        .bind(text_list(&item.keywords))
        .bind(item.collection_tmdb_id)
        .bind(from_bool(item.is_manual))
        .bind(from_bool(item.is_enabled))
        .bind(from_bool(item.is_adult))
        .bind(&item.created_at)
        .bind(&item.updated_at)
        .bind(&item.refreshed_at)
        .bind(&item.refresh_after)
        .bind(&item.refresh_error)
        .execute(&mut **tx)
        .await
        .context("failed to write media_item")?;

    Ok(())
}

async fn upsert_external_ids(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    item: &MediaItem,
) -> Result<()> {
    // Ids this work no longer claims must go, or a stale row keeps resolving to it.
    sqlx::query(db.sql("DELETE FROM media_external_id WHERE media_id = ?"))
        .bind(&item.id)
        .execute(&mut **tx)
        .await?;

    let created = now();

    for (source, value) in item.external_ids.rows(item.kind) {
        // Another work may already claim this id — for instance two TMDB entries
        // sharing an IMDb id. Last writer wins rather than aborting the refresh.
        sqlx::query(db.sql(
            "INSERT INTO media_external_id (media_id, source, value, created_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (source, value) DO UPDATE SET media_id = excluded.media_id",
        ))
        .bind(&item.id)
        .bind(source.as_str())
        .bind(&value)
        .bind(&created)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("failed to write external id {source}={value}"))?;
    }

    Ok(())
}

/// Replace provider-sourced children; manual rows (`is_manual = 1`) are kept.
///
/// This is the refresh path, so it only ever *writes* provider rows: the loops
/// below skip anything flagged manual. Credits, images and alternative titles
/// have no natural key, so re-inserting a manual row read back from the database
/// would duplicate it on every refresh.
///
/// Creating a manual child therefore needs its own write path, which does not
/// exist yet — the native API can only add whole works by hand today.
async fn replace_children(db: &Db, tx: &mut Transaction<'_, Any>, item: &MediaItem) -> Result<()> {
    for table in [
        "media_season",
        "media_episode",
        "media_image",
        "media_credit",
        "media_alternative_title",
        "media_translation",
    ] {
        sqlx::query(db.sql(&format!(
            "DELETE FROM {table} WHERE media_id = ? AND is_manual = 0"
        )))
        .bind(&item.id)
        .execute(&mut **tx)
        .await?;
    }

    // Ratings carry no manual flag: they are wholly provider-derived.
    sqlx::query(db.sql("DELETE FROM media_rating WHERE media_id = ?"))
        .bind(&item.id)
        .execute(&mut **tx)
        .await?;

    let created = now();

    for season in &item.seasons {
        if season.is_manual {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO media_season
                 (id, media_id, season_number, title, overview, air_date, tmdb_id,
                  tvdb_id, is_manual, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)
             ON CONFLICT (media_id, season_number) DO UPDATE SET
                 title = excluded.title,
                 overview = excluded.overview,
                 air_date = excluded.air_date,
                 tmdb_id = excluded.tmdb_id,
                 tvdb_id = excluded.tvdb_id,
                 updated_at = excluded.updated_at",
        ))
        .bind(if season.id.is_empty() {
            new_id()
        } else {
            season.id.clone()
        })
        .bind(&item.id)
        .bind(season.season_number)
        .bind(&season.title)
        .bind(&season.overview)
        .bind(&season.air_date)
        .bind(season.tmdb_id)
        .bind(season.tvdb_id)
        .bind(&created)
        .bind(&created)
        .execute(&mut **tx)
        .await
        .context("failed to write season")?;
    }

    for ep in &item.episodes {
        if ep.is_manual {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO media_episode
                 (id, media_id, season_number, episode_number, absolute_episode_number,
                  aired_after_season_number, aired_before_season_number,
                  aired_before_episode_number, title, overview, air_date, air_date_utc,
                  runtime, finale_type, image, tvdb_id, tmdb_id, rating_value,
                  rating_count, is_manual, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)
             ON CONFLICT (media_id, season_number, episode_number) DO UPDATE SET
                 absolute_episode_number = excluded.absolute_episode_number,
                 aired_after_season_number = excluded.aired_after_season_number,
                 aired_before_season_number = excluded.aired_before_season_number,
                 aired_before_episode_number = excluded.aired_before_episode_number,
                 title = excluded.title,
                 overview = excluded.overview,
                 air_date = excluded.air_date,
                 air_date_utc = excluded.air_date_utc,
                 runtime = excluded.runtime,
                 finale_type = excluded.finale_type,
                 image = excluded.image,
                 tvdb_id = excluded.tvdb_id,
                 tmdb_id = excluded.tmdb_id,
                 rating_value = excluded.rating_value,
                 rating_count = excluded.rating_count,
                 updated_at = excluded.updated_at",
        ))
        .bind(if ep.id.is_empty() {
            new_id()
        } else {
            ep.id.clone()
        })
        .bind(&item.id)
        .bind(ep.season_number)
        .bind(ep.episode_number)
        .bind(ep.absolute_episode_number)
        .bind(ep.aired_after_season_number)
        .bind(ep.aired_before_season_number)
        .bind(ep.aired_before_episode_number)
        .bind(&ep.title)
        .bind(&ep.overview)
        .bind(&ep.air_date)
        .bind(&ep.air_date_utc)
        .bind(ep.runtime)
        .bind(&ep.finale_type)
        .bind(&ep.image)
        .bind(ep.tvdb_id)
        .bind(ep.tmdb_id)
        .bind(ep.rating.map(|r| r.value))
        .bind(ep.rating.map(|r| r.votes))
        .bind(&created)
        .bind(&created)
        .execute(&mut **tx)
        .await
        .context("failed to write episode")?;
    }

    // Season-scoped images are stored alongside item-scoped ones.
    let images = item
        .images
        .iter()
        .chain(item.seasons.iter().flat_map(|s| s.images.iter()));

    for image in images {
        if image.is_manual {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO media_image
                 (id, media_id, season_number, cover_type, url, language, sort_order,
                  source, is_manual, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)
             ON CONFLICT (media_id, cover_type, url) DO NOTHING",
        ))
        .bind(if image.id.is_empty() {
            new_id()
        } else {
            image.id.clone()
        })
        .bind(&item.id)
        .bind(image.season_number)
        .bind(image.cover_type.as_str())
        .bind(&image.url)
        .bind(&image.language)
        .bind(image.sort_order)
        .bind(&image.source)
        .bind(&created)
        .execute(&mut **tx)
        .await
        .context("failed to write image")?;
    }

    for credit in &item.credits {
        if credit.is_manual {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO media_credit
                 (id, media_id, credit_type, person_name, character_name, image,
                  tmdb_person_id, credit_tmdb_id, sort_order, is_manual, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)",
        ))
        .bind(if credit.id.is_empty() {
            new_id()
        } else {
            credit.id.clone()
        })
        .bind(&item.id)
        .bind(credit.credit_type.as_str())
        .bind(&credit.person_name)
        .bind(&credit.character_name)
        .bind(&credit.image)
        .bind(credit.tmdb_person_id)
        .bind(&credit.credit_tmdb_id)
        .bind(credit.sort_order)
        .bind(&created)
        .execute(&mut **tx)
        .await
        .context("failed to write credit")?;
    }

    for alt in &item.alternative_titles {
        if alt.is_manual {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO media_alternative_title
                 (id, media_id, title, title_type, language, is_manual, created_at)
             VALUES (?, ?, ?, ?, ?, 0, ?)
             ON CONFLICT DO NOTHING",
        ))
        .bind(if alt.id.is_empty() {
            new_id()
        } else {
            alt.id.clone()
        })
        .bind(&item.id)
        .bind(&alt.title)
        .bind(&alt.title_type)
        .bind(&alt.language)
        .bind(&created)
        .execute(&mut **tx)
        .await
        .context("failed to write alternative title")?;
    }

    for rating in &item.ratings {
        sqlx::query(db.sql(
            "INSERT INTO media_rating (media_id, source, value, votes, rating_type)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (media_id, source) DO UPDATE SET
                 value = excluded.value,
                 votes = excluded.votes,
                 rating_type = excluded.rating_type",
        ))
        .bind(&item.id)
        .bind(&rating.source)
        .bind(rating.value)
        .bind(rating.votes)
        .bind(&rating.rating_type)
        .execute(&mut **tx)
        .await
        .context("failed to write rating")?;
    }

    for tr in &item.translations {
        if tr.is_manual {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO media_translation (media_id, language, title, overview, is_manual)
             VALUES (?, ?, ?, ?, 0)
             ON CONFLICT (media_id, language) DO UPDATE SET
                 title = excluded.title,
                 overview = excluded.overview",
        ))
        .bind(&item.id)
        .bind(&tr.language)
        .bind(&tr.title)
        .bind(&tr.overview)
        .execute(&mut **tx)
        .await
        .context("failed to write translation")?;
    }

    Ok(())
}

pub async fn delete(db: &Db, id: &str) -> Result<bool> {
    // Every child table cascades from media_item.
    let result = sqlx::query(db.sql("DELETE FROM media_item WHERE id = ?"))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn set_enabled(db: &Db, id: &str, enabled: bool) -> Result<bool> {
    let result =
        sqlx::query(db.sql("UPDATE media_item SET is_enabled = ?, updated_at = ? WHERE id = ?"))
            .bind(from_bool(enabled))
            .bind(now())
            .bind(id)
            .execute(db.pool())
            .await?;

    Ok(result.rows_affected() > 0)
}

// ─── refresh bookkeeping ─────────────────────────────────────────────────────

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

pub async fn mark_refreshed(
    db: &Db,
    id: &str,
    next_refresh: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(db.sql(
        "UPDATE media_item
         SET refreshed_at = ?, refresh_after = ?, refresh_error = ?, updated_at = ?
         WHERE id = ?",
    ))
    .bind(now())
    .bind(next_refresh)
    .bind(error)
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        domain::{CoverType, ExternalIds, RatingValue},
    };

    /// A real database, in memory, with the real migrations applied.
    ///
    /// These queries are built by hand and bound positionally, so a column added
    /// to the list without a matching placeholder compiles cleanly and fails at
    /// runtime — which is how `34 values for 35 columns` reached a running
    /// server. A round trip through an actual engine is what catches that.
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

    fn sample() -> MediaItem {
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

    #[tokio::test]
    async fn writers_to_different_works_wait_for_each_other_rather_than_fail() {
        // What an import of several films at once did: every write opened a
        // deferred transaction, *read* (the slug check) and then wrote. A
        // second writer committing in between left the first holding a stale
        // snapshot, and SQLite refuses that upgrade at once — `database is
        // locked`, a 500 — without consulting the busy timeout at all, since
        // waiting could not make the snapshot fresh again. A real file and a
        // real pool here, because an in-memory database with one connection
        // cannot have two writers.
        let path = std::env::temp_dir().join(format!("ams-writers-{}.db", crate::db::new_id()));
        let db = Db::connect(&config::Database {
            url: format!("sqlite://{}?mode=rwc", path.display()),
            max_connections: 8,
            acquire_timeout: std::time::Duration::from_secs(10),
        })
        .await
        .expect("a file database");
        db.migrate().await.expect("migrations");

        let writes: Vec<_> = (0..16)
            .map(|n| {
                let db = db.clone();
                tokio::spawn(async move {
                    let mut item = sample();
                    item.id = crate::db::new_id();
                    item.title = format!("Work {n}");
                    item.slug = crate::domain::make_slug(&item.title, item.year);
                    item.external_ids = ExternalIds {
                        tmdb: Some(100_000 + n),
                        ..Default::default()
                    };

                    upsert(
                        &db,
                        ItemWrite {
                            item: &item,
                            replace_children: true,
                        },
                    )
                    .await
                })
            })
            .collect();

        let mut failed = Vec::new();
        for write in writes {
            if let Err(e) = write.await.expect("the task ran") {
                failed.push(format!("{e:#}"));
            }
        }

        db.close().await;
        for suffix in ["", "-shm", "-wal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }

        assert!(
            failed.is_empty(),
            "{} of 16 writes failed: {failed:?}",
            failed.len()
        );
    }

    /// The plan SQLite chooses for a list query, as `search` would build it.
    async fn plan(db: &Db, q: &Query) -> String {
        let mut sql = format!(
            "EXPLAIN QUERY PLAN SELECT id FROM {} WHERE 1 = 1",
            from_clause(db, q)
        );
        let mut args = AnyArguments::default();
        narrow(q, &mut sql, &mut args).expect("narrowed");
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
        let db = db().await;

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
    async fn two_works_sharing_a_title_and_year_both_get_stored() {
        // Films do share a title and a year. Before this, the second one made
        // the write fail and the request answer 500.
        let db = db().await;

        let mut first = sample();
        first.id = crate::db::new_id();

        let mut second = sample();
        second.id = crate::db::new_id();
        second.external_ids = ExternalIds {
            tmdb: Some(9999),
            ..Default::default()
        };

        for item in [&first, &second] {
            upsert(
                &db,
                ItemWrite {
                    item,
                    replace_children: false,
                },
            )
            .await
            .expect("both writes succeed");
        }

        let stored = get(&db, &second.id).await.unwrap().expect("second item");
        assert_eq!(
            stored.slug, "round-trip-2026-2",
            "disambiguated, not rejected"
        );

        let kept = get(&db, &first.id).await.unwrap().expect("first item");
        assert_eq!(
            kept.slug, "round-trip-2026",
            "the first keeps the plain one"
        );
    }

    #[tokio::test]
    async fn rewriting_the_same_work_keeps_its_slug() {
        let db = db().await;
        let item = sample();

        for _ in 0..2 {
            upsert(
                &db,
                ItemWrite {
                    item: &item,
                    replace_children: false,
                },
            )
            .await
            .expect("write");
        }

        let stored = get(&db, &item.id).await.unwrap().expect("item");
        assert_eq!(stored.slug, "round-trip-2026", "not bumped by its own row");
    }

    #[tokio::test]
    async fn an_item_survives_a_round_trip_through_the_database() {
        let db = db().await;
        let written = sample();

        upsert(
            &db,
            ItemWrite {
                item: &written,
                replace_children: true,
            },
        )
        .await
        .expect("write");

        let mut read = get(&db, &written.id).await.expect("read").expect("present");
        load_children(&db, &mut read).await.expect("children");

        // Every scalar the insert lists must come back, or a column has drifted
        // out of alignment with its placeholder.
        assert_eq!(read.title, written.title);
        assert_eq!(read.slug, written.slug);
        assert_eq!(read.year, written.year);
        assert_eq!(read.overview, written.overview);
        assert_eq!(read.status, written.status);
        assert_eq!(read.runtime, written.runtime);
        assert_eq!(read.content_rating, written.content_rating);
        assert_eq!(read.content_rating_country, written.content_rating_country);
        assert_eq!(read.in_cinemas, written.in_cinemas);
        assert_eq!(read.genres, written.genres);
        assert_eq!(read.keywords, written.keywords);
        assert_eq!(read.popularity, written.popularity);
        assert_eq!(read.external_ids, written.external_ids);

        assert_eq!(read.credits.len(), 1);
        assert_eq!(read.credits[0].person_name, "A Person");
        assert_eq!(
            read.credits[0].credit_tmdb_id.as_deref(),
            Some("52fe4726c3a36847f812048b")
        );

        assert_eq!(read.images.len(), 1);
        assert_eq!(read.images[0].cover_type, CoverType::Poster);

        assert_eq!(read.episodes.len(), 1);
        assert_eq!(read.episodes[0].title, "One");
        assert_eq!(read.episodes[0].rating.map(|r| r.votes), Some(10));

        assert_eq!(read.ratings.len(), 1);
        assert_eq!(read.ratings[0].value, Some(7.5));
    }

    #[tokio::test]
    async fn an_upsert_keeps_the_identity_of_the_row_it_replaces() {
        let db = db().await;
        let mut item = sample();

        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        item.title = "Round Trip, Revised".into();
        item.runtime = Some(101);
        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(count(&db, Some(MediaKind::Movie)).await.unwrap(), 1);

        let read = get(&db, &item.id).await.unwrap().unwrap();
        assert_eq!(read.title, "Round Trip, Revised");
        assert_eq!(read.runtime, Some(101));
        assert_eq!(read.created_at, item.created_at);
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

    #[tokio::test]
    async fn a_work_is_found_by_any_of_its_external_ids() {
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

        assert_eq!(
            find_id_by_external(&db, ExternalSource::TmdbMovie, "4242")
                .await
                .unwrap(),
            Some(item.id.clone())
        );
        assert_eq!(
            find_id_by_external(&db, ExternalSource::Imdb, "tt4242424")
                .await
                .unwrap(),
            Some(item.id.clone())
        );
        // A movie's TMDB id must not resolve in the series namespace.
        assert_eq!(
            find_id_by_external(&db, ExternalSource::TmdbTv, "4242")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_refresh_replaces_provider_children_but_spares_manual_ones() {
        let db = db().await;
        let item = sample();

        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        // Stand in for a write path that does not exist yet: the refresh loops
        // deliberately skip manual rows, so nothing else can create one. What is
        // under test is that the refresh's DELETE spares them.
        sqlx::query(db.sql(
            "INSERT INTO media_credit
                 (id, media_id, credit_type, person_name, character_name, image,
                  tmdb_person_id, credit_tmdb_id, sort_order, is_manual, created_at)
             VALUES (?, ?, 'actor', 'Added By Hand', NULL, NULL, NULL, NULL, 1, 1, ?)",
        ))
        .bind(new_id())
        .bind(&item.id)
        .bind(now())
        .execute(db.pool())
        .await
        .unwrap();

        // A refresh carries only what the provider returned.
        let mut refreshed = sample();
        refreshed.id = item.id.clone();
        refreshed.credits[0].person_name = "A Person, Renamed Upstream".into();
        upsert(
            &db,
            ItemWrite {
                item: &refreshed,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        let mut read = get(&db, &item.id).await.unwrap().unwrap();
        load_children(&db, &mut read).await.unwrap();

        let names: Vec<&str> = read
            .credits
            .iter()
            .map(|c| c.person_name.as_str())
            .collect();
        assert!(
            names.contains(&"A Person, Renamed Upstream"),
            "provider row replaced"
        );
        assert!(
            names.contains(&"Added By Hand"),
            "manual row survives a refresh"
        );
        assert_eq!(read.credits.len(), 2, "no duplicates");
    }

    // ─── filtering, ordering and reading across works ───────────────────────

    /// A work built from [`sample`], changed by `adjust`, stored.
    async fn stored(db: &Db, adjust: impl FnOnce(&mut MediaItem)) -> MediaItem {
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
                count: 2
            }
        );
        assert!(facets.genres.iter().all(|g| g.value != "Hentai"));
        assert_eq!(facets.networks.len(), 2);
        assert_eq!((facets.year_min, facets.year_max), (Some(2008), Some(2013)));
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
    async fn a_persons_credits_are_found_on_every_work_newest_first() {
        let db = db().await;
        let credit = |character: &str| Credit {
            id: String::new(),
            credit_type: CreditType::Actor,
            person_name: "Bryan Cranston".into(),
            character_name: Some(character.into()),
            image: None,
            tmdb_person_id: Some(17419),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        };

        stored(&db, |i| {
            i.title = "Older".into();
            i.year = Some(1998);
            i.credits = vec![credit("Hal")];
        })
        .await;
        stored(&db, |i| {
            i.title = "Newer".into();
            i.year = Some(2008);
            i.credits = vec![credit("Walter White")];
        })
        .await;

        let found = person_credits(&db, 17419, false).await.expect("read");
        let characters: Vec<_> = found
            .iter()
            .map(|c| c.credit.character_name.as_deref())
            .collect();

        assert_eq!(characters, [Some("Walter White"), Some("Hal")]);
        assert!(
            person_credits(&db, 1, false)
                .await
                .expect("read")
                .is_empty()
        );
    }
}
