//! Material every instance must hold the same of, kept in the database so an
//! instance started afresh finds what the others use rather than making its
//! own: the key the identity provider's cookies are sealed with, the
//! authority the clients trust, the certificate it issued.
//!
//! A value is first written by whichever instance gets there first, and the
//! others take it: [`put_if_absent`] never overwrites, so two instances
//! starting together settle on one value without talking to each other; and
//! one that must replace what is there says what it expects to replace, so
//! two replacing at once settle on one as well.

use anyhow::Result;

use crate::db::{Db, RowExt, now};

/// The names the entries go by.
pub mod names {
    /// The key the OIDC sign-in's cookies are sealed with, base64.
    pub const COOKIE_KEY: &str = "auth.cookieKey";
    /// The authority the clients trust: its certificate and its key, PEM,
    /// as one JSON document — written together or not at all.
    pub const CA: &str = "tls.ca";
    /// The certificate the authority issued for the clients' door, and its
    /// key, the same way.
    pub const CERT: &str = "tls.cert";
}

/// The entry, when there is one.
pub async fn get(db: &Db, name: &str) -> Result<Option<String>> {
    let row = sqlx::query(db.sql("SELECT value FROM keystore WHERE name = ?"))
        .bind(name)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.as_ref().map(|row| row.text("value")).transpose()?)
}

/// Write the entry unless one exists: what is there afterwards, whichever
/// instance wrote it.
pub async fn put_if_absent(db: &Db, name: &str, value: &str) -> Result<String> {
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO keystore (name, value, created_at, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (name) DO NOTHING",
    ))
    .bind(name)
    .bind(value)
    .bind(&at)
    .bind(&at)
    .execute(db.pool())
    .await?;
    match get(db, name).await? {
        Some(value) => Ok(value),
        // Deleted between the two statements: written again.
        None => Box::pin(put_if_absent(db, name, value)).await,
    }
}

/// Replace the entry, if it still reads `expected`: whether it was this
/// caller's write that landed.
pub async fn replace_if(db: &Db, name: &str, expected: &str, value: &str) -> Result<bool> {
    let done = sqlx::query(
        db.sql("UPDATE keystore SET value = ?, updated_at = ? WHERE name = ? AND value = ?"),
    )
    .bind(value)
    .bind(now())
    .bind(name)
    .bind(expected)
    .execute(db.pool())
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Write the entry, over whatever was there.
pub async fn put(db: &Db, name: &str, value: &str) -> Result<()> {
    let at = now();
    sqlx::query(db.sql(
        "INSERT INTO keystore (name, value, created_at, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    ))
    .bind(name)
    .bind(value)
    .bind(&at)
    .bind(&at)
    .execute(db.pool())
    .await?;
    Ok(())
}

#[cfg(test)]
pub async fn delete(db: &Db, name: &str) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM keystore WHERE name = ?"))
        .bind(name)
        .execute(db.pool())
        .await?;
    Ok(())
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
    async fn the_first_writer_wins_and_a_put_overwrites() {
        let db = db().await;
        assert_eq!(get(&db, "k").await.unwrap(), None);
        assert_eq!(put_if_absent(&db, "k", "one").await.unwrap(), "one");
        assert_eq!(
            put_if_absent(&db, "k", "two").await.unwrap(),
            "one",
            "what is there is what everybody takes"
        );
        assert!(replace_if(&db, "k", "one", "two").await.unwrap());
        assert!(
            !replace_if(&db, "k", "one", "three").await.unwrap(),
            "somebody else replaced it first"
        );
        assert_eq!(get(&db, "k").await.unwrap().as_deref(), Some("two"));
        put(&db, "k", "three").await.unwrap();
        assert_eq!(get(&db, "k").await.unwrap().as_deref(), Some("three"));
        delete(&db, "k").await.unwrap();
        assert_eq!(get(&db, "k").await.unwrap(), None);
    }
}
