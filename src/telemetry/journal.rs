//! The application log, as the administration reads it: the last lines the
//! server wrote, kept in memory, so that what happened is read without
//! `docker logs`.
//!
//! The same lines as standard output — `AMS_LOG` decides both — each with
//! its level, its target, its message and its fields, and the ids of the
//! request it was written in. Every text in them is masked as the log masks
//! a secret ([`super::redact`]), whatever the code that wrote it did. Kept
//! by this process alone: among several instances, each keeps its own.

use std::{
    collections::VecDeque,
    fmt,
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan};
use utoipa::ToSchema;

/// How many lines are kept: the oldest goes as each new one comes.
pub const CAPACITY: usize = 2000;

/// How long a message or a field's value may run, in characters: past it,
/// the rest is cut and an ellipsis says so.
const LONGEST: usize = 2000;

/// How many fields of a line are kept, in the order they were written.
const MOST_FIELDS: usize = 16;

/// What a request's span says of it, for the lines written under it: kept
/// on the span by this layer, and read by the JSON format too.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ids {
    pub request_id: Option<String>,
    pub trace_id: Option<String>,
    /// The method and the masked address, as the request span holds them.
    pub method: Option<String>,
    pub uri: Option<String>,
}

impl Ids {
    /// The trace a line belongs to: the one the request span names, or the
    /// request's own id, which is what the trace is called when this server
    /// started it.
    pub(crate) fn trace(&self) -> Option<&str> {
        self.trace_id
            .as_deref()
            .or(self.request_id.as_deref())
            .filter(|id| !id.is_empty())
    }

    /// `GET /v1/tvdb/shows/en/81189`: what the request asked.
    pub(super) fn request(&self) -> Option<String> {
        match (&self.method, &self.uri) {
            (Some(method), Some(uri)) => Some(format!("{method} {uri}")),
            _ => None,
        }
    }

    fn is_empty(&self) -> bool {
        self.request_id.is_none() && self.trace_id.is_none()
    }
}

impl Visit for Ids {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.take(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if matches!(field.name(), "request_id" | "trace_id" | "method" | "uri") {
            self.take(field, format!("{value:?}"));
        }
    }
}

impl Ids {
    fn take(&mut self, field: &Field, value: String) {
        match field.name() {
            "request_id" => self.request_id = Some(value),
            "trace_id" => self.trace_id = Some(value),
            "method" => self.method = Some(value),
            "uri" => self.uri = Some(value),
            _ => {}
        }
    }
}

/// How grave a line is. In this order: a filter asks for a level and every
/// graver one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
// Named explicitly: a level is many things in this document.
#[schema(as = LogLevel)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    fn of(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::TRACE => Self::Trace,
        }
    }

    /// As the text log writes it: `ERROR`, ` WARN`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

/// One of a line's fields: its name, and its value as text.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[schema(as = LogField)]
pub struct LogField {
    pub name: String,
    pub value: String,
}

/// One line of the log.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = LogEntry)]
pub struct Entry {
    /// Its place among the lines written since the start, one more for each:
    /// what `after` asks to be past.
    pub seq: u64,
    /// When it was written, to the millisecond.
    pub at: String,
    pub level: Level,
    /// The module that wrote it: `arr_metadata_server::jobs::refresh`.
    pub target: String,
    pub message: String,
    pub fields: Vec<LogField>,
    /// The trace of the request it was written in: the request's own id,
    /// unless a proxy or the traces named another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// The id the request was answered with, in `x-request-id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// What the request asked: its method and its address, masked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}

impl Entry {
    /// The line as the text log writes it, for a file to download.
    pub fn as_text(&self) -> String {
        let mut line = format!(
            "{} {:>5} {}: {}",
            self.at,
            self.level.as_str(),
            self.target,
            self.message
        );
        for field in &self.fields {
            line.push(' ');
            line.push_str(&field.name);
            line.push('=');
            line.push_str(&field.value);
        }
        if let Some(request) = &self.request {
            line.push_str(" request=\"");
            line.push_str(request);
            line.push('"');
        }
        if let Some(id) = &self.request_id {
            line.push_str(" request_id=");
            line.push_str(id);
        }
        if let Some(id) = self
            .trace_id
            .as_ref()
            .filter(|id| Some(*id) != self.request_id.as_ref())
        {
            line.push_str(" trace_id=");
            line.push_str(id);
        }
        line
    }

    /// Whether every word of `text` is in what the line says, its ids
    /// included, without regard to case.
    fn says(&self, words: &[String]) -> bool {
        if words.is_empty() {
            return true;
        }
        let mut haystack = format!("{} {} {}", self.level.as_str(), self.target, self.message);
        for field in &self.fields {
            haystack.push(' ');
            haystack.push_str(&field.name);
            haystack.push('=');
            haystack.push_str(&field.value);
        }
        for id in [&self.trace_id, &self.request_id, &self.request]
            .into_iter()
            .flatten()
        {
            haystack.push(' ');
            haystack.push_str(id);
        }
        let haystack = haystack.to_lowercase();
        words.iter().all(|word| haystack.contains(word.as_str()))
    }
}

/// The lines kept, oldest first, and the number the next one takes.
struct Journal {
    lines: Mutex<VecDeque<Entry>>,
    next: AtomicU64,
}

static JOURNAL: LazyLock<Journal> = LazyLock::new(|| Journal {
    lines: Mutex::new(VecDeque::with_capacity(CAPACITY)),
    next: AtomicU64::new(1),
});

impl Journal {
    fn push(&self, mut entry: Entry) {
        let mut lines = self.lines.lock();
        // Numbered under the lock, so the lines kept are in their order.
        entry.seq = self.next.fetch_add(1, Ordering::Relaxed);
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(entry);
    }
}

/// What to read of the journal.
#[derive(Clone, Debug)]
pub struct Query {
    /// This level and every graver one.
    pub level: Level,
    /// Words each line must hold, separated by spaces.
    pub text: Option<String>,
    /// Only the lines written after this one.
    pub after: Option<u64>,
    /// At most this many: the newest.
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            level: Level::Trace,
            text: None,
            after: None,
            limit: CAPACITY,
        }
    }
}

/// The lines that answer `query`, newest first.
pub fn read(query: &Query) -> Vec<Entry> {
    let words: Vec<String> = query
        .text
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let lines = JOURNAL.lines.lock();
    lines
        .iter()
        .rev()
        .take_while(|entry| query.after.is_none_or(|after| entry.seq > after))
        .filter(|entry| entry.level <= query.level && entry.says(&words))
        .take(query.limit)
        .cloned()
        .collect()
}

/// How many lines are kept now.
pub fn held() -> usize {
    JOURNAL.lines.lock().len()
}

/// The layer that keeps the lines, and the ids of the requests' spans.
pub(crate) fn layer() -> Layer {
    Layer
}

pub(crate) struct Layer;

impl<S> tracing_subscriber::Layer<S> for Layer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut ids = Ids::default();
        attrs.record(&mut ids);
        if !ids.is_empty()
            && let Some(span) = ctx.span(id)
        {
            span.extensions_mut().insert(ids);
        }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        if let Some(ids) = extensions.get_mut::<Ids>() {
            values.record(ids);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut said = Said::default();
        event.record(&mut said);
        let ids = ctx.event_scope(event).and_then(|scope| {
            scope
                .into_iter()
                .find_map(|span| span.extensions().get::<Ids>().cloned())
        });
        let meta = event.metadata();
        JOURNAL.push(Entry {
            seq: 0,
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level: Level::of(*meta.level()),
            target: meta.target().to_string(),
            message: masked(&said.message),
            fields: said.fields,
            trace_id: ids.as_ref().and_then(Ids::trace).map(str::to_string),
            request_id: ids.as_ref().and_then(|ids| ids.request_id.clone()),
            request: ids.as_ref().and_then(Ids::request).map(|r| masked(&r)),
        });
    }
}

/// What a line says: its message, and its other fields, each masked.
#[derive(Default)]
pub(super) struct Said {
    pub message: String,
    pub fields: Vec<LogField>,
}

impl Said {
    fn keep(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = value;
        } else if self.fields.len() < MOST_FIELDS {
            self.fields.push(LogField {
                name: field.name().to_string(),
                value: masked(&value),
            });
        }
    }
}

impl Visit for Said {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.keep(field, value.to_string());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        // The whole chain of causes, as `{:#}` writes an `anyhow` error.
        let mut text = value.to_string();
        let mut source = value.source();
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        self.keep(field, text);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.keep(field, format!("{value:?}"));
    }
}

/// A text as the journal keeps it: masked as the log masks a secret, and
/// cut at [`LONGEST`] characters.
pub(super) fn masked(text: &str) -> String {
    // Cut before the masking as well as after it, so that a text of a
    // megabyte is not searched for every secret first.
    let text = super::redact(&clip(text, LONGEST * 4));
    clip(&text, LONGEST)
}

fn clip(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::prelude::*;

    use super::*;

    /// What the journal kept of the lines whose message holds `marker`,
    /// newest first: the journal is the process's, shared by every test.
    fn kept(marker: &str) -> Vec<Entry> {
        read(&Query {
            text: Some(marker.to_string()),
            ..Query::default()
        })
    }

    #[test]
    fn a_line_is_kept_with_its_level_its_fields_and_its_request() {
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!(
                "request",
                method = %"GET",
                uri = %"/v1/tvdb/shows/en/81189",
                request_id = %"0192f5a2c1d07a3b9e4f2c6d8a1b3c5e",
                trace_id = tracing::field::Empty,
            );
            request.in_scope(|| {
                tracing::warn!(provider = "tmdb", attempts = 3, "kept-line-marker slow");
            });
            tracing::error!(error = %"boom", "kept-line-marker outside");
        });

        let lines = kept("kept-line-marker");
        assert_eq!(lines.len(), 2, "{lines:?}");
        let (outside, inside) = (&lines[0], &lines[1]);
        assert!(outside.seq > inside.seq);
        assert_eq!(outside.level, Level::Error);
        assert!(outside.trace_id.is_none() && outside.request.is_none());
        assert_eq!(outside.fields[0].name, "error");
        assert_eq!(outside.fields[0].value, "boom");

        assert_eq!(inside.level, Level::Warn);
        assert_eq!(inside.message, "kept-line-marker slow");
        assert_eq!(
            inside.trace_id.as_deref(),
            Some("0192f5a2c1d07a3b9e4f2c6d8a1b3c5e")
        );
        assert_eq!(
            inside.request.as_deref(),
            Some("GET /v1/tvdb/shows/en/81189")
        );
        let fields: Vec<(&str, &str)> = inside
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str()))
            .collect();
        assert_eq!(fields, [("provider", "tmdb"), ("attempts", "3")]);
        assert!(inside.as_text().contains(" WARN "));
        assert!(
            inside
                .as_text()
                .ends_with("request_id=0192f5a2c1d07a3b9e4f2c6d8a1b3c5e")
        );
    }

    #[test]
    fn nothing_secret_is_kept() {
        super::super::redact_also("journal-test-registered-secret");
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                url = %"https://api.themoviedb.org/3/movie/1?api_key=abcdef123456",
                header = "Authorization: Bearer abcdefghijklmnop",
                "secret-line-marker journal-test-registered-secret postgres://ams:hunter22@db/ams"
            );
        });
        let lines = kept("secret-line-marker");
        let line = lines.first().expect("the line");
        let text = line.as_text();
        for secret in [
            "abcdef123456",
            "abcdefghijklmnop",
            "journal-test-registered-secret",
            "hunter22",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert!(text.contains("api_key=***"), "{text}");
    }

    #[test]
    fn the_journal_is_read_by_level_by_words_and_after_a_line() {
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("levels-marker one");
            tracing::warn!("levels-marker two");
            tracing::error!("levels-marker three");
        });
        let all = kept("levels-marker");
        assert_eq!(all.len(), 3);
        let graver = read(&Query {
            level: Level::Warn,
            text: Some("LEVELS-MARKER".into()),
            ..Query::default()
        });
        let said: Vec<&str> = graver.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(said, ["levels-marker three", "levels-marker two"]);

        let newer = read(&Query {
            text: Some("levels-marker".into()),
            after: Some(all[1].seq),
            ..Query::default()
        });
        assert_eq!(newer.len(), 1);
        assert_eq!(newer[0].message, "levels-marker three");

        let both_words = read(&Query {
            text: Some("levels-marker two".into()),
            ..Query::default()
        });
        assert_eq!(both_words.len(), 1);
    }

    #[test]
    fn a_long_text_is_cut_and_says_so() {
        let long = "x".repeat(LONGEST + 50);
        let cut = masked(&long);
        assert_eq!(cut.chars().count(), LONGEST + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(masked("short"), "short");
    }
}
