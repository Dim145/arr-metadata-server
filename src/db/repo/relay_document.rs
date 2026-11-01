//! The reference lists the relays keep in the database (migration 0047):
//! TMDB's configuration, genres, certifications and watch-provider regions,
//! TheTVDB's languages, genres, types and statuses — what a client asks for
//! first and cannot start without. Read when the service fails and the
//! memory holds nothing, not even a stale copy (`api::relay::stand_in`).
//!
//! Never AniList's: its terms want nothing stored, and the table refuses
//! any relay but TMDB's and TheTVDB's. Nothing older than [`KEEP_DAYS`] is
//! read back, and [`purge`] deletes it.

use anyhow::Result;

use crate::db::{Db, RowExt, to_rfc3339};

/// The most days a document is kept and served: TMDB's terms allow six
/// months, and TheTVDB's lists keep to the same rule.
pub const KEEP_DAYS: i64 = 180;

/// A document kept, as the service answered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub content_type: String,
    /// The document as the service wrote it: JSON.
    pub body: String,
    /// When the service answered it, RFC 3339.
    pub fetched_at: String,
}

/// The instant before which a document is past keeping: [`KEEP_DAYS`] ago.
pub fn cutoff() -> String {
    to_rfc3339(chrono::Utc::now() - chrono::TimeDelta::days(KEEP_DAYS))
}

/// Keep a document `relay` was answered — `tmdb` or `tvdb` — under `key`,
/// in place of whatever was kept under it.
pub async fn write(db: &Db, relay: &str, key: &str, document: &Document) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO relay_document (relay, cache_key, content_type, body, fetched_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (relay, cache_key) DO UPDATE
         SET content_type = excluded.content_type, body = excluded.body,
             fetched_at = excluded.fetched_at",
    ))
    .bind(relay)
    .bind(key)
    .bind(&document.content_type)
    .bind(&document.body)
    .bind(&document.fetched_at)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// The document `relay` kept under `key`, unless it was fetched before
/// `not_before` — [`cutoff`], for one that may still be served.
pub async fn read(db: &Db, relay: &str, key: &str, not_before: &str) -> Result<Option<Document>> {
    let row = sqlx::query(db.sql(
        "SELECT content_type, body, fetched_at FROM relay_document
          WHERE relay = ? AND cache_key = ? AND fetched_at >= ?",
    ))
    .bind(relay)
    .bind(key)
    .bind(not_before)
    .fetch_optional(db.pool())
    .await?;

    row.map(|row| {
        Ok(Document {
            content_type: row.text("content_type")?,
            body: row.text("body")?,
            fetched_at: row.text("fetched_at")?,
        })
    })
    .transpose()
}

/// Delete the documents fetched before `before` (RFC 3339) — [`cutoff`],
/// for those past keeping. How many went.
pub async fn purge(db: &Db, before: &str) -> Result<u64> {
    let result = sqlx::query(db.sql("DELETE FROM relay_document WHERE fetched_at < ?"))
        .bind(before)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{now, testing::db};

    fn document(body: &str, fetched_at: &str) -> Document {
        Document {
            content_type: "application/json;charset=utf-8".into(),
            body: body.into(),
            fetched_at: fetched_at.into(),
        }
    }

    #[tokio::test]
    async fn a_reference_list_is_kept_read_back_and_written_over() {
        let db = db().await;
        let key = "https://api.themoviedb.org:/3/genre/movie/list?language=fr-FR";
        assert_eq!(read(&db, "tmdb", key, &cutoff()).await.unwrap(), None);

        let first = document(r#"{"genres":[{"id":28,"name":"Action"}]}"#, &now());
        write(&db, "tmdb", key, &first).await.unwrap();
        assert_eq!(
            read(&db, "tmdb", key, &cutoff()).await.unwrap(),
            Some(first)
        );
        // Another relay's, and another key, are other documents.
        assert_eq!(read(&db, "tvdb", key, &cutoff()).await.unwrap(), None);
        assert_eq!(
            read(&db, "tmdb", "/3/genre/tv/list", &cutoff())
                .await
                .unwrap(),
            None
        );

        let second = document(r#"{"genres":[]}"#, &now());
        write(&db, "tmdb", key, &second).await.unwrap();
        assert_eq!(
            read(&db, "tmdb", key, &cutoff()).await.unwrap(),
            Some(second)
        );
    }

    #[tokio::test]
    async fn a_list_past_six_months_is_neither_served_nor_kept() {
        let db = db().await;
        let old = to_rfc3339(chrono::Utc::now() - chrono::TimeDelta::days(KEEP_DAYS + 1));
        let recent = to_rfc3339(chrono::Utc::now() - chrono::TimeDelta::days(KEEP_DAYS - 1));
        write(&db, "tvdb", "/v4/languages", &document("[]", &old))
            .await
            .unwrap();
        write(&db, "tvdb", "/v4/genres", &document("[]", &recent))
            .await
            .unwrap();

        assert_eq!(
            read(&db, "tvdb", "/v4/languages", &cutoff()).await.unwrap(),
            None,
            "past keeping, not served"
        );
        assert!(
            read(&db, "tvdb", "/v4/genres", &cutoff())
                .await
                .unwrap()
                .is_some()
        );

        assert_eq!(purge(&db, &cutoff()).await.unwrap(), 1);
        let left = sqlx::query(db.sql("SELECT cache_key FROM relay_document"))
            .fetch_all(db.pool())
            .await
            .unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].text("cache_key").unwrap(), "/v4/genres");
    }

    #[tokio::test]
    async fn anilist_is_never_kept() {
        let db = db().await;
        assert!(
            write(&db, "anilist", "q", &document("{}", &now()))
                .await
                .is_err(),
            "the table refuses any relay but TMDB's and TheTVDB's"
        );
    }
}
