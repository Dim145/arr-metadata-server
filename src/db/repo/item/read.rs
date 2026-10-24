//! Reads of a work and what hangs off it: a work by its id, by one of its
//! identifiers elsewhere or by its slug, which works hold which identifiers,
//! and every child a work's page is drawn from. The rows are mapped here for
//! the other parts too, so that a column is read one way wherever it is read.

use anyhow::Result;
use serde::Serialize;
use sqlx::{Arguments, any::AnyArguments};
use utoipa::ToSchema;

use crate::{
    db::{Db, RowExt},
    domain::{
        AlternativeTitle, CoverType, Credit, CreditType, Episode, ExternalIds, ExternalSource,
        Image, MediaItem, MediaKind, Rating, RatingValue, Relation, Season, Translation,
        person::{People, PersonRef},
    },
};

pub(super) const ITEM_COLUMNS: &str = "
    id, kind, slug, title, sort_title, original_title, overview, status,
    original_language, original_country, runtime, year, first_aired, last_aired,
    in_cinemas, physical_release, digital_release, air_time, network, studio,
    content_rating, content_rating_country, homepage, trailer_youtube_id, theme_music, title_qualifier,
    popularity, genres, keywords,
    collection_tmdb_id, is_manual, is_enabled, is_adult, created_at, updated_at,
    refreshed_at, refresh_after, refresh_error,
    tagline, format, production_countries, production_companies
";

pub(super) fn map_item(row: &sqlx::any::AnyRow) -> Result<MediaItem> {
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
        theme_music: row.opt_text("theme_music")?,
        title_qualifier: row.opt_text("title_qualifier")?,
        tagline: row.opt_text("tagline")?,
        format: row.opt_text("format")?,
        popularity: row.opt_real("popularity")?,
        collection_tmdb_id: row.opt_big("collection_tmdb_id")?,
        genres: row.text_list("genres")?,
        keywords: row.text_list("keywords")?,
        production_countries: row.text_list("production_countries")?,
        production_companies: row.text_list("production_companies")?,
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
        relations: Vec::new(),
        releases: Vec::new(),
        people: Default::default(),
        locked_fields: Vec::new(),
        primary_images: Default::default(),
        locked_texts: Vec::new(),
        hidden: Default::default(),
        pictures: Default::default(),
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

/// The series one of whose episodes TheTVDB numbers `tvdb_id`: what the
/// TheTVDB relay patches a single episode's document for.
pub async fn find_id_by_episode_tvdb(db: &Db, tvdb_id: i64) -> Result<Option<String>> {
    let row = sqlx::query(
        db.sql("SELECT media_id FROM media_episode WHERE tvdb_id = ? ORDER BY media_id LIMIT 1"),
    )
    .bind(tvdb_id)
    .fetch_optional(db.pool())
    .await?;
    Ok(match row {
        Some(row) => Some(row.text("media_id")?),
        None => None,
    })
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

/// The TMDB ids of the films held that changed at `since` or after — an
/// instant written as [`now`](crate::db::now) writes them — switched off or not: written
/// again, listed again (which a lock put on, changed or lifted asks for, and
/// IMDb's list rescoring one), or locked since. A lock moves neither
/// `updated_at` nor `refreshed_at`.
pub async fn movies_changed_since(db: &Db, since: &str) -> Result<Vec<i64>> {
    let rows = sqlx::query(db.sql(
        "SELECT DISTINCT e.value FROM media_external_id e
           JOIN media_item m ON m.id = e.media_id
          WHERE e.source = ? AND m.kind = 'movie'
            AND (m.updated_at >= ? OR m.listed_at >= ?
                 OR EXISTS (SELECT 1 FROM media_override o
                             WHERE o.media_id = m.id AND o.updated_at >= ?))",
    ))
    .bind(ExternalSource::TmdbMovie.as_str())
    .bind(since)
    .bind(since)
    .bind(since)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .iter()
        .filter_map(|row| row.text("value").ok()?.parse().ok())
        .collect())
}

/// A work named in passing — by an audit entry, a job — as a reader knows it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkRef {
    pub id: String,
    pub title: String,
    pub kind: MediaKind,
}

/// The title and kind of each of some works still held, by id.
pub async fn titles(db: &Db, ids: &[String]) -> Result<std::collections::HashMap<String, WorkRef>> {
    let mut found = std::collections::HashMap::new();
    for chunk in ids.chunks(400) {
        let mut args = AnyArguments::default();
        for id in chunk {
            args.add(id.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let sql = format!(
            "SELECT id, title, kind FROM media_item WHERE id IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            let id = row.text("id")?;
            found.insert(
                id.clone(),
                WorkRef {
                    id,
                    title: row.text("title")?,
                    kind: row.text("kind")?.parse()?,
                },
            );
        }
    }
    Ok(found)
}

/// Which of some identifiers of one source the store holds a work for.
/// The works holding each of these external ids, by the id's value — one
/// query per four hundred values, not one per value.
pub async fn held_external_ids(
    db: &Db,
    source: ExternalSource,
    values: &[String],
) -> Result<std::collections::HashMap<String, String>> {
    let mut held = std::collections::HashMap::new();
    for chunk in values.chunks(400) {
        let mut args = AnyArguments::default();
        args.add(source.as_str().to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for value in chunk {
            args.add(value.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT value, media_id FROM media_external_id WHERE source = ? AND value IN ({placeholders})"
        );
        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?;
        for row in rows {
            held.insert(row.text("value")?, row.text("media_id")?);
        }
    }
    Ok(held)
}

pub async fn held_externals(
    db: &Db,
    source: ExternalSource,
    values: &[String],
) -> Result<std::collections::HashSet<String>> {
    let mut held = std::collections::HashSet::new();
    for chunk in values.chunks(400) {
        let mut args = AnyArguments::default();
        args.add(source.as_str().to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for value in chunk {
            args.add(value.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let sql = format!(
            "SELECT value FROM media_external_id WHERE source = ? AND value IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            held.insert(row.text("value")?);
        }
    }
    Ok(held)
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
    let (seasons, episodes, images, credits, alt_titles, ratings, translations, relations) = tokio::try_join!(
        load_seasons(db, &item.id),
        load_episodes(db, &item.id),
        load_images(db, &item.id),
        load_credits(db, &item.id),
        load_alternative_titles(db, &item.id),
        load_ratings(db, &item.id),
        load_translations(db, &item.id),
        load_relations(db, &item.id),
    )?;
    item.releases = crate::db::repo::release::load(db, &item.id).await?;

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

    let (credits, people) = credits;
    item.seasons = seasons;
    item.episodes = episodes;
    item.images = item_images;
    item.credits = credits;
    item.people = people;
    item.alternative_titles = alt_titles;
    item.ratings = ratings;
    item.translations = translations;
    item.relations = relations;

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
                primary_images: Default::default(),
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
pub(super) const EPISODE_COLUMNS: &str =
    "id, season_number, episode_number, absolute_episode_number,
    aired_after_season_number, aired_before_season_number, aired_before_episode_number,
    title, overview, air_date, air_date_utc, runtime, finale_type, image, tvdb_id, tmdb_id,
    rating_value, rating_count, is_manual";

pub(super) fn map_episode(row: &sqlx::any::AnyRow) -> Result<Episode> {
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

/// The work held here that a relation's entry is, as a subquery on a row `r`
/// of `media_relation`: the one switched on that carries that id from that
/// source — and, unless adult works are asked for, not one for adults.
pub(super) fn held(include_adult: bool) -> String {
    format!(
        "(SELECT x.media_id FROM media_external_id x
           JOIN media_item w ON w.id = x.media_id
          WHERE x.source = r.source AND x.value = CAST(r.external_id AS TEXT)
            AND w.is_enabled = 1{}
          LIMIT 1)",
        if include_adult {
            ""
        } else {
            " AND w.is_adult = 0"
        }
    )
}

/// Whether the work held here that a relation's entry is, is for adults:
/// the same subquery, answering with the flag rather than the id.
pub(super) const HELD_ADULT: &str = "(SELECT w.is_adult FROM media_external_id x
                            JOIN media_item w ON w.id = x.media_id
                           WHERE x.source = r.source AND x.value = CAST(r.external_id AS TEXT)
                             AND w.is_enabled = 1
                           LIMIT 1)";

/// Every relation of a work, with the work here that each is when the
/// catalogue holds it, whatever it is; an entry for adults — by AniList's
/// flag or by the work held — is marked so, for the reader's policy to keep
/// or drop.
async fn load_relations(db: &Db, media_id: &str) -> Result<Vec<Relation>> {
    let held = held(true);
    let rows = sqlx::query(db.sql(&format!(
        "SELECT r.id, r.relation_type, r.source, r.external_id, r.mal_id, r.title, r.medium,
                r.format, r.year, r.image, r.is_adult, r.sort_order, {held} AS work_id,
                COALESCE({HELD_ADULT}, 0) AS work_is_adult
           FROM media_relation r WHERE r.media_id = ? ORDER BY r.sort_order"
    )))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(map_relation)
        // An entry that is this very work — a film filed under its series,
        // where the series is several entries — is not another work to lead
        // to.
        .filter(|r| !matches!(r, Ok(r) if r.work_id.as_deref() == Some(media_id)))
        .collect()
}

/// The Fan-Kai cut from a work, as relations of it: every production whose
/// wiki page names one of this work's AniList or MyAnimeList entries as its
/// original.
///
/// Found when read, never stored: a work's own relations are AniList's, and a
/// refresh writes them over whole — a Fan-Kai added later would not appear,
/// and one stored beside them would be lost.
pub async fn recuts(db: &Db, media_id: &str, ids: &ExternalIds) -> Result<Vec<Relation>> {
    let marks = |n: usize| vec!["?"; n].join(", ");
    let mut matches = Vec::new();
    let mut values: Vec<i64> = Vec::new();

    if !ids.anilist.is_empty() {
        matches.push(format!(
            "(r.source = 'anilist' AND r.external_id IN ({}))",
            marks(ids.anilist.len())
        ));
        values.extend(&ids.anilist);
    }
    if !ids.mal.is_empty() {
        let list = marks(ids.mal.len());
        matches.push(format!(
            "(r.source = 'mal' AND r.external_id IN ({list})) OR r.mal_id IN ({list})"
        ));
        values.extend(&ids.mal);
        values.extend(&ids.mal);
    }
    if matches.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT DISTINCT r.media_id, w.title, w.year, w.is_adult,
                (SELECT x.value FROM media_external_id x
                  WHERE x.media_id = r.media_id AND x.source = 'fankai' LIMIT 1) AS fankai_id,
                (SELECT i.url FROM media_image i
                  WHERE i.media_id = r.media_id AND i.cover_type = 'poster'
                    AND i.season_number IS NULL
                  ORDER BY i.sort_order LIMIT 1) AS poster
           FROM media_relation r
           JOIN media_item w ON w.id = r.media_id
          WHERE r.relation_type = 'ORIGINAL' AND w.is_enabled = 1 AND r.media_id <> ?
            AND ({})
          ORDER BY w.year, w.title",
        matches.join(" OR ")
    );

    let mut query = sqlx::query(db.sql(&sql)).bind(media_id);
    for value in values {
        query = query.bind(value);
    }

    let mut out: Vec<Relation> = Vec::new();
    for row in query.fetch_all(db.pool()).await? {
        let work_id = row.text("media_id")?;
        if out
            .iter()
            .any(|r| r.work_id.as_deref() == Some(work_id.as_str()))
        {
            continue;
        }
        out.push(Relation {
            id: format!("recut:{work_id}"),
            relation_type: "RECUT".to_string(),
            source: "fankai".to_string(),
            external_id: row
                .opt_text("fankai_id")?
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            mal_id: None,
            title: row.text("title")?,
            medium: "anime".to_string(),
            format: None,
            year: row.opt_int("year")?,
            image: row.opt_text("poster")?,
            is_adult: row.flag("is_adult")?,
            work_id: Some(work_id),
            sort_order: i32::try_from(out.len()).unwrap_or(i32::MAX),
        });
    }

    Ok(out)
}

fn map_relation(row: &sqlx::any::AnyRow) -> Result<Relation> {
    Ok(Relation {
        id: row.text("id")?,
        relation_type: row.text("relation_type")?,
        source: row.text("source")?,
        external_id: row.big("external_id")?,
        mal_id: row.opt_big("mal_id")?,
        title: row.text("title")?,
        medium: row.text("medium")?,
        format: row.opt_text("format")?,
        year: row.opt_int("year")?,
        image: row.opt_text("image")?,
        is_adult: row.flag("is_adult")? || row.flag("work_is_adult")?,
        work_id: row.opt_text("work_id")?,
        sort_order: row.int("sort_order")?,
    })
}

/// The work's own credits — its episodes' are not its cast, and are read
/// where an episode or a person is — each with the person it is filed under.
async fn load_credits(db: &Db, media_id: &str) -> Result<(Vec<Credit>, People)> {
    let rows = sqlx::query(db.sql(
        "SELECT c.id, c.credit_type, c.person_name, c.character_name, c.image,
                c.tmdb_person_id, c.credit_tmdb_id, c.sort_order, c.is_manual, c.person_id,
                p.tmdb_id AS person_tmdb_id, p.imdb_id AS person_imdb_id,
                p.tvdb_id AS person_tvdb_id
           FROM media_credit c LEFT JOIN person p ON p.id = c.person_id
          WHERE c.media_id = ? AND c.season_number IS NULL
          ORDER BY c.credit_type, c.sort_order",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    let mut credits = Vec::with_capacity(rows.len());
    let mut people = People::default();
    for row in &rows {
        let credit = map_credit(row)?;
        if let Some(person_id) = row.opt_text("person_id")? {
            people.by_credit.insert(
                credit.id.clone(),
                PersonRef {
                    person_id: Some(person_id),
                    tmdb_id: row.opt_big("person_tmdb_id")?,
                    imdb_id: row.opt_text("person_imdb_id")?,
                    tvdb_id: row.opt_big("person_tvdb_id")?,
                    ..PersonRef::default()
                },
            );
        }
        credits.push(credit);
    }
    Ok((credits, people))
}

pub(super) fn map_credit(row: &sqlx::any::AnyRow) -> Result<Credit> {
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

pub(super) fn map_image(row: &sqlx::any::AnyRow) -> Result<Image> {
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

pub(super) fn map_rating(row: &sqlx::any::AnyRow) -> Result<Rating> {
    Ok(Rating {
        source: row.text("source")?,
        value: row.opt_real("value")?,
        votes: row.opt_big("votes")?,
        rating_type: row.opt_text("rating_type")?,
    })
}

async fn load_translations(db: &Db, media_id: &str) -> Result<Vec<Translation>> {
    let rows = sqlx::query(db.sql(
        "SELECT language, title, overview, tagline, is_manual FROM media_translation WHERE media_id = ?",
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
                tagline: row.opt_text("tagline")?,
                is_manual: row.flag("is_manual")?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        repo::item::{
            ItemWrite, prequels_of,
            testing::{sample, stored},
            upsert,
        },
        testing::db,
    };

    #[tokio::test]
    async fn an_anime_leads_to_the_fan_kai_cut_from_it() {
        let db = db().await;

        let mut original = sample();
        original.id = crate::db::new_id();
        original.kind = MediaKind::Series;
        original.title = "Naruto Shippuden".into();
        original.external_ids = ExternalIds {
            tvdb: Some(79824),
            anilist: vec![1735],
            mal: vec![1735],
            ..Default::default()
        };

        let mut recut = sample();
        recut.id = crate::db::new_id();
        recut.kind = MediaKind::Series;
        recut.title = "Naruto Shippuden Yabai".into();
        recut.year = Some(2007);
        recut.external_ids = ExternalIds {
            fankai: Some(12),
            ..Default::default()
        };
        recut.images[0].url = "https://metadata.fankai.fr/series/12/image/poster".into();
        recut.relations = vec![Relation {
            id: String::new(),
            relation_type: "ORIGINAL".into(),
            source: "anilist".into(),
            external_id: 1735,
            mal_id: Some(1735),
            title: "Naruto Shippuden".into(),
            medium: "anime".into(),
            format: Some("TV".into()),
            year: Some(2007),
            image: None,
            is_adult: false,
            work_id: None,
            sort_order: 0,
        }];

        // Another anime's Fan-Kai, which must not be offered here.
        let mut other = recut.clone();
        other.id = crate::db::new_id();
        other.title = "Naruto Yabai".into();
        other.external_ids.fankai = Some(25);
        other.relations[0].external_id = 20;
        other.relations[0].mal_id = Some(20);

        for item in [&original, &recut, &other] {
            upsert(
                &db,
                ItemWrite {
                    item,
                    replace_children: true,
                },
            )
            .await
            .expect("the writes succeed");
        }

        let found = recuts(&db, &original.id, &original.external_ids)
            .await
            .unwrap();
        assert_eq!(found.len(), 1, "one Fan-Kai, listed once: {found:?}");
        assert_eq!(found[0].relation_type, "RECUT");
        assert_eq!(found[0].work_id.as_deref(), Some(recut.id.as_str()));
        assert_eq!(found[0].external_id, 12);
        assert_eq!(found[0].title, "Naruto Shippuden Yabai");
        assert_eq!(
            found[0].image.as_deref(),
            Some("https://metadata.fankai.fr/series/12/image/poster")
        );

        // By MyAnimeList alone, it is found too; the Fan-Kai itself finds nothing.
        let by_mal = ExternalIds {
            mal: vec![1735],
            ..Default::default()
        };
        assert_eq!(recuts(&db, &original.id, &by_mal).await.unwrap().len(), 1);
        assert!(
            recuts(&db, &recut.id, &recut.external_ids)
                .await
                .unwrap()
                .is_empty()
        );

        // And the Fan-Kai's own relation leads back to the anime held here.
        let stored = get(&db, &recut.id).await.unwrap().expect("the Fan-Kai");
        let mut stored = stored;
        load_children(&db, &mut stored).await.unwrap();
        assert_eq!(
            stored.relations[0].work_id.as_deref(),
            Some(original.id.as_str())
        );
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
    async fn works_are_named_by_id_in_batches_and_the_gone_are_not() {
        let db = db().await;
        assert!(super::titles(&db, &[]).await.unwrap().is_empty());

        let held = stored(&db, |i| i.title = "Held".into()).await;
        let mut ids: Vec<String> = (0..450).map(|_| crate::db::new_id()).collect();
        ids.push(held.id.clone());

        let named = super::titles(&db, &ids).await.unwrap();
        assert_eq!(named.len(), 1, "an id nothing holds is not named");
        assert_eq!(named[&held.id].title, "Held");
    }

    fn related(
        relation_type: &str,
        external_id: i64,
        title: &str,
        year: Option<i32>,
        sort_order: i32,
    ) -> Relation {
        Relation {
            id: String::new(),
            relation_type: relation_type.into(),
            source: "anilist".into(),
            external_id,
            mal_id: None,
            title: title.into(),
            medium: "anime".into(),
            format: Some("TV".into()),
            year,
            image: None,
            is_adult: false,
            work_id: None,
            sort_order,
        }
    }

    #[tokio::test]
    async fn a_related_work_is_kept_and_found_here_once_the_catalogue_holds_it() {
        let db = db().await;
        let mut first = sample();
        first.relations = vec![
            related("PREQUEL", 20811, "No Regrets", Some(2014), 0),
            related("SEQUEL", 20958, "Season 2", Some(2017), 1),
        ];
        upsert(
            &db,
            ItemWrite {
                item: &first,
                replace_children: true,
            },
        )
        .await
        .expect("stored");

        let mut read = first.clone();
        load_children(&db, &mut read).await.expect("loaded");
        assert_eq!(
            read.relations
                .iter()
                .map(|r| r.external_id)
                .collect::<Vec<_>>(),
            [20811, 20958]
        );
        assert!(
            read.relations.iter().all(|r| r.work_id.is_none()),
            "nothing held yet"
        );

        // The sequel arrives, filed under its AniList id.
        let mut second = sample();
        second.id = crate::db::new_id();
        second.slug = "season-2".into();
        second.title = "Season 2".into();
        second.external_ids = ExternalIds {
            anilist: vec![20958],
            ..Default::default()
        };
        upsert(
            &db,
            ItemWrite {
                item: &second,
                replace_children: false,
            },
        )
        .await
        .expect("stored");

        load_children(&db, &mut read).await.expect("loaded");
        assert_eq!(
            read.relations[1].work_id.as_deref(),
            Some(second.id.as_str())
        );
        assert_eq!(read.relations[0].work_id, None);

        let kin = prequels_of(&db, &[first.id.clone(), second.id.clone()], true)
            .await
            .expect("read");
        assert_eq!(
            kin.get(&first.id).map(|k| k.title.as_str()),
            Some("No Regrets")
        );
        assert!(!kin.contains_key(&second.id), "nothing filed before it");
    }

    #[tokio::test]
    async fn an_entry_that_is_this_very_work_is_not_a_related_one() {
        // One work here can be several AniList entries — a series and the
        // film filed under it — and one entry relates to the other.
        let db = db().await;
        let mut item = sample();
        item.external_ids = ExternalIds {
            anilist: vec![21, 12001],
            ..Default::default()
        };
        item.relations = vec![
            related("SIDE_STORY", 12001, "The Film", Some(2012), 0),
            related("PREQUEL", 12001, "The Film", Some(2012), 1),
            related("SEQUEL", 30, "Something else", None, 2),
            related("PREQUEL", 40, "The one before", Some(2000), 3),
        ];
        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");

        let mut read = item.clone();
        load_children(&db, &mut read).await.expect("loaded");
        assert_eq!(
            read.relations
                .iter()
                .map(|r| r.external_id)
                .collect::<Vec<_>>(),
            [30, 40]
        );
        // The later prequel is the work itself, so the earlier one is named.
        let kin = prequels_of(&db, &[item.id.clone()], true).await.unwrap();
        assert_eq!(
            kin.get(&item.id).map(|k| k.title.as_str()),
            Some("The one before")
        );
    }

    #[tokio::test]
    async fn what_is_for_adults_is_marked_and_kept_from_whoever_may_not_see_it() {
        let db = db().await;
        let mut first = sample();
        let mut flagged = related("PREQUEL", 777, "Grown-up prequel", Some(2019), 0);
        flagged.is_adult = true;
        first.relations = vec![
            flagged,
            related("PREQUEL", 20811, "No Regrets", Some(2014), 1),
            related("PREQUEL", 555, "The old one", Some(2001), 2),
        ];
        upsert(
            &db,
            ItemWrite {
                item: &first,
                replace_children: true,
            },
        )
        .await
        .expect("stored");

        // The catalogue holds "No Regrets", and holds it as a work for adults.
        let mut held = sample();
        held.id = crate::db::new_id();
        held.slug = "no-regrets".into();
        held.title = "No Regrets".into();
        held.is_adult = true;
        held.external_ids = ExternalIds {
            anilist: vec![20811],
            ..Default::default()
        };
        upsert(
            &db,
            ItemWrite {
                item: &held,
                replace_children: false,
            },
        )
        .await
        .expect("stored");

        let mut read = first.clone();
        load_children(&db, &mut read).await.expect("loaded");
        let marked: Vec<(i64, bool, bool)> = read
            .relations
            .iter()
            .map(|r| (r.external_id, r.is_adult, r.work_id.is_some()))
            .collect();
        assert_eq!(
            marked,
            [(777, true, false), (20811, true, true), (555, false, false)],
            "flagged by AniList, flagged by what is held, and plain"
        );

        // A chart that may show adult works names the latest prequel; one that
        // may not names the latest that is not for adults, and never leads to
        // the held work for adults.
        let kin = prequels_of(&db, &[first.id.clone()], true).await.unwrap();
        assert_eq!(
            kin.get(&first.id).map(|k| k.title.as_str()),
            Some("Grown-up prequel")
        );
        let kin = prequels_of(&db, &[first.id.clone()], false).await.unwrap();
        assert_eq!(
            kin.get(&first.id)
                .map(|k| (k.title.as_str(), k.work_id.is_some())),
            Some(("The old one", false))
        );
    }
}
