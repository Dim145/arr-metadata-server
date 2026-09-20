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
    service::{movie, series},
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
    let language = state.tmdb.language().to_string();
    let ids = &item.external_ids;

    // Prefer the provider that holds the richest document for this work.
    if let Some(tmdb_id) = ids.tmdb
        && state.tmdb.is_configured()
        && let Some(refreshed) = force_series_from_tmdb(state, tmdb_id).await?
    {
        return Ok(Some(refreshed));
    }

    if let Some(tvdb_id) = ids.tvdb {
        return series::by_tvdb_id(state, tvdb_id, &language).await;
    }

    Ok(None)
}

/// Refetch from TMDB unconditionally, bypassing the local-first ladder.
async fn force_series_from_tmdb(state: &AppState, tmdb_id: i64) -> Result<Option<MediaItem>> {
    let Some((raw, tv)) = state.tmdb.tv(tmdb_id).await? else {
        return Ok(None);
    };

    let numbers: Vec<i32> = tv.seasons.iter().map(|s| s.season_number).collect();
    let seasons = state.tmdb.tv_seasons(tmdb_id, &numbers).await;

    let mapped = crate::providers::tmdb::map::tv_to_item(&tv, &seasons);

    Ok(Some(
        crate::service::persist(state, mapped, crate::providers::names::TMDB, Some(&raw)).await?,
    ))
}

async fn refresh_movie(state: &AppState, item: &MediaItem) -> Result<Option<MediaItem>> {
    if !state.tmdb.is_configured() {
        return Ok(None);
    }

    let Some(tmdb_id) = item.external_ids.tmdb else {
        // Resolve through IMDb, which also links the TMDB id for next time.
        return match &item.external_ids.imdb {
            Some(imdb) => movie::by_imdb_id(state, imdb).await,
            None => Ok(None),
        };
    };

    let Some((raw, movie)) = state.tmdb.movie(tmdb_id).await? else {
        return Ok(None);
    };

    let mapped = crate::providers::tmdb::map::movie_to_item(&movie);

    Ok(Some(
        crate::service::persist(state, mapped, crate::providers::names::TMDB, Some(&raw)).await?,
    ))
}

/// Run the scheduler until the process shuts down.
pub async fn run(state: AppState) {
    let cfg = state.config.refresh.clone();

    if !cfg.enabled {
        tracing::info!("automatic refresh is disabled");
        return;
    }

    tracing::info!(
        interval_secs = cfg.interval.as_secs(),
        batch = cfg.batch_size,
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

    let mut ticker = tokio::time::interval(cfg.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        run_sweep(&state, cfg.batch_size as i64).await;

        // Expired sessions accumulate otherwise; this is as good a moment as any.
        if let Err(e) = repo::user::purge_expired_sessions(&state.db).await {
            tracing::warn!(error = %e, "could not purge expired sessions");
        }

        // One rate-limit bucket is kept per address seen; drop the quiet ones.
        state.limiter.prune();

        prune_audit(&state).await;
        prune_jobs(&state).await;
    }
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
        let Some(item) = crate::service::load(state, &id).await? else {
            continue;
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

    let cutoff = to_rfc3339(chrono::Utc::now() - chrono::Duration::days(i64::from(days)));

    match job::prune(&state.db, &cutoff).await {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, "pruned job runs"),
        Err(e) => tracing::warn!(error = %e, "could not prune job runs"),
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

    let cutoff = to_rfc3339(chrono::Utc::now() - chrono::Duration::days(i64::from(days)));

    match repo::audit::prune(&state.db, &cutoff).await {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, retention_days = days, "pruned audit entries"),
        Err(e) => tracing::warn!(error = %e, "could not prune the audit log"),
    }
}

async fn mark_failure(state: &AppState, id: &str, error: &str) {
    let next = to_rfc3339(chrono::Utc::now() + chrono::Duration::hours(FAILURE_BACKOFF_HOURS));

    if let Err(e) = repo::item::mark_refreshed(&state.db, id, Some(&next), Some(error)).await {
        tracing::warn!(%id, error = %e, "could not record the refresh failure");
    }

    state.caches.items.invalidate(&format!("item:{id}")).await;
}
