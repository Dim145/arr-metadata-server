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
}

/// Whether the row was still there to mark: one forgotten while its bytes
/// were being fetched is not stored, whatever the store now holds.
pub async fn mark_stored(db: &Db, id: &str, stored: &Stored<'_>) -> Result<bool> {
    let done = sqlx::query(db.sql(
        "UPDATE media_asset
         SET status = 'stored', key = ?, content_type = ?, bytes = ?, sha256 = ?, width = ?,
             height = ?, has_thumb = ?, error = NULL, next_attempt_at = NULL, stored_at = ?,
             claimed_by = NULL, claimed_at = NULL
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

/// Everything stored, for the index kept in memory: origin, key, and the
/// thumbnail made. The content type is the key's own.
pub async fn stored_index(db: &Db) -> Result<Vec<(String, String, Thumb)>> {
    let rows = sqlx::query(db.sql(
        "SELECT origin, key, has_thumb FROM media_asset
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
            ))
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
/// a work's theme — and any locked value, on any work, that is the address
/// itself, or with `by_path`, one that is the path of its bytes here: a
/// lock can name a picture another work holds, and one written before locks
/// were filed by their origin names it by its path.
fn naming(by_path: bool) -> String {
    let mut sql = String::from(
        "EXISTS (SELECT 1 FROM media_image i WHERE i.url = ?)
         OR EXISTS (SELECT 1 FROM media_episode e WHERE e.image = ?)
         OR EXISTS (SELECT 1 FROM media_credit c WHERE c.image = ?)
         OR EXISTS (SELECT 1 FROM media_relation r WHERE r.image = ?)
         OR EXISTS (SELECT 1 FROM media_item m WHERE m.theme_music = ?)
         OR EXISTS (SELECT 1 FROM media_override o WHERE o.value = ?)",
    );
    if by_path {
        sql.push_str("\n         OR EXISTS (SELECT 1 FROM media_override o WHERE o.value LIKE ?)");
    }
    sql
}

/// What [`naming`] is asked with: the address, five times; as a locked
/// value holds it, in JSON; and the path of its bytes, when it has any.
fn naming_binds(origin: &str, key: Option<&str>) -> Result<Vec<String>> {
    let mut binds = vec![origin.to_string(); 5];
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
/// in is looked through, and the locked values a caller passes — those are
/// JSON, and read in Rust.
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
           AND NOT EXISTS (SELECT 1 FROM media_item m WHERE m.theme_music = a.origin)"
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
            },
        )
        .await
        .unwrap();

        let index = stored_index(&db).await.unwrap();
        assert_eq!(
            index,
            vec![("https://a/1.jpg".into(), "ab.jpg".into(), Thumb::Jpeg)]
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
    /// work, a theme, a lock on any work — by the upload's own name, or by
    /// the path of its bytes here — keeps it; nothing left naming it, it goes.
    #[tokio::test]
    async fn an_upload_is_kept_while_anything_names_it() {
        use crate::{db::repo::override_field, domain::fields::Scope};
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
}
