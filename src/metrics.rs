//! What the server counts about itself, offered to a Prometheus scrape.
//!
//! A registry of its own rather than a crate: a few counters and histograms
//! with fixed labels, added to without a lock and read in one pass, and the
//! text format is four lines a metric. Requests are counted by the surface
//! they land on — Sonarr's, Radarr's, the TMDB relay, the native API, the
//! interface — and the calls made upstream by the provider asked, each with
//! how long it took. What the catalogue holds, what the caches keep and what
//! the jobs did are read off the state when the scrape comes.

use std::{
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::{Duration, Instant},
};

use axum::{extract::Request, middleware::Next, response::Response};

/// The surfaces a request lands on, in the order they are counted. The
/// liveness probes are a surface of their own, so a monitor does not count
/// as readers.
pub const SURFACES: [&str; 6] = ["sonarr", "radarr", "tmdb", "native", "probe", "ui"];

/// The providers asked upstream, in the order they are counted.
pub const PROVIDERS: [&str; 10] = [
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

/// A histogram of durations: how many fell in each bucket, and their sum.
/// Each observation touches one bucket, and the cumulative counts Prometheus
/// wants are added up from one reading when the scrape comes — so a reading
/// never shows a lower bound counted above a higher one.
pub struct Histogram {
    buckets: [AtomicU64; 12],
    sum_micros: AtomicU64,
}

impl Histogram {
    const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; 12],
            sum_micros: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        let bucket = BOUNDS
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
        for (count, bound) in counts.iter().zip(BOUNDS) {
            seen += count;
            out.push_str(&format!(
                "{name}_bucket{{{labels}{comma}le=\"{bound}\"}} {seen}\n"
            ));
        }
        seen += counts[11];
        out.push_str(&format!(
            "{name}_bucket{{{labels}{comma}le=\"+Inf\"}} {seen}\n"
        ));
        out.push_str(&format!(
            "{name}_sum{{{labels}}} {}\n",
            self.sum_micros.load(Relaxed) as f64 / 1_000_000.0
        ));
        out.push_str(&format!("{name}_count{{{labels}}} {seen}\n"));
    }
}

struct Registry {
    /// Requests answered, by surface and status class.
    requests: [[AtomicU64; 5]; 6],
    request_time: [Histogram; 6],
    /// Calls made upstream, by provider and outcome.
    upstream: [[AtomicU64; 4]; PROVIDERS.len()],
    upstream_time: [Histogram; PROVIDERS.len()],
}

static REGISTRY: Registry = Registry {
    requests: [const { [const { AtomicU64::new(0) }; 5] }; 6],
    request_time: [const { Histogram::new() }; 6],
    upstream: [const { [const { AtomicU64::new(0) }; 4] }; PROVIDERS.len()],
    upstream_time: [const { Histogram::new() }; PROVIDERS.len()],
};

/// Which surface a path lands on.
pub fn surface_of(path: &str) -> usize {
    if path.starts_with("/v1/tvdb/") {
        0
    } else if path.starts_with("/v1/") {
        1
    } else if path.starts_with("/3/") || path.starts_with("/4/") {
        2
    } else if path.starts_with("/api/") {
        3
    } else if path == "/health" || path == "/ready" {
        4
    } else {
        5
    }
}

/// Count a request as it is answered: the surface, the status and the time.
pub async fn observe(request: Request, next: Next) -> Response {
    let surface = surface_of(request.uri().path());
    let started = Instant::now();
    let response = next.run(request).await;
    let class = usize::from(response.status().as_u16() / 100).clamp(1, 5) - 1;
    REGISTRY.requests[surface][class].fetch_add(1, Relaxed);
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

    for metric in gauges {
        out.push_str(&format!("# HELP {} {}\n", metric.name, metric.help));
        out.push_str(&format!("# TYPE {} gauge\n", metric.name));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_lands_on_one_surface() {
        assert_eq!(SURFACES[surface_of("/v1/tvdb/shows/en/81189")], "sonarr");
        assert_eq!(SURFACES[surface_of("/v1/tvdb/search/en")], "sonarr");
        assert_eq!(SURFACES[surface_of("/v1/movie/550")], "radarr");
        assert_eq!(SURFACES[surface_of("/v1/search")], "radarr");
        assert_eq!(SURFACES[surface_of("/3/tv/1396")], "tmdb");
        assert_eq!(SURFACES[surface_of("/4/list/1")], "tmdb");
        assert_eq!(SURFACES[surface_of("/api/v1/items")], "native");
        assert_eq!(SURFACES[surface_of("/api/docs")], "native");
        assert_eq!(SURFACES[surface_of("/health")], "probe");
        assert_eq!(SURFACES[surface_of("/ready")], "probe");
        assert_eq!(SURFACES[surface_of("/")], "ui");
        assert_eq!(SURFACES[surface_of("/assets/index.js")], "ui");
        assert_eq!(SURFACES[surface_of("/work/x")], "ui");
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
}
