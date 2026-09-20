//! Reads and writes for works and their children.
//!
//! Writes distinguish **provider-sourced** rows from **manual** ones: a refresh
//! deletes and re-inserts only `is_manual = 0` children, so anything a human
//! added by hand survives every refresh. Manual *edits* to provider rows live in
//! `media_override` and are applied on read, never written back here.

use anyhow::{Context, Result};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};

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
    collection_tmdb_id, is_manual, is_enabled, created_at, updated_at,
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
    let rows = sqlx::query(db.sql(
        "SELECT id, season_number, episode_number, absolute_episode_number,
                aired_after_season_number, aired_before_season_number,
                aired_before_episode_number, title, overview, air_date, air_date_utc,
                runtime, finale_type, image, tvdb_id, tmdb_id, rating_value,
                rating_count, is_manual
         FROM media_episode WHERE media_id = ?
         ORDER BY season_number, episode_number",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
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
        })
        .collect()
}

async fn load_images(db: &Db, media_id: &str) -> Result<Vec<Image>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, season_number, cover_type, url, language, sort_order, source, is_manual
         FROM media_image WHERE media_id = ? ORDER BY cover_type, sort_order",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let mut images: Vec<Image> = rows
        .iter()
        .map(|row| {
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
        })
        .collect::<Result<_>>()?;

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

    rows.iter()
        .map(|row| {
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
        })
        .collect()
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

    rows.iter()
        .map(|row| {
            Ok(Rating {
                source: row.text("source")?,
                value: row.opt_real("value")?,
                votes: row.opt_big("votes")?,
                rating_type: row.opt_text("rating_type")?,
            })
        })
        .collect()
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

#[derive(Debug, Default)]
pub struct Query {
    pub term: Option<String>,
    pub kind: Option<MediaKind>,
    pub year: Option<i32>,
    pub manual_only: bool,
    pub include_disabled: bool,
    pub limit: i64,
    pub offset: i64,
}

/// Shallow search over the canonical store.
///
/// Matching is a case-insensitive substring over the title, the sort title and
/// every alternative title, which is what clients searching by a localised name
/// need. Anything more (ranking, typo tolerance) belongs in a later FTS index.
pub async fn search(db: &Db, q: &Query) -> Result<Vec<MediaItem>> {
    let mut sql = format!("SELECT {ITEM_COLUMNS} FROM media_item WHERE 1 = 1");
    let mut args = AnyArguments::default();

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

    if let Some(term) = q.term.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let pattern = format!("%{}%", term.to_lowercase());

        // The stored title is the provider's. Someone who renamed a work will
        // look for it by the name they gave it, so overrides are searched too:
        // the value is JSON, but a substring match over the encoded string finds
        // it either way.
        sql.push_str(
            " AND (LOWER(title) LIKE ? OR LOWER(COALESCE(sort_title, '')) LIKE ?
                   OR id IN (SELECT media_id FROM media_alternative_title
                             WHERE LOWER(title) LIKE ?)
                   OR id IN (SELECT media_id FROM media_override
                             WHERE field IN ('title', 'sortTitle', 'originalTitle')
                               AND LOWER(COALESCE(value, '')) LIKE ?))",
        );

        for _ in 0..4 {
            args.add(pattern.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }

    sql.push_str(" ORDER BY popularity DESC NULLS LAST, title ASC LIMIT ? OFFSET ?");
    args.add(q.limit.clamp(1, 500))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    args.add(q.offset.max(0))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut item = map_item(row)?;
        item.external_ids = load_external_ids(db, &item.id).await?;
        items.push(item);
    }

    Ok(items)
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

// ─── writes ──────────────────────────────────────────────────────────────────

/// Insert or update a work and, optionally, its provider-sourced children.
pub async fn upsert(db: &Db, write: ItemWrite<'_>) -> Result<()> {
    let mut tx = db.pool().begin().await?;

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
            popularity, genres, keywords, collection_tmdb_id, is_manual, is_enabled,
            created_at, updated_at, refreshed_at, refresh_after, refresh_error
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
}
