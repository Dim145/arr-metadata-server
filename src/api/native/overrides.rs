//! Manual edits and the locks they create.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{self, audit::Action},
    domain::{
        MediaKind,
        fields::{self, Override, Scope},
    },
    error::{AppError, AppResult},
    service,
    state::AppState,
};

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Locks";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, set, clear))
        .routes(routes!(unset))
}

/// Every manual edit stored against a work.
///
/// Of a work this caller may see, as its own page decides it. Who made each
/// edit is said to whoever maintains the catalogue: to anybody else it would
/// name accounts and keys.
#[utoipa::path(
    get, path = "/items/{id}/overrides", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, body = Vec<Override>),
        (status = 404, description = "No such work, or none this caller may see"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<Override>>> {
    super::visible_work(&state, &identity, &id).await?;

    let mut overrides = repo::override_field::list(&state.db, &id).await?;
    if !identity.can_write() {
        for edit in &mut overrides {
            edit.updated_by = None;
        }
    }
    Ok(Json(overrides))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetRequest {
    /// `item`, `season:3`, `episode:3x7`. Defaults to the work itself.
    #[serde(default = "default_scope")]
    pub scope: String,
    pub field: String,
    /// Absent or `null` stores an explicit "cleared" value, which still locks
    /// the field. To hand it back to the provider, delete the override instead.
    pub value: Option<Value>,
}

fn default_scope() -> String {
    "item".to_string()
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetResponse {
    pub locked_fields: Vec<String>,
}

/// Record an edit, locking the field.
///
/// From this point every refresh leaves the field alone. Sending a `null` value
/// stores an explicit "cleared" state, which still locks it; to hand the field
/// back to the provider, delete the override instead.
#[utoipa::path(
    put, path = "/items/{id}/overrides", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    request_body = SetRequest,
    responses(
        (status = 200, description = "Every field now locked on this work", body = SetResponse),
        (status = 400, description = "Unknown field, wrong type, or unparsable scope"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such work, season or episode"),
    ),
)]
async fn set(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(mut request): Json<SetRequest>,
) -> AppResult<Json<SetResponse>> {
    require_write(&identity)?;

    let scope: Scope = request
        .scope
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;

    // An address of this server's own — the editor shows the copies kept —
    // is locked as the provider's: what the sweep looks for, and what stays
    // right if the copy is ever forgotten.
    if matches!(
        request.field.as_str(),
        "image" | "themeMusic" | "primaryPoster" | "primaryFanart"
    ) && let Some(serde_json::Value::String(url)) = &request.value
    {
        request.value = Some(serde_json::Value::String(state.media.unlocalize(url)));
    }

    fields::validate(scope, &request.field, request.value.as_ref())
        .map_err(AppError::BadRequest)?;

    // Refuse to attach an override to something that is not there: it would
    // silently do nothing on read.
    let item = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    match scope {
        Scope::Season(n) if !item.seasons.iter().any(|s| s.season_number == n) => {
            return Err(AppError::NotFound);
        }
        Scope::Episode { season, episode }
            if !item
                .episodes
                .iter()
                .any(|e| e.season_number == season && e.episode_number == episode) =>
        {
            return Err(AppError::NotFound);
        }
        _ => {}
    }

    lock(
        &state,
        (&item.id, item.kind),
        scope,
        &request.field,
        request.value.as_ref(),
        &identity.label(),
    )
    .await?;
    service::listing::after_write(&state, &id).await;

    tracing::info!(
        %id, %scope, field = %request.field, actor = %identity.label(),
        "locked a field with a manual edit"
    );

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OverrideSet,
            target: Some(&format!("{id}#{scope}/{}", request.field)),
            // The value itself is not recorded: an overview runs to paragraphs,
            // and the current value is one read away.
            detail: Some(if request.value.is_some() {
                "set"
            } else {
                "cleared"
            }),
        },
    )
    .await;

    let updated = service::load(&state, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(SetResponse {
        locked_fields: updated.locked_fields,
    }))
}

/// Unlock one field, handing it back to provider data on the next refresh.
#[utoipa::path(
    delete, path = "/items/{id}/overrides/{scope}/{field}", tag = TAG,
    params(
        ("id" = String, Path, description = "The work's identifier"),
        ("scope" = String, Path, description = "`item`, `season:3` or `episode:3x7`"),
        ("field" = String, Path, description = "The field name, as the registry reports it"),
    ),
    responses(
        (status = 204, description = "Unlocked"),
        (status = 400, description = "Unparsable scope"),
        (status = 403, description = "The caller may not write"),
        (status = 404, description = "No such override"),
    ),
)]
async fn unset(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((id, scope, field)): Path<(String, String, String)>,
) -> AppResult<StatusCode> {
    require_write(&identity)?;

    let scope: Scope = scope
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;

    // An upload locked into the field goes with the lock.
    let uploads = uploads_locked(&state, &id, Some((scope, field.as_str()))).await?;
    if !repo::override_field::unset(&state.db, &id, scope, &field).await? {
        return Err(AppError::NotFound);
    }
    super::media::forget_uploads(&state, uploads.iter().map(String::as_str)).await;

    state.caches.touched(&id).await;
    service::listing::after_write(&state, &id).await;
    tracing::info!(%id, %scope, %field, actor = %identity.label(), "unlocked a field");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OverrideRemoved,
            target: Some(&format!("{id}#{scope}/{field}")),
            detail: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClearResponse {
    pub removed: u64,
}

/// Unlock every field of a work at once.
#[utoipa::path(
    delete, path = "/items/{id}/overrides", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses(
        (status = 200, body = ClearResponse),
        (status = 403, description = "The caller may not write"),
    ),
)]
async fn clear(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<Json<ClearResponse>> {
    require_write(&identity)?;

    let uploads = uploads_locked(&state, &id, None).await?;
    let removed = repo::override_field::clear(&state.db, &id).await?;
    super::media::forget_uploads(&state, uploads.iter().map(String::as_str)).await;

    state.caches.touched(&id).await;
    service::listing::after_write(&state, &id).await;
    tracing::info!(%id, removed, actor = %identity.label(), "unlocked every field");

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::OverridesCleared,
            target: Some(&id),
            detail: Some(&format!("{removed} fields unlocked")),
        },
    )
    .await;

    Ok(Json(ClearResponse { removed }))
}

/// Lock a field of a work, or of one of its seasons or episodes: what an edit
/// by hand and an imported lock both come to.
///
/// A lock on the work's identity — adult, its slug, its identifiers — is
/// written to its row as well, in the lock's own transaction: the lists, the
/// calendar, the feeds, the addresses and the clients' lookups read the row,
/// not the lock, and an adult work locked so is kept from all of them at
/// once. A slug or an identifier another work holds refuses the lock, with
/// nothing written (409). The value has been checked against the registry.
/// The work is served afresh from here on; listing it again is the caller's.
pub(super) async fn lock(
    state: &AppState,
    (id, kind): (&str, MediaKind),
    scope: Scope,
    field: &str,
    value: Option<&Value>,
    by: &str,
) -> AppResult<()> {
    match scope {
        Scope::Item => {
            repo::override_field::set_on_work(&state.db, id, kind, field, value, Some(by))
                .await?
                .map_err(|taken| AppError::Conflict(taken.0))?;
        }
        _ => repo::override_field::set(&state.db, id, scope, field, value, Some(by)).await?,
    }

    state.caches.touched(id).await;
    Ok(())
}

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}

/// The uploads a work's locks point at — one lock's, or every lock's — for
/// deleting when the lock goes.
async fn uploads_locked(
    state: &AppState,
    id: &str,
    only: Option<(Scope, &str)>,
) -> AppResult<Vec<String>> {
    let locked = repo::override_field::list(&state.db, id).await?;
    Ok(locked
        .into_iter()
        .filter(|lock| {
            only.is_none_or(|(scope, field)| {
                lock.field == field && lock.scope.parse::<Scope>().ok() == Some(scope)
            })
        })
        .filter_map(|lock| match lock.value {
            Some(serde_json::Value::String(url)) if url.starts_with("upload:") => Some(url),
            _ => None,
        })
        .collect())
}
