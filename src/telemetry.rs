//! Tracing setup.

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Initialise the global subscriber.
///
/// `AMS_LOG` (or `RUST_LOG`) sets the filter. `AMS_LOG_FORMAT=json` switches to
/// structured output for log shippers.
pub fn init() {
    let filter = EnvFilter::try_from_env("AMS_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,arr_metadata_server=info,tower_http=info"));

    let json = std::env::var("AMS_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    let registry = tracing_subscriber::registry().with(filter);

    if json {
        registry
            .with(fmt::layer().json().flatten_event(true))
            .init();
    } else {
        registry.with(fmt::layer().with_target(true)).init();
    }
}
