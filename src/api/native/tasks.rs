//! The server's background tasks, and running one now.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action},
    error::{AppError, AppResult},
    jobs::{self, tasks},
    state::AppState,
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(run))
        .routes(routes!(cancel))
}

/// Every background task: when it runs, how it last went, when it runs next.
#[utoipa::path(
    get, path = "/tasks", tag = TAG,
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
    params(("id" = String, Path, description = "`refresh.sweep`, `refresh.all`, `import.anime`, `import.imdb` or `export.nfo`")),
    responses(
        (status = 202, body = Started),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such task"),
        (status = 409, description = "It is running already, or its source is off"),
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
            jobs::datasets::import_in_background(&state, jobs::datasets::ANIME, &identity, ip)?
        }
        tasks::IMPORT_IMDB => {
            jobs::datasets::import_in_background(&state, jobs::datasets::IMDB, &identity, ip)?
        }
        tasks::EXPORT_NFO => {
            crate::api::native::export::start_export(&state, &identity, &ip)
                .await?
                .1
        }
        _ => return Err(AppError::NotFound),
    };

    audit::record(
        &state,
        Event {
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
        return Err(AppError::Conflict(
            "that run is not running, or cannot be stopped partway".into(),
        ));
    }

    audit::record(
        &state,
        Event {
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
