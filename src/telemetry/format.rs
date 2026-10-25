//! The JSON line, with the id of the trace it was written in at its head.

use tracing::{Event, Subscriber};
use tracing_subscriber::{
    fmt::{FmtContext, FormatEvent, FormatFields, format::Writer},
    registry::LookupSpan,
};

use super::journal::Ids;

/// `tracing_subscriber`'s own JSON line, with `"trace_id"` first: the id of
/// the trace the line was written in — the request's own id unless a proxy
/// or the traces named another (see `RequestSpan`). What a log shipper keys
/// a line on, and what leads from a line to its trace: Loki's derived field
/// reads `"trace_id":"(\w+)"`. A line written outside any request has none.
pub(super) struct Correlated<F>(pub F);

impl<S, N, F> FormatEvent<S, N> for Correlated<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let trace = ctx.event_scope().and_then(|scope| {
            scope.into_iter().find_map(|span| {
                span.extensions()
                    .get::<Ids>()
                    .and_then(Ids::trace)
                    .map(str::to_string)
            })
        });
        let Some(trace) = trace else {
            return self.0.format_event(ctx, writer, event);
        };
        let mut line = String::new();
        self.0.format_event(ctx, Writer::new(&mut line), event)?;
        match line.strip_prefix('{') {
            Some(rest) => {
                let id = serde_json::to_string(&trace).map_err(|_| std::fmt::Error)?;
                write!(writer, "{{\"trace_id\":{id},{rest}")
            }
            None => writer.write_str(&line),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::{fmt, prelude::*};

    use super::*;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> fmt::MakeWriter<'a> for Buffer {
        type Writer = Buffer;
        fn make_writer(&'a self) -> Buffer {
            self.clone()
        }
    }

    /// Every line written in a request carries its trace's id, first, as a
    /// field of the line itself; one written outside carries none.
    #[test]
    fn a_line_names_the_trace_of_the_request_it_was_written_in() {
        let written = Buffer::default();
        let subscriber = tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .json()
                    .flatten_event(true)
                    .map_event_format(Correlated)
                    .with_writer(written.clone()),
            )
            .with(super::super::journal::layer());

        tracing::subscriber::with_default(subscriber, || {
            let plain = tracing::info_span!(
                target: super::super::REQUEST_TARGET,
                "request",
                request_id = "0192f5a2c1d07a3b9e4f2c6d8a1b3c5e",
                trace_id = tracing::field::Empty,
            );
            plain.in_scope(|| tracing::info!(n = 1, "in a request"));

            let traced = tracing::info_span!(
                target: super::super::REQUEST_TARGET,
                "request",
                request_id = "proxy-7",
                trace_id = tracing::field::Empty,
            );
            traced.record("trace_id", "4bf92f3577b34da6a3ce929d0e0e4736");
            traced.in_scope(|| {
                // In a span of its own inside the request: still the request's.
                tracing::info_span!("inner").in_scope(|| tracing::info!("deeper"));
            });

            tracing::info!("outside");
        });

        let text = String::from_utf8(written.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
            .collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(text.lines().next().unwrap().starts_with("{\"trace_id\":"));
        assert_eq!(lines[0]["trace_id"], "0192f5a2c1d07a3b9e4f2c6d8a1b3c5e");
        assert_eq!(lines[0]["message"], "in a request");
        assert_eq!(lines[0]["n"], 1);
        assert_eq!(lines[1]["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert!(lines[2].get("trace_id").is_none(), "{}", lines[2]);
    }
}
