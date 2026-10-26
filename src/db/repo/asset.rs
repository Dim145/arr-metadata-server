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

use crate::{
    db::{Db, RowExt, new_id, now},
    domain::CoverType,
};

/// What an asset is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: what a setting holds is a `Kind` too, and utoipa keys
// schemas on the leaf name alone.
#[schema(as = AssetKind)]
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
}

/// Where an asset stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: an account, a delivery and the answer of a Sonarr or a
// Radarr each have a `Status` too, and utoipa keys schemas on the leaf name
// alone.
#[schema(as = AssetStatus)]
pub enum Status {
    /// Waiting to be fetched, or to be tried again.
    Pending,
    Stored,
    /// Given up on, until somebody asks again.
    Failed,
}

impl Status {
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
    /// The colour most of a picture is, `#rrggbb`, once worked out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// A picture's ThumbHash, base64, once worked out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbhash: Option<String>,
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
     has_thumb, color, thumbhash, attempts, error, next_attempt_at, wanted_by, uploaded_by, \
     created_at, stored_at";

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
        color: row.opt_text("color")?,
        thumbhash: row.opt_text("thumbhash")?,
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

/// A claim older than this is a worker that died mid-fetch: the medium is
/// anybody's again.
const CLAIM_STALE: chrono::Duration = chrono::Duration::minutes(10);

fn claim_cutoff() -> String {
    crate::db::to_rfc3339(chrono::Utc::now() - CLAIM_STALE)
}

/// The next addresses to fetch: in line, not put off until later, and not
/// claimed lately by a worker — this instance's or another's.
pub async fn due(db: &Db, limit: i64) -> Result<Vec<Asset>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset
         WHERE status = 'pending' AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
           AND (claimed_at IS NULL OR claimed_at < ?)
         ORDER BY created_at ASC
         LIMIT ?"
    )))
    .bind(now())
    .bind(claim_cutoff())
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

/// What a fetch that never came back is noted as, when its claim is taken
/// over: the server stopped — was stopped — in the middle of it.
pub const CUT_SHORT: &str = "the last fetch was cut short";
/// What one is given up on with, once every try it had was cut short.
pub const GIVEN_UP: &str = "every fetch of it was cut short";

/// What came of asking for one in line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// This worker's to fetch, and counted as a try.
    Taken,
    /// Another's, or no longer in line.
    Lost,
    /// Every try it had was taken and none came back — the server died
    /// fetching it, each time: given up on, and filed past every try.
    GivenUp,
}

/// Take one in line to fetch it: whether it was this worker's to take —
/// still in line, and not claimed lately by another. One statement, so two
/// workers asking at once are answered yes once.
///
/// The try is counted here, before a byte is fetched, and the next one put
/// off until `retry_at`: a picture that brings the server down as it is
/// decoded would otherwise be taken again at every start, its count never
/// moving. One whose `max_attempts` tries were all taken and never came back
/// is given up on instead.
pub async fn claim(
    db: &Db,
    id: &str,
    by: &str,
    max_attempts: i32,
    retry_at: &str,
) -> Result<Claim> {
    let given_up = sqlx::query(db.sql(
        "UPDATE media_asset
         SET status = 'failed', attempts = ?, error = ?, next_attempt_at = NULL,
             claimed_by = NULL, claimed_at = NULL
         WHERE id = ? AND status = 'pending' AND attempts >= ?
           AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
           AND (claimed_at IS NULL OR claimed_at < ?)",
    ))
    .bind(UNFIT)
    .bind(GIVEN_UP)
    .bind(id)
    .bind(max_attempts)
    .bind(now())
    .bind(claim_cutoff())
    .execute(db.pool())
    .await?;
    if given_up.rows_affected() > 0 {
        return Ok(Claim::GivenUp);
    }

    // A claim still standing is a fetch that never came back: said so on
    // the row, where the administration lists what went wrong.
    let taken = sqlx::query(db.sql(
        "UPDATE media_asset
         SET claimed_by = ?, claimed_at = ?, attempts = attempts + 1, next_attempt_at = ?,
             error = CASE WHEN claimed_at IS NULL THEN error ELSE ? END
         WHERE id = ? AND status = 'pending' AND attempts < ?
           AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
           AND (claimed_at IS NULL OR claimed_at < ?)",
    ))
    .bind(by)
    .bind(now())
    .bind(retry_at)
    .bind(CUT_SHORT)
    .bind(id)
    .bind(max_attempts)
    .bind(now())
    .bind(claim_cutoff())
    .execute(db.pool())
    .await?;
    Ok(if taken.rows_affected() > 0 {
        Claim::Taken
    } else {
        Claim::Lost
    })
}

/// File the key a fetch's bytes will be kept under on its row, before they
/// are put: from then on the row names them, and nothing that deletes a
/// file nobody names — another row's removal, the sweep — takes them while
/// they are being written. Whether the row was still in line to take it.
pub async fn reserve_key(db: &Db, id: &str, key: &str) -> Result<bool> {
    let done =
        sqlx::query(db.sql("UPDATE media_asset SET key = ? WHERE id = ? AND status = 'pending'"))
            .bind(key)
            .bind(id)
            .execute(db.pool())
            .await?;
    Ok(done.rows_affected() > 0)
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
    /// A picture's placeholder: its colour, `#rrggbb`, and its ThumbHash,
    /// base64 — none for a sound, or a picture that could not be read.
    pub color: Option<&'a str>,
    pub thumbhash: Option<&'a str>,
    /// What version of the server prepared it: 0 for nothing done.
    pub prepared: i64,
}

/// Whether the row was still there to mark: one forgotten while its bytes
/// were being fetched is not stored, whatever the store now holds.
pub async fn mark_stored(db: &Db, id: &str, stored: &Stored<'_>) -> Result<bool> {
    let done = sqlx::query(db.sql(
        "UPDATE media_asset
         SET status = 'stored', key = ?, content_type = ?, bytes = ?, sha256 = ?, width = ?,
             height = ?, has_thumb = ?, color = ?, thumbhash = ?, prepared = ?, error = NULL,
             next_attempt_at = NULL, stored_at = ?, claimed_by = NULL, claimed_at = NULL
         WHERE id = ?",
    ))
    .bind(stored.key)
    .bind(stored.content_type)
    .bind(stored.bytes)
    .bind(stored.sha256)
    .bind(stored.width)
    .bind(stored.height)
    .bind(stored.thumb.as_i64())
    .bind(stored.color)
    .bind(stored.thumbhash)
    .bind(stored.prepared)
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// A fetch that failed: tried again at `next_attempt_at`, or given up on
/// when there is no next time — or, `unfit`, filed past every try, since
/// asking again would get the same answer. The try itself was counted when
/// it was claimed. The key it may have reserved goes: nothing was kept
/// under it.
pub async fn mark_failed(
    db: &Db,
    id: &str,
    error: &str,
    next_attempt_at: Option<&str>,
    unfit: bool,
) -> Result<()> {
    if unfit {
        sqlx::query(db.sql(
            "UPDATE media_asset
             SET status = 'failed', attempts = ?, error = ?, next_attempt_at = NULL, key = NULL,
                 claimed_by = NULL, claimed_at = NULL
             WHERE id = ? AND status = 'pending'",
        ))
        .bind(UNFIT)
        .bind(error)
        .bind(id)
        .execute(db.pool())
        .await?;
    } else {
        let status = if next_attempt_at.is_some() {
            "pending"
        } else {
            "failed"
        };
        sqlx::query(db.sql(
            "UPDATE media_asset
             SET status = ?, error = ?, next_attempt_at = ?, key = NULL,
                 claimed_by = NULL, claimed_at = NULL
             WHERE id = ? AND status = 'pending'",
        ))
        .bind(status)
        .bind(error)
        .bind(next_attempt_at)
        .bind(id)
        .execute(db.pool())
        .await?;
    }
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
/// but not for what answered with the wrong thing, which would again, nor
/// for what brought the server down each time it was fetched.
pub async fn retry_troubled(db: &Db) -> Result<u64> {
    let done = sqlx::query(db.sql(
        "UPDATE media_asset
         SET status = 'pending', attempts = 0, error = NULL, next_attempt_at = NULL
         WHERE (status = 'failed' OR (status = 'pending' AND error IS NOT NULL)) AND attempts < ?",
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
              color, thumbhash, prepared, attempts, wanted_by, uploaded_by, created_at, stored_at)
         VALUES (?, ?, ?, 'stored', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)",
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
    .bind(upload.stored.color)
    .bind(upload.stored.thumbhash)
    .bind(upload.stored.prepared)
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

/// One asset stored, as the index kept in memory knows it: its origin, its
/// key, the thumbnail made, and what a page draws before the picture is
/// there. The content type is the key's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexed {
    pub origin: String,
    pub key: String,
    pub thumb: Thumb,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub color: Option<String>,
    pub thumbhash: Option<String>,
}

/// Everything stored: origin, key, and the thumbnail made. The content
/// type is the key's own.
pub async fn stored_index(db: &Db) -> Result<Vec<(String, String, Thumb)>> {
    Ok(indexed(db)
        .await?
        .into_iter()
        .map(|row| (row.origin, row.key, row.thumb))
        .collect())
}

/// Everything stored, for the index kept in memory.
pub async fn indexed(db: &Db) -> Result<Vec<Indexed>> {
    let rows = sqlx::query(db.sql(
        "SELECT origin, key, has_thumb, width, height, color, thumbhash FROM media_asset
         WHERE status = 'stored' AND key IS NOT NULL",
    ))
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Indexed {
                origin: row.text("origin")?,
                key: row.text("key")?,
                thumb: Thumb::from_i64(row.big("has_thumb")?),
                width: row.opt_int("width")?,
                height: row.opt_int("height")?,
                color: row.opt_text("color")?,
                thumbhash: row.opt_text("thumbhash")?,
            })
        })
        .collect()
}

/// Whether any row holds these bytes — stored, or in line with the key
/// reserved: the file is theirs, and stays.
pub async fn key_named(db: &Db, key: &str) -> Result<bool> {
    let row = sqlx::query(db.sql("SELECT 1 AS hit FROM media_asset WHERE key = ? LIMIT 1"))
        .bind(key)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.is_some())
}

/// Whether any row holds bytes of this hash, whatever their kind: their
/// file and its thumbnail stay.
pub async fn stem_named(db: &Db, stem: &str) -> Result<bool> {
    let keys = crate::media::file::keys_of_stem(stem);
    let marks = vec!["?"; keys.len()].join(", ");
    let mut query = sqlx::query(db.sql(&format!(
        "SELECT 1 AS hit FROM media_asset WHERE key IN ({marks}) LIMIT 1"
    )));
    for key in keys {
        query = query.bind(key);
    }
    Ok(query.fetch_optional(db.pool()).await?.is_some())
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

/// The hash of every key any row holds, stored or reserved: what the store
/// should hold — those files and their thumbnails — and no more.
pub async fn held_stems(db: &Db) -> Result<HashSet<String>> {
    let rows = sqlx::query(db.sql("SELECT key FROM media_asset WHERE key IS NOT NULL"))
        .fetch_all(db.pool())
        .await?;
    let mut stems = HashSet::with_capacity(rows.len());
    for row in &rows {
        stems.insert(crate::media::file::stem_of(&row.text("key")?).to_string());
    }
    Ok(stems)
}

/// Every place an address is named from: a work's pictures and its
/// seasons', an episode's still, a cast's photographs, a relation's poster,
/// a work's theme, a selection's cover — and any locked value, on any work,
/// that is the address itself, or with `by_path`, one that is the path of its
/// bytes here: a lock can name a picture another work holds, and one written
/// before locks were filed by their origin names it by its path.
fn naming(by_path: bool) -> String {
    let mut sql = String::from(
        "EXISTS (SELECT 1 FROM media_image i WHERE i.url = ?)
         OR EXISTS (SELECT 1 FROM media_episode e WHERE e.image = ?)
         OR EXISTS (SELECT 1 FROM media_credit c WHERE c.image = ?)
         OR EXISTS (SELECT 1 FROM media_relation r WHERE r.image = ?)
         OR EXISTS (SELECT 1 FROM media_item m WHERE m.theme_music = ?)
         OR EXISTS (SELECT 1 FROM curated_list l WHERE l.cover = ?)
         OR EXISTS (SELECT 1 FROM media_override o WHERE o.value = ?)",
    );
    if by_path {
        sql.push_str("\n         OR EXISTS (SELECT 1 FROM media_override o WHERE o.value LIKE ?)");
    }
    sql
}

/// What [`naming`] is asked with: the address, six times; as a locked
/// value holds it, in JSON; and the path of its bytes, when it has any.
fn naming_binds(origin: &str, key: Option<&str>) -> Result<Vec<String>> {
    let mut binds = vec![origin.to_string(); 6];
    binds.push(serde_json::to_string(origin)?);
    if let Some(key) = key {
        let stem = crate::media::file::stem_of(key);
        binds.push(format!("%{}{stem}%", crate::media::ROUTE));
    }
    Ok(binds)
}

/// Delete a row unless anything still names its address — the one check
/// made before an upload, or anything the sweep takes, is deleted: in the
/// same statement, so nothing slips in between the asking and the deleting.
/// Its key, when it has one, finds the locks that name it by the path of
/// its bytes. Whether it went.
pub async fn delete_unless_referenced(db: &Db, asset: &Asset) -> Result<bool> {
    let mut query = sqlx::query(db.sql(&format!(
        "DELETE FROM media_asset WHERE id = ? AND NOT ({})",
        naming(asset.key.is_some())
    )))
    .bind(asset.id.clone());
    for bind in naming_binds(&asset.origin, asset.key.as_deref())? {
        query = query.bind(bind);
    }
    Ok(query.execute(db.pool()).await?.rows_affected() > 0)
}

/// The rows no work points at any more, among those older than
/// `created_before` — an upload's row is written a moment before the
/// picture or the lock that points at it: every column an address can sit
/// in is looked through, a selection's cover among them, and the locked
/// values a caller passes — those are JSON, and read in Rust.
pub async fn unreferenced(
    db: &Db,
    locked: &HashSet<String>,
    created_before: &str,
) -> Result<Vec<Asset>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset a
         WHERE a.created_at < ?
           AND NOT EXISTS (SELECT 1 FROM media_image i WHERE i.url = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_episode e WHERE e.image = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_credit c WHERE c.image = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_relation r WHERE r.image = a.origin)
           AND NOT EXISTS (SELECT 1 FROM media_item m WHERE m.theme_music = a.origin)
           AND NOT EXISTS (SELECT 1 FROM curated_list l WHERE l.cover = a.origin)"
    )))
    .bind(created_before)
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
// Named explicitly: the accounts and the deliveries are counted in a `Counts`
// too, and utoipa keys schemas on the leaf name alone.
#[schema(as = AssetCounts)]
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

// ─── by work ─────────────────────────────────────────────────────────────────

/// Every address a work points at, once per work: its pictures and its
/// seasons', its episodes' stills, its cast's portraits, its relations'
/// covers, its theme — what the work's own media list shows. A picture
/// shared by two works is each one's.
const WORK_ADDRESSES: &str = "
    SELECT media_id, url AS origin FROM media_image
    UNION SELECT media_id, image FROM media_episode WHERE image IS NOT NULL
    UNION SELECT media_id, image FROM media_credit WHERE image IS NOT NULL
    UNION SELECT media_id, image FROM media_relation WHERE image IS NOT NULL
    UNION SELECT id, theme_music FROM media_item WHERE theme_music IS NOT NULL";

/// What one work's media come to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkMediaTotals)]
pub struct WorkTotals {
    /// The media kept for it.
    pub stored: i64,
    /// Waiting to be fetched, or tried again.
    pub pending: i64,
    /// Given up on.
    pub failed: i64,
    /// What the media kept weigh, in bytes.
    pub bytes: i64,
    /// What the copies made of its pictures weigh: the sizes the pages are
    /// given, the night's AVIF.
    pub copies_bytes: i64,
}

/// How works are ranked by their media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkOrder {
    /// The heaviest first, copies included.
    Space,
    /// The most media kept first.
    Count,
    /// The most media given up on first.
    Failed,
}

/// A work, and what its media come to.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkMediaLine)]
pub struct WorkLine {
    pub id: String,
    pub title: String,
    pub kind: crate::domain::MediaKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(flatten)]
    pub totals: WorkTotals,
}

/// The totals of each work that points at any medium, as one statement:
/// `{select}` from it, the rest of the statement after.
fn by_work_sql(tail: &str) -> String {
    format!(
        "WITH refs AS ({WORK_ADDRESSES}),
              copies AS (SELECT stem, SUM(bytes) AS bytes FROM media_variant GROUP BY stem),
              totals AS (
                SELECT r.media_id AS id,
                       SUM(CASE WHEN a.status = 'stored' THEN 1 ELSE 0 END) AS stored,
                       SUM(CASE WHEN a.status = 'pending' THEN 1 ELSE 0 END) AS pending,
                       SUM(CASE WHEN a.status = 'failed' THEN 1 ELSE 0 END) AS failed,
                       CAST(COALESCE(SUM(CASE WHEN a.status = 'stored' THEN a.bytes ELSE 0 END), 0) AS BIGINT) AS bytes,
                       CAST(COALESCE(SUM(CASE WHEN a.status = 'stored' THEN c.bytes ELSE 0 END), 0) AS BIGINT) AS copies
                  FROM refs r
                  JOIN media_asset a ON a.origin = r.origin
                  LEFT JOIN copies c ON c.stem = a.sha256
                 GROUP BY r.media_id)
         {tail}"
    )
}

fn totals_of(row: &sqlx::any::AnyRow) -> Result<WorkTotals> {
    Ok(WorkTotals {
        stored: row.big("stored")?,
        pending: row.big("pending")?,
        failed: row.big("failed")?,
        bytes: row.big("bytes")?,
        copies_bytes: row.big("copies")?,
    })
}

/// The works whose media weigh most — or that keep the most, or failed the
/// most — a page at a time, and how many works point at any medium. A work
/// merged into another is left out: its pictures are the survivor's.
pub async fn by_work(
    db: &Db,
    order: WorkOrder,
    limit: i64,
    offset: i64,
) -> Result<(Vec<WorkLine>, i64)> {
    let ordered = match order {
        WorkOrder::Space => "t.bytes + t.copies DESC, t.stored DESC",
        WorkOrder::Count => "t.stored DESC, t.bytes + t.copies DESC",
        WorkOrder::Failed => "t.failed DESC, t.pending DESC",
    };
    let rows = sqlx::query(db.sql(&by_work_sql(&format!(
        "SELECT t.id, t.stored, t.pending, t.failed, t.bytes, t.copies,
                m.title, m.kind, m.year
           FROM totals t
           JOIN media_item m ON m.id = t.id
          WHERE m.merged_into IS NULL
          ORDER BY {ordered}, m.title, t.id
          LIMIT ? OFFSET ?"
    ))))
    .bind(limit.clamp(1, 200))
    .bind(offset.max(0))
    .fetch_all(db.pool())
    .await?;
    let mut works = Vec::with_capacity(rows.len());
    for row in &rows {
        works.push(WorkLine {
            id: row.text("id")?,
            title: row.text("title")?,
            // Only ever written from a work's kind.
            kind: row
                .text("kind")?
                .parse()
                .unwrap_or(crate::domain::MediaKind::Movie),
            year: row.opt_int("year")?,
            totals: totals_of(row)?,
        });
    }
    let total = sqlx::query(db.sql(&by_work_sql(
        "SELECT COUNT(*) AS n FROM totals t
           JOIN media_item m ON m.id = t.id
          WHERE m.merged_into IS NULL",
    )))
    .fetch_one(db.pool())
    .await?
    .big("n")?;
    Ok((works, total))
}

/// What one work's media come to; nought of each for a work that points at
/// none.
pub async fn work_totals(db: &Db, id: &str) -> Result<WorkTotals> {
    // Its own rows only, each table by the work's index; each picture's
    // copies by the picture's hash.
    let row = sqlx::query(db.sql(
        "WITH refs AS (
            SELECT url AS origin FROM media_image WHERE media_id = ?
            UNION SELECT image FROM media_episode WHERE media_id = ? AND image IS NOT NULL
            UNION SELECT image FROM media_credit WHERE media_id = ? AND image IS NOT NULL
            UNION SELECT image FROM media_relation WHERE media_id = ? AND image IS NOT NULL
            UNION SELECT theme_music FROM media_item WHERE id = ? AND theme_music IS NOT NULL)
         SELECT COALESCE(SUM(CASE WHEN x.status = 'stored' THEN 1 ELSE 0 END), 0) AS stored,
                COALESCE(SUM(CASE WHEN x.status = 'pending' THEN 1 ELSE 0 END), 0) AS pending,
                COALESCE(SUM(CASE WHEN x.status = 'failed' THEN 1 ELSE 0 END), 0) AS failed,
                CAST(COALESCE(SUM(CASE WHEN x.status = 'stored' THEN x.bytes ELSE 0 END), 0) AS BIGINT) AS bytes,
                CAST(COALESCE(SUM(CASE WHEN x.status = 'stored' THEN x.copies ELSE 0 END), 0) AS BIGINT) AS copies
           FROM (SELECT a.status, a.bytes,
                        (SELECT COALESCE(SUM(v.bytes), 0) FROM media_variant v WHERE v.stem = a.sha256) AS copies
                   FROM refs r
                   JOIN media_asset a ON a.origin = r.origin) x",
    ))
    .bind(id)
    .bind(id)
    .bind(id)
    .bind(id)
    .bind(id)
    .fetch_one(db.pool())
    .await?;
    totals_of(&row)
}

/// Those given up on, and those put off after a failure or a fetch cut
/// short, newest failure first: what the media page lists to be looked at.
/// Not one being fetched for the first time, though its try is counted.
pub async fn troubled(db: &Db, limit: i64) -> Result<Vec<Asset>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset
         WHERE status = 'failed' OR (status = 'pending' AND error IS NOT NULL)
         ORDER BY attempts DESC, created_at DESC
         LIMIT ?"
    )))
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

/// What a work's rows have a picture as, by its address: a poster first,
/// then a background, then whatever else — what decides the copy made of
/// it as it is stored, and the night's. `None` for an address no work's
/// pictures hold: a still, a photograph, a lock's.
pub async fn cover_of(db: &Db, origin: &str) -> Result<Option<CoverType>> {
    let row = sqlx::query(db.sql(
        "SELECT cover_type FROM media_image WHERE url = ?
         ORDER BY CASE cover_type WHEN 'poster' THEN 0 WHEN 'keyart' THEN 0
                                  WHEN 'fanart' THEN 1 ELSE 2 END
         LIMIT 1",
    ))
    .bind(origin)
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| -> Result<CoverType> {
        Ok(row
            .text("cover_type")?
            .parse()
            .unwrap_or(CoverType::Unknown))
    })
    .transpose()
}

/// The next pictures stored that this version has not prepared — no
/// placeholder worked out, no first copy made — after `after` in the order
/// of their ids: what the task `media.variants` takes, a few at a time,
/// walking them once. Each one prepared leaves the list, so the next run
/// takes up where one stopped partway.
pub async fn unprepared(db: &Db, prepared: i64, after: &str, limit: i64) -> Result<Vec<Asset>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM media_asset
         WHERE status = 'stored' AND kind = 'image' AND key IS NOT NULL AND prepared < ?
           AND id > ?
         ORDER BY id
         LIMIT ?"
    )))
    .bind(prepared)
    .bind(after)
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map).collect()
}

/// How many pictures stored this version has not prepared: what the media
/// page says the task `media.variants` would do.
pub async fn unprepared_count(db: &Db, prepared: i64) -> Result<i64> {
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS n FROM media_asset
         WHERE status = 'stored' AND kind = 'image' AND key IS NOT NULL AND prepared < ?",
    ))
    .bind(prepared)
    .fetch_one(db.pool())
    .await?;
    Ok(row.big("n")?)
}

/// Note a picture prepared: its placeholder, when one could be worked out,
/// and the version that did it. Whether the row was still there.
pub async fn mark_prepared(
    db: &Db,
    id: &str,
    color: Option<&str>,
    thumbhash: Option<&str>,
    prepared: i64,
) -> Result<bool> {
    let done = sqlx::query(db.sql(
        "UPDATE media_asset SET color = ?, thumbhash = ?, prepared = ?
         WHERE id = ? AND status = 'stored'",
    ))
    .bind(color)
    .bind(thumbhash)
    .bind(prepared)
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// The pictures a night's AVIF copies are made of: those a work has for a
/// poster, a key art or a background, after `after` in the order of their
/// ids — the run walks them all once — with what they are.
pub async fn avif_candidates(db: &Db, after: &str, limit: i64) -> Result<Vec<(Asset, CoverType)>> {
    let rows = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS},
                (SELECT i.cover_type FROM media_image i
                  WHERE i.url = a.origin AND i.cover_type IN ('poster', 'keyart', 'fanart')
                  ORDER BY CASE i.cover_type WHEN 'fanart' THEN 1 ELSE 0 END
                  LIMIT 1) AS cover
         FROM media_asset a
         WHERE a.status = 'stored' AND a.kind = 'image' AND a.key IS NOT NULL AND a.id > ?
           AND EXISTS (SELECT 1 FROM media_image i
                        WHERE i.url = a.origin AND i.cover_type IN ('poster', 'keyart', 'fanart'))
         ORDER BY a.id
         LIMIT ?"
    )))
    .bind(after)
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            let cover = row
                .opt_text("cover")?
                .and_then(|c| c.parse().ok())
                .unwrap_or(CoverType::Unknown);
            Ok((map(row)?, cover))
        })
        .collect()
}

/// A copy made of a picture, as its row records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    /// `<sha256>-480.webp`: see [`crate::media::file::Made`].
    pub key: String,
    /// The picture's hash.
    pub stem: String,
    pub width: i32,
    pub height: i32,
    /// `webp`, `avif` or `jpeg`.
    pub format: String,
    pub bytes: i64,
}

/// Note a copy made. One noted already — made again, here or by another
/// instance — is left as it is.
pub async fn add_variant(db: &Db, variant: &Variant) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO media_variant (key, stem, width, height, format, bytes, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (key) DO NOTHING",
    ))
    .bind(&variant.key)
    .bind(&variant.stem)
    .bind(variant.width)
    .bind(variant.height)
    .bind(&variant.format)
    .bind(variant.bytes)
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// The copies made of a picture.
pub async fn variants_of(db: &Db, stem: &str) -> Result<Vec<Variant>> {
    let rows = sqlx::query(db.sql(
        "SELECT key, stem, width, height, format, bytes FROM media_variant
         WHERE stem = ? ORDER BY key",
    ))
    .bind(stem)
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map_variant).collect()
}

fn map_variant(row: &sqlx::any::AnyRow) -> Result<Variant> {
    Ok(Variant {
        key: row.text("key")?,
        stem: row.text("stem")?,
        width: row.int("width")?,
        height: row.int("height")?,
        format: row.text("format")?,
        bytes: row.big("bytes")?,
    })
}

/// Forget a copy: its file is gone.
pub async fn delete_variant(db: &Db, key: &str) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM media_variant WHERE key = ?"))
        .bind(key)
        .execute(db.pool())
        .await?;
    Ok(())
}

/// The copies written as `format` — `webp`, `avif`, `jpeg` — after `after`
/// in the order of their keys, `limit` of them at the most: what deleting
/// every copy of a format takes, a few hundred at a time, walking them once.
pub async fn variants_by_format(
    db: &Db,
    format: &str,
    after: &str,
    limit: i64,
) -> Result<Vec<Variant>> {
    let rows = sqlx::query(db.sql(
        "SELECT key, stem, width, height, format, bytes FROM media_variant
         WHERE format = ? AND key > ?
         ORDER BY key
         LIMIT ?",
    ))
    .bind(format)
    .bind(after)
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;
    rows.iter().map(map_variant).collect()
}

/// Forget these copies at once: their files are gone. How many were noted.
pub async fn delete_variants(db: &Db, keys: &[String]) -> Result<u64> {
    let mut deleted = 0;
    // Asked in slices a bind list takes, as `by_origins` asks.
    for slice in keys.chunks(400) {
        let marks = vec!["?"; slice.len()].join(", ");
        let mut query =
            sqlx::query(db.sql(&format!("DELETE FROM media_variant WHERE key IN ({marks})")));
        for key in slice {
            query = query.bind(key);
        }
        deleted += query.execute(db.pool()).await?.rows_affected();
    }
    Ok(deleted)
}

/// Every AVIF copy made, by its picture's hash and its width: what the
/// index in memory tells the pages, which offer one only where it exists.
pub async fn avif_index(db: &Db) -> Result<Vec<(String, u32)>> {
    let rows = sqlx::query(
        db.sql("SELECT stem, width FROM media_variant WHERE format = 'avif' ORDER BY stem, width"),
    )
    .fetch_all(db.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok((
                row.text("stem")?,
                u32::try_from(row.int("width")?).unwrap_or(0),
            ))
        })
        .collect()
}

/// The copies whose picture no row holds any more, among those noted
/// before `created_before`: the sweep's to take away, with their files.
pub async fn orphan_variants(
    db: &Db,
    held: &HashSet<String>,
    created_before: &str,
) -> Result<Vec<Variant>> {
    let rows = sqlx::query(db.sql(
        "SELECT key, stem, width, height, format, bytes FROM media_variant
         WHERE created_at < ? ORDER BY key",
    ))
    .bind(created_before)
    .fetch_all(db.pool())
    .await?;
    let mut out = Vec::new();
    for row in &rows {
        let variant = map_variant(row)?;
        if !held.contains(&variant.stem) {
            out.push(variant);
        }
    }
    Ok(out)
}

/// What the copies made weigh, by what they are written as.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VariantCounts {
    /// The WebP copies the interface is given.
    pub webp: i64,
    pub webp_bytes: i64,
    /// The AVIF copies the night's task made.
    pub avif: i64,
    pub avif_bytes: i64,
    /// The squares Plex is given.
    pub jpeg: i64,
    pub jpeg_bytes: i64,
}

pub async fn variant_counts(db: &Db) -> Result<VariantCounts> {
    let rows = sqlx::query(db.sql(
        // Cast, as the store's own tallies are: see `counts`.
        "SELECT format, COUNT(*) AS n, CAST(COALESCE(SUM(bytes), 0) AS BIGINT) AS b
         FROM media_variant GROUP BY format",
    ))
    .fetch_all(db.pool())
    .await?;
    let mut counts = VariantCounts::default();
    for row in &rows {
        let (n, b) = (row.big("n")?, row.big("b")?);
        match row.text("format")?.as_str() {
            "webp" => (counts.webp, counts.webp_bytes) = (n, b),
            "avif" => (counts.avif, counts.avif_bytes) = (n, b),
            "jpeg" => (counts.jpeg, counts.jpeg_bytes) = (n, b),
            _ => {}
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::db;

    const LATER: &str = "2999-01-01T00:00:00.000Z";
    const SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn wanted(origin: &str) -> Wanted<'_> {
        Wanted {
            origin,
            kind: Kind::Image,
            wanted_by: None,
        }
    }

    /// As if the claim had been made long ago, by a worker that never came
    /// back, and its try had come due.
    async fn abandon(db: &Db, id: &str) {
        sqlx::query(db.sql(
            "UPDATE media_asset
             SET claimed_at = '2000-01-01T00:00:00.000Z', next_attempt_at = NULL
             WHERE id = ?",
        ))
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn work_with(db: &Db, images: &[&str], theme: Option<&str>) -> String {
        let mut item = crate::domain::MediaItem::empty(crate::domain::MediaKind::Series);
        item.title = "A".into();
        item.slug = item.id.clone();
        item.theme_music = theme.map(str::to_string);
        for url in images {
            item.images.push(crate::domain::Image {
                id: new_id(),
                season_number: None,
                cover_type: crate::domain::CoverType::Poster,
                url: (*url).into(),
                language: None,
                sort_order: 0,
                // A provider's row: a hand-added one is written by its own
                // path, not with the work.
                source: Some("tmdb".into()),
                is_manual: false,
            });
        }
        crate::db::repo::item::upsert(
            db,
            crate::db::repo::item::ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .unwrap();
        item.id
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

        // Taken, and put off: not due until then.
        assert_eq!(
            claim(&db, &line[0].id, "me", 5, LATER).await.unwrap(),
            Claim::Taken
        );
        assert_eq!(
            claim(&db, &line[0].id, "you", 5, LATER).await.unwrap(),
            Claim::Lost,
            "taken once"
        );
        mark_failed(&db, &line[0].id, "timed out", Some(LATER), false)
            .await
            .unwrap();
        assert_eq!(due(&db, 10).await.unwrap().len(), 1);
        assert_eq!(pending_count(&db).await.unwrap(), 2);
        let troubled_now = troubled(&db, 10).await.unwrap();
        assert_eq!(troubled_now.len(), 1);
        assert_eq!(troubled_now[0].attempts, 1, "the try was counted once");

        // Given up on: listed among the troubled, asked again on request.
        mark_failed(&db, &line[0].id, "gone", None, false)
            .await
            .unwrap();
        assert_eq!(troubled(&db, 10).await.unwrap()[0].status, Status::Failed);
        assert!(retry(&db, &line[0].id).await.unwrap());
        assert_eq!(due(&db, 10).await.unwrap().len(), 2);
    }

    /// A try is counted as it is taken: a fetch that brings the server down
    /// counts all the same, and is given up on once every try it had was.
    #[tokio::test]
    async fn a_fetch_cut_short_too_often_is_given_up() {
        let db = db().await;
        enqueue(&db, &[wanted("https://a/poison.jpg")])
            .await
            .unwrap();
        let id = due(&db, 1).await.unwrap().remove(0).id;

        assert_eq!(claim(&db, &id, "me", 2, LATER).await.unwrap(), Claim::Taken);
        let row = get(&db, &id).await.unwrap().unwrap();
        assert_eq!((row.attempts, row.error.as_deref()), (1, None));
        assert!(
            troubled(&db, 10).await.unwrap().is_empty(),
            "a first fetch under way is no trouble"
        );

        // The server dies mid-fetch; the claim goes stale; another takes it.
        abandon(&db, &id).await;
        assert_eq!(claim(&db, &id, "me", 2, LATER).await.unwrap(), Claim::Taken);
        let row = get(&db, &id).await.unwrap().unwrap();
        assert_eq!((row.attempts, row.error.as_deref()), (2, Some(CUT_SHORT)));
        assert_eq!(troubled(&db, 10).await.unwrap().len(), 1);

        // And dies again: that was its last try.
        abandon(&db, &id).await;
        assert_eq!(
            claim(&db, &id, "me", 2, LATER).await.unwrap(),
            Claim::GivenUp
        );
        let row = get(&db, &id).await.unwrap().unwrap();
        assert_eq!(row.status, Status::Failed);
        assert_eq!(
            (row.attempts, row.error.as_deref()),
            (UNFIT, Some(GIVEN_UP))
        );
        assert_eq!(
            retry_troubled(&db).await.unwrap(),
            0,
            "not asked again with the rest"
        );
        assert!(due(&db, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_stored_row_is_indexed_and_counted() {
        let db = db().await;
        enqueue(&db, &[wanted("https://a/1.jpg")]).await.unwrap();
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
                color: Some("#14285a"),
                thumbhash: Some("1QcSHQRnh493V4dIh4eXh1h4kJUI"),
                prepared: 1,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            stored_index(&db).await.unwrap(),
            vec![("https://a/1.jpg".into(), "ab.jpg".into(), Thumb::Jpeg)]
        );
        let index = indexed(&db).await.unwrap();
        assert_eq!(
            index,
            vec![Indexed {
                origin: "https://a/1.jpg".into(),
                key: "ab.jpg".into(),
                thumb: Thumb::Jpeg,
                width: Some(2),
                height: Some(3),
                color: Some("#14285a".into()),
                thumbhash: Some("1QcSHQRnh493V4dIh4eXh1h4kJUI".into()),
            }]
        );
        assert_eq!(unprepared_count(&db, 1).await.unwrap(), 0);
        assert_eq!(
            unprepared_count(&db, 2).await.unwrap(),
            1,
            "prepared by an earlier version"
        );
        let counts = counts(&db).await.unwrap();
        assert_eq!((counts.stored, counts.bytes, counts.pending), (1, 10, 0));
        assert!(held_stems(&db).await.unwrap().contains("ab"));
        assert!(key_named(&db, "ab.jpg").await.unwrap());
        assert!(stem_named(&db, "ab").await.unwrap());
        assert!(!key_named(&db, "cd.jpg").await.unwrap());
        assert!(
            !retry(&db, &row.id).await.unwrap(),
            "stored is not asked again"
        );
        assert!(
            !reserve_key(&db, &row.id, "cd.jpg").await.unwrap(),
            "a stored row's key stays its own"
        );
    }

    /// A fetch files its key before it puts the bytes, so nothing deletes
    /// them as nobody's meanwhile; a failure takes the key back.
    #[tokio::test]
    async fn a_key_is_named_from_the_moment_it_is_reserved() {
        let db = db().await;
        enqueue(&db, &[wanted("https://a/1.jpg")]).await.unwrap();
        let id = due(&db, 1).await.unwrap().remove(0).id;
        let key = format!("{SHA}.jpg");

        assert!(!key_named(&db, &key).await.unwrap());
        assert!(reserve_key(&db, &id, &key).await.unwrap());
        assert!(key_named(&db, &key).await.unwrap());
        assert!(stem_named(&db, SHA).await.unwrap(), "and its thumbnail too");
        assert!(held_stems(&db).await.unwrap().contains(SHA));
        assert!(
            stored_index(&db).await.unwrap().is_empty(),
            "reserved is not stored"
        );

        mark_failed(&db, &id, "the address answered 503", Some(LATER), false)
            .await
            .unwrap();
        assert!(!key_named(&db, &key).await.unwrap());
        assert!(!stem_named(&db, SHA).await.unwrap());
    }

    #[tokio::test]
    async fn a_row_nobody_points_at_is_unreferenced() {
        let db = db().await;
        enqueue(
            &db,
            &[
                wanted("https://a/kept.jpg"),
                wanted("https://a/lost.jpg"),
                wanted("https://a/locked.jpg"),
            ],
        )
        .await
        .unwrap();
        // A work holding the first, as its poster.
        work_with(&db, &["https://a/kept.jpg"], None).await;

        let locked: HashSet<String> = ["https://a/locked.jpg".to_string()].into();
        let lost = unreferenced(&db, &locked, LATER).await.unwrap();
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].origin, "https://a/lost.jpg");
        assert!(
            unreferenced(&db, &locked, "2000-01-01T00:00:00.000Z")
                .await
                .unwrap()
                .is_empty(),
            "a row younger than the cutoff is left: its reference may be on its way"
        );

        let unknown = unknown_origins(&db, true, true).await.unwrap();
        assert!(unknown.is_empty(), "the poster is known already");
    }

    /// The one check made before an upload is deleted: a picture of any
    /// work, a theme, a selection's cover, a lock on any work — by the
    /// upload's own name, or by the path of its bytes here — keeps it; nothing
    /// left naming it, it goes.
    #[tokio::test]
    async fn an_upload_is_kept_while_anything_names_it() {
        use crate::{
            db::repo::{
                list::{self, ListFields, ListKind, ListMode},
                override_field,
            },
            domain::fields::Scope,
        };
        use serde_json::Value;

        let db = db().await;
        let origin = "upload:0190f2a8-7b8f-7c3e-9b1a-3d2f1e0c9b8a";
        let key = format!("{SHA}.png");
        insert_upload(
            &db,
            &Upload {
                id: "0190f2a8-7b8f-7c3e-9b1a-3d2f1e0c9b8a",
                origin,
                kind: Kind::Image,
                stored: Stored {
                    key: &key,
                    content_type: "image/png",
                    bytes: 10,
                    sha256: SHA,
                    width: Some(2),
                    height: Some(3),
                    thumb: Thumb::Png,
                    color: None,
                    thumbhash: None,
                    prepared: 0,
                },
                uploaded_by: "editor:bob",
                wanted_by: "w",
            },
        )
        .await
        .unwrap();
        let asset = by_origin(&db, origin).await.unwrap().unwrap();
        let kept = |why: &str| {
            let (db, asset, why) = (&db, &asset, why.to_string());
            async move {
                assert!(
                    !delete_unless_referenced(db, asset).await.unwrap(),
                    "{why}: deleted"
                );
                assert!(get(db, &asset.id).await.unwrap().is_some(), "{why}");
            }
        };

        // One of a work's pictures.
        work_with(&db, &[origin], None).await;
        kept("a picture").await;
        sqlx::query(db.sql("DELETE FROM media_image WHERE url = ?"))
            .bind(origin)
            .execute(db.pool())
            .await
            .unwrap();

        // A theme, on another work.
        work_with(&db, &[], Some(origin)).await;
        kept("a theme").await;
        sqlx::query(db.sql("UPDATE media_item SET theme_music = NULL"))
            .execute(db.pool())
            .await
            .unwrap();

        // A selection's cover, which keeps the address it was chosen by: the
        // upload, taken off the work it was made for, stays the cover. The
        // sweep does not offer the row either.
        let covered = list::create(
            &db,
            ListFields {
                slug: "covered",
                name: "Covered",
                description: None,
                kind: ListKind::Mixed,
                mode: ListMode::Manual,
                filter: None,
                is_public: true,
                cover: Some(origin),
            },
            None,
        )
        .await
        .unwrap();
        kept("a cover").await;
        assert!(
            unreferenced(&db, &HashSet::new(), LATER)
                .await
                .unwrap()
                .is_empty(),
            "a cover names it for the sweep too"
        );
        sqlx::query(db.sql("UPDATE curated_list SET cover = NULL WHERE id = ?"))
            .bind(&covered.id)
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            unreferenced(&db, &HashSet::new(), LATER)
                .await
                .unwrap()
                .len(),
            1,
            "the cover taken away, nothing names it"
        );

        // A lock on a work, by the upload's name: an episode's still, a
        // season's chosen poster, the work's background or theme.
        let locker = work_with(&db, &[], None).await;
        for (scope, field) in [
            (
                Scope::Episode {
                    season: 1,
                    episode: 2,
                },
                "image",
            ),
            (Scope::Season(1), "primaryPoster"),
            (Scope::Item, "primaryFanart"),
            (Scope::Item, "themeMusic"),
        ] {
            override_field::set(
                &db,
                &locker,
                scope,
                field,
                Some(&Value::String(origin.into())),
                Some("editor:bob"),
            )
            .await
            .unwrap();
            kept(field).await;
            override_field::clear(&db, &locker).await.unwrap();
        }

        // By the path of its bytes, as a lock written before locks were
        // filed by their origin holds it — the thumbnail's path too.
        for path in [
            format!("/media/{key}"),
            format!("https://ams.example/media/{SHA}-t.png"),
        ] {
            override_field::set(
                &db,
                &locker,
                Scope::Item,
                "primaryPoster",
                Some(&Value::String(path.clone())),
                None,
            )
            .await
            .unwrap();
            kept(&path).await;
            override_field::clear(&db, &locker).await.unwrap();
        }

        // Another upload's lock, or a path of other bytes, is not this one.
        override_field::set(
            &db,
            &locker,
            Scope::Item,
            "primaryPoster",
            Some(&Value::String("upload:another".into())),
            None,
        )
        .await
        .unwrap();

        // Nothing names it: it goes.
        assert!(delete_unless_referenced(&db, &asset).await.unwrap());
        assert!(get(&db, &asset.id).await.unwrap().is_none());
    }

    /// A picture's copies are noted once each, counted by what they are
    /// written as, and left to the sweep once no row holds their picture.
    #[tokio::test]
    async fn a_pictures_copies_are_noted_counted_and_found_orphaned() {
        let db = db().await;
        let variant = |made: &str, format: &str, bytes: i64| Variant {
            key: format!("{SHA}-{made}"),
            stem: SHA.into(),
            width: 480,
            height: 720,
            format: format.into(),
            bytes,
        };
        add_variant(&db, &variant("480.webp", "webp", 100))
            .await
            .unwrap();
        add_variant(&db, &variant("480.webp", "webp", 999))
            .await
            .unwrap();
        add_variant(&db, &variant("480.avif", "avif", 60))
            .await
            .unwrap();
        assert_eq!(
            variants_of(&db, SHA).await.unwrap().len(),
            2,
            "made again, noted once"
        );
        let counts = variant_counts(&db).await.unwrap();
        assert_eq!(
            (
                counts.webp,
                counts.webp_bytes,
                counts.avif,
                counts.avif_bytes
            ),
            (1, 100, 1, 60)
        );
        assert_eq!(avif_index(&db).await.unwrap(), vec![(SHA.to_string(), 480)]);

        // No row holds the picture: its copies are the sweep's, once they
        // have had their hour.
        let nobody = HashSet::new();
        assert_eq!(orphan_variants(&db, &nobody, LATER).await.unwrap().len(), 2);
        assert!(
            orphan_variants(&db, &nobody, "2000-01-01T00:00:00.000Z")
                .await
                .unwrap()
                .is_empty()
        );
        let held: HashSet<String> = [SHA.to_string()].into();
        assert!(orphan_variants(&db, &held, LATER).await.unwrap().is_empty());

        delete_variant(&db, &format!("{SHA}-480.avif"))
            .await
            .unwrap();
        assert!(avif_index(&db).await.unwrap().is_empty());
    }

    /// A format's copies are walked once, a few at a time in the order of
    /// their keys, and forgotten together; another format's are left as
    /// they are.
    #[tokio::test]
    async fn a_formats_copies_are_walked_once_and_forgotten_together() {
        let db = db().await;
        for (made, format) in [
            ("240.webp", "webp"),
            ("480.webp", "webp"),
            ("960.webp", "webp"),
            ("480.avif", "avif"),
            ("square.jpg", "jpeg"),
        ] {
            add_variant(
                &db,
                &Variant {
                    key: format!("{SHA}-{made}"),
                    stem: SHA.into(),
                    width: 480,
                    height: 720,
                    format: format.into(),
                    bytes: 10,
                },
            )
            .await
            .unwrap();
        }
        let keys = |copies: &[Variant]| -> Vec<String> {
            copies.iter().map(|copy| copy.key.clone()).collect()
        };

        let first = variants_by_format(&db, "webp", "", 2).await.unwrap();
        assert_eq!(
            keys(&first),
            [format!("{SHA}-240.webp"), format!("{SHA}-480.webp")]
        );
        let rest = variants_by_format(&db, "webp", &first[1].key, 2)
            .await
            .unwrap();
        assert_eq!(keys(&rest), [format!("{SHA}-960.webp")]);
        assert!(
            variants_by_format(&db, "webp", &rest[0].key, 2)
                .await
                .unwrap()
                .is_empty(),
            "walked once"
        );

        let webp = [keys(&first), keys(&rest)].concat();
        assert_eq!(delete_variants(&db, &webp).await.unwrap(), 3);
        assert_eq!(
            delete_variants(&db, &webp).await.unwrap(),
            0,
            "forgotten already"
        );
        assert_eq!(delete_variants(&db, &[]).await.unwrap(), 0);
        let counts = variant_counts(&db).await.unwrap();
        assert_eq!((counts.webp, counts.avif, counts.jpeg), (0, 1, 1));
        assert_eq!(variants_of(&db, SHA).await.unwrap().len(), 2);
    }

    /// What a work has a picture as decides the copies made of it; the
    /// pictures this version has not prepared are listed until each is, so
    /// a run stopped partway is taken up where it stopped.
    #[tokio::test]
    async fn a_picture_is_known_by_what_its_work_has_it_as() {
        let db = db().await;
        work_with(&db, &["https://a/poster.jpg"], None).await;
        assert_eq!(
            cover_of(&db, "https://a/poster.jpg").await.unwrap(),
            Some(CoverType::Poster)
        );
        assert_eq!(cover_of(&db, "https://a/still.jpg").await.unwrap(), None);

        enqueue(
            &db,
            &[
                wanted("https://a/poster.jpg"),
                wanted("https://a/still.jpg"),
            ],
        )
        .await
        .unwrap();
        for (n, row) in due(&db, 10).await.unwrap().into_iter().enumerate() {
            let stem = format!("{n}{}", &SHA[1..]);
            let key = format!("{stem}.jpg");
            mark_stored(
                &db,
                &row.id,
                &Stored {
                    key: &key,
                    content_type: "image/jpeg",
                    bytes: 10,
                    sha256: &stem,
                    width: Some(2),
                    height: Some(3),
                    thumb: Thumb::None,
                    color: None,
                    thumbhash: None,
                    prepared: 0,
                },
            )
            .await
            .unwrap();
        }

        let line = unprepared(&db, 1, "", 10).await.unwrap();
        assert_eq!(line.len(), 2);
        assert!(
            unprepared(&db, 1, &line[1].id, 10)
                .await
                .unwrap()
                .is_empty(),
            "a run walks them once"
        );
        assert!(
            mark_prepared(&db, &line[0].id, Some("#000000"), None, 1)
                .await
                .unwrap()
        );
        let rest = unprepared(&db, 1, "", 10).await.unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(
            rest[0].id, line[1].id,
            "the next run takes up where it stopped"
        );
        assert_eq!(unprepared_count(&db, 1).await.unwrap(), 1);
        let prepared = get(&db, &line[0].id).await.unwrap().unwrap();
        assert_eq!(prepared.color.as_deref(), Some("#000000"));

        // The night's: the poster, and not the still.
        let candidates = avif_candidates(&db, "", 10).await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0.origin, "https://a/poster.jpg");
        assert_eq!(candidates[0].1, CoverType::Poster);
        assert!(
            avif_candidates(&db, &candidates[0].0.id, 10)
                .await
                .unwrap()
                .is_empty(),
            "walked once"
        );
    }
}
