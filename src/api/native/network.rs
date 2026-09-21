//! Who may call the address-guarded surfaces, and who has been trying.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::{
        self,
        audit::Action,
        network::{Caller, NetworkRule},
    },
    error::{AppError, AppResult},
    state::AppState,
};

pub const TAG: &str = "Network";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(rules, add))
        .routes(routes!(remove, rename))
        .routes(routes!(callers))
}

/// Every address or block allowed to call Sonarr's and Radarr's surfaces.
///
/// `AMS_ALLOWLIST` seeds this the first time the server starts against an empty
/// table and is ignored afterwards — the list is editable here, like a key.
#[utoipa::path(
    get, path = "/network/rules", tag = TAG,
    responses(
        (status = 200, body = Vec<NetworkRule>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn rules(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<Vec<NetworkRule>>> {
    require_admin(&identity)?;

    Ok(Json(repo::network::list_rules(&state.db).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewRule {
    /// An address (`172.31.0.7`) or a block (`172.31.0.0/24`).
    pub cidr: String,
    /// What to call the client behind it — `sonarr`, `radarr`. Settings hang
    /// off this rule, so naming it is what makes per-client settings possible.
    pub name: Option<String>,
    pub note: Option<String>,
}

/// Allow an address or a block.
#[utoipa::path(
    post, path = "/network/rules", tag = TAG,
    request_body = NewRule,
    responses(
        (status = 201, body = NetworkRule),
        (status = 400, description = "Not an address or a CIDR block"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "Already allowed"),
    ),
)]
async fn add(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(request): Json<NewRule>,
) -> AppResult<(StatusCode, Json<NetworkRule>)> {
    require_admin(&identity)?;

    let parsed = repo::network::parse_rule(&request.cidr)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // Worth saying out loud rather than refusing: an operator behind a private
    // network may mean it, and this server cannot know what is in front of it.
    if parsed.prefix_len() == 0 {
        tracing::warn!(
            cidr = %request.cidr,
            actor = %identity.label(),
            "a network rule was added that allows every address"
        );
    }

    let existing = repo::network::list_rules(&state.db).await?;
    if existing.iter().any(|rule| rule.cidr == request.cidr.trim()) {
        return Err(AppError::Conflict("that address is already allowed".into()));
    }

    let rule = repo::network::add_rule(
        &state.db,
        &request.cidr,
        request.name.as_deref(),
        request.note.as_deref(),
        Some(&identity.label()),
    )
    .await?;

    // The guard reads a cached copy, so it has to be told.
    state.reload_allowlist().await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::NetworkRuleAdded,
            target: Some(&rule.cidr),
            detail: rule.note.as_deref(),
        },
    )
    .await;

    Ok((StatusCode::CREATED, Json(rule)))
}

/// Stop allowing an address or a block.
#[utoipa::path(
    delete, path = "/network/rules/{id}", tag = TAG,
    params(("id" = String, Path, description = "The rule's identifier")),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such rule"),
    ),
)]
async fn remove(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_admin(&identity)?;

    let removed = repo::network::list_rules(&state.db)
        .await?
        .into_iter()
        .find(|rule| rule.id == id)
        .ok_or(AppError::NotFound)?;

    if !repo::network::remove_rule(&state.db, &id).await? {
        return Err(AppError::NotFound);
    }

    // Its settings would otherwise outlive it and attach to whatever rule
    // happened to be created next.
    state
        .forget_settings(crate::settings::Scope::Peer, &id)
        .await?;
    state.reload_allowlist().await?;

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::NetworkRuleRemoved,
            target: Some(&removed.cidr),
            detail: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Rename {
    /// What to call the client behind this address. Empty clears it.
    pub name: Option<String>,
}

/// Name the client an address stands for.
///
/// Naming is what makes per-client settings possible: it is the only stable
/// handle on a client that presents no credential, since the address it calls
/// from changes whenever its container restarts.
#[utoipa::path(
    patch, path = "/network/rules/{id}", tag = TAG,
    params(("id" = String, Path, description = "The rule's identifier")),
    request_body = Rename,
    responses(
        (status = 200, body = NetworkRule),
        (status = 403, description = "The caller is not an administrator"),
        (status = 404, description = "No such rule"),
    ),
)]
async fn rename(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(request): Json<Rename>,
) -> AppResult<Json<NetworkRule>> {
    require_admin(&identity)?;

    if !repo::network::rename(&state.db, &id, request.name.as_deref()).await? {
        return Err(AppError::NotFound);
    }

    repo::network::list_rules(&state.db)
        .await?
        .into_iter()
        .find(|rule| rule.id == id)
        .map(Json)
        .ok_or(AppError::NotFound)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct CallerQuery {
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
}

/// Everyone who has called a guarded surface, allowed or refused.
///
/// The refusals are the point: a client that cannot reach this server leaves no
/// other trace, and its address — with whatever name the resolver and the hosts
/// file give it — is what an operator needs to let it in.
#[utoipa::path(
    get, path = "/network/callers", tag = TAG,
    params(CallerQuery),
    responses(
        (status = 200, body = Vec<Caller>),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn callers(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<CallerQuery>,
) -> AppResult<Json<Vec<Caller>>> {
    require_admin(&identity)?;

    Ok(Json(
        repo::network::callers(&state.db, query.limit.unwrap_or(100)).await?,
    ))
}

fn require_admin(identity: &Identity) -> AppResult<()> {
    identity.is_admin().then_some(()).ok_or(AppError::Forbidden)
}
