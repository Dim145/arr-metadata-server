//! What a person keeps of their own way of working, by name, with their
//! account: the views of the catalogue they saved, the columns a table shows
//! them and how tightly it is set (migration 0045).
//!
//! The value is the JSON the interface wrote, kept as it came: the server
//! checks its name and its size (`api::native::account`), and reads nothing
//! inside it. A value that no longer reads as JSON is left out of what is
//! answered rather than failing the whole of it.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::Value;

use crate::db::{Db, RowExt, now};

/// Every preference a person keeps, by name.
pub async fn all(db: &Db, user_id: &str) -> Result<BTreeMap<String, Value>> {
    let rows = sqlx::query(
        db.sql("SELECT key, value FROM account_preference WHERE user_id = ? ORDER BY key"),
    )
    .bind(user_id)
    .fetch_all(db.pool())
    .await?;

    let mut kept = BTreeMap::new();
    for row in &rows {
        let key = row.text("key")?;
        match serde_json::from_str(&row.text("value")?) {
            Ok(value) => {
                kept.insert(key, value);
            }
            Err(e) => {
                tracing::warn!(user = %user_id, preference = %key, error = %e, "a preference no longer reads as JSON");
            }
        }
    }
    Ok(kept)
}

/// Keep `value` under `key`, in place of whatever was kept there.
pub async fn put(db: &Db, user_id: &str, key: &str, value: &Value) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO account_preference (user_id, key, value, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id, key) DO UPDATE
         SET value = excluded.value, updated_at = excluded.updated_at",
    ))
    .bind(user_id)
    .bind(key)
    .bind(serde_json::to_string(value)?)
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Whether a person keeps something under `key`.
pub async fn has(db: &Db, user_id: &str, key: &str) -> Result<bool> {
    let row = sqlx::query(
        db.sql("SELECT 1 AS one FROM account_preference WHERE user_id = ? AND key = ?"),
    )
    .bind(user_id)
    .bind(key)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.is_some())
}

/// How many preferences a person keeps.
pub async fn count(db: &Db, user_id: &str) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM account_preference WHERE user_id = ?"))
        .bind(user_id)
        .fetch_one(db.pool())
        .await?;
    Ok(row.big("n")?)
}

/// Forget what is kept under `key`: whether there was anything.
pub async fn delete(db: &Db, user_id: &str, key: &str) -> Result<bool> {
    let done = sqlx::query(db.sql("DELETE FROM account_preference WHERE user_id = ? AND key = ?"))
        .bind(user_id)
        .bind(key)
        .execute(db.pool())
        .await?;
    Ok(done.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::{
        repo::user::{self, NewUser, Role, Status},
        testing::db,
    };

    async fn person(db: &Db, username: &str) -> String {
        user::create(
            db,
            NewUser {
                username,
                password_hash: user::NO_PASSWORD,
                role: Role::Editor,
                status: Status::Active,
                display_name: None,
                email: None,
                invited_by: None,
                oidc: None,
            },
        )
        .await
        .expect("an account")
        .id
    }

    #[tokio::test]
    async fn a_preference_is_a_person_s_own_and_goes_with_them() {
        let db = db().await;
        let alice = person(&db, "alice-prefs").await;
        let bob = person(&db, "bob-prefs").await;

        let views = json!([{"name": "Sans affiche", "query": "missing=poster"}]);
        put(&db, &alice, "catalogue.views", &views)
            .await
            .expect("kept");
        put(&db, &alice, "table.catalogue", &json!({"hidden": ["ids"]}))
            .await
            .expect("kept");
        // Kept again under the same name: in place of the first.
        let fewer = json!([]);
        put(&db, &alice, "catalogue.views", &fewer)
            .await
            .expect("kept again");

        let kept = all(&db, &alice).await.expect("read");
        assert_eq!(kept.len(), 2);
        assert_eq!(kept["catalogue.views"], fewer);
        assert_eq!(kept["table.catalogue"], json!({"hidden": ["ids"]}));
        assert_eq!(count(&db, &alice).await.expect("counted"), 2);
        assert!(has(&db, &alice, "table.catalogue").await.expect("asked"));

        // Nobody else's.
        assert!(all(&db, &bob).await.expect("read").is_empty());
        assert!(!has(&db, &bob, "table.catalogue").await.expect("asked"));
        assert!(
            !delete(&db, &bob, "table.catalogue")
                .await
                .expect("forgotten")
        );

        assert!(
            delete(&db, &alice, "table.catalogue")
                .await
                .expect("forgotten")
        );
        assert_eq!(count(&db, &alice).await.expect("counted"), 1);

        // Gone with the account.
        user::delete(&db, &alice).await.expect("deleted");
        assert_eq!(count(&db, &alice).await.expect("counted"), 0);
    }
}
