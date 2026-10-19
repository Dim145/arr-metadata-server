//! The archives of the database, for the Backups page: listed, made now,
//! downloaded, deleted, restored. An administrator's alone: an archive holds
//! every account's password hash and every key's.

use axum::{
    Extension, Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    backup::Archive,
    db::repo::{self, audit::Action},
    error::{AppError, AppResult},
    jobs::{self, backup::RestoreState},
    state::AppState,
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(remove))
        .routes(routes!(download))
        .routes(routes!(restore))
        .routes(routes!(restore_state))
}

/// The archives, where they are kept, and how the server makes them.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupList)]
pub struct Listing {
    /// Newest first.
    pub archives: Vec<Archive>,
    /// `directory`, or `bucket` among several instances whose media are in
    /// one.
    pub place: &'static str,
    /// Where: the directory's path, or the bucket and its folder.
    pub location: String,
    /// Whether the archives the server makes are encrypted:
    /// `AMS_BACKUP_PASSPHRASE` is set.
    pub encrypted: bool,
    /// Whether an archive can carry the media: they are kept on disk.
    pub media_on_disk: bool,
    /// Where the media are kept: `off`, `filesystem` or `s3`.
    pub media_store: &'static str,
    /// `off`, `daily`, `weekly` or `monthly`.
    pub schedule: String,
    /// How many of the schedule's archives are kept.
    pub keep: i64,
    /// Whether the schedule's archives carry the media kept on disk.
    pub include_media: bool,
    /// When the newest archive was made.
    #[schema(required)]
    pub last_at: Option<String>,
    /// When the schedule makes the next; `null` while it is off.
    #[schema(required)]
    pub next_at: Option<String>,
    /// Whether the newest archive is older than twice the schedule's
    /// interval, or there is none, while the schedule is on.
    pub overdue: bool,
    /// The run making one now, when one is.
    #[schema(required)]
    pub running: Option<repo::job::Job>,
    /// Where the last restore stands.
    pub restore: RestoreState,
}

/// Every archive, newest first, and how the server makes them.
#[utoipa::path(
    get, path = "/admin/backups", tag = TAG,
    operation_id = "backup_list",
    responses(
        (status = 200, body = Listing),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Listing>> {
    identity.require_admin()?;

    let backups = &state.backups;
    // Running where its lock is held: here, or on another instance.
    let (last, _) = repo::job::latest(&state.db, jobs::backup::KIND).await?;
    let held = jobs::backup::is_busy() || state.coord.is_held(jobs::backup::HOLD).await;
    let running = last.filter(|job| job.status == repo::job::Status::Running && held);
    Ok(Json(Listing {
        archives: backups.list().await?,
        place: backups.place().kind(),
        location: backups.place().label(),
        encrypted: backups.encrypts(),
        media_on_disk: backups.media_on_disk(),
        media_store: backups.media_store_kind(),
        schedule: state
            .settings
            .resolve(jobs::backup::SCHEDULE, None, None)
            .unwrap_or_else(|| "weekly".to_string()),
        keep: jobs::backup::keep(&state) as i64,
        include_media: state.flag(jobs::backup::INCLUDE_MEDIA, false),
        last_at: backups.newest_at().await?,
        next_at: jobs::backup::next_at(&state)
            .await?
            .map(crate::db::to_rfc3339),
        overdue: jobs::backup::overdue(&state).await?,
        running,
        restore: jobs::backup::restore_state(&state).await?,
    }))
}

/// What an archive made now is to hold.
#[derive(Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupRequest)]
pub struct CreateRequest {
    /// The media kept on disk with the database. Left out, as the
    /// schedule's archives have them.
    #[serde(default)]
    pub include_media: Option<bool>,
}

/// Make an archive now, in the background: its run's id at once, the
/// archive in the list once it is whole.
#[utoipa::path(
    post, path = "/admin/backups", tag = TAG,
    operation_id = "backup_create",
    request_body = CreateRequest,
    responses(
        (status = 202, body = super::tasks::Started),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "An archive is being made, or one restored"),
    ),
)]
async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<CreateRequest>,
) -> AppResult<(StatusCode, Json<super::tasks::Started>)> {
    identity.require_admin()?;
    let job_id =
        jobs::backup::back_up_now(&state, &identity.label(), request.include_media).await?;

    // What the Jobs page's own "run now" is written down as.
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::TaskStarted,
            target: Some(jobs::backup::KIND),
            detail: job_id.as_deref(),
        },
    )
    .await;

    Ok((StatusCode::ACCEPTED, Json(super::tasks::Started { job_id })))
}

/// An archive's bytes, as kept: streamed, never held whole, and checked by
/// its SHA-256 — the `ETag` — when the server made it.
#[utoipa::path(
    get, path = "/admin/backups/{name}/download", tag = TAG,
    operation_id = "backup_download",
    params(("name" = String, Path, description = "The archive's name, as listed")),
    responses(
        (status = 200, description = "The archive", content_type = "application/octet-stream"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such archive is kept"),
    ),
)]
async fn download(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(name): Path<String>,
) -> AppResult<Response> {
    identity.require_admin()?;
    let found = state
        .backups
        .download(&name)
        .await?
        .ok_or(AppError::NotFound)?;

    let mut response = Body::from_stream(found.stream).into_response();
    let headers = response.headers_mut();
    // Plain zstd says what it is; an encrypted one is bytes to whoever has
    // no passphrase.
    let kind = if name.ends_with(".age") {
        "application/octet-stream"
    } else {
        "application/zstd"
    };
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(found.size));
    // The name is an archive's, which is letters, digits, dashes and dots.
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(sha) = found.sha256
        && let Ok(tag) = HeaderValue::from_str(&format!("\"{sha}\""))
    {
        headers.insert(header::ETAG, tag);
    }
    Ok(response)
}

/// Take an archive away, and forget it.
#[utoipa::path(
    delete, path = "/admin/backups/{name}", tag = TAG,
    operation_id = "backup_remove",
    params(("name" = String, Path, description = "The archive's name, as listed")),
    responses(
        (status = 204, description = "Taken away"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such archive"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(name): Path<String>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;
    if !state.backups.delete(&name).await? {
        return Err(AppError::NotFound);
    }
    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::BackupDeleted,
            target: Some(&name),
            detail: None,
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// What opens the archive to restore.
#[derive(Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BackupRestoreRequest)]
pub struct RestoreRequest {
    /// Its passphrase, when it is encrypted with another than the server's
    /// own. Never kept, never logged.
    #[serde(default)]
    pub passphrase: Option<String>,
}

/// Restore an archive over the database, in the background. An archive of
/// the database is made first — what undoes it — and every door of this
/// instance answers 503 until it is done: the interface, the API, Sonarr's
/// and Radarr's surfaces, the relays. Everybody is signed out but those
/// whose session the archive holds. An archive this server cannot read
/// whole, or a passphrase that does not open it, is refused before anything
/// starts.
#[utoipa::path(
    post, path = "/admin/backups/{name}/restore", tag = TAG,
    operation_id = "backup_restore",
    params(("name" = String, Path, description = "The archive's name, as listed")),
    request_body = RestoreRequest,
    responses(
        (status = 202, description = "Started: follow it at /admin/backups/restore"),
        (status = 400, description = "The archive cannot be read: encrypted with another passphrase, or damaged"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such archive is kept"),
        (status = 409, description = "An archive is being made or restored, or this one was made by a later version"),
    ),
)]
async fn restore(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(name): Path<String>,
    Json(request): Json<RestoreRequest>,
) -> AppResult<StatusCode> {
    identity.require_admin()?;
    jobs::backup::restore_now(&state, &identity, ip, &name, request.passphrase).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Where the last restore stands: the one this instance ran, else the last
/// the history holds. While one runs the server answers 503 to everything,
/// this included, with the step it is at.
#[utoipa::path(
    get, path = "/admin/backups/restore", tag = TAG,
    operation_id = "backup_restore_state",
    responses(
        (status = 200, body = RestoreState),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn restore_state(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<RestoreState>> {
    identity.require_admin()?;
    Ok(Json(jobs::backup::restore_state(&state).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::user::Role;

    fn admin() -> Extension<Identity> {
        Extension(crate::auth::testing::key(&["admin"], None))
    }

    #[tokio::test]
    async fn the_archives_are_an_administrator_s_alone() {
        let state = super::super::testing::server().await;
        for identity in [
            crate::auth::testing::key(&["read", "write"], Some(Role::Editor)),
            crate::auth::testing::key(&["metrics"], None),
            Identity::Visitor,
        ] {
            assert!(matches!(
                list(State(state.clone()), Extension(identity.clone())).await,
                Err(AppError::Forbidden)
            ));
            assert!(matches!(
                download(
                    State(state.clone()),
                    Extension(identity.clone()),
                    Path("ams-20261019-020000-manual.tar.zst".into())
                )
                .await,
                Err(AppError::Forbidden)
            ));
        }
    }

    #[tokio::test]
    async fn an_archive_made_now_is_listed_downloaded_and_deleted() {
        let state = super::super::testing::server().await;
        let Json(empty) = list(State(state.clone()), admin()).await.unwrap();
        assert!(empty.archives.is_empty());
        assert!(empty.overdue, "none made, the schedule weekly");
        assert_eq!(empty.schedule, "weekly");
        assert_eq!(empty.keep, 4);
        assert_eq!(empty.place, "directory");

        let made = state
            .backups
            .make(crate::backup::Ask::new(crate::backup::Reason::Manual))
            .await
            .unwrap();
        let Json(listed) = list(State(state.clone()), admin()).await.unwrap();
        assert_eq!(listed.archives.len(), 1);
        let archive = &listed.archives[0];
        assert_eq!(archive.name, made.name);
        assert!(archive.present && archive.described);
        assert_eq!(archive.size_bytes, made.size);
        assert!(!listed.overdue);

        let response = download(State(state.clone()), admin(), Path(made.name.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::ETAG],
            format!("\"{}\"", made.sha256)
        );
        assert!(
            response.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .contains(&made.name)
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 30)
            .await
            .unwrap();
        assert_eq!(bytes.len() as u64, made.size);
        assert_eq!(crate::backup::archive::sha256_of(&bytes), made.sha256);

        // A name that is not an archive's names nothing, wherever it points.
        for name in ["../ams.db", "ams.db", "ams-20261019-020000-manual.tar.zst"] {
            assert!(matches!(
                download(State(state.clone()), admin(), Path(name.into())).await,
                Err(AppError::NotFound)
            ));
        }

        let gone = remove(
            State(state.clone()),
            admin(),
            ClientIp::NONE,
            Path(made.name.clone()),
        )
        .await
        .unwrap();
        assert_eq!(gone, StatusCode::NO_CONTENT);
        assert!(matches!(
            remove(
                State(state.clone()),
                admin(),
                ClientIp::NONE,
                Path(made.name)
            )
            .await,
            Err(AppError::NotFound)
        ));
        let Json(listed) = list(State(state), admin()).await.unwrap();
        assert!(listed.archives.is_empty());
    }

    async fn titles(state: &AppState) -> Vec<String> {
        let mut titles: Vec<String> = repo::item::search(
            &state.db,
            &repo::item::Query {
                limit: 50,
                include_disabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .into_iter()
        .map(|work| work.title)
        .collect();
        titles.sort();
        titles
    }

    async fn store(state: &AppState, title: &str) {
        let mut work = crate::domain::MediaItem::empty(crate::domain::MediaKind::Movie);
        work.title = title.into();
        work.slug = crate::domain::make_slug(title, Some(2026));
        repo::item::upsert(
            &state.db,
            repo::item::ItemWrite {
                item: &work,
                replace_children: false,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_restore_from_the_page_shuts_the_doors_until_it_is_done_and_is_written_down() {
        let state = super::super::testing::server().await;
        store(&state, "Kept in the archive").await;
        let made = state
            .backups
            .make(crate::backup::Ask::new(crate::backup::Reason::Manual))
            .await
            .unwrap();
        store(&state, "Made after it").await;

        assert!(matches!(
            restore(
                State(state.clone()),
                admin(),
                ClientIp::NONE,
                Path("ams-20200101-000000-manual.tar.zst".into()),
                Json(RestoreRequest::default()),
            )
            .await,
            Err(AppError::NotFound)
        ));

        let started = restore(
            State(state.clone()),
            admin(),
            ClientIp::NONE,
            Path(made.name.clone()),
            Json(RestoreRequest::default()),
        )
        .await
        .unwrap();
        assert_eq!(started, StatusCode::ACCEPTED);

        let mut waited = 0;
        let done = loop {
            let Json(now) = restore_state(State(state.clone()), admin()).await.unwrap();
            if now.state != jobs::backup::Status::Running {
                break now;
            }
            waited += 1;
            assert!(waited < 600, "the restore never ended");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };
        assert_eq!(
            done.state,
            jobs::backup::Status::Succeeded,
            "{:?}",
            done.error
        );
        assert!(!state.maintenance.is_on(), "the doors are open again");
        assert_eq!(titles(&state).await, ["Kept in the archive"]);

        // What undoes it is listed, and the restore is in the history and
        // the journal of the database restored.
        let restored = done.restored.expect("what was restored");
        let safety = restored.safety.expect("a safety copy");
        let Json(listed) = list(State(state.clone()), admin()).await.unwrap();
        assert!(listed.archives.iter().any(|a| a.name == safety));
        let (run, _) = repo::job::latest(&state.db, jobs::backup::RESTORE_KIND)
            .await
            .unwrap();
        let run = run.expect("a run");
        assert_eq!(run.status, repo::job::Status::Succeeded);
        assert_eq!(run.target.as_deref(), Some(made.name.as_str()));
        let trail = repo::audit::list(
            &state.db,
            &repo::audit::Query {
                action: Some("backup.restored".into()),
                limit: 5,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(trail.len(), 1);
    }
}
