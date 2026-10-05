//! The refresh scheduler.
//!
//! Any entry carrying at least one external id is refetched on a schedule that
//! follows its status: a running series changes weekly, an ended one never. The
//! refresh replaces provider snapshots and provider-sourced children. It does
//! not touch `media_override`, which is what makes a manual edit permanent.

use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::Result;
use futures::FutureExt as _;

use crate::{
    db::{
        repo::{self, job},
        to_rfc3339,
    },
    domain::{MediaItem, MediaKind},
    service::series,
    state::AppState,
};

/// Refetch one entry from its provider.
///
/// Returns the refreshed entry, or `None` if there was nothing to refresh from.
pub async fn refresh_one(state: &AppState, item: &MediaItem) -> Result<Option<MediaItem>> {
    let ids = &item.external_ids;

    if ids.is_empty() {
        tracing::debug!(id = %item.id, "nothing to refresh: no external ids");
        return Ok(None);
    }

    let refreshed = match item.kind {
        MediaKind::Series => refresh_series(state, item).await?,
        MediaKind::Movie => refresh_movie(state, item).await?,
    };

    Ok(refreshed)
}

async fn refresh_series(state: &AppState, item: &MediaItem) -> Result<Option<MediaItem>> {
    let ids = &item.external_ids;

    // A Fan-Kai has one source. It carries no TheTVDB or TMDB id for the
    // others to be asked by, and one given by hand would have them answer for
    // the anime it was cut from.
    if let Some(fankai_id) = ids.fankai {
        return crate::service::gather::fankai_series(state, fankai_id).await;
    }

    // Both ids are already known here, so every provider is asked at once
    // rather than one resolving the other first.
    if (ids.tmdb.is_some() || ids.tvdb.is_some())
        && let Some(refreshed) = force_series(state, ids.tmdb, ids.tvdb).await?
    {
        return Ok(Some(refreshed));
    }

    // Fetched again, not looked up: the copy held is no refresh. Through the
    // lookup Sonarr's requests take, it came back — fresh as it was, or kept
    // because nobody answered — and a refresh nobody answered was reported
    // as one from a provider.
    if let Some(tvdb_id) = ids.tvdb {
        return series::refetch_by_tvdb_id(state, tvdb_id).await;
    }

    Ok(None)
}

/// Refetch from every provider, bypassing the local-first ladder.
async fn force_series(
    state: &AppState,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
) -> Result<Option<MediaItem>> {
    crate::service::gather::series(state, tmdb_id, tvdb_id).await
}

async fn refresh_movie(state: &AppState, item: &MediaItem) -> Result<Option<MediaItem>> {
    let ids = &item.external_ids;

    if ids.tmdb.is_none() && ids.imdb.is_none() {
        return Ok(None);
    }

    crate::service::gather::movie(state, ids.tmdb, ids.imdb.as_deref()).await
}

/// Writes down what a run did to each work, in the order it took them: the
/// detail the history opens under a run's one-line summary. Without a run —
/// one whose row could not be opened — nothing is written, and the work goes
/// on regardless.
pub struct Recorder {
    run: Option<String>,
    position: std::sync::atomic::AtomicI64,
}

impl Recorder {
    pub fn for_run(run: Option<&str>) -> Self {
        Self {
            run: run.map(str::to_string),
            position: std::sync::atomic::AtomicI64::new(0),
        }
    }

    /// One work's outcome, with the work as it was read when it could be.
    /// Losing the note must not stop the work: it is logged, not raised.
    pub async fn note(
        &self,
        db: &crate::db::Db,
        id: &str,
        work: Option<&MediaItem>,
        outcome: job::Outcome,
        note: &str,
    ) {
        let Some(run) = &self.run else { return };
        let position = self
            .position
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let touched = job::Touched {
            id,
            title: work.map(|w| w.title.as_str()),
            kind: work.map(|w| match w.kind {
                MediaKind::Series => "series",
                MediaKind::Movie => "movie",
            }),
        };
        if let Err(e) = job::add_entry(db, run, position, touched, outcome, Some(note)).await {
            tracing::debug!(%id, error = %e, "could not write down what the run did to a work");
        }
    }
}

/// Held by whatever is refreshing works in bulk — a sweep, the schedule's or
/// one asked for by hand, or a refresh of everything — so that two never run
/// at once and ask the providers the same thing twice.
static SWEEPING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

fn busy() -> crate::error::AppError {
    crate::error::AppError::Conflict(
        "works are being refreshed already; its run is in the history".into(),
    )
}

/// What every bulk refresh is held under among several instances.
pub const HOLD: &str = "refresh";

/// One sweep now, as `by` asked, in the background: the run's id at once.
pub async fn sweep_now(state: &AppState, by: &str) -> crate::error::AppResult<Option<String>> {
    let held = SWEEPING.clone().try_lock_owned().map_err(|_| busy())?;
    let shared = state
        .coord
        .hold_for(HOLD, "works are being refreshed")
        .await?;
    let record = job::start_by(&state.db, job::kinds::REFRESH_SWEEP, None, Some(by))
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "could not open a job run"))
        .ok();

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let recorder = Recorder::for_run(record.as_deref());
        close(
            &state,
            record,
            sweep(&state, batch(&state), &recorder).await,
        )
        .await;
    });

    Ok(id)
}

/// Every work that has an identifier elsewhere, refreshed now, as `by`
/// asked: one pass over them in id order, gently, the run saying how far it
/// has got. It can be stopped between two works, and nothing about the works'
/// own schedule is touched to make it — stopped, the rest simply keep theirs.
pub async fn refresh_everything(
    state: &AppState,
    by: &str,
) -> crate::error::AppResult<Option<String>> {
    let held = SWEEPING.clone().try_lock_owned().map_err(|_| busy())?;
    let shared = state
        .coord
        .hold_for(HOLD, "works are being refreshed")
        .await?;
    let total = repo::item::count_refresh_candidates(&state.db).await?;
    // Not without its record: a run nobody can see is a run nobody can stop.
    let record = job::start_by(&state.db, job::kinds::REFRESH_ALL, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let flag = super::cancel::register(&record);
        match everything(&state, total, &record, &flag).await {
            Ok(Pass::Done(summary)) => close(&state, Some(record), Ok(summary)).await,
            Ok(Pass::Stopped(summary)) => {
                if let Err(e) = job::stop(&state.db, &record, &summary).await {
                    tracing::warn!(error = %e, "could not close the stopped run");
                }
            }
            Err(e) => close(&state, Some(record), Err(e)).await,
        }
    });

    Ok(Some(id))
}

/// The pause between two works of a refresh of everything: providers are
/// asked for thousands of things, and are not in a hurry.
const EVERYTHING_PAUSE: Duration = Duration::from_millis(250);

/// How a pass over everything ended.
enum Pass {
    Done(String),
    Stopped(String),
}

async fn everything(
    state: &AppState,
    total: i64,
    record: &str,
    flag: &super::cancel::Registered,
) -> Result<Pass> {
    let (mut done, mut failed) = (0i64, 0i64);
    let mut after: Option<String> = None;
    let recorder = Recorder::for_run(Some(record));

    loop {
        let batch = repo::item::refresh_candidates(&state.db, after.as_deref(), 25).await?;
        let Some((last, _)) = batch.last() else {
            break;
        };
        after = Some(last.clone());

        for (id, _kind) in batch {
            if flag.stopped() {
                return Ok(Pass::Stopped(format!(
                    "stopped: {done} refreshed, {failed} failed, of {total}"
                )));
            }

            if refresh_due(state, &id, &recorder).await {
                done += 1;
            } else {
                failed += 1;
            }

            if (done + failed) % 5 == 0
                && let Err(e) = job::progress(
                    &state.db,
                    record,
                    &format!(
                        "{} of {total}: {done} refreshed, {failed} failed",
                        done + failed
                    ),
                )
                .await
            {
                tracing::debug!(error = %e, "could not say how far the refresh got");
            }

            tokio::time::sleep(EVERYTHING_PAUSE).await;
        }
    }

    Ok(Pass::Done(format!(
        "{done} refreshed, {failed} failed, of {total}"
    )))
}

/// Whether works are being refreshed in bulk right now — what tells a run
/// that is still marked running from one a crash left behind.
pub fn is_busy() -> bool {
    SWEEPING.try_lock().is_err()
}

/// Close a run with what came of it.
async fn close(state: &AppState, record: Option<String>, outcome: Result<String>) {
    if let Some(record) = record {
        let closed = match &outcome {
            Ok(summary) => job::finish(&state.db, &record, Some(summary), None).await,
            Err(e) => job::finish(&state.db, &record, None, Some(&format!("{e:#}"))).await,
        };
        if let Err(e) = closed {
            tracing::warn!(error = %e, "could not close the job run");
        }
    }
    if let Err(e) = outcome {
        tracing::error!(error = ?e, "a bulk refresh failed");
    }
}

/// Run the scheduler until the process shuts down.
pub async fn run(state: AppState) {
    // The scheduler always runs. Whether it does anything is a setting now, and
    // it is read on every tick — an operator who turns refresh off, or moves the
    // interval, should not have to restart the server for it to take.
    tracing::info!(
        interval_secs = interval(&state).as_secs(),
        batch = batch(&state),
        enabled = state.flag("refresh.enabled", true),
        "refresh scheduler started"
    );

    // Let the server finish starting before the first sweep — and, among
    // several instances, hear the others announce themselves.
    tokio::time::sleep(Duration::from_secs(30)).await;

    // A run is only ever closed by the task that opened it, so a process killed
    // mid-sweep leaves one behind. Close those before opening any more — this
    // instance's own, among several; not while another instance goes by the
    // same name, whose live runs would be closed with them.
    if state.coord.has_twin() {
        tracing::warn!(
            "runs left open by a previous stop are not closed: another instance goes by this \
             instance's name"
        );
    } else {
        match job::fail_orphaned(&state.db, state.coord.is_multi(), job::process_started()).await {
            Ok(0) => {}
            Ok(closed) => tracing::warn!(closed, "closed job runs left open by a previous stop"),
            Err(e) => tracing::warn!(error = %e, "could not close orphaned job runs"),
        }
    }

    // The tick is the shortest the setting allows, and each wake decides
    // whether enough time has passed — rather than rebuilding the ticker, which
    // would mean tracking when it last fired anyway.
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut swept = tokio::time::Instant::now();

    loop {
        ticker.tick().await;

        // One rate-limit bucket is kept per address seen; drop the quiet ones.
        // This instance's own, whether or not it leads.
        state.limiter.prune();

        // The rest is the leader's: alone, always this instance; among
        // several, the one holding the lease.
        if !state.coord.leads() {
            continue;
        }

        // Not while works are being refreshed by hand, here or elsewhere:
        // the next tick asks again.
        if state.flag("refresh.enabled", true)
            && swept.elapsed() >= interval(&state)
            && let Ok(_held) = SWEEPING.try_lock()
            && let Some(_shared) = state.coord.hold(HOLD).await
        {
            swept = tokio::time::Instant::now();
            run_sweep(&state, batch(&state)).await;
        }

        // Below the sweep, and deliberately not inside it. Turning refresh off
        // is a statement about talking to providers, not about housekeeping —
        // and it used to skip all of this, so an operator who switched it off
        // got a rate limiter whose per-address map grew until the process died
        // and an audit table that was never pruned again.

        // Expired sessions accumulate otherwise; this is as good a moment as any.
        if let Err(e) = repo::user::purge_expired_sessions(&state.db).await {
            tracing::warn!(error = %e, "could not purge expired sessions");
        }

        prune_audit(&state).await;
        prune_jobs(&state).await;
        prune_callers(&state).await;
    }
}

pub fn interval(state: &AppState) -> Duration {
    state
        .settings
        .int_at("refresh.intervalSeconds", None, None)
        .and_then(|n| u64::try_from(n).ok())
        .map(Duration::from_secs)
        .unwrap_or(state.config.refresh.interval)
}

fn batch(state: &AppState) -> i64 {
    state
        .settings
        .int_at("refresh.batchSize", None, None)
        .unwrap_or(i64::from(state.config.refresh.batch_size))
}

/// Run one sweep, recording it as a job so an operator can see it happened.
async fn run_sweep(state: &AppState, batch: i64) {
    let record = match job::start(&state.db, job::kinds::REFRESH_SWEEP, None).await {
        Ok(id) => Some(id),
        Err(e) => {
            // Losing the record must not stop the work.
            tracing::warn!(error = %e, "could not open a job run");
            None
        }
    };

    let recorder = Recorder::for_run(record.as_deref());
    let outcome = sweep(state, batch, &recorder).await;

    let Some(record) = record else { return };

    let closed = match &outcome {
        Ok(summary) => job::finish(&state.db, &record, Some(summary), None).await,
        Err(e) => job::finish(&state.db, &record, None, Some(&e.to_string())).await,
    };

    if let Err(e) = closed {
        tracing::warn!(error = %e, "could not close the job run");
    }

    if let Err(e) = outcome {
        tracing::error!(error = ?e, "refresh sweep failed");
    }
}

/// Refresh what is due, returning a one-line summary of what happened; each
/// work taken is written down for the run.
async fn sweep(state: &AppState, batch: i64, recorder: &Recorder) -> Result<String> {
    let due = repo::item::due_for_refresh(&state.db, batch).await?;

    if due.is_empty() {
        return Ok("nothing was due".to_string());
    }

    tracing::info!(count = due.len(), "refreshing entries");

    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for (id, _kind) in due {
        if refresh_due(state, &id, recorder).await {
            succeeded += 1;
        } else {
            failed += 1;
        }
    }

    tracing::info!(succeeded, failed, "refresh sweep finished");
    Ok(format!("{succeeded} refreshed, {failed} failed"))
}

/// The fixed notes a run leaves on a work, which the interface puts in words.
pub mod notes {
    pub const REFRESHED: &str = "refreshed from a provider";
    pub const KEPT: &str = "no provider answered; the stored entry was kept";
    pub const UNRESOLVED: &str = "no provider could resolve this entry";
    pub const GONE: &str = "gone since it was listed";
}

/// Refresh one work that is due; whether it came back from a provider. What
/// happened to it is written down for the run.
///
/// Never `?`: a work that cannot be read — a bad override, a corrupt row —
/// would otherwise end the sweep before the rest of the batch was touched,
/// and never have its own deadline pushed out: it sorts first by
/// `refresh_after`, so it would be the first work of every sweep from then
/// on, and nothing in the library would be refreshed again.
async fn refresh_due(state: &AppState, id: &str, recorder: &Recorder) -> bool {
    let item = match crate::service::load(state, id).await {
        Ok(Some(item)) => item,
        // Gone since it was listed.
        Ok(None) => {
            recorder
                .note(&state.db, id, None, job::Outcome::Skipped, notes::GONE)
                .await;
            return true;
        }
        Err(e) => {
            tracing::warn!(%id, error = format_args!("{e:#}"), "could not read an entry due for refresh");
            let why = format!("could not be read: {e}");
            mark_failure(state, id, None, &why).await;
            recorder
                .note(&state.db, id, None, job::Outcome::Failed, &why)
                .await;
            return false;
        }
    };

    // A panic refreshing one work — on a provider's answer, in the merge —
    // fails that work, not the sweep: unwound, it would end the loop that
    // runs every sweep for good, and the work, still first by its deadline,
    // would be the first of the next.
    let refreshed = std::panic::AssertUnwindSafe(refresh_one(state, &item))
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(anyhow::anyhow!("the refresh panicked: {what}"))
        });

    let (came_back, outcome, note) = match refreshed {
        // Answered with what was stored — no provider had it — and still due:
        // pushed out, or it would take the first slot of every sweep.
        Ok(Some(fresh))
            if fresh
                .refresh_after
                .as_deref()
                .is_some_and(|at| at <= crate::db::now().as_str()) =>
        {
            mark_failure(state, id, Some(&item), notes::KEPT).await;
            (false, job::Outcome::Failed, notes::KEPT.to_string())
        }
        // Written, with what the providers that failed gave before: the
        // failure is on the work, and it is tried again sooner.
        Ok(Some(fresh)) if fresh.refresh_error.is_some() => (
            false,
            job::Outcome::Failed,
            fresh.refresh_error.unwrap_or_default(),
        ),
        Ok(Some(_)) => (true, job::Outcome::Ok, notes::REFRESHED.to_string()),
        Ok(None) => {
            // Nothing to refresh from. Push the deadline out so this entry
            // does not occupy a slot in every future batch.
            mark_failure(state, id, Some(&item), notes::UNRESOLVED).await;
            (false, job::Outcome::Failed, notes::UNRESOLVED.to_string())
        }
        Err(e) => {
            tracing::warn!(%id, error = format_args!("{e:#}"), "refresh failed");
            let why = e.to_string();
            mark_failure(state, id, Some(&item), &why).await;
            (false, job::Outcome::Failed, why)
        }
    };
    recorder
        .note(&state.db, id, Some(&item), outcome, &note)
        .await;
    came_back
}

/// Drop job runs past the retention window, which shares the audit setting.
async fn prune_jobs(state: &AppState) {
    let days = state.config.security.audit_retention_days;
    if days == 0 {
        return;
    }

    let Some(cutoff) = days_ago(i64::from(days)) else {
        tracing::warn!(days, "the retention window is too long to be a date");
        return;
    };

    match job::prune(&state.db, &cutoff).await {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, "pruned job runs"),
        Err(e) => tracing::warn!(error = %e, "could not prune job runs"),
    }
}

/// Forget callers nobody has seen for a fortnight.
///
/// The table is meant to answer "what is using this server", not "what ever
/// touched it". A client that has been gone two weeks is not the answer to
/// either question, and its address may well belong to something else by now.
async fn prune_callers(state: &AppState) {
    let Some(cutoff) = days_ago(14) else { return };

    match repo::network::prune(&state.db, &cutoff).await {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, "forgot callers not seen recently"),
        Err(e) => tracing::warn!(error = %e, "could not prune the callers table"),
    }
}

/// Drop audit entries past the retention window.
///
/// The trail is append-only and grows with use; without this a long-running
/// instance accumulates it forever. `0` days means the operator wants it kept.
async fn prune_audit(state: &AppState) {
    let days = state.config.security.audit_retention_days;
    if days == 0 {
        return;
    }

    let Some(cutoff) = days_ago(i64::from(days)) else {
        tracing::warn!(days, "the retention window is too long to be a date");
        return;
    };

    match repo::audit::prune(&state.db, &cutoff).await {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, retention_days = days, "pruned audit entries"),
        Err(e) => tracing::warn!(error = %e, "could not prune the audit log"),
    }
}

/// A timestamp that many days ago, or `None` if there is no such date.
///
/// `AMS_AUDIT_RETENTION_DAYS` is an operator-supplied `u32` and chrono panics
/// rather than saturating on a subtraction that leaves the representable range.
/// A typo in a compose file is a bad reason for the process to abort thirty
/// seconds after it starts.
fn days_ago(days: i64) -> Option<String> {
    let delta = chrono::TimeDelta::try_days(days)?;
    chrono::Utc::now().checked_sub_signed(delta).map(to_rfc3339)
}

/// Record a failed refresh, and when to try again: see
/// [`crate::service::retry_after_failure`]. `item` is the work as it was read,
/// when it could be.
async fn mark_failure(state: &AppState, id: &str, item: Option<&MediaItem>, error: &str) {
    let next = crate::service::retry_after_failure(item);
    let error = crate::providers::clip(error, crate::service::REFRESH_ERROR_CHARS);

    if let Err(e) = repo::item::mark_refreshed(&state.db, id, Some(&next), Some(&error)).await {
        tracing::warn!(%id, error = %e, "could not record the refresh failure");
    }

    state.caches.touched(id).await;
}
