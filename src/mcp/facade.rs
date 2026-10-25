//! The native API, as the tools read it.
//!
//! Each tool asks the API what the interface would — `GET /search`,
//! `GET /items/{id}`, `GET /calendar` — of its handlers, in this process, and
//! shapes their answer for an agent. What a reader may see, how a lock is laid
//! over, which works are kept from whom: decided once, by the handlers, for
//! the interface and the agents alike. Only `GET` is ever asked, of paths the
//! tools write themselves, so nothing an agent sends can reach a handler that
//! writes.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::Value;
use tower::ServiceExt as _;
use utoipa_axum::router::OpenApiRouter;

use crate::{auth::Identity, state::AppState};

/// The most of an answer read: a long-running series with every episode is
/// a megabyte or two of it, and nothing the API answers is ten times that.
const MOST: usize = 16 * 1024 * 1024;

/// Why the API did not answer what a tool asked, in words for the agent.
#[derive(Debug)]
pub(super) enum Refusal {
    /// Nothing there: no work, no list by that name — or nothing this key may
    /// see, which reads the same.
    NotFound(String),
    /// Asked in a way the API does not take: its own words for why.
    Asked(String),
    /// The server could not answer just now.
    Failed(String),
}

impl Refusal {
    /// What the agent is told.
    pub(super) fn text(&self) -> &str {
        match self {
            Self::NotFound(text) | Self::Asked(text) | Self::Failed(text) => text,
        }
    }
}

/// The native API's handlers, without the guard in front of them: the tools
/// say who asks.
#[derive(Clone)]
pub(super) struct Facade {
    router: Router,
}

impl Facade {
    pub(super) fn new(state: AppState) -> Self {
        let (router, _) = OpenApiRouter::new()
            .merge(crate::api::native::router())
            .split_for_parts();
        Self {
            router: router.with_state(state),
        }
    }

    /// The answer to `GET path?query`, asked as `reader`, as JSON.
    pub(super) async fn get(
        &self,
        reader: &Identity,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, Refusal> {
        let mut uri = path.to_string();
        let pairs = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(query.iter().map(|(name, value)| (*name, value.as_str())))
            .finish();
        if !pairs.is_empty() {
            uri.push('?');
            uri.push_str(&pairs);
        }

        let mut request = Request::get(&uri)
            .body(Body::empty())
            .map_err(|e| Refusal::Failed(format!("the request could not be made: {e}")))?;
        request.extensions_mut().insert(reader.clone());

        let response = match self.router.clone().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), MOST)
            .await
            .map_err(|_| Refusal::Failed("the answer was too long to read".into()))?;

        if status.is_success() {
            return serde_json::from_slice(&body)
                .map_err(|_| Refusal::Failed("the answer was not JSON".into()));
        }
        // The API's own words for a refusal, which name what to change.
        let said = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|error| error.get("message")?.as_str().map(str::to_string))
            .unwrap_or_else(|| status.to_string());
        Err(match status {
            StatusCode::NOT_FOUND => Refusal::NotFound(said),
            status if status.is_client_error() => Refusal::Asked(said),
            _ => Refusal::Failed(said),
        })
    }
}

/// A path segment as the API's router reads it: an id, a slug — written so
/// that nothing in it can reach another path.
pub(super) fn segment(text: &str) -> String {
    urlencoding::encode(text.trim()).into_owned()
}
