//! Keeping the search index current; see `crate::search`.
//!
//! A write indexes its own work as it happens. What is left comes here: a
//! work whose names changed some other way — a title locked, an alternative
//! title or an episode added by hand — which the database marks as behind,
//! and every work after the upgrade that brought the index, which start
//! marked. Worked through a few hundred at a time, every few seconds while
//! anything is behind, by the instance that runs the schedules.
//!
//! The task `search.rebuild` writes the whole index again — every work's rows
//! and everybody's — for an administrator who doubts it: the rows written in
//! place, so that the search answers throughout.

use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::Result;

use crate::{
    db::{
        Db, Dialect, RowExt,
        repo::{self, job},
    },
    error::{AppError, AppResult},
    state::AppState,
};

/// The task's name, as the history files its runs.
pub const KIND: &str = "search.rebuild";

/// What one rebuild at a time is held under, among several instances.
pub const HOLD: &str = "search";

/// What a rebuild says when it is done, in words the history translates.
pub const DONE: &str = "search index built again";

/// How long after the start the index is first looked at: the server is
/// answering by then.
const FIRST_AFTER: Duration = Duration::from_secs(5);

/// How often the works behind are looked for: what a lock changed is found
/// by its new name within this.
const TICK: Duration = Duration::from_secs(5);

/// The most works one pass writes. An index built from nothing — fifty
/// thousand works after the upgrade — is written in passes a moment apart,
/// the database shared with everything else meanwhile.
const MOST: usize = 500;

/// One pass, or one rebuild, at a time on this instance.
static WORKING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether the index is being written again from scratch, or caught up, on
/// this instance.
pub fn is_busy() -> bool {
    WORKING.try_lock().is_err()
}

/// Run until the process shuts down.
pub async fn run(state: AppState) {
    tokio::time::sleep(FIRST_AFTER).await;
    check_engine(&state.db).await;
    if let Err(e) = heal(&state.db).await {
        tracing::warn!(
            error = format_args!("{e:#}"),
            "could not look at the search index"
        );
    }

    loop {
        let mut more = false;
        // The leader's work, as the other schedules are; never while a backup
        // is being restored under it.
        if state.coord.leads()
            && !state.maintenance.is_on()
            && let Ok(_held) = WORKING.try_lock()
        {
            match repo::search::catch_up(&state.db, MOST).await {
                Ok((written, behind)) => {
                    if written > 0 {
                        tracing::debug!(works = written, "wrote works into the search index");
                        // What a search answered before these rows were
                        // written is kept under the catalogue's mark: moved
                        // on, here and on every instance, the next search
                        // reads them.
                        state.caches.bump_epoch().await;
                    }
                    more = behind;
                }
                Err(e) => tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not write works into the search index; trying again shortly"
                ),
            }
        }
        let pause = if more {
            Duration::from_millis(200)
        } else {
            TICK
        };
        tokio::time::sleep(pause).await;
    }
}

/// An index with no row while the catalogue holds works none of which is
/// marked as behind is one that lost its rows — a table emptied by hand,
/// an archive restored without it: every work is marked, and written again.
async fn heal(db: &Db) -> Result<()> {
    let standing = repo::search::standings(db).await?;
    let rows = standing.works + standing.people + standing.episodes;
    if rows == 0 && standing.held > 0 && standing.behind == 0 {
        let marked = repo::search::mark_all_stale(db).await?;
        tracing::info!(
            works = marked,
            "the search index is empty: writing it again from every work"
        );
    } else if standing.behind > 0 {
        tracing::info!(
            works = standing.behind,
            "writing the works the search index is behind on, in the background"
        );
    }
    Ok(())
}

/// Say, once at the start, what a PostgreSQL's search will do without: no
/// `pg_trgm` — every row read, no misspelling forgiven — or a cluster made
/// with `LC_CTYPE=C`, under which `pg_trgm` takes no trigram of a letter
/// outside ASCII, so that a Japanese title is read row by row.
async fn check_engine(db: &Db) {
    if db.dialect() != Dialect::Postgres {
        return;
    }
    match crate::search::candidates::has_trigrams(db).await {
        Ok(false) => {
            tracing::warn!(
                "pg_trgm is not installed: a search reads every row of the index, and a \
                 misspelt term finds nothing. CREATE EXTENSION pg_trgm, then run the task \
                 search.rebuild"
            );
            return;
        }
        Ok(true) => {}
        Err(e) => {
            tracing::warn!(error = format_args!("{e:#}"), "could not look for pg_trgm");
            return;
        }
    }
    let probe = sqlx::query(
        db.sql("SELECT COALESCE(array_length(show_trgm('日本語'), 1), 0)::bigint AS n"),
    )
    .fetch_one(db.pool())
    .await;
    match probe.map(|row| row.big("n")) {
        Ok(Ok(0)) => tracing::warn!(
            "pg_trgm takes no trigram of a letter outside ASCII on this database, whose \
             LC_CTYPE is C: a title in another script is searched by reading every row. A \
             database made with LC_CTYPE=C.UTF-8 does not have that limit"
        ),
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "could not read pg_trgm's probe"),
        Err(e) => tracing::warn!(error = %e, "could not ask pg_trgm for trigrams"),
    }
}

/// Write the whole index again, in the background: every work marked and
/// written, then everybody the credits name. Refused while it, or a pass of
/// the catch-up, runs here, or while a rebuild runs on another instance.
pub async fn rebuild_now(state: &AppState, by: &str) -> AppResult<Option<String>> {
    if state.maintenance.is_on() {
        return Err(AppError::Conflict("a backup is being restored".into()));
    }
    let held = WORKING
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::Conflict("the search index is being written".into()))?;
    let shared = state
        .coord
        .hold_for(HOLD, "the search index is being written")
        .await?;
    let record = job::start_by(&state.db, KIND, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let db = &state.db;
        let outcome = rebuild(db).await;
        // Every search answered before is of the index as it was.
        state.caches.bump_epoch().await;
        let closed = match &outcome {
            Ok(()) => job::finish(db, &record, Some(DONE), None).await,
            Err(e) => job::finish(db, &record, None, Some(&format!("{e:#}"))).await,
        };
        if let Err(e) = closed {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not close the search index's run"
            );
        }
    });
    Ok(Some(id))
}

/// Every work's rows and everybody's, written again.
pub async fn rebuild(db: &Db) -> Result<()> {
    let marked = repo::search::mark_all_stale(db).await?;
    let mut works = 0;
    loop {
        let (written, more) = repo::search::catch_up(db, MOST).await?;
        works += written;
        if !more {
            break;
        }
    }
    let people = repo::search::index_everybody(db).await?;
    tracing::info!(works, marked, people, "the search index was built again");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::{
            repo::item::{ItemWrite, upsert},
            testing::db,
        },
        domain::{Credit, CreditType, MediaItem, MediaKind},
    };

    #[tokio::test]
    async fn the_rebuild_writes_every_row_again_and_takes_out_the_stray_ones() {
        let db = db().await;
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Paris, Texas".into();
        item.original_title = Some("Paris, Texas".into());
        item.slug = "paris-texas-1984".into();
        item.year = Some(1984);
        item.credits = vec![Credit {
            id: String::new(),
            credit_type: CreditType::Director,
            person_name: "Wim Wenders".into(),
            character_name: None,
            image: None,
            tmdb_person_id: Some(2725),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        }];
        upsert(
            &db,
            ItemWrite {
                item: &item,
                replace_children: true,
            },
        )
        .await
        .expect("stored");
        let written = repo::search::standings(&db).await.expect("read");

        // A row nothing names any more, and the work's own taken out by hand.
        sqlx::query(db.sql(
            "INSERT INTO search_doc (id, kind, ref_id, work_id, lang, norm, text, popularity, year)
             VALUES (?, 'person', 'tmdb:1', NULL, NULL, 'nobody', 'Nobody', 1, NULL)",
        ))
        .bind(crate::search::index::entry_id(
            crate::search::Kind::Person,
            "tmdb:1",
            "nobody",
        ))
        .execute(db.pool())
        .await
        .expect("a stray row");
        sqlx::query(db.sql("DELETE FROM search_doc WHERE kind = 'work'"))
            .execute(db.pool())
            .await
            .expect("taken out");

        rebuild(&db).await.expect("rebuilt");

        let rebuilt = repo::search::standings(&db).await.expect("read");
        assert_eq!(rebuilt, written, "as the write had them, nothing more");
        assert_eq!(rebuilt.behind, 0);
        assert!(rebuilt.works > 0 && rebuilt.people == 1);
    }
}
