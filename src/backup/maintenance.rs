//! Maintenance: while an archive is restored, every door of this instance
//! answers 503 — the interface, its API, Sonarr's and Radarr's surfaces, the
//! relays, the clients' door — and nothing reads or writes the database
//! halfway through being replaced.
//!
//! The liveness probe alone is still answered: a container whose health
//! check fails in the middle of a restore is one an orchestrator restarts,
//! and the restore with it. The readiness probe says it is not ready, which
//! is the truth. What the 503 says — a JSON document for a program, a page
//! that reloads itself for a browser — names the step the restore is at, so
//! the page that started it can follow it through a door that is shut.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Response},
};
use serde::Serialize;

use crate::state::AppState;

/// How long a client is told to wait before it asks again.
const RETRY_AFTER_SECONDS: u32 = 5;

/// What the server is busy with, when it is.
#[derive(Default)]
pub struct Maintenance {
    now: parking_lot::RwLock<Option<Under>>,
}

/// The operation under way, and how far it got.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Under {
    /// `restore`.
    pub operation: &'static str,
    /// `safety` (the copy made first), `reading` (the archive), `copying`
    /// (the rows), `media`, `reloading` (what is held in memory).
    pub phase: &'static str,
    /// The table being copied, while the rows are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    pub step: usize,
    pub steps: usize,
    pub since: String,
}

impl Maintenance {
    /// Shut the doors for `operation`. False when they are shut already:
    /// one operation at a time.
    pub fn begin(&self, operation: &'static str) -> bool {
        let mut now = self.now.write();
        if now.is_some() {
            return false;
        }
        *now = Some(Under {
            operation,
            phase: "safety",
            table: None,
            step: 0,
            steps: 0,
            since: crate::db::now(),
        });
        true
    }

    /// Say how far the operation got.
    pub fn phase(&self, phase: &'static str, table: Option<&str>, step: usize, steps: usize) {
        if let Some(under) = self.now.write().as_mut() {
            under.phase = phase;
            under.table = table.map(str::to_string);
            under.step = step;
            under.steps = steps;
        }
    }

    /// Open the doors again.
    pub fn end(&self) {
        *self.now.write() = None;
    }

    pub fn current(&self) -> Option<Under> {
        self.now.read().clone()
    }

    pub fn is_on(&self) -> bool {
        self.now.read().is_some()
    }
}

/// The doors shut while `state` is under maintenance, open again when this
/// is dropped — at the end of the operation however it ends, a panic
/// included.
pub struct Shut {
    state: AppState,
}

impl Shut {
    /// Shut them, unless another operation has them shut already.
    pub fn begin(state: &AppState, operation: &'static str) -> Option<Self> {
        state.maintenance.begin(operation).then(|| Self {
            state: state.clone(),
        })
    }

    pub fn phase(&self, phase: &'static str, table: Option<&str>, step: usize, steps: usize) {
        self.state.maintenance.phase(phase, table, step, steps);
    }
}

impl Drop for Shut {
    fn drop(&mut self) {
        self.state.maintenance.end();
    }
}

/// What every door runs first: the request goes on, unless the server is
/// under maintenance — and then only the liveness probe does.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(under) = state.maintenance.current() else {
        return next.run(request).await;
    };
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    let page = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    refusal(&under, page)
}

#[derive(Serialize)]
struct Body<'a> {
    error: &'static str,
    message: &'static str,
    maintenance: &'a Under,
}

const MESSAGE: &str = "the server is restoring a backup, and answers again once it is done";

/// The 503 a door answers while the server is under maintenance.
fn refusal(under: &Under, page: bool) -> Response {
    let mut response = if page {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(format!(
                "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
                 <meta http-equiv=\"refresh\" content=\"{RETRY_AFTER_SECONDS}\">\n\
                 <title>Restoring a backup</title>\n</head>\n<body>\n\
                 <h1>Restoring a backup</h1>\n<p>This server is restoring a backup, and \
                 answers again once it is done. This page asks again every few seconds.</p>\n\
                 </body>\n</html>\n"
            )),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(Body {
                error: "maintenance",
                message: MESSAGE,
                maintenance: under,
            }),
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(RETRY_AFTER_SECONDS));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body as Raw, http::Request as HttpRequest, routing::get};
    use tower::ServiceExt as _;

    async fn asked(
        router: &Router,
        path: &str,
        accept: &str,
    ) -> (StatusCode, String, Option<String>) {
        let response = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri(path)
                    .header(header::ACCEPT, accept)
                    .body(Raw::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let retry = response
            .headers()
            .get(header::RETRY_AFTER)
            .map(|v| v.to_str().unwrap().to_string());
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).to_string(), retry)
    }

    #[tokio::test]
    async fn under_maintenance_every_door_but_the_liveness_probe_answers_503() {
        let state = crate::api::native::testing::server().await;
        let router = Router::new()
            .route("/health", get(|| async { "alive" }))
            .route("/ready", get(|| async { "ready" }))
            .route("/api/v1/items", get(|| async { "items" }))
            .route("/v1/tvdb/shows/en/1", get(|| async { "a series" }))
            .layer(axum::middleware::from_fn_with_state(state.clone(), guard));

        // Open: everything answers.
        assert_eq!(
            asked(&router, "/api/v1/items", "*/*").await.0,
            StatusCode::OK
        );

        let shut = Shut::begin(&state, "restore").expect("shut");
        assert!(
            Shut::begin(&state, "restore").is_none(),
            "one operation at a time"
        );
        shut.phase("copying", Some("media_episode"), 6, 40);

        let (status, body, _) = asked(&router, "/health", "*/*").await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "alive"));

        for path in ["/ready", "/api/v1/items", "/v1/tvdb/shows/en/1"] {
            let (status, body, retry) = asked(&router, path, "application/json").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
            assert_eq!(retry.as_deref(), Some("5"), "{path}");
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["error"], "maintenance");
            assert_eq!(body["maintenance"]["phase"], "copying");
            assert_eq!(body["maintenance"]["table"], "media_episode");
            assert_eq!(body["maintenance"]["step"], 6);
        }

        // A browser is given a page that asks again on its own.
        let (status, body, _) = asked(&router, "/admin", "text/html,*/*").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("http-equiv=\"refresh\""), "{body}");

        // Over, however it ended: open again.
        drop(shut);
        assert!(!state.maintenance.is_on());
        assert_eq!(
            asked(&router, "/api/v1/items", "*/*").await.0,
            StatusCode::OK
        );
    }
}
