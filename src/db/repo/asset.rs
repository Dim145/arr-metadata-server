//! The media kept: a row per address a work points at, or per upload.
//!
//! The row is the record; the bytes are in the store, under `key`. A work's
//! own rows keep the provider's addresses — those are what a merge compares,
//! a refresh puts back and a provenance names — and this table says which of
//! them are here. Nothing here says which work an address belongs to: the
//! same photograph of an actor is one row however many casts name it, and
//! the references are found by the addresses themselves.

use std::collections::HashSet;

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;

use crate::db::{Db, RowExt, new_id, now};

/// What an asset is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Audio,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "audio" {
            Self::Audio
        } else {
            Self::Image
        }
    }
}

/// The thumbnail made of a picture, if any: a JPEG, or a PNG for a picture
/// that is transparent somewhere — a logo. Kept as a small number in the
/// row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Thumb {
    #[default]
    None,
    Jpeg,
    Png,
}

impl Thumb {
    pub const fn as_i64(self) -> i64 {
        match self {
            Self::None => 0,
            Self::Jpeg => 1,
            Self::Png => 2,
        }
    }

    pub const fn from_i64(n: i64) -> Self {
        match n {
            1 => Self::Jpeg,
            2 => Self::Png,
            _ => Self::None,
        }
    }

    pub const fn ext(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Jpeg => Some("jpg"),
            Self::Png => Some("png"),
        }
    }

    pub const fn content_type(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Jpeg => Some("image/jpeg"),
            Self::Png => Some("image/png"),
        }
    }
}

/// Where an asset stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting to be fetched, or to be tried again.
    Pending,
    Stored,
    /// Given up on, until somebody asks again.
    Failed,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Stored => "stored",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "stored" => Self::Stored,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    /// The address it was fetched from, or `upload:<id>` for an upload.
    pub origin: String,
    pub kind: Kind,
    pub status: Status,
    /// Where the bytes are, once stored: `<sha256>.<ext>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    pub has_thumb: bool,
    #[serde(skip)]
    pub thumb: Thumb,
    pub attempts: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<String>,
    /// The work that wanted it first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wanted_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_by: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_at: Option<String>,
}

const COLUMNS: &str = "id, origin, kind, status, key, content_type, bytes, sha256, width, height, \
     has_thumb, attempts, error, next_attempt_at, wanted_by, uploaded_by, created_at, stored_at";

fn map(row: &sqlx::any::AnyRow) -> Result<Asset> {
    Ok(Asset {
        id: row.text("id")?,
        origin: row.text("origin")?,
        kind: Kind::parse(&row.text("kind")?),
        status: Status::parse(&row.text("status")?),
        key: row.opt_text("key")?,
        content_type: row.opt_text("content_type")?,
        bytes: row.opt_big("bytes")?,
        sha256: row.opt_text("sha256")?,
        width: row.opt_int("width")?,
        height: row.opt_int("height")?,
        has_thumb: Thumb::from_i64(row.big("has_thumb")?) != Thumb::None,
        thumb: Thumb::from_i64(row.big("has_thumb")?),
        attempts: row.int("attempts")?,
        error: row.opt_text("error")?,
        next_attempt_at: row.opt_text("next_attempt_at")?,
        wanted_by: row.opt_text("wanted_by")?,
        uploaded_by: row.opt_text("uploaded_by")?,
        created_at: row.text("created_at")?,
        stored_at: row.opt_text("stored_at")?,
    })
}

/// An address to fetch, as a work wants it.
pub struct Wanted<'a> {
    pub origin: &'a str,
    pub kind: Kind,
    /// The work asking, when one is.
    pub wanted_by: Option<&'a str>,
}

/// Put addresses in line to be fetched. One already known — in line, here or
/// given up on — is left as it is; the number of new rows is returned.
pub async fn enqueue(db: &Db, wanted: &[Wanted<'_>]) -> Result<usize> {
    let mut added = 0;
    for want in wanted {
        let done = sqlx::query(db.sql(
            "INSERT INTO media_asset (id, origin, kind, status, wanted_by, created_at)
             VALUES (?, ?, ?, 'pending', ?, ?)
             ON CONFLICT (origin) DO NOTHING",
        ))
        .bind(new_id())
        .bind(want.origin)
        .bind(want.kind.as_str())
        .bind(want.wanted_by)
        .bind(now())
        .execute(db.pool())
        .await?;
        added += done.rows_affected() as usize;
    }
    Ok(added)
}

/// Every address a work's rows hold that no row here knows, kind by kind.
/// The people's photographs and the themes only when asked for.
pub async fn unknown_origins(db: &Db, people: bool, audio: bool) -> Result<Vec<(String, Kind)>> {
    let mut columns: Vec<(&str, &str, Kind)> = vec![
        ("media_image", "url", Kind::Image),
        ("media_episode", "image", Kind::Image),
        ("media_relation", "image", Kind::Image),
    ];
    if people {
        columns.push(("media_credit", "image", Kind::Image));
    }
    if audio {
        columns.push(("media_item", "theme_music", Kind::Audio));
    }

    let mut out = Vec::new();
    for (table, column, kind) in columns {
        let rows = sqlx::query(db.sql(&format!(
            "SELECT DISTINCT {column} AS origin FROM {table}
             WHERE {column} LIKE 'http%'
               AND NOT EXISTS (SELECT 1 FROM media_asset a WHERE a.origin = {table}.{column})"
        )))
        .fetch_all(db.pool())
        .await?;
        out.extend(
            rows.iter()
                .filter_map(|row| row.text("origin").ok())
                .map(|origin| (origin, kind)),
        );
    }
    Ok(out)
}

/// The next addresses to fetch: in line, and not put off until later.
pub async fn due(db: &Db, limit: i64) -> Result<Vec<Asset>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset
         WHERE status = 'pending' AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
         ORDER BY created_at ASC
         LIMIT ?"
    )))
    .bind(now())
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

/// How many are in line, put off or not.
pub async fn pending_count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_asset WHERE status = 'pending'"))
        .fetch_one(db.pool())
        .await?;
    Ok(row.big("n")?)
}

/// What was learned of the bytes when they were stored.
pub struct Stored<'a> {
    pub key: &'a str,
    pub content_type: &'a str,
    pub bytes: i64,
    pub sha256: &'a str,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub thumb: Thumb,
}

/// Whether the row was still there to mark: one forgotten while its bytes
/// were being fetched is not stored, whatever the store now holds.
pub async fn mark_stored(db: &Db, id: &str, stored: &Stored<'_>) -> Result<bool> {
    let done = sqlx::query(db.sql(
        "UPDATE media_asset
         SET status = 'stored', key = ?, content_type = ?, bytes = ?, sha256 = ?, width = ?,
             height = ?, has_thumb = ?, error = NULL, next_attempt_at = NULL, stored_at = ?
         WHERE id = ?",
    ))
    .bind(stored.key)
    .bind(stored.content_type)
    .bind(stored.bytes)
    .bind(stored.sha256)
    .bind(stored.width)
    .bind(stored.height)
    .bind(stored.thumb.as_i64())
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// A fetch that failed: tried again at `next_attempt_at`, or given up on
/// when there is no next time.
pub async fn mark_failed(
    db: &Db,
    id: &str,
    attempts: i32,
    error: &str,
    next_attempt_at: Option<&str>,
) -> Result<()> {
    let status = if next_attempt_at.is_some() {
        "pending"
    } else {
        "failed"
    };
    sqlx::query(db.sql(
        "UPDATE media_asset SET status = ?, attempts = ?, error = ?, next_attempt_at = ? WHERE id = ?",
    ))
    .bind(status)
    .bind(attempts)
    .bind(error)
    .bind(next_attempt_at)
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Tries past which an address answering with the wrong thing is filed:
/// one more than any fetch gets, so "try everything again" passes it by.
pub const UNFIT: i32 = 6;

/// Ask again for one given up on, or put off: fetched with the next batch.
pub async fn retry(db: &Db, id: &str) -> Result<bool> {
    let done = sqlx::query(db.sql(
        "UPDATE media_asset
         SET status = 'pending', attempts = 0, error = NULL, next_attempt_at = NULL
         WHERE id = ? AND status <> 'stored'",
    ))
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Ask again, now, for everything given up on or put off after a failure —
/// but not for what answered with the wrong thing, which would again.
pub async fn retry_troubled(db: &Db) -> Result<u64> {
    let done = sqlx::query(db.sql(
        "UPDATE media_asset
         SET status = 'pending', attempts = 0, error = NULL, next_attempt_at = NULL
         WHERE (status = 'failed' OR (status = 'pending' AND attempts > 0)) AND attempts < ?",
    ))
    .bind(UNFIT)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected())
}

/// An upload: stored the moment it is recorded.
pub struct Upload<'a> {
    pub id: &'a str,
    pub origin: &'a str,
    pub kind: Kind,
    pub stored: Stored<'a>,
    pub uploaded_by: &'a str,
    pub wanted_by: &'a str,
}

pub async fn insert_upload(db: &Db, upload: &Upload<'_>) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO media_asset
             (id, origin, kind, status, key, content_type, bytes, sha256, width, height, has_thumb,
              attempts, wanted_by, uploaded_by, created_at, stored_at)
         VALUES (?, ?, ?, 'stored', ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)",
    ))
    .bind(upload.id)
    .bind(upload.origin)
    .bind(upload.kind.as_str())
    .bind(upload.stored.key)
    .bind(upload.stored.content_type)
    .bind(upload.stored.bytes)
    .bind(upload.stored.sha256)
    .bind(upload.stored.width)
    .bind(upload.stored.height)
    .bind(upload.stored.thumb.as_i64())
    .bind(upload.wanted_by)
    .bind(upload.uploaded_by)
    .bind(now())
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn get(db: &Db, id: &str) -> Result<Option<Asset>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM media_asset WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    row.as_ref().map(map).transpose()
}

pub async fn by_origin(db: &Db, origin: &str) -> Result<Option<Asset>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset WHERE origin = ?"
    )))
    .bind(origin)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref().map(map).transpose()
}

/// The rows for these addresses, in no particular order.
pub async fn by_origins(db: &Db, origins: &[String]) -> Result<Vec<Asset>> {
    let mut out = Vec::new();
    // A series' stills run to a thousand: asked in slices a bind list takes.
    for slice in origins.chunks(400) {
        let marks = vec!["?"; slice.len()].join(", ");
        let mut query = sqlx::query(db.sql(&format!(
            "SELECT {COLUMNS} FROM media_asset WHERE origin IN ({marks})"
        )));
        for origin in slice {
            query = query.bind(origin);
        }
        let rows = query.fetch_all(db.pool()).await?;
        for row in &rows {
            out.push(map(row)?);
        }
    }
    Ok(out)
}

/// Everything stored, for the index kept in memory: origin, key, the
/// thumbnail made, and the content type.
pub async fn stored_index(db: &Db) -> Result<Vec<(String, String, Thumb, String)>> {
    let rows = sqlx::query(db.sql(
        "SELECT origin, key, has_thumb, content_type FROM media_asset
         WHERE status = 'stored' AND key IS NOT NULL",
    ))
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok((
                row.text("origin")?,
                row.text("key")?,
                Thumb::from_i64(row.big("has_thumb")?),
                row.opt_text("content_type")?
                    .unwrap_or_else(|| "application/octet-stream".into()),
            ))
        })
        .collect()
}

/// Whether these bytes are held by any row already, and the thumbnail made
/// of them: the same picture from two providers is put once.
pub async fn thumb_of_key(db: &Db, key: &str) -> Result<Option<Thumb>> {
    let row = sqlx::query(
        db.sql("SELECT has_thumb FROM media_asset WHERE key = ? AND status = 'stored' LIMIT 1"),
    )
    .bind(key)
    .fetch_optional(db.pool())
    .await?;
    Ok(row
        .map(|row| row.big("has_thumb"))
        .transpose()?
        .map(Thumb::from_i64))
}

/// Whether another row than `except` holds these bytes: the file is theirs
/// too, and stays.
pub async fn key_shared(db: &Db, key: &str, except: &str) -> Result<bool> {
    let row =
        sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_asset WHERE key = ? AND id <> ?"))
            .bind(key)
            .bind(except)
            .fetch_one(db.pool())
            .await?;
    Ok(row.big("n")? > 0)
}

pub async fn delete(db: &Db, id: &str) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_asset WHERE id = ?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(())
}

/// Forget every fetched row: what the store was moved from, or a start
/// over. The uploads stay — their row is the only record of them — and the
/// files are not touched; a sweep removes what no row names.
pub async fn delete_fetched(db: &Db) -> Result<u64> {
    let done = sqlx::query(db.sql("DELETE FROM media_asset WHERE origin NOT LIKE 'upload:%'"))
        .execute(db.pool())
        .await?;
    Ok(done.rows_affected())
}

/// Every key any row holds: what the store should hold, and no more.
pub async fn all_keys(db: &Db) -> Result<HashSet<String>> {
    let rows = sqlx::query(db.sql("SELECT key, has_thumb FROM media_asset WHERE key IS NOT NULL"))
        .fetch_all(db.pool())
        .await?;
    let mut keys = HashSet::with_capacity(rows.len() * 2);
    for row in &rows {
        let key = row.text("key")?;
        if let Some(thumb) = crate::media::thumb_key(&key, Thumb::from_i64(row.big("has_thumb")?)) {
            keys.insert(thumb);
        }
        keys.insert(key);
    }
    Ok(keys)
}

/// The rows no work points at any more: every column an address can sit in
/// is looked through, and the locked values a caller passes — those are
/// JSON, and read in Rust.
pub async fn unreferenced(db: &Db, locked: &HashSet<String>) -> Result<Vec<Asset>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset a
         WHERE NOT EXISTS (SELECT 1 FROM media_image i WHERE i.url = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_episode e WHERE e.image = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_credit c WHERE c.image = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_relation r WHERE r.image = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_item m WHERE m.theme_music = a.origin)"
    )))
    .fetch_all(db.pool())
    .await?;
    let mut out = Vec::new();
    for row in &rows {
        let asset = map(row)?;
        if !locked.contains(&asset.origin) {
            out.push(asset);
        }
    }
    Ok(out)
}

/// The tallies the media page opens on.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub pending: i64,
    pub stored: i64,
    pub failed: i64,
    /// Of what is stored, in bytes.
    pub bytes: i64,
}

pub async fn counts(db: &Db) -> Result<Counts> {
    let rows = sqlx::query(db.sql(
        // Cast, or PostgreSQL sums a BIGINT into a NUMERIC the driver cannot
        // read.
        "SELECT status, COUNT(*) AS n, CAST(COALESCE(SUM(bytes), 0) AS BIGINT) AS b
         FROM media_asset GROUP BY status",
    ))
    .fetch_all(db.pool())
    .await?;
    let mut counts = Counts::default();
    for row in &rows {
        let n = row.big("n")?;
        match row.text("status")?.as_str() {
            "pending" => counts.pending = n,
            "stored" => {
                counts.stored = n;
                counts.bytes = row.big("b")?;
            }
            "failed" => counts.failed = n,
            _ => {}
        }
    }
    Ok(counts)
}

/// Those given up on, and those put off after a failure, newest failure
/// first: what the media page lists to be looked at.
pub async fn troubled(db: &Db, limit: i64) -> Result<Vec<Asset>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset
         WHERE status = 'failed' OR (status = 'pending' AND attempts > 0)
         ORDER BY attempts DESC, created_at DESC
         LIMIT ?"
    )))
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> Db {
        let db = Db::connect(&crate::config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .unwrap();
        db.migrate().await.unwrap();
        db
    }

    #[tokio::test]
    async fn an_address_is_put_in_line_once_and_fetched_in_order() {
        let db = db().await;
        let wanted = [
            Wanted {
                origin: "https://a/1.jpg",
                kind: Kind::Image,
                wanted_by: Some("w1"),
            },
            Wanted {
                origin: "https://a/1.jpg",
                kind: Kind::Image,
                wanted_by: Some("w2"),
            },
            Wanted {
                origin: "https://a/t.mp3",
                kind: Kind::Audio,
                wanted_by: None,
            },
        ];
        assert_eq!(enqueue(&db, &wanted).await.unwrap(), 2);

        let line = due(&db, 10).await.unwrap();
        assert_eq!(line.len(), 2);
        assert_eq!(line[0].origin, "https://a/1.jpg");
        assert_eq!(
            line[0].wanted_by.as_deref(),
            Some("w1"),
            "the first to ask is kept"
        );
        assert_eq!(line[1].kind, Kind::Audio);

        // Put off: not due until then.
        mark_failed(
            &db,
            &line[0].id,
            1,
            "timed out",
            Some("2999-01-01T00:00:00Z"),
        )
        .await
        .unwrap();
        assert_eq!(due(&db, 10).await.unwrap().len(), 1);
        assert_eq!(pending_count(&db).await.unwrap(), 2);

        // Given up on: listed among the troubled, asked again on request.
        mark_failed(&db, &line[0].id, 5, "gone", None)
            .await
            .unwrap();
        assert_eq!(troubled(&db, 10).await.unwrap()[0].status, Status::Failed);
        assert!(retry(&db, &line[0].id).await.unwrap());
        assert_eq!(due(&db, 10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_stored_row_is_indexed_and_counted() {
        let db = db().await;
        enqueue(
            &db,
            &[Wanted {
                origin: "https://a/1.jpg",
                kind: Kind::Image,
                wanted_by: None,
            }],
        )
        .await
        .unwrap();
        let row = due(&db, 1).await.unwrap().remove(0);
        mark_stored(
            &db,
            &row.id,
            &Stored {
                key: "ab.jpg",
                content_type: "image/jpeg",
                bytes: 10,
                sha256: "ab",
                width: Some(2),
                height: Some(3),
                thumb: Thumb::Jpeg,
            },
        )
        .await
        .unwrap();

        let index = stored_index(&db).await.unwrap();
        assert_eq!(
            index,
            vec![(
                "https://a/1.jpg".into(),
                "ab.jpg".into(),
                Thumb::Jpeg,
                "image/jpeg".into()
            )]
        );
        let counts = counts(&db).await.unwrap();
        assert_eq!((counts.stored, counts.bytes, counts.pending), (1, 10, 0));
        let keys = all_keys(&db).await.unwrap();
        assert!(keys.contains("ab.jpg") && keys.contains("ab-t.jpg"));
        assert!(!key_shared(&db, "ab.jpg", &row.id).await.unwrap());
        assert!(
            !retry(&db, &row.id).await.unwrap(),
            "stored is not asked again"
        );
    }

    #[tokio::test]
    async fn a_row_nobody_points_at_is_unreferenced() {
        let db = db().await;
        enqueue(
            &db,
            &[
                Wanted {
                    origin: "https://a/kept.jpg",
                    kind: Kind::Image,
                    wanted_by: None,
                },
                Wanted {
                    origin: "https://a/lost.jpg",
                    kind: Kind::Image,
                    wanted_by: None,
                },
                Wanted {
                    origin: "https://a/locked.jpg",
                    kind: Kind::Image,
                    wanted_by: None,
                },
            ],
        )
        .await
        .unwrap();
        // A work holding the first, as its poster.
        let mut item = crate::domain::MediaItem::empty(crate::domain::MediaKind::Movie);
        item.title = "A".into();
        item.images.push(crate::domain::Image {
            id: new_id(),
            season_number: None,
            cover_type: crate::domain::CoverType::Poster,
            url: "https://a/kept.jpg".into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        });
        crate::db::repo::item::upsert(
            &db,
            crate::db::repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();

        let locked: HashSet<String> = ["https://a/locked.jpg".to_string()].into();
        let lost = unreferenced(&db, &locked).await.unwrap();
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].origin, "https://a/lost.jpg");

        let unknown = unknown_origins(&db, true, true).await.unwrap();
        assert!(unknown.is_empty(), "the poster is known already");
    }
}
