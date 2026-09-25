//! Webhooks: what happened, told to an address the operator names.
//!
//! Every entry the audit trail takes is offered here; those whose action the
//! operator asked for are posted as JSON to the address set in
//! `webhooks.url`, with a `content` line beside the fields for the services
//! — Discord, Slack and their kind — that show one. Sent from a task of its
//! own, after the entry is written: a webhook that is slow or down costs the
//! action nothing.
//!
//! Two things the trail holds are not passed on. The name typed into a failed
//! sign-in was typed by whoever came to the door, with no credential at all,
//! and is not something to relay to a chat channel; and the webhook address
//! itself is, for a Discord or Slack webhook, the secret that lets anyone
//! post there, so a change to it is told without its value.

use std::time::Duration;

use serde_json::json;
use tokio::sync::Semaphore;

use crate::{db::repo::audit::Action, outbound, state::AppState};

/// The actions told of when the operator has not chosen: what enters and
/// leaves the catalogue, not every sign-in and refresh.
pub const DEFAULT_EVENTS: &str = "item.imported,item.created,item.deleted,dataset.imported";

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

/// Tell the address of this, if it is one of the events asked for. Returns
/// at once; the telling is a task of its own.
pub fn notify(state: &AppState, happening: Happening) {
    let url = state.settings.resolve("webhooks.url", None, None);
    let events = state.settings.resolve("webhooks.events", None, None);
    let Some(url) = destination(url.as_deref(), events.as_deref(), happening.action) else {
        return;
    };
    let Ok(permit) = IN_FLIGHT.try_acquire() else {
        tracing::warn!(
            event = happening.action.as_str(),
            "the webhook was not told: {MOST_IN_FLIGHT} deliveries are already under way"
        );
        return;
    };
    let happening = discreet(happening);
    let http = state.http.clone();
    tokio::spawn(async move {
        let _permit = permit;
        deliver(&http, &url, &happening).await;
    });
}

/// The address to tell of an action: the one set, if one is, and if the
/// action is among the events asked for — the default ones when none are.
pub fn destination(url: Option<&str>, events: Option<&str>, action: Action) -> Option<String> {
    let url = url.map(str::trim).filter(|u| !u.is_empty())?;
    wanted(events.unwrap_or(DEFAULT_EVENTS), action).then(|| url.to_string())
}

/// Whether an action is among those named — `*` for every one.
pub fn wanted(events: &str, action: Action) -> bool {
    events
        .split(',')
        .map(str::trim)
        .any(|e| e == "*" || e == action.as_str())
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
fn payload(happening: &Happening, at: &str) -> serde_json::Value {
    let line = [
        Some(happening.action.as_str().to_string()),
        happening.detail.clone(),
        happening.actor.as_ref().map(|a| format!("by {a}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    json!({
        "event": happening.action.as_str(),
        "actor": happening.actor,
        "target": happening.target,
        "detail": happening.detail,
        "at": at,
        "content": format!("Cinémathèque · {line}"),
        "text": format!("Cinémathèque · {line}"),
    })
}

async fn deliver(http: &reqwest::Client, url: &str, happening: &Happening) {
    // An address to post to, and not one that only means something from in
    // here: the operator names it, but a setting is a thing that can be
    // guessed at by whoever holds the administration.
    let Some(host) = acceptable(url) else {
        tracing::warn!("webhooks.url is not an http(s) address this server will post to");
        return;
    };
    // Asked again, of the resolver, just before the post: a name that meant
    // a public address when it was set can mean this machine now.
    if outbound::resolves_internally(url).await {
        tracing::warn!(
            host,
            "webhooks.url resolves to an address only reachable from here"
        );
        return;
    }
    let at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let body = payload(happening, &at);
    match http.post(url).timeout(TIMEOUT).json(&body).send().await {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            tracing::warn!(host, status = %response.status(), event = happening.action.as_str(), "the webhook refused the event");
        }
        Err(e) => {
            tracing::warn!(host, error = %e, event = happening.action.as_str(), "the webhook could not be reached");
        }
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

    #[test]
    fn only_the_events_asked_for_are_told() {
        assert!(wanted("*", Action::SignedIn));
        assert!(wanted(DEFAULT_EVENTS, Action::ItemImported));
        assert!(!wanted(DEFAULT_EVENTS, Action::SignedIn));
        assert!(wanted(" item.deleted , list.created ", Action::ListCreated));
        assert!(!wanted("", Action::ItemCreated));
    }

    #[test]
    fn nothing_is_told_without_an_address_and_the_default_events_stand_in() {
        let hook = "https://hooks.example.com/x";
        assert_eq!(destination(None, None, Action::ItemImported), None);
        assert_eq!(destination(Some("  "), None, Action::ItemImported), None);
        assert_eq!(
            destination(Some(hook), None, Action::ItemImported).as_deref(),
            Some(hook)
        );
        assert_eq!(destination(Some(hook), None, Action::SignedIn), None);
        assert_eq!(
            destination(Some(hook), Some("*"), Action::SignedIn).as_deref(),
            Some(hook)
        );
        assert_eq!(
            destination(Some(hook), Some(""), Action::ItemImported),
            None
        );
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
            &happening(
                Action::ItemImported,
                Some("Blade Runner 2099"),
                Some("series 01a0cff7-d8dc-70bc-a6b1-38379ee39bd0"),
            ),
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
            &discreet(happening(Action::SignInFailed, Some("x"), Some("y"))),
            "2026-09-25T20:00:00Z",
        );
        assert_eq!(body["target"], serde_json::Value::Null);
        assert_eq!(
            body["content"],
            "Cinémathèque · auth.sign_in_failed · by admin"
        );
    }
}
