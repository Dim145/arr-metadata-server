//! Traces: the spans of a request — the request itself, and each call it
//! made to a provider — sent over OTLP to an OpenTelemetry collector, Tempo,
//! Jaeger or anything else that reads it.
//!
//! Off unless the environment names a collector, in the standard variables:
//! `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, or `OTEL_EXPORTER_OTLP_ENDPOINT` to
//! which `/v1/traces` is added. Nothing is built otherwise, nothing is sent,
//! and the request path pays for one atomic read. The rest is the standard's
//! too: `OTEL_SERVICE_NAME` (`arr-metadata-server` unless set),
//! `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_EXPORTER_OTLP_HEADERS` for a token,
//! `OTEL_EXPORTER_OTLP_TIMEOUT`, `OTEL_TRACES_SAMPLER` and
//! `OTEL_TRACES_SAMPLER_ARG` — every trace unless they say otherwise,
//! `parentbased_traceidratio` and `0.1` for one in ten — and the batch
//! processor's `OTEL_BSP_*`. The protocol is HTTP with protobuf, port 4318
//! by convention.
//!
//! A trace starts at the door: named by the request's own id, which is a
//! random trace id (see `MakeId`), or carried on from a trusted proxy's
//! `traceparent` (see `judge_request_id`). It goes on to the providers: each
//! call made to one inside a request is a client span, and carries the
//! trace to the provider in its own `traceparent`. Only spans are sent, with
//! the attributes this module gives them, every address masked: the lines
//! written meanwhile stay in the log, which names the trace on each, and do
//! not travel with it.

use std::{
    cell::Cell,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    extract::{MatchedPath, Request},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use opentelemetry::{
    KeyValue,
    propagation::{Extractor, Injector, TextMapPropagator as _},
    trace::{SpanId, TraceContextExt as _, TraceId, TracerProvider as _},
};
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{IdGenerator, RandomIdGenerator, SdkTracerProvider},
};
use tower_http::request_id::RequestId;
use tracing::{Instrument as _, Span, Subscriber, level_filters::LevelFilter};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{
    filter::filter_fn, layer::Layer as SubscriberLayer, registry::LookupSpan,
};

use super::Note;

/// The target of the spans made for the trace alone: the request served,
/// the calls made. The log does not write them — its filters stop at
/// `info`, and these are at `trace` — and the trace sees nothing else.
pub const TARGET: &str = module_path!();

/// The name the service goes by when the environment gives it none.
const SERVICE: &str = "arr-metadata-server";

/// Whether the traces are on: set once, at the start.
static ON: AtomicBool = AtomicBool::new(false);

/// What sends the traces, for the stop to flush.
static PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// Whether the traces are on.
pub fn is_on() -> bool {
    ON.load(Ordering::Relaxed) || on_here()
}

/// Whether a test turned the traces on for its own thread.
#[cfg(test)]
fn on_here() -> bool {
    tests::on_here()
}

#[cfg(not(test))]
fn on_here() -> bool {
    false
}

// ── Setting up ──────────────────────────────────────────────────────────

/// The layer that turns the trace's spans into OpenTelemetry's, when the
/// environment names a collector; and what to say of it once the log is
/// there to say it.
pub(super) fn layer<S>() -> (Option<impl SubscriberLayer<S>>, Vec<Note>)
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let mut notes = Vec::new();
    let Some(endpoint) = wanted(&mut notes) else {
        return (None, notes);
    };

    use opentelemetry_otlp::WithExportConfig as _;

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .build()
    {
        Ok(exporter) => exporter,
        Err(e) => {
            notes.push(Note {
                warning: true,
                text: format!(
                    "traces are off: the exporter for {} could not be made: {}",
                    super::redact(&endpoint),
                    super::redact(&e.to_string())
                ),
            });
            return (None, notes);
        }
    };

    let mut resource = Resource::builder();
    if !named_by_environment() {
        resource = resource.with_service_name(SERVICE);
    }
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_id_generator(Chosen::default())
        .with_resource(
            resource
                .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
                .build(),
        )
        .build();
    let tracer = provider.tracer(SERVICE);
    let _ = PROVIDER.set(provider);
    ON.store(true, Ordering::Relaxed);
    notes.push(Note {
        warning: false,
        text: format!("traces are sent to {} over OTLP", super::redact(&endpoint)),
    });
    (Some(spans(tracer)), notes)
}

/// The layer that hands the trace's spans to `tracer`, and nothing else: no
/// line of the log travels with the trace.
fn spans<S>(tracer: opentelemetry_sdk::trace::SdkTracer) -> impl SubscriberLayer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_location(false)
        .with_threads(false)
        .with_target(false)
        .with_tracked_inactivity(false)
        .with_filter(
            filter_fn(|meta| meta.is_span() && meta.target() == TARGET)
                .with_max_level_hint(LevelFilter::TRACE),
        )
}

/// Where the traces are to go, when the environment says they are to go
/// anywhere: the collector's address as given.
fn wanted(notes: &mut Vec<Note>) -> Option<String> {
    let var = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let endpoint =
        var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").or_else(|| var("OTEL_EXPORTER_OTLP_ENDPOINT"))?;
    if var("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        notes.push(Note {
            warning: false,
            text: "traces are off: OTEL_SDK_DISABLED says so".into(),
        });
        return None;
    }
    match var("OTEL_TRACES_EXPORTER").as_deref() {
        None | Some("otlp") => {}
        Some("none") => return None,
        Some(other) => {
            notes.push(Note {
                warning: true,
                text: format!(
                    "traces are off: OTEL_TRACES_EXPORTER asks for {other:?}, and this server \
                     sends OTLP alone"
                ),
            });
            return None;
        }
    }
    if let Some(protocol) = var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL")
        .or_else(|| var("OTEL_EXPORTER_OTLP_PROTOCOL"))
        .filter(|p| p != "http/protobuf")
    {
        notes.push(Note {
            warning: true,
            text: format!(
                "OTEL_EXPORTER_OTLP_PROTOCOL asks for {protocol:?}: traces are sent as \
                 http/protobuf, the one protocol this server speaks — to port 4318 by \
                 convention, not gRPC's 4317"
            ),
        });
    }
    Some(endpoint)
}

/// Whether the environment names the service itself.
fn named_by_environment() -> bool {
    std::env::var("OTEL_SERVICE_NAME").is_ok_and(|v| !v.trim().is_empty())
        || std::env::var("OTEL_RESOURCE_ATTRIBUTES").is_ok_and(|attributes| {
            attributes
                .split(',')
                .any(|pair| pair.trim_start().starts_with("service.name="))
        })
}

/// Send what is still held, given five seconds: off the runtime, as the
/// exporter's client blocks.
pub(super) async fn shutdown() {
    let Some(provider) = PROVIDER.get().cloned() else {
        return;
    };
    let flushed = tokio::task::spawn_blocking(move || provider.shutdown());
    match tokio::time::timeout(Duration::from_secs(5), flushed).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => tracing::warn!(error = %e, "the last traces could not be sent"),
        Ok(Err(e)) => tracing::warn!(error = %e, "the last traces could not be sent"),
        Err(_) => tracing::warn!("the last traces were not sent in time"),
    }
}

// ── Ids ─────────────────────────────────────────────────────────────────

thread_local! {
    /// The id the next trace started on this thread takes: the request's.
    static CHOSEN: Cell<Option<TraceId>> = const { Cell::new(None) };
}

/// Random ids, as the SDK's own, but for a trace a request starts, which is
/// named by the request's id: set just before its span starts, on the
/// thread it starts on, and taken by the first trace that asks.
#[derive(Debug, Default)]
struct Chosen(RandomIdGenerator);

impl IdGenerator for Chosen {
    fn new_trace_id(&self) -> TraceId {
        CHOSEN
            .with(Cell::take)
            .unwrap_or_else(|| self.0.new_trace_id())
    }

    fn new_span_id(&self) -> SpanId {
        self.0.new_span_id()
    }
}

/// A request's id as a trace's: 32 hexadecimal digits that are not all
/// zeroes — what this server makes, and nginx's `$request_id`.
fn as_trace_id(request_id: &str) -> Option<TraceId> {
    (request_id.len() == 32 && request_id.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| TraceId::from_hex(request_id).ok())
        .flatten()
        .filter(|id| *id != TraceId::INVALID)
}

// ── The door ────────────────────────────────────────────────────────────

/// The span the trace serves a request in, kept on the request for the
/// request span to be made under and the route to be named on.
#[derive(Clone)]
pub struct Served(Span);

impl Served {
    /// Its id, for the request span's parent.
    pub fn id(&self) -> Option<tracing::Id> {
        self.0.id()
    }
}

/// Serve a request in a span of the trace — a server span, named by the
/// request's id or carried on from a trusted proxy's `traceparent` — and
/// tell it how the request was answered. Applied just outside the request
/// span's layer, inside the one that gives the request its id; with traces
/// off, it hands the request on.
pub async fn serve(mut request: Request, next: Next) -> Response {
    if !is_on() {
        return next.run(request).await;
    }
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .and_then(|id| id.header_value().to_str().ok())
        .unwrap_or_default()
        .to_string();
    let path = request.uri().path();
    let surface = crate::metrics::SURFACES[crate::metrics::surface_of(path)];
    let span = tracing::trace_span!(
        target: TARGET,
        parent: None,
        "server",
        otel.name = %request.method(),
        otel.kind = "server",
        otel.status_code = tracing::field::Empty,
        http.request.method = %request.method(),
        http.route = tracing::field::Empty,
        http.response.status_code = tracing::field::Empty,
        url.path = %super::redact(&super::redact_path(path)),
        url.query = tracing::field::Empty,
        network.protocol.version = %protocol(request.version()),
        user_agent.original = tracing::field::Empty,
        ams.surface = surface,
        ams.request_id = %request_id,
    );
    if span.is_disabled() {
        return next.run(request).await;
    }
    if let Some(query) = request.uri().query() {
        span.record("url.query", super::redact(query).as_str());
    }
    if let Some(agent) = request
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
    {
        span.record("user_agent.original", super::redact(agent).as_str());
    }

    // The trace a trusted proxy is in, when it sent one — the door took any
    // other off — or one named by the request's id.
    match parent_of(request.headers()) {
        Some(parent) => {
            let _ = span.set_parent(parent);
        }
        None => {
            if let Some(id) = as_trace_id(&request_id) {
                CHOSEN.with(|chosen| chosen.set(Some(id)));
            }
        }
    }
    // Started here, while the chosen id waits on this thread.
    let _ = span.context();
    CHOSEN.with(|chosen| chosen.set(None));

    request.extensions_mut().insert(Served(span.clone()));
    let response = next.run(request).instrument(span.clone()).await;
    answered(&span, response.status());
    response
}

/// The trace a request's lines belong to: the one it is served in, with
/// traces on; or, with them off, the one a trusted proxy said it is in.
pub fn trace_id(served: Option<&Served>, headers: &HeaderMap) -> Option<String> {
    match served {
        Some(Served(span)) => {
            let id = span.context().span().span_context().trace_id();
            (id != TraceId::INVALID).then(|| id.to_string())
        }
        None => super::traced_by(headers).map(str::to_string),
    }
}

/// Tell the server span how the request was answered: a 5xx is a failure,
/// as OpenTelemetry's conventions have it for a server.
fn answered(span: &Span, status: StatusCode) {
    span.record("http.response.status_code", status.as_u16());
    if status.is_server_error() {
        span.record("otel.status_code", "error");
    }
}

/// Name the server span by the route the request took — `GET
/// /v1/tvdb/shows/{language}/{id}` rather than `GET` — once the router has
/// found it. A route layer of each door's router.
pub async fn name_route(request: Request, next: Next) -> Response {
    if is_on()
        && let (Some(Served(span)), Some(route)) = (
            request.extensions().get::<Served>(),
            request.extensions().get::<MatchedPath>(),
        )
    {
        span.record("http.route", route.as_str());
        span.context()
            .span()
            .update_name(format!("{} {}", request.method(), route.as_str()));
    }
    next.run(request).await
}

/// The parent a trusted proxy's `traceparent` names, with its `tracestate`.
fn parent_of(headers: &HeaderMap) -> Option<opentelemetry::Context> {
    if !headers.contains_key(super::TRACEPARENT) {
        return None;
    }
    let parent = TraceContextPropagator::new().extract(&Headers(headers));
    parent.span().span_context().is_valid().then_some(parent)
}

fn protocol(version: axum::http::Version) -> &'static str {
    match version {
        axum::http::Version::HTTP_09 => "0.9",
        axum::http::Version::HTTP_10 => "1.0",
        axum::http::Version::HTTP_2 => "2",
        axum::http::Version::HTTP_3 => "3",
        _ => "1.1",
    }
}

/// A request's headers, as the propagator reads them.
struct Headers<'a>(&'a HeaderMap);

impl Extractor for Headers<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|name| name.as_str()).collect()
    }
}

// ── The calls made ──────────────────────────────────────────────────────

/// A call about to be made to `provider`, as a client span of the trace the
/// caller is in — none outside one: a call made by a background task, with
/// no request to belong to, is not a trace of its own.
pub fn client_span(provider: &str, request: &reqwest::Request) -> Option<Span> {
    if !is_on() || !opentelemetry::Context::current().has_active_span() {
        return None;
    }
    let url = request.url();
    let span = tracing::trace_span!(
        target: TARGET,
        "client",
        otel.name = %format!("{} {provider}", request.method()),
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        http.request.method = %request.method(),
        http.response.status_code = tracing::field::Empty,
        server.address = url.host_str().unwrap_or_default(),
        server.port = i64::from(url.port_or_known_default().unwrap_or_default()),
        url.full = %super::redact(url.as_str()),
        peer.service = provider,
        error.type = tracing::field::Empty,
    );
    (!span.is_disabled()).then_some(span)
}

/// Put the trace in a call's headers, its parent the call's own span.
pub fn inject(span: &Span, headers: &mut reqwest::header::HeaderMap) {
    TraceContextPropagator::new().inject_context(&span.context(), &mut HeadersOut(headers));
}

/// Tell a call's span how it ended: the status it was answered with, or why
/// it was not. A 404 is an answer, as the metrics count it; a 429 or a 5xx
/// is a failure.
pub fn ended(span: &Span, outcome: &reqwest::Result<reqwest::Response>) {
    match outcome {
        Ok(response) => {
            let status = response.status();
            span.record("http.response.status_code", status.as_u16());
            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                span.record("error.type", status.as_str());
                span.record("otel.status_code", "error");
            }
        }
        Err(e) => {
            let kind = if e.is_timeout() {
                "timeout"
            } else if e.is_connect() {
                "connect"
            } else {
                "request"
            };
            span.record("error.type", kind);
            span.record("otel.status_code", "error");
        }
    }
}

/// A call's headers, as the propagator writes them.
struct HeadersOut<'a>(&'a mut reqwest::header::HeaderMap);

impl Injector for HeadersOut<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(key.as_bytes()),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            self.0.insert(name, value);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::{
        net::SocketAddr,
        sync::{Arc, Mutex},
    };

    use axum::{
        Router,
        body::Body,
        extract::ConnectInfo,
        http::{Request as HttpRequest, header::HeaderName},
        routing::get,
    };
    use opentelemetry::trace::{SpanKind, Status};
    use opentelemetry_sdk::{
        error::OTelSdkResult,
        trace::{SpanData, SpanExporter},
    };
    use tower::ServiceExt as _;
    use tower_http::trace::TraceLayer;
    use tracing_subscriber::prelude::*;

    use super::*;

    thread_local! {
        /// Traces on for this thread alone: a test's, on a runtime of one
        /// thread, while the others run without.
        static ON_HERE: Cell<bool> = const { Cell::new(false) };
    }

    pub(in crate::telemetry) fn on_here() -> bool {
        ON_HERE.with(Cell::get)
    }

    /// The spans ended, as an exporter of them would be handed them.
    #[derive(Clone, Debug, Default)]
    struct Kept(Arc<Mutex<Vec<SpanData>>>);

    impl SpanExporter for Kept {
        fn export(
            &self,
            batch: Vec<SpanData>,
        ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
            self.0.lock().unwrap().extend(batch);
            std::future::ready(Ok(()))
        }
    }

    /// A provider that answers anything with 200, and tells what `traceparent`
    /// each request it was sent carried.
    async fn provider() -> (SocketAddr, Arc<Mutex<Vec<Option<String>>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().fallback({
            let seen = seen.clone();
            move |request: HttpRequest<Body>| {
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push(
                        request
                            .headers()
                            .get("traceparent")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string),
                    );
                    "answered"
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (address, seen)
    }

    /// The door as the server builds it, to a route that asks the provider.
    fn door(upstream: SocketAddr, trusted: Vec<ipnet::IpNet>) -> Router {
        let client = reqwest::Client::new();
        let router = Router::new()
            .route(
                "/v1/tvdb/shows/{language}/{id}",
                get(move || {
                    let client = client.clone();
                    async move {
                        let answer = crate::outbound::send(
                            "tvdb",
                            client.get(format!(
                                "http://{upstream}/series/81189?api_key=sekret123456"
                            )),
                        )
                        .await
                        .unwrap();
                        answer.text().await.unwrap()
                    }
                }),
            )
            .route_layer(axum::middleware::from_fn(name_route))
            .layer(TraceLayer::new_for_http().make_span_with(crate::telemetry::RequestSpan))
            .layer(axum::middleware::from_fn(serve));
        crate::telemetry::with_request_id(router, trusted)
    }

    fn asking(peer: &str, headers: &[(&str, &str)]) -> HttpRequest<Body> {
        let mut request = HttpRequest::get("/v1/tvdb/shows/en/81189")
            .header("user-agent", "Sonarr/4.0.15")
            .body(Body::empty())
            .unwrap();
        for (name, value) in headers {
            request.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        request
            .extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
        request
    }

    fn attribute<'a>(span: &'a SpanData, key: &str) -> Option<&'a opentelemetry::Value> {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| &kv.value)
    }

    /// A request is followed from the door to the provider: a server span
    /// named by the request's id and the route it took, and a client span
    /// under it, whose own id the provider is handed in `traceparent`. A
    /// trusted proxy's trace is the one joined, and a stranger's is not.
    #[tokio::test(flavor = "current_thread")]
    async fn a_request_is_followed_from_the_door_to_the_provider() {
        ON_HERE.with(|on| on.set(true));
        let kept = Kept::default();
        let provider_spans = SdkTracerProvider::builder()
            .with_simple_exporter(kept.clone())
            .with_id_generator(Chosen::default())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(super::super::journal::layer())
            .with(spans(provider_spans.tracer("test")));
        let _default = tracing::subscriber::set_default(subscriber);

        let (upstream, seen) = provider().await;
        let proxy: ipnet::IpNet = "10.0.0.0/8".parse().unwrap();
        let send = |request: HttpRequest<Body>| async {
            let response = door(upstream, vec![proxy]).oneshot(request).await.unwrap();
            let id = response.headers()["x-request-id"]
                .to_str()
                .unwrap()
                .to_string();
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(&body[..], b"answered");
            id
        };

        // Started here: the trace is named by the request's id.
        let id = send(asking("203.0.113.5:4000", &[])).await;
        let spans = std::mem::take(&mut *kept.0.lock().unwrap());
        assert_eq!(spans.len(), 2, "{spans:#?}");
        let server = spans
            .iter()
            .find(|s| s.span_kind == SpanKind::Server)
            .unwrap();
        let client = spans
            .iter()
            .find(|s| s.span_kind == SpanKind::Client)
            .unwrap();
        assert_eq!(server.span_context.trace_id().to_string(), id);
        assert_eq!(server.parent_span_id, SpanId::INVALID);
        assert_eq!(server.name, "GET /v1/tvdb/shows/{language}/{id}");
        assert_eq!(
            attribute(server, "http.route")
                .map(ToString::to_string)
                .as_deref(),
            Some("/v1/tvdb/shows/{language}/{id}")
        );
        assert_eq!(
            attribute(server, "http.response.status_code")
                .map(ToString::to_string)
                .as_deref(),
            Some("200")
        );
        assert_eq!(
            attribute(server, "ams.surface")
                .map(ToString::to_string)
                .as_deref(),
            Some("sonarr")
        );
        assert_eq!(
            attribute(server, "user_agent.original")
                .map(ToString::to_string)
                .as_deref(),
            Some("Sonarr/4.0.15")
        );
        assert_eq!(
            client.span_context.trace_id(),
            server.span_context.trace_id()
        );
        assert_eq!(client.parent_span_id, server.span_context.span_id());
        assert_eq!(client.name, "GET tvdb");
        assert_eq!(client.status, Status::Unset);
        let full = attribute(client, "url.full").unwrap().to_string();
        assert!(
            full.contains("api_key=***") && !full.contains("sekret123456"),
            "{full}"
        );
        assert_eq!(
            seen.lock().unwrap().pop().flatten(),
            Some(format!("00-{id}-{}-01", client.span_context.span_id()))
        );

        // A trusted proxy's trace, joined: its id names the request too.
        let parent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let id = send(asking("10.0.0.2:4000", &[("traceparent", parent)])).await;
        assert_eq!(id, "4bf92f3577b34da6a3ce929d0e0e4736");
        let spans = std::mem::take(&mut *kept.0.lock().unwrap());
        let server = spans
            .iter()
            .find(|s| s.span_kind == SpanKind::Server)
            .unwrap();
        assert_eq!(
            server.span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(server.parent_span_id.to_string(), "00f067aa0ba902b7");
        assert!(server.parent_span_is_remote);

        // A stranger's, not: the trace is this server's own.
        let id = send(asking("203.0.113.5:4000", &[("traceparent", parent)])).await;
        assert_ne!(id, "4bf92f3577b34da6a3ce929d0e0e4736");
        let spans = std::mem::take(&mut *kept.0.lock().unwrap());
        let server = spans
            .iter()
            .find(|s| s.span_kind == SpanKind::Server)
            .unwrap();
        assert_eq!(server.span_context.trace_id().to_string(), id);
        assert_eq!(server.parent_span_id, SpanId::INVALID);

        ON_HERE.with(|on| on.set(false));
    }

    /// A call made outside any request — a background task's — is sent as
    /// it is: no span, no `traceparent`.
    #[tokio::test(flavor = "current_thread")]
    async fn a_call_outside_a_request_starts_no_trace() {
        ON_HERE.with(|on| on.set(true));
        let kept = Kept::default();
        let provider_spans = SdkTracerProvider::builder()
            .with_simple_exporter(kept.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(spans(provider_spans.tracer("test")));
        let _default = tracing::subscriber::set_default(subscriber);

        let (upstream, seen) = provider().await;
        let answer = crate::outbound::send(
            "tvdb",
            reqwest::Client::new().get(format!("http://{upstream}/")),
        )
        .await
        .unwrap();
        assert_eq!(answer.text().await.unwrap(), "answered");
        assert_eq!(seen.lock().unwrap().pop(), Some(None));
        assert!(kept.0.lock().unwrap().is_empty());
        ON_HERE.with(|on| on.set(false));
    }

    #[test]
    fn only_an_id_of_a_trace_s_shape_names_a_trace() {
        assert!(as_trace_id("4bf92f3577b34da6a3ce929d0e0e4736").is_some());
        assert!(as_trace_id("00000000000000000000000000000000").is_none());
        assert!(as_trace_id("proxy-7").is_none());
        assert!(as_trace_id("abc").is_none());
    }
}
