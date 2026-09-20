//! The refresh scheduler.
//!
//! Any entry carrying at least one external id is refetched on a schedule that
//! follows its status: a running series changes weekly, an ended one never. The
//! refresh replaces provider snapshots and provider-sourced children. It does
//! not touch `media_override`, which is what makes a manual edit permanent.

use std::time::Duration;

use anyhow::Result;

use crate::{
    db::{repo, to_rfc3339},
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

    // Let the server finish starting before the first sweep.
    tokio::time::sleep(Duration::from_secs(30)).await;

    let mut ticker = tokio::time::interval(cfg.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        if let Err(e) = sweep(&state, cfg.batch_size as i64).await {
            tracing::error!(error = ?e, "refresh sweep failed");
        }

        // Expired sessions accumulate otherwise; this is as good a moment as any.
        if let Err(e) = repo::user::purge_expired_sessions(&state.db).await {
            tracing::warn!(error = %e, "could not purge expired sessions");
        }

        // One rate-limit bucket is kept per address seen; drop the quiet ones.
        state.limiter.prune();
    }
}

async fn sweep(state: &AppState, batch: i64) -> Result<()> {
    let due = repo::item::due_for_refresh(&state.db, batch).await?;

    if due.is_empty() {
        return Ok(());
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
    Ok(())
}

async fn mark_failure(state: &AppState, id: &str, error: &str) {
    let next = to_rfc3339(chrono::Utc::now() + chrono::Duration::hours(FAILURE_BACKOFF_HOURS));

    if let Err(e) = repo::item::mark_refreshed(&state.db, id, Some(&next), Some(error)).await {
        tracing::warn!(%id, error = %e, "could not record the refresh failure");
    }

    state.caches.items.invalidate(&format!("item:{id}")).await;
}
