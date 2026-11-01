//! Writes: a work and its provider-sourced children in one transaction, a
//! work entered by hand, a work switched off or deleted.

use anyhow::{Context, Result};
use sqlx::{Any, Arguments, Transaction, any::AnyArguments};

use super::listed::{Listed, mark_changed};
use crate::{
    db::{Db, Double, RowExt, from_bool, new_id, now, text_list},
    domain::{Credit, ExternalSource, ImageDetails, MediaItem},
};

/// Everything written for one work in a single transaction.
pub struct ItemWrite<'a> {
    pub item: &'a MediaItem,
    /// When true, existing provider-sourced children are replaced.
    pub replace_children: bool,
}

/// Insert or update a work and, optionally, its provider-sourced children,
/// with no record of where its values came from: what a test stores a work
/// with. The server writes a refresh with [`upsert_traced`], a sync with
/// [`upsert_unchanged`] and a work entered by hand with [`insert_manual`].
#[cfg(test)]
pub async fn upsert(db: &Db, write: ItemWrite<'_>) -> Result<()> {
    write_item(db, write, None, None).await.map(|_| ())
}

/// [`upsert`], with where its values came from in the same transaction: a
/// reader never finds the one beside the other's predecessor.
pub async fn upsert_traced(
    db: &Db,
    write: ItemWrite<'_>,
    provenance: &crate::merge::provenance::Provenance,
) -> Result<()> {
    write_item(db, write, None, Some(provenance))
        .await
        .map(|_| ())
}

/// How a write made on condition went.
#[derive(Debug, PartialEq, Eq)]
pub enum Conditional {
    Written,
    /// Written by something else since it was read: nothing was written.
    Changed,
    /// Gone since it was read.
    Gone,
}

/// [`upsert_traced`], only if the work is still as it was when `read_at` —
/// its `updated_at` then — was read. The look and the write share the
/// transaction, under SQLite's write lock or the row's own on PostgreSQL, so
/// nothing lands between them: not a refresh, not a work switched off.
pub async fn upsert_unchanged(
    db: &Db,
    write: ItemWrite<'_>,
    read_at: &str,
    provenance: &crate::merge::provenance::Provenance,
) -> Result<Conditional> {
    write_item(db, write, Some(read_at), Some(provenance)).await
}

/// Write a work entered by hand with its identifiers, in one transaction, so
/// long as none of them is another work's: when one is, it is named and
/// nothing is written.
///
/// Claimed where it is written. A look beforehand let two entries made at
/// once with the same id both through, and the upsert a refresh writes with —
/// the last writer wins — then moved the id from the first to the second.
pub async fn insert_manual(
    db: &Db,
    item: &MediaItem,
) -> Result<Result<(), (ExternalSource, String)>> {
    let mut tx = db.begin_write().await?;

    upsert_row(db, &mut tx, item).await?;

    let created = now();
    let mut claimed: Vec<(ExternalSource, String)> = Vec::new();
    for (source, value) in item.external_ids.rows(item.kind) {
        // An id given twice is claimed once.
        if claimed.iter().any(|(s, v)| *s == source && *v == value) {
            continue;
        }
        let written = sqlx::query(db.sql(
            "INSERT INTO media_external_id (media_id, source, value, created_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (source, value) DO NOTHING",
        ))
        .bind(&item.id)
        .bind(source.as_str())
        .bind(&value)
        .bind(&created)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("failed to write external id {source}={value}"))?;

        if written.rows_affected() == 0 {
            // Another work's: the whole entry goes with the transaction.
            return Ok(Err((source, value)));
        }
        claimed.push((source, value));
    }

    // Found by the search the moment it exists, as a refreshed work is.
    let people =
        crate::db::repo::search::index_works(db, &mut tx, std::slice::from_ref(&item.id)).await?;

    tx.commit()
        .await
        .context("failed to commit the work entered by hand")?;
    index_people(db, &people).await;
    Ok(Ok(()))
}

async fn write_item(
    db: &Db,
    write: ItemWrite<'_>,
    read_at: Option<&str>,
    provenance: Option<&crate::merge::provenance::Provenance>,
) -> Result<Conditional> {
    let mut tx = db.begin_write().await?;

    if let Some(read_at) = read_at {
        let sql = match db.dialect() {
            crate::db::Dialect::Postgres => {
                "SELECT updated_at FROM media_item WHERE id = ? FOR UPDATE"
            }
            crate::db::Dialect::Sqlite => "SELECT updated_at FROM media_item WHERE id = ?",
        };
        let current: Option<String> = sqlx::query_scalar(db.sql(sql))
            .bind(&write.item.id)
            .fetch_optional(&mut *tx)
            .await
            .context("failed to read the work before writing it")?;
        match current {
            None => return Ok(Conditional::Gone),
            Some(current) if current != read_at => return Ok(Conditional::Changed),
            Some(_) => {}
        }
    }

    upsert_row(db, &mut tx, write.item).await?;
    let displaced = upsert_external_ids(db, &mut tx, write.item).await?;

    // Who the credits name before they are replaced: somebody a refresh takes
    // off the work has their row written again too, for one work fewer.
    let ids = std::slice::from_ref(&write.item.id);
    let mut people = if write.replace_children {
        crate::db::repo::search::people_of(db, &mut tx, ids).await?
    } else {
        Default::default()
    };
    if write.replace_children {
        replace_children(db, &mut tx, write.item).await?;
    }

    if let Some(provenance) = provenance {
        sqlx::query(db.sql("UPDATE media_item SET provenance = ? WHERE id = ?"))
            .bind(serde_json::to_string(provenance)?)
            .bind(&write.item.id)
            .execute(&mut *tx)
            .await
            .context("failed to record where the work's values came from")?;
    }

    // The work's rows of the search index, from what was just written, in
    // the same transaction: a reader never finds the work by a name it no
    // longer has, nor misses it by the one it was given.
    people.extend(crate::db::repo::search::index_works(db, &mut tx, ids).await?);

    tx.commit().await.context("failed to commit item write")?;

    // After the commit, one row at a time, so no two works' locks are held
    // together here; a work that lost an IMDb id loses IMDb's figure with it.
    for other in displaced {
        mark_changed(db, &other).await?;
    }
    index_people(db, &people).await;

    Ok(Conditional::Written)
}

/// Write again the search index's rows of the people a write touched, once it
/// is committed: in their own transaction, since a person is credited on many
/// works and two writes holding one person's row each would wait on each
/// other. One that fails is said and left for the index's next rebuild — the
/// work itself is written, and a person's row a little behind costs a
/// search nothing worse than a count of works off by one.
async fn index_people(db: &Db, people: &std::collections::BTreeSet<String>) {
    if people.is_empty() {
        return;
    }
    if let Err(e) = crate::db::repo::search::index_people(db, people).await {
        tracing::warn!(
            error = format_args!("{e:#}"),
            "could not write the people of a work into the search index"
        );
    }
}

/// A slug no other work of this kind is already using.
///
/// Slugs come from the title and year, and two different films genuinely share
/// both — there are several unrelated *Ram (2023)*. The column is unique so a
/// slug addresses one work, which means the collision has to be resolved here
/// rather than rejected: an entry nobody can look up is worth less than one
/// under `ram-2023-2`.
///
/// A work that arrives without one is given one from its title and year: an
/// empty slug named the NFO export's folder `series/`, which the export
/// refuses to write, and the next such work `series/-2`.
async fn free_slug(db: &Db, tx: &mut Transaction<'_, Any>, item: &MediaItem) -> Result<String> {
    let sql = "SELECT id FROM media_item WHERE kind = ? AND slug = ? LIMIT 1";
    let base = if item.slug.trim().is_empty() {
        crate::domain::make_slug(&item.title, item.year)
    } else {
        item.slug.clone()
    };

    for attempt in 1..=50 {
        let candidate = match attempt {
            1 => base.clone(),
            n => format!("{base}-{n}"),
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
    Ok(format!("{base}-{}", &item.id[..8]))
}

async fn upsert_row(db: &Db, tx: &mut Transaction<'_, Any>, item: &MediaItem) -> Result<()> {
    let slug = free_slug(db, tx, item).await?;

    // `created_at` is preserved on conflict; everything else is overwritten.
    let sql = "
        INSERT INTO media_item (
            id, kind, slug, title, sort_title, original_title, overview, status,
            original_language, original_country, runtime, year, first_aired, last_aired,
            in_cinemas, physical_release, digital_release, air_time, network, studio,
            content_rating, content_rating_country, homepage, trailer_youtube_id, theme_music,
            title_qualifier,
            popularity, genres, keywords, collection_tmdb_id, is_manual, is_enabled, is_adult,
            created_at, updated_at, refreshed_at, refresh_after, refresh_error,
            listed_title, listed_year, listed_genres, listed_keywords, listed_network,
            listed_studio, listed_status, listed_language, listed_release, listed_score,
            tagline, format, production_countries, production_companies
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                  ?, ?, ?, ?)
        ON CONFLICT (id) DO UPDATE SET
            tagline = excluded.tagline,
            format = excluded.format,
            production_countries = excluded.production_countries,
            production_companies = excluded.production_companies,
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
            theme_music = excluded.theme_music,
            title_qualifier = excluded.title_qualifier,
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
            refresh_error = excluded.refresh_error,
            -- What it is listed by stays until it is listed again, with its
            -- locks: written from the providers' values here, a locked genre
            -- would drop out of its filter until then.
            listed_change = media_item.listed_change + 1
    ";

    // A new work is listed by its own values straight away, so it is in every
    // filter from its first moment; its locks and IMDb's figure follow when it
    // is listed properly, which its first write has already asked for.
    let listed = Listed::of(item);

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
        .bind(&item.theme_music)
        .bind(&item.title_qualifier)
        .bind(Double(item.popularity))
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
        .bind(&listed.title)
        .bind(listed.year)
        .bind(text_list(&listed.genres))
        .bind(text_list(&listed.keywords))
        .bind(&listed.network)
        .bind(&listed.studio)
        .bind(&listed.status)
        .bind(&listed.language)
        .bind(&listed.release)
        .bind(Double(listed.score))
        .bind(&item.tagline)
        .bind(&item.format)
        .bind(text_list(&item.production_countries))
        .bind(text_list(&item.production_companies))
        .execute(&mut **tx)
        .await
        .context("failed to write media_item")?;

    Ok(())
}

/// Returns the other works that held one of these ids until now.
async fn upsert_external_ids(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    item: &MediaItem,
) -> Result<Vec<String>> {
    // Ids this work no longer claims must go, or a stale row keeps resolving to it.
    sqlx::query(db.sql("DELETE FROM media_external_id WHERE media_id = ?"))
        .bind(&item.id)
        .execute(&mut **tx)
        .await?;

    let created = now();
    let mut displaced: Vec<String> = Vec::new();

    for (source, value) in item.external_ids.rows(item.kind) {
        let owner: Option<String> = sqlx::query_scalar(
            db.sql("SELECT media_id FROM media_external_id WHERE source = ? AND value = ?"),
        )
        .bind(source.as_str())
        .bind(&value)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(owner) = owner.filter(|o| o != &item.id && !displaced.contains(o)) {
            displaced.push(owner);
        }

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

    Ok(displaced)
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
    // Whom the credits are filed under, read before they go: a credit given
    // again keeps its person (see `repo::person`).
    let kept = crate::db::repo::person::kept(db, tx, &item.id).await?;
    // What was said of the pictures, read before they go: a picture given
    // again without it — put back from the stored work when nobody answered
    // with pictures — keeps it.
    let said = image_details(db, tx, &item.id).await?;

    // The work's own credits are given again whole; its episodes' only when
    // a source gave some. None means nobody said — TMDB not asked, or not
    // answering — and what is held of them stands, as an empty list stands
    // for the rest in `service::keep_what_nobody_answered`.
    sqlx::query(db.sql(
        "DELETE FROM media_credit
          WHERE media_id = ? AND is_manual = 0 AND (season_number IS NULL OR ? = 1)",
    ))
    .bind(&item.id)
    .bind(from_bool(!item.people.episodes.is_empty()))
    .execute(&mut **tx)
    .await?;

    for table in [
        "media_season",
        "media_episode",
        "media_image",
        "media_relation",
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

    // A season or an episode added by hand outlives the delete above, so a
    // provider listing the same number lands on it: the update leaves it as
    // the person wrote it (`WHERE … is_manual = 0`), on both engines, rather
    // than writing the provider's text, date and still over it.
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
                 updated_at = excluded.updated_at
             WHERE media_season.is_manual = 0",
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
                 updated_at = excluded.updated_at
             WHERE media_episode.is_manual = 0",
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
        .bind(Double(ep.rating.map(|r| r.value)))
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
        let details = item
            .image_details
            .get(&image.url)
            .or_else(|| said.get(&image.url))
            .copied()
            .unwrap_or_default();
        sqlx::query(db.sql(
            "INSERT INTO media_image
                 (id, media_id, season_number, cover_type, url, language, sort_order,
                  source, is_manual, created_at, width, height, vote_average, vote_count)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?)
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
        .bind(details.width)
        .bind(details.height)
        .bind(Double(details.vote_average))
        .bind(details.vote_count)
        .execute(&mut **tx)
        .await
        .context("failed to write image")?;
    }

    write_credits(db, tx, item, &kept, &created).await?;

    for relation in &item.relations {
        sqlx::query(db.sql(
            "INSERT INTO media_relation
                 (id, media_id, relation_type, source, external_id, mal_id, title, medium,
                  format, year, image, is_adult, sort_order, is_manual, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)",
        ))
        .bind(if relation.id.is_empty() {
            new_id()
        } else {
            relation.id.clone()
        })
        .bind(&item.id)
        .bind(&relation.relation_type)
        .bind(&relation.source)
        .bind(relation.external_id)
        .bind(relation.mal_id)
        .bind(&relation.title)
        .bind(&relation.medium)
        .bind(&relation.format)
        .bind(relation.year)
        .bind(&relation.image)
        .bind(from_bool(relation.is_adult))
        .bind(relation.sort_order)
        .bind(&created)
        .execute(&mut **tx)
        .await
        .context("failed to write relation")?;
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
        .bind(Double(rating.value))
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
            "INSERT INTO media_translation (media_id, language, title, overview, tagline, is_manual)
             VALUES (?, ?, ?, ?, ?, 0)
             ON CONFLICT (media_id, language) DO UPDATE SET
                 title = excluded.title,
                 overview = excluded.overview,
                 tagline = excluded.tagline",
        ))
        .bind(&item.id)
        .bind(&tr.language)
        .bind(&tr.title)
        .bind(&tr.overview)
        .bind(&tr.tagline)
        .execute(&mut **tx)
        .await
        .context("failed to write translation")?;
    }

    // Wholly the providers', like the ratings: replaced, never merged.
    crate::db::repo::release::replace(db, tx, &item.id, &item.releases).await?;

    // The seasons' texts in other languages, the companies, TMDB's keywords
    // and the videos: written again when the refresh gives some. None means
    // nobody said — TMDB not asked, or not answering — and what is held of
    // them stands, as an empty list stands for the rest in
    // `service::keep_what_nobody_answered`.
    crate::db::repo::translation::replace_seasons(db, tx, &item.id, &item.seasons).await?;
    if !item.companies.is_empty() {
        crate::db::repo::company::replace(db, tx, &item.id, &item.companies).await?;
    }
    if !item.keyword_entities.is_empty() {
        crate::db::repo::keyword::replace(db, tx, &item.id, &item.keyword_entities).await?;
    }
    if !item.videos.is_empty() {
        crate::db::repo::video::replace(db, tx, &item.id, &item.videos).await?;
    }

    Ok(())
}

/// What a work's kept answers say that this version keeps and its rows may
/// not hold yet: written by a pass over those answers
/// (`jobs::refresh::replay_snapshots`) rather than at the next refresh.
#[derive(Debug, Default)]
pub struct Filled {
    pub companies: Vec<crate::domain::CompanyCredit>,
    pub keywords: Vec<crate::domain::Keyword>,
    pub videos: Vec<crate::domain::Video>,
    /// Each of TMDB's pictures, by its address: the language written on it,
    /// and what TMDB says of it.
    pub pictures: Vec<(String, Option<String>, ImageDetails)>,
    /// The department and the job of each credit of the crew, by TMDB's id
    /// for the credit.
    pub jobs: Vec<(String, String, String)>,
}

/// Write into a work's rows what `filled` holds, in one transaction: its
/// companies, keywords and videos in place of those it has, when there are
/// any; the language and what is said of each of TMDB's pictures it holds,
/// a language it holds standing; the department and job of each credit of
/// its crew TMDB's id names. Nothing else of the work is touched.
pub async fn fill_in(db: &Db, media_id: &str, filled: &Filled) -> Result<()> {
    let mut tx = db.begin_write().await?;
    if !filled.companies.is_empty() {
        crate::db::repo::company::replace(db, &mut tx, media_id, &filled.companies).await?;
    }
    if !filled.keywords.is_empty() {
        crate::db::repo::keyword::replace(db, &mut tx, media_id, &filled.keywords).await?;
    }
    if !filled.videos.is_empty() {
        crate::db::repo::video::replace(db, &mut tx, media_id, &filled.videos).await?;
    }
    for (url, language, details) in &filled.pictures {
        sqlx::query(db.sql(
            "UPDATE media_image SET language = COALESCE(language, ?), width = ?, height = ?,
                    vote_average = ?, vote_count = ?
              WHERE media_id = ? AND url = ? AND is_manual = 0 AND source = 'tmdb'",
        ))
        .bind(language)
        .bind(details.width)
        .bind(details.height)
        .bind(Double(details.vote_average))
        .bind(details.vote_count)
        .bind(media_id)
        .bind(url)
        .execute(&mut *tx)
        .await
        .context("failed to fill in what is said of a picture")?;
    }
    for (credit, department, job) in &filled.jobs {
        sqlx::query(db.sql(
            "UPDATE media_credit SET department = ?, job = ?
              WHERE media_id = ? AND credit_tmdb_id = ? AND is_manual = 0",
        ))
        .bind(department)
        .bind(job)
        .bind(media_id)
        .bind(credit)
        .execute(&mut *tx)
        .await
        .context("failed to fill in a credit's job")?;
    }
    tx.commit()
        .await
        .context("failed to commit what was filled in")?;
    Ok(())
}

/// What a work's rows say of its sources' pictures, by address: what a
/// write keeps of a picture given again without it.
async fn image_details(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    media_id: &str,
) -> Result<std::collections::HashMap<String, ImageDetails>> {
    let rows = sqlx::query(db.sql(
        "SELECT url, width, height, vote_average, vote_count FROM media_image
          WHERE media_id = ? AND is_manual = 0
            AND (width IS NOT NULL OR height IS NOT NULL
                 OR vote_average IS NOT NULL OR vote_count IS NOT NULL)",
    ))
    .bind(media_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut out = std::collections::HashMap::with_capacity(rows.len());
    for row in &rows {
        out.insert(
            row.text("url")?,
            ImageDetails {
                width: row.opt_int("width")?,
                height: row.opt_int("height")?,
                vote_average: row.opt_real("vote_average")?,
                vote_count: row.opt_big("vote_count")?,
            },
        );
    }
    Ok(out)
}

/// How many credits one insert writes, at sixteen values each.
const CREDITS_PER_INSERT: usize = 50;

/// Write the credits the sources gave — the work's own, then its episodes',
/// each on the episode of the work it is of — each filed under its person.
async fn write_credits(
    db: &Db,
    tx: &mut Transaction<'_, Any>,
    item: &MediaItem,
    kept: &crate::db::repo::person::Kept,
    created: &str,
) -> Result<()> {
    use crate::db::repo::person::{self, Linkable};

    let mut credits: Vec<(&Credit, Option<(i32, i32)>)> = item
        .credits
        .iter()
        .filter(|credit| !credit.is_manual)
        .map(|credit| (credit, None))
        .collect();
    credits.extend(
        placed_on_episodes(item)
            .into_iter()
            .map(|(credit, place)| (credit, Some(place))),
    );
    if credits.is_empty() {
        return Ok(());
    }

    let linkables: Vec<Linkable<'_>> = credits
        .iter()
        .map(|(credit, place)| Linkable::of(credit, *place, item.people.of(credit)))
        .collect();
    let people = person::resolve(db, tx, kept, &linkables).await?;

    let rows: Vec<_> = credits.iter().zip(people).collect();
    for chunk in rows.chunks(CREDITS_PER_INSERT) {
        let sql = format!(
            "INSERT INTO media_credit
                 (id, media_id, credit_type, person_name, character_name, image,
                  tmdb_person_id, credit_tmdb_id, sort_order, is_manual, created_at,
                  person_id, season_number, episode_number, department, job)
             VALUES {}",
            vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for ((credit, place), person) in chunk {
            let id = if credit.id.is_empty() {
                new_id()
            } else {
                credit.id.clone()
            };
            add(&mut args, id)?;
            add(&mut args, item.id.clone())?;
            add(&mut args, credit.credit_type.as_str().to_string())?;
            add(&mut args, credit.person_name.clone())?;
            add(&mut args, credit.character_name.clone())?;
            add(&mut args, credit.image.clone())?;
            add(&mut args, credit.tmdb_person_id)?;
            add(&mut args, credit.credit_tmdb_id.clone())?;
            add(&mut args, credit.sort_order)?;
            add(&mut args, created.to_string())?;
            add(&mut args, person.clone())?;
            add(&mut args, place.map(|(season, _)| season))?;
            add(&mut args, place.map(|(_, episode)| episode))?;
            add(&mut args, credit.department.clone())?;
            add(&mut args, credit.job.clone())?;
        }
        sqlx::query_with(db.sql(&sql), args)
            .execute(&mut **tx)
            .await
            .context("failed to write credits")?;
    }
    Ok(())
}

/// Bind one more value to a statement being built.
fn add<'t>(
    args: &mut AnyArguments,
    value: impl sqlx::Encode<'t, Any> + sqlx::Type<Any>,
) -> Result<()> {
    args.add(value).map_err(|e| anyhow::anyhow!("{e}"))
}

/// The episode credits a source gave, each on the episode of the work it is
/// of: by the episode's TMDB id where the source gives one — TMDB numbers a
/// series its own way where TheTVDB numbers the work, and the merge filed
/// TMDB's id on the work's episode that is TMDB's — else by the numbers it
/// gives. One the work has no episode for is left out, as the merge leaves
/// out an episode only one source has. The same credit twice on one
/// episode is one.
fn placed_on_episodes(item: &MediaItem) -> Vec<(&Credit, (i32, i32))> {
    use std::collections::{HashMap, HashSet};

    if item.people.episodes.is_empty() {
        return Vec::new();
    }
    let by_tmdb: HashMap<i64, (i32, i32)> = item
        .episodes
        .iter()
        .filter_map(|e| Some((e.tmdb_id?, (e.season_number, e.episode_number))))
        .collect();
    let held: HashSet<(i32, i32)> = item
        .episodes
        .iter()
        .map(|e| (e.season_number, e.episode_number))
        .collect();

    let mut seen: HashSet<(i32, i32, &'static str, String)> = HashSet::new();
    let mut out = Vec::new();
    for given in &item.people.episodes {
        let place = match given.tmdb_episode_id {
            Some(tmdb) => by_tmdb.get(&tmdb).copied(),
            None => Some((given.season_number, given.episode_number))
                .filter(|place| held.contains(place)),
        };
        let Some(place) = place else { continue };
        let credit = &given.credit;
        let standing = (
            place.0,
            place.1,
            credit.credit_type.as_str(),
            crate::db::repo::person::key(credit.tmdb_person_id, &credit.person_name),
        );
        if seen.insert(standing) {
            out.push((credit, place));
        }
    }
    out
}

pub async fn delete(db: &Db, id: &str) -> Result<bool> {
    let mut tx = db.begin_write().await?;
    // Who its credits name, for their rows of the search index to count one
    // work fewer; its own rows and its episodes' go with it, by their key.
    let people =
        crate::db::repo::search::people_of(db, &mut tx, std::slice::from_ref(&id.to_string()))
            .await?;

    // Every child table cascades from media_item.
    let result = sqlx::query(db.sql("DELETE FROM media_item WHERE id = ?"))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await.context("failed to commit the deletion")?;

    index_people(db, &people).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        db::{
            repo::item::{
                count, find_id_by_external, get, load_children,
                testing::{sample, stored},
            },
            testing::db,
        },
        domain::{CoverType, ExternalIds, MediaKind},
    };

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
        let path = crate::testing::scratch_path("writers").with_extension("db");
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
    async fn a_work_that_arrives_without_a_slug_is_given_one() {
        let db = db().await;

        let mut item = sample();
        item.slug = String::new();
        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("the write succeeds");

        let stored = get(&db, &item.id).await.unwrap().expect("stored");
        assert_eq!(stored.slug, "round-trip-2026");
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
        assert_eq!(read.theme_music, written.theme_music);
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

    #[tokio::test]
    async fn a_work_entered_by_hand_takes_no_identifier_from_another() {
        let db = db().await;
        let held = stored(&db, |i| {
            i.title = "Held".into();
            i.external_ids = ExternalIds {
                tmdb: Some(4242),
                ..Default::default()
            };
        })
        .await;

        let mut claiming = sample();
        claiming.id = crate::db::new_id();
        claiming.title = "Claiming".into();
        claiming.slug = "claiming".into();
        claiming.credits.clear();
        claiming.ratings.clear();
        claiming.external_ids = ExternalIds {
            imdb: Some("tt0000007".into()),
            tmdb: Some(4242),
            ..Default::default()
        };

        let refused = insert_manual(&db, &claiming).await.expect("asked");
        assert_eq!(
            refused,
            Err((ExternalSource::TmdbMovie, "4242".to_string())),
            "the id another work goes by is named"
        );
        // Nothing of it was written, and the id stays where it was.
        assert!(get(&db, &claiming.id).await.unwrap().is_none());
        assert_eq!(
            find_id_by_external(&db, ExternalSource::TmdbMovie, "4242")
                .await
                .unwrap(),
            Some(held.id.clone())
        );
        assert_eq!(
            find_id_by_external(&db, ExternalSource::Imdb, "tt0000007")
                .await
                .unwrap(),
            None
        );

        // With an id of its own, and the same one given twice, it is written.
        claiming.external_ids = ExternalIds {
            mal: vec![77, 77],
            ..Default::default()
        };
        assert_eq!(insert_manual(&db, &claiming).await.expect("asked"), Ok(()));
        let read = get(&db, &claiming.id).await.unwrap().expect("written");
        assert_eq!(read.external_ids.mal, [77]);
    }

    fn company_credit(
        role: crate::domain::CompanyRole,
        name: &str,
        tmdb: Option<i64>,
        tvdb: Option<i64>,
        order: i32,
    ) -> crate::domain::CompanyCredit {
        crate::domain::CompanyCredit {
            company: crate::domain::Company {
                id: String::new(),
                kind: role.kind(),
                name: name.into(),
                tmdb_id: tmdb,
                tvdb_id: tvdb,
                logo: tmdb.map(|id| format!("https://image.tmdb.org/t/p/original/{id}.png")),
                country: Some("US".into()),
            },
            role,
            sort_order: order,
            source: Some(if tmdb.is_some() { "tmdb" } else { "tvdb" }.into()),
        }
    }

    fn video(key: &str, order: i32) -> crate::domain::Video {
        crate::domain::Video {
            id: String::new(),
            site: "YouTube".into(),
            key: key.into(),
            name: Some(format!("Trailer {key}")),
            video_type: Some("Trailer".into()),
            language: Some("eng".into()),
            country: Some("US".into()),
            official: order == 0,
            published_at: Some("2024-09-17T16:44:25.000Z".into()),
            size: Some(1080),
            tmdb_id: Some(format!("v{key}")),
            source: Some("tmdb".into()),
            sort_order: order,
        }
    }

    /// The companies, TMDB's keywords, the videos, the seasons' texts in
    /// other languages, the crew's departments and jobs and what is said of
    /// each picture: written with the work and read back as they were.
    #[tokio::test]
    async fn the_models_new_lists_are_written_and_read_back() {
        use crate::domain::{CompanyRole, ImageDetails, Keyword, Translation};

        let db = db().await;
        let mut item = sample();
        item.id = crate::db::new_id();
        item.title = "Lists Round Trip".into();
        item.slug = "lists-round-trip".into();
        item.external_ids = ExternalIds::default();
        item.episodes.clear();
        let mut composer = item.credits[0].clone();
        composer.credit_type = crate::domain::CreditType::Crew;
        composer.person_name = "A Composer".into();
        composer.character_name = Some("Original Music Composer".into());
        composer.tmdb_person_id = Some(8);
        composer.credit_tmdb_id = Some("crew-8".into());
        composer.department = Some("Sound".into());
        composer.job = Some("Original Music Composer".into());
        item.credits.push(composer);
        item.companies = vec![
            company_credit(CompanyRole::Network, "AMC", Some(174), Some(389), 0),
            company_credit(CompanyRole::Production, "Sony", Some(11073), None, 0),
            company_credit(CompanyRole::Studio, "A Studio", None, Some(77), 0),
        ];
        item.keyword_entities = vec![
            Keyword {
                id: String::new(),
                tmdb_id: Some(1508),
                name: "new mexico".into(),
            },
            Keyword {
                id: String::new(),
                tmdb_id: Some(2231),
                name: "drug dealer".into(),
            },
        ];
        item.videos = vec![video("a", 0), video("b", 1)];
        item.image_details.insert(
            "https://example.invalid/p.jpg".into(),
            ImageDetails {
                width: Some(2000),
                height: Some(3000),
                vote_average: Some(5.5),
                vote_count: Some(12),
            },
        );
        item.images[0].language = Some("eng".into());
        let mut season = crate::db::repo::child::blank_season(1);
        season.is_manual = false;
        season.translations = vec![Translation {
            language: "fra".into(),
            title: Some("Saison un".into()),
            overview: Some("Le début.".into()),
            tagline: None,
            is_manual: false,
        }];
        item.seasons = vec![season];

        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("written");

        let mut read = get(&db, &item.id).await.unwrap().expect("a work");
        load_children(&db, &mut read).await.unwrap();

        // Its role, its name, TMDB's and TheTVDB's ids, whether it has an id here.
        type Read<'a> = (CompanyRole, &'a str, Option<i64>, Option<i64>, bool);
        let companies: Vec<Read<'_>> = read
            .companies
            .iter()
            .map(|c| {
                (
                    c.role,
                    c.company.name.as_str(),
                    c.company.tmdb_id,
                    c.company.tvdb_id,
                    c.company.id.is_empty(),
                )
            })
            .collect();
        assert_eq!(
            companies,
            [
                (CompanyRole::Network, "AMC", Some(174), Some(389), false),
                (CompanyRole::Production, "Sony", Some(11073), None, false),
                (CompanyRole::Studio, "A Studio", None, Some(77), false),
            ]
        );
        assert_eq!(read.companies[0].company.country.as_deref(), Some("US"));
        assert_eq!(read.companies[0].source.as_deref(), Some("tmdb"));
        let keywords: Vec<(Option<i64>, &str)> = read
            .keyword_entities
            .iter()
            .map(|k| (k.tmdb_id, k.name.as_str()))
            .collect();
        assert_eq!(
            keywords,
            [(Some(1508), "new mexico"), (Some(2231), "drug dealer")]
        );
        let mut videos = read.videos.clone();
        for v in &mut videos {
            v.id.clear();
        }
        assert_eq!(videos, [video("a", 0), video("b", 1)]);
        assert_eq!(read.seasons[0].translations.len(), 1);
        assert_eq!(
            read.seasons[0].translations[0].title.as_deref(),
            Some("Saison un")
        );
        assert_eq!(
            read.image_details["https://example.invalid/p.jpg"],
            item.image_details["https://example.invalid/p.jpg"]
        );
        assert_eq!(read.images[0].language.as_deref(), Some("eng"));
        let crew = read
            .credits
            .iter()
            .find(|c| c.credit_type == crate::domain::CreditType::Crew)
            .expect("the composer");
        assert_eq!(crew.department.as_deref(), Some("Sound"));
        assert_eq!(crew.job.as_deref(), Some("Original Music Composer"));

        // Another work naming AMC by TheTVDB's id alone, and the studio by
        // its name alone, finds the same companies.
        let mut other = sample();
        other.id = crate::db::new_id();
        other.title = "Lists Round Trip Two".into();
        other.slug = "lists-round-trip-two".into();
        other.external_ids = ExternalIds::default();
        other.credits.clear();
        other.episodes.clear();
        other.companies = vec![
            company_credit(CompanyRole::Network, "AMC", None, Some(389), 0),
            company_credit(CompanyRole::Studio, "a studio ", None, None, 0),
        ];
        upsert(
            &db,
            ItemWrite {
                item: &other,
                replace_children: true,
            },
        )
        .await
        .expect("written");
        let theirs = crate::db::repo::company::of_work(&db, &other.id)
            .await
            .unwrap();
        assert_eq!(theirs[0].company.id, read.companies[0].company.id);
        assert_eq!(theirs[0].company.tmdb_id, Some(174), "the row's ids");
        assert_eq!(theirs[1].company.id, read.companies[2].company.id);

        // A refresh that gives none of them keeps them; the pictures given
        // again without what was said of them keep it.
        let mut again = item.clone();
        again.companies.clear();
        again.keyword_entities.clear();
        again.videos.clear();
        again.image_details.clear();
        for season in &mut again.seasons {
            season.translations.clear();
        }
        upsert(
            &db,
            ItemWrite {
                item: &again,
                replace_children: true,
            },
        )
        .await
        .expect("written again");
        let mut kept = get(&db, &item.id).await.unwrap().expect("a work");
        load_children(&db, &mut kept).await.unwrap();
        assert_eq!(kept.companies.len(), 3);
        assert_eq!(kept.keyword_entities.len(), 2);
        assert_eq!(kept.videos.len(), 2);
        assert_eq!(kept.seasons[0].translations.len(), 1);
        assert_eq!(
            kept.image_details["https://example.invalid/p.jpg"].vote_count,
            Some(12)
        );

        // One that gives them anew replaces them whole.
        let mut fewer = item.clone();
        fewer.companies.truncate(1);
        fewer.videos.truncate(1);
        fewer.keyword_entities.truncate(1);
        upsert(
            &db,
            ItemWrite {
                item: &fewer,
                replace_children: true,
            },
        )
        .await
        .expect("written anew");
        let mut now = get(&db, &item.id).await.unwrap().expect("a work");
        load_children(&db, &mut now).await.unwrap();
        assert_eq!(now.companies.len(), 1);
        assert_eq!(now.videos.len(), 1);
        assert_eq!(now.keyword_entities.len(), 1);

        // Deleting the work takes its links, never the companies.
        assert!(delete(&db, &item.id).await.unwrap());
        assert!(
            crate::db::repo::company::of_work(&db, &item.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            crate::db::repo::company::by_tmdb(&db, crate::domain::CompanyKind::Network, 174)
                .await
                .unwrap()
                .is_some()
        );
    }

    /// An id another company holds is not taken from it: two rows stay, as
    /// the sources said, and the name's match does not merge them.
    #[tokio::test]
    async fn a_company_never_takes_an_id_another_holds() {
        use crate::domain::{CompanyKind, CompanyRole};

        let db = db().await;
        let write = |title: &str, companies| {
            let mut item = sample();
            item.id = crate::db::new_id();
            item.title = title.into();
            item.slug = crate::domain::make_slug(title, None);
            item.external_ids = ExternalIds::default();
            item.credits.clear();
            item.episodes.clear();
            item.companies = companies;
            item
        };
        let first = write(
            "Ids Kept One",
            vec![company_credit(
                CompanyRole::Network,
                "HBO",
                Some(49),
                None,
                0,
            )],
        );
        let second = write(
            "Ids Kept Two",
            vec![company_credit(
                CompanyRole::Network,
                "HBO Max",
                None,
                Some(1),
                0,
            )],
        );
        // TMDB's HBO now says it is TheTVDB's 1, which HBO Max holds.
        let third = write(
            "Ids Kept Three",
            vec![company_credit(
                CompanyRole::Network,
                "HBO",
                Some(49),
                Some(1),
                0,
            )],
        );
        for item in [&first, &second, &third] {
            upsert(
                &db,
                ItemWrite {
                    item,
                    replace_children: true,
                },
            )
            .await
            .expect("written");
        }
        let hbo = crate::db::repo::company::by_tmdb(&db, CompanyKind::Network, 49)
            .await
            .unwrap()
            .expect("HBO");
        assert_eq!(hbo.tvdb_id, None, "HBO Max keeps it");
        let max = crate::db::repo::company::by_tvdb(&db, CompanyKind::Network, 1)
            .await
            .unwrap()
            .expect("HBO Max");
        assert_ne!(max.id, hbo.id);
        assert_eq!(
            crate::db::repo::company::works_of(&db, &hbo.id, true)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// What a pass over the kept answers fills in lands in the rows it names
    /// and nowhere else: a language held stands, a picture or a credit of
    /// another source is left alone.
    #[tokio::test]
    async fn what_is_filled_in_lands_where_it_belongs() {
        use crate::domain::{CompanyRole, ImageDetails};

        let db = db().await;
        let mut item = sample();
        item.id = crate::db::new_id();
        item.title = "Filled In".into();
        item.slug = "filled-in".into();
        item.external_ids = ExternalIds::default();
        item.episodes.clear();
        item.images[0].url = "https://image.tmdb.org/t/p/original/filled.jpg".into();
        let mut held = item.images[0].clone();
        held.url = "https://image.tmdb.org/t/p/original/held.jpg".into();
        held.language = Some("fra".into());
        item.images.push(held);
        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("written");

        let said = ImageDetails {
            width: Some(2000),
            height: None,
            vote_average: Some(6.5),
            vote_count: None,
        };
        fill_in(
            &db,
            &item.id,
            &Filled {
                companies: vec![company_credit(
                    CompanyRole::Network,
                    "Filled Network",
                    Some(900_001),
                    None,
                    0,
                )],
                keywords: Vec::new(),
                videos: vec![video("filled", 0)],
                pictures: vec![
                    (item.images[0].url.clone(), Some("eng".into()), said),
                    (item.images[1].url.clone(), Some("eng".into()), said),
                    ("https://elsewhere/none.jpg".into(), None, said),
                ],
                jobs: vec![(
                    "52fe4726c3a36847f812048b".into(),
                    "Acting".into(),
                    "Actor".into(),
                )],
            },
        )
        .await
        .expect("filled in");

        let mut read = get(&db, &item.id).await.unwrap().expect("the work");
        load_children(&db, &mut read).await.unwrap();
        let language = |url: &str| {
            read.images
                .iter()
                .find(|i| i.url == url)
                .and_then(|i| i.language.clone())
        };
        assert_eq!(language(&item.images[0].url).as_deref(), Some("eng"));
        assert_eq!(
            language(&item.images[1].url).as_deref(),
            Some("fra"),
            "a language held stands"
        );
        assert_eq!(read.image_details[&item.images[1].url], said);
        assert_eq!(read.companies.len(), 1);
        assert_eq!(read.videos.len(), 1);
        assert_eq!(read.credits[0].job.as_deref(), Some("Actor"));
    }
}
