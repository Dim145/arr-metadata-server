//! API client management.

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    auth::{Identity, secrets},
    db::repo::{self, client::ApiClient},
    error::{AppError, AppResult},
    state::AppState,
};

/// Scopes a key may carry. `read` is implied by existing at all.
const KNOWN_SCOPES: &[&str] = &["read", "write", "admin"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/clients", get(list).post(create))
        .route("/clients/{id}", get(detail).patch(update).delete(remove))
}

async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<ApiClient>>> {
    require_admin(&identity)?;

    Ok(Json(repo::client::list(&state.db).await?))
}

async fn detail(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<ApiClient>> {
    require_admin(&identity)?;

    repo::client::get(&state.db, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub note: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResponse {
    #[serde(flatten)]
    pub client: ApiClient,
    /// Shown once. It is not recoverable afterwards — only its hash is stored.
    pub key: String,
}

async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<CreateRequest>,
) -> AppResult<(StatusCode, Json<CreateResponse>)> {
    require_admin(&identity)?;

    let name = request.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name must not be empty".into()));
    }

    for scope in &request.scopes {
        if !KNOWN_SCOPES.contains(&scope.as_str()) {
            return Err(AppError::BadRequest(format!(
                "unknown scope {scope:?}; expected one of {}",
                KNOWN_SCOPES.join(", ")
            )));
        }
    }

    let generated = secrets::generate_api_key()?;

    let client = repo::client::create(
        &state.db,
        repo::client::NewClient {
            name,
            key_prefix: &generated.prefix,
            key_hash: &generated.hash,
            scopes: &request.scopes,
            expires_at: request.expires_at.as_deref(),
            note: request.note.as_deref(),
        },
    )
    .await
    .map_err(|e| {
        // The unique index on name is the only constraint a caller can trip here.
        if e.to_string().contains("UNIQUE") || e.to_string().contains("duplicate key") {
            AppError::Conflict(format!("a client named {name:?} already exists"))
        } else {
            AppError::Internal(e)
        }
    })?;

    tracing::info!(client = %client.name, actor = %identity.label(), "issued an API key");

    Ok((
        StatusCode::CREATED,
        Json(CreateResponse {
            client,
            key: generated.plaintext,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequest {
    pub is_enabled: Option<bool>,
}

async fn update(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> AppResult<Json<ApiClient>> {
    require_admin(&identity)?;

    if let Some(enabled) = request.is_enabled
        && !repo::client::set_enabled(&state.db, &id, enabled).await?
    {
        return Err(AppError::NotFound);
    }

    repo::client::get(&state.db, &id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_admin(&identity)?;

    // Refuse to delete the key currently being used: it would lock the caller
    // out mid-session with no way back in.
    if let Identity::Client(current) = &identity
        && current.id == id
    {
        return Err(AppError::Conflict(
            "this is the key you are authenticated with; disable it from another session".into(),
        ));
    }

    if !repo::client::delete(&state.db, &id).await? {
        return Err(AppError::NotFound);
    }

    tracing::info!(%id, actor = %identity.label(), "revoked an API key");

    Ok(StatusCode::NO_CONTENT)
}

fn require_admin(identity: &Identity) -> AppResult<()> {
    identity.is_admin().then_some(()).ok_or(AppError::Forbidden)
}
