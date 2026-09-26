//! Invitations to open an account.
//!
//! A code is a secret like a key: shown once, in the link an administrator
//! sends, and kept only as a SHA-256. An invitation says which role the
//! account it opens gets, how many accounts it may open, and until when.

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;

use crate::db::{Db, RowExt, new_id, now, repo::user::Role};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    pub id: String,
    /// The visible head of the code, `inv_4Q2F`.
    pub code_prefix: String,
    pub role: Role,
    pub max_uses: i64,
    pub uses: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by_name: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    /// Whether whoever made it is still an active administrator. An
    /// invitation is worth what its maker's rights are: demoted, disabled or
    /// deleted, they take it with them.
    pub creator_active: bool,
}

impl Invitation {
    /// Whether it still opens an account: not revoked, not expired, not used
    /// up, and its maker still an administrator. [`USABLE`] says the same in
    /// SQL.
    pub fn is_usable(&self) -> bool {
        self.revoked_at.is_none()
            && self.creator_active
            && self.uses < self.max_uses
            && self
                .expires_at
                .as_deref()
                .is_none_or(|at| at > now().as_str())
    }
}

/// [`Invitation::is_usable`] as a condition on the table, with the time bound
/// as its one parameter. Written without an alias, so an `UPDATE` can use it.
const USABLE: &str = "revoked_at IS NULL AND uses < max_uses
     AND (expires_at IS NULL OR expires_at > ?)
     AND (created_by IS NULL OR EXISTS (
            SELECT 1 FROM admin_user a
             WHERE a.id = user_invitation.created_by AND a.role = 'admin' AND a.status = 'active'))";

pub struct NewInvitation<'a> {
    pub code_hash: &'a str,
    pub code_prefix: &'a str,
    pub role: Role,
    pub max_uses: i64,
    pub expires_at: Option<&'a str>,
    pub note: Option<&'a str>,
    pub created_by: Option<&'a str>,
}

const SELECT: &str = "SELECT user_invitation.id, code_prefix, user_invitation.role, max_uses, uses,
                             expires_at, note, created_by, u.username AS created_by_name,
                             user_invitation.created_at, revoked_at,
                             CASE WHEN created_by IS NULL OR (u.role = 'admin' AND u.status = 'active')
                                  THEN 1 ELSE 0 END AS creator_active
                        FROM user_invitation
                        LEFT JOIN admin_user u ON u.id = user_invitation.created_by";

pub async fn create(db: &Db, new: NewInvitation<'_>) -> Result<Invitation> {
    let id = new_id();

    sqlx::query(db.sql(
        "INSERT INTO user_invitation
             (id, code_hash, code_prefix, role, max_uses, uses, expires_at, note, created_by,
              created_at, revoked_at)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?, ?, NULL)",
    ))
    .bind(&id)
    .bind(new.code_hash)
    .bind(new.code_prefix)
    .bind(new.role.as_str())
    .bind(new.max_uses)
    .bind(new.expires_at)
    .bind(new.note)
    .bind(new.created_by)
    .bind(now())
    .execute(db.pool())
    .await?;

    get(db, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("an invitation vanished as it was created"))
}

pub async fn get(db: &Db, id: &str) -> Result<Option<Invitation>> {
    let row = sqlx::query(db.sql(&format!("{SELECT} WHERE user_invitation.id = ?")))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;

    row.as_ref().map(map).transpose()
}

pub async fn find_by_code_hash(db: &Db, code_hash: &str) -> Result<Option<Invitation>> {
    let row = sqlx::query(db.sql(&format!("{SELECT} WHERE code_hash = ?")))
        .bind(code_hash)
        .fetch_optional(db.pool())
        .await?;

    row.as_ref().map(map).transpose()
}

/// The invitations, usable ones first and newest first among each — sorted
/// in SQL, so that a usable one is never among those the limit leaves out.
pub async fn list(db: &Db) -> Result<Vec<Invitation>> {
    let rows = sqlx::query(db.sql(&format!(
        "{SELECT} ORDER BY CASE WHEN {USABLE} THEN 0 ELSE 1 END, user_invitation.created_at DESC
         LIMIT 200"
    )))
    .bind(now())
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(map).collect()
}

/// Take one use of an invitation, if it has one left — in one statement, so
/// two sign-ups racing for the last use cannot both have it.
pub async fn take_use(db: &Db, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql(&format!(
        "UPDATE user_invitation SET uses = uses + 1 WHERE id = ? AND {USABLE}"
    )))
    .bind(id)
    .bind(now())
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Give a use back, when the account it was taken for could not be made.
pub async fn return_use(db: &Db, id: &str) -> Result<()> {
    sqlx::query(db.sql("UPDATE user_invitation SET uses = uses - 1 WHERE id = ? AND uses > 0"))
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(())
}

pub async fn revoke(db: &Db, id: &str) -> Result<bool> {
    let result = sqlx::query(
        db.sql("UPDATE user_invitation SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL"),
    )
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Withdraw every invitation a person made: what goes with them when their
/// account is deleted, since the link from the invitation to them does not
/// survive the deletion.
pub async fn revoke_by_creator(db: &Db, user_id: &str) -> Result<u64> {
    let result = sqlx::query(db.sql(
        "UPDATE user_invitation SET revoked_at = ? WHERE created_by = ? AND revoked_at IS NULL",
    ))
    .bind(now())
    .bind(user_id)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected())
}

pub async fn count_usable(db: &Db) -> Result<(i64, i64)> {
    let row = sqlx::query(db.sql(&format!(
        "SELECT COUNT(*) AS n, COALESCE(SUM(max_uses - uses), 0) AS places
           FROM user_invitation
          WHERE {USABLE}"
    )))
    .bind(now())
    .fetch_one(db.pool())
    .await?;

    Ok((row.big("n")?, row.big("places")?))
}

fn map(row: &sqlx::any::AnyRow) -> Result<Invitation> {
    Ok(Invitation {
        id: row.text("id")?,
        code_prefix: row.text("code_prefix")?,
        role: row.text("role")?.parse()?,
        max_uses: row.big("max_uses")?,
        uses: row.big("uses")?,
        expires_at: row.opt_text("expires_at")?,
        note: row.opt_text("note")?,
        created_by: row.opt_text("created_by")?,
        created_by_name: row.opt_text("created_by_name")?,
        created_at: row.text("created_at")?,
        revoked_at: row.opt_text("revoked_at")?,
        creator_active: row.flag("creator_active")?,
    })
}
