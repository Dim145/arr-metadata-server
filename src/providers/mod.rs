//! Upstream metadata providers.
//!
//! Every provider produces two things: a raw payload, stored verbatim as a
//! snapshot, and a canonical [`crate::domain::MediaItem`] mapped from it. Storing
//! both means a mapping bug can be fixed and replayed without re-fetching.

pub mod anilist;
pub mod fanart;
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
