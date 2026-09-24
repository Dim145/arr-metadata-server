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
    domain::fields::{self, Override, Scope},
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
#[utoipa::path(
    get, path = "/items/{id}/overrides", tag = TAG,
    params(("id" = String, Path, description = "The work's identifier")),
    responses((status = 200, body = Vec<Override>)),
)]
async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<Override>>> {
    Ok(Json(repo::override_field::list(&state.db, &id).await?))
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
    Json(request): Json<SetRequest>,
) -> AppResult<Json<SetResponse>> {
    require_write(&identity)?;

    let scope: Scope = request
        .scope
        .parse()
        .map_err(|e: anyhow::Error| AppError::BadRequest(e.to_string()))?;

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

    repo::override_field::set(
        &state.db,
        &id,
        scope,
        &request.field,
        request.value.as_ref(),
        Some(&identity.label()),
    )
    .await?;

    state.caches.items.invalidate(&format!("item:{id}")).await;
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

    if !repo::override_field::unset(&state.db, &id, scope, &field).await? {
        return Err(AppError::NotFound);
    }

    state.caches.items.invalidate(&format!("item:{id}")).await;
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

    let removed = repo::override_field::clear(&state.db, &id).await?;

    state.caches.items.invalidate(&format!("item:{id}")).await;
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

fn require_write(identity: &Identity) -> AppResult<()> {
    identity
        .can_write()
        .then_some(())
        .ok_or(AppError::Forbidden)
}
