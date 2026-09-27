//! HTTP server: router assembly, middleware stack, TLS, graceful shutdown.

mod etag;

use anyhow::{Context, Result};
use axum::{
    Json, Router, ServiceExt,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
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

use crate::{api, state::AppState};

/// Largest request body accepted anywhere, but for an upload: the only
/// sizeable one otherwise is a bulk movie lookup, which is a list of
/// integers. Axum's own limit rather than a layer that cuts every body, so
/// the one route that takes a file can be given more.
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub async fn serve(state: AppState) -> Result<()> {
    let bind = state.config.server.bind;

    // Always started: whether it sweeps is a setting it re-reads, so turning
    // refresh on no longer needs a restart.
    tokio::spawn(crate::jobs::refresh::run(state.clone()));
    tokio::spawn(crate::jobs::datasets::run(state.clone()));
    tokio::spawn(crate::jobs::listing::run(state.clone()));
    // The media in line, fetched; the media nobody points at, swept.
    tokio::spawn(crate::media::worker::run(state.clone()));
    tokio::spawn(crate::media::worker::run_sweeps(state.clone()));

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
    let router = Router::new()
        .route("/api/docs", get(docs_entry))
        .route("/api/docs/", get(docs_entry))
        .fallback_service(normalized);

    match state.config.server.tls.clone() {
        Some(tls) => {
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
                .await
                .with_context(|| {
                    format!(
                        "could not load the TLS certificate {} / key {}",
                        tls.cert.display(),
                        tls.key.display()
                    )
                })?;

            tracing::info!(%bind, "listening (TLS)");

            axum_server::bind_rustls(bind, config)
                .serve(
                    ServiceExt::<Request>::into_make_service_with_connect_info::<
                        std::net::SocketAddr,
                    >(router),
                )
                .await
                .context("server error")?;
        }
        None => {
            tracing::info!(%bind, "listening");

            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .with_context(|| format!("cannot bind {bind}"))?;

            axum::serve(
                listener,
                ServiceExt::<Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(
                    router,
                ),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("server error")?;
        }
    }

    state.db.close().await;
    Ok(())
}

fn build_router(state: AppState) -> Router {
    let timeout = state.config.server.request_timeout;

    // Each surface carries its own authentication policy, applied inside; the
    // spec is collected from the same handlers, so it cannot drift from them.
    let (surfaces, openapi) = api::build(state.clone());

    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        // The media kept, outside every guard: a key is a hash nobody
        // guesses, and Sonarr fetches a poster with no credential.
        .route("/media/{key}", get(crate::media::serve::get))
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
        .with_state(state.clone())
        // Outside everything, because a request for a name this server does not
        // answer to should not reach a guard, let alone a handler.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            check_host,
        ))
        .layer(cors(&state))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            content_security_policy(&state),
        ))
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        // Not the sounds: a song is already as small as it gets, and a range
        // of one must arrive as the bytes asked for.
        .layer(CompressionLayer::new().compress_when({
            use tower_http::compression::Predicate as _;
            tower_http::compression::DefaultPredicate::new().and(
                tower_http::compression::predicate::NotForContentType::new("audio/"),
            )
        }))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            timeout,
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        // Counted last of all, so what is counted is what was answered.
        .layer(axum::middleware::from_fn(crate::metrics::observe))
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
async fn check_host(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let allowed = &state.config.server.allowed_hosts;

    if allowed.is_empty() {
        return next.run(request).await;
    }

    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(strip_port)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if allowed.iter().any(|a| a == &host) {
        return next.run(request).await;
    }

    tracing::warn!(%host, "refused a request addressed to a name this server does not answer to");
    (StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response()
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
/// code, so everything is `'self'`. Two exceptions, both forced:
///
/// * **images** come from wherever a provider filed them — TMDB, TheTVDB,
///   Fanart.tv and whatever host a manual entry names;
/// * **inline styles** are how React writes a `style` attribute, and the
///   interface uses them for per-card animation delays and accent colours.
///
/// * **media** from the same hosts, for a work's theme music, fetched only
///   once somebody presses play;
/// * **one frame**: a work's trailer, played from YouTube's no-cookie domain —
///   and only once somebody presses play, so the page itself sends nobody to
///   YouTube.
///
/// `frame-ancestors 'none'` is the one that earns its place on a server on a
/// home network: it is what stops a page elsewhere framing this one and
/// borrowing an administrator's clicks. `base-uri` and `form-action` close the
/// two ways a stray tag could redirect a relative URL or a form off-origin.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     img-src 'self' data: https:; \
     media-src 'self' https:; \
     style-src 'self' 'unsafe-inline'; \
     script-src 'self'; \
     connect-src 'self'; \
     frame-src https://www.youtube-nocookie.com; \
     font-src 'self' data:; \
     object-src 'none'; \
     base-uri 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'";

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
            "img-src 'self' data: https:;",
            &format!("img-src 'self' data: https: {bucket};"),
        )
        .replace(
            "media-src 'self' https:;",
            &format!("media-src 'self' https: {bucket};"),
        );
    HeaderValue::from_str(&policy)
        .unwrap_or_else(|_| HeaderValue::from_static(CONTENT_SECURITY_POLICY))
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
        assert!(CONTENT_SECURITY_POLICY.contains("img-src 'self' data: https:;"));
        assert!(CONTENT_SECURITY_POLICY.contains("media-src 'self' https:;"));
        let policy = CONTENT_SECURITY_POLICY
            .replace(
                "img-src 'self' data: https:;",
                "img-src 'self' data: https: http://127.0.0.1:3900;",
            )
            .replace(
                "media-src 'self' https:;",
                "media-src 'self' https: http://127.0.0.1:3900;",
            );
        assert!(HeaderValue::from_str(&policy).is_ok());
        assert_eq!(policy.matches("http://127.0.0.1:3900").count(), 2);
    }
}
