//! The work the server does on its own, as one list an administrator reads:
//! what each task is, when it runs, when it last did and how that went, and
//! when it will next — and the way to run one now.
//!
//! Each task keeps its own lock with its scheduler, so a run started by hand
//! and one the schedule starts can never overlap: the second is told the
//! first is running.
//!
//! What is said here is read from what every instance shares — the history
//! of runs in the database, the holds on the cache server — so that a page
//! following the tasks through one instance sees a run another instance is
//! making, and how far it has got.

use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    db::repo::{
        self,
        job::{Job, Status},
    },
    jobs::schedule::{self, Schedule, Setting},
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
pub const MEDIA_VARIANTS: &str = crate::jobs::media::VARIANTS;
pub const MEDIA_AVIF: &str = crate::jobs::media::AVIF;
pub const TLS_RENEW: &str = repo::job::kinds::TLS_RENEW;
pub const UPDATE_CHECK: &str = repo::job::kinds::UPDATE_CHECK;
pub const ARR_SEED: &str = crate::arr::seed::KIND;
pub const BACKUP_RUN: &str = crate::jobs::backup::KIND;
pub const SEARCH_REBUILD: &str = crate::jobs::search::KIND;
pub const IMPORT_BATCH: &str = crate::jobs::import::KIND;
pub const IMPORT_RULES: &str = crate::jobs::rules::KIND;
pub const LISTING: &str = crate::jobs::listing::KIND;

/// A task, sent under the name the history files its runs by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub enum TaskId {
    #[serde(rename = "refresh.sweep")]
    RefreshSweep,
    #[serde(rename = "refresh.all")]
    RefreshAll,
    #[serde(rename = "import.anime")]
    ImportAnime,
    #[serde(rename = "import.imdb")]
    ImportImdb,
    #[serde(rename = "import.batch")]
    ImportBatch,
    #[serde(rename = "import.rules")]
    ImportRules,
    #[serde(rename = "export.nfo")]
    ExportNfo,
    #[serde(rename = "media.store")]
    MediaStore,
    #[serde(rename = "media.sweep")]
    MediaSweep,
    #[serde(rename = "media.variants")]
    MediaVariants,
    #[serde(rename = "media.avif")]
    MediaAvif,
    #[serde(rename = "tls.renew")]
    TlsRenew,
    #[serde(rename = "update.check")]
    UpdateCheck,
    #[serde(rename = "arr.seed")]
    ArrSeed,
    #[serde(rename = "backup.run")]
    BackupRun,
    #[serde(rename = "search.rebuild")]
    SearchRebuild,
    #[serde(rename = "listing.relist")]
    Listing,
}

impl TaskId {
    /// Every task, in the order the page shows them.
    pub const ALL: [TaskId; 17] = [
        Self::RefreshSweep,
        Self::RefreshAll,
        Self::ImportAnime,
        Self::ImportImdb,
        Self::ImportBatch,
        Self::ImportRules,
        Self::ExportNfo,
        Self::MediaStore,
        Self::MediaSweep,
        Self::MediaVariants,
        Self::MediaAvif,
        Self::TlsRenew,
        Self::UpdateCheck,
        Self::ArrSeed,
        Self::BackupRun,
        Self::SearchRebuild,
        Self::Listing,
    ];

    /// Its name, as the history files its runs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RefreshSweep => REFRESH_SWEEP,
            Self::RefreshAll => REFRESH_ALL,
            Self::ImportAnime => IMPORT_ANIME,
            Self::ImportImdb => IMPORT_IMDB,
            Self::ImportBatch => IMPORT_BATCH,
            Self::ImportRules => IMPORT_RULES,
            Self::ExportNfo => EXPORT_NFO,
            Self::MediaStore => MEDIA_STORE,
            Self::MediaSweep => MEDIA_SWEEP,
            Self::MediaVariants => MEDIA_VARIANTS,
            Self::MediaAvif => MEDIA_AVIF,
            Self::TlsRenew => TLS_RENEW,
            Self::UpdateCheck => UPDATE_CHECK,
            Self::ArrSeed => ARR_SEED,
            Self::BackupRun => BACKUP_RUN,
            Self::SearchRebuild => SEARCH_REBUILD,
            Self::Listing => LISTING,
        }
    }

    /// The task of that name, if there is one.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|task| task.as_str() == name)
    }
}

/// Why a task cannot be run now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
// Named explicitly: utoipa keys schemas on the leaf name alone, and a
// `Blocked` could be anything's.
#[schema(as = TaskBlocked)]
pub enum Blocked {
    /// The source it reads is switched off.
    SourceOff,
    /// What it needs is not configured: the export's path.
    NotConfigured,
    /// It is running.
    Running,
    /// A task under the same lock is.
    Busy,
    /// No media store is configured.
    MediaOff,
    /// The server issues no certificate of its own.
    NoAuthority,
    /// The check for a newer version is off.
    UpdatesOff,
    /// Neither Sonarr nor Radarr is connected.
    ArrOff,
    /// The AVIF copies are off: `media.avif`.
    AvifOff,
    /// No import rule is switched on.
    NoRules,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: how many of this server there are is a `Mode` too, and
// utoipa keys schemas on the leaf name alone.
#[schema(as = TaskMode)]
pub enum Mode {
    /// Runs on a schedule, and may be run now too.
    Scheduled,
    /// Runs only when somebody asks.
    Manual,
    /// Its source or its switch is off: it does not run.
    Off,
}

/// How far a run has got, as its own count says: what a progress bar is
/// drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TaskProgress)]
pub struct Progress {
    /// How many it has done.
    pub done: i64,
    /// Of how many, when it knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

/// How far a run has got, read from what it says of itself while it runs:
/// `120 of 3400: …` — every task that counts its works says it so — or a
/// count alone, `1200 looked at: …`, `2000 listed again`, for one that does
/// not know how many there are.
pub fn progress_of(detail: Option<&str>) -> Option<Progress> {
    let detail = detail?;
    let number = |text: &str| -> Option<(i64, usize)> {
        let end = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        Some((text.get(..end)?.parse().ok()?, end))
    };
    let (done, at) = number(detail)?;
    let rest = &detail[at..];
    if let Some(after) = rest.strip_prefix(" of ")
        && let Some((total, end)) = number(after)
        && after[end..].starts_with(':')
    {
        return Some(Progress {
            done,
            total: Some(total),
        });
    }
    rest.starts_with(' ')
        .then_some(Progress { done, total: None })
}

#[derive(Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskState {
    pub id: TaskId,
    pub mode: Mode,
    /// Seconds between runs, when it is scheduled on a cadence of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every_seconds: Option<i64>,
    /// Whether a window of the day or a cron expression can be set for it.
    pub schedulable: bool,
    /// The window or the expression somebody set, when one is: its own
    /// cadence otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Setting>,
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
    /// How far the run in progress has got, when it counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// Whether a run in progress can be stopped.
    pub cancelable: bool,
    /// Why it cannot be run now, when it cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<Blocked>,
}

/// Every task as it stands.
pub async fn states(state: &AppState) -> anyhow::Result<Vec<TaskState>> {
    let schedules = schedule::all(state).await?;
    let mut out = Vec::with_capacity(TaskId::ALL.len());
    for task in TaskId::ALL {
        out.push(state_of(state, task, schedules.get(task.as_str())).await?);
    }
    Ok(out)
}

/// What a page following the tasks is sent: every task as it stands, and
/// the newest run of all, so that a page can tell its history moved without
/// asking for it.
#[derive(Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TaskBoard)]
pub struct Board {
    pub tasks: Vec<TaskState>,
    /// The newest run of any task, a refresh of one work's included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_run: Option<String>,
}

pub async fn board(state: &AppState) -> anyhow::Result<Board> {
    Ok(Board {
        tasks: states(state).await?,
        newest_run: repo::job::newest_id(&state.db).await?,
    })
}

/// Whether the lock a task works under is held right now, on this
/// instance. A run the history still calls running, with its lock free, is
/// one a crash left behind.
fn lock_busy(id: &str) -> bool {
    match id {
        REFRESH_SWEEP | REFRESH_ALL => crate::jobs::refresh::is_busy(),
        IMPORT_ANIME | IMPORT_IMDB => crate::jobs::datasets::is_busy(),
        IMPORT_BATCH => crate::jobs::import::is_busy(),
        IMPORT_RULES => crate::jobs::rules::is_busy(),
        EXPORT_NFO => crate::api::native::export::is_exporting(),
        MEDIA_STORE => crate::media::worker::is_storing(),
        MEDIA_SWEEP => crate::media::worker::is_sweeping(),
        MEDIA_VARIANTS => crate::jobs::media::is_preparing(),
        MEDIA_AVIF => crate::jobs::media::is_making_avif(),
        TLS_RENEW => crate::tls::is_renewing(),
        UPDATE_CHECK => crate::jobs::update_check::is_checking(),
        ARR_SEED => crate::arr::seed::is_busy(),
        BACKUP_RUN => crate::jobs::backup::is_busy(),
        SEARCH_REBUILD => crate::jobs::search::is_busy(),
        LISTING => crate::jobs::listing::is_busy(),
        _ => false,
    }
}

/// What a task is held under among several instances; nothing for a task
/// that is one instance's own.
pub fn hold_of(id: &str) -> Option<&'static str> {
    match id {
        REFRESH_SWEEP | REFRESH_ALL => Some(crate::jobs::refresh::HOLD),
        IMPORT_ANIME | IMPORT_IMDB => Some(crate::jobs::datasets::HOLD),
        IMPORT_BATCH => Some(crate::jobs::import::HOLD),
        IMPORT_RULES => Some(crate::jobs::rules::HOLD),
        MEDIA_STORE => Some(repo::job::kinds::MEDIA_STORE),
        MEDIA_SWEEP => Some(repo::job::kinds::MEDIA_SWEEP),
        MEDIA_VARIANTS => Some(crate::jobs::media::VARIANTS),
        MEDIA_AVIF => Some(crate::jobs::media::AVIF),
        TLS_RENEW => Some(repo::job::kinds::TLS_RENEW),
        ARR_SEED => Some(crate::arr::seed::HOLD),
        BACKUP_RUN => Some(crate::jobs::backup::HOLD),
        SEARCH_REBUILD => Some(crate::jobs::search::HOLD),
        LISTING => Some(crate::jobs::listing::HOLD),
        _ => None,
    }
}

/// Whether a run of this kind checks for a stop between two works.
pub fn stops_partway(kind: &str) -> bool {
    matches!(
        kind,
        REFRESH_ALL
            | MEDIA_STORE
            | MEDIA_VARIANTS
            | MEDIA_AVIF
            | ARR_SEED
            | IMPORT_BATCH
            | IMPORT_RULES
    )
}

async fn state_of(
    state: &AppState,
    task: TaskId,
    schedule: Option<&(Setting, Schedule)>,
) -> anyhow::Result<TaskState> {
    let id = task.as_str();
    let (last, last_success_at) = repo::job::latest(&state.db, id).await?;
    // Held here, or — among several instances — held elsewhere.
    let busy = lock_busy(id)
        || match hold_of(id) {
            Some(name) => state.coord.is_held(name).await,
            None => false,
        };
    // A run is in progress when the history says so and the instance it
    // names is at it: this one with its lock held, or another still heard of.
    let mine = |job: &Job| {
        job.instance
            .as_deref()
            .is_none_or(|name| name == state.coord.instance.name)
    };
    let running = last
        .as_ref()
        .filter(|job| {
            job.status == Status::Running
                && if mine(job) {
                    lock_busy(id)
                } else {
                    job.instance
                        .as_deref()
                        .is_some_and(|name| state.coord.is_alive(name))
                }
        })
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
                (!on).then_some(Blocked::SourceOff),
            )
        }
        IMPORT_IMDB => {
            let on = state.flag("imdb.enabled", false);
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::jobs::datasets::IMDB_EVERY.num_seconds()),
                (!on).then_some(Blocked::SourceOff),
            )
        }
        // From the import page, with its lines: never on a schedule.
        IMPORT_BATCH => (Mode::Manual, None, None),
        // Each rule on its own schedule: the task is on while one is.
        IMPORT_RULES => {
            let (any, _) = crate::jobs::rules::standing(state).await?;
            (
                if any { Mode::Scheduled } else { Mode::Off },
                None,
                (!any).then_some(Blocked::NoRules),
            )
        }
        EXPORT_NFO => {
            let set = state.config.export.nfo_path.is_some();
            (
                if set { Mode::Manual } else { Mode::Off },
                None,
                (!set).then_some(Blocked::NotConfigured),
            )
        }
        MEDIA_STORE => {
            let on = state.media.is_on();
            (
                if on { Mode::Manual } else { Mode::Off },
                None,
                (!on).then_some(Blocked::MediaOff),
            )
        }
        MEDIA_SWEEP => {
            let on = state.media.is_on();
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::media::worker::SWEEP_EVERY.as_secs() as i64),
                (!on).then_some(Blocked::MediaOff),
            )
        }
        // By hand only, never at start: preparing a catalogue's pictures is
        // hours of decoding on a small machine, and when to spend them is
        // the administrator's to say.
        MEDIA_VARIANTS => {
            let on = state.media.is_on();
            (
                if on { Mode::Manual } else { Mode::Off },
                None,
                (!on).then_some(Blocked::MediaOff),
            )
        }
        // At night, once switched on.
        MEDIA_AVIF => {
            let store = state.media.is_on();
            let on = store && state.flag("media.avif", false);
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::jobs::media::NIGHT_EVERY.as_secs() as i64),
                if !store {
                    Some(Blocked::MediaOff)
                } else {
                    (!on).then_some(Blocked::AvifOff)
                },
            )
        }
        TLS_RENEW => {
            let on = crate::tls::renews(state);
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::tls::RENEW_EVERY.as_secs() as i64),
                (!on).then_some(Blocked::NoAuthority),
            )
        }
        UPDATE_CHECK => {
            let on = crate::jobs::update_check::on(state);
            (
                if on { Mode::Scheduled } else { Mode::Off },
                on.then(|| crate::jobs::update_check::EVERY.as_secs() as i64),
                (!on).then_some(Blocked::UpdatesOff),
            )
        }
        // By hand only: the library is read when somebody is about to point
        // Sonarr or Radarr here, and when they want what came since.
        ARR_SEED => {
            let on = crate::arr::seed::connected(state);
            (
                if on { Mode::Manual } else { Mode::Off },
                None,
                (!on).then_some(Blocked::ArrOff),
            )
        }
        // Off, it can still be run by hand: an archive before a change is
        // what somebody who backs up some other way still wants.
        BACKUP_RUN => match crate::jobs::backup::interval(state) {
            Some(every) => (Mode::Scheduled, Some(every.as_secs() as i64), None),
            None => (Mode::Manual, None, None),
        },
        // By hand: the index keeps itself current, and is written again
        // whole only for somebody who doubts it.
        SEARCH_REBUILD => (Mode::Manual, None, None),
        // Every half minute, whatever the rest is doing.
        LISTING => (
            Mode::Scheduled,
            Some(crate::jobs::listing::EVERY.as_secs() as i64),
            None,
        ),
        _ => (Mode::Off, None, None),
    };

    // When the schedule runs it next, from what its own cadence says and the
    // window or the expression set for it, as its loop reckons it.
    let now = Utc::now();
    let next_at = if mode == Mode::Scheduled {
        next_run(state, id, every, last.as_ref(), schedule, now)
            .await?
            .map(|at| crate::db::to_rfc3339(at.max(now)))
    } else {
        None
    };

    // Stoppable here by its flag; on another instance, by word to it.
    let cancelable = running.as_ref().is_some_and(|job| {
        crate::jobs::cancel::is_cancelable(&job.id) || (!mine(job) && stops_partway(&job.kind))
    });

    // Its own run, or its sibling's under the same lock: either way not now.
    let blocked = off.or(if running.is_some() {
        Some(Blocked::Running)
    } else if busy {
        Some(Blocked::Busy)
    } else {
        None
    });

    let progress = running
        .as_ref()
        .and_then(|job| progress_of(job.detail.as_deref()));

    Ok(TaskState {
        id: task,
        mode,
        every_seconds: every,
        schedulable: schedule::is_schedulable(id),
        schedule: schedule.map(|(setting, _)| setting.clone()),
        blocked,
        last,
        last_success_at,
        next_at,
        running,
        progress,
        cancelable,
    })
}

/// When a scheduled task runs next, as its own loop reckons it: the sweep an
/// interval after the last one began; an import an interval after the list
/// was last imported — or an hour after a failure, which is when it is
/// tried again — and either kept to its window or its expression when one is
/// set; the archive as its own schedule says; the rest by their own clocks.
async fn next_run(
    state: &AppState,
    id: &str,
    every: Option<i64>,
    last: Option<&Job>,
    read: Option<&(Setting, Schedule)>,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<DateTime<Utc>>> {
    let began = || {
        last.and_then(|job| job.started_at.as_deref())
            .and_then(crate::db::parse_rfc3339)
    };
    Ok(match id {
        REFRESH_SWEEP => {
            let clock = crate::jobs::refresh::sweep_clock(state).await?;
            schedule::next_of(read, clock, now)
        }
        IMPORT_ANIME | IMPORT_IMDB => {
            let list = if id == IMPORT_ANIME {
                crate::jobs::datasets::ANIME
            } else {
                crate::jobs::datasets::IMDB
            };
            match crate::jobs::datasets::clock(state, list).await {
                Some(clock) => schedule::next_of(read, clock, now),
                None => None,
            }
        }
        // Its own schedule says: a minute after the start, then a day after
        // the last check, whoever asked for it.
        UPDATE_CHECK => crate::jobs::update_check::next_at(),
        // An interval after the newest archive, whoever made it.
        BACKUP_RUN => crate::jobs::backup::next_at(state).await?,
        // The next night.
        MEDIA_AVIF => Some(crate::jobs::media::next_night(now)),
        // The rule due soonest.
        IMPORT_RULES => crate::jobs::rules::standing(state).await?.1,
        // Every half minute, always: a next moment would only ever say now.
        LISTING => None,
        _ => every.map(|seconds| began().map_or(now, |at| at + chrono::Duration::seconds(seconds))),
    })
}

/// One task as it stands: what a change to its schedule answers with.
pub async fn state_named(state: &AppState, task: TaskId) -> anyhow::Result<TaskState> {
    let read = schedule::of(state, task.as_str()).await?;
    state_of(state, task, read.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_is_sent_under_the_name_its_runs_are_filed_by() {
        for task in TaskId::ALL {
            assert_eq!(serde_json::to_value(task).unwrap(), task.as_str());
            assert_eq!(TaskId::named(task.as_str()), Some(task));
        }
        let names: Vec<&str> = TaskId::ALL.iter().map(|task| task.as_str()).collect();
        let mut distinct = names.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), names.len(), "{names:?}");
        assert_eq!(
            serde_json::to_value(Blocked::NotConfigured).unwrap(),
            "not_configured"
        );
        assert_eq!(TaskId::named("refresh.item"), None);
    }

    #[test]
    fn a_runs_own_words_say_how_far_it_has_got() {
        let read = |text: &str| progress_of(Some(text));
        // Every task that counts its works.
        for (text, done, total) in [
            ("12 of 3400: 10 refreshed, 2 failed", 12, 3400),
            (
                "5 of 44: 1 already here, 3 imported, 1 not found, 0 failed",
                5,
                44,
            ),
            ("40 of 120: 38 stored, 2 failed", 40, 120),
            ("16 of 900: 16 prepared, 0 failed", 16, 900),
        ] {
            assert_eq!(
                read(text),
                Some(Progress {
                    done,
                    total: Some(total)
                }),
                "{text}"
            );
        }
        // A count alone, for a run that does not know how many there are.
        assert_eq!(
            read("1200 looked at: 300 copies made, 0 failed"),
            Some(Progress {
                done: 1200,
                total: None
            })
        );
        assert_eq!(
            read("2000 listed again"),
            Some(Progress {
                done: 2000,
                total: None
            })
        );
        // Nothing countable.
        assert_eq!(read("synced from tmdb"), None);
        assert_eq!(progress_of(None), None);
        assert_eq!(read("3.1.0 is out"), None);
    }
}
