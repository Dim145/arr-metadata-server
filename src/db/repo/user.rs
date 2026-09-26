//! Accounts and their sessions.
//!
//! Every person who signs in has a row here: an administrator, an editor who
//! corrects the catalogue, or a member who browses and holds keys of their
//! own. Sessions are server-side: the cookie carries an opaque token and the
//! database stores only its SHA-256, so a database leak does not yield usable
//! sessions.

use std::{fmt, str::FromStr};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Arguments, any::AnyArguments};
use utoipa::ToSchema;

use crate::db::{Db, RowExt, from_bool, new_id, now, to_rfc3339};

/// The password hash of an account that has none — one made by an identity
/// provider. No password verifies against it: it is not a PHC string.
pub const NO_PASSWORD: &str = "!";

/// What an account may do, least first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Browses, and manages their own profile, sessions and keys.
    Member,
    /// And corrects the catalogue: fields, imports, refreshes, lists.
    Editor,
    /// And settles everything else: accounts, access, keys, settings.
    Admin,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Editor => "editor",
            Self::Admin => "admin",
        }
    }

    /// The key scopes this role can grant: a key never does more than the
    /// person it belongs to.
    pub const fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Member => &["read"],
            Self::Editor => &["read", "write"],
            Self::Admin => &["read", "write", "admin"],
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "member" => Ok(Self::Member),
            "editor" => Ok(Self::Editor),
            "admin" => Ok(Self::Admin),
            other => anyhow::bail!("unknown role {other:?}: member, editor or admin"),
        }
    }
}

/// Whether an account may sign in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    /// Signed up where sign-ups wait for an administrator.
    Pending,
    Disabled,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
            Self::Disabled => "disabled",
        }
    }
}

impl FromStr for Status {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "pending" => Ok(Self::Pending),
            "disabled" => Ok(Self::Disabled),
            other => anyhow::bail!("unknown status {other:?}: active, pending or disabled"),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub role: Role,
    pub status: Status,
    /// The interface's language, when chosen: `fr`, `en`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Whether an identity provider vouches for this account.
    pub oidc_linked: bool,
    /// Whether the account has a password of its own to sign in with.
    pub has_password: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invited_by: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<String>,
}

impl User {
    /// What to call them: the name they gave, or their username.
    pub fn name(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&self.username)
    }
}

pub struct Credentials {
    pub user: User,
    pub password_hash: String,
}

/// A new account.
pub struct NewUser<'a> {
    pub username: &'a str,
    /// A PHC string, or [`NO_PASSWORD`].
    pub password_hash: &'a str,
    pub role: Role,
    pub status: Status,
    pub display_name: Option<&'a str>,
    pub email: Option<&'a str>,
    pub invited_by: Option<&'a str>,
    pub oidc: Option<(&'a str, &'a str)>,
}

const COLUMNS: &str = "id, username, password_hash, role, status, display_name, email, locale,
                       oidc_issuer, oidc_subject, invited_by, created_at, updated_at, last_login_at";

pub async fn create(db: &Db, new: NewUser<'_>) -> Result<User> {
    let id = new_id();
    let created = now();

    sqlx::query(db.sql(
        "INSERT INTO admin_user
             (id, username, password_hash, is_admin, role, status, display_name, email,
              oidc_issuer, oidc_subject, invited_by, created_at, updated_at, last_login_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    ))
    .bind(&id)
    .bind(new.username)
    .bind(new.password_hash)
    .bind(from_bool(new.role == Role::Admin))
    .bind(new.role.as_str())
    .bind(new.status.as_str())
    .bind(new.display_name)
    .bind(new.email)
    .bind(new.oidc.map(|(issuer, _)| issuer))
    .bind(new.oidc.map(|(_, subject)| subject))
    .bind(new.invited_by)
    .bind(&created)
    .bind(&created)
    .execute(db.pool())
    .await?;

    get(db, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("an account vanished as it was created"))
}

pub async fn get(db: &Db, id: &str) -> Result<Option<User>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM admin_user WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    row.as_ref().map(map).transpose()
}

pub async fn find_by_username(db: &Db, username: &str) -> Result<Option<Credentials>> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT {COLUMNS} FROM admin_user WHERE LOWER(username) = LOWER(?)"
    )))
    .bind(username)
    .fetch_optional(db.pool())
    .await?;

    let Some(row) = row else { return Ok(None) };

    Ok(Some(Credentials {
        password_hash: row.text("password_hash")?,
        user: map(&row)?,
    }))
}

/// An account and its password hash, by id: what checking the password of
/// someone already signed in reads, rather than their username, which another
/// account could share but for its case.
pub async fn credentials(db: &Db, id: &str) -> Result<Option<Credentials>> {
    let row = sqlx::query(db.sql(&format!("SELECT {COLUMNS} FROM admin_user WHERE id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    let Some(row) = row else { return Ok(None) };

    Ok(Some(Credentials {
        password_hash: row.text("password_hash")?,
        user: map(&row)?,
    }))
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM admin_user"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

/// Active administrators, leaving one out: whether removing that one would
/// leave the server with nobody to administer it.
pub async fn count_active_admins(db: &Db, except: Option<&str>) -> Result<i64> {
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS n FROM admin_user
          WHERE role = 'admin' AND status = 'active' AND id <> ?",
    ))
    .bind(except.unwrap_or(""))
    .fetch_one(db.pool())
    .await?;

    Ok(row.big("n")?)
}

/// An account in a list, with what it holds.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    #[serde(flatten)]
    pub user: User,
    pub keys: i64,
    pub sessions: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub term: Option<String>,
    pub role: Option<Role>,
    pub status: Option<Status>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Clone, Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub total: i64,
    pub pending: i64,
    pub admins: i64,
    pub oidc: i64,
}

/// The filter a list is asked for, as SQL and the values it binds, in order.
fn filter(q: &Query) -> (String, Vec<String>) {
    let mut sql = String::from(" WHERE 1 = 1");
    let mut values = Vec::new();

    if let Some(term) = q.term.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        sql.push_str(
            r" AND (LOWER(u.username) LIKE ? ESCAPE '\'
                   OR LOWER(COALESCE(u.display_name, '')) LIKE ? ESCAPE '\'
                   OR LOWER(COALESCE(u.email, '')) LIKE ? ESCAPE '\')",
        );
        // The term is matched as typed: `john_doe` is a username, and its
        // underscore is not a wildcard.
        let escaped = term
            .to_lowercase()
            .replace('\\', r"\\")
            .replace('%', r"\%")
            .replace('_', r"\_");
        let like = format!("%{escaped}%");
        values.extend([like.clone(), like.clone(), like]);
    }
    if let Some(role) = q.role {
        sql.push_str(" AND u.role = ?");
        values.push(role.as_str().to_string());
    }
    if let Some(status) = q.status {
        sql.push_str(" AND u.status = ?");
        values.push(status.as_str().to_string());
    }

    (sql, values)
}

pub async fn list(db: &Db, q: &Query) -> Result<(Vec<Listed>, i64)> {
    let (filter, values) = filter(q);

    let total = {
        let mut args = AnyArguments::default();
        for value in &values {
            args.add(value.clone()).map_err(anyhow::Error::msg)?;
        }
        let sql = format!("SELECT COUNT(*) AS n FROM admin_user u{filter}");
        let row = sqlx::query_with(db.sql(&sql), args)
            .fetch_one(db.pool())
            .await?;
        row.big("n")?
    };

    let columns = COLUMNS
        .split(',')
        .map(|c| format!("u.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {columns},
                (SELECT COUNT(*) FROM api_client k WHERE k.owner_id = u.id) AS key_count,
                (SELECT COUNT(*) FROM admin_session s
                  WHERE s.user_id = u.id AND s.expires_at > ?) AS session_count
           FROM admin_user u{filter}
          ORDER BY CASE u.status WHEN 'pending' THEN 0 ELSE 1 END, LOWER(u.username)
          LIMIT ? OFFSET ?"
    );

    // The session cutoff comes first in the statement, then the filter, then
    // the page.
    let mut args = AnyArguments::default();
    args.add(now()).map_err(anyhow::Error::msg)?;
    for value in values {
        args.add(value).map_err(anyhow::Error::msg)?;
    }
    args.add(q.limit.clamp(1, 500))
        .map_err(anyhow::Error::msg)?;
    args.add(q.offset.max(0)).map_err(anyhow::Error::msg)?;

    let rows = sqlx::query_with(db.sql(&sql), args)
        .fetch_all(db.pool())
        .await?;

    let users = rows
        .iter()
        .map(|row| {
            Ok(Listed {
                user: map(row)?,
                keys: row.big("key_count")?,
                sessions: row.big("session_count")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok((users, total))
}

pub async fn counts(db: &Db) -> Result<Counts> {
    let row = sqlx::query(db.sql(
        "SELECT COUNT(*) AS total,
                COALESCE(SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), 0) AS pending,
                COALESCE(SUM(CASE WHEN role = 'admin' THEN 1 ELSE 0 END), 0) AS admins,
                COALESCE(SUM(CASE WHEN oidc_subject IS NOT NULL THEN 1 ELSE 0 END), 0) AS oidc
           FROM admin_user",
    ))
    .fetch_one(db.pool())
    .await?;

    Ok(Counts {
        total: row.big("total")?,
        pending: row.big("pending")?,
        admins: row.big("admins")?,
        oidc: row.big("oidc")?,
    })
}

/// What a person may change about themselves.
pub async fn update_profile(
    db: &Db,
    id: &str,
    display_name: Option<&str>,
    email: Option<&str>,
    locale: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "UPDATE admin_user SET display_name = ?, email = ?, locale = ?, updated_at = ? WHERE id = ?",
    ))
    .bind(display_name)
    .bind(email)
    .bind(locale)
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn set_role(db: &Db, id: &str, role: Role) -> Result<bool> {
    let result = sqlx::query(
        db.sql("UPDATE admin_user SET role = ?, is_admin = ?, updated_at = ? WHERE id = ?"),
    )
    .bind(role.as_str())
    .bind(from_bool(role == Role::Admin))
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn set_status(db: &Db, id: &str, status: Status) -> Result<bool> {
    let result =
        sqlx::query(db.sql("UPDATE admin_user SET status = ?, updated_at = ? WHERE id = ?"))
            .bind(status.as_str())
            .bind(now())
            .bind(id)
            .execute(db.pool())
            .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn set_password(db: &Db, id: &str, password_hash: &str) -> Result<bool> {
    let result =
        sqlx::query(db.sql("UPDATE admin_user SET password_hash = ?, updated_at = ? WHERE id = ?"))
            .bind(password_hash)
            .bind(now())
            .bind(id)
            .execute(db.pool())
            .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn delete(db: &Db, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM admin_user WHERE id = ?"))
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

/// One of a person's sessions, as their device list shows it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// The SHA-256 of the token: names the row, reveals nothing usable.
    pub id: String,
    pub created_at: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Whether it is the session that asked.
    pub current: bool,
}

pub async fn create_session(
    db: &Db,
    token_hash: &str,
    user_id: &str,
    ttl: chrono::Duration,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<String> {
    let expires_at = to_rfc3339(chrono::Utc::now() + ttl);
    let created = now();

    sqlx::query(db.sql(
        "INSERT INTO admin_session (id, user_id, created_at, expires_at, user_agent, ip, last_seen_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(token_hash)
    .bind(user_id)
    .bind(&created)
    .bind(&expires_at)
    .bind(user_agent.map(|ua| ua.chars().take(300).collect::<String>()))
    .bind(ip)
    .bind(&created)
    .execute(db.pool())
    .await?;

    Ok(expires_at)
}

/// Resolve a session token hash to its account, if the session has not
/// expired and the account may still sign in.
///
/// A disabled or pending account's sessions stop working at once, whatever
/// their expiry: they are deleted when an administrator changes its status,
/// and refused here meanwhile.
pub async fn find_session_user(db: &Db, token_hash: &str) -> Result<Option<User>> {
    let columns = COLUMNS
        .split(',')
        .map(|c| format!("u.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let row = sqlx::query(db.sql(&format!(
        "SELECT {columns}, s.last_seen_at AS seen
           FROM admin_session s
           JOIN admin_user u ON u.id = s.user_id
          WHERE s.id = ? AND s.expires_at > ? AND u.status = 'active'"
    )))
    .bind(token_hash)
    .bind(now())
    .fetch_optional(db.pool())
    .await?;

    let Some(row) = row else { return Ok(None) };

    // When it was last used, at most once a minute: the interface asks for
    // several things per page, and each would otherwise be a write.
    let cutoff = to_rfc3339(chrono::Utc::now() - chrono::Duration::minutes(1));
    if row.opt_text("seen")?.is_none_or(|seen| seen < cutoff)
        && let Err(e) =
            sqlx::query(db.sql("UPDATE admin_session SET last_seen_at = ? WHERE id = ?"))
                .bind(now())
                .bind(token_hash)
                .execute(db.pool())
                .await
    {
        tracing::warn!(error = %e, "could not record a session's use");
    }

    Ok(Some(map(&row)?))
}

pub async fn list_sessions(db: &Db, user_id: &str, current: Option<&str>) -> Result<Vec<Session>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, created_at, expires_at, last_seen_at, user_agent, ip
           FROM admin_session WHERE user_id = ? AND expires_at > ?
          ORDER BY COALESCE(last_seen_at, created_at) DESC",
    ))
    .bind(user_id)
    .bind(now())
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            let id = row.text("id")?;
            Ok(Session {
                current: current == Some(id.as_str()),
                id,
                created_at: row.text("created_at")?,
                expires_at: row.text("expires_at")?,
                last_seen_at: row.opt_text("last_seen_at")?,
                user_agent: row.opt_text("user_agent")?,
                ip: row.opt_text("ip")?,
            })
        })
        .collect()
}

pub async fn delete_session(db: &Db, token_hash: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM admin_session WHERE id = ?"))
        .bind(token_hash)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected() > 0)
}

/// One of a person's own sessions: another's is never theirs to close.
pub async fn delete_user_session(db: &Db, user_id: &str, session_id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM admin_session WHERE id = ? AND user_id = ?"))
        .bind(session_id)
        .bind(user_id)
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

/// Every session of a user but one: "sign out everywhere else".
pub async fn delete_other_sessions(db: &Db, user_id: &str, keep: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM admin_session WHERE user_id = ? AND id <> ?"))
        .bind(user_id)
        .bind(keep)
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

fn map(row: &sqlx::any::AnyRow) -> Result<User> {
    Ok(User {
        id: row.text("id")?,
        username: row.text("username")?,
        display_name: row.opt_text("display_name")?,
        email: row.opt_text("email")?,
        role: row.text("role")?.parse()?,
        status: row.text("status")?.parse()?,
        locale: row.opt_text("locale")?,
        oidc_linked: row.opt_text("oidc_subject")?.is_some(),
        has_password: row.text("password_hash")? != NO_PASSWORD,
        invited_by: row.opt_text("invited_by")?,
        created_at: row.text("created_at")?,
        updated_at: row.opt_text("updated_at")?,
        last_login_at: row.opt_text("last_login_at")?,
    })
}
