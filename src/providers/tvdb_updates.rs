//! TheTVDB's feed of updates, read for the series it deleted or merged into
//! another (wave P, P4).
//!
//! `/updates` lists what changed at TheTVDB since an instant, a page at a
//! time: each record says what kind of record it is (`entityType`), what was
//! done to it (`method`, and `methodInt`: 1 created, 2 updated, 3 deleted),
//! which it is (`recordId`) and when (`timeStamp`); one deleted as the
//! duplicate of another names the record it was merged into (`mergeToId`,
//! and its kind, `mergeToType` — `mergeToEntityType` in TheTVDB's swagger),
//! nothing otherwise. Asked for the deletions
//! of series alone (`type=series&action=delete`), it says which series
//! TheTVDB no longer knows, and which of them it kept under another id: what
//! `service::numbering` freezes the numbering of.

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use super::tvdb::TvdbClient;

/// The most pages one reading follows: a week of TheTVDB's deletions of
/// series is a page or two, and a feed that goes on past this is not read to
/// its end — what it leaves is found by asking for each series again.
const MOST_PAGES: usize = 50;

/// A series TheTVDB deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedSeries {
    pub tvdb_id: i64,
    /// The series it was merged into as a duplicate, which TheTVDB keeps in
    /// its place.
    pub merged_into: Option<i64>,
    /// When, in seconds since the epoch.
    pub at: i64,
}

/// One record of the feed, as far as it is read: every field may be
/// missing or null, and a record that cannot be read is passed over.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    entity_type: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    method_int: Option<i64>,
    #[serde(default)]
    record_id: Option<i64>,
    #[serde(default)]
    time_stamp: Option<i64>,
    #[serde(default)]
    merge_to_id: Option<i64>,
    /// `mergeToType` in the feed as TheTVDB sends it (read on 3 November
    /// 2026); `mergeToEntityType` in its swagger.
    #[serde(default, alias = "mergeToType")]
    merge_to_entity_type: Option<String>,
}

impl Record {
    /// The series this record says TheTVDB deleted, when it says so.
    fn deleted_series(&self) -> Option<DeletedSeries> {
        let series = self
            .entity_type
            .as_deref()
            .is_none_or(|kind| kind.trim().eq_ignore_ascii_case("series"));
        let deleted = self.method_int == Some(3)
            || self
                .method
                .as_deref()
                .is_some_and(|method| method.trim().eq_ignore_ascii_case("delete"));
        let tvdb_id = self.record_id.filter(|id| *id > 0)?;
        if !series || !deleted {
            return None;
        }
        // Merged into a series, and not into itself: a record merged into a
        // film, or into nothing, is a series deleted.
        let into_a_series = self
            .merge_to_entity_type
            .as_deref()
            .map(str::trim)
            .is_none_or(|kind| kind.is_empty() || kind.eq_ignore_ascii_case("series"));
        let merged_into = self
            .merge_to_id
            .filter(|into| *into > 0 && *into != tvdb_id && into_a_series);
        Some(DeletedSeries {
            tvdb_id,
            merged_into,
            at: self.time_stamp.unwrap_or_default(),
        })
    }
}

/// The series one page of the feed says TheTVDB deleted, and whether it says
/// there is a page after it.
fn read_page(body: &Value) -> (Vec<DeletedSeries>, bool) {
    let deleted = body
        .get("data")
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(|record| Record::deserialize(record).ok())
                .filter_map(|record| record.deleted_series())
                .collect()
        })
        .unwrap_or_default();
    let more = body
        .pointer("/links/next")
        .and_then(Value::as_str)
        .is_some_and(|next| !next.trim().is_empty());
    (deleted, more)
}

/// Each series once, as its latest record says: a series deleted and then
/// deleted again as a duplicate is the duplicate.
fn latest_of_each(found: Vec<DeletedSeries>) -> Vec<DeletedSeries> {
    let mut latest: Vec<DeletedSeries> = Vec::new();
    for one in found {
        match latest.iter_mut().find(|held| held.tvdb_id == one.tvdb_id) {
            Some(held) => {
                held.merged_into = if one.at >= held.at {
                    one.merged_into.or(held.merged_into)
                } else {
                    held.merged_into.or(one.merged_into)
                };
                held.at = held.at.max(one.at);
            }
            None => latest.push(one),
        }
    }
    latest.sort_by_key(|one| (one.at, one.tvdb_id));
    latest
}

impl TvdbClient {
    /// The series TheTVDB deleted since `since`, in seconds since the epoch,
    /// each once, oldest first, with the series it was merged into when it
    /// was deleted as a duplicate. Empty when TheTVDB is not configured. An
    /// error when TheTVDB did not answer: what was not read is read at the
    /// next reading, from the same instant.
    pub async fn deleted_series(&self, since: i64) -> Result<Vec<DeletedSeries>> {
        if !self.is_enabled() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        for page in 0..MOST_PAGES {
            let path = format!("updates?since={since}&type=series&action=delete&page={page}");
            // A page past the last is answered 404, or with no records.
            let Some(body) = self.get(&path).await? else {
                break;
            };
            let (deleted, more) = read_page(&body);
            let empty = deleted.is_empty()
                && body
                    .get("data")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty);
            found.extend(deleted);
            if !more || empty {
                break;
            }
            if page + 1 == MOST_PAGES {
                tracing::warn!(
                    since,
                    pages = MOST_PAGES,
                    "TheTVDB's feed of deleted series goes on past what one reading follows"
                );
            }
        }
        Ok(latest_of_each(found))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use super::*;
    use crate::{api::relay::testing::StandIn, config};

    fn client_at(upstream: &str) -> TvdbClient {
        TvdbClient::new(
            reqwest::Client::new(),
            &config::Tvdb {
                upstream: upstream.into(),
                api_key: Some("key".into()),
                pin: None,
                enabled: true,
                passthrough: false,
            },
            "en",
            "test".into(),
        )
    }

    /// TheTVDB's feed as the stand-in answers it: two pages of deletions —
    /// a series deleted, one merged into another, an episode, a film merged
    /// into a series, a series updated — then nothing.
    fn feed(asked: &crate::api::relay::testing::Asked) -> (StatusCode, Value) {
        match asked.path.as_str() {
            "/login" => (StatusCode::OK, json!({ "data": { "token": "t" } })),
            "/updates" if asked.query.contains("page=0") => (
                StatusCode::OK,
                json!({
                    "data": [
                        // As TheTVDB's feed sends them: a series deleted
                        // names nothing, one merged names its kind
                        // `mergeToType`.
                        { "entityType": "series", "methodInt": 3, "method": "delete",
                          "recordType": "", "extraInfo": "",
                          "recordId": 81189, "timeStamp": 1_762_000_000 },
                        { "entityType": "series", "methodInt": 3, "method": "delete",
                          "recordId": 70001, "timeStamp": 1_762_000_100,
                          "mergeToId": 70002, "mergeToType": "series" },
                        { "entityType": "episodes", "methodInt": 3, "method": "delete",
                          "recordId": 5555, "timeStamp": 1_762_000_200, "seriesId": 81189,
                          "mergeToId": 0, "mergeToEntityType": "" },
                        { "entityType": "series", "methodInt": 2, "method": "update",
                          "recordId": 90001, "timeStamp": 1_762_000_300,
                          "mergeToId": 0, "mergeToEntityType": "" },
                        { "entityType": "series", "method": "delete",
                          "recordId": 60001, "timeStamp": 1_762_000_400,
                          "mergeToId": 60002, "mergeToType": "movie" },
                        "not a record"
                    ],
                    "links": { "prev": null, "self": "/v4/updates?page=0",
                               "next": "/v4/updates?page=1", "total_items": 6, "page_size": 6 }
                }),
            ),
            "/updates" if asked.query.contains("page=1") => (
                StatusCode::OK,
                json!({
                    "data": [
                        // The same series again, deleted as a duplicate since.
                        { "entityType": "series", "methodInt": 3, "method": "delete",
                          "recordId": 81189, "timeStamp": 1_762_000_500,
                          "mergeToId": 81190, "mergeToEntityType": "series" }
                    ],
                    "links": { "prev": "/v4/updates?page=0", "self": "/v4/updates?page=1",
                               "next": null, "total_items": 7, "page_size": 6 }
                }),
            ),
            _ => (StatusCode::NOT_FOUND, json!({ "status": "failure" })),
        }
    }

    /// Every deletion of a series the feed lists, each series once as its
    /// latest record says, oldest first; an episode, an update and a record
    /// that cannot be read are passed over, and a series merged into a film
    /// is a series deleted.
    #[tokio::test]
    async fn the_series_thetvdb_deleted_are_read_off_its_feed_page_by_page() {
        let stand_in = StandIn::start(feed).await;
        let tvdb = client_at(&stand_in.at);

        let deleted = tvdb.deleted_series(1_761_900_000).await.expect("read");
        assert_eq!(
            deleted,
            [
                DeletedSeries {
                    tvdb_id: 70001,
                    merged_into: Some(70002),
                    at: 1_762_000_100
                },
                DeletedSeries {
                    tvdb_id: 60001,
                    merged_into: None,
                    at: 1_762_000_400
                },
                DeletedSeries {
                    tvdb_id: 81189,
                    merged_into: Some(81190),
                    at: 1_762_000_500
                },
            ]
        );
        // The deletions of series alone, from the instant asked.
        let asked = stand_in.asked_at("/updates");
        assert_eq!(asked.len(), 2);
        for (page, one) in asked.iter().enumerate() {
            for part in [
                "since=1761900000".to_string(),
                "type=series".to_string(),
                "action=delete".to_string(),
                format!("page={page}"),
            ] {
                assert!(one.query.contains(&part), "{}", one.query);
            }
        }
    }

    /// TheTVDB not answering is an error, for the reading to be made again
    /// from the same instant; an empty feed is no deletion.
    #[tokio::test]
    async fn a_feed_thetvdb_did_not_answer_is_an_error_and_an_empty_one_nothing() {
        fn down(asked: &crate::api::relay::testing::Asked) -> (StatusCode, Value) {
            match asked.path.as_str() {
                "/login" => (StatusCode::OK, json!({ "data": { "token": "t" } })),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({ "status": "failure" }),
                ),
            }
        }
        fn quiet(asked: &crate::api::relay::testing::Asked) -> (StatusCode, Value) {
            match asked.path.as_str() {
                "/login" => (StatusCode::OK, json!({ "data": { "token": "t" } })),
                _ => (
                    StatusCode::OK,
                    json!({ "data": null, "links": { "next": null } }),
                ),
            }
        }
        let stand_in = StandIn::start(down).await;
        assert!(client_at(&stand_in.at).deleted_series(1).await.is_err());

        let stand_in = StandIn::start(quiet).await;
        assert_eq!(client_at(&stand_in.at).deleted_series(1).await.unwrap(), []);
    }
}
