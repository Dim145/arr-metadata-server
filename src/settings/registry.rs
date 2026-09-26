//! Every setting this server has, what it holds, and where it may be set.
//!
//! The list is here rather than spread across the modules that read it, so that
//! one place answers "what can be configured" — which is what the interface
//! renders, what the API documents, and what validation checks against.

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Server,
    /// An API key.
    Client,
    /// An allowlist rule, standing for a client that sends no credential.
    Peer,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
            Self::Peer => "peer",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "server" => Some(Self::Server),
            "client" => Some(Self::Client),
            "peer" => Some(Self::Peer),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Kind {
    Bool,
    /// A whole number, with the range the interface should offer.
    Int {
        min: i64,
        max: i64,
    },
    Text,
    /// One of a fixed set.
    Choice {
        options: &'static [&'static str],
    },
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub key: &'static str,
    pub kind: Kind,
    /// Where this may be set. Anything narrower than the first entry overrides.
    pub scopes: &'static [Scope],
}

const ALL: &[Scope] = &[Scope::Server, Scope::Client, Scope::Peer];
const SERVER_ONLY: &[Scope] = &[Scope::Server];
const PER_CLIENT: &[Scope] = &[Scope::Client, Scope::Peer];

/// The text settings that may be set to nothing, which switches them off:
/// an address to post to is one, a language is not.
const OPTIONAL: &[&str] = &["webhooks.url"];

pub const REGISTRY: &[Definition] = &[
    // ── Answering ────────────────────────────────────────────────────────────
    Definition {
        key: "tmdb.language",
        kind: Kind::Text,
        // Per client because Sonarr asks in its URL and Radarr cannot: a
        // Radarr that wants French needs somebody to say so on its behalf.
        scopes: ALL,
    },
    Definition {
        key: "tmdb.watchRegion",
        // The country whose services a work's page lists — per client and
        // per address too, since a household's key may be used abroad.
        kind: Kind::Text,
        scopes: ALL,
    },
    Definition {
        key: "tmdb.searchLimit",
        kind: Kind::Int { min: 1, max: 50 },
        scopes: SERVER_ONLY,
    },
    // ── Providers ────────────────────────────────────────────────────────────
    Definition {
        key: "tvdb.searchFallback",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "skyhook.fallback",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "skyhook.enrich",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "radarr.fallback",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "radarr.enrich",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    // ── Further sources, each off until turned on ────────────────────────────
    Definition {
        key: "tvmaze.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "anilist.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "mal.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "imdb.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "fankai.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "fankai.wiki",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    // ── Accounts ─────────────────────────────────────────────────────────────
    Definition {
        key: "keys.maxPerUser",
        kind: Kind::Int { min: 0, max: 50 },
        scopes: SERVER_ONLY,
    },
    // ── Access ───────────────────────────────────────────────────────────────
    Definition {
        // Whether the catalogue can be read without signing in. The APIs are
        // not the site: they ask for their credential either way.
        key: "site.access",
        kind: Kind::Choice {
            options: &["private", "public"],
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "registration.mode",
        kind: Kind::Choice {
            options: &["closed", "invite", "approval", "open"],
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        // The role an account gets once an administrator approves it. Never
        // `admin`, and only in approval mode: an open door always opens a
        // member's account, or anybody on the internet could edit.
        key: "registration.role",
        kind: Kind::Choice {
            options: &["member", "editor"],
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "api.sonarr",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "api.radarr",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "api.tmdb",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Members on the TMDB relay, which spends the operator's quota.
        key: "api.tmdbMembers",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Keys on the native API only: the interface goes through its session
        // whatever this says, or switching it off would lock the door on the
        // person who did.
        key: "api.native",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    // ── Refresh ──────────────────────────────────────────────────────────────
    Definition {
        key: "refresh.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "refresh.intervalSeconds",
        // Under a minute is a scheduler that never finishes a sweep before the
        // next one starts; a week is a scheduler that may as well be off.
        kind: Kind::Int {
            min: 60,
            max: 604_800,
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "refresh.batchSize",
        kind: Kind::Int { min: 1, max: 500 },
        scopes: SERVER_ONLY,
    },
    // ── Webhooks ─────────────────────────────────────────────────────────────
    Definition {
        // Where what happens is posted, as JSON; nothing is posted while it is
        // unset.
        key: "webhooks.url",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Which actions of the audit trail are posted: their names, separated
        // by commas, or `*` for every one.
        key: "webhooks.events",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    // ── Adult titles ─────────────────────────────────────────────────────────
    Definition {
        key: "adult.mode",
        kind: Kind::Choice {
            options: &["hidden", "visible"],
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "adult.clientPolicy",
        // `inherit` is what the server says; the other two are a decision made
        // for this client and only this client.
        kind: Kind::Choice {
            options: &["inherit", "allow", "deny"],
        },
        scopes: PER_CLIENT,
    },
    Definition {
        key: "adult.force",
        // When set, `include_adult` from the client is ignored and this
        // server's own answer is used, both towards the providers and when
        // filtering what goes back.
        kind: Kind::Bool,
        scopes: ALL,
    },
];

pub fn find(key: &str) -> Option<&'static Definition> {
    REGISTRY.iter().find(|def| def.key == key)
}

/// Whether the value is one this key can hold.
pub fn validate(def: &Definition, value: &str) -> Result<(), String> {
    match def.kind {
        Kind::Bool => match value {
            "true" | "false" => Ok(()),
            other => Err(format!("{other:?} is not true or false")),
        },
        Kind::Int { min, max } => match value.parse::<i64>() {
            Ok(n) if (min..=max).contains(&n) => Ok(()),
            Ok(n) => Err(format!("{n} is outside {min}–{max}")),
            Err(_) => Err(format!("{value:?} is not a whole number")),
        },
        Kind::Text => {
            if value.trim().is_empty() && !OPTIONAL.contains(&def.key) {
                Err("must not be empty".to_string())
            } else if value.len() > 200 {
                Err("is too long".to_string())
            } else {
                Ok(())
            }
        }
        Kind::Choice { options } => {
            if options.contains(&value) {
                Ok(())
            } else {
                Err(format!("must be one of {}", options.join(", ")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(key: &str) -> &'static Definition {
        find(key).unwrap_or_else(|| panic!("{key} is not in the registry"))
    }

    #[test]
    fn every_key_is_unique() {
        let mut keys: Vec<&str> = REGISTRY.iter().map(|d| d.key).collect();
        let before = keys.len();
        keys.sort_unstable();
        keys.dedup();

        assert_eq!(keys.len(), before, "two definitions share a key");
    }

    #[test]
    fn every_setting_can_be_set_somewhere() {
        for definition in REGISTRY {
            assert!(
                !definition.scopes.is_empty(),
                "{} can be set nowhere",
                definition.key,
            );
        }
    }

    #[test]
    fn a_refresh_interval_has_to_be_survivable() {
        // Thirty seconds would start a sweep before the last one finished.
        assert!(validate(def("refresh.intervalSeconds"), "30").is_err());
        assert!(validate(def("refresh.intervalSeconds"), "900").is_ok());
        assert!(validate(def("refresh.intervalSeconds"), "soon").is_err());
    }

    #[test]
    fn a_choice_is_one_of_its_options() {
        assert!(validate(def("adult.mode"), "hidden").is_ok());
        assert!(validate(def("adult.mode"), "visible").is_ok());
        assert!(validate(def("adult.mode"), "sometimes").is_err());
        assert!(validate(def("webhooks.url"), "").is_ok());
        assert!(validate(def("webhooks.url"), "https://example.com/hook").is_ok());
        assert!(validate(def("tmdb.language"), "").is_err());
    }

    #[test]
    fn a_flag_is_a_flag() {
        assert!(validate(def("refresh.enabled"), "true").is_ok());
        assert!(validate(def("refresh.enabled"), "yes").is_err());
    }

    #[test]
    fn adult_policy_belongs_to_a_client_not_the_server() {
        // The server says whether adult titles exist at all; a client says
        // whether it wants them. Setting the client's answer server-wide would
        // be a second, silent way of saying the first thing.
        assert_eq!(def("adult.clientPolicy").scopes, PER_CLIENT);
        assert_eq!(def("adult.mode").scopes, SERVER_ONLY);
    }
}
