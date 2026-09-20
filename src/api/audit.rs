//! Recording and reading the audit trail.

use axum::{
    Extension, Json, Router,
    extract::{Query as AxumQuery, State},
    routing::get,
};
use serde::{Deserialize, Serialize};

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

pub fn router() -> Router<AppState> {
    Router::new().route("/audit", get(list))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub since: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse {
    pub entries: Vec<Entry>,
    pub total: i64,
    /// Every action name, so a UI can offer them as filters without hard-coding.
    pub actions: Vec<&'static str>,
}

/// The trail is administrative: it names who did what, so it is admin-only.
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
