//! Administrators and their sessions.
//!
//! Sessions are server-side: the cookie carries an opaque token and the database
//! stores only its SHA-256, so a database leak does not yield usable sessions.

use anyhow::Result;
use serde::Serialize;

use crate::db::{Db, RowExt, from_bool, new_id, now, to_rfc3339};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUser {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

pub struct Credentials {
    pub user: AdminUser,
    pub password_hash: String,
}

pub async fn create(db: &Db, username: &str, password_hash: &str, is_admin: bool) -> Result<AdminUser> {
    let id = new_id();
    let created = now();

    sqlx::query(db.sql(
        "INSERT INTO admin_user (id, username, password_hash, is_admin, created_at, last_login_at)
         VALUES (?, ?, ?, ?, ?, NULL)",
    ))
    .bind(&id)
    .bind(username)
    .bind(password_hash)
    .bind(from_bool(is_admin))
    .bind(&created)
    .execute(db.pool())
    .await?;

    Ok(AdminUser {
        id,
        username: username.to_string(),
        is_admin,
        created_at: created,
        last_login_at: None,
    })
}

pub async fn find_by_username(db: &Db, username: &str) -> Result<Option<Credentials>> {
    let row = sqlx::query(db.sql(
        "SELECT id, username, password_hash, is_admin, created_at, last_login_at
         FROM admin_user WHERE username = ?",
    ))
    .bind(username)
    .fetch_optional(db.pool())
    .await?;

    let Some(row) = row else { return Ok(None) };

    Ok(Some(Credentials {
        password_hash: row.text("password_hash")?,
        user: map(&row)?,
    }))
}

pub async fn get(db: &Db, id: &str) -> Result<Option<AdminUser>> {
    let row = sqlx::query(db.sql(
        "SELECT id, username, is_admin, created_at, last_login_at FROM admin_user WHERE id = ?",
    ))
    .bind(id)
    .fetch_optional(db.pool())
    .await?;

    row.as_ref().map(map).transpose()
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM admin_user"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

pub async fn set_password(db: &Db, id: &str, password_hash: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("UPDATE admin_user SET password_hash = ? WHERE id = ?"))
        .bind(password_hash)
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn mark_login(db: &Db, id: &str) -> Result<()> {
    sqlx::query(db.sql("UPDATE admin_user SET last_login_at = ? WHERE id = ?"))
        .bind(now())
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(())
}

// ─── sessions ────────────────────────────────────────────────────────────────

pub async fn create_session(
    db: &Db,
    token_hash: &str,
    user_id: &str,
    ttl: chrono::Duration,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<String> {
    let expires_at = to_rfc3339(chrono::Utc::now() + ttl);

    sqlx::query(db.sql(
        "INSERT INTO admin_session (id, user_id, created_at, expires_at, user_agent, ip)
         VALUES (?, ?, ?, ?, ?, ?)",
    ))
    .bind(token_hash)
    .bind(user_id)
    .bind(now())
    .bind(&expires_at)
    .bind(user_agent)
    .bind(ip)
    .execute(db.pool())
    .await?;

    Ok(expires_at)
}

/// Resolve a session token hash to its user, if the session has not expired.
pub async fn find_session_user(db: &Db, token_hash: &str) -> Result<Option<AdminUser>> {
    let row = sqlx::query(db.sql(
        "SELECT u.id, u.username, u.is_admin, u.created_at, u.last_login_at
         FROM admin_session s
         JOIN admin_user u ON u.id = s.user_id
         WHERE s.id = ? AND s.expires_at > ?",
    ))
    .bind(token_hash)
    .bind(now())
    .fetch_optional(db.pool())
    .await?;

    row.as_ref().map(map).transpose()
}

pub async fn delete_session(db: &Db, token_hash: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM admin_session WHERE id = ?"))
        .bind(token_hash)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Drop every session belonging to a user, e.g. after a password change.
pub async fn delete_sessions_for_user(db: &Db, user_id: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM admin_session WHERE user_id = ?"))
        .bind(user_id)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected())
}

pub async fn purge_expired_sessions(db: &Db) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM admin_session WHERE expires_at <= ?"))
        .bind(now())
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected())
}

fn map(row: &sqlx::any::AnyRow) -> Result<AdminUser> {
    Ok(AdminUser {
        id: row.text("id")?,
        username: row.text("username")?,
        is_admin: row.flag("is_admin")?,
        created_at: row.text("created_at")?,
        last_login_at: row.opt_text("last_login_at")?,
    })
}
