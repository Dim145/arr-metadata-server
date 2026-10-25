//! The application log, for whoever maintains the server: the last lines it
//! wrote, read without `docker logs`, and downloaded as a file.

use axum::{
    Extension, Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    auth::Identity,
    error::AppResult,
    state::AppState,
    telemetry::journal::{self, Entry, Level},
};

const TAG: &str = super::meta::TAG;

/// How many lines a page holds when the caller says nothing.
const PAGE: usize = 500;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

/// The forms the log is downloaded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = LogFormat)]
pub enum Format {
    /// The lines as the text log writes them, oldest first.
    Text,
    /// One JSON object a line, oldest first.
    Json,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogQuery {
    /// This level and every graver one: `warn` is the warnings and the
    /// errors. Every level when left out.
    #[param(inline)]
    pub level: Option<Level>,
    /// Words every line must hold, in its message, its fields, its module or
    /// its ids — a trace's id finds the lines of one request.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub q: Option<String>,
    /// Only the lines written after this one, by its `seq`: what a page that
    /// follows the log asks for.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub after: Option<u64>,
    /// At most this many, the newest: 500 unless said, and never more than
    /// the journal keeps.
    #[serde(default, deserialize_with = "crate::api::extract::empty_as_none")]
    pub limit: Option<usize>,
    /// `text` or `json`: every line the filters match, oldest first, as a
    /// file to download, `limit` set aside.
    #[param(inline)]
    pub format: Option<Format>,
}

/// The last lines the server wrote.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = LogResponse)]
pub struct LogResponse {
    /// The instance whose log this is: among several, each keeps its own.
    pub instance: String,
    /// How many lines are kept: the oldest goes as each new one comes.
    pub capacity: usize,
    /// How many are kept now.
    pub held: usize,
    /// Whether the traces are sent to a collector: the environment's
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` says.
    pub traces: bool,
    /// Whether the errors are reported to a Sentry: the settings' DSN says.
    pub reports: bool,
    /// Those that answer, newest first.
    pub entries: Vec<Entry>,
}

/// The application log: the last lines the server wrote to its standard
/// output — as many as `capacity` says, those `AMS_LOG` lets through — each
/// with its level, its module, its message, its fields and the ids of the
/// request it was written in, every secret in them masked. With `format`,
/// every line the filters match, as a file. For an administrator.
#[utoipa::path(
    get, path = "/admin/logs", tag = TAG,
    operation_id = "logs_list",
    params(LogQuery),
    responses(
        (status = 200, description = "The lines that answer, newest first; with `format`, as a file, oldest first", content(
            (LogResponse = "application/json"),
            (String = "text/plain"),
            (String = "application/x-ndjson"),
        )),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(asked): Query<LogQuery>,
) -> AppResult<Response> {
    identity.require_admin()?;

    let query = journal::Query {
        level: asked.level.unwrap_or(Level::Trace),
        text: asked.q,
        after: asked.after,
        limit: match asked.format {
            Some(_) => journal::CAPACITY,
            None => asked.limit.unwrap_or(PAGE).clamp(1, journal::CAPACITY),
        },
    };
    let entries = journal::read(&query);
    let instance = state.coord.instance.name.clone();

    if let Some(format) = asked.format {
        return Ok(download(&instance, entries, format));
    }
    Ok(Json(LogResponse {
        instance,
        capacity: journal::CAPACITY,
        held: journal::held(),
        traces: crate::telemetry::trace::is_on(),
        reports: crate::telemetry::report::is_on(),
        entries,
    })
    .into_response())
}

/// The lines as a file, oldest first, as a log reads.
fn download(instance: &str, mut entries: Vec<Entry>, format: Format) -> Response {
    entries.reverse();
    let (body, kind, extension) = match format {
        Format::Text => (
            entries
                .iter()
                .map(|entry| entry.as_text() + "\n")
                .collect::<String>(),
            "text/plain; charset=utf-8",
            "log",
        ),
        Format::Json => (
            entries
                .iter()
                .filter_map(|entry| serde_json::to_string(entry).ok())
                .map(|line| line + "\n")
                .collect::<String>(),
            "application/x-ndjson",
            "jsonl",
        ),
    };
    // A name of letters, digits, dashes and dots, whatever the instance is
    // called.
    let instance: String = instance
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = format!(
        "ams-log-{instance}-{}.{extension}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );

    let mut response = Body::from(body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    // What the server did is not for a cache to keep.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;
    use crate::{auth::testing::key, db::repo::user::Role, error::AppError};

    /// Three lines, each holding `marker`, written as the server writes its
    /// own: through the journal's layer. The journal is the process's, and
    /// the tests run at once: each finds its own lines by its marker.
    fn write_lines(marker: &str) {
        use tracing_subscriber::prelude::*;
        let subscriber = tracing_subscriber::registry().with(journal::layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("{marker} started");
            tracing::warn!(provider = "tmdb", "{marker} slow");
            tracing::error!(error = %"boom", "{marker} failed");
        });
    }

    async fn asked(state: &AppState, identity: Identity, query: &str) -> AppResult<Response> {
        let Query(query) =
            Query::<LogQuery>::try_from_uri(&format!("/admin/logs?{query}").parse().unwrap())
                .unwrap();
        list(State(state.clone()), Extension(identity), Query(query)).await
    }

    async fn body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn the_log_is_read_by_an_administrator_and_nobody_else() {
        let state = super::super::testing::server().await;
        write_lines("logs-api-reader");

        for identity in [key(&["admin"], None), Identity::Anonymous] {
            let response = asked(&state, identity, "q=logs-api-reader").await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let read: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
            assert_eq!(read["capacity"], journal::CAPACITY);
            assert_eq!(read["entries"].as_array().unwrap().len(), 3, "{read}");
            assert_eq!(read["entries"][0]["level"], "error");
            assert_eq!(read["entries"][0]["fields"][0]["name"], "error");
        }

        for (case, identity) in [
            key(&["read", "write"], Some(Role::Editor)),
            key(&["metrics"], None),
            Identity::Visitor,
            Identity::Network(None),
        ]
        .into_iter()
        .enumerate()
        {
            let refused = asked(&state, identity.clone(), "").await.err();
            assert!(
                matches!(refused, Some(AppError::Forbidden)),
                "case {case}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_log_is_filtered_by_level_and_downloaded_oldest_first() {
        let state = super::super::testing::server().await;
        write_lines("logs-api-levels");

        let response = asked(
            &state,
            key(&["admin"], None),
            "q=logs-api-levels&level=warn",
        )
        .await
        .unwrap();
        let read: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
        let levels: Vec<&str> = read["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["level"].as_str().unwrap())
            .collect();
        assert_eq!(levels, ["error", "warn"]);

        let response = asked(
            &state,
            key(&["admin"], None),
            "q=logs-api-levels&format=text",
        )
        .await
        .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        let disposition = response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            disposition.starts_with("attachment; filename=\"ams-log-"),
            "{disposition}"
        );
        assert!(disposition.ends_with(".log\""), "{disposition}");
        let text = body(response).await;
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].contains(" INFO ") && lines[0].ends_with("logs-api-levels started"));
        assert!(lines[2].contains("ERROR") && lines[2].contains("error=boom"));

        let response = asked(
            &state,
            key(&["admin"], None),
            "q=logs-api-levels&format=json",
        )
        .await
        .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/x-ndjson"
        );
        let text = body(response).await;
        let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["message"], "logs-api-levels started");
    }
}
