//! What the server counts about itself, offered to a Prometheus scrape.
//!
//! A registry of its own rather than a crate: a few counters and histograms
//! with fixed labels, added to without a lock and read in one pass, and the
//! text format is four lines a metric. Requests are counted by the surface
//! they land on — Sonarr's, Radarr's, Plex's provider, the TMDB relay, the
//! native API, the interface — by status class and by status, and the calls
//! made upstream by the provider asked, each with how long it took; and how
//! long a request waited for one of the database's connections. What the
//! catalogue holds, what the caches keep, what waits in each queue, how the
//! pool of connections stands, when the certificates end and what the jobs
//! did are read off the state when the scrape comes.

use std::{
    sync::{
        LazyLock, RwLock,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use axum::{extract::Request, middleware::Next, response::Response};

/// The surfaces a request lands on, in the order they are counted. The
/// liveness probes are a surface of their own, so a monitor does not count
/// as readers; so are the media kept, and the authority's files a client
/// fetches to trust this server; and the MCP server an agent calls.
pub const SURFACES: [&str; 12] = [
    "sonarr", "radarr", "plex", "tmdb", "tvdb", "anilist", "native", "media", "ca", "probe", "mcp",
    "ui",
];

/// The providers asked upstream, in the order they are counted.
pub const PROVIDERS: [&str; 12] = [
    "tmdb",
    "tvdb",
    "tvmaze",
    "anilist",
    "mal",
    "fanart",
    "skyhook",
    "radarr",
    "fankai",
    "fankaiwiki",
    // Sonarr's scene-mapping list and TheXEM's names, read to add titles.
    "sonarr_services",
    "thexem",
];

/// How an upstream call ended: answered (a 404 is an answer), refused by the
/// provider, failed on its side, or never reached.
const OUTCOMES: [&str; 4] = ["ok", "refused", "failed", "unreachable"];

/// Status classes, `1xx` to `5xx`.
const CLASSES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];

/// The histogram's upper bounds, in seconds; `+Inf` is implied.
const BOUNDS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// The bounds a wait for a connection is counted under: most take a few
/// microseconds, and one that takes a second is the pool run dry.
const WAIT_BOUNDS: [f64; 11] = [
    0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 2.5, 10.0,
];

/// The statuses counted one by one: every one HTTP has, 100 to 599.
const STATUSES: usize = 500;

/// A histogram of durations: how many fell in each bucket, and their sum.
/// Each observation touches one bucket, and the cumulative counts Prometheus
/// wants are added up from one reading when the scrape comes — so a reading
/// never shows a lower bound counted above a higher one.
pub struct Histogram {
    bounds: &'static [f64; 11],
    buckets: [AtomicU64; 12],
    sum_micros: AtomicU64,
}

impl Histogram {
    const fn new() -> Self {
        Self::with(&BOUNDS)
    }

    const fn with(bounds: &'static [f64; 11]) -> Self {
        Self {
            bounds,
            buckets: [const { AtomicU64::new(0) }; 12],
            sum_micros: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        let bucket = self
            .bounds
            .iter()
            .position(|bound| seconds <= *bound)
            .unwrap_or(11);
        self.buckets[bucket].fetch_add(1, Relaxed);
        self.sum_micros.fetch_add(
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
            Relaxed,
        );
    }

    fn render(&self, out: &mut String, name: &str, labels: &str) {
        let comma = if labels.is_empty() { "" } else { "," };
        let counts: [u64; 12] = std::array::from_fn(|i| self.buckets[i].load(Relaxed));
        let mut seen = 0;
        for (count, bound) in counts.iter().zip(self.bounds) {
            seen += count;
            out.push_str(&format!(
                "{name}_bucket{{{labels}{comma}le=\"{bound}\"}} {seen}\n"
            ));
        }
        seen += counts[11];
        out.push_str(&format!(
            "{name}_bucket{{{labels}{comma}le=\"+Inf\"}} {seen}\n"
        ));
        // No braces at all where there is no label: `name_sum 0.5`.
        let braced = if labels.is_empty() {
            String::new()
        } else {
            format!("{{{labels}}}")
        };
        out.push_str(&format!(
            "{name}_sum{braced} {}\n",
            self.sum_micros.load(Relaxed) as f64 / 1_000_000.0
        ));
        out.push_str(&format!("{name}_count{braced} {seen}\n"));
    }
}

struct Registry {
    /// Requests answered, by surface and status class.
    requests: [[AtomicU64; 5]; SURFACES.len()],
    /// And by status, 100 to 599: what an alert on 5xx above a share reads.
    responses: [[AtomicU64; STATUSES]; SURFACES.len()],
    request_time: [Histogram; SURFACES.len()],
    /// Calls made upstream, by provider and outcome.
    upstream: [[AtomicU64; 4]; PROVIDERS.len()],
    upstream_time: [Histogram; PROVIDERS.len()],
    /// How long each connection of the database's pool was waited for.
    pool_wait: Histogram,
}

static REGISTRY: Registry = Registry {
    requests: [const { [const { AtomicU64::new(0) }; 5] }; SURFACES.len()],
    responses: [const { [const { AtomicU64::new(0) }; STATUSES] }; SURFACES.len()],
    request_time: [const { Histogram::new() }; SURFACES.len()],
    upstream: [const { [const { AtomicU64::new(0) }; 4] }; PROVIDERS.len()],
    upstream_time: [const { Histogram::new() }; PROVIDERS.len()],
    pool_wait: Histogram::with(&WAIT_BOUNDS),
};

/// The index of a surface's name in [`SURFACES`].
fn surface(name: &str) -> usize {
    SURFACES
        .iter()
        .position(|s| *s == name)
        .unwrap_or(SURFACES.len() - 1)
}

/// Which surface a path lands on.
pub fn surface_of(path: &str) -> usize {
    let name = if path.starts_with("/v1/tvdb/")
        || path.starts_with("/v2/tvdb/")
        || path == "/v1/scenemapping"
    {
        // Sonarr's: Skyhook's two editions in its place, and
        // services.sonarr.tv's list.
        "sonarr"
    } else if path.starts_with("/v1/") {
        "radarr"
    } else if path == "/plex" || path.starts_with("/plex/") {
        "plex"
    } else if path.starts_with("/3/") || path.starts_with("/4/") {
        "tmdb"
    } else if path.starts_with("/v4/") || path == "/v4" {
        "tvdb"
    } else if path.starts_with("/api/") {
        "native"
    } else if path.starts_with("/media/") {
        "media"
    } else if path == "/ca.crt" || path == "/trust-ca.sh" {
        "ca"
    } else if path == "/health" || path == "/ready" {
        "probe"
    } else if path == "/mcp" || path.starts_with("/mcp/") {
        "mcp"
    } else {
        "ui"
    };
    surface(name)
}

/// Which surface a request to the clients' door lands on: what it was
/// asked of decides first — AniList's queries all go to `/`, and every path
/// of services.sonarr.tv is Sonarr's — and the path after that.
fn door_surface_of(request: &Request) -> usize {
    if crate::api::anilist::is_anilist_host(request.headers(), request.uri()) {
        surface("anilist")
    } else if crate::api::sonarr_services::is_services_host(request.headers(), request.uri()) {
        surface("sonarr")
    } else {
        surface_of(request.uri().path())
    }
}

/// Count a request as it is answered: the surface, the status and the time.
pub async fn observe(request: Request, next: Next) -> Response {
    let surface = surface_of(request.uri().path());
    count(surface, request, next).await
}

/// [`observe`], for the clients' door.
pub async fn observe_door(request: Request, next: Next) -> Response {
    let surface = door_surface_of(&request);
    count(surface, request, next).await
}

async fn count(surface: usize, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let class = usize::from(status / 100).clamp(1, 5) - 1;
    REGISTRY.requests[surface][class].fetch_add(1, Relaxed);
    if let Some(each) = usize::from(status)
        .checked_sub(100)
        .and_then(|at| REGISTRY.responses[surface].get(at))
    {
        each.fetch_add(1, Relaxed);
    }
    REGISTRY.request_time[surface].observe(started.elapsed());
    response
}

/// Count a call made upstream: the provider, how long it took, and how it
/// ended — by the status it answered with, or none where it was not reached.
pub fn upstream(provider: &str, started: Instant, status: Option<reqwest::StatusCode>) {
    let Some(index) = PROVIDERS.iter().position(|p| *p == provider) else {
        return;
    };
    let outcome = match status {
        Some(s) if s.is_success() || s.is_redirection() || s == reqwest::StatusCode::NOT_FOUND => 0,
        Some(s) if s.is_client_error() => 1,
        Some(_) => 2,
        None => 3,
    };
    REGISTRY.upstream[index][outcome].fetch_add(1, Relaxed);
    REGISTRY.upstream_time[index].observe(started.elapsed());
}

/// A value read at scrape time, with its samples: a label set and a number
/// each. An empty label set is one sample without labels.
pub struct Metric {
    pub name: &'static str,
    pub help: &'static str,
    pub samples: Vec<(String, f64)>,
}

/// Whether a newer release is out, as the update check last found: `1`
/// when one is. Offered only while the check is on.
pub fn update_available(found: bool) -> Metric {
    Metric {
        name: "ams_update_available",
        help: "Whether a newer release is out, as the update check last found: 1 when one is.",
        samples: vec![(String::new(), if found { 1.0 } else { 0.0 })],
    }
}

/// How old the newest archive is, in seconds: what an alert on backups
/// that stopped reads. Offered only while one exists — `absent()` is how
/// an alert says none was ever made.
pub fn backup_age(seconds: f64) -> Metric {
    Metric {
        name: "ams_backup_age_seconds",
        help: "Seconds since the newest archive of the database was made.",
        samples: vec![(String::new(), seconds.max(0.0))],
    }
}

/// A label value, escaped as the text format asks.
pub fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Everything counted so far, and the gauges given, in the text format.
pub fn render(gauges: &[Metric]) -> String {
    let mut out = String::with_capacity(8192);

    out.push_str(
        "# HELP ams_http_requests_total Requests answered, by surface and status class.\n",
    );
    out.push_str("# TYPE ams_http_requests_total counter\n");
    for (s, surface) in SURFACES.iter().enumerate() {
        for (c, class) in CLASSES.iter().enumerate() {
            out.push_str(&format!(
                "ams_http_requests_total{{surface=\"{surface}\",status=\"{class}\"}} {}\n",
                REGISTRY.requests[s][c].load(Relaxed)
            ));
        }
    }

    out.push_str(
        "# HELP ams_http_responses_total Requests answered, by surface and status: those seen.\n",
    );
    out.push_str("# TYPE ams_http_responses_total counter\n");
    for (s, surface) in SURFACES.iter().enumerate() {
        for (at, count) in REGISTRY.responses[s].iter().enumerate() {
            let count = count.load(Relaxed);
            if count > 0 {
                out.push_str(&format!(
                    "ams_http_responses_total{{surface=\"{surface}\",status=\"{}\"}} {count}\n",
                    at + 100
                ));
            }
        }
    }

    out.push_str(
        "# HELP ams_http_request_duration_seconds How long requests took to answer, by surface.\n",
    );
    out.push_str("# TYPE ams_http_request_duration_seconds histogram\n");
    for (s, surface) in SURFACES.iter().enumerate() {
        REGISTRY.request_time[s].render(
            &mut out,
            "ams_http_request_duration_seconds",
            &format!("surface=\"{surface}\""),
        );
    }

    out.push_str(
        "# HELP ams_upstream_requests_total Calls made to providers, by provider and outcome.\n",
    );
    out.push_str("# TYPE ams_upstream_requests_total counter\n");
    for (p, provider) in PROVIDERS.iter().enumerate() {
        for (o, outcome) in OUTCOMES.iter().enumerate() {
            out.push_str(&format!(
                "ams_upstream_requests_total{{provider=\"{provider}\",outcome=\"{outcome}\"}} {}\n",
                REGISTRY.upstream[p][o].load(Relaxed)
            ));
        }
    }

    out.push_str(
        "# HELP ams_upstream_duration_seconds How long providers took to answer, by provider.\n",
    );
    out.push_str("# TYPE ams_upstream_duration_seconds histogram\n");
    for (p, provider) in PROVIDERS.iter().enumerate() {
        REGISTRY.upstream_time[p].render(
            &mut out,
            "ams_upstream_duration_seconds",
            &format!("provider=\"{provider}\""),
        );
    }

    out.push_str(
        "# HELP ams_db_pool_wait_seconds How long a connection of the database's pool was waited for.\n",
    );
    out.push_str("# TYPE ams_db_pool_wait_seconds histogram\n");
    REGISTRY
        .pool_wait
        .render(&mut out, "ams_db_pool_wait_seconds", "");

    for metric in gauges {
        out.push_str(&format!("# HELP {} {}\n", metric.name, metric.help));
        // What only grows is a counter, and says so by its name.
        let kind = if metric.name.ends_with("_total") {
            "counter"
        } else {
            "gauge"
        };
        out.push_str(&format!("# TYPE {} {kind}\n", metric.name));
        for (labels, value) in &metric.samples {
            if labels.is_empty() {
                out.push_str(&format!("{} {value}\n", metric.name));
            } else {
                out.push_str(&format!("{}{{{labels}}} {value}\n", metric.name));
            }
        }
    }

    out
}

// ── The pool's waits ────────────────────────────────────────────────────

/// What sqlx's pool says each time a connection is handed out, at `trace`
/// (`db::Db::connect` asks it to): how long it was waited for. Counted here,
/// into `ams_db_pool_wait_seconds`; the log never writes it, its filters
/// stopping at `info` but for the slow ones sqlx says at `warn`.
const POOL_ACQUIRE: &str = "sqlx::pool::acquire";

/// The layer that reads those waits off the pool's own lines.
pub fn pool_wait_layer<S>() -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::Layer as _;
    PoolWaits.with_filter(
        tracing_subscriber::filter::Targets::new().with_target(POOL_ACQUIRE, tracing::Level::TRACE),
    )
}

struct PoolWaits;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for PoolWaits {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(seconds) = waited(event) {
            REGISTRY.pool_wait.observe(Duration::from_secs_f64(seconds));
        }
    }
}

/// The seconds a pool's line says a connection took to come, when it says.
fn waited(event: &tracing::Event<'_>) -> Option<f64> {
    struct Seconds(Option<f64>);
    impl tracing::field::Visit for Seconds {
        fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
            if field.name() == "acquired_after_secs" {
                self.0 = Some(value);
            }
        }
        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    }
    if event.metadata().target() != POOL_ACQUIRE {
        return None;
    }
    let mut seconds = Seconds(None);
    event.record(&mut seconds);
    seconds.0.filter(|s| s.is_finite() && *s >= 0.0 && *s < 1e9)
}

// ── Read when the scrape comes ──────────────────────────────────────────

/// When the certificates the server was started with end, by name, in
/// seconds since the epoch: the interface's own, and the clients' when the
/// operator brought it. Those the authority issues are read off it.
static CERTIFICATES: LazyLock<RwLock<Vec<(&'static str, i64)>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Note when the certificate in a PEM file ends, as the server loads it: the
/// first certificate the file holds, which is the one shown.
pub fn note_certificate(name: &'static str, pem: &std::path::Path) {
    let Some(end) = std::fs::read(pem).ok().and_then(|bytes| ends(&bytes)) else {
        tracing::warn!(
            path = %pem.display(),
            "the certificate's end could not be read: ams_tls_expiry_seconds will not say it"
        );
        return;
    };
    if let Ok(mut known) = CERTIFICATES.write() {
        known.retain(|(known, _)| *known != name);
        known.push((name, end));
    }
}

/// When the first certificate of a PEM text ends, in seconds since the epoch.
fn ends(pem: &[u8]) -> Option<i64> {
    let (_, block) = x509_parser::pem::parse_x509_pem(pem).ok()?;
    let certificate = block.parse_x509().ok()?;
    Some(certificate.validity().not_after.timestamp())
}

/// What is read off the state when the scrape comes, beside the counters:
/// the pool of connections, what waits in each queue, the works whose last
/// refresh failed, and when the certificates end. What the database cannot
/// answer is left out, as the rest of the scrape's figures are.
pub async fn server(state: &crate::state::AppState) -> Vec<Metric> {
    let pool = state.db.pool();
    let mut gauges = vec![
        Metric {
            name: "ams_db_pool_size",
            help: "Connections the database's pool holds open, busy or idle.",
            samples: vec![(String::new(), f64::from(pool.size()))],
        },
        Metric {
            name: "ams_db_pool_idle",
            help: "Connections of the pool waiting for work.",
            samples: vec![(String::new(), pool.num_idle() as f64)],
        },
        Metric {
            name: "ams_db_pool_max",
            help: "The most connections the pool opens.",
            samples: vec![(
                String::new(),
                f64::from(pool.options().get_max_connections()),
            )],
        },
    ];

    match queue_depths(&state.db).await {
        Ok(depths) => gauges.push(Metric {
            name: "ams_queue_depth",
            help: "What waits in each queue: works due for a refresh, media to fetch, \
                   webhook deliveries to send.",
            samples: depths
                .into_iter()
                .map(|(kind, n)| (format!("kind=\"{kind}\""), n as f64))
                .collect(),
        }),
        Err(e) => tracing::warn!(error = %e, "the scrape goes without the queues"),
    }

    let failed = crate::db::repo::item::count_matching(
        &state.db,
        &crate::db::repo::item::Query {
            include_adult: true,
            include_disabled: true,
            refresh_failed: true,
            ..Default::default()
        },
    )
    .await;
    match failed {
        Ok(failed) => gauges.push(Metric {
            name: "ams_refresh_failed",
            help: "Works whose last refresh failed.",
            samples: vec![(String::new(), failed as f64)],
        }),
        Err(e) => tracing::warn!(error = %e, "the scrape goes without the failed refreshes"),
    }

    let certificates = certificate_ends(state);
    if !certificates.is_empty() {
        let now = chrono::Utc::now().timestamp();
        gauges.push(Metric {
            name: "ams_tls_expiry_seconds",
            help: "Seconds left before each certificate ends; below nought once it has.",
            samples: certificates
                .into_iter()
                .map(|(name, end)| (format!("certificate=\"{name}\""), (end - now) as f64))
                .collect(),
        });
    }
    gauges
}

/// What the process holds in memory, and what its cgroup is charged for it
/// (see `memory`): the program's own bytes everywhere, the rest on Linux.
pub fn memory() -> Vec<Metric> {
    let gauge = |name, help, value: u64| Metric {
        name,
        help,
        samples: vec![(String::new(), value as f64)],
    };
    let mut gauges = vec![gauge(
        "ams_memory_heap_bytes",
        "Bytes the program holds from its allocator: what it asked for and has not given back.",
        crate::memory::heap_bytes(),
    )];
    if let Some(resident) = crate::memory::resident() {
        gauges.push(Metric {
            name: "ams_memory_resident_bytes",
            help: "Memory the kernel keeps resident for the process, by kind: its own \
                   (anonymous: the heap, what the allocator keeps for reuse, the stacks), the \
                   files it maps (its binary among them), shared.",
            samples: [
                ("anonymous", resident.anonymous),
                ("file", resident.file),
                ("shared", resident.shared),
            ]
            .into_iter()
            .map(|(kind, bytes)| (format!("kind=\"{kind}\""), bytes as f64))
            .collect(),
        });
        gauges.push(gauge(
            "ams_memory_resident_peak_bytes",
            "The most memory the process held resident at once since it started.",
            resident.peak,
        ));
        gauges.push(gauge(
            "ams_process_threads",
            "Threads the process runs.",
            resident.threads,
        ));
    }
    if let Some(charged) = crate::memory::charged() {
        let inactive = charged
            .kinds
            .iter()
            .find_map(|(kind, bytes)| (*kind == "inactive_file").then_some(*bytes))
            .unwrap_or(0);
        gauges.push(gauge(
            "ams_cgroup_memory_bytes",
            "Everything the process's cgroup — its container — is charged for, page cache \
             included: memory.current.",
            charged.current,
        ));
        gauges.push(gauge(
            "ams_cgroup_memory_working_set_bytes",
            "What docker stats shows: the cgroup's charge less the page cache the kernel takes \
             back first (inactive_file).",
            charged.current.saturating_sub(inactive),
        ));
        if let Some(limit) = charged.limit {
            gauges.push(gauge(
                "ams_cgroup_memory_limit_bytes",
                "The cgroup's memory ceiling, when it has one: memory.max.",
                limit,
            ));
        }
        gauges.push(Metric {
            name: "ams_cgroup_memory_stat_bytes",
            help: "The cgroup's charge by kind, as memory.stat counts it: anon, the processes' \
                   own; file, the page cache, active_file and inactive_file its two lists; \
                   shmem, tmpfs; kernel; sock, the sockets' buffers. The kinds overlap.",
            samples: charged
                .kinds
                .iter()
                .map(|(kind, bytes)| (format!("kind=\"{kind}\""), *bytes as f64))
                .collect(),
        });
        gauges.push(gauge(
            "ams_cgroup_memory_ceiling_hits_total",
            "Times the cgroup reached its ceiling and the kernel took memory back to stay \
             under it: max in memory.events.",
            charged.at_ceiling,
        ));
    }
    gauges
}

/// What waits in each queue, by kind: `refresh`, the works whose refresh is
/// due; `media`, the pictures and themes yet to be fetched; `webhooks`, the
/// deliveries yet to be sent.
async fn queue_depths(db: &crate::db::Db) -> anyhow::Result<Vec<(&'static str, i64)>> {
    use crate::db::RowExt as _;

    let refresh = sqlx::query(db.sql(
        "SELECT COUNT(*) AS n FROM media_item
         WHERE is_enabled = 1
           AND refresh_after IS NOT NULL
           AND refresh_after <= ?
           AND EXISTS (SELECT 1 FROM media_external_id e WHERE e.media_id = media_item.id)",
    ))
    .bind(crate::db::now())
    .fetch_one(db.pool())
    .await?
    .big("n")?;
    let media = crate::db::repo::asset::pending_count(db).await?;
    let webhooks =
        sqlx::query(db.sql("SELECT COUNT(*) AS n FROM webhook_delivery WHERE status = 'pending'"))
            .fetch_one(db.pool())
            .await?
            .big("n")?;
    Ok(vec![
        ("refresh", refresh),
        ("media", media),
        ("webhooks", webhooks),
    ])
}

/// When each certificate ends: those noted at the start, then the clients'
/// door's, as its authority issued it, and the authority's own.
fn certificate_ends(state: &crate::state::AppState) -> Vec<(&'static str, i64)> {
    let mut ends = CERTIFICATES
        .read()
        .map(|known| known.clone())
        .unwrap_or_default();
    if let Some(door) = &state.tls.clients {
        if let Some(issued) = door.issued() {
            ends.retain(|(name, _)| *name != "clients");
            ends.push(("clients", issued.not_after.unix_timestamp()));
        }
        if let Some(authority) = &door.authority {
            ends.push(("authority", authority.not_after.unix_timestamp()));
        }
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_lands_on_one_surface() {
        assert_eq!(SURFACES[surface_of("/v1/tvdb/shows/en/81189")], "sonarr");
        assert_eq!(
            SURFACES[surface_of("/v2/tvdb/shows/eng/81189/official")],
            "sonarr"
        );
        assert_eq!(SURFACES[surface_of("/v1/tvdb/search/en")], "sonarr");
        assert_eq!(SURFACES[surface_of("/v1/movie/550")], "radarr");
        assert_eq!(SURFACES[surface_of("/v1/search")], "radarr");
        assert_eq!(SURFACES[surface_of("/plex")], "plex");
        assert_eq!(
            SURFACES[surface_of("/plex/library/metadata/matches")],
            "plex"
        );
        assert_eq!(SURFACES[surface_of("/plexus")], "ui");
        assert_eq!(SURFACES[surface_of("/3/tv/1396")], "tmdb");
        assert_eq!(SURFACES[surface_of("/4/list/1")], "tmdb");
        assert_eq!(SURFACES[surface_of("/api/v1/items")], "native");
        assert_eq!(SURFACES[surface_of("/api/docs")], "native");
        assert_eq!(SURFACES[surface_of("/health")], "probe");
        assert_eq!(SURFACES[surface_of("/ready")], "probe");
        assert_eq!(SURFACES[surface_of("/")], "ui");
        assert_eq!(SURFACES[surface_of("/assets/index.js")], "ui");
        assert_eq!(SURFACES[surface_of("/work/x")], "ui");
        // Each relay and file kind under its own name, not the interface's.
        assert_eq!(SURFACES[surface_of("/v1/scenemapping")], "sonarr");
        assert_eq!(SURFACES[surface_of("/v4/series/81189")], "tvdb");
        assert_eq!(SURFACES[surface_of("/media/abc.jpg")], "media");
        assert_eq!(SURFACES[surface_of("/ca.crt")], "ca");
        assert_eq!(SURFACES[surface_of("/trust-ca.sh")], "ca");
        assert_eq!(SURFACES[surface_of("/mcp")], "mcp");
        assert_eq!(SURFACES[surface_of("/mcp/")], "mcp");
        assert_eq!(SURFACES[surface_of("/mcpx")], "ui");
    }

    #[test]
    fn the_clients_door_counts_by_the_name_asked() {
        let request = |host: &str, path: &str| {
            axum::http::Request::builder()
                .uri(path)
                .header(axum::http::header::HOST, host)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        assert_eq!(
            SURFACES[door_surface_of(&request("graphql.anilist.co", "/"))],
            "anilist"
        );
        assert_eq!(
            SURFACES[door_surface_of(&request("services.sonarr.tv", "/v1/update"))],
            "sonarr"
        );
        assert_eq!(
            SURFACES[door_surface_of(&request("api.radarr.video", "/v1/movie/550"))],
            "radarr"
        );
    }

    #[test]
    fn a_histogram_counts_under_every_bound_above_the_value() {
        let histogram = Histogram::new();
        histogram.observe(Duration::from_millis(30));
        histogram.observe(Duration::from_secs(20));

        let mut out = String::new();
        histogram.render(&mut out, "t", "k=\"v\"");
        assert!(out.contains("t_bucket{k=\"v\",le=\"0.025\"} 0\n"), "{out}");
        assert!(out.contains("t_bucket{k=\"v\",le=\"0.05\"} 1\n"), "{out}");
        assert!(out.contains("t_bucket{k=\"v\",le=\"10\"} 1\n"), "{out}");
        assert!(out.contains("t_bucket{k=\"v\",le=\"+Inf\"} 2\n"), "{out}");
        assert!(out.contains("t_sum{k=\"v\"} 20.03\n"), "{out}");
        assert!(out.contains("t_count{k=\"v\"} 2\n"), "{out}");
    }

    #[test]
    fn an_upstream_call_is_counted_by_how_it_ended() {
        let before = REGISTRY.upstream[0]
            .iter()
            .map(|c| c.load(Relaxed))
            .collect::<Vec<_>>();
        let now = Instant::now();
        upstream("tmdb", now, Some(reqwest::StatusCode::OK));
        upstream("tmdb", now, Some(reqwest::StatusCode::NOT_FOUND));
        upstream("tmdb", now, Some(reqwest::StatusCode::TOO_MANY_REQUESTS));
        upstream("tmdb", now, Some(reqwest::StatusCode::BAD_GATEWAY));
        upstream("tmdb", now, None);
        // Nothing counted under a name this does not know.
        upstream("nobody", now, None);
        let after = REGISTRY.upstream[0]
            .iter()
            .map(|c| c.load(Relaxed))
            .collect::<Vec<_>>();
        let grown: Vec<u64> = after.iter().zip(&before).map(|(a, b)| a - b).collect();
        assert_eq!(grown, [2, 1, 1, 1]);
    }

    #[test]
    fn the_text_is_shaped_as_prometheus_reads_it() {
        let text = render(&[
            Metric {
                name: "ams_works",
                help: "Works held, by kind.",
                samples: vec![
                    ("kind=\"series\"".into(), 3.0),
                    ("kind=\"movie\"".into(), 4.0),
                ],
            },
            Metric {
                name: "ams_uptime_seconds",
                help: "Seconds since the server started.",
                samples: vec![(String::new(), 12.5)],
            },
        ]);
        assert!(
            text.starts_with("# HELP ams_http_requests_total "),
            "{text}"
        );
        assert!(text.contains("# TYPE ams_http_requests_total counter\n"));
        assert!(text.contains("ams_http_requests_total{surface=\"native\",status=\"2xx\"} "));
        assert!(text.contains("# TYPE ams_http_request_duration_seconds histogram\n"));
        assert!(
            text.contains("ams_upstream_duration_seconds_bucket{provider=\"tvdb\",le=\"+Inf\"} ")
        );
        assert!(text.contains(
            "# TYPE ams_works gauge\nams_works{kind=\"series\"} 3\nams_works{kind=\"movie\"} 4\n"
        ));
        assert!(text.contains("ams_uptime_seconds 12.5\n"));
        assert_eq!(label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn a_newer_release_is_a_gauge_of_one() {
        let text = render(&[update_available(true)]);
        assert!(text.contains("# TYPE ams_update_available gauge\nams_update_available 1\n"));
        let text = render(&[update_available(false)]);
        assert!(text.contains("ams_update_available 0\n"));
    }

    #[test]
    fn an_answer_is_counted_by_its_status_too() {
        let router = axum::Router::new()
            .route(
                "/api/v1/status-test-418",
                axum::routing::get(|| async { axum::http::StatusCode::IM_A_TEAPOT }),
            )
            .layer(axum::middleware::from_fn(observe));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            use tower::ServiceExt as _;
            for _ in 0..2 {
                router
                    .clone()
                    .oneshot(
                        axum::http::Request::get("/api/v1/status-test-418")
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
            }
        });
        let text = render(&[]);
        assert!(
            text.contains("# TYPE ams_http_responses_total counter\n"),
            "{text}"
        );
        // No other test answers a 418: both are this test's.
        assert!(
            text.contains("ams_http_responses_total{surface=\"native\",status=\"418\"} 2\n"),
            "{text}"
        );
        // A status nobody answered with is not written.
        assert!(!text.contains("status=\"599\""));
    }

    #[test]
    fn a_wait_for_a_connection_is_read_off_the_pool_s_line() {
        use tracing_subscriber::prelude::*;

        let before = REGISTRY.pool_wait.buckets[3].load(Relaxed);
        let subscriber = tracing_subscriber::registry().with(pool_wait_layer());
        tracing::subscriber::with_default(subscriber, || {
            // As sqlx writes it: between one and five milliseconds.
            tracing::trace!(
                target: "sqlx::pool::acquire",
                acquired_after_secs = 0.002_f64,
                "acquired connection"
            );
            // Another target with the same field is not the pool's.
            tracing::trace!(acquired_after_secs = 0.002_f64, "elsewhere");
        });
        assert_eq!(REGISTRY.pool_wait.buckets[3].load(Relaxed), before + 1);

        let text = render(&[]);
        assert!(text.contains("# TYPE ams_db_pool_wait_seconds histogram\n"));
        assert!(text.contains("ams_db_pool_wait_seconds_bucket{le=\"0.005\"} "));
        assert!(text.contains("\nams_db_pool_wait_seconds_count "), "{text}");
    }

    /// What the scrape reads off the state, on either engine: the pool, each
    /// queue, the failed refreshes; no certificate where the server has none.
    #[tokio::test]
    async fn the_scrape_reads_the_pool_the_queues_and_the_failed_refreshes() {
        let state = crate::api::native::testing::server().await;
        let gauges = server(&state).await;
        let named = |name: &str| {
            gauges
                .iter()
                .find(|metric| metric.name == name)
                .unwrap_or_else(|| panic!("{name} is not offered"))
        };
        assert!(named("ams_db_pool_max").samples[0].1 >= 1.0);
        assert!(named("ams_db_pool_size").samples[0].1 >= named("ams_db_pool_idle").samples[0].1);
        let kinds: Vec<&str> = named("ams_queue_depth")
            .samples
            .iter()
            .map(|(labels, _)| labels.as_str())
            .collect();
        assert_eq!(
            kinds,
            ["kind=\"refresh\"", "kind=\"media\"", "kind=\"webhooks\""]
        );
        assert_eq!(named("ams_refresh_failed").samples[0].1, 0.0);
        assert!(gauges.iter().all(|m| m.name != "ams_tls_expiry_seconds"));
    }

    #[test]
    fn a_certificate_s_end_is_read_from_its_pem() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["metadata.test".into()]).unwrap();
        params.not_after = rcgen::date_time_ymd(2031, 1, 2);
        let pem = params.self_signed(&key).unwrap().pem();
        let expected = chrono::NaiveDate::from_ymd_opt(2031, 1, 2)
            .and_then(|day| day.and_hms_opt(0, 0, 0))
            .map(|at| at.and_utc().timestamp());
        assert_eq!(ends(pem.as_bytes()), expected);
        assert_eq!(ends(b"not a certificate"), None);
    }

    #[test]
    fn the_newest_archive_s_age_is_a_gauge_never_below_nought() {
        let text = render(&[backup_age(3600.0)]);
        assert!(
            text.contains("# TYPE ams_backup_age_seconds gauge\nams_backup_age_seconds 3600\n")
        );
        // A clock set back since the archive was made does not make it young.
        let text = render(&[backup_age(-5.0)]);
        assert!(text.contains("ams_backup_age_seconds 0\n"));
    }
}
