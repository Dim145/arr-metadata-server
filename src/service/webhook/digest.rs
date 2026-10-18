//! Failed refreshes, told together.
//!
//! A destination that groups them — `grouped`, on by default for the kinds
//! people read — is not posted a message for each work whose refresh failed.
//! The failures wait [`WINDOW`] from the first, then go as one message: how
//! many, counted by cause — the source and the kind of failure — and the
//! first works named. An outage of a source is one message, not one per work
//! it touched; a failure alone in its window goes as it came.
//!
//! Held in memory: what waits when the server stops is not told, and each of
//! several instances groups its own.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use super::kinds::Told;

/// How long the failures wait, from the first, before they are told.
pub const WINDOW: Duration = Duration::from_secs(300);

/// How many failures a waiting message keeps whole; past it, they are
/// counted without their cause.
const MOST_KEPT: usize = 1_000;

/// How many works a message names before saying how many more.
const MOST_NAMED: usize = 8;

/// One failed refresh, as a grouped message counts and names it.
#[derive(Clone, Debug)]
pub struct Failure {
    /// The work as a fact names it: `“Title” (series {id})`.
    pub named: String,
    /// What it failed by: see [`cause`].
    pub cause: String,
}

/// One destination's failures, waiting.
struct Waiting {
    since: Instant,
    /// The first, as it would have been told alone.
    first: Told,
    failures: Vec<Failure>,
    /// Past [`MOST_KEPT`]: counted, not kept.
    more: usize,
}

/// The failures waiting for each destination that groups them, by its id.
#[derive(Default)]
pub struct Digest {
    waiting: parking_lot::Mutex<HashMap<String, Waiting>>,
}

impl Digest {
    /// Hold a failure for a destination that groups them; `told` is the
    /// failure as it would have been told alone.
    pub fn hold(&self, destination: &str, told: &Told, failure: Failure) {
        self.hold_at(destination, told, failure, Instant::now());
    }

    fn hold_at(&self, destination: &str, told: &Told, failure: Failure, now: Instant) {
        let mut waiting = self.waiting.lock();
        let held = waiting
            .entry(destination.to_string())
            .or_insert_with(|| Waiting {
                since: now,
                first: told.clone(),
                failures: Vec::new(),
                more: 0,
            });
        if held.failures.len() < MOST_KEPT {
            held.failures.push(failure);
        } else {
            held.more += 1;
        }
    }

    /// The messages due now — each destination's whose window is past —
    /// taken out of the waiting.
    pub fn due(&self) -> Vec<(String, Told)> {
        self.due_at(Instant::now())
    }

    pub(super) fn due_at(&self, now: Instant) -> Vec<(String, Told)> {
        let mut waiting = self.waiting.lock();
        let ready: Vec<String> = waiting
            .iter()
            .filter(|(_, held)| now.duration_since(held.since) >= WINDOW)
            .map(|(id, _)| id.clone())
            .collect();
        ready
            .into_iter()
            .filter_map(|id| waiting.remove(&id).map(|held| (id, told_of(held))))
            .collect()
    }

    /// How long until the next message falls due, when one waits.
    pub fn until_due(&self) -> Option<Duration> {
        let now = Instant::now();
        self.waiting
            .lock()
            .values()
            .map(|held| WINDOW.saturating_sub(now.duration_since(held.since)))
            .min()
    }
}

/// A destination's failures as one message — or, alone, the one as it came.
fn told_of(held: Waiting) -> Told {
    let total = held.failures.len() + held.more;
    if total <= 1 {
        return held.first;
    }

    let mut counted: HashMap<&str, usize> = HashMap::new();
    for failure in &held.failures {
        *counted.entry(failure.cause.as_str()).or_default() += 1;
    }
    let mut causes: Vec<(&str, usize)> = counted.into_iter().collect();
    causes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    let mut lines = vec![format!("{total} refreshes failed since {}", held.first.at)];
    lines.extend(causes.iter().map(|(cause, n)| format!("{cause}: {n}")));
    if held.more > 0 {
        lines.push(format!("{} more, not counted by cause", held.more));
    }
    let named: Vec<&str> = held
        .failures
        .iter()
        .take(MOST_NAMED)
        .map(|f| f.named.as_str())
        .collect();
    let unnamed = total - named.len();
    lines.push(if unnamed > 0 {
        format!("{}, and {unnamed} more", named.join(", "))
    } else {
        named.join(", ")
    });

    Told {
        event: held.first.event,
        actor: held.first.actor,
        target: None,
        detail: Some(lines.join("\n")),
        at: held.first.at,
        count: Some(u32::try_from(total).unwrap_or(u32::MAX)),
    }
}

/// What a refresh failed by, as a grouped message counts it: each source
/// that did not answer and the kind of failure — `TheTVDB — timed out`,
/// `TMDB — 404 Not Found` — or, for a failure no source is named in, the
/// start of what was said.
pub fn cause(error: &str) -> String {
    const IN_PART: &str = "refreshed in part; kept what these gave before — ";
    let Some(listed) = error.strip_prefix(IN_PART) else {
        return kind(error).unwrap_or_else(|| start_of(error));
    };
    let mut causes: Vec<String> = listed
        .split("; ")
        .map(|part| match part.split_once(": ") {
            Some((source, said)) => format!(
                "{} — {}",
                source_name(source),
                kind(said).unwrap_or_else(|| start_of(said))
            ),
            None => start_of(part),
        })
        .collect();
    causes.sort();
    causes.dedup();
    causes.join(", ")
}

/// The kind of failure a provider's error says, where it says one.
fn kind(said: &str) -> Option<String> {
    let lower = said.to_lowercase();
    let kind = if lower.contains("timed out") {
        "timed out"
    } else if lower.contains("could not connect") {
        "could not connect"
    } else if lower.contains("rate limit") || lower.contains("left alone") {
        "rate limited"
    } else if lower.contains("went unanswered") {
        "a season went unanswered"
    } else {
        return status_in(said);
    };
    Some(kind.to_string())
}

/// An HTTP status as a provider's error writes it — `404 Not Found` — and
/// not any number that happens to be in it.
fn status_in(said: &str) -> Option<String> {
    let words: Vec<&str> = said.split_whitespace().collect();
    words.iter().enumerate().find_map(|(at, word)| {
        let code: u16 = word.parse().ok()?;
        if !(400..600).contains(&code) {
            return None;
        }
        // Its reason, the capitalised words after it: `Not Found`.
        let mut reason = Vec::new();
        for next in &words[at + 1..] {
            let bare = next.trim_end_matches([':', ',', ';', '.']);
            if !bare.starts_with(|c: char| c.is_ascii_uppercase()) {
                break;
            }
            reason.push(bare);
            if bare.len() != next.len() {
                break;
            }
        }
        (!reason.is_empty()).then(|| format!("{code} {}", reason.join(" ")))
    })
}

/// The start of what was said, to count by when nothing better is known.
fn start_of(said: &str) -> String {
    crate::providers::clip(said.trim(), 60)
}

/// A provider's name as people know it.
fn source_name(source: &str) -> &str {
    use crate::providers::names;
    match source.trim() {
        names::TMDB => "TMDB",
        names::TVDB => "TheTVDB",
        names::SKYHOOK => "Skyhook",
        names::RADARR => "Radarr's metadata",
        names::FANART => "Fanart.tv",
        names::TVMAZE => "TVmaze",
        names::ANILIST => "AniList",
        names::MAL => "MyAnimeList",
        names::FANKAI => "Fankai",
        names::FANKAI_WIKI => "the Fankai wiki",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(at: &str, named: &str) -> Told {
        Told {
            event: "refresh.failed".into(),
            actor: Some("system".into()),
            target: Some("01-work".into()),
            detail: Some(format!("{named}: timed out")),
            at: at.into(),
            count: None,
        }
    }

    fn failure(named: &str, cause: &str) -> Failure {
        Failure {
            named: named.into(),
            cause: cause.into(),
        }
    }

    #[test]
    fn a_cause_is_the_source_and_the_kind_of_failure() {
        assert_eq!(
            cause(
                "refreshed in part; kept what these gave before — tvdb: TheTVDB request failed: timed out: operation timed out"
            ),
            "TheTVDB — timed out"
        );
        assert_eq!(
            cause(
                "refreshed in part; kept what these gave before — tmdb: TMDB returned 404 Not Found for /3/tv/1399: {}; skyhook: could not connect: refused"
            ),
            "Skyhook — could not connect, TMDB — 404 Not Found"
        );
        // A number in an address is no status.
        assert_eq!(
            cause(
                "refreshed in part; kept what these gave before — tmdb: no such work at /3/tv/404 here"
            ),
            "TMDB — no such work at /3/tv/404 here"
        );
        // Named by no source: the kind, or the start of what was said.
        assert_eq!(
            cause("TheTVDB asked to be left alone for another 30 s"),
            "rate limited"
        );
        assert_eq!(cause("no provider had it"), "no provider had it");
    }

    #[test]
    fn failures_wait_their_window_then_go_as_one_message_counted_by_cause() {
        let digest = Digest::default();
        let start = Instant::now();
        for (i, cause) in [
            "TheTVDB — timed out",
            "TheTVDB — timed out",
            "TMDB — 404 Not Found",
        ]
        .into_iter()
        .enumerate()
        {
            digest.hold_at(
                "ntfy",
                &told("2026-10-18T02:10:04Z", "first"),
                failure(&format!("“Work {i}” (series {i})"), cause),
                start,
            );
        }

        // Within the window, nothing is due.
        assert!(digest.due_at(start + WINDOW / 2).is_empty());
        assert!(digest.until_due().is_some());

        let due = digest.due_at(start + WINDOW);
        assert_eq!(due.len(), 1);
        let (destination, message) = &due[0];
        assert_eq!(destination, "ntfy");
        assert_eq!(message.event, "refresh.failed");
        assert_eq!(message.count, Some(3));
        assert_eq!(message.target, None);
        assert_eq!(
            message.detail.as_deref(),
            Some(
                "3 refreshes failed since 2026-10-18T02:10:04Z\n\
                 TheTVDB — timed out: 2\n\
                 TMDB — 404 Not Found: 1\n\
                 “Work 0” (series 0), “Work 1” (series 1), “Work 2” (series 2)"
            )
        );

        // Told once: nothing waits any more.
        assert!(digest.due_at(start + WINDOW * 2).is_empty());
        assert_eq!(digest.until_due(), None);
    }

    #[test]
    fn a_failure_alone_in_its_window_goes_as_it_came() {
        let digest = Digest::default();
        let start = Instant::now();
        let alone = told("2026-10-18T02:10:04Z", "“Dark” (series 1)");
        digest.hold_at(
            "slack",
            &alone,
            failure("“Dark” (series 1)", "TMDB — timed out"),
            start,
        );
        assert_eq!(
            digest.due_at(start + WINDOW),
            vec![("slack".to_string(), alone)]
        );
    }

    #[test]
    fn many_failures_name_a_few_and_say_how_many_more() {
        let digest = Digest::default();
        let start = Instant::now();
        for i in 0..20 {
            digest.hold_at(
                "discord",
                &told("2026-10-18T02:10:04Z", "first"),
                failure(&format!("W{i}"), "TVmaze — timed out"),
                start,
            );
        }
        let (_, message) = digest.due_at(start + WINDOW).remove(0);
        let detail = message.detail.unwrap_or_default();
        assert!(
            detail.ends_with("W0, W1, W2, W3, W4, W5, W6, W7, and 12 more"),
            "{detail}"
        );
        assert!(detail.contains("TVmaze — timed out: 20"), "{detail}");
    }
}
