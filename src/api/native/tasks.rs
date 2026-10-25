//! The server's background tasks, and running one now; when the periodic
//! ones run; and the tasks followed as they go.

use std::{
    convert::Infallible,
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderName, HeaderValue, StatusCode},
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event as Audited},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action},
    error::{AppError, AppResult},
    jobs::{
        self,
        schedule::{self, Setting},
        tasks::{self, TaskId},
    },
    state::AppState,
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(run))
        .routes(routes!(detail))
        .routes(routes!(cancel))
        .routes(routes!(stream))
        .routes(routes!(set_schedule, clear_schedule))
        .routes(routes!(preview_schedule))
}

/// How many of a run's works the detail hands back at once: a refresh of
/// everything over a large catalogue runs to thousands, and a page that long
/// says less than its count.
const MOST_ENTRIES: i64 = 500;

/// One run, with what it did to each work.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobDetail {
    pub job: repo::job::Job,
    /// The works taken, in order — the first five hundred of them.
    pub entries: Vec<repo::job::Entry>,
    /// How many there were in all.
    pub entries_total: i64,
}

/// One run of the history, and the detail behind its summary: which works it
/// took, and how each went. A run older than the detail, or of a task that
/// keeps none, has an empty list.
#[utoipa::path(
    get, path = "/jobs/{id}", tag = TAG,
    operation_id = "tasks_detail",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = JobDetail),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such run"),
    ),
)]
async fn detail(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<JobDetail>> {
    identity.require_admin()?;

    let mut job = repo::job::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    // A refresh names its work by id; the page names it by its title.
    if let Some(target) = job.target.clone() {
        job.work = repo::item::titles(&state.db, &[target])
            .await?
            .into_values()
            .next();
    }
    let mut entries = repo::job::entries(&state.db, &id, MOST_ENTRIES).await?;
    let entries_total = repo::job::count_entries(&state.db, &id).await?;

    // A work gone since the run is named by its title alone: an id that
    // leads nowhere is no link.
    let ids: Vec<String> = entries.iter().filter_map(|e| e.media_id.clone()).collect();
    let held = repo::item::titles(&state.db, &ids).await?;
    for entry in &mut entries {
        if entry
            .media_id
            .as_deref()
            .is_some_and(|media_id| !held.contains_key(media_id))
        {
            entry.media_id = None;
        }
    }

    Ok(Json(JobDetail {
        job,
        entries,
        entries_total,
    }))
}

/// Every background task: when it runs, how it last went, when it runs next.
#[utoipa::path(
    get, path = "/tasks", tag = TAG,
    operation_id = "tasks_list",
    responses(
        (status = 200, body = Vec<tasks::TaskState>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<tasks::TaskState>>> {
    identity.require_admin()?;
    Ok(Json(tasks::states(&state).await?))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    /// The run, as the history files it, once it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

/// Run a task now, in the background.
#[utoipa::path(
    post, path = "/tasks/{id}/run", tag = TAG,
    operation_id = "tasks_run",
    params(("id" = tasks::TaskId, Path, description = "The task to run")),
    responses(
        (status = 202, body = Started),
        (status = 400, description = "A batch of works, which is started from the import page with its lines"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such task"),
        (status = 409, description = "It is running already, or a backup is being restored"),
        (status = 503, description = "Its source or its switch is off: no media store, no export path, no certificate of the server's own, the check for a newer version off, neither Sonarr nor Radarr connected, the AVIF copies off, no import rule switched on"),
    ),
)]
async fn run(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<(StatusCode, Json<Started>)> {
    identity.require_admin()?;
    let by = identity.label();

    let job_id = match id.as_str() {
        tasks::REFRESH_SWEEP => jobs::refresh::sweep_now(&state, &by).await?,
        tasks::REFRESH_ALL => jobs::refresh::refresh_everything(&state, &by).await?,
        tasks::IMPORT_ANIME => {
            jobs::datasets::import_in_background(&state, jobs::datasets::ANIME, &identity, ip)
                .await?
        }
        tasks::IMPORT_IMDB => {
            jobs::datasets::import_in_background(&state, jobs::datasets::IMDB, &identity, ip)
                .await?
        }
        tasks::IMDB_AKAS => {
            jobs::datasets::import_in_background(&state, jobs::datasets::IMDB_AKAS, &identity, ip)
                .await?
        }
        tasks::IMDB_EPISODES => {
            jobs::datasets::import_in_background(
                &state,
                jobs::datasets::IMDB_EPISODES,
                &identity,
                ip,
            )
            .await?
        }
        tasks::IMPORT_BATCH => {
            return Err(AppError::BadRequest(
                "a batch is started from the import page, with the lines it imports".into(),
            ));
        }
        tasks::IMPORT_RULES => jobs::rules::start_all(&state, &by).await?,
        tasks::EXPORT_NFO => {
            crate::api::native::export::start_export(&state, &identity, &ip)
                .await?
                .1
        }
        tasks::MEDIA_STORE => crate::media::worker::store_all(&state, &by).await?,
        tasks::MEDIA_SWEEP => crate::media::worker::sweep_now(&state, &by).await?,
        tasks::MEDIA_VARIANTS => jobs::media::prepare_now(&state, &by).await?,
        tasks::MEDIA_AVIF => jobs::media::avif_now(&state, Some(&by)).await?,
        tasks::TLS_RENEW => crate::tls::renew_now(&state, &by).await?,
        tasks::UPDATE_CHECK => jobs::update_check::check_now(&state, &by).await?,
        tasks::ARR_SEED => crate::arr::seed::start(&state, &by).await?,
        tasks::BACKUP_RUN => jobs::backup::back_up_now(&state, &by, None).await?,
        tasks::SEARCH_REBUILD => jobs::search::rebuild_now(&state, &by).await?,
        tasks::LISTING => jobs::listing::now(&state, &by).await?,
        _ => return Err(AppError::NotFound),
    };

    audit::record(
        &state,
        Audited {
            identity: Some(&identity),
            ip: &ip,
            action: Action::TaskStarted,
            target: Some(&id),
            detail: job_id.as_deref(),
        },
    )
    .await;

    Ok((StatusCode::ACCEPTED, Json(Started { job_id })))
}

/// Ask a run to stop at the next work. Only a run that can stop partway.
#[utoipa::path(
    post, path = "/jobs/{id}/cancel", tag = TAG,
    operation_id = "tasks_cancel",
    params(("id" = String, Path)),
    responses(
        (status = 202, description = "Asked; it stops at the next work"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such run"),
        (status = 409, description = "It is not running, or cannot stop partway"),
    ),
)]
async fn cancel(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;

    let run = repo::job::get(&state.db, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !jobs::cancel::cancel(&run.id) {
        // Not running here: perhaps on another instance, which is told.
        let elsewhere = run.status == repo::job::Status::Running
            && jobs::tasks::stops_partway(&run.kind)
            && run.instance.as_deref().is_some_and(|name| {
                name != state.coord.instance.name && state.coord.is_alive(name)
            });
        if !elsewhere {
            return Err(AppError::Conflict(
                "that run is not running, or cannot be stopped partway".into(),
            ));
        }
        state
            .coord
            .tell(crate::coord::Message::Stop(run.id.clone()));
    }

    audit::record(
        &state,
        Audited {
            identity: Some(&identity),
            ip: &ip,
            action: Action::TaskStopped,
            target: Some(&run.kind),
            detail: Some(&run.id),
        },
    )
    .await;

    Ok(StatusCode::ACCEPTED)
}

// ─── followed as they go ─────────────────────────────────────────────────────

/// How often a stream asks whether anything moved: a run started, said how
/// far it got, or ended; a schedule changed.
const PULSE_EVERY: Duration = Duration::from_secs(1);

/// How often it reads the whole board whatever the pulse says: what moves
/// without a run — a source switched on, a hold taken on another instance,
/// the next run coming nearer — is said within this.
const BOARD_EVERY: Duration = Duration::from_secs(15);

/// How long one stream lasts before the page is asked to open another. The
/// credential is asked for again each time, so a session that ended is
/// followed no longer than this.
const STREAM_LIFE: Duration = Duration::from_secs(5 * 60);

/// How long a page waits before it opens another stream, once one ends.
const RETRY: Duration = Duration::from_secs(2);

/// What a stream keeps between two events.
struct Follow {
    state: AppState,
    opened: Instant,
    looked: bool,
    pulse: Option<String>,
    board_at: Option<Instant>,
    sent: Option<String>,
}

/// What changes whenever the board may have: the runs in progress and what
/// they last said, the newest run, the schedules. Nothing when it could not
/// be read, which reads the whole board instead.
async fn pulse(state: &AppState) -> Option<String> {
    let runs = repo::job::pulse(&state.db).await.ok()?;
    let schedules = repo::task_schedule::changed_at(&state.db).await.ok()?;
    Some(format!("{runs}#{schedules}"))
}

/// The tasks, followed: an event `tasks` carrying the whole board — every
/// task as `GET /tasks` gives it, and the newest run — at once, then each
/// time it changes, whichever instance made the change: it is read from the
/// history every instance writes and the holds they share, never from this
/// process's memory alone. A comment keeps a quiet stream open through a
/// proxy; the stream ends after five minutes and the page opens another,
/// which asks for the credential again.
#[utoipa::path(
    get, path = "/admin/tasks/stream", tag = TAG,
    operation_id = "tasks_stream",
    responses(
        (status = 200, body = tasks::Board, content_type = "text/event-stream", description = "Server-sent events, each named `tasks`, whose data is the board as JSON"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn stream(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<impl IntoResponse> {
    identity.require_admin()?;

    let follow = Follow {
        state,
        opened: Instant::now(),
        looked: false,
        pulse: None,
        board_at: None,
        sent: None,
    };
    let events = futures::stream::unfold(follow, |mut follow| async move {
        loop {
            if follow.opened.elapsed() >= STREAM_LIFE {
                return None;
            }
            if follow.looked {
                tokio::time::sleep(PULSE_EVERY).await;
            }
            follow.looked = true;

            let pulse = pulse(&follow.state).await;
            let stale = follow.board_at.is_none_or(|at| at.elapsed() >= BOARD_EVERY);
            if !stale && pulse.is_some() && pulse == follow.pulse {
                continue;
            }
            follow.pulse = pulse;
            follow.board_at = Some(Instant::now());

            let text = match tasks::board(&follow.state).await {
                Ok(board) => match serde_json::to_string(&board) {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::debug!(error = %e, "could not write the tasks' board");
                        continue;
                    }
                },
                Err(e) => {
                    tracing::debug!(
                        error = format_args!("{e:#}"),
                        "could not read the tasks' board"
                    );
                    continue;
                }
            };
            if follow.sent.as_deref() == Some(text.as_str()) {
                continue;
            }
            let first = follow.sent.is_none();
            follow.sent = Some(text.clone());
            let mut event = Event::default().event("tasks").data(text);
            if first {
                event = event.retry(RETRY);
            }
            return Some((Ok::<_, Infallible>(event), follow));
        }
    });

    Ok((
        // A proxy that holds an answer back until it is whole would hold
        // this one for ever: nginx is told not to.
        [(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        )],
        Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))),
    ))
}

// ─── when the periodic ones run ──────────────────────────────────────────────

/// Set when a periodic task runs: its own cadence kept to a window of the
/// day, or the moments a cron expression names instead — in UTC. The sweep,
/// the two lists downloaded whole and the archive take one; the rest keep
/// their own cadence.
#[utoipa::path(
    put, path = "/tasks/{id}/schedule", tag = TAG,
    operation_id = "tasks_set_schedule",
    params(("id" = tasks::TaskId, Path, description = "The task")),
    request_body = Setting,
    responses(
        (status = 200, body = tasks::TaskState),
        (status = 400, description = "Not a window or an expression this server reads — said why — or a task that keeps its own cadence"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such task"),
    ),
)]
async fn set_schedule(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(setting): Json<Setting>,
) -> AppResult<Json<tasks::TaskState>> {
    identity.require_admin()?;
    let task = schedulable(&id)?;
    let setting = setting.normalized().map_err(AppError::BadRequest)?;
    let by = identity.label();
    repo::task_schedule::set(
        &state.db,
        task.as_str(),
        repo::task_schedule::Set {
            kind: setting.kind.as_str(),
            window_start: setting.from.as_deref(),
            window_end: setting.to.as_deref(),
            cron: setting.cron.as_deref(),
            by: Some(&by),
        },
    )
    .await?;

    let said = match (&setting.from, &setting.to, &setting.cron) {
        (Some(from), Some(to), _) => format!("from {from} to {to} UTC"),
        (_, _, Some(cron)) => format!("cron {cron}"),
        _ => String::new(),
    };
    audit::record(
        &state,
        Audited {
            identity: Some(&identity),
            ip: &ip,
            action: Action::TaskScheduled,
            target: Some(task.as_str()),
            detail: Some(&said),
        },
    )
    .await;

    Ok(Json(tasks::state_named(&state, task).await?))
}

/// Leave a periodic task to its own cadence again.
#[utoipa::path(
    delete, path = "/tasks/{id}/schedule", tag = TAG,
    operation_id = "tasks_clear_schedule",
    params(("id" = tasks::TaskId, Path, description = "The task")),
    responses(
        (status = 200, body = tasks::TaskState),
        (status = 400, description = "A task that keeps its own cadence"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such task"),
    ),
)]
async fn clear_schedule(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Json<tasks::TaskState>> {
    identity.require_admin()?;
    let task = schedulable(&id)?;
    if repo::task_schedule::clear(&state.db, task.as_str()).await? {
        audit::record(
            &state,
            Audited {
                identity: Some(&identity),
                ip: &ip,
                action: Action::TaskScheduled,
                target: Some(task.as_str()),
                detail: Some("its own cadence"),
            },
        )
        .await;
    }
    Ok(Json(tasks::state_named(&state, task).await?))
}

/// The task of that name, when a schedule can be set for it.
fn schedulable(id: &str) -> AppResult<TaskId> {
    let task = TaskId::named(id).ok_or(AppError::NotFound)?;
    if !schedule::is_schedulable(task.as_str()) {
        return Err(AppError::BadRequest(format!(
            "{id} keeps its own cadence: no window or expression can be set for it"
        )));
    }
    Ok(task)
}

/// The next moments a schedule names.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleMoments {
    /// The next few, from now: a window's next openings — now, when it is
    /// open — or an expression's next moments.
    pub next: Vec<String>,
}

/// What a schedule being written comes to: its next moments, or why it is
/// none. Nothing is kept.
#[utoipa::path(
    post, path = "/tasks/schedules/preview", tag = TAG,
    operation_id = "tasks_preview_schedule",
    request_body = Setting,
    responses(
        (status = 200, body = ScheduleMoments),
        (status = 400, description = "Not a window or an expression this server reads, and why"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn preview_schedule(
    Extension(identity): Extension<Identity>,
    Json(setting): Json<Setting>,
) -> AppResult<Json<ScheduleMoments>> {
    // The rules' page asks it too, which an editor may open.
    identity.require_write()?;
    let read = setting.read().map_err(AppError::BadRequest)?;
    Ok(Json(ScheduleMoments {
        next: schedule::upcoming(&read, chrono::Utc::now(), 3)
            .into_iter()
            .map(crate::db::to_rfc3339)
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::header, response::Response};
    use futures::StreamExt as _;

    use super::*;
    use crate::api::native::testing::server;

    fn window(from: &str, to: &str) -> Setting {
        Setting {
            kind: schedule::Kind::Window,
            from: Some(from.into()),
            to: Some(to.into()),
            cron: None,
            updated_by: None,
            updated_at: None,
        }
    }

    fn cron(expression: &str) -> Setting {
        Setting {
            kind: schedule::Kind::Cron,
            from: None,
            to: None,
            cron: Some(expression.into()),
            updated_by: None,
            updated_at: None,
        }
    }

    async fn set(state: &AppState, task: &str, setting: Setting) -> AppResult<tasks::TaskState> {
        set_schedule(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path(task.to_string()),
            Json(setting),
        )
        .await
        .map(|Json(task)| task)
    }

    #[tokio::test]
    async fn a_schedule_is_set_read_back_and_taken_away() {
        let state = server().await;

        // The sweep at night: its next run inside the window.
        let task = set(&state, "refresh.sweep", window("01:00", "06:00"))
            .await
            .expect("set");
        assert!(task.schedulable);
        let setting = task.schedule.expect("its schedule");
        assert_eq!(setting.kind, schedule::Kind::Window);
        assert_eq!(setting.from.as_deref(), Some("01:00"));
        assert_eq!(setting.updated_by.as_deref(), Some("anonymous"));
        let next = task.next_at.expect("a next run");
        let hour: u32 = next[11..13].parse().unwrap();
        assert!((1..6).contains(&hour), "{next} is inside the window");

        // IMDb on Sundays, written with spaces to spare.
        set(&state, "import.imdb", cron("0  4 * * SUN"))
            .await
            .expect("set");

        // What is no schedule is said why; a task that keeps its own cadence
        // takes none; a task that does not exist is not found.
        assert!(matches!(
            set(&state, "refresh.sweep", cron("0 0 30 2 *")).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            set(&state, "refresh.sweep", window("03:00", "03:00")).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            set(&state, "tls.renew", cron("0 4 * * 0")).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            set(&state, "nothing", cron("0 4 * * 0")).await,
            Err(AppError::NotFound)
        ));

        // Every task as it stands says what was set, as it was kept.
        let states = tasks::states(&state).await.unwrap();
        let of = |id: &str| states.iter().find(|t| t.id.as_str() == id).unwrap();
        assert_eq!(
            of("import.imdb")
                .schedule
                .as_ref()
                .and_then(|s| s.cron.as_deref()),
            Some("0 4 * * SUN")
        );
        // The listing, every half minute, without a schedule of its own —
        // nor a next moment, which would only ever say now.
        assert_eq!(of("listing.relist").mode, tasks::Mode::Scheduled);
        assert_eq!(of("listing.relist").every_seconds, Some(30));
        assert!(of("listing.relist").next_at.is_none());
        assert!(!of("listing.relist").schedulable);
        // No rule switched on: the rules' task is off, and says why.
        assert_eq!(of("import.rules").blocked, Some(tasks::Blocked::NoRules));

        // Taken away: its own cadence again.
        let Json(task) = clear_schedule(
            State(state.clone()),
            Extension(Identity::Anonymous),
            ClientIp::NONE,
            Path("refresh.sweep".into()),
        )
        .await
        .expect("cleared");
        assert!(task.schedule.is_none());

        // A schedule being written, previewed.
        let Json(moments) = preview_schedule(Extension(Identity::Anonymous), Json(cron("@weekly")))
            .await
            .expect("previewed");
        assert_eq!(moments.next.len(), 3);
        assert!(matches!(
            preview_schedule(Extension(Identity::Anonymous), Json(cron("nonsense"))).await,
            Err(AppError::BadRequest(_))
        ));
        // Not for a visitor.
        assert!(
            preview_schedule(Extension(Identity::Visitor), Json(cron("@weekly")))
                .await
                .is_err()
        );
    }

    /// The next event of a stream, whole.
    async fn next_event(body: &mut axum::body::BodyDataStream) -> String {
        let mut text = String::new();
        while !text.contains("\n\n") {
            let chunk = tokio::time::timeout(Duration::from_secs(10), body.next())
                .await
                .expect("an event in time")
                .expect("the stream goes on")
                .expect("a chunk");
            text.push_str(&String::from_utf8_lossy(&chunk));
        }
        text
    }

    #[tokio::test]
    async fn the_tasks_are_followed_as_a_stream_of_their_board() {
        let state = server().await;

        // Nobody but an administrator.
        assert!(
            stream(State(state.clone()), Extension(Identity::Visitor))
                .await
                .is_err()
        );

        let answer: Response = stream(State(state.clone()), Extension(Identity::Anonymous))
            .await
            .expect("a stream")
            .into_response();
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(answer.headers()[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(answer.headers()["x-accel-buffering"], "no");

        // The first event, at once: the whole board, and when to come back.
        let mut body = Body::new(answer.into_body()).into_data_stream();
        let first = next_event(&mut body).await;
        assert!(first.contains("event: tasks"), "{first}");
        assert!(first.contains("retry: 2000"), "{first}");
        let data = first
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("its data");
        let board: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(
            board["tasks"].as_array().map(Vec::len),
            Some(TaskId::ALL.len())
        );

        // A run opened — by this instance or another, the history is the
        // same — is said in the next event, within a second or two.
        let run = repo::job::start_by(&state.db, tasks::IMPORT_BATCH, None, Some("admin:x"))
            .await
            .unwrap();
        repo::job::progress(
            &state.db,
            &run,
            "3 of 500: 1 already here, 2 imported, 0 not found, 0 failed",
        )
        .await
        .unwrap();
        let mut said = next_event(&mut body).await;
        while !said.contains(&run) {
            said = next_event(&mut body).await;
        }
        assert!(said.contains("\"newestRun\""), "{said}");
    }
}
