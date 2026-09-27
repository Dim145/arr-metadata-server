//! The work the server does on its own, as one list an administrator reads:
//! what each task is, when it runs, when it last did and how that went, and
//! when it will next — and the way to run one now.
//!
//! Each task keeps its own lock with its scheduler, so a run started by hand
//! and one the schedule starts can never overlap: the second is told the
//! first is running.

use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    db::repo::{self, job::Job},
    state::AppState,
};

/// A task's name, as the history files its runs.
pub const REFRESH_SWEEP: &str = repo::job::kinds::REFRESH_SWEEP;
pub const REFRESH_ALL: &str = repo::job::kinds::REFRESH_ALL;
pub const IMPORT_ANIME: &str = repo::job::kinds::IMPORT_ANIME;
pub const IMPORT_IMDB: &str = repo::job::kinds::IMPORT_IMDB;
pub const EXPORT_NFO: &str = repo::job::kinds::EXPORT_NFO;
pub const MEDIA_STORE: &str = repo::job::kinds::MEDIA_STORE;
pub const MEDIA_SWEEP: &str = repo::job::kinds::MEDIA_SWEEP;

/// Every task, in the order the page shows them.
pub const ALL: [&str; 7] = [
    REFRESH_SWEEP,
    REFRESH_ALL,
    IMPORT_ANIME,
    IMPORT_IMDB,
    EXPORT_NFO,
    MEDIA_STORE,
    MEDIA_SWEEP,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Runs on a schedule, and may be run now too.
    Scheduled,
    /// Runs only when somebody asks.
    Manual,
    /// Its source or its switch is off: it does not run.
    Off,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskState {
    pub id: &'static str,
    pub mode: Mode,
    /// Seconds between runs, when it is scheduled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every_seconds: Option<i64>,
    /// Its latest run, whatever its outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<Job>,
    /// When it last succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<String>,
    /// When the schedule will run it next, as near as can be said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_at: Option<String>,
    /// The run in progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<Job>,
    /// Whether a run in progress can be stopped.
    pub cancelable: bool,
    /// Why it cannot be run now, when it cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<&'static str>,
}

/// Every task as it stands.
pub async fn states(state: &AppState) -> anyhow::Result<Vec<TaskState>> {
    let mut out = Vec::with_capacity(ALL.len());
    for id in ALL {
        out.push(state_of(state, id).await?);
    }
    Ok(out)
}

/// Whether the lock a task works under is held right now. A run the history
/// still calls running, with its lock free, is one a crash left behind.
fn lock_busy(id: &str) -> bool {
    match id {
        REFRESH_SWEEP | REFRESH_ALL => crate::jobs::refresh::is_busy(),
        IMPORT_ANIME | IMPORT_IMDB => crate::jobs::datasets::is_busy(),
        EXPORT_NFO => crate::api::native::export::is_exporting(),
        MEDIA_STORE => crate::media::worker::is_storing(),
        MEDIA_SWEEP => crate::media::worker::is_sweeping(),
        _ => false,
    }
}

async fn state_of(state: &AppState, id: &'static str) -> anyhow::Result<TaskState> {
    let (last, last_success_at) = repo::job::latest(&state.db, id).await?;
    let busy = lock_busy(id);
    let running = last
        .as_ref()
        .filter(|job| job.status == "running" && busy)
        .cloned();

    let (mode, every, off) = match id {
        REFRESH_SWEEP => {
            let on = state.flag("refresh.enabled", true);
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::jobs::refresh::interval(state).as_secs() as i64),
                None,
            )
        }
        REFRESH_ALL => (Mode::Manual, None, None),
        IMPORT_ANIME => {
            let on = crate::jobs::datasets::anime_wanted(state);
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::jobs::datasets::ANIME_EVERY.num_seconds()),
                (!on).then_some("source_off"),
            )
        }
        IMPORT_IMDB => {
            let on = state.flag("imdb.enabled", false);
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::jobs::datasets::IMDB_EVERY.num_seconds()),
                (!on).then_some("source_off"),
            )
        }
        EXPORT_NFO => {
            let set = state.config.export.nfo_path.is_some();
            (
                if set { Mode::Manual } else { Mode::Off },
                None,
                (!set).then_some("not_configured"),
            )
        }
        MEDIA_STORE => {
            let on = state.media.is_on();
            (
                if on { Mode::Manual } else { Mode::Off },
                None,
                (!on).then_some("media_off"),
            )
        }
        MEDIA_SWEEP => {
            let on = state.media.is_on();
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::media::worker::SWEEP_EVERY.as_secs() as i64),
                (!on).then_some("media_off"),
            )
        }
        _ => (Mode::Off, None, None),
    };

    // When the schedule runs it next. The sweep: an interval after the last
    // one began. An import: an interval after the list was last imported — or
    // an hour after a failure, which is when it is tried again.
    let now = chrono::Utc::now();
    let next_at = match (mode, every) {
        (Mode::Scheduled, Some(seconds)) => {
            let next = match id {
                IMPORT_ANIME | IMPORT_IMDB => {
                    let list = if id == IMPORT_ANIME {
                        crate::jobs::datasets::ANIME
                    } else {
                        crate::jobs::datasets::IMDB
                    };
                    let imported = repo::import::get(&state.db, list)
                        .await?
                        .and_then(|i| crate::db::parse_rfc3339(&i.imported_at))
                        .map(|at| at + chrono::Duration::seconds(seconds));
                    let retry = last
                        .as_ref()
                        .filter(|job| job.status == "failed")
                        .and_then(|job| job.finished_at.as_deref())
                        .and_then(crate::db::parse_rfc3339)
                        .and_then(|at| {
                            chrono::Duration::from_std(crate::jobs::datasets::RETRY_AFTER)
                                .ok()
                                .map(|wait| at + wait)
                        });
                    match (imported, retry) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    }
                }
                _ => last
                    .as_ref()
                    .and_then(|job| job.started_at.as_deref())
                    .and_then(crate::db::parse_rfc3339)
                    .map(|at| at + chrono::Duration::seconds(seconds)),
            };
            Some(crate::db::to_rfc3339(next.map_or(now, |at| at.max(now))))
        }
        _ => None,
    };

    let cancelable = running
        .as_ref()
        .is_some_and(|job| crate::jobs::cancel::is_cancelable(&job.id));

    // Its own run, or its sibling's under the same lock: either way not now.
    let blocked = off.or(if running.is_some() {
        Some("running")
    } else if busy {
        Some("busy")
    } else {
        None
    });

    Ok(TaskState {
        id,
        mode,
        every_seconds: every,
        blocked,
        last,
        last_success_at,
        next_at,
        running,
        cancelable,
    })
}
