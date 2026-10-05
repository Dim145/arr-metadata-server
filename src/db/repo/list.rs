//! Curated lists: named selections of works, composed by hand in an order or
//! by a filter kept and evaluated as the list is read.
//!
//! Shown on the site as the catalogue's own selections, and served to Sonarr
//! and Radarr in the shape their "custom list" import lists read — so a
//! selection made here becomes what those clients add on their own.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::db::{Db, RowExt, from_bool, new_id, now};

/// What a list holds, and so which client reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ListKind {
    Series,
    Movie,
    #[default]
    Mixed,
}

impl ListKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Series => "series",
            Self::Movie => "movie",
            Self::Mixed => "mixed",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "series" => Self::Series,
            "movie" => Self::Movie,
            _ => Self::Mixed,
        }
    }
}

/// How a list is composed: by hand, in an order, or by a filter evaluated as
/// it is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ListMode {
    #[default]
    Manual,
    Filter,
}

impl ListMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Filter => "filter",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "filter" => Self::Filter,
            _ => Self::Manual,
        }
    }
}

/// A filter kept with a list, in the native list query's own vocabulary.
/// The kind is the list's own and is not repeated here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ListFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    /// Works carrying every one of these genres.
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year_from: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year_to: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    /// A network or a studio, by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<i64>,
    /// A score out of ten the works must reach.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_rating: Option<f64>,
    /// `popularity`, `rating`, `release`, `title` or `added`; popularity when
    /// absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// The sort's own direction when absent — and absent, not null, when
    /// written out, so a form reads it back as the same thing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descending: Option<bool>,
    /// How many works at most; a hundred when absent, five hundred at most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CuratedList {
    pub id: String,
    /// What the list is addressed by, derived from its name.
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: ListKind,
    pub mode: ListMode,
    /// The filter, for a list composed by one.
    pub filter: Option<ListFilter>,
    /// Whether a reader with no credential may see it, where public browsing
    /// is on at all.
    pub is_public: bool,
    /// How many works a hand-made list holds, of those switched on; a
    /// filter's are counted as they are read, and this is nought.
    pub item_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// What a list is made of, on creation and on every change.
pub struct ListFields<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub kind: ListKind,
    pub mode: ListMode,
    pub filter: Option<&'a ListFilter>,
    pub is_public: bool,
}

const COLUMNS: &str = "l.id, l.slug, l.name, l.description, l.kind, l.mode, l.filter_json,
                       l.is_public, l.created_at, l.updated_at,
                       CASE WHEN l.mode = 'manual'
                            THEN (SELECT COUNT(*) FROM curated_list_item i
                                    JOIN media_item m ON m.id = i.media_id AND m.is_enabled = 1
                                   WHERE i.list_id = l.id)
                            ELSE 0 END AS item_count";

/// Create a list, with its members when given, in one transaction: a failure
/// between the two left a list without the works it was made with.
pub async fn create(
    db: &Db,
    fields: ListFields<'_>,
    members: Option<&[String]>,
) -> Result<CuratedList> {
    let id = new_id();
    let at = now();
    let filter_json = fields.filter.map(serde_json::to_string).transpose()?;
    let mut tx = db.begin_write().await?;

    sqlx::query(db.sql(
        "INSERT INTO curated_list
             (id, slug, name, description, kind, mode, filter_json, is_public, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(fields.slug)
    .bind(fields.name)
    .bind(fields.description)
    .bind(fields.kind.as_str())
    .bind(fields.mode.as_str())
    .bind(&filter_json)
    .bind(from_bool(fields.is_public))
    .bind(&at)
    .bind(&at)
    .execute(&mut *tx)
    .await
    .context("failed to create the list")?;

    if let Some(members) = members {
        write_members(db, &mut tx, &id, members).await?;
    }

    tx.commit().await.context("failed to commit the new list")?;

    get(db, &id)
        .await?
        .context("the list just created is not there")
}

/// Replace what a list is made of, and its members when given, in one
/// transaction. `false` when there is no such list, and nothing is written.
pub async fn update(
    db: &Db,
    id: &str,
    fields: ListFields<'_>,
    members: Option<&[String]>,
) -> Result<bool> {
    let filter_json = fields.filter.map(serde_json::to_string).transpose()?;
    let mut tx = db.begin_write().await?;

    let result = sqlx::query(db.sql(
        "UPDATE curated_list
            SET slug = ?, name = ?, description = ?, kind = ?, mode = ?, filter_json = ?,
                is_public = ?, updated_at = ?
          WHERE id = ?",
    ))
    .bind(fields.slug)
    .bind(fields.name)
    .bind(fields.description)
    .bind(fields.kind.as_str())
    .bind(fields.mode.as_str())
    .bind(&filter_json)
    .bind(from_bool(fields.is_public))
    .bind(now())
    .bind(id)
    .execute(&mut *tx)
    .await
    .context("failed to update the list")?;

    if result.rows_affected() == 0 {
        return Ok(false);
    }
    if let Some(members) = members {
        write_members(db, &mut tx, id, members).await?;
    }

    tx.commit()
        .await
        .context("failed to commit the list's change")?;
    Ok(true)
}

pub async fn delete(db: &Db, id: &str) -> Result<bool> {
    let mut tx = db.begin_write().await?;

    // Its members first: the cascade is there on both engines, and is not
    // relied on.
    sqlx::query(db.sql("DELETE FROM curated_list_item WHERE list_id = ?"))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query(db.sql("DELETE FROM curated_list WHERE id = ?"))
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await.context("failed to commit the deletion")?;
    Ok(result.rows_affected() > 0)
}

/// Every list, by name — the public ones only, for a reader who is not
/// maintaining them.
pub async fn list(db: &Db, public_only: bool) -> Result<Vec<CuratedList>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM curated_list l {} ORDER BY l.name, l.created_at",
        if public_only {
            "WHERE l.is_public = 1"
        } else {
            ""
        }
    );
    let rows = sqlx::query(db.sql(&sql)).fetch_all(db.pool()).await?;
    rows.iter().map(map).collect()
}

pub async fn get(db: &Db, id: &str) -> Result<Option<CuratedList>> {
    let sql = format!("SELECT {COLUMNS} FROM curated_list l WHERE l.id = ?");
    let row = sqlx::query(db.sql(&sql))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

pub async fn by_slug(db: &Db, slug: &str) -> Result<Option<CuratedList>> {
    let sql = format!("SELECT {COLUMNS} FROM curated_list l WHERE l.slug = ?");
    let row = sqlx::query(db.sql(&sql))
        .bind(slug)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

/// The members of a hand-made list, in its order.
pub async fn member_ids(db: &Db, list_id: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(db.sql(
        "SELECT media_id FROM curated_list_item WHERE list_id = ? ORDER BY position, added_at",
    ))
    .bind(list_id)
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(|row| Ok(row.text("media_id")?)).collect()
}

/// Replace the members of a list with these, in this order. A work named
/// twice is kept once, where it first appears.
pub async fn set_members(db: &Db, list_id: &str, media_ids: &[String]) -> Result<()> {
    let mut tx = db.begin_write().await?;
    write_members(db, &mut tx, list_id, media_ids).await?;
    tx.commit()
        .await
        .context("failed to commit the list's members")
}

/// [`set_members`], inside the transaction that writes the list.
async fn write_members(
    db: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    list_id: &str,
    media_ids: &[String],
) -> Result<()> {
    let at = now();

    sqlx::query(db.sql("DELETE FROM curated_list_item WHERE list_id = ?"))
        .bind(list_id)
        .execute(&mut **tx)
        .await?;

    let mut seen = std::collections::HashSet::new();
    let mut position: i64 = 0;
    for media_id in media_ids {
        if !seen.insert(media_id.as_str()) {
            continue;
        }
        sqlx::query(db.sql(
            "INSERT INTO curated_list_item (list_id, media_id, position, added_at)
             VALUES (?, ?, ?, ?)",
        ))
        .bind(list_id)
        .bind(media_id)
        .bind(position)
        .bind(&at)
        .execute(&mut **tx)
        .await?;
        position += 1;
    }

    sqlx::query(db.sql("UPDATE curated_list SET updated_at = ? WHERE id = ?"))
        .bind(&at)
        .bind(list_id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

/// Count the hand-made lists' members again as a reader who may not be
/// shown a work for adults sees them: of those switched on and not for
/// adults. The count otherwise is every member switched on, which told such
/// a reader how many adult works a list holds.
pub async fn count_without_adult(db: &Db, lists: &mut [CuratedList]) -> Result<()> {
    let manual: Vec<String> = lists
        .iter()
        .filter(|l| l.mode == ListMode::Manual)
        .map(|l| l.id.clone())
        .collect();
    let mut counts = std::collections::HashMap::new();

    for chunk in manual.chunks(400) {
        let sql = format!(
            "SELECT i.list_id, COUNT(*) AS n FROM curated_list_item i
               JOIN media_item m ON m.id = i.media_id AND m.is_enabled = 1 AND m.is_adult = 0
              WHERE i.list_id IN ({})
              GROUP BY i.list_id",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut query = sqlx::query(db.sql(&sql));
        for id in chunk {
            query = query.bind(id);
        }
        for row in query.fetch_all(db.pool()).await? {
            counts.insert(row.text("list_id")?, row.big("n")?);
        }
    }

    for list in lists.iter_mut().filter(|l| l.mode == ListMode::Manual) {
        list.item_count = counts.get(&list.id).copied().unwrap_or(0);
    }
    Ok(())
}

/// The hand-made lists holding a work, by name. A list composed by a filter
/// is not asked: what it holds is decided as it is read.
pub async fn holding(db: &Db, media_id: &str, public_only: bool) -> Result<Vec<CuratedList>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM curated_list l
           JOIN curated_list_item i ON i.list_id = l.id
          WHERE i.media_id = ? AND l.mode = 'manual' {}
          ORDER BY l.name, l.created_at",
        if public_only {
            "AND l.is_public = 1"
        } else {
            ""
        }
    );
    let rows = sqlx::query(db.sql(&sql))
        .bind(media_id)
        .fetch_all(db.pool())
        .await?;
    rows.iter().map(map).collect()
}

fn map(row: &sqlx::any::AnyRow) -> Result<CuratedList> {
    let id = row.text("id")?;
    // A filter that no longer reads — the shape moved under it — is an empty
    // one rather than a listing that fails for every reader.
    let filter = row
        .opt_text("filter_json")?
        .filter(|json| !json.trim().is_empty())
        .map(|json| match serde_json::from_str::<ListFilter>(&json) {
            Ok(filter) => filter,
            Err(e) => {
                tracing::warn!(list = %id, error = %e, "a list's filter is not the shape it was written in");
                ListFilter::default()
            }
        });
    Ok(CuratedList {
        id,
        slug: row.text("slug")?,
        name: row.text("name")?,
        description: row.opt_text("description")?,
        kind: ListKind::parse(&row.text("kind")?),
        mode: ListMode::parse(&row.text("mode")?),
        filter,
        is_public: row.flag("is_public")?,
        item_count: row.big("item_count")?,
        created_at: row.text("created_at")?,
        updated_at: row.text("updated_at")?,
    })
}
