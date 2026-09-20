//! Recording and reading the audit trail.

use axum::{
    Extension, Json,
    extract::{Query as AxumQuery, State},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::extract::ClientIp,
    auth::Identity,
    db::repo::{
        self,
        audit::{Action, Entry},
    },
    error::{AppError, AppResult},
    state::AppState,
};

/// One thing that happened, ready to record.
pub struct Event<'a> {
    pub identity: Option<&'a Identity>,
    pub ip: &'a ClientIp,
    pub action: Action,
    pub target: Option<&'a str>,
    pub detail: Option<&'a str>,
}

/// Record an event, best effort.
///
/// A failure here is logged and swallowed: the action already happened, and
/// refusing it after the fact because the trail could not be written would be
/// worse than an incomplete trail.
pub async fn record(state: &AppState, event: Event<'_>) {
    let actor = event.identity.map(Identity::label);
    let ip = event.ip.as_text();

    let result = repo::audit::record(
        &state.db,
        repo::audit::Record {
            actor: actor.as_deref(),
            action: event.action,
            target: event.target,
            detail: event.detail,
            ip: ip.as_deref(),
        },
    )
    .await;

    if let Err(e) = result {
        tracing::warn!(action = %event.action, error = %e, "could not write the audit entry");
    }
}

// ─── endpoint ────────────────────────────────────────────────────────────────

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "Audit";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub since: Option<String>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub offset: Option<i64>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Named explicitly: several modules declare a type with this name, and
// utoipa keys schemas on the leaf name alone — a collision silently
// drops one of them from the spec.
#[schema(as = AuditListResponse)]
pub struct ListResponse {
    pub entries: Vec<Entry>,
    pub total: i64,
    /// Every action name, so a UI can offer them as filters without hard-coding.
    pub actions: Vec<&'static str>,
}

/// Read the audit trail, newest first.
///
/// Administrative: it names who did what, so it requires an administrator or a
/// key carrying the `admin` scope.
#[utoipa::path(
    get, path = "/audit", tag = TAG,
    params(ListQuery),
    responses(
        (status = 200, body = ListResponse),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    AxumQuery(query): AxumQuery<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    if !identity.is_admin() {
        return Err(AppError::Forbidden);
    }

    let entries = repo::audit::list(
        &state.db,
        &repo::audit::Query {
            action: query.action,
            actor: query.actor,
            target: query.target,
            since: query.since,
            limit: query.limit.unwrap_or(100),
            offset: query.offset.unwrap_or(0),
        },
    )
    .await?;

    Ok(Json(ListResponse {
        entries,
        total: repo::audit::count(&state.db).await?,
        actions: ACTIONS.iter().map(|a| a.as_str()).collect(),
    }))
}

const ACTIONS: &[Action] = &[
    Action::ItemCreated,
    Action::ItemUpdated,
    Action::ItemDeleted,
    Action::ItemRefreshed,
    Action::OverrideSet,
    Action::OverrideRemoved,
    Action::OverridesCleared,
    Action::ClientCreated,
    Action::ClientUpdated,
    Action::ClientRevoked,
    Action::SignedIn,
    Action::SignInFailed,
    Action::SignedOut,
    Action::PasswordChanged,
    Action::CacheCleared,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_distinct_name() {
        let mut names: Vec<&str> = ACTIONS.iter().map(|a| a.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();

        assert_eq!(names.len(), count, "two actions share a name");
    }

    #[test]
    fn action_names_are_namespaced() {
        // A log shipper filters on the prefix, so every name must carry one.
        for action in ACTIONS {
            assert!(
                action.as_str().contains('.'),
                "{} is not namespaced",
                action.as_str()
            );
        }
    }
}
