//! Keeping every work's listed values current; see `service::listing`.
//!
//! There is seldom anything to do: a write lists its own work as it happens.
//! What is left comes here — every work after an upgrade that changed the way
//! works are listed, the works IMDb's daily list rescored, all of them when
//! IMDb is switched on or off, and any work a write could not list itself.

use std::time::Duration;

use crate::{service::listing, state::AppState};

/// How often to look for works whose listed values are behind.
const TICK: Duration = Duration::from_secs(30);

/// The most works listed in one pass. A backlog — a catalogue of a hundred
/// thousand listed again after an upgrade — is worked through in passes a
/// second apart, so the database is shared with everything else meanwhile
/// rather than held for a minute.
const MOST: usize = 2_000;

/// Run until the process shuts down.
pub async fn run(state: AppState) {
    // Let the server finish starting first.
    tokio::time::sleep(Duration::from_secs(5)).await;

    loop {
        // The leader's work: alone, this instance; among several, the one
        // holding the lease.
        if !state.coord.leads() {
            tokio::time::sleep(TICK).await;
            continue;
        }

        let (listed, more) =
            match listing::relist_stale(&state.db, listing::imdb_on(&state), MOST).await {
                Ok(pass) => pass,
                Err(e) => {
                    tracing::warn!(
                        error = format_args!("{e:#}"),
                        "could not list works again; trying again shortly"
                    );
                    (0, false)
                }
            };

        if listed > 0 {
            tracing::info!(works = listed, "listed works again");
        }

        // A pass that stopped at its limit has more behind it.
        let pause = if more { Duration::from_secs(1) } else { TICK };
        tokio::time::sleep(pause).await;
    }
}
