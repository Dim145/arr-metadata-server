//! The archives the server makes of itself, on a schedule and when asked,
//! and the restore the Backups page starts.
//!
//! `backup.schedule` says how often — `off`, `daily`, `weekly` (the
//! default), `monthly` — counted from the newest archive, whoever made it:
//! one made by hand on Tuesday puts the schedule's off until the next
//! Tuesday. `backup.keep` says how many of the schedule's own are kept;
//! those made by hand, by the command line or before a restore stay until
//! somebody deletes them. `backup.includeMedia` puts the media kept on disk
//! in the schedule's archives. Each run writes down the archive it made and
//! those it took away, and tells the webhook `backup.completed` or
//! `backup.failed`.
//!
//! Among several instances the leader keeps the schedule, and one archive
//! or one restore is under way at a time across all of them.

use std::{
    sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    backup::{
        Ask, Made, Reason,
        maintenance::Shut,
        restore::{Request, Restored, Source, Step},
    },
    db::repo::{
        audit::Action,
        job::{self, Outcome, Touched},
    },
    error::{AppError, AppResult},
    state::AppState,
};

/// The task's name, as the history files its runs.
pub const KIND: &str = "backup.run";

/// A restore, as the history files it.
pub const RESTORE_KIND: &str = "backup.restore";

/// What one archive or one restore at a time is held under, among several
/// instances.
pub const HOLD: &str = "backup";

/// How often: `off`, `daily`, `weekly` or `monthly`.
pub const SCHEDULE: &str = "backup.schedule";

/// How many of the schedule's archives are kept.
pub const KEEP: &str = "backup.keep";

/// Whether the schedule's archives carry the media kept on disk.
pub const INCLUDE_MEDIA: &str = "backup.includeMedia";

/// What each schedule comes to.
pub const SCHEDULES: [(&str, Option<Duration>); 4] = [
    ("off", None),
    ("daily", Some(Duration::from_secs(24 * 3600))),
    ("weekly", Some(Duration::from_secs(7 * 24 * 3600))),
    ("monthly", Some(Duration::from_secs(30 * 24 * 3600))),
];

/// The schedule's archives kept when nothing says how many: Sonarr's four
/// weeks, at the weekly default.
pub const DEFAULT_KEEP: i64 = 4;

/// How long after the start the schedule first looks: long enough for a
/// server that keeps restarting not to make an archive each time.
const FIRST_AFTER: Duration = Duration::from_secs(10 * 60);

/// How often it looks again: a schedule changed is followed within this.
const TICK: Duration = Duration::from_secs(10 * 60);

/// How long after the schedule's archive failed it is tried again: soon
/// enough for a disk that was full for a while, not so often that every
/// tick tells the webhook the same failure.
const RETRY_AFTER: Duration = Duration::from_secs(3600);

/// One archive or one restore at a time, on this instance: the schedule's,
/// or one somebody asked for.
static WORKING: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Whether an archive is being made, or restored, on this instance.
pub fn is_busy() -> bool {
    WORKING.try_lock().is_err()
}

/// How long between two archives, when the schedule is on.
pub fn interval(state: &AppState) -> Option<Duration> {
    let chosen = state
        .settings
        .resolve(SCHEDULE, None, None)
        .unwrap_or_else(|| "weekly".to_string());
    SCHEDULES
        .iter()
        .find(|(name, _)| *name == chosen)
        .and_then(|(_, every)| *every)
}

/// How many of the schedule's archives are kept.
pub fn keep(state: &AppState) -> usize {
    state
        .settings
        .int_at(KEEP, None, None)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(DEFAULT_KEEP as usize)
        // The one just made, at the least: never pruned by its own run.
        .max(1)
}

/// When the newest archive this database describes was made.
pub async fn last_at(state: &AppState) -> Result<Option<DateTime<Utc>>> {
    Ok(state
        .backups
        .newest_at()
        .await?
        .as_deref()
        .and_then(crate::db::parse_rfc3339))
}

/// When the schedule makes the next one: an interval after the newest, or
/// now when there is none — an hour after the last run, when that one
/// failed. Nothing while it is off.
pub async fn next_at(state: &AppState) -> Result<Option<DateTime<Utc>>> {
    let Some(every) = interval(state) else {
        return Ok(None);
    };
    let now = Utc::now();
    let due = match last_at(state).await? {
        Some(last) => last + delta(every),
        None => now,
    };
    let (run, _) = job::latest(&state.db, KIND).await?;
    let retry = run
        .filter(|run| run.status == job::Status::Failed)
        .and_then(|run| {
            run.finished_at
                .as_deref()
                .and_then(crate::db::parse_rfc3339)
        })
        .map(|failed| failed + delta(RETRY_AFTER));
    Ok(Some(due.max(retry.unwrap_or(due)).max(now)))
}

/// Whether the newest archive is old enough to be worth saying so: older
/// than twice the schedule's interval, or none at all. Never while the
/// schedule is off: whoever turned it off backs up some other way.
pub async fn overdue(state: &AppState) -> Result<bool> {
    let Some(every) = interval(state) else {
        return Ok(false);
    };
    Ok(match last_at(state).await? {
        Some(last) => Utc::now() - last > delta(every * 2),
        None => true,
    })
}

/// The schedule: ten minutes after the start, then every ten minutes, an
/// archive when the newest is an interval old. The leader's to keep.
pub async fn run(state: AppState) {
    tokio::time::sleep(FIRST_AFTER).await;
    loop {
        if state.coord.leads() && !state.maintenance.is_on() {
            match next_at(&state).await {
                Ok(Some(at)) if at <= Utc::now() => {
                    if let Ok(held) = WORKING.clone().try_lock_owned()
                        && let Some(shared) = state.coord.hold(HOLD).await
                    {
                        let record = job::start(&state.db, KIND, None)
                            .await
                            .inspect_err(
                                |e| tracing::warn!(error = %e, "could not open the backup's run"),
                            )
                            .ok();
                        let include_media = state.flag(INCLUDE_MEDIA, false);
                        let outcome = back_up(
                            &state,
                            Reason::Scheduled,
                            record.as_deref(),
                            None,
                            include_media,
                        )
                        .await;
                        close(&state, record, outcome).await;
                        drop(shared);
                        drop(held);
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not tell whether a backup is due"
                ),
            }
        }
        tokio::time::sleep(TICK).await;
    }
}

/// An archive now, as `by` asked, in the background: the run's id at once.
/// The media kept on disk go in it when `include_media` says so, else when
/// the schedule's would have them.
pub async fn back_up_now(
    state: &AppState,
    by: &str,
    include_media: Option<bool>,
) -> AppResult<Option<String>> {
    if state.maintenance.is_on() {
        return Err(AppError::Conflict("a backup is being restored".into()));
    }
    let held = WORKING
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::Conflict("a backup or a restore is under way".into()))?;
    let shared = state
        .coord
        .hold_for(HOLD, "a backup or a restore is under way")
        .await?;
    let record = job::start_by(&state.db, KIND, None, Some(by)).await?;
    let include_media = include_media.unwrap_or_else(|| state.flag(INCLUDE_MEDIA, false));

    let state = state.clone();
    let by = by.to_string();
    let id = record.clone();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let outcome = back_up(
            &state,
            Reason::Manual,
            Some(&record),
            Some(by),
            include_media,
        )
        .await;
        close(&state, Some(record), outcome).await;
    });
    Ok(Some(id))
}

/// Make an archive, write down what the run did, take away what the
/// schedule no longer keeps, and tell the webhook: the run's summary, as
/// `<name>, <bytes> bytes, <n> removed`.
async fn back_up(
    state: &AppState,
    reason: Reason,
    record: Option<&str>,
    by: Option<String>,
    include_media: bool,
) -> Result<String> {
    let made = state
        .backups
        .make(Ask {
            include_media,
            by,
            job: record.map(str::to_string),
            ..Ask::new(reason)
        })
        .await;
    let made: Made = match made {
        Ok(made) => made,
        Err(e) => {
            crate::service::webhook::backup_failed(state, reason.as_str(), &format!("{e:#}"));
            return Err(e);
        }
    };
    if let Some(record) = record {
        // One of the fixed notes the Jobs page words in each language; what
        // the archive weighs and holds is in the run's summary and the
        // webhook's message.
        note(
            state,
            record,
            0,
            &made.name,
            Outcome::Written,
            "archive written",
        )
        .await;
    }

    let removed = if reason == Reason::Scheduled {
        match state.backups.prune(keep(state)).await {
            Ok(removed) => removed,
            Err(e) => {
                tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not take away the archives past those kept"
                );
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    if let Some(record) = record {
        for (position, name) in removed.iter().enumerate() {
            note(
                state,
                record,
                position as i64 + 1,
                name,
                Outcome::Removed,
                "past those the schedule keeps",
            )
            .await;
        }
    }

    crate::service::webhook::backup_completed(state, &made.name, &describe(&made));
    Ok(format!(
        "{}, {} bytes, {} removed",
        made.name,
        made.size,
        removed.len()
    ))
}

/// What the webhook and the run's entry say of an archive.
fn describe(made: &Made) -> String {
    let mut said = format!(
        "{} bytes, {} database, {}",
        made.size, made.manifest.engine, made.manifest.reason
    );
    if made.manifest.media.included {
        said.push_str(&format!(", {} media", made.manifest.media.files));
    }
    if made.encrypted {
        said.push_str(", encrypted");
    }
    said.push_str(&format!(", kept in {}", made.at));
    said
}

async fn note(
    state: &AppState,
    record: &str,
    position: i64,
    name: &str,
    outcome: Outcome,
    what: &str,
) {
    if let Err(e) = job::add_entry(
        &state.db,
        record,
        position,
        Touched {
            id: "",
            title: Some(name),
            kind: None,
        },
        outcome,
        Some(what),
    )
    .await
    {
        tracing::warn!(error = %e, "could not write down what the backup did");
    }
}

/// Close the run with what came of it.
async fn close(state: &AppState, record: Option<String>, outcome: Result<String>) {
    let Some(record) = record else {
        if let Err(e) = outcome {
            tracing::warn!(error = format_args!("{e:#}"), "the backup failed");
        }
        return;
    };
    let closed = match &outcome {
        Ok(summary) => job::finish(&state.db, &record, Some(summary), None).await,
        Err(e) => {
            tracing::warn!(error = format_args!("{e:#}"), "the backup failed");
            job::finish(&state.db, &record, None, Some(&format!("{e:#}"))).await
        }
    };
    if let Err(e) = closed {
        tracing::warn!(error = %e, "could not close the backup's run");
    }
}

// ─── restore ─────────────────────────────────────────────────────────────────

/// How a restore stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = BackupRestoreStatus)]
pub enum Status {
    /// None was asked for.
    #[default]
    Idle,
    Running,
    Succeeded,
    /// It stopped, and the database is as it was.
    Failed,
}

/// Where the last restore this instance ran stands.
#[derive(Clone, Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupRestore)]
pub struct RestoreState {
    pub state: Status,
    #[schema(required)]
    pub archive: Option<String>,
    #[schema(required)]
    pub started_at: Option<String>,
    #[schema(required)]
    pub finished_at: Option<String>,
    /// What went wrong; the database is as it was before.
    #[schema(required)]
    pub error: Option<String>,
    /// What was restored, once it was.
    #[schema(required)]
    pub restored: Option<Restored>,
}

static LAST_RESTORE: Mutex<Option<RestoreState>> = Mutex::new(None);

fn last_restore() -> MutexGuard<'static, Option<RestoreState>> {
    LAST_RESTORE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the last restore stands: the one this instance ran, else the last
/// the history holds — another instance's, or this one's before a restart.
pub async fn restore_state(state: &AppState) -> AppResult<RestoreState> {
    if let Some(known) = last_restore().clone() {
        return Ok(known);
    }
    let (last, _) = job::latest(&state.db, RESTORE_KIND).await?;
    Ok(match last {
        Some(run) => RestoreState {
            state: match run.status {
                job::Status::Succeeded => Status::Succeeded,
                job::Status::Running => Status::Running,
                job::Status::Failed | job::Status::Stopped => Status::Failed,
            },
            archive: run.target,
            started_at: run.started_at,
            finished_at: run.finished_at,
            error: run.error,
            restored: None,
        },
        None => RestoreState::default(),
    })
}

/// Restore the archive `name` over the database, in the background: a
/// safety copy first, every door shut until it is done. The archive's
/// manifest is read before anything starts, so that an archive this server
/// cannot restore, or a passphrase that does not open it, is said at once.
pub async fn restore_now(
    state: &AppState,
    identity: &Identity,
    ip: ClientIp,
    name: &str,
    passphrase: Option<String>,
) -> AppResult<()> {
    if !crate::backup::is_archive_name(name) || state.backups.place().size(name).await?.is_none() {
        return Err(AppError::NotFound);
    }
    let held = WORKING
        .clone()
        .try_lock_owned()
        .map_err(|_| AppError::Conflict("a backup or a restore is under way".into()))?;
    let shared = state
        .coord
        .hold_for(HOLD, "a backup or a restore is under way")
        .await?;

    let passphrase = passphrase
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(age::secrecy::SecretString::from);
    let source = Source::Kept(name.to_string());
    let manifest = state
        .backups
        .manifest_of(&source, passphrase.clone())
        .await
        .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
    if let Some(refusal) = manifest.refusal(crate::db::latest_migration()) {
        return Err(AppError::Conflict(refusal));
    }

    let started_at = crate::db::now();
    *last_restore() = Some(RestoreState {
        state: Status::Running,
        archive: Some(name.to_string()),
        started_at: Some(started_at.clone()),
        ..RestoreState::default()
    });

    let state = state.clone();
    let identity = identity.clone();
    let name = name.to_string();
    tokio::spawn(async move {
        let _held = held;
        let _shared = shared;
        let outcome = restore(&state, &identity, &name, passphrase).await;
        let finished_at = crate::db::now();

        // Written into the database as it now is: the history of the one
        // restored, or of the one kept when the restore failed.
        let record = job::start_by(
            &state.db,
            RESTORE_KIND,
            Some(&name),
            Some(&identity.label()),
        )
        .await;
        if let Ok(record) = &record {
            // Dated to when it began, which it could not be written at: the
            // database it would have been written to was the one replaced.
            if let Err(e) = sqlx::query(
                state
                    .db
                    .sql("UPDATE job_run SET started_at = ?, created_at = ? WHERE id = ?"),
            )
            .bind(&started_at)
            .bind(&started_at)
            .bind(record)
            .execute(state.db.pool())
            .await
            {
                tracing::warn!(error = %e, "could not date the restore's run");
            }
            let closed = match &outcome {
                Ok(restored) => {
                    let summary = format!(
                        "{} rows, {} media put back{}",
                        restored.rows,
                        restored.media_restored,
                        restored
                            .safety
                            .as_deref()
                            .map(|safety| format!("; undone by restoring {safety}"))
                            .unwrap_or_default()
                    );
                    job::finish(&state.db, record, Some(&summary), None).await
                }
                Err(e) => job::finish(&state.db, record, None, Some(&format!("{e:#}"))).await,
            };
            if let Err(e) = closed {
                tracing::warn!(error = %e, "could not close the restore's run");
            }
        }
        audit::record(
            &state,
            Event {
                identity: Some(&identity),
                ip: &ip,
                action: Action::BackupRestored,
                target: Some(&name),
                detail: Some(&match &outcome {
                    Ok(restored) => format!(
                        "{} rows{}",
                        restored.rows,
                        restored
                            .safety
                            .as_deref()
                            .map(|safety| format!("; safety copy {safety}"))
                            .unwrap_or_default()
                    ),
                    Err(e) => format!(
                        "failed, nothing changed: {}",
                        crate::providers::clip(&format!("{e:#}"), 300)
                    ),
                }),
            },
        )
        .await;

        *last_restore() = Some(match outcome {
            Ok(restored) => RestoreState {
                state: Status::Succeeded,
                archive: Some(name),
                started_at: Some(started_at),
                finished_at: Some(finished_at),
                error: None,
                restored: Some(restored),
            },
            Err(e) => {
                tracing::error!(error = format_args!("{e:#}"), "the restore failed");
                RestoreState {
                    state: Status::Failed,
                    archive: Some(name),
                    started_at: Some(started_at),
                    finished_at: Some(finished_at),
                    error: Some(format!("{e:#}")),
                    restored: None,
                }
            }
        });
    });
    Ok(())
}

/// The restore itself, every door shut until it is done and what the
/// server holds in memory read again.
async fn restore(
    state: &AppState,
    identity: &Identity,
    name: &str,
    passphrase: Option<age::secrecy::SecretString>,
) -> Result<Restored> {
    let Some(shut) = Shut::begin(state, "restore") else {
        anyhow::bail!("the server is under maintenance already");
    };
    let progress = |step: Step| match step {
        Step::Safety => shut.phase("safety", None, 0, 0),
        Step::Reading => shut.phase("reading", None, 0, 0),
        Step::Copying { table, step, steps } => shut.phase("copying", Some(&table), step, steps),
    };
    let restored = state
        .backups
        .restore(Request {
            source: Source::Kept(name.to_string()),
            replace: true,
            passphrase,
            by: Some(identity.label()),
            progress: Some(&progress),
        })
        .await?;
    shut.phase("reloading", None, 0, 0);
    reload(state).await;
    Ok(restored)
}

/// Read again what the server holds in memory of a database that was just
/// replaced: the settings, the rules, the media's index, the webhooks'
/// destinations, the sessions remembered — here, and on the other
/// instances, which are told. The certificate the clients' door shows is
/// read again when it changed; the authority behind it, at the next start.
async fn reload(state: &AppState) {
    if let Err(e) = state.settings.reload().await {
        tracing::warn!(error = %e, "could not read the restored settings");
    }
    state.sync_providers().await;
    if let Err(e) = state.reload_allowlist().await {
        tracing::warn!(error = %e, "could not read the restored network rules");
    }
    if state.media.is_on()
        && let Err(e) = state.media.load_index(&state.db).await
    {
        tracing::warn!(error = %e, "could not read the restored media's index");
    }
    if let Err(e) = state.webhooks.reload(&state.db).await {
        tracing::warn!(error = %e, "could not read the restored webhooks");
    }
    state.caches.forget_sessions();
    state.caches.invalidate_all().await;
    match crate::tls::reload(state).await {
        Ok(true) => state.coord.tell(crate::coord::Message::TlsReload),
        Ok(false) => {}
        Err(e) => tracing::warn!(
            error = format_args!("{e:#}"),
            "could not load the restored clients' certificate"
        ),
    }
}

/// A wait, as a span of the clock.
fn delta(wait: Duration) -> TimeDelta {
    TimeDelta::from_std(wait).unwrap_or(TimeDelta::MAX)
}
