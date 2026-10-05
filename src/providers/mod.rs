//! Upstream metadata providers.
//!
//! Every provider produces two things: a raw payload, stored verbatim as a
//! snapshot, and a canonical [`crate::domain::MediaItem`] mapped from it. Storing
//! both means a mapping bug can be fixed and replayed without re-fetching.

pub mod anilist;
pub mod fanart;
pub mod fankai;
pub mod fankai_wiki;
pub mod lang;
pub mod mal;
pub mod radarr;
pub mod skyhook;
pub mod tmdb;
pub mod tvdb;
pub mod tvmaze;

/// The most of one provider answer this server will read into memory.
///
/// Generous — TheTVDB's extended document for a long-running anime, with every
/// translation attached, runs to a few megabytes — and finite, which the reads
/// it replaces were not. Every upstream here is a URL an operator can point
/// somewhere else, and a redirected one that answers with a gigabyte was
/// buffered whole and mapped into a row per element.
pub const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// The most of an answer that is not a success this server reads: what it
/// says is wanted for the message, and a redirected upstream answering an
/// error with a gigabyte must not get round [`MAX_BODY_BYTES`] by failing.
pub const ERROR_BODY_BYTES: usize = 64 * 1024;

/// The most of a provider's error that is kept in a message — which ends up in
/// a log line, a work's `refreshError` and a run's history.
pub const MESSAGE_CHARS: usize = 300;

/// A request error as a log may carry it: what kind of failure it was and
/// what lay under it — never the address it was sent to, which carries this
/// server's key in its query.
pub fn describe_request_error(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else if e.is_redirect() {
        "too many redirects"
    } else if e.is_body() || e.is_decode() {
        "the body could not be read"
    } else if e.is_request() {
        "the request could not be sent"
    } else {
        "failed"
    };
    let mut text = kind.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(under) = cause {
        text.push_str(": ");
        text.push_str(&under.to_string());
        cause = under.source();
    }
    text
}

/// What an answer that is not a success says, as a message may carry it: its
/// first [`ERROR_BODY_BYTES`] read and no more, spaces folded, cut to
/// [`MESSAGE_CHARS`]. Empty when it says nothing, or cannot be read.
pub async fn error_text(mut response: reqwest::Response) -> String {
    let mut body = Vec::new();
    while body.len() < ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = ERROR_BODY_BYTES - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&body);
    clip(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        MESSAGE_CHARS,
    )
}

/// `text` cut to `max` characters, marked as cut when it was.
pub fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// A value put into the path of a provider's URL, as one segment of it: a
/// `/`, `?`, `#` or `..` in it is the value's, not the path's.
pub fn segment(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

/// The longest a provider's own word on when to ask again is taken for:
/// past this, its `Retry-After` is read as this.
pub const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a provider that answers 429 without saying for how long is left
/// alone.
const DEFAULT_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a `Retry-After` asks for, bounded: seconds, or a date.
fn retry_after(response: &reqwest::Response) -> Option<std::time::Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    let wait = match value.parse::<u64>() {
        Ok(seconds) => std::time::Duration::from_secs(seconds),
        Err(_) => {
            let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
            (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
                .to_std()
                .unwrap_or_default()
        }
    };
    Some(wait.clamp(std::time::Duration::from_secs(1), MAX_BACKOFF))
}

/// One provider's door: how many of its requests are in flight at once, and
/// how long it is left alone once it has said so.
///
/// A 429 — or a 503 that says when to come back — closes it for as long as
/// its `Retry-After` asks, up to [`MAX_BACKOFF`], for every caller at once:
/// asking again before then only earns another. A caller waits for it to
/// open when that is within [`PATIENCE`], as a paced source's caller waits
/// for its turn, and goes without the provider otherwise. The request that
/// was refused is sent once more after the wait, when the wait is that short.
pub struct Gate {
    /// The provider's name in the metrics: `tmdb`, `tvdb`…
    metric: &'static str,
    /// Its name in a message.
    label: &'static str,
    permits: tokio::sync::Semaphore,
    quiet_until: parking_lot::Mutex<Option<tokio::time::Instant>>,
}

impl Gate {
    pub fn new(metric: &'static str, label: &'static str, at_once: usize) -> Self {
        Self {
            metric,
            label,
            permits: tokio::sync::Semaphore::new(at_once.max(1)),
            quiet_until: parking_lot::Mutex::new(None),
        }
    }

    /// Send what `request` builds once the door is open, holding one of its
    /// places until the permit returned with the answer is dropped — after
    /// the body is read, so the cap covers the whole exchange. Errors carry
    /// the kind of failure, never the address.
    pub async fn send(
        &self,
        request: impl Fn() -> reqwest::RequestBuilder,
    ) -> anyhow::Result<(reqwest::Response, tokio::sync::SemaphorePermit<'_>)> {
        let mut retried = false;
        loop {
            self.wait_out().await?;
            let permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| anyhow::anyhow!("{} is shut", self.label))?;

            let started = std::time::Instant::now();
            let sent = request().send().await;
            crate::metrics::upstream(self.metric, started, sent.as_ref().ok().map(|r| r.status()));
            let response = sent.map_err(|e| anyhow::anyhow!(describe_request_error(&e)))?;

            match self.back_off(&response) {
                Some(wait) if !retried && wait <= PATIENCE => {
                    tracing::debug!(
                        provider = self.metric,
                        wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                        "the provider asked to be left alone; asking again after the wait"
                    );
                    retried = true;
                    drop(permit);
                    continue;
                }
                _ => return Ok((response, permit)),
            }
        }
    }

    /// Wait for the door to open, or say why not when that is too far off.
    async fn wait_out(&self) -> anyhow::Result<()> {
        let until = *self.quiet_until.lock();
        let Some(until) = until else { return Ok(()) };
        let now = tokio::time::Instant::now();
        if until <= now {
            return Ok(());
        }
        if until - now > PATIENCE {
            anyhow::bail!(
                "{} asked to be left alone for another {} s; this fetch goes without it",
                self.label,
                (until - now).as_secs().max(1)
            );
        }
        tokio::time::sleep_until(until).await;
        Ok(())
    }

    /// Close the door for as long as a 429, or a 503 that says when, asks.
    /// The wait, when it was such an answer.
    fn back_off(&self, response: &reqwest::Response) -> Option<std::time::Duration> {
        let wait = match response.status() {
            reqwest::StatusCode::TOO_MANY_REQUESTS => {
                retry_after(response).unwrap_or(DEFAULT_BACKOFF)
            }
            reqwest::StatusCode::SERVICE_UNAVAILABLE => retry_after(response)?,
            _ => return None,
        };
        let until = tokio::time::Instant::now() + wait;
        let mut quiet = self.quiet_until.lock();
        if quiet.is_none_or(|current| current < until) {
            *quiet = Some(until);
        }
        tracing::warn!(
            provider = self.metric,
            status = response.status().as_u16(),
            wait_s = wait.as_secs(),
            "the provider asked to be left alone"
        );
        Some(wait)
    }
}

/// Read a provider response as JSON, refusing one that will not fit.
///
/// Streamed rather than trusting `Content-Length`, which a chunked answer does
/// not carry and a careless one can understate.
pub async fn read_json(response: reqwest::Response) -> anyhow::Result<serde_json::Value> {
    let body = read_body(response, MAX_BODY_BYTES).await?;
    Ok(serde_json::from_slice(&body)?)
}

/// Read a response body whole, refusing one larger than `limit` bytes.
pub async fn read_body(response: reqwest::Response, limit: u64) -> anyhow::Result<Vec<u8>> {
    use futures::StreamExt as _;

    if response.content_length().is_some_and(|n| n > limit) {
        anyhow::bail!("the answer is larger than {limit} bytes");
    }

    let mut body = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;

        if body.len() as u64 + chunk.len() as u64 > limit {
            anyhow::bail!("the answer is larger than {limit} bytes");
        }

        body.extend_from_slice(&chunk);
    }

    Ok(body)
}

/// How long a fetch waits for a turn with a paced source before going without
/// it.
///
/// Those sources only ever add to a work, and one queue serves the whole
/// process: a Radarr bulk refresh can put a hundred lookups in it at once, and
/// the thirtieth anime film behind AniList's two-second spacing would wait past
/// the server's own request timeout — failing the whole batch for the sake of
/// a score. Past this, the fetch goes ahead without that source, and the next
/// refresh asks again.
pub const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// Keeps calls to one provider at least `gap` apart.
///
/// The sources added later publish their limits — TVmaze twenty calls in ten
/// seconds, AniList thirty a minute at the moment, Jikan three a second — and
/// answer 429 past them. Spacing calls out keeps under the limit instead of
/// discovering it. Each caller reserves its slot and then waits for it, so
/// callers go in order, and one that would have to wait too long can decline
/// without taking a slot from anybody.
pub struct Pacer {
    gap: std::time::Duration,
    next: parking_lot::Mutex<tokio::time::Instant>,
}

impl Pacer {
    pub fn new(gap: std::time::Duration) -> Self {
        Self {
            gap,
            next: parking_lot::Mutex::new(tokio::time::Instant::now()),
        }
    }

    /// Wait for a turn covering `calls` requests — a request that is answered
    /// with a redirect is two — unless it would start more than `patience`
    /// from now. Returns whether the turn was taken.
    pub async fn turn(&self, calls: u32, patience: std::time::Duration) -> bool {
        let start = {
            let mut next = self.next.lock();
            let now = tokio::time::Instant::now();
            let start = std::cmp::max(now, *next);

            if start - now > patience {
                return false;
            }

            *next = start + self.gap * calls.max(1);
            start
        };

        tokio::time::sleep_until(start).await;
        true
    }
}

/// A title a provider gives, as an alternative one — or nothing, when it is
/// blank.
pub fn alternative_title(
    title: &str,
    title_type: &str,
    language: Option<&str>,
) -> Option<crate::domain::AlternativeTitle> {
    let title = title.trim();

    (!title.is_empty()).then(|| crate::domain::AlternativeTitle {
        id: crate::db::new_id(),
        title: title.to_string(),
        title_type: Some(title_type.to_string()),
        language: language.map(String::from),
        is_manual: false,
    })
}

/// Text out of the small HTML a provider puts in a synopsis.
///
/// TVmaze wraps every summary in `<p>` and AniList uses `<br>` and `<i>`. None
/// of it is rendered anywhere — Sonarr and Radarr show the field as text, and
/// so does the interface — so the markup would be shown literally. Paragraph
/// and line breaks become line breaks; every other tag goes; the entities these
/// sources actually emit are decoded.
pub fn plain_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            // A lone `<` is text, not a tag.
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };

        let tag = rest[open + 1..open + close].trim().to_ascii_lowercase();
        let name = tag
            .trim_start_matches('/')
            .split([' ', '/'])
            .next()
            .unwrap_or("");
        if matches!(name, "br" | "p" | "div" | "li") && !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }

        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);

    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");

    decoded
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Names used in `media_provider_snapshot.provider` and in merge priority.
pub mod names {
    pub const TMDB: &str = "tmdb";
    pub const SKYHOOK: &str = "skyhook";
    pub const RADARR: &str = "radarr";
    pub const FANART: &str = "fanart";
    pub const TVDB: &str = "tvdb";
    pub const TVMAZE: &str = "tvmaze";
    pub const ANILIST: &str = "anilist";
    pub const MAL: &str = "mal";
    pub const IMDB: &str = "imdb";
    pub const FANKAI: &str = "fankai";
    pub const FANKAI_WIKI: &str = "fankaiwiki";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_in_a_synopsis_becomes_text() {
        assert_eq!(
            plain_text("<p>A <b>chemistry</b> teacher &amp; his student.</p>"),
            "A chemistry teacher & his student."
        );
        assert_eq!(
            plain_text("First line.<br><br>Second line.<br />(Source: AniList)"),
            "First line.\nSecond line.\n(Source: AniList)"
        );
        assert_eq!(plain_text("<p>One.</p><p>Two.</p>"), "One.\nTwo.");
        assert_eq!(plain_text("It&#039;s 3 &lt; 4"), "It's 3 < 4");
        // Not a tag, just a character.
        assert_eq!(plain_text("x < y"), "x < y");
        assert_eq!(plain_text(""), "");
    }

    #[test]
    fn a_message_is_cut_where_a_character_ends() {
        assert_eq!(clip("  short  ", 10), "short");
        assert_eq!(clip("ééééé", 3), "ééé…");
        assert_eq!(clip("abc", 3), "abc");
    }

    #[test]
    fn a_value_in_a_path_is_one_segment_of_it() {
        assert_eq!(segment("eng"), "eng");
        assert_eq!(segment("../login?x=1#y"), "..%2Flogin%3Fx%3D1%23y");
        assert_eq!(segment("ur12/34"), "ur12%2F34");
    }

    /// A server that answers each connection with the next of `answers`, as
    /// written, and counts what it was asked.
    async fn answering(
        answers: Vec<&'static str>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        tokio::spawn(async move {
            for answer in answers {
                let Ok((mut connection, _)) = listener.accept().await else {
                    return;
                };
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut request = [0u8; 4096];
                let _ = connection.read(&mut request).await;
                let _ = connection.write_all(answer.as_bytes()).await;
                let _ = connection.shutdown().await;
            }
        });
        (at, asked)
    }

    const TOO_MANY: &str = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const FINE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";

    #[tokio::test]
    async fn a_provider_that_asks_for_a_second_is_asked_again_after_it() {
        let (at, asked) = answering(vec![TOO_MANY, FINE]).await;
        let gate = Gate::new("tmdb", "TMDB", 4);
        let http = reqwest::Client::new();

        let started = std::time::Instant::now();
        let (response, _permit) = gate.send(|| http.get(&at)).await.unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    }

    #[tokio::test]
    async fn a_provider_that_asks_for_longer_than_patience_is_gone_without_and_not_asked() {
        const LONG: &str = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (at, asked) = answering(vec![LONG, FINE]).await;
        let gate = Gate::new("tvdb", "TheTVDB", 4);
        let http = reqwest::Client::new();

        // Answered as it came: the caller reads the 429.
        let (response, permit) = gate.send(|| http.get(&at)).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
        drop(permit);

        // An hour, read as a minute: the next caller does not knock.
        let refused = gate.send(|| http.get(&at)).await.unwrap_err();
        assert!(refused.to_string().contains("left alone"), "{refused}");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        let quiet = gate.quiet_until.lock().unwrap();
        assert!(quiet - tokio::time::Instant::now() <= MAX_BACKOFF);
    }

    #[tokio::test]
    async fn an_error_body_is_read_to_a_point_and_cut() {
        let long = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            200_000,
            "oops  \n".repeat(200_000 / 7) + &" ".repeat(200_000 % 7)
        );
        let long: &'static str = Box::leak(long.into_boxed_str());
        let (at, _) = answering(vec![long]).await;
        let response = reqwest::get(&at).await.unwrap();
        let text = error_text(response).await;
        assert!(text.starts_with("oops oops"));
        assert!(text.chars().count() <= MESSAGE_CHARS + 1);
        assert!(text.ends_with('…'));
    }

    const GAP: std::time::Duration = std::time::Duration::from_millis(500);

    #[tokio::test(start_paused = true)]
    async fn calls_are_spaced_by_the_gap() {
        let pacer = Pacer::new(GAP);
        let started = tokio::time::Instant::now();

        for _ in 0..3 {
            assert!(pacer.turn(1, PATIENCE).await);
        }

        // The first goes at once; the next two wait half a second each.
        assert_eq!(started.elapsed(), GAP * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_redirected_request_takes_two_slots() {
        let pacer = Pacer::new(GAP);
        let started = tokio::time::Instant::now();

        assert!(pacer.turn(2, PATIENCE).await);
        assert!(pacer.turn(1, PATIENCE).await);

        assert_eq!(started.elapsed(), GAP * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_caller_that_would_wait_too_long_goes_without_and_costs_nobody() {
        let pacer = Pacer::new(GAP);

        // Four slots taken: the next free one is two seconds away.
        assert!(pacer.turn(4, PATIENCE).await);
        let asked = tokio::time::Instant::now();

        assert!(!pacer.turn(1, std::time::Duration::from_secs(1)).await);
        assert_eq!(
            asked.elapsed(),
            std::time::Duration::ZERO,
            "it did not wait"
        );

        // Declining reserved nothing, so the next caller is not pushed back.
        assert!(pacer.turn(1, PATIENCE).await);
        assert_eq!(asked.elapsed(), GAP * 4);
    }

    #[tokio::test(start_paused = true)]
    async fn callers_waiting_at_once_are_spread_out() {
        let pacer = std::sync::Arc::new(Pacer::new(GAP));
        let started = tokio::time::Instant::now();

        let waits = (0..3).map(|_| {
            let pacer = pacer.clone();
            tokio::spawn(async move {
                pacer.turn(1, PATIENCE).await;
                started.elapsed()
            })
        });
        let mut ends: Vec<_> = futures::future::join_all(waits)
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect();
        ends.sort();

        assert_eq!(ends, [std::time::Duration::ZERO, GAP, GAP * 2]);
    }
}
