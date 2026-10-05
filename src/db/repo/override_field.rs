//! Manual overrides — the locking layer.
//!
//! Nothing outside an explicit user action ever writes here. The refresh path
//! must not touch this table; that invariant is what makes an edit permanent.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::{
    db::{Db, RowExt, from_bool, now},
    domain::{
        ExternalIds, MediaKind,
        fields::{IDENTITY, Override, Scope},
    },
};

/// Why a lock on a work's identity was not set: the slug or the identifier
/// it names is another work's. Said as the caller is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Taken(pub String);

pub async fn list(db: &Db, media_id: &str) -> Result<Vec<Override>> {
    let rows = sqlx::query(db.sql(
        "SELECT scope, field, value, updated_at, updated_by
         FROM media_override WHERE media_id = ? ORDER BY scope, field",
    ))
    .bind(media_id)
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            let raw: Option<String> = row.opt_text("value")?;
            Ok(Override {
                scope: row.text("scope")?,
                field: row.text("field")?,
                value: raw.and_then(|s| serde_json::from_str(&s).ok()),
                updated_at: row.text("updated_at")?,
                updated_by: row.opt_text("updated_by")?,
            })
        })
        .collect()
}

/// Every override in the catalogue, with the work each is on, in a fixed
/// order: what an export is made of.
pub async fn all(db: &Db) -> Result<Vec<(String, Override)>> {
    let rows = sqlx::query(db.sql(
        "SELECT media_id, scope, field, value, updated_at, updated_by
         FROM media_override ORDER BY media_id, scope, field",
    ))
    .fetch_all(db.pool())
    .await?;

    rows.iter()
        .map(|row| {
            let raw: Option<String> = row.opt_text("value")?;
            Ok((
                row.text("media_id")?,
                Override {
                    scope: row.text("scope")?,
                    field: row.text("field")?,
                    value: raw.and_then(|s| serde_json::from_str(&s).ok()),
                    updated_at: row.text("updated_at")?,
                    updated_by: row.opt_text("updated_by")?,
                },
            ))
        })
        .collect()
}

/// Record an edit. `value = None` stores an explicit "cleared" marker, which is
/// different from deleting the override.
pub async fn set(
    db: &Db,
    media_id: &str,
    scope: Scope,
    field: &str,
    value: Option<&Value>,
    by: Option<&str>,
) -> Result<()> {
    let encoded = value.map(serde_json::to_string).transpose()?;
    let at = now();
    // The lock and the change it makes to how the work is listed, together:
    // written apart, a failure between the two left the lock unlisted, and
    // nothing afterwards would ever list it.
    let mut tx = db.begin_write().await?;

    sqlx::query(db.sql(
        "INSERT INTO media_override (media_id, scope, field, value, created_at, updated_at, updated_by)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (media_id, scope, field) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at,
             updated_by = excluded.updated_by",
    ))
    .bind(media_id)
    .bind(scope.to_string())
    .bind(field)
    .bind(encoded)
    .bind(&at)
    .bind(&at)
    .bind(by)
    .execute(&mut *tx)
    .await?;

    // A lock on the work itself changes what it is listed by: its genre, its
    // year, its title's place in the order.
    if scope == Scope::Item {
        super::item::mark_changed_in(db, &mut tx, media_id).await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Record an edit on the work itself — and, for its identity, write what the
/// lock says to the row, in the same transaction: whether it is for adults,
/// its slug, the identifiers it goes by elsewhere. Every list, the calendar,
/// the feeds, the addresses and the clients' lookups read the row, not the
/// lock; written apart, a lock set without its row — by an import of locks,
/// or a failure between the two — left a work locked adult in every one of
/// them.
///
/// A slug another work of the kind has, or an identifier another work goes
/// by, is [`Taken`], and nothing is written. The caller has checked the value
/// against the field registry.
pub async fn set_on_work(
    db: &Db,
    media_id: &str,
    kind: MediaKind,
    field: &str,
    value: Option<&Value>,
    by: Option<&str>,
) -> Result<Result<(), Taken>> {
    let encoded = value.map(serde_json::to_string).transpose()?;
    let at = now();
    let mut tx = db.begin_write().await?;

    match (field, value) {
        ("isAdult", Some(Value::Bool(adult))) => {
            sqlx::query(db.sql("UPDATE media_item SET is_adult = ?, updated_at = ? WHERE id = ?"))
                .bind(from_bool(*adult))
                .bind(&at)
                .bind(media_id)
                .execute(&mut *tx)
                .await?;
        }
        ("slug", Some(Value::String(slug))) => {
            let owner: Option<String> = sqlx::query_scalar(
                db.sql("SELECT id FROM media_item WHERE kind = ? AND slug = ? AND id <> ? LIMIT 1"),
            )
            .bind(kind.as_str())
            .bind(slug)
            .bind(media_id)
            .fetch_optional(&mut *tx)
            .await?;
            if owner.is_some() {
                return Ok(Err(Taken(format!(
                    "another {} already has the address {slug:?}",
                    kind.as_str()
                ))));
            }
            sqlx::query(db.sql("UPDATE media_item SET slug = ?, updated_at = ? WHERE id = ?"))
                .bind(slug)
                .bind(&at)
                .bind(media_id)
                .execute(&mut *tx)
                .await?;
        }
        ("externalIds", Some(value @ Value::Object(_))) => {
            let ids: ExternalIds = serde_json::from_value(value.clone())
                .context("the identifiers locked cannot be read")?;
            let mut rows: Vec<(crate::domain::ExternalSource, String)> = Vec::new();
            for row in ids.rows(kind) {
                if !rows.contains(&row) {
                    rows.push(row);
                }
            }

            for (source, id) in &rows {
                let owner: Option<String> = sqlx::query_scalar(db.sql(
                    "SELECT media_id FROM media_external_id
                     WHERE source = ? AND value = ? AND media_id <> ? LIMIT 1",
                ))
                .bind(source.as_str())
                .bind(id)
                .bind(media_id)
                .fetch_optional(&mut *tx)
                .await?;
                if owner.is_some() {
                    return Ok(Err(Taken(format!(
                        "another work already goes by {} {id}",
                        source.as_str()
                    ))));
                }
            }

            sqlx::query(db.sql("DELETE FROM media_external_id WHERE media_id = ?"))
                .bind(media_id)
                .execute(&mut *tx)
                .await?;
            for (source, id) in &rows {
                sqlx::query(db.sql(
                    "INSERT INTO media_external_id (media_id, source, value, created_at)
                     VALUES (?, ?, ?, ?)",
                ))
                .bind(media_id)
                .bind(source.as_str())
                .bind(id)
                .bind(&at)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query(db.sql("UPDATE media_item SET updated_at = ? WHERE id = ?"))
                .bind(&at)
                .bind(media_id)
                .execute(&mut *tx)
                .await?;
        }
        _ => {}
    }

    sqlx::query(db.sql(
        "INSERT INTO media_override (media_id, scope, field, value, created_at, updated_at, updated_by)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (media_id, scope, field) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at,
             updated_by = excluded.updated_by",
    ))
    .bind(media_id)
    .bind(Scope::Item.to_string())
    .bind(field)
    .bind(encoded)
    .bind(&at)
    .bind(&at)
    .bind(by)
    .execute(&mut *tx)
    .await?;

    super::item::mark_changed_in(db, &mut tx, media_id).await?;

    tx.commit().await?;
    Ok(Ok(()))
}

/// The work's next refresh, brought forward to now: a lock on its identity
/// lifted leaves the row as the lock had it, and only its providers can say
/// what it is without one. Asked for in the transaction that lifts it.
async fn refresh_soon(
    db: &Db,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    media_id: &str,
) -> Result<()> {
    sqlx::query(db.sql("UPDATE media_item SET refresh_after = ? WHERE id = ?"))
        .bind(now())
        .bind(media_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Remove an override, handing the field back to provider data — for one of
/// the work's identity (see [`set_on_work`]), at its next refresh, which is
/// asked for now.
pub async fn unset(db: &Db, media_id: &str, scope: Scope, field: &str) -> Result<bool> {
    let mut tx = db.begin_write().await?;

    let result = sqlx::query(
        db.sql("DELETE FROM media_override WHERE media_id = ? AND scope = ? AND field = ?"),
    )
    .bind(media_id)
    .bind(scope.to_string())
    .bind(field)
    .execute(&mut *tx)
    .await?;

    let removed = result.rows_affected() > 0;
    if removed && scope == Scope::Item {
        super::item::mark_changed_in(db, &mut tx, media_id).await?;
        if IDENTITY.contains(&field) {
            refresh_soon(db, &mut tx, media_id).await?;
        }
    }

    tx.commit().await?;
    Ok(removed)
}

/// Unlock every field of a work at once — and, where its identity was among
/// them, ask for its next refresh now, as [`unset`] does.
pub async fn clear(db: &Db, media_id: &str) -> Result<u64> {
    let mut tx = db.begin_write().await?;

    let sql = format!(
        "SELECT COUNT(*) AS n FROM media_override
         WHERE media_id = ? AND scope = 'item' AND field IN ({})",
        vec!["?"; IDENTITY.len()].join(", ")
    );
    let mut query = sqlx::query(db.sql(&sql)).bind(media_id);
    for field in IDENTITY {
        query = query.bind(*field);
    }
    let identity = query.fetch_one(&mut *tx).await?.big("n")?;

    let result = sqlx::query(db.sql("DELETE FROM media_override WHERE media_id = ?"))
        .bind(media_id)
        .execute(&mut *tx)
        .await?;

    if result.rows_affected() > 0 {
        super::item::mark_changed_in(db, &mut tx, media_id).await?;
    }
    if identity > 0 {
        refresh_soon(db, &mut tx, media_id).await?;
    }

    tx.commit().await?;
    Ok(result.rows_affected())
}

/// Overrides for several works at once, keyed by media id.
///
/// A list view needs every item's edits applied; doing that one query per row
/// turns a page of fifty into fifty round trips.
pub async fn list_for_many(
    db: &Db,
    media_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<Override>>> {
    use std::collections::HashMap;

    let mut out: HashMap<String, Vec<Override>> = HashMap::new();

    if media_ids.is_empty() {
        return Ok(out);
    }

    // Chunked to stay well inside every engine's bind-parameter limit.
    for chunk in media_ids.chunks(200) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT media_id, scope, field, value, updated_at, updated_by
             FROM media_override WHERE media_id IN ({placeholders})
             ORDER BY media_id, scope, field"
        );

        let mut query = sqlx::query(db.sql(&sql));
        for id in chunk {
            query = query.bind(id);
        }

        for row in query.fetch_all(db.pool()).await? {
            let raw: Option<String> = row.opt_text("value")?;
            out.entry(row.text("media_id")?)
                .or_default()
                .push(Override {
                    scope: row.text("scope")?,
                    field: row.text("field")?,
                    value: raw.and_then(|s| serde_json::from_str(&s).ok()),
                    updated_at: row.text("updated_at")?,
                    updated_by: row.opt_text("updated_by")?,
                });
        }
    }

    Ok(out)
}

pub async fn count(db: &Db) -> Result<i64> {
    let row = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM media_override"))
        .fetch_one(db.pool())
        .await?;

    Ok(row.big("n")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config,
        db::repo::item,
        domain::{ExternalSource, MediaItem},
    };
    use serde_json::json;

    async fn db() -> Db {
        let db = Db::connect(&config::Database {
            url: "sqlite::memory:".into(),
            max_connections: 1,
            acquire_timeout: std::time::Duration::from_secs(5),
        })
        .await
        .expect("in-memory database");

        db.migrate().await.expect("migrations");
        db
    }

    async fn film(db: &Db, title: &str, tmdb: i64) -> MediaItem {
        let mut work = MediaItem::empty(MediaKind::Movie);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, None);
        work.external_ids = ExternalIds {
            tmdb: Some(tmdb),
            ..Default::default()
        };
        item::upsert(
            db,
            item::ItemWrite {
                item: &work,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        work
    }

    async fn row(db: &Db, id: &str) -> MediaItem {
        item::get(db, id).await.unwrap().expect("held")
    }

    #[tokio::test]
    async fn a_lock_on_the_works_identity_is_written_to_its_row_with_it() {
        let db = db().await;
        let heat = film(&db, "Heat", 949).await;
        let other = film(&db, "Ronin", 8195).await;

        // Adult: the row the lists filter on says so, with the lock.
        let set = set_on_work(
            &db,
            &heat.id,
            MediaKind::Movie,
            "isAdult",
            Some(&json!(true)),
            None,
        )
        .await
        .unwrap();
        assert_eq!(set, Ok(()));
        assert!(row(&db, &heat.id).await.is_adult);
        assert_eq!(list(&db, &heat.id).await.unwrap().len(), 1);

        // A slug another film has: refused, and nothing written — neither
        // the row nor the lock.
        let taken = set_on_work(
            &db,
            &heat.id,
            MediaKind::Movie,
            "slug",
            Some(&json!(other.slug)),
            None,
        )
        .await
        .unwrap();
        assert!(taken.is_err());
        assert_eq!(row(&db, &heat.id).await.slug, heat.slug);
        assert_eq!(list(&db, &heat.id).await.unwrap().len(), 1);

        // An identifier another film goes by: the same.
        let taken = set_on_work(
            &db,
            &heat.id,
            MediaKind::Movie,
            "externalIds",
            Some(&json!({ "tmdb": 8195 })),
            None,
        )
        .await
        .unwrap();
        assert!(taken.is_err());
        assert_eq!(
            item::find_id_by_external(&db, ExternalSource::TmdbMovie, "8195")
                .await
                .unwrap(),
            Some(other.id.clone())
        );

        // Its own: written, the old ones let go.
        let set = set_on_work(
            &db,
            &heat.id,
            MediaKind::Movie,
            "externalIds",
            Some(&json!({ "tmdb": 949, "imdb": "tt0113277" })),
            None,
        )
        .await
        .unwrap();
        assert_eq!(set, Ok(()));
        let ids = row(&db, &heat.id).await.external_ids;
        assert_eq!(
            (ids.tmdb, ids.imdb.as_deref()),
            (Some(949), Some("tt0113277"))
        );
        assert_eq!(list(&db, &heat.id).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn lifting_a_lock_on_the_works_identity_asks_for_its_refresh() {
        let db = db().await;
        let heat = film(&db, "Heat", 949).await;
        let later = crate::db::to_rfc3339(chrono::Utc::now() + chrono::TimeDelta::days(7));
        sqlx::query(db.sql("UPDATE media_item SET refresh_after = ? WHERE id = ?"))
            .bind(&later)
            .bind(&heat.id)
            .execute(db.pool())
            .await
            .unwrap();
        let due = |db: &Db| {
            let db = db.clone();
            async move {
                item::due_for_refresh(&db, 10)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>()
            }
        };

        // Another field lifted: the schedule stands.
        set(
            &db,
            &heat.id,
            Scope::Item,
            "overview",
            Some(&json!("x")),
            None,
        )
        .await
        .unwrap();
        assert!(unset(&db, &heat.id, Scope::Item, "overview").await.unwrap());
        assert!(due(&db).await.is_empty());

        // Adult lifted: the row is as the lock had it until its providers
        // are asked, which is now.
        set_on_work(
            &db,
            &heat.id,
            MediaKind::Movie,
            "isAdult",
            Some(&json!(true)),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(unset(&db, &heat.id, Scope::Item, "isAdult").await.unwrap());
        assert_eq!(due(&db).await, std::slice::from_ref(&heat.id));

        // Every lock at once, the identity among them: the same.
        sqlx::query(db.sql("UPDATE media_item SET refresh_after = ? WHERE id = ?"))
            .bind(&later)
            .bind(&heat.id)
            .execute(db.pool())
            .await
            .unwrap();
        set_on_work(
            &db,
            &heat.id,
            MediaKind::Movie,
            "slug",
            Some(&json!("heat-1995")),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(clear(&db, &heat.id).await.unwrap(), 1);
        assert_eq!(due(&db).await, [heat.id]);
    }
}
