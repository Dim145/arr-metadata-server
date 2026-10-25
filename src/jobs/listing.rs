//! Keeping every work's listed values current; see `service::listing`.
//!
//! There is seldom anything to do: a write lists its own work as it happens.
//! What is left comes here — every work after an upgrade that changed the way
//! works are listed, the works IMDb's daily list rescored, all of them when
//! IMDb is switched on or off, and any work a write could not list itself.
//!
//! It is the task `listing.relist` on the tasks page. A look that finds
//! nothing behind is no run; one that does opens one, which stays open while
//! the backlog lasts — a catalogue listed again after an upgrade is one run,
//! not fifty — and says how many works it listed as it goes.
//!
//! The import rules' schedule runs beside it, in the same loop's keeping: see
//! [`crate::jobs::rules`].

use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::{
    db::repo::job,
    error::{AppError, AppResult},
    service::listing,
    state::AppState,
};

/// The task's name, as the history files its runs, and what a pass is held
/// under among several instances.
pub const KIND: &str = "listing.relist";
pub const HOLD: &str = KIND;

/// How often to look for works whose listed values are behind.
pub const EVERY: Duration = Duration::from_secs(30);

/// The most works listed in one pass. A backlog — a catalogue of a hundred
/// thousand listed again after an upgrade — is worked through in passes a
/// second apart, so the database is shared with everything else meanwhile
/// rather than held for a minute.
const MOST: usize = 2_000;

/// Held by a pass, the schedule's or one asked for by hand.
static PASSING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether works are being listed again on this instance.
pub fn is_busy() -> bool {
    PASSING.try_lock().is_err()
}

/// Run until the process shuts down: the works listed again, and the
/// import rules' schedule, side by side.
pub async fn run(state: AppState) {
    tokio::join!(relist(state.clone()), crate::jobs::rules::run(state));
}

/// A run while it is open: its id, and how many it listed so far.
struct Open {
    record: String,
    listed: u64,
}

async fn relist(state: AppState) {
    // Let the server finish starting first.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let mut open: Option<Open> = None;
    loop {
        // The leader's work: alone, this instance; among several, the one
        // holding the lease. A run left open by one that no longer leads is
        // closed with what it did.
        if !state.coord.leads() {
            if let Some(run) = open.take() {
                close(&state, run, None).await;
            }
            tokio::time::sleep(EVERY).await;
            continue;
        }

        let pass = match (
            PASSING.clone().try_lock_owned(),
            state.coord.hold(HOLD).await,
        ) {
            (Ok(_held), Some(_shared)) => {
                Some(listing::relist_stale(&state.db, listing::imdb_on(&state), MOST).await)
            }
            // A pass somebody asked for is under way.
            _ => None,
        };

        let more = match pass {
            None => false,
            Some(Ok((listed, more))) => {
                if listed > 0 && open.is_none() {
                    match job::start(&state.db, KIND, None).await {
                        Ok(record) => open = Some(Open { record, listed: 0 }),
                        Err(e) => tracing::debug!(error = %e, "could not open the listing's run"),
                    }
                }
                if let Some(run) = open.as_mut() {
                    run.listed += listed;
                    if let Err(e) = job::progress(&state.db, &run.record, &said(run.listed)).await {
                        tracing::debug!(error = %e, "could not say how far the listing got");
                    }
                }
                if listed > 0 {
                    tracing::info!(works = listed, "listed works again");
                }
                if !more && let Some(run) = open.take() {
                    close(&state, run, None).await;
                }
                more
            }
            Some(Err(e)) => {
                tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not list works again; trying again shortly"
                );
                if let Some(run) = open.take() {
                    close(&state, run, Some(format!("{e:#}"))).await;
                }
                false
            }
        };

        // A pass that stopped at its limit has more behind it.
        let pause = if more { Duration::from_secs(1) } else { EVERY };
        tokio::time::sleep(pause).await;
    }
}

/// How a run says what it did, as the interface reads it.
fn said(listed: u64) -> String {
    format!("{listed} listed again")
}

async fn close(state: &AppState, run: Open, error: Option<String>) {
    if let Err(e) = job::finish(
        &state.db,
        &run.record,
        Some(&said(run.listed)),
        error.as_deref(),
    )
    .await
    {
        tracing::warn!(error = %e, "could not close the listing's run");
    }
}

/// List what is behind now, as `by` asked, on this instance, in the
/// background: the run's id at once. It goes on until nothing is behind.
pub async fn now(state: &AppState, by: &str) -> AppResult<Option<String>> {
    let held = PASSING.clone().try_lock_owned().map_err(|_| {
        AppError::Conflict("works are being listed again already; its run is in the history".into())
    })?;
    let shared = state
        .coord
        .hold_for(HOLD, "works are being listed again")
        .await?;
    let record = job::start_by(&state.db, KIND, None, Some(by)).await?;

    let state = state.clone();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let mut run = Open { record, listed: 0 };
        loop {
            match listing::relist_stale(&state.db, listing::imdb_on(&state), MOST).await {
                Ok((listed, more)) => {
                    run.listed += listed;
                    if let Err(e) = job::progress(&state.db, &run.record, &said(run.listed)).await {
                        tracing::debug!(error = %e, "could not say how far the listing got");
                    }
                    if !more {
                        close(&state, run, None).await;
                        return;
                    }
                }
                Err(e) => {
                    close(&state, run, Some(format!("{e:#}"))).await;
                    return;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    Ok(Some(id))
}
