//! Reading and changing what this server does.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    auth::Identity,
    db::repo::audit::Action,
    error::{AppError, AppResult},
    settings::{Definition, Effective, REGISTRY, Scope},
    state::AppState,
};

pub const TAG: &str = "Settings";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(catalogue))
        .routes(routes!(effective))
        .routes(routes!(put, clear))
}

/// Every setting this server has, and what each one may hold.
///
/// The interface renders from this rather than from a list of its own, so a
/// setting added to the server appears without the interface being changed.
#[utoipa::path(
    get, path = "/settings/registry", tag = TAG,
    responses((status = 200, body = Vec<Definition>)),
)]
async fn catalogue(
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<&'static [Definition]>> {
    require_admin(&identity)?;

    Ok(Json(REGISTRY))
}

/// What applies at a scope, and whether that scope sets it or inherits it.
#[utoipa::path(
    get, path = "/settings/{scope}/{id}", tag = TAG,
    params(
        ("scope" = String, Path, description = "server, client or peer"),
        ("id" = String, Path, description = "The key's or rule's id; `-` for the server"),
    ),
    responses(
        (status = 200, body = Vec<Effective>),
        (status = 400, description = "No such scope"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn effective(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((scope, id)): Path<(String, String)>,
) -> AppResult<Json<Vec<Effective>>> {
    require_admin(&identity)?;

    let (scope, id) = address(&scope, &id)?;

    Ok(Json(state.settings.effective(scope, &id)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingWrite {
    pub key: String,
    /// The value, as text. `null` stops overriding at this scope.
    pub value: Option<String>,
}

/// Set one setting at one scope.
#[utoipa::path(
    put, path = "/settings/{scope}/{id}", tag = TAG,
    params(
        ("scope" = String, Path, description = "server, client or peer"),
        ("id" = String, Path, description = "The key's or rule's id; `-` for the server"),
    ),
    request_body = SettingWrite,
    responses(
        (status = 200, body = Vec<Effective>),
        (status = 400, description = "No such setting, or a value it cannot hold"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn put(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Path((scope, id)): Path<(String, String)>,
    Json(write): Json<SettingWrite>,
) -> AppResult<Json<Vec<Effective>>> {
    require_admin(&identity)?;

    let (scope, id) = address(&scope, &id)?;

    // The identity provider and the password switch are written as a whole,
    // where they are checked together — a password switch stored here would
    // wait, unchecked, for the provider to become ready and lock the door.
    if write.key.starts_with("oidc.") || write.key.starts_with("auth.") {
        return Err(AppError::BadRequest(format!(
            "{} is set with the identity provider, on the Opening & APIs page",
            write.key
        )));
    }

    match write.value {
        Some(value) => state
            .settings
            .set(scope, &id, &write.key, &value, Some(&identity.label()))
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?,
        None => {
            state
                .settings
                .clear(scope, &id, &write.key)
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
        }
    }

    // A cache switch is a setting of its own: flipped here and felt nowhere
    // else, not a reason to move every provider on and empty every space.
    if write.key.starts_with("cache.") {
        state.caches.sync_switches(|key| state.flag(key, true));
        // The other instances set their switches from the table too.
        state.coord.tell(crate::coord::Message::Settings);
    } else {
        state.sync_providers().await;
    }

    // A secret's value never reaches the journal: only that it moved.
    let now_set = state.settings.at(scope, &id, &write.key);
    let detail = match crate::settings::registry::find(&write.key) {
        Some(def) if def.kind.is_secret() => {
            Some(if now_set.is_some() { "set" } else { "cleared" })
        }
        _ => now_set.as_deref().or(Some("inherited")),
    };

    audit::record(
        &state,
        Event {
            identity: Some(&identity),
            ip: &ip,
            action: Action::SettingChanged,
            target: Some(&format!("{}:{}", scope.as_str(), write.key)),
            detail,
        },
    )
    .await;

    Ok(Json(state.settings.effective(scope, &id)))
}

/// Stop overriding everything at a scope, so it inherits again.
#[utoipa::path(
    delete, path = "/settings/{scope}/{id}", tag = TAG,
    params(
        ("scope" = String, Path, description = "client or peer"),
        ("id" = String, Path, description = "The key's or rule's id"),
    ),
    responses(
        (status = 204, description = "Cleared"),
        (status = 400, description = "The server scope cannot inherit from anything"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn clear(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((scope, id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    require_admin(&identity)?;

    let (scope, id) = address(&scope, &id)?;

    if scope == Scope::Server {
        return Err(AppError::BadRequest(
            "the server scope has nothing to inherit from".into(),
        ));
    }

    state.settings.forget(scope, &id).await?;
    state.sync_providers().await;

    Ok(StatusCode::NO_CONTENT)
}

/// A scope and the thing it applies to, from the path.
///
/// The server scope has no id; `-` stands in for one so the route shape stays
/// the same for all three and the interface does not need two call sites.
fn address(scope: &str, id: &str) -> AppResult<(Scope, String)> {
    let scope = Scope::parse(scope)
        .ok_or_else(|| AppError::BadRequest(format!("no scope called {scope:?}")))?;

    let id = match scope {
        Scope::Server => String::new(),
        _ if id == "-" || id.is_empty() => {
            return Err(AppError::BadRequest(
                "that scope needs the id of a key or a rule".into(),
            ));
        }
        _ => id.to_string(),
    };

    Ok((scope, id))
}

fn require_admin(identity: &Identity) -> AppResult<()> {
    identity.is_admin().then_some(()).ok_or(AppError::Forbidden)
}
