//! API clients.
//!
//! A client is a named credential. The key is shown once at creation and never
//! recoverable afterwards; only its SHA-256 and a display prefix are stored.

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;

use crate::db::{Db, RowExt, from_bool, new_id, now, text_list};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiClient {
    pub id: String,
    pub name: String,
    /// The visible head of the key, e.g. `ams_a1b2c3d4`.
    pub key_prefix: String,
    pub scopes: Vec<String>,
    pub is_enabled: bool,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub last_used_ip: Option<String>,
    pub note: Option<String>,
}

/// What authentication needs, which is the record plus nothing secret.
pub struct Authenticated {
    pub client: ApiClient,
    pub key_hash: String,
}

pub struct NewClient<'a> {
    pub name: &'a str,
    pub key_prefix: &'a str,
    pub key_hash: &'a str,
    pub scopes: &'a [String],
    pub expires_at: Option<&'a str>,
    pub note: Option<&'a str>,
}

pub async fn create(db: &Db, new: NewClient<'_>) -> Result<ApiClient> {
    let id = new_id();
    let created = now();

    sqlx::query(db.sql(
        "INSERT INTO api_client
             (id, name, key_prefix, key_hash, scopes, is_enabled, expires_at,
              created_at, last_used_at, last_used_ip, note)
         VALUES (?, ?, ?, ?, ?, 1, ?, ?, NULL, NULL, ?)",
    ))
    .bind(&id)
    .bind(new.name)
    .bind(new.key_prefix)
    .bind(new.key_hash)
    .bind(text_list(new.scopes))
    .bind(new.expires_at)
    .bind(&created)
    .bind(new.note)
    .execute(db.pool())
    .await?;

    Ok(ApiClient {
        id,
        name: new.name.to_string(),
        key_prefix: new.key_prefix.to_string(),
        scopes: new.scopes.to_vec(),
        is_enabled: true,
        expires_at: new.expires_at.map(str::to_string),
        created_at: created,
        last_used_at: None,
        last_used_ip: None,
        note: new.note.map(str::to_string),
    })
}

const COLUMNS: &str = "id, name, key_prefix, scopes, is_enabled, expires_at,
                       created_at, last_used_at, last_used_ip, note";

pub async fn list(db: &Db) -> Result<Vec<ApiClient>> {
    let sql = format!("SELECT {COLUMNS} FROM api_client ORDER BY created_at DESC");

    let rows = sqlx::query(db.sql(&sql)).fetch_all(db.pool()).await?;
    rows.iter().map(map).collect()
}

pub async fn get(db: &Db, id: &str) -> Result<Option<ApiClient>> {
    let sql = format!("SELECT {COLUMNS} FROM api_client WHERE id = ?");

    let row = sqlx::query(db.sql(&sql))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    row.as_ref().map(map).transpose()
}

/// Look a client up by the hash of a presented key.
///
/// The caller still compares hashes in constant time; this only narrows to a
/// candidate row via the unique index.
pub async fn find_by_key_hash(db: &Db, key_hash: &str) -> Result<Option<Authenticated>> {
    let sql = format!("SELECT {COLUMNS}, key_hash FROM api_client WHERE key_hash = ?");

    let row = sqlx::query(db.sql(&sql))
        .bind(key_hash)
        .fetch_optional(db.pool())
        .await?;

    let Some(row) = row else { return Ok(None) };

    Ok(Some(Authenticated {
        client: map(&row)?,
        key_hash: row.text("key_hash")?,
    }))
}

/// Record that a key was used, at most once a minute per client.
///
/// Sonarr refreshes in bursts; an unconditional write here would turn every
/// metadata read into a database write and, on SQLite, serialise them.
pub async fn touch(db: &Db, id: &str, ip: Option<&str>) -> Result<()> {
    let cutoff = crate::db::to_rfc3339(chrono::Utc::now() - chrono::Duration::minutes(1));

    sqlx::query(db.sql(
        "UPDATE api_client SET last_used_at = ?, last_used_ip = ?
         WHERE id = ? AND (last_used_at IS NULL OR last_used_at < ?)",
    ))
    .bind(now())
    .bind(ip)
    .bind(id)
    .bind(cutoff)
    .execute(db.pool())
    .await?;

    Ok(())
}

pub async fn set_enabled(db: &Db, id: &str, enabled: bool) -> Result<bool> {
    let result = sqlx::query(db.sql("UPDATE api_client SET is_enabled = ? WHERE id = ?"))
        .bind(from_bool(enabled))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn delete(db: &Db, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM api_client WHERE id = ?"))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM api_client"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

fn map(row: &sqlx::any::AnyRow) -> Result<ApiClient> {
    Ok(ApiClient {
        id: row.text("id")?,
        name: row.text("name")?,
        key_prefix: row.text("key_prefix")?,
        scopes: row.text_list("scopes")?,
        is_enabled: row.flag("is_enabled")?,
        expires_at: row.opt_text("expires_at")?,
        created_at: row.text("created_at")?,
        last_used_at: row.opt_text("last_used_at")?,
        last_used_ip: row.opt_text("last_used_ip")?,
        note: row.opt_text("note")?,
    })
}
