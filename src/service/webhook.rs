//! Webhooks: what happened, told to an address the operator names.
//!
//! Every entry the audit trail takes is offered here, and so are the facts
//! nobody did — a work's refresh failing, a medium given up on, a newer
//! release out. Those whose name the operator asked for are posted as JSON
//! to the address set in `webhooks.url`, with a `content` line beside the
//! fields for the services — Discord, Slack and their kind — that show one.
//! Sent from a task of its own, after the entry is written: a webhook that is
//! slow or down costs the action nothing.
//!
//! Two things the trail holds are not passed on. The name typed into a failed
//! sign-in was typed by whoever came to the door, with no credential at all,
//! and is not something to relay to a chat channel; and the webhook address
//! itself is, for a Discord or Slack webhook, the secret that lets anyone
//! post there, so a change to it is told without its value.

use std::{
    collections::HashMap,
    sync::LazyLock,
    time::{Duration, Instant},
};

use serde_json::json;
use tokio::sync::Semaphore;

use crate::{
    db::repo::{self, audit::Action},
    domain::MediaItem,
    outbound,
    state::AppState,
};

/// The actions told of when the operator has not chosen: what enters and
/// leaves the catalogue, not every sign-in and refresh.
pub const DEFAULT_EVENTS: &str = "item.imported,item.created,item.deleted,dataset.imported";

/// What happens without anybody doing it, told as the trail's actions are:
/// by name, with `system` for the actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fact {
    /// A work's refresh failed, where its refreshes had not been failing.
    RefreshFailed,
    /// A medium was given up on, its tries spent.
    MediaFailed,
    /// A release newer than the one running is out.
    UpdateAvailable,
}

impl Fact {
    /// Every fact, in the order the settings offer them.
    pub const ALL: &'static [Fact] = &[
        Self::RefreshFailed,
        Self::MediaFailed,
        Self::UpdateAvailable,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RefreshFailed => "refresh.failed",
            Self::MediaFailed => "media.failed",
            Self::UpdateAvailable => "update.available",
        }
    }
}

/// What a test is posted as. Never one to ask for: it goes to the address
/// whatever `webhooks.events` says, and is no entry of the trail.
pub const TEST_EVENT: &str = "webhook.test";

/// What a test says.
const TEST_DETAIL: &str = "a test, sent from the settings: this address works";

/// Who a fact is told as the doing of.
const SYSTEM: &str = "system";

/// The most of an error a fact carries: enough to say what went wrong,
/// short enough for a line in a chat.
const ERROR_CHARS: usize = 300;

/// How long a webhook is given to answer.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How many deliveries may be under way at once. Past that, an event is
/// dropped from the webhook — never from the trail, which is written first —
/// rather than queued without end behind an address that is not answering.
const MOST_IN_FLIGHT: usize = 8;

/// The deliveries under way.
static IN_FLIGHT: Semaphore = Semaphore::const_new(MOST_IN_FLIGHT);

/// The setting whose value is a secret, and so is never told.
const SECRET_SETTINGS: &[&str] = &["webhooks.url"];

/// One thing that happened, owned, so it can outlive the request it came in.
pub struct Happening {
    pub action: Action,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub detail: Option<String>,
}

/// One thing to post, an action or a fact: its event's name, and the rest
/// as the payload carries it.
struct Told {
    event: &'static str,
    actor: Option<String>,
    target: Option<String>,
    detail: Option<String>,
}

/// Tell the address of this, if it is one of the events asked for. Returns
/// at once; the telling is a task of its own.
pub fn notify(state: &AppState, happening: Happening) {
    let happening = discreet(happening);
    send(
        state,
        Told {
            event: happening.action.as_str(),
            actor: happening.actor,
            target: happening.target,
            detail: happening.detail,
        },
    );
}

/// Tell the address of a fact, if it is one of the events asked for.
/// Returns at once, as [`notify`] does.
pub fn tell(state: &AppState, fact: Fact, target: Option<String>, detail: Option<String>) {
    send(
        state,
        Told {
            event: fact.as_str(),
            actor: Some(SYSTEM.to_string()),
            target,
            detail,
        },
    );
}

/// Whether a fact would be told now: an address set, and the fact among the
/// events asked for. Asked before anything is looked up to tell it with.
pub fn wants(state: &AppState, fact: Fact) -> bool {
    let url = state.settings.resolve("webhooks.url", None, None);
    let events = state.settings.resolve("webhooks.events", None, None);
    destination(url.as_deref(), events.as_deref(), fact.as_str()).is_some()
}

/// A work's refresh failed where its refreshes had not been failing:
/// `refresh.failed`, with the work and the start of what went wrong. Told
/// when a work starts failing and not at each try while it goes on failing,
/// which an outage would turn into a post per work every few hours.
pub fn refresh_failed(state: &AppState, id: &str, work: Option<&MediaItem>, error: &str) {
    if !wants(state, Fact::RefreshFailed) || !first_telling(&TOLD_FAILING, id, Instant::now()) {
        return;
    }
    let named = match work {
        Some(work) => named_work(id, &work.title, work.kind),
        None => id.to_string(),
    };
    tell(
        state,
        Fact::RefreshFailed,
        Some(id.to_string()),
        Some(format!(
            "{named}: {}",
            crate::providers::clip(error, ERROR_CHARS)
        )),
    );
}

/// A medium given up on, its tries spent: `media.failed`, with the address
/// it was fetched from — without its query, which may carry a token — the
/// work that wanted it, by its title while it is held, and what went wrong
/// the last time.
pub async fn media_failed(state: &AppState, asset: &repo::asset::Asset, error: &str) {
    if !wants(state, Fact::MediaFailed) {
        return;
    }
    let address = without_query(&asset.origin);
    let wanted = match &asset.wanted_by {
        Some(id) => {
            let held = repo::item::titles(&state.db, std::slice::from_ref(id))
                .await
                .ok()
                .and_then(|mut titles| titles.remove(id));
            Some(match held {
                Some(work) => named_work(id, &work.title, work.kind),
                None => id.clone(),
            })
        }
        None => None,
    };
    let what = match wanted {
        Some(work) => format!("the {} of {work}", asset.kind.as_str()),
        None => format!("an {}", asset.kind.as_str()),
    };
    tell(
        state,
        Fact::MediaFailed,
        Some(address.clone()),
        Some(format!(
            "{what} was given up on: {} — {address}",
            crate::providers::clip(error, ERROR_CHARS)
        )),
    );
}

/// A newer release: `update.available`, with its version and its page.
pub fn update_available(state: &AppState, release: &crate::jobs::update_check::Release) {
    tell(
        state,
        Fact::UpdateAvailable,
        Some(release.version.clone()),
        Some(format!(
            "{} is out, and this server runs {}: {}",
            release.version,
            env!("CARGO_PKG_VERSION"),
            release.url
        )),
    );
}

/// The works a failed refresh was told of lately, and when. One failure can
/// be seen twice — by the fetch that fell back on the copy held, and then by
/// the sweep that asked for it, each reading the work as it was before — and
/// is told once.
static TOLD_FAILING: LazyLock<parking_lot::Mutex<HashMap<String, Instant>>> =
    LazyLock::new(Default::default);

/// How long a work told of is not told of again.
const TOLD_FOR: Duration = Duration::from_secs(3600);

/// The most works remembered as told of: past that, one not remembered is
/// told, rather than the memory growing with an outage.
const MOST_TOLD: usize = 10_000;

/// Whether `id` is told of now: not told of within [`TOLD_FOR`].
fn first_telling(
    told: &parking_lot::Mutex<HashMap<String, Instant>>,
    id: &str,
    now: Instant,
) -> bool {
    let mut told = told.lock();
    if told
        .get(id)
        .is_some_and(|at| now.duration_since(*at) < TOLD_FOR)
    {
        return false;
    }
    if told.len() >= MOST_TOLD {
        told.retain(|_, at| now.duration_since(*at) < TOLD_FOR);
    }
    if told.len() < MOST_TOLD {
        told.insert(id.to_string(), now);
    }
    true
}

/// A work as a fact names it: `“Title” (series {id})`.
fn named_work(id: &str, title: &str, kind: crate::domain::MediaKind) -> String {
    format!("“{title}” ({} {id})", kind.as_str())
}

/// An address without its query or its fragment.
fn without_query(address: &str) -> String {
    address
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// How a post went.
#[derive(Debug)]
pub enum Delivery {
    /// Answered, with this status, after this long.
    Answered {
        status: reqwest::StatusCode,
        took: Duration,
    },
    /// Not reached, for this reason — in words that name no address —
    /// after this long.
    Unreached { why: String, took: Duration },
    /// Not sent: the address is not one this server posts to.
    Refused(Refusal),
}

/// Why an address is not posted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not an http or https address, or one that names this machine.
    NotWeb,
    /// A name that resolves to an address only reachable from here.
    Internal,
}

impl Refusal {
    /// The code an interface reads.
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotWeb => "not_web",
            Self::Internal => "internal",
        }
    }
}

/// Post a test to the address set, whatever `webhooks.events` says: what
/// came of it, or `None` with no address to post to. `by` is who asked,
/// and the actor the test is posted as.
pub async fn test(state: &AppState, by: &str) -> Option<Delivery> {
    let url = state.settings.resolve("webhooks.url", None, None)?;
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let told = Told {
        event: TEST_EVENT,
        actor: Some(by.to_string()),
        target: None,
        detail: Some(TEST_DETAIL.to_string()),
    };
    Some(deliver(&state.http, url, &told).await)
}

/// Post this, in a task of its own, if the address wants it.
fn send(state: &AppState, told: Told) {
    let url = state.settings.resolve("webhooks.url", None, None);
    let events = state.settings.resolve("webhooks.events", None, None);
    let Some(url) = destination(url.as_deref(), events.as_deref(), told.event) else {
        return;
    };
    let Ok(permit) = IN_FLIGHT.try_acquire() else {
        tracing::warn!(
            event = told.event,
            "the webhook was not told: {MOST_IN_FLIGHT} deliveries are already under way"
        );
        return;
    };
    let http = state.http.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let event = told.event;
        let host = acceptable(&url).unwrap_or_default();
        match deliver(&http, &url, &told).await {
            Delivery::Answered { status, .. } if status.is_success() => {}
            Delivery::Answered { status, .. } => {
                tracing::warn!(host, status = %status, event, "the webhook refused the event");
            }
            Delivery::Unreached { why, .. } => {
                // Never the address itself: it is the webhook's secret.
                tracing::warn!(host, error = %why, event, "the webhook could not be reached");
            }
            Delivery::Refused(Refusal::NotWeb) => {
                tracing::warn!("webhooks.url is not an http(s) address this server will post to");
            }
            Delivery::Refused(Refusal::Internal) => {
                tracing::warn!(
                    host,
                    "webhooks.url resolves to an address only reachable from here"
                );
            }
        }
    });
}

/// The address to tell of an event: the one set, if one is, and if the
/// event is among those asked for — the default ones when none are.
pub fn destination(url: Option<&str>, events: Option<&str>, event: &str) -> Option<String> {
    let url = url.map(str::trim).filter(|u| !u.is_empty())?;
    wanted(events.unwrap_or(DEFAULT_EVENTS), event).then(|| url.to_string())
}

/// Whether an event is among those named — `*` for every one.
pub fn wanted(events: &str, event: &str) -> bool {
    events
        .split(',')
        .map(str::trim)
        .any(|e| e == "*" || e == event)
}

/// The happening with what is not to be told left out: the name typed into
/// a failed sign-in, and the new value of a secret setting.
fn discreet(mut happening: Happening) -> Happening {
    match happening.action {
        Action::SignInFailed => {
            happening.target = None;
            happening.detail = None;
        }
        Action::SettingChanged => {
            // The target is `{scope}:{key}`.
            let key = happening
                .target
                .as_deref()
                .and_then(|t| t.split_once(':'))
                .map(|(_, key)| key);
            if key.is_some_and(|k| SECRET_SETTINGS.contains(&k)) {
                happening.detail = None;
            }
        }
        _ => {}
    }
    happening
}

/// What is posted: the fields, and a one-line `content` and `text` for the
/// services that show one.
fn payload(told: &Told, at: &str) -> serde_json::Value {
    let line = [
        Some(told.event.to_string()),
        told.detail.clone(),
        told.actor.as_ref().map(|a| format!("by {a}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    json!({
        "event": told.event,
        "actor": told.actor,
        "target": told.target,
        "detail": told.detail,
        "at": at,
        "content": format!("Cinémathèque · {line}"),
        "text": format!("Cinémathèque · {line}"),
    })
}

/// Post one, and say how it went.
async fn deliver(http: &reqwest::Client, url: &str, told: &Told) -> Delivery {
    // An address to post to, and not one that only means something from in
    // here: the operator names it, but a setting is a thing that can be
    // guessed at by whoever holds the administration.
    if acceptable(url).is_none() {
        return Delivery::Refused(Refusal::NotWeb);
    }
    // Asked again, of the resolver, just before the post: a name that meant
    // a public address when it was set can mean this machine now.
    if outbound::resolves_internally(url).await {
        return Delivery::Refused(Refusal::Internal);
    }
    let at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let body = payload(told, &at);
    let started = Instant::now();
    match http.post(url).timeout(TIMEOUT).json(&body).send().await {
        Ok(response) => Delivery::Answered {
            status: response.status(),
            took: started.elapsed(),
        },
        // Never `%e`: reqwest's own words end with the address, and the
        // address is the webhook's secret.
        Err(e) => Delivery::Unreached {
            why: crate::providers::describe_request_error(&e),
            took: started.elapsed(),
        },
    }
}

/// The host of an address this server will post to: http or https, and not
/// an address that only means something from where the server stands.
fn acceptable(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || outbound::names_internal_host(url) {
        return None;
    }
    parsed.host_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn happening(action: Action, target: Option<&str>, detail: Option<&str>) -> Happening {
        Happening {
            action,
            actor: Some("admin".into()),
            target: target.map(String::from),
            detail: detail.map(String::from),
        }
    }

    /// What [`notify`] posts for a happening.
    fn told(happening: Happening) -> Told {
        let happening = discreet(happening);
        Told {
            event: happening.action.as_str(),
            actor: happening.actor,
            target: happening.target,
            detail: happening.detail,
        }
    }

    #[test]
    fn only_the_events_asked_for_are_told() {
        assert!(wanted("*", Action::SignedIn.as_str()));
        assert!(wanted(DEFAULT_EVENTS, Action::ItemImported.as_str()));
        assert!(!wanted(DEFAULT_EVENTS, Action::SignedIn.as_str()));
        assert!(wanted(
            " item.deleted , list.created ",
            Action::ListCreated.as_str()
        ));
        assert!(!wanted("", Action::ItemCreated.as_str()));
        // The facts are asked for by name, as an action is, and not by default.
        assert!(wanted("item.created,refresh.failed", "refresh.failed"));
        assert!(wanted("*", Fact::UpdateAvailable.as_str()));
        for fact in Fact::ALL {
            assert!(!wanted(DEFAULT_EVENTS, fact.as_str()), "{}", fact.as_str());
        }
    }

    #[test]
    fn nothing_is_told_without_an_address_and_the_default_events_stand_in() {
        let hook = "https://hooks.example.com/x";
        let imported = Action::ItemImported.as_str();
        assert_eq!(destination(None, None, imported), None);
        assert_eq!(destination(Some("  "), None, imported), None);
        assert_eq!(
            destination(Some(hook), None, imported).as_deref(),
            Some(hook)
        );
        assert_eq!(
            destination(Some(hook), None, Action::SignedIn.as_str()),
            None
        );
        assert_eq!(
            destination(Some(hook), Some("*"), Action::SignedIn.as_str()).as_deref(),
            Some(hook)
        );
        assert_eq!(destination(Some(hook), Some(""), imported), None);
    }

    #[test]
    fn a_fact_never_shares_a_name_with_an_action_or_the_test() {
        let mut names: Vec<&str> = Action::ALL
            .iter()
            .map(|a| a.as_str())
            .chain(Fact::ALL.iter().map(|f| f.as_str()))
            .chain([TEST_EVENT])
            .collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two events share a name");
    }

    #[test]
    fn only_an_ordinary_web_address_is_posted_to() {
        assert_eq!(
            acceptable("https://discord.com/api/webhooks/1/abc").as_deref(),
            Some("discord.com")
        );
        assert!(acceptable("ftp://example.com/x").is_none());
        assert!(acceptable("not a url").is_none());
        assert!(acceptable("http://127.0.0.1:8080/hook").is_none());
        assert!(acceptable("http://169.254.169.254/latest").is_none());
        assert!(acceptable("http://localhost:8479/hook").is_none());
    }

    #[tokio::test]
    async fn a_test_to_an_address_not_posted_to_says_why_and_sends_nothing() {
        let http = reqwest::Client::new();
        let told = Told {
            event: TEST_EVENT,
            actor: Some("admin".into()),
            target: None,
            detail: Some(TEST_DETAIL.into()),
        };
        for refused in ["ftp://example.com/x", "http://127.0.0.1:8080/hook"] {
            assert!(
                matches!(
                    deliver(&http, refused, &told).await,
                    Delivery::Refused(Refusal::NotWeb)
                ),
                "{refused}"
            );
        }
    }

    #[test]
    fn the_name_typed_into_a_failed_sign_in_is_not_relayed() {
        let told = discreet(happening(
            Action::SignInFailed,
            Some("'; DROP TABLE users; -- @everyone"),
            Some("wrong password"),
        ));
        assert_eq!(told.target, None);
        assert_eq!(told.detail, None);
        assert_eq!(told.actor.as_deref(), Some("admin"));

        // A sign-in that succeeded names an account that exists.
        let told = discreet(happening(Action::SignedIn, Some("admin"), None));
        assert_eq!(told.target.as_deref(), Some("admin"));
    }

    #[test]
    fn the_webhook_address_is_never_told_to_itself() {
        let told = discreet(happening(
            Action::SettingChanged,
            Some("server:webhooks.url"),
            Some("https://discord.com/api/webhooks/1/secret"),
        ));
        assert_eq!(told.target.as_deref(), Some("server:webhooks.url"));
        assert_eq!(told.detail, None);

        // Any other setting is told with its value, as the trail holds it.
        let told = discreet(happening(
            Action::SettingChanged,
            Some("server:webhooks.events"),
            Some("*"),
        ));
        assert_eq!(told.detail.as_deref(), Some("*"));
    }

    #[test]
    fn the_payload_carries_the_fields_and_a_line_for_chat() {
        let body = payload(
            &told(happening(
                Action::ItemImported,
                Some("Blade Runner 2099"),
                Some("series 01a0cff7-d8dc-70bc-a6b1-38379ee39bd0"),
            )),
            "2026-09-25T20:00:00Z",
        );
        assert_eq!(body["event"], "item.imported");
        assert_eq!(body["actor"], "admin");
        assert_eq!(body["target"], "Blade Runner 2099");
        assert_eq!(body["at"], "2026-09-25T20:00:00Z");
        assert_eq!(
            body["content"],
            "Cinémathèque · item.imported · series 01a0cff7-d8dc-70bc-a6b1-38379ee39bd0 · by admin"
        );
        assert_eq!(body["content"], body["text"]);

        // Nothing withheld leaves a hole in the line.
        let body = payload(
            &told(happening(Action::SignInFailed, Some("x"), Some("y"))),
            "2026-09-25T20:00:00Z",
        );
        assert_eq!(body["target"], serde_json::Value::Null);
        assert_eq!(
            body["content"],
            "Cinémathèque · auth.sign_in_failed · by admin"
        );
    }

    #[test]
    fn a_fact_is_posted_in_the_same_shape_as_an_action() {
        let body = payload(
            &Told {
                event: Fact::RefreshFailed.as_str(),
                actor: Some(SYSTEM.into()),
                target: Some("01a0cff7-d8dc-70bc-a6b1-38379ee39bd0".into()),
                detail: Some(format!(
                    "{}: TMDB could not be reached",
                    named_work(
                        "01a0cff7-d8dc-70bc-a6b1-38379ee39bd0",
                        "Blade Runner 2099",
                        crate::domain::MediaKind::Series
                    )
                )),
            },
            "2026-10-10T08:00:00Z",
        );
        let mut fields: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "actor", "at", "content", "detail", "event", "target", "text"
            ]
        );
        assert_eq!(body["event"], "refresh.failed");
        assert_eq!(body["actor"], "system");
        assert_eq!(
            body["content"],
            "Cinémathèque · refresh.failed · “Blade Runner 2099” (series \
             01a0cff7-d8dc-70bc-a6b1-38379ee39bd0): TMDB could not be reached · by system"
        );
    }

    #[test]
    fn a_failing_work_is_told_of_once_an_hour_at_most() {
        let told = parking_lot::Mutex::new(HashMap::new());
        let now = Instant::now();
        assert!(first_telling(&told, "a", now));
        // The same failure, seen by the sweep after the fetch.
        assert!(!first_telling(&told, "a", now + Duration::from_secs(1)));
        assert!(first_telling(&told, "b", now));
        // An hour on, a work failing again is news again.
        assert!(first_telling(&told, "a", now + TOLD_FOR));
    }

    #[test]
    fn a_medium_is_named_without_what_its_query_carries() {
        assert_eq!(
            without_query("https://img.example/p/a.jpg?token=secret#x"),
            "https://img.example/p/a.jpg"
        );
        assert_eq!(
            without_query("https://img.example/p/a.jpg"),
            "https://img.example/p/a.jpg"
        );
    }
}
