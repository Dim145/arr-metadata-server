//! Error reports: each error the server writes, sent to a Sentry or a
//! GlitchTip — or anything else that reads Sentry's protocol — when the
//! settings name one.
//!
//! Off until `sentry.dsn` is set, and set from **Settings › Error reports**:
//! the DSN is a secret, written and never shown again; `sentry.environment`
//! and `sentry.sampleRate` say which environment the reports are filed under
//! and what share of them is sent. A change takes effect at once, on every
//! instance, without a restart ([`adopt`]).
//!
//! What is sent is the error line and nothing around it: its message, its
//! fields, the module that wrote it, the request it was written in — method
//! and masked address — and the ids that find the same request in the log
//! and in the traces, `trace_id` and `request_id`. No breadcrumb of the lines
//! before, no address of the caller, no request body. Every text is masked
//! before it leaves, as the log masks a secret ([`scrub`]).

use std::{borrow::Cow, sync::Arc, time::Duration};

use arc_swap::ArcSwapOption;
use parking_lot::Mutex;
use sentry::protocol::{Event as Report, Level as Severity, Map, Value};
use tracing::{Event, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::{
    filter::FilterExt as _,
    layer::{Context, Layer},
    registry::LookupSpan,
};

use super::journal::{Ids, Said};
use crate::settings::Scope;

/// What the environment is called when the settings name none.
pub const DEFAULT_ENVIRONMENT: &str = "production";

/// The share of errors sent when the settings say none, in percent.
pub const DEFAULT_SAMPLE_PERCENT: i64 = 100;

/// The client the reports are sent with, while a DSN is set.
static CLIENT: ArcSwapOption<sentry::Client> = ArcSwapOption::const_empty();

/// What that client was made from, so that a reload of the settings that
/// changed nothing of it leaves it be.
static MADE_FROM: Mutex<Option<Wanted>> = parking_lot::const_mutex(None);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Wanted {
    dsn: String,
    environment: String,
    sample_percent: i64,
    instance: String,
}

/// Whether reports are sent now.
pub fn is_on() -> bool {
    CLIENT.load().is_some()
}

/// Take the settings as they stand: make the client the DSN asks for, or
/// none, and let the one before go once it has sent what it held. Called as
/// every change of the settings is taken, here or on another instance.
pub fn adopt(settings: &crate::settings::Store, instance: &str) {
    let wanted = wanted(settings, instance);
    let mut made_from = MADE_FROM.lock();
    if *made_from == wanted {
        return;
    }
    let client = match wanted.as_ref().map(client_for) {
        None => None,
        Some(Ok(client)) => Some(Arc::new(client)),
        Some(Err(why)) => {
            tracing::warn!("error reports are off: {why}");
            None
        }
    };
    let on = client.is_some();
    let previous = CLIENT.swap(client);
    *made_from = wanted;
    drop(made_from);

    // Never the DSN itself, which holds the project's key.
    match (on, previous.is_some()) {
        (true, false) => tracing::info!("error reports are sent from now on"),
        (true, true) => tracing::info!("error reports are sent as the settings now say"),
        (false, true) => tracing::info!("error reports are no longer sent"),
        (false, false) => {}
    }
    if let Some(previous) = previous {
        // Given two seconds to send what it held, away from the caller.
        std::thread::spawn(move || {
            previous.close(Some(Duration::from_secs(2)));
        });
    }
}

/// What the settings ask of the reports: nothing without a DSN.
fn wanted(settings: &crate::settings::Store, instance: &str) -> Option<Wanted> {
    let read = |key: &str| {
        settings
            .at(Scope::Server, "", key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    read("sentry.dsn").map(|dsn| Wanted {
        dsn,
        environment: read("sentry.environment").unwrap_or_else(|| DEFAULT_ENVIRONMENT.into()),
        sample_percent: read("sentry.sampleRate")
            .and_then(|rate| rate.parse().ok())
            .unwrap_or(DEFAULT_SAMPLE_PERCENT)
            .clamp(0, 100),
        instance: instance.to_string(),
    })
}

/// The client a DSN asks for, or why it cannot be made.
fn client_for(wanted: &Wanted) -> Result<sentry::Client, String> {
    let dsn = wanted
        .dsn
        .parse::<sentry::types::Dsn>()
        .map_err(|_| "sentry.dsn is not a DSN Sentry reads".to_string())?;
    let mut options = sentry::ClientOptions::new()
        .release(concat!("arr-metadata-server@", env!("CARGO_PKG_VERSION")))
        .environment(wanted.environment.clone())
        .sample_rate(wanted.sample_percent as f32 / 100.0)
        .server_name(wanted.instance.clone())
        .max_breadcrumbs(0)
        .before_send(scrub);
    options.dsn = Some(dsn);
    options.send_default_pii = false;
    options.attach_stacktrace = false;
    options.default_integrations = false;
    Ok(sentry::Client::from_config(sentry::apply_defaults(options)))
}

/// Whether `value` is a DSN a client can be made from: what the settings
/// check before they keep one.
pub fn check_dsn(value: &str) -> Result<(), String> {
    value
        .trim()
        .parse::<sentry::types::Dsn>()
        .map(|_| ())
        .map_err(|_| {
            "is not a DSN: one reads https://<key>@<host>/<project>, as the project's \
             settings give it"
                .to_string()
        })
}

/// Send what is still held, given two seconds: the stop's last word.
pub(super) async fn shutdown() {
    let Some(client) = CLIENT.swap(None) else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || client.close(Some(Duration::from_secs(2)))).await;
}

/// The layer that reports the error lines: those at `error`, and the
/// request spans whose ids the reports carry.
pub(crate) fn layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    Reports::default().with_filter(LevelFilter::ERROR.or(super::request_spans()))
}

/// The layer: it sends with the client the settings made, or, in a test,
/// with one of its own.
#[derive(Default)]
struct Reports {
    fixed: Option<Arc<sentry::Client>>,
}

impl<S> Layer<S> for Reports
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(client) = self.fixed.clone().or_else(|| CLIENT.load_full()) else {
            return;
        };
        let meta = event.metadata();
        if *meta.level() != tracing::Level::ERROR || !reported(meta.target()) {
            return;
        }
        let mut said = Said::default();
        event.record(&mut said);
        let ids = ctx.event_scope(event).and_then(|scope| {
            scope
                .into_iter()
                .find_map(|span| span.extensions().get::<Ids>().cloned())
        });
        client.capture_event(report_of(meta.target(), said, ids.as_ref()), None);
    }
}

/// Whether the error lines of a target are reported. Not the client's own,
/// on its sending: reported, it would report itself. Not the traces' own
/// either — a collector out of reach says so at every batch, and the log is
/// where that is read. Nor `tower_http`'s "response failed", which follows
/// every 5xx the request's own error line has said why of: a second report
/// of one failure.
fn reported(target: &str) -> bool {
    !["sentry", "opentelemetry", "tower_http"]
        .iter()
        .any(|own| target.starts_with(own))
}

/// The report an error line makes.
fn report_of(target: &str, said: Said, ids: Option<&Ids>) -> Report<'static> {
    let mut tags = Map::new();
    let mut extra = Map::new();
    let mut message = said.message;
    for field in said.fields {
        // The error a line carries says what went wrong: in the message, so
        // that a timeout and a refusal are filed apart.
        if field.name == "error" && !field.value.is_empty() {
            message = format!("{message}: {}", field.value);
        }
        extra.insert(field.name, Value::String(field.value));
    }
    if let Some(ids) = ids {
        if let Some(trace) = ids.trace() {
            tags.insert("trace_id".into(), trace.to_string());
        }
        if let Some(request) = &ids.request_id {
            tags.insert("request_id".into(), request.clone());
        }
    }
    Report {
        level: Severity::Error,
        message: Some(message),
        logger: Some(target.to_string()),
        transaction: ids.and_then(Ids::request),
        platform: Cow::Borrowed("native"),
        tags,
        extra,
        ..Default::default()
    }
}

/// A report with every text in it masked as the log masks a secret, whatever
/// put it there: the last thing done before one leaves. One that cannot be
/// read through is not sent at all.
pub fn scrub(report: Report<'static>) -> Option<Report<'static>> {
    let mut tree = serde_json::to_value(&report).ok()?;
    mask(&mut tree);
    serde_json::from_value(tree).ok()
}

fn mask(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => {
            let masked = super::redact(text);
            if masked != *text {
                *text = masked;
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(mask),
        serde_json::Value::Object(fields) => fields.values_mut().for_each(mask),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dsn_is_checked_before_it_is_kept() {
        assert!(check_dsn("https://public@sentry.example.com/42").is_ok());
        assert!(check_dsn(" https://public@glitchtip.lan:8000/1 ").is_ok());
        for wrong in [
            "",
            "sentry.example.com",
            "https://sentry.example.com/42",
            "not a dsn",
        ] {
            assert!(check_dsn(wrong).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn nothing_secret_leaves_in_a_report() {
        crate::telemetry::redact_also("report-test-registered-secret");
        let said = Said {
            message: "request failed".into(),
            fields: vec![super::super::journal::LogField {
                name: "error".into(),
                value: "upstream: https://api.themoviedb.org/3/movie/1?api_key=abcdef123456".into(),
            }],
        };
        let mut report = report_of("arr_metadata_server::error", said, None);
        report.extra.insert(
            "note".into(),
            Value::String("report-test-registered-secret".into()),
        );
        report.tags.insert(
            "auth".into(),
            "Authorization: Bearer abcdefghijklmnop".into(),
        );
        let scrubbed = scrub(report).expect("a report that reads through");
        let text = serde_json::to_string(&scrubbed).unwrap();
        for planted in [
            "abcdef123456",
            "abcdefghijklmnop",
            "report-test-registered-secret",
        ] {
            assert!(!text.contains(planted), "{planted} in {text}");
        }
        assert_eq!(
            scrubbed.message.as_deref(),
            Some("request failed: upstream: https://api.themoviedb.org/3/movie/1?api_key=***")
        );
    }

    /// What a Sentry is sent: an error line written in a request reaches it,
    /// masked, with the request's ids; a line under `error` does not.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_error_line_reaches_the_sentry_masked() {
        use std::sync::Mutex as Shared;

        use axum::{Router, body::Bytes};
        use tracing_subscriber::prelude::*;

        let received = Arc::new(Shared::new(Vec::<String>::new()));
        let app = Router::new().fallback({
            let received = received.clone();
            move |body: Bytes| {
                let received = received.clone();
                async move {
                    received
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&body).into_owned());
                    "{}"
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = Arc::new(
            client_for(&Wanted {
                dsn: format!("http://publickey@{address}/7"),
                environment: "testing".into(),
                sample_percent: 100,
                instance: "instance-one".into(),
            })
            .expect("a client"),
        );
        crate::telemetry::redact_also("report-delivery-secret");
        let subscriber = tracing_subscriber::registry()
            .with(super::super::journal::layer())
            .with(Reports {
                fixed: Some(client.clone()),
            });
        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!(
                target: super::super::REQUEST_TARGET,
                "request",
                method = %"GET",
                uri = %"/v1/tvdb/shows/en/81189",
                request_id = %"0192f5a2c1d07a3b9e4f2c6d8a1b3c5e",
                trace_id = tracing::field::Empty,
            );
            request.in_scope(|| {
                tracing::warn!("only a warning");
                tracing::error!(
                    error = %"TMDB request failed: report-delivery-secret",
                    "request failed"
                );
            });
        });
        let flushed = tokio::task::spawn_blocking({
            let client = client.clone();
            move || client.flush(Some(Duration::from_secs(5)))
        })
        .await
        .unwrap();
        assert!(flushed);

        let received = received.lock().unwrap().clone();
        assert_eq!(received.len(), 1, "{received:?}");
        let envelope = &received[0];
        assert!(
            envelope.contains("request failed: TMDB request failed: ***"),
            "{envelope}"
        );
        assert!(!envelope.contains("report-delivery-secret"), "{envelope}");
        assert!(!envelope.contains("only a warning"), "{envelope}");
        assert!(
            envelope.contains("\"trace_id\":\"0192f5a2c1d07a3b9e4f2c6d8a1b3c5e\""),
            "{envelope}"
        );
        assert!(
            envelope.contains("\"environment\":\"testing\""),
            "{envelope}"
        );
        assert!(
            envelope.contains("GET /v1/tvdb/shows/en/81189"),
            "{envelope}"
        );
        assert!(envelope.contains("arr-metadata-server@"), "{envelope}");
    }

    /// The settings decide: nothing without a DSN, and what they say of the
    /// environment and the share when one is set.
    #[tokio::test]
    async fn the_settings_say_whether_and_how_errors_are_reported() {
        let state = crate::api::native::testing::server().await;
        assert_eq!(wanted(&state.settings, "one"), None);

        state
            .settings
            .set(
                Scope::Server,
                "",
                "sentry.dsn",
                "https://key@sentry.example.com/3",
                None,
            )
            .await
            .unwrap();
        let asked = wanted(&state.settings, "one").expect("reports asked for");
        assert_eq!(asked.environment, DEFAULT_ENVIRONMENT);
        assert_eq!(asked.sample_percent, DEFAULT_SAMPLE_PERCENT);
        assert!(client_for(&asked).is_ok());

        state
            .settings
            .set(Scope::Server, "", "sentry.sampleRate", "25", None)
            .await
            .unwrap();
        state
            .settings
            .set(Scope::Server, "", "sentry.environment", "staging", None)
            .await
            .unwrap();
        let asked = wanted(&state.settings, "one").unwrap();
        assert_eq!(
            (asked.environment.as_str(), asked.sample_percent),
            ("staging", 25)
        );

        state
            .settings
            .clear(Scope::Server, "", "sentry.dsn")
            .await
            .unwrap();
        assert_eq!(wanted(&state.settings, "one"), None);
    }

    #[test]
    fn a_failure_is_reported_once() {
        assert!(reported("arr_metadata_server::error"));
        assert!(reported("panic"));
        assert!(!reported("tower_http::trace::on_failure"));
        assert!(!reported("sentry::transports"));
        assert!(!reported("opentelemetry_sdk"));
    }

    #[test]
    fn a_report_names_the_request_and_its_trace() {
        let ids = Ids {
            request_id: Some("proxy-7".into()),
            trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".into()),
            ..Ids::default()
        };
        let report = report_of(
            "panic",
            Said {
                message: "a thread panicked".into(),
                fields: Vec::new(),
            },
            Some(&ids),
        );
        assert_eq!(report.tags["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(report.tags["request_id"], "proxy-7");
        assert_eq!(report.logger.as_deref(), Some("panic"));
        assert_eq!(report.level, Severity::Error);
    }
}
