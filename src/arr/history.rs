//! What was told to Sonarr and Radarr lately, and how it went.
//!
//! One row a work, written when its command was sent or could not be; the
//! page lists the newest. Only the latest [`KEPT`] stay: this is a view of
//! what happened lately, not a record anybody audits — the log has the rest.

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    arr::client::App,
    db::{Db, RowExt, new_id, now, text_list},
};

/// How many rows are kept.
pub const KEPT: i64 = 300;

/// How telling one work went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: a run's entry and a test of a destination each end in an
// `Outcome` too, and utoipa keys schemas on the leaf name alone.
#[schema(as = ArrPushOutcome)]
pub enum Outcome {
    /// Found there, and asked to read it again.
    Sent,
    /// Not held there, or not by any id this server serves it under: there
    /// was nothing to ask.
    Absent,
    /// It could not be asked: what went wrong is in the detail.
    Failed,
}

impl Outcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Absent => "absent",
            Self::Failed => "failed",
        }
    }

    /// An outcome as the history keeps it. One this version does not know —
    /// a later version's — reads as a failure: whatever came of it, the
    /// detail says.
    fn parse(text: &str) -> Self {
        match text {
            "sent" => Self::Sent,
            "absent" => Self::Absent,
            _ => Self::Failed,
        }
    }
}

/// One work told of, as the page lists it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: utoipa keys schemas on the leaf name alone.
#[schema(as = ArrPush)]
pub struct Push {
    pub id: String,
    pub at: String,
    pub app: App,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The id the work is served under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<i64>,
    /// Its own id there, when it holds the work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arr_id: Option<i64>,
    pub outcome: Outcome,
    /// Why it was told: `lock`, `edit`, `sync`, `refresh`.
    pub reasons: Vec<String>,
    /// The command it went in, or what went wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One row to write.
pub struct Told<'a> {
    pub app: App,
    pub media_id: &'a str,
    pub title: &'a str,
    pub client_id: Option<i64>,
    pub arr_id: Option<i64>,
    pub outcome: Outcome,
    pub reasons: &'a [String],
    pub detail: Option<&'a str>,
}

/// Write what came of telling some works, then let the oldest go.
pub async fn add(db: &Db, told: &[Told<'_>]) -> Result<()> {
    for row in told {
        sqlx::query(db.sql(
            "INSERT INTO arr_push (id, at, app, media_id, title, client_id, arr_id, outcome, reasons, detail)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        ))
        .bind(new_id())
        .bind(now())
        .bind(row.app.as_str())
        .bind(row.media_id)
        .bind(row.title)
        .bind(row.client_id)
        .bind(row.arr_id)
        .bind(row.outcome.as_str())
        .bind(text_list(row.reasons))
        .bind(row.detail)
        .execute(db.pool())
        .await?;
    }
    prune(db).await
}

/// Everything but the newest [`KEPT`] rows, gone. By id as well as by time:
/// a command for forty works writes forty rows in the same millisecond, and
/// ids made by one process follow the order they were made in.
async fn prune(db: &Db) -> Result<()> {
    sqlx::query(db.sql(
        "DELETE FROM arr_push WHERE id NOT IN
             (SELECT id FROM arr_push ORDER BY at DESC, id DESC LIMIT ?)",
    ))
    .bind(KEPT)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// The newest `limit` rows, newest first.
pub async fn latest(db: &Db, limit: i64) -> Result<Vec<Push>> {
    let rows = sqlx::query(db.sql(
        "SELECT id, at, app, media_id, title, client_id, arr_id, outcome, reasons, detail
         FROM arr_push ORDER BY at DESC, id DESC LIMIT ?",
    ))
    .bind(limit.clamp(1, KEPT))
    .fetch_all(db.pool())
    .await?;

    let mut pushes = Vec::with_capacity(rows.len());
    for row in &rows {
        // Only ever written for Sonarr or Radarr: a row naming another app
        // is a later version's, which this one has no place for on the page.
        let app = row.text("app")?;
        let Some(app) = App::ALL.into_iter().find(|known| known.as_str() == app) else {
            continue;
        };
        pushes.push(Push {
            id: row.text("id")?,
            at: row.text("at")?,
            app,
            media_id: row.opt_text("media_id")?,
            title: row.opt_text("title")?,
            client_id: row.opt_big("client_id")?,
            arr_id: row.opt_big("arr_id")?,
            outcome: Outcome::parse(&row.text("outcome")?),
            reasons: row.text_list("reasons")?,
            detail: row.opt_text("detail")?,
        });
    }
    Ok(pushes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::db;

    #[tokio::test]
    async fn the_newest_are_listed_first_and_the_oldest_let_go() {
        let db = db().await;
        let reasons = vec!["lock".to_string()];
        for n in 0..(KEPT + 5) {
            let title = format!("Work {n}");
            add(
                &db,
                &[Told {
                    app: App::Sonarr,
                    media_id: "work",
                    title: &title,
                    client_id: Some(81189),
                    arr_id: Some(12),
                    outcome: Outcome::Sent,
                    reasons: &reasons,
                    detail: Some("command 4521"),
                }],
            )
            .await
            .unwrap();
        }

        let kept = latest(&db, KEPT + 10).await.unwrap();
        assert_eq!(kept.len() as i64, KEPT);
        assert_eq!(
            kept[0].title.as_deref(),
            Some(&*format!("Work {}", KEPT + 4))
        );
        assert_eq!(kept[0].reasons, ["lock"]);
        assert_eq!(kept[0].arr_id, Some(12));
        assert!(
            kept.iter()
                .all(|push| push.title.as_deref() != Some("Work 0"))
        );
        // Sent as it is kept.
        let sent = serde_json::to_value(&kept[0]).unwrap();
        assert_eq!(sent["app"], "sonarr");
        assert_eq!(sent["outcome"], "sent");
    }

    #[tokio::test]
    async fn a_row_in_words_this_version_does_not_know_still_reads() {
        let db = db().await;
        for (id, app, outcome) in [("a", "radarr", "queued"), ("b", "lidarr", "sent")] {
            sqlx::query(db.sql(
                "INSERT INTO arr_push (id, at, app, media_id, title, outcome, reasons)
                 VALUES (?, ?, ?, 'work', 'A work', ?, '[]')",
            ))
            .bind(id)
            .bind(now())
            .bind(app)
            .bind(outcome)
            .execute(db.pool())
            .await
            .unwrap();
        }

        // An outcome it does not know is a failure, whose detail says the
        // rest; an app it does not know has no place on the page.
        let kept = latest(&db, 10).await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            (kept[0].app, kept[0].outcome),
            (App::Radarr, Outcome::Failed)
        );
    }
}
