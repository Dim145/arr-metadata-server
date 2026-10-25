//! HTTP server: router assembly, middleware stack, TLS, graceful shutdown.

mod etag;
mod listen;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tower::Layer;
use utoipa_swagger_ui::SwaggerUi;

use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, cors::CorsLayer,
    normalize_path::NormalizePathLayer, set_header::SetResponseHeaderLayer, timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::{
    api, coord, jobs,
    jobs::supervise::{Ends, Supervisor},
    media,
    state::AppState,
    tls,
};

/// Largest request body accepted anywhere, but for an upload: the only
/// sizeable one otherwise is a bulk movie lookup, which is a list of
/// integers. Axum's own limit rather than a layer that cuts every body, so
/// the one route that takes a file can be given more.
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub async fn serve(state: AppState) -> Result<()> {
    let bind = state.config.server.bind;
    let limits = listen::Limits {
        connections: state.config.server.max_connections,
        head: state.config.server.header_read_timeout,
    };

    // The loops that run for as long as the process does, kept: one that
    // panics or returns when it should not is said so and started again.
    let tasks = Supervisor::new();
    let keep = Keep {
        tasks: &tasks,
        state: &state,
    };
    // Always started: whether it sweeps is a setting it re-reads, so turning
    // refresh on no longer needs a restart.
    keep.run("refresh scheduler", Ends::Never, jobs::refresh::run);
    keep.run("dataset imports", Ends::Never, jobs::datasets::run);
    keep.run("listing", Ends::Never, jobs::listing::run);
    // Whether it asks is a setting too, off until somebody turns it on.
    keep.run("update check", Ends::Never, jobs::update_check::run);
    // The archives of the database, as often as the setting says.
    keep.run("backups", Ends::Never, jobs::backup::run);
    // The search index, caught up on the works whose names changed outside
    // a write — and, after the upgrade that brought it, built.
    keep.run("search index", Ends::Never, jobs::search::run);
    // The works changed here, told to the operator's Sonarr and Radarr when
    // that is asked for: idle until a change is offered.
    keep.run("sonarr and radarr", Ends::Never, crate::arr::push::run);
    // The media in line, fetched; the media nobody points at, swept. Neither
    // has anything to do without a store.
    keep.run("media worker", Ends::WhenDone, media::worker::run);
    keep.run("media sweeps", Ends::WhenDone, media::worker::run_sweeps);
    // What is owed to the webhooks and notifications, sent and tried again.
    keep.run(
        "webhook deliveries",
        Ends::Never,
        crate::service::webhook::run,
    );
    // The cache server, attached when it answers and listened to: its
    // heartbeat and its channel are kept too, once it does.
    tasks.keep("cache server", Ends::WhenDone, {
        let (state, tasks) = (state.clone(), tasks.clone());
        move || crate::cache::attach(state.clone(), tasks.clone())
    });
    // Among several instances: the lead kept or taken, this one announced.
    // Alone, neither has anything to do.
    keep.run("lease", Ends::WhenDone, coord::keep_lease);
    keep.run("announcements", Ends::WhenDone, coord::run);

    // Sonarr builds its URLs from `.../v1/tvdb/{route}/{language}/` — with a
    // trailing slash — and its hostname is compiled in, so there is no way to
    // ask it not to. `NormalizePathLayer` applied with `Router::layer` runs
    // *after* routing and so cannot help; it has to wrap the whole router as a
    // service, before a path is ever matched.
    let normalized = NormalizePathLayer::trim_trailing_slash().layer(build_router(state.clone()));

    // The documentation's own entrance, answered *outside* the normalisation.
    // Swagger UI redirects `/api/docs` to `/api/docs/`, and the layer above —
    // which Sonarr needs — strips that slash again, so the page the README
    // points at redirected to itself until the browser gave up. Its index is
    // the one address neither step rewrites, and a real redirect to it (not a
    // rewrite) is what makes the page's relative asset URLs resolve under
    // `/api/docs/`. It reveals nothing: the index itself is still guarded.
    // Every layer the door has wraps these two as well as the rest.
    let router = protect(
        &state,
        Router::new()
            .route("/api/docs", get(docs_entry))
            .route("/api/docs/", get(docs_entry))
            .fallback_service(normalized),
    );

    // The clients' door, when there is one: bound first, so a port that is
    // taken stops the start rather than being found out later.
    let clients_door = match &state.tls.clients {
        Some(clients) => {
            let listener = std::net::TcpListener::bind(clients.bind)
                .with_context(|| format!("cannot bind {} for the clients", clients.bind))?;
            listener
                .set_nonblocking(true)
                .context("could not set the clients' listener non-blocking")?;
            let listener = tokio::net::TcpListener::from_std(listener)
                .context("could not take the clients' listener over")?;
            let handle = axum_server::Handle::<listen::Peer>::new();
            let clients_router = Router::new().fallback_service(
                NormalizePathLayer::trim_trailing_slash()
                    .layer(build_clients_router(state.clone())),
            );
            tracing::info!(
                bind = %clients.bind,
                names = ?clients.names.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "listening for the clients (TLS)"
            );
            let served = listen::serve(
                listen::Door::new(listener, limits.connections),
                Some(clients.rustls.clone()),
                clients_router,
                handle.clone(),
                limits,
            );
            let task = tokio::spawn(async move {
                if let Err(e) = served.await {
                    tracing::error!(error = %e, "the clients' listener failed");
                }
            });
            keep.run("certificate renewals", Ends::WhenDone, tls::run_renewals);
            Some((handle, task))
        }
        None => None,
    };

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("cannot bind {bind}"))?;
    let tls = match state.config.server.tls.clone() {
        Some(tls) => Some(
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
                .await
                .with_context(|| {
                    format!(
                        "could not load the TLS certificate {} / key {}",
                        tls.cert.display(),
                        tls.key.display()
                    )
                })?,
        ),
        None => None,
    };
    match tls {
        Some(_) => tracing::info!(%bind, "listening (TLS)"),
        None => tracing::info!(%bind, "listening"),
    }
    // When the certificates loaded from files end, for the metrics: the
    // interface's, and the clients' when the operator brought it. Those the
    // authority issues are read off it.
    if let Some(own) = &state.config.server.tls {
        crate::metrics::note_certificate("interface", &own.cert);
    }
    if let Some(own) = state.config.clients.as_ref().and_then(|c| c.tls.as_ref()) {
        crate::metrics::note_certificate("clients", &own.cert);
    }

    // One signal stops every door at once — the clients' too, which used to
    // go on accepting until the interface's had drained — and each is given
    // `DRAIN` before what is still open is closed.
    let handle = axum_server::Handle::<listen::Peer>::new();
    tokio::spawn({
        let doors: Vec<_> = std::iter::once(handle.clone())
            .chain(clients_door.as_ref().map(|(door, _)| door.clone()))
            .collect();
        let tasks = tasks.clone();
        async move {
            shutdown_signal().await;
            // What ends from here on is the stop's doing: not started again.
            tasks.stop();
            for door in doors {
                door.graceful_shutdown(Some(DRAIN));
            }
        }
    });

    listen::serve(
        listen::Door::new(listener, limits.connections),
        tls,
        router,
        handle,
        limits,
    )
    .await
    .context("server error")?;

    // The clients' door is waited for: a Sonarr mid-fetch gets its answer
    // before the database goes.
    if let Some((_, task)) = clients_door
        && let Err(e) = task.await
    {
        tracing::warn!(error = %e, "the clients' listener did not end cleanly");
    }

    // The loops are ended before the database is closed, so what one holds
    // is handed back rather than waited for.
    if tokio::time::timeout(DRAIN, tasks.shutdown()).await.is_err() {
        tracing::warn!("the background tasks did not all end in time");
    }

    // Given the drain's time to have its connections handed back: a job
    // holding one must not keep the process up until it is killed.
    if tokio::time::timeout(DRAIN, state.db.close()).await.is_err() {
        tracing::warn!("the database's connections were not all handed back in time");
    }

    // The last traces and error reports, sent before the process goes.
    crate::telemetry::shutdown().await;
    Ok(())
}

/// How long in-flight requests are given to finish once a stop is asked.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(5);

/// What keeps the loops that take the state and nothing else, as most do.
struct Keep<'a> {
    tasks: &'a Supervisor,
    state: &'a AppState,
}

impl Keep<'_> {
    fn run<Fut>(&self, name: &'static str, ends: Ends, run: fn(AppState) -> Fut)
    where
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let state = self.state.clone();
        self.tasks.keep(name, ends, move || run(state.clone()));
    }
}

/// What every answer of the interface's door goes through, inside out: the
/// name it was addressed to, CORS, the body limit, compression, the timeout,
/// a panic turned into a 500 — and only then the security headers, so that
/// a 504 or a 500 made by those carries them too — the trace, the request's
/// id, and the count.
fn protect(state: &AppState, router: Router) -> Router {
    let router = router
        // Nearest the handlers: while an archive is restored nothing reaches
        // one but the liveness probe, and the 503 goes out through every
        // layer below like any answer.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::backup::maintenance::guard,
        ))
        // Before any guard: a request for a name this server does not answer
        // to should not reach one, let alone a handler.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            check_host,
        ))
        .layer(cors(state))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        // Not the sounds: a song is already as small as it gets, and a range
        // of one must arrive as the bytes asked for. Nor an archive, which
        // is compressed already, and encrypted when it is not plain zstd.
        .layer(CompressionLayer::new().compress_when({
            use tower_http::compression::{Predicate as _, predicate::NotForContentType};
            tower_http::compression::DefaultPredicate::new()
                .and(NotForContentType::new("audio/"))
                .and(NotForContentType::new("application/zstd"))
                .and(NotForContentType::new("application/octet-stream"))
        }))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            state.config.server.request_timeout,
        ))
        .layer(CatchPanicLayer::new())
        // One policy for the interface, and one for the API's documentation
        // alone: Swagger UI styles the page it draws inline, which the
        // policy refuses everywhere else. Chosen by path, so the exception
        // never reaches a page a reader is served.
        .layer(axum::middleware::from_fn({
            let policy = content_security_policy(state);
            let docs = docs_content_security_policy(&policy);
            move |request: Request, next: axum::middleware::Next| {
                let chosen = if is_docs(request.uri().path()) {
                    docs.clone()
                } else {
                    policy.clone()
                };
                async move {
                    let mut response = next.run(request).await;
                    response
                        .headers_mut()
                        .insert(header::CONTENT_SECURITY_POLICY, chosen);
                    response
                }
            }
        }))
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(header::X_FRAME_OPTIONS, "SAMEORIGIN"))
        .layer(header_layer(PERMISSIONS_POLICY, PERMISSIONS))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            transport_headers,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(crate::telemetry::RequestSpan))
        // With traces on, the request is served in a span of the trace,
        // which the request span is made under: just outside it.
        .layer(axum::middleware::from_fn(crate::telemetry::trace::serve));
    // Outside the trace, whose span is where the id is read into.
    crate::telemetry::with_request_id(router, state.config.server.trusted_proxies.clone())
        // Counted last of all, so what is counted is what was answered.
        .layer(axum::middleware::from_fn(crate::metrics::observe))
}

/// What no page here asks for: the camera, the microphone, where somebody
/// is, a payment. Denied to the page and to whatever it frames.
const PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");
const PERMISSIONS: &str = "camera=(), microphone=(), geolocation=(), payment=()";

/// A year, without the subdomains — other services under the same name may
/// well be plain HTTP on a home network — and without asking browsers to
/// ship it preloaded.
const HSTS: &str = "max-age=31536000";

/// The headers that depend on how the request came and on what it is
/// answered with: HSTS where it came in TLS — this server's own, or a
/// trusted proxy's word — and, on a page, the opener policy that keeps a
/// window this page opens, or that opened it, from reaching into it.
async fn transport_headers(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let server = &state.config.server;
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0);
    let https = crate::auth::ip::scheme(
        peer,
        request.headers(),
        &server.trusted_proxies,
        server.tls.is_some(),
    ) == Some("https");

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if https {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS),
        );
    }
    let page = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|kind| kind.starts_with("text/html"));
    if page {
        headers.insert(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        );
    }
    response
}

fn build_router(state: AppState) -> Router {
    // Each surface carries its own authentication policy, applied inside; the
    // spec is collected from the same handlers, so it cannot drift from them.
    let (surfaces, openapi) = api::build(state.clone());

    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        // The media kept, outside every guard: a key is a hash nobody
        // guesses, and Sonarr fetches a poster with no credential.
        .route("/media/{key}", get(crate::media::serve::get))
        // The authority the clients are to trust, and the script that makes
        // a container trust it: public, since a client fetches them before
        // it can trust anything.
        .route("/ca.crt", get(crate::tls::ca_certificate))
        .route("/trust-ca.sh", get(crate::tls::trust_script))
        // Every API answer carries a validator, so a client that holds one is
        // told "unchanged" rather than sent it again. Inside the compression,
        // which would otherwise vary the bytes the tag is taken from.
        .merge(surfaces.layer(axum::middleware::from_fn(etag::conditional)))
        // Documentation sits behind the native guard: it is an administrative
        // view of the server, and the UI reaches it with its session cookie.
        .merge(
            Router::<AppState>::from(SwaggerUi::new("/api/docs").url("/api/openapi.json", openapi))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    crate::auth::middleware::guard_native,
                )),
        )
        // The UI's fallback must be last: it answers every path the API did not.
        .merge(crate::ui::router())
        // The route a request took, given to its trace once the router has
        // found it: every route above, none of the fallback's.
        .route_layer(axum::middleware::from_fn(
            crate::telemetry::trace::name_route,
        ))
        .with_state(state)
}

/// The clients' door: Sonarr's, Radarr's and the TMDB relay's surfaces and
/// nothing of the interface's — under the names the clients have compiled
/// in, which is the one host check that needs no configuring.
fn build_clients_router(state: AppState) -> Router {
    let timeout = state.config.server.request_timeout;
    let names: std::sync::Arc<Vec<String>> = std::sync::Arc::new(
        state
            .tls
            .clients
            .as_ref()
            .map(|clients| clients.names.iter().map(ToString::to_string).collect())
            .unwrap_or_default(),
    );

    let trusted = state.config.server.trusted_proxies.clone();
    let router = Router::new()
        .route("/health", get(health))
        .route("/ca.crt", get(crate::tls::ca_certificate))
        .route("/trust-ca.sh", get(crate::tls::trust_script))
        .merge(
            api::build_clients(state.clone()).layer(axum::middleware::from_fn(etag::conditional)),
        )
        .route_layer(axum::middleware::from_fn(
            crate::telemetry::trace::name_route,
        ))
        // A router's layers wrap its default fallback too: without a fallback
        // of its own, this door would answer an unknown path with a surface's
        // refusal rather than "nothing here". Asked of services.sonarr.tv, a
        // path is Sonarr's, and relayed; asked of graphql.anilist.co, it is
        // a query for AniList.
        .fallback({
            let relays = std::sync::Arc::new(api::HostRelays::new(state.clone()));
            move |request: Request| {
                let relays = relays.clone();
                async move { relays.answer(request).await }
            }
        })
        .with_state(state.clone())
        // The clients are shut out while an archive is restored, as the
        // interface is.
        .layer(axum::middleware::from_fn_with_state(
            state,
            crate::backup::maintenance::guard,
        ))
        .layer(axum::middleware::from_fn(
            move |request: Request, next: axum::middleware::Next| {
                let names = names.clone();
                async move { answer_to(&names, request, next).await }
            },
        ))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            timeout,
        ))
        .layer(CatchPanicLayer::new())
        // Outside the timeout and the panic, as on the interface's door.
        // Nothing this door answers is a page to render: what it relays
        // under the services' names is given no right to run anything.
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'; sandbox",
        ))
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(TraceLayer::new_for_http().make_span_with(crate::telemetry::RequestSpan))
        .layer(axum::middleware::from_fn(crate::telemetry::trace::serve));
    crate::telemetry::with_request_id(router, trusted)
        .layer(axum::middleware::from_fn(crate::metrics::observe_door))
}

/// Refuse a request addressed to a name the clients' door does not answer
/// to: the names in its certificate, and the operator's.
async fn answer_to(names: &[String], request: Request, next: axum::middleware::Next) -> Response {
    let host = host_of(&request);
    if names.iter().any(|name| name == &host) {
        return next.run(request).await;
    }
    tracing::warn!(%host, "refused a request addressed to a name the clients' door does not answer to");
    (StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response()
}

/// Refuse a request addressed to a name this server does not answer to.
///
/// The defence against DNS rebinding. A name with a one-second TTL that
/// resolves first to the attacker's host and then to this server's address
/// makes the victim's own browser treat the attacker's page as *same-origin*
/// with this server — CORS does not apply to a same-origin request, so it never
/// gets a say. What the script can then do depends on the surface policy: with
/// the default `apikey` it still has no credential, but with `allowlist` the
/// browser is calling from an address that is on the list.
///
/// Off unless `AMS_ALLOWED_HOSTS` names something, because there is no safe
/// guess — this is reached by container name, LAN address, and whatever the
/// router calls it.
///
/// The probes and the authority's files are answered whatever the name:
/// the container's own health check asks `127.0.0.1`, a client's trust
/// script and a proxy's health check ask by address, and none of them
/// reveals anything to a page that rebinds a name to this server.
async fn check_host(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let allowed = &state.config.server.allowed_hosts;

    if allowed.is_empty() || ANY_NAME.contains(&request.uri().path()) {
        return next.run(request).await;
    }

    let host = host_of(&request);

    if allowed.iter().any(|a| a.trim_matches(['[', ']']) == host) {
        return next.run(request).await;
    }

    tracing::warn!(%host, "refused a request addressed to a name this server does not answer to");
    (StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response()
}

/// What is answered under any name. See [`check_host`].
const ANY_NAME: [&str; 4] = ["/health", "/ready", "/ca.crt", "/trust-ca.sh"];

/// The name a request was addressed to, without its port: the `Host` header
/// over HTTP/1, the `:authority` hyper files in the URI over HTTP/2. An
/// IPv6 address loses its brackets, so it reads as the address is written.
fn host_of(request: &Request) -> String {
    request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| request.uri().host())
        .map(strip_port)
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase()
}

/// `example.com:8080` is `example.com`; `[::1]:8080` is `[::1]`.
fn strip_port(host: &str) -> &str {
    let host = host.trim();

    if let Some(end) = host.strip_prefix('[').and_then(|_| host.find(']')) {
        return &host[..=end];
    }

    match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    }
}

/// What a page here may load, and who may embed it.
///
/// The interface is served from this same origin and pulls in no third-party
/// code, so everything is `'self'`. Three exceptions, all forced:
///
/// * **images** come from wherever a provider filed them — TMDB, TheTVDB,
///   Fanart.tv and whatever host a manual entry names — and, as `blob:`, from
///   the file an editor has just picked, previewed before it is uploaded;
/// * **media** from the same hosts, for a work's theme music, fetched only
///   once somebody presses play;
/// * **two frames**: a work's trailer, played from YouTube's no-cookie domain —
///   and only once somebody presses play, so the page itself sends nobody to
///   YouTube — and this origin's own pages, which the work editor shows
///   beside itself as a preview of the public page.
///
/// Styles are `'self'` alone. The interface's are one stylesheet the build
/// writes, loaded by a `<link>`, and the per-card delays and accent colours
/// React writes through a `style` prop go through the CSSOM, which the policy
/// does not judge — only a `<style>` element and a `style=""` attribute in
/// markup do, and neither is written anywhere here. Swagger UI's are its own
/// stylesheets, loaded the same way.
///
/// `frame-ancestors 'self'` is the one that earns its place on a server on a
/// home network: it is what stops a page elsewhere framing this one and
/// borrowing an administrator's clicks, while the editor's preview may frame
/// the public page from this same origin (`X-Frame-Options: SAMEORIGIN` says
/// the same to an engine older than the directive). `base-uri` and
/// `form-action` close the two ways a stray tag could redirect a relative URL
/// or a form off-origin.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     img-src 'self' data: blob: https:; \
     media-src 'self' https:; \
     style-src 'self'; \
     script-src 'self'; \
     connect-src 'self'; \
     frame-src 'self' https://www.youtube-nocookie.com; \
     font-src 'self' data:; \
     object-src 'none'; \
     base-uri 'self'; \
     form-action 'self'; \
     frame-ancestors 'self'";

/// The policy, with the bucket a reader is redirected to when the policy
/// does not allow it already: `https:` covers Amazon's own and any bucket
/// behind TLS, but a Garage on the home network speaks http.
fn content_security_policy(state: &AppState) -> HeaderValue {
    let bucket = state
        .config
        .media
        .s3
        .as_ref()
        .map(bucket_origins)
        .unwrap_or_default();
    if bucket.is_empty() {
        return HeaderValue::from_static(CONTENT_SECURITY_POLICY);
    }
    let bucket = bucket.join(" ");
    let policy = CONTENT_SECURITY_POLICY
        .replace(
            "img-src 'self' data: blob: https:;",
            &format!("img-src 'self' data: blob: https: {bucket};"),
        )
        .replace(
            "media-src 'self' https:;",
            &format!("media-src 'self' https: {bucket};"),
        );
    HeaderValue::from_str(&policy)
        .unwrap_or_else(|_| HeaderValue::from_static(CONTENT_SECURITY_POLICY))
}

/// The policy the API's documentation is served under: the same, but for
/// the styles Swagger UI writes inline as it draws its page. `/api/docs` is
/// opened by an administrator with a credential, and renders this server's
/// own document; scripts stay this origin's there as everywhere.
fn docs_content_security_policy(policy: &HeaderValue) -> HeaderValue {
    let relaxed = policy
        .to_str()
        .unwrap_or(CONTENT_SECURITY_POLICY)
        .replace("style-src 'self';", "style-src 'self' 'unsafe-inline';");
    HeaderValue::from_str(&relaxed)
        .unwrap_or_else(|_| HeaderValue::from_static(CONTENT_SECURITY_POLICY))
}

/// Whether a path is the API documentation's: the page and what it loads,
/// and no path that merely begins with the same letters.
fn is_docs(path: &str) -> bool {
    path == "/api/docs" || path.starts_with("/api/docs/")
}

/// The origins a bucket's presigned addresses are on, when they are not on
/// https: the endpoint's, and the bucket's own host under it when the bucket
/// is addressed as a subdomain.
fn bucket_origins(s3: &crate::config::S3) -> Vec<String> {
    let Some(url) = s3
        .endpoint
        .as_deref()
        .and_then(|endpoint| reqwest::Url::parse(endpoint).ok())
    else {
        return Vec::new();
    };
    let Some(host) = url.host_str().filter(|_| url.scheme() == "http") else {
        return Vec::new();
    };
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let mut origins = vec![format!("http://{host}{port}")];
    if !s3.path_style {
        origins.push(format!("http://{}.{host}{port}", s3.bucket));
    }
    origins
}

/// One fixed response header, on every answer this server gives.
fn header_layer(
    name: header::HeaderName,
    value: &'static str,
) -> tower_http::set_header::SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

fn cors(state: &AppState) -> CorsLayer {
    let origins = &state.config.server.cors_origins;

    if origins.is_empty() {
        // Same-origin only: the UI is served from this server.
        return CorsLayer::new();
    }

    let parsed: Vec<HeaderValue> = origins
        .iter()
        // `*` cannot be combined with credentials, and tower-http answers that
        // by panicking rather than refusing — so a plausible thing to write in
        // a compose file took the process down at startup instead of being
        // reported. There is no wildcard to have here: the cookie rides along.
        .filter(|o| {
            if o.trim() == "*" {
                tracing::warn!(
                    "ignoring `*` in AMS_CORS_ORIGINS: this server sends credentials, \
                     so every allowed origin has to be named"
                );
                return false;
            }
            true
        })
        .filter_map(|o| match o.parse() {
            Ok(value) => Some(value),
            Err(_) => {
                tracing::warn!(origin = %o, "ignoring an invalid CORS origin");
                None
            }
        })
        .collect();

    if parsed.is_empty() {
        return CorsLayer::new();
    }

    CorsLayer::new()
        .allow_origin(parsed)
        .allow_credentials(true)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            "x-api-key".parse().unwrap(),
        ])
}

async fn docs_entry() -> axum::response::Redirect {
    axum::response::Redirect::to("/api/docs/index.html")
}

async fn health() -> StatusCode {
    StatusCode::OK
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Ready {
    database: bool,
    tmdb_configured: bool,
}

/// Liveness is `/health`; this is readiness, and it touches the database.
async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Ready>) {
    let database = state.db.health().await.is_ok();

    let status = if database {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(Ready {
            database,
            tmdb_configured: state.tmdb.is_configured(),
        }),
    )
}

/// Resolve on SIGINT or SIGTERM so container stops drain in-flight requests.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => tracing::warn!(error = %e, "cannot install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_host_header_is_compared_without_its_port() {
        assert_eq!(strip_port("metadata.example.com"), "metadata.example.com");
        assert_eq!(
            strip_port("metadata.example.com:8080"),
            "metadata.example.com"
        );
        assert_eq!(strip_port(" 192.168.1.50:8080 "), "192.168.1.50");
        assert_eq!(strip_port("[::1]:8080"), "[::1]");
        assert_eq!(strip_port("[2001:db8::1]"), "[2001:db8::1]");
        // Not a port, so not stripped.
        assert_eq!(strip_port("host:notaport"), "host:notaport");
    }

    use super::*;
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn s3(endpoint: Option<&str>, path_style: bool) -> crate::config::S3 {
        crate::config::S3 {
            endpoint: endpoint.map(str::to_string),
            region: "garage".into(),
            bucket: "ams-media".into(),
            access_key: "k".into(),
            secret_key: "s".into(),
            prefix: None,
            path_style,
        }
    }

    /// A bucket over plain http is let in, by its origin; one behind TLS,
    /// or Amazon's own, is allowed already.
    #[test]
    fn a_bucket_on_http_is_let_into_the_policy() {
        assert_eq!(
            bucket_origins(&s3(Some("http://127.0.0.1:3900"), true)),
            ["http://127.0.0.1:3900"]
        );
        assert_eq!(
            bucket_origins(&s3(Some("http://garage.lan"), false)),
            ["http://garage.lan", "http://ams-media.garage.lan"]
        );
        assert!(bucket_origins(&s3(Some("https://s3.example"), true)).is_empty());
        assert!(bucket_origins(&s3(None, true)).is_empty());
        assert!(bucket_origins(&s3(Some("not a url"), true)).is_empty());
    }

    /// The directives the bucket is added to are there to be added to.
    #[test]
    fn the_policy_names_the_directives_a_bucket_joins() {
        assert!(CONTENT_SECURITY_POLICY.contains("img-src 'self' data: blob: https:;"));
        assert!(CONTENT_SECURITY_POLICY.contains("media-src 'self' https:;"));
        let policy = CONTENT_SECURITY_POLICY
            .replace(
                "img-src 'self' data: blob: https:;",
                "img-src 'self' data: blob: https: http://127.0.0.1:3900;",
            )
            .replace(
                "media-src 'self' https:;",
                "media-src 'self' https: http://127.0.0.1:3900;",
            );
        assert!(HeaderValue::from_str(&policy).is_ok());
        assert_eq!(policy.matches("http://127.0.0.1:3900").count(), 2);
    }

    /// The rest of the policy is what it was: scripts from this origin only,
    /// nothing framing this one but itself, no plugin, no base or form elsewhere.
    #[test]
    fn the_policy_keeps_its_closed_doors() {
        for directive in [
            "default-src 'self'",
            "script-src 'self';",
            "style-src 'self';",
            "object-src 'none'",
            "base-uri 'self'",
            "form-action 'self'",
            // Framed by this origin alone: the editor's preview of a page.
            "frame-ancestors 'self'",
            "frame-src 'self' https://www.youtube-nocookie.com;",
        ] {
            assert!(CONTENT_SECURITY_POLICY.contains(directive), "{directive}");
        }
        assert!(HeaderValue::from_str(PERMISSIONS).is_ok());
        assert!(HeaderValue::from_str(HSTS).is_ok());
        assert!(!HSTS.contains("preload"));
    }

    /// Nothing inline runs or styles a page here: a `<style>` or a
    /// `style=""` an injection managed to write would be refused, as a
    /// `<script>` is.
    #[test]
    fn nothing_inline_is_allowed() {
        for keyword in ["'unsafe-inline'", "'unsafe-eval'", "'unsafe-hashes'"] {
            assert!(!CONTENT_SECURITY_POLICY.contains(keyword), "{keyword}");
        }
        assert!(CONTENT_SECURITY_POLICY.contains("style-src 'self';"));
    }

    /// Swagger UI styles its page inline, which the policy refuses
    /// everywhere else: the documentation's paths alone are allowed it, and
    /// nothing more — scripts stay this origin's, and a path that merely
    /// begins with the same letters is a page like any other.
    #[test]
    fn the_documentation_alone_may_style_inline() {
        let policy = HeaderValue::from_static(CONTENT_SECURITY_POLICY);
        let docs = docs_content_security_policy(&policy);
        let docs = docs.to_str().unwrap();
        assert!(docs.contains("style-src 'self' 'unsafe-inline';"));
        assert!(docs.contains("script-src 'self';"));
        assert_eq!(docs.matches("'unsafe-inline'").count(), 1);
        assert!(is_docs("/api/docs") && is_docs("/api/docs/index.html"));
        assert!(!is_docs("/api/docs-like") && !is_docs("/api/v1/items") && !is_docs("/"));
    }

    #[test]
    fn the_probes_and_the_authority_answer_under_any_name() {
        for path in ["/health", "/ready", "/ca.crt", "/trust-ca.sh"] {
            assert!(ANY_NAME.contains(&path), "{path}");
        }
        assert!(!ANY_NAME.contains(&"/api/v1/items"));
    }
}
