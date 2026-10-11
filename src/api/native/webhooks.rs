//! The webhook, from the administration: the events it can be told of, and
//! a test of its address.

use axum::{Extension, Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    db::repo::audit::Action,
    error::{AppError, AppResult},
    service::webhook::{self, Delivery, Fact},
    state::AppState,
};

const TAG: &str = super::meta::TAG;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(events))
        .routes(routes!(test))
}

/// Every name `webhooks.events` can hold, besides `*`.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEvents {
    /// What happens on its own: a refresh failed, a medium given up on, a
    /// newer release out.
    pub facts: Vec<&'static str>,
    /// The actions of the audit trail, in the order its filter offers them.
    pub actions: Vec<&'static str>,
    /// What is told while `webhooks.events` is not set at all.
    pub defaults: Vec<&'static str>,
}

/// The events a webhook can be told of, for the settings to offer.
#[utoipa::path(
    get, path = "/admin/webhooks/events", tag = TAG,
    responses(
        (status = 200, body = WebhookEvents),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn events(Extension(identity): Extension<Identity>) -> AppResult<Json<WebhookEvents>> {
    identity.require_admin()?;
    Ok(Json(WebhookEvents {
        facts: Fact::ALL.iter().map(|f| f.as_str()).collect(),
        actions: Action::ALL.iter().map(|a| a.as_str()).collect(),
        defaults: webhook::DEFAULT_EVENTS.split(',').collect(),
    }))
}

/// What a test of the address came to.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookTest {
    /// Whether the address took it: it answered, with a success.
    pub delivered: bool,
    /// What it answered with, when it answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Why it was not reached, in words that name no address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why nothing was sent: `not_web`, not an http(s) address or one that
    /// names this machine; `internal`, a name that resolves to an address
    /// only reachable from here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<&'static str>,
    /// How long it took, in milliseconds.
    pub millis: u64,
}

/// Post `webhook.test` to the address set, whatever `webhooks.events` says,
/// and say what came of it. Nothing is written in the audit trail, and no
/// other address is told.
#[utoipa::path(
    post, path = "/admin/webhooks/test", tag = TAG,
    responses(
        (status = 200, body = WebhookTest),
        (status = 403, description = "The caller is not an administrator"),
        (status = 409, description = "No webhook address is set"),
    ),
)]
async fn test(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<WebhookTest>> {
    identity.require_admin()?;
    let by = identity.label();
    let delivery = webhook::test(&state, &by)
        .await
        .ok_or_else(|| AppError::Conflict("no webhook address is set".into()))?;

    let millis = |took: std::time::Duration| u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
    let tried = match delivery {
        Delivery::Answered { status, took } => WebhookTest {
            delivered: status.is_success(),
            status: Some(status.as_u16()),
            error: None,
            refused: None,
            millis: millis(took),
        },
        Delivery::Unreached { why, took } => WebhookTest {
            delivered: false,
            status: None,
            error: Some(why),
            refused: None,
            millis: millis(took),
        },
        Delivery::Refused(why) => WebhookTest {
            delivered: false,
            status: None,
            error: None,
            refused: Some(why.code()),
            millis: 0,
        },
    };
    tracing::info!(
        by,
        delivered = tried.delivered,
        status = tried.status,
        "the webhook was sent a test"
    );
    Ok(Json(tried))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Authentication switched off: an administrator, as far as a guard asks.
    fn admin() -> Extension<Identity> {
        Extension(Identity::Anonymous)
    }

    #[tokio::test]
    async fn the_events_offered_are_the_facts_and_every_action() {
        let Json(offered) = events(admin()).await.expect("offered");
        assert_eq!(
            offered.facts,
            ["refresh.failed", "media.failed", "update.available"]
        );
        assert_eq!(offered.actions.len(), Action::ALL.len());
        assert_eq!(
            offered.defaults,
            [
                "item.imported",
                "item.created",
                "item.deleted",
                "dataset.imported"
            ]
        );
        assert!(matches!(
            events(Extension(Identity::Visitor)).await,
            Err(AppError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn a_test_needs_an_address_and_an_administrator() {
        let state = crate::api::native::testing::server().await;
        assert!(matches!(
            test(State(state.clone()), admin()).await,
            Err(AppError::Conflict(_))
        ));

        // An address this server never posts to is refused, and said so.
        state
            .settings
            .set(
                crate::settings::registry::Scope::Server,
                "",
                "webhooks.url",
                "http://127.0.0.1:9/hook",
                None,
            )
            .await
            .expect("set");
        let Json(tried) = test(State(state.clone()), admin()).await.expect("tried");
        assert!(!tried.delivered);
        assert_eq!(tried.refused, Some("not_web"));
        assert_eq!(tried.status, None);

        assert!(matches!(
            test(State(state), Extension(Identity::Visitor)).await,
            Err(AppError::Forbidden)
        ));
    }
}
