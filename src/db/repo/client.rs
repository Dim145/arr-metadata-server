//! API keys.
//!
//! A key is a named credential. The secret is shown once at creation and never
//! recoverable afterwards; only its SHA-256 and a display prefix are stored.
//!
//! A key either belongs to a person — then it acts in their name, with no more
//! than their role allows, and stops working when they do — or to the server,
//! made by an administrator for a service: Sonarr in the living room, a script.

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;

use crate::db::{
    Db, RowExt, from_bool, new_id, now,
    repo::user::{Role, Status},
    text_list,
};

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
    /// The account it belongs to; none for a server key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    /// The owner's role and status, read with the key: what caps it.
    #[serde(skip)]
    pub owner: Option<(Role, Status)>,
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
    pub owner_id: Option<&'a str>,
}

/// Whose keys to list.
#[derive(Clone, Copy, Debug)]
pub enum Owner<'a> {
    /// Every key, the server's and everyone's.
    Any,
    User(&'a str),
}

const SELECT: &str = "SELECT c.id, c.name, c.key_prefix, c.scopes, c.is_enabled, c.expires_at,
                             c.created_at, c.last_used_at, c.last_used_ip, c.note, c.owner_id,
                             o.username AS owner_name, o.role AS owner_role,
                             o.status AS owner_status
                        FROM api_client c
                        LEFT JOIN admin_user o ON o.id = c.owner_id";

pub async fn create(db: &Db, new: NewClient<'_>) -> Result<ApiClient> {
    let id = new_id();

    sqlx::query(db.sql(
        "INSERT INTO api_client
             (id, name, key_prefix, key_hash, scopes, is_enabled, expires_at,
              created_at, last_used_at, last_used_ip, note, owner_id)
         VALUES (?, ?, ?, ?, ?, 1, ?, ?, NULL, NULL, ?, ?)",
    ))
    .bind(&id)
    .bind(new.name)
    .bind(new.key_prefix)
    .bind(new.key_hash)
    .bind(text_list(new.scopes))
    .bind(new.expires_at)
    .bind(now())
    .bind(new.note)
    .bind(new.owner_id)
    .execute(db.pool())
    .await?;

    get(db, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("a key vanished as it was created"))
}

pub async fn list(db: &Db, owner: Owner<'_>) -> Result<Vec<ApiClient>> {
    let (filter, bound) = match owner {
        Owner::Any => ("", None),
        Owner::User(id) => (" WHERE c.owner_id = ?", Some(id)),
    };
    let sql = format!("{SELECT}{filter} ORDER BY c.created_at DESC");

    let mut query = sqlx::query(db.sql(&sql));
    if let Some(id) = bound {
        query = query.bind(id);
    }

    let rows = query.fetch_all(db.pool()).await?;
    rows.iter().map(map).collect()
}

pub async fn get(db: &Db, id: &str) -> Result<Option<ApiClient>> {
    let sql = format!("{SELECT} WHERE c.id = ?");

    let row = sqlx::query(db.sql(&sql))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    row.as_ref().map(map).transpose()
}

/// Look a key up by the hash of a presented secret.
///
/// The caller still compares hashes in constant time; this only narrows to a
/// candidate row via the unique index.
pub async fn find_by_key_hash(db: &Db, key_hash: &str) -> Result<Option<Authenticated>> {
    let sql = format!("{SELECT} WHERE c.key_hash = ?").replacen(
        "c.owner_id,",
        "c.owner_id, c.key_hash,",
        1,
    );

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

/// How many keys a person holds.
pub async fn count_owned(db: &Db, owner_id: &str) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM api_client WHERE owner_id = ?"))
        .bind(owner_id)
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

/// Whether a name is already used among one owner's keys — or among the
/// server's, for a key without an owner. Two people may each call theirs
/// "Sonarr"; one person may not have two.
pub async fn name_taken(
    db: &Db,
    owner_id: Option<&str>,
    name: &str,
    except: Option<&str>,
) -> Result<bool> {
    let sql = match owner_id {
        Some(_) => {
            "SELECT COUNT(*) AS n FROM api_client
              WHERE owner_id = ? AND LOWER(name) = LOWER(?) AND id <> ?"
        }
        None => {
            "SELECT COUNT(*) AS n FROM api_client
              WHERE owner_id IS NULL AND LOWER(name) = LOWER(?) AND id <> ?"
        }
    };

    let mut query = sqlx::query(db.sql(sql));
    if let Some(owner) = owner_id {
        query = query.bind(owner);
    }
    let row = query
        .bind(name.trim())
        .bind(except.unwrap_or(""))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")? > 0)
}

/// Record that a key was used, at most once a minute per key.
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

pub async fn rename(db: &Db, id: &str, name: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("UPDATE api_client SET name = ? WHERE id = ?"))
        .bind(name.trim())
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn set_scopes(db: &Db, id: &str, scopes: &[String]) -> Result<bool> {
    let result = sqlx::query(db.sql("UPDATE api_client SET scopes = ? WHERE id = ?"))
        .bind(text_list(scopes))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

/// A new secret for the same key: its name, settings and owner stay; the old
/// secret stops working at once.
pub async fn rotate(db: &Db, id: &str, key_prefix: &str, key_hash: &str) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "UPDATE api_client SET key_prefix = ?, key_hash = ?, last_used_at = NULL, last_used_ip = NULL
          WHERE id = ?",
    ))
    .bind(key_prefix)
    .bind(key_hash)
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
    let owner = match (row.opt_text("owner_role")?, row.opt_text("owner_status")?) {
        (Some(role), Some(status)) => Some((role.parse()?, status.parse()?)),
        _ => None,
    };

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
        owner_id: row.opt_text("owner_id")?,
        owner_name: row.opt_text("owner_name")?,
        owner,
    })
}
