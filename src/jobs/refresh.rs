//! The refresh scheduler.
//!
//! Any entry carrying at least one external id is refetched on a schedule that
//! follows its status: a running series changes weekly, an ended one never. The
//! refresh replaces provider snapshots and provider-sourced children. It does
//! not touch `media_override`, which is what makes a manual edit permanent.

use std::time::Duration;

use anyhow::Result;

use crate::{
    db::{
        repo::{self, job},
        to_rfc3339,
    },
    domain::{MediaItem, MediaKind},
    service::series,
    state::AppState,
};

/// How long to wait after a failure before trying that entry again.
///
/// Without this, an id that has been deleted upstream would be retried on every
/// scheduler tick forever.
const FAILURE_BACKOFF_HOURS: i64 = 6;

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

    // Both ids are already known here, so every provider is asked at once
    // rather than one resolving the other first.
    if (ids.tmdb.is_some() || ids.tvdb.is_some())
        && let Some(refreshed) = force_series(state, ids.tmdb, ids.tvdb).await?
    {
        return Ok(Some(refreshed));
    }

    if let Some(tvdb_id) = ids.tvdb {
        return series::by_tvdb_id(state, tvdb_id).await;
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

    // A run is only ever closed by the task that opened it, so a process killed
    // mid-sweep leaves one behind. Close those before opening any more.
    match job::fail_orphaned(&state.db).await {
        Ok(0) => {}
        Ok(closed) => tracing::warn!(closed, "closed job runs left open by a previous stop"),
        Err(e) => tracing::warn!(error = %e, "could not close orphaned job runs"),
    }

    // Let the server finish starting before the first sweep.
    tokio::time::sleep(Duration::from_secs(30)).await;

    // The tick is the shortest the setting allows, and each wake decides
    // whether enough time has passed — rather than rebuilding the ticker, which
    // would mean tracking when it last fired anyway.
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut swept = tokio::time::Instant::now();

    loop {
        ticker.tick().await;

        if state.flag("refresh.enabled", true) && swept.elapsed() >= interval(&state) {
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

        // One rate-limit bucket is kept per address seen; drop the quiet ones.
        state.limiter.prune();

        prune_audit(&state).await;
        prune_jobs(&state).await;
        prune_callers(&state).await;
    }
}

fn interval(state: &AppState) -> Duration {
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

    let outcome = sweep(state, batch).await;

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

/// Refresh what is due, returning a one-line summary of what happened.
async fn sweep(state: &AppState, batch: i64) -> Result<String> {
    let due = repo::item::due_for_refresh(&state.db, batch).await?;

    if due.is_empty() {
        return Ok("nothing was due".to_string());
    }

    tracing::info!(count = due.len(), "refreshing entries");

    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for (id, _kind) in due {
        // Not `?`. An entry that cannot be read — a bad override, a corrupt row
        // — would otherwise end the sweep before the rest of the batch was
        // touched, and never have its own deadline pushed out: it sorts first
        // by `refresh_after`, so it would be the first entry of every sweep
        // from then on, and nothing in the library would be refreshed again.
        let item = match crate::service::load(state, &id).await {
            Ok(Some(item)) => item,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(%id, error = format_args!("{e:#}"), "could not read an entry due for refresh");
                mark_failure(state, &id, &format!("could not be read: {e}")).await;
                failed += 1;
                continue;
            }
        };

        match refresh_one(state, &item).await {
            Ok(Some(_)) => succeeded += 1,
            Ok(None) => {
                // Nothing to refresh from. Push the deadline out so this entry
                // does not occupy a slot in every future batch.
                mark_failure(state, &id, "no provider could resolve this entry").await;
                failed += 1;
            }
            Err(e) => {
                tracing::warn!(%id, error = %e, "refresh failed");
                mark_failure(state, &id, &e.to_string()).await;
                failed += 1;
            }
        }
    }

    tracing::info!(succeeded, failed, "refresh sweep finished");
    Ok(format!("{succeeded} refreshed, {failed} failed"))
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

async fn mark_failure(state: &AppState, id: &str, error: &str) {
    let next = to_rfc3339(chrono::Utc::now() + chrono::Duration::hours(FAILURE_BACKOFF_HOURS));

    if let Err(e) = repo::item::mark_refreshed(&state.db, id, Some(&next), Some(error)).await {
        tracing::warn!(%id, error = %e, "could not record the refresh failure");
    }

    state.caches.items.invalidate(&format!("item:{id}")).await;
}
