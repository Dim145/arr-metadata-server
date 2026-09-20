//! Manual edits and the locks they create.

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    auth::Identity,
    db::repo,
    domain::fields::{self, Override, Scope},
    error::{AppError, AppResult},
    service,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/items/{id}/overrides", get(list).put(set).delete(clear))
        .route("/items/{id}/overrides/{scope}/{field}", delete(unset))
}

async fn list(State(state): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Vec<Override>>> {
    Ok(Json(repo::override_field::list(&state.db, &id).await?))
}

#[derive(Deserialize)]
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetResponse {
    pub locked_fields: Vec<String>,
}

/// Record an edit. From this point the field is locked: every refresh leaves it
/// alone until the override is deleted.
async fn set(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
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
    let item = service::load(&state, &id).await?.ok_or(AppError::NotFound)?;

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

    tracing::info!(
        %id, %scope, field = %request.field, actor = %identity.label(),
        "locked a field with a manual edit"
    );

    let updated = service::load(&state, &id).await?.ok_or(AppError::NotFound)?;

    Ok(Json(SetResponse { locked_fields: updated.locked_fields }))
}

/// Unlock one field, handing it back to provider data on the next refresh.
async fn unset(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
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
    tracing::info!(%id, %scope, %field, actor = %identity.label(), "unlocked a field");

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearResponse {
    pub removed: u64,
}

/// Unlock every field of a work at once.
async fn clear(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<ClearResponse>> {
    require_write(&identity)?;

    let removed = repo::override_field::clear(&state.db, &id).await?;

    state.caches.items.invalidate(&format!("item:{id}")).await;
    tracing::info!(%id, removed, actor = %identity.label(), "unlocked every field");

    Ok(Json(ClearResponse { removed }))
}

fn require_write(identity: &Identity) -> AppResult<()> {
    identity.can_write().then_some(()).ok_or(AppError::Forbidden)
}
