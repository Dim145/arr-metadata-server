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
// Named explicitly: an asset's kind is a `Kind` too, and utoipa keys schemas
// on the leaf name alone.
#[schema(as = SettingKind)]
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
    /// Text that is written and never read back: the settings API shows it
    /// masked and the journal says only that it changed.
    Secret,
}

impl Kind {
    pub fn is_secret(&self) -> bool {
        matches!(self, Kind::Secret)
    }
}

/// What a secret is shown as, once set.
pub const MASK: &str = "••••••••";

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
const OPTIONAL: &[&str] = &[
    "oidc.issuer",
    "oidc.clientId",
    "oidc.buttonLabel",
    "oidc.roleClaim",
    "oidc.adminValues",
    "oidc.editorValues",
    // No address: that one is not connected.
    "arr.sonarrUrl",
    "arr.radarrUrl",
];

/// The text settings that may run longer than a line, and how long. None
/// today: the webhook's events, which did, are each destination's own now,
/// kept with it on the Notifications page.
const LONG: &[(&str, usize)] = &[];

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
    Definition {
        // The cast kept of TMDB's, top of the bill first: a long-running
        // series lists hundreds, each a row and a portrait to fetch.
        key: "tmdb.castLimit",
        kind: Kind::Int {
            min: 1,
            max: crate::providers::tmdb::map::MAX_CAST as i64,
        },
        scopes: SERVER_ONLY,
    },
    // ── Providers ────────────────────────────────────────────────────────────
    Definition {
        key: "tvdb.searchFallback",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // TheTVDB's characters kept of a series, in its order: matched into
        // the cast another source gave, or the cast where none did. Unset,
        // as many as `tmdb.castLimit` begins at (`providers::tvdb::cast_limit`).
        key: "tvdb.castLimit",
        kind: Kind::Int {
            min: 1,
            max: crate::providers::tmdb::map::MAX_CAST as i64,
        },
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
        key: "sonarr.sceneMappings",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "sonarr.sceneMappingSearch",
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
    // ── The operator's own Sonarr and Radarr, reached the other way ──────────
    // Set on their page, which checks an address as one before keeping it.
    Definition {
        // Where this server reaches them — `http://sonarr:8989`, with the URL
        // base when there is one — to tell them of a change and read what
        // they hold.
        key: "arr.sonarrUrl",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "arr.sonarrKey",
        kind: Kind::Secret,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "arr.radarrUrl",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "arr.radarrKey",
        kind: Kind::Secret,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Ask them to read a work again once it changes here, rather than at
        // their next refresh; off until turned on.
        key: "arr.pushRefresh",
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
        // On by default, unlike the switches around it: the next way into
        // MyAnimeList, when the one asked does not answer — Tenrai after the
        // official API, then a Jikan of one's own, the public one having
        // closed on 2026-10-01.
        key: "mal.fallback",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "imdb.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // IMDb's titles by region, read weekly; Radarr is given those of
        // `imdb.regions`.
        key: "imdb.akas",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // IMDb's episodes, read weekly: an episode's IMDb id and rating.
        key: "imdb.episodes",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // The regions whose titles a Radarr is given — per key and per
        // address too, as a household's languages differ. Unset, the
        // country of the language it is answered in.
        key: "imdb.regions",
        kind: Kind::Text,
        scopes: ALL,
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
        key: "api.tvdb",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "api.anilist",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // The metadata provider Plex Media Server is given by address.
        key: "api.plex",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    // ── Signing in through an identity provider ──────────────────────────────
    Definition {
        key: "oidc.enabled",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // The provider's issuer URL; its discovery document is read from it.
        key: "oidc.issuer",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "oidc.clientId",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        // AMS_OIDC_CLIENT_SECRET wins over this when it is set.
        key: "oidc.clientSecret",
        kind: Kind::Secret,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Space-separated; `openid` is always asked for.
        key: "oidc.scopes",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "oidc.buttonLabel",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Open an account for someone the provider vouches for and who has
        // none here yet.
        key: "oidc.autoRegister",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // The claim roles are read from — `groups`, `realm_access.roles` — and
        // the values that make an administrator or an editor. With a claim
        // named, the provider decides the role at every sign-in.
        key: "oidc.roleClaim",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "oidc.adminValues",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "oidc.editorValues",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Off, only the identity provider signs people in — except the
        // account AMS_ADMIN_USERNAME names, which keeps a door.
        key: "auth.passwordLogin",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Members on the relays — TMDB's, TheTVDB's and AniList's — which
        // spend the operator's quotas.
        key: "api.tmdbMembers",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Whose key the TMDB relay asks with: this server's, in place of
        // whatever the client sent, or the client's own, handed on as it
        // came — a key issued here is then refused, and the address list
        // guards the relay instead of a key.
        key: "tmdb.relayCredential",
        kind: Kind::Choice {
            options: &["server", "client"],
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        // The same for the TheTVDB relay. AMS_TVDB_AUTH still says how the
        // surface is guarded; `client` reads its key policy as the address
        // list, as it does AMS_TMDB_AUTH's.
        key: "tvdb.relayCredential",
        kind: Kind::Choice {
            options: &["server", "client"],
        },
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
    Definition {
        // The MCP server, `/mcp`: off until an administrator turns it on.
        key: crate::mcp::SETTING,
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    // ── Media ────────────────────────────────────────────────────────────────
    // Each cache space, switchable from the Cache page: off, it answers
    // nothing and keeps nothing, and what it held expires.
    Definition {
        key: "cache.items",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "cache.searches",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "cache.lists",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "cache.relay",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        key: "cache.sessions",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Fetch and keep the media a work points at, as it is stored.
        key: "media.store",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // Through this server, or sent to the bucket's own address.
        key: "media.serve",
        kind: Kind::Choice {
            options: &["proxy", "redirect"],
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        // The cast's photographs too: one per person, many per work.
        key: "media.people",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // The themes: a Fan-Kai's, a few megabytes each.
        key: "media.audio",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // The night's AVIF copies of the posters and the backgrounds: off
        // until somebody turns them on, as they cost seconds of a processor
        // each.
        key: "media.avif",
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
    Definition {
        // How long a work still changing — a running series, a film not out
        // yet — keeps before it is fetched again, in seconds: a minute to a
        // year. Read when the work is scheduled, so a change applies from the
        // next refresh of each. TMDB's terms allow its answers to be kept six
        // months at the most, so past 180 days (`service::SIX_MONTHS`) the
        // sweep asks TMDB again anyway, by a light refresh of it alone,
        // whatever either of these says.
        key: "refresh.ttlContinuing",
        kind: Kind::Int {
            min: 60,
            max: 31_536_000,
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        // And a work done changing — an ended series, a released film: a
        // minute to ten years, the six months TMDB allows holding here too.
        key: "refresh.ttlEnded",
        kind: Kind::Int {
            min: 60,
            max: 315_360_000,
        },
        scopes: SERVER_ONLY,
    },
    // ── Sources ──────────────────────────────────────────────────────────────
    Definition {
        // The order the merge takes the providers in, most trusted first: their
        // names, separated by commas, as the Sources page puts them in order. A
        // provider left out comes after the rest. Read by every merge.
        key: "sources.priority",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    // ── Numbering ────────────────────────────────────────────────────────────
    Definition {
        // How many refreshes in a row, most of a day apart, TheTVDB may leave a
        // series unanswered before TMDB numbers it in its place; `0`, never.
        // Read at every refresh of a series (`service::gather`).
        key: "numbering.fallbackAfter",
        kind: Kind::Int { min: 0, max: 30 },
        scopes: SERVER_ONLY,
    },
    // ── Journal ──────────────────────────────────────────────────────────────
    Definition {
        // How many days each work's journal keeps a change, `0` for ever: a
        // year unless set (`jobs::refresh::HISTORY_KEEP_DAYS`), ten at the
        // most. Read by the pass that prunes the audit trail.
        key: "history.keepDays",
        kind: Kind::Int { min: 0, max: 3650 },
        scopes: SERVER_ONLY,
    },
    // ── Merges ───────────────────────────────────────────────────────────────
    Definition {
        // For how many days a merge of two works can be undone, `0` for not
        // at all: two weeks unless set (`service::merge::DEFAULT_UNDO_DAYS`),
        // a year at the most. Read when an undo is asked for.
        key: "merge.undoDays",
        kind: Kind::Int { min: 0, max: 365 },
        scopes: SERVER_ONLY,
    },
    // Webhooks were two settings here, an address and its events, until
    // they became destinations of their own: see `service::webhook`.
    // ── Updates ──────────────────────────────────────────────────────────────
    Definition {
        // GitHub asked daily for the latest release; off until turned on.
        key: "updates.check",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    // ── Backups ──────────────────────────────────────────────────────────────
    Definition {
        // How often the server makes an archive of itself, counted from the
        // newest one: see `jobs::backup`.
        key: "backup.schedule",
        kind: Kind::Choice {
            options: &["off", "daily", "weekly", "monthly"],
        },
        scopes: SERVER_ONLY,
    },
    Definition {
        // How many of the schedule's own archives are kept: one at the
        // least, a year of daily ones at the most.
        key: "backup.keep",
        kind: Kind::Int { min: 1, max: 365 },
        scopes: SERVER_ONLY,
    },
    Definition {
        // Whether the schedule's archives carry the media kept on disk.
        key: "backup.includeMedia",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    Definition {
        // How many archives made before upgrades are kept, apart from the
        // schedule's: see `backup::Backups::before_upgrade`.
        key: "backup.keepUpgrades",
        kind: Kind::Int { min: 1, max: 20 },
        scopes: SERVER_ONLY,
    },
    Definition {
        // Whether a PostgreSQL is archived before a start applies
        // migrations; an SQLite file always is.
        key: "backup.beforeUpgrade",
        kind: Kind::Bool,
        scopes: SERVER_ONLY,
    },
    // ── Error reports ────────────────────────────────────────────────────────
    // Each error the server writes, sent to a Sentry or a GlitchTip: off until
    // a DSN is set, and taken at once when one is (`telemetry::report`).
    Definition {
        key: "sentry.dsn",
        kind: Kind::Secret,
        scopes: SERVER_ONLY,
    },
    Definition {
        // What the reports are filed under: `production` to begin with.
        key: "sentry.environment",
        kind: Kind::Text,
        scopes: SERVER_ONLY,
    },
    Definition {
        // The share of the errors sent, in percent: all of them to begin with.
        key: "sentry.sampleRate",
        kind: Kind::Int { min: 0, max: 100 },
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
            let most = LONG
                .iter()
                .find(|(key, _)| *key == def.key)
                .map_or(200, |(_, most)| *most);
            if value.trim().is_empty() && !OPTIONAL.contains(&def.key) {
                Err("must not be empty".to_string())
            } else if value.len() > most {
                Err("is too long".to_string())
            } else if def.key == "sources.priority" {
                // Providers the merge knows, each once: a name it does not
                // know would be passed over without a word.
                crate::merge::rules::parse_priority(value).map(|_| ())
            } else if def.key == "imdb.regions" {
                // Read as Radarr is answered: a region misspelt would match
                // no title, and say nothing of it.
                crate::service::imdb::parse_regions(value).map(|_| ())
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
        Kind::Secret => {
            if value.contains(MASK) {
                Err("is the mask a stored secret is shown as, not a secret".to_string())
            } else if value.trim().is_empty() {
                Err("must not be empty; clear it instead".to_string())
            } else if value.len() > 1000 {
                Err("is too long".to_string())
            } else if def.key == "sentry.dsn" {
                // Refused here rather than found unreadable once reports fail.
                crate::telemetry::report::check_dsn(value)
            } else {
                Ok(())
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
    fn thetvdbs_cast_is_cut_where_tmdbs_is() {
        assert_eq!(def("tvdb.castLimit").scopes, SERVER_ONLY);
        assert!(validate(def("tvdb.castLimit"), "1").is_ok());
        assert!(validate(def("tvdb.castLimit"), "200").is_ok());
        assert!(validate(def("tvdb.castLimit"), "0").is_err());
        assert!(validate(def("tvdb.castLimit"), "201").is_err());
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
        assert!(validate(def("oidc.issuer"), "").is_ok());
        assert!(validate(def("tmdb.language"), "").is_err());
    }

    #[test]
    fn a_text_keeps_to_a_line_and_the_webhook_is_no_setting_now() {
        assert!(validate(def("tmdb.language"), &"x".repeat(201)).is_err());
        // Its address and events are each destination's own.
        assert!(find("webhooks.url").is_none());
        assert!(find("webhooks.events").is_none());
    }

    #[test]
    fn the_update_check_is_the_server_s_and_a_flag() {
        assert_eq!(def("updates.check").scopes, SERVER_ONLY);
        assert!(validate(def("updates.check"), "false").is_ok());
        assert!(validate(def("updates.check"), "daily").is_err());
    }

    #[test]
    fn the_backups_are_the_server_s_and_kept_within_reason() {
        for key in [
            "backup.schedule",
            "backup.keep",
            "backup.includeMedia",
            "backup.keepUpgrades",
            "backup.beforeUpgrade",
        ] {
            assert_eq!(def(key).scopes, SERVER_ONLY, "{key}");
        }
        for schedule in ["off", "daily", "weekly", "monthly"] {
            assert!(
                validate(def("backup.schedule"), schedule).is_ok(),
                "{schedule}"
            );
        }
        assert!(validate(def("backup.schedule"), "hourly").is_err());
        assert!(validate(def("backup.keep"), "0").is_err());
        assert!(validate(def("backup.keep"), "4").is_ok());
        assert!(validate(def("backup.includeMedia"), "false").is_ok());
        assert!(validate(def("backup.keepUpgrades"), "0").is_err());
        assert!(validate(def("backup.keepUpgrades"), "2").is_ok());
        assert!(validate(def("backup.beforeUpgrade"), "true").is_ok());
    }

    #[test]
    fn the_journal_is_the_server_s_and_kept_from_never_to_ten_years() {
        assert_eq!(def("history.keepDays").scopes, SERVER_ONLY);
        assert!(validate(def("history.keepDays"), "0").is_ok(), "for ever");
        assert!(validate(def("history.keepDays"), "365").is_ok());
        assert!(validate(def("history.keepDays"), "3651").is_err());
        assert!(validate(def("history.keepDays"), "-1").is_err());
    }

    #[test]
    fn a_merge_is_undone_within_days_the_server_sets_or_never() {
        assert_eq!(def("merge.undoDays").scopes, SERVER_ONLY);
        assert!(validate(def("merge.undoDays"), "0").is_ok(), "no undo");
        assert!(validate(def("merge.undoDays"), "14").is_ok());
        assert!(validate(def("merge.undoDays"), "366").is_err());
        assert!(validate(def("merge.undoDays"), "-1").is_err());
    }

    #[test]
    fn the_error_reports_are_the_server_s_and_their_dsn_a_secret_that_reads() {
        for key in ["sentry.dsn", "sentry.environment", "sentry.sampleRate"] {
            assert_eq!(def(key).scopes, SERVER_ONLY, "{key}");
        }
        assert!(def("sentry.dsn").kind.is_secret());
        assert!(validate(def("sentry.dsn"), "https://public@sentry.example.com/42").is_ok());
        assert!(validate(def("sentry.dsn"), "sentry.example.com").is_err());
        assert!(validate(def("sentry.dsn"), MASK).is_err());
        assert!(validate(def("sentry.sampleRate"), "0").is_ok());
        assert!(validate(def("sentry.sampleRate"), "100").is_ok());
        assert!(validate(def("sentry.sampleRate"), "101").is_err());
        assert!(validate(def("sentry.environment"), "staging").is_ok());
        assert!(validate(def("sentry.environment"), "").is_err());
    }

    #[test]
    fn imdbs_titles_and_episodes_are_the_server_s_and_its_regions_anyone_s() {
        for key in ["imdb.akas", "imdb.episodes"] {
            assert_eq!(def(key).scopes, SERVER_ONLY, "{key}");
            assert!(validate(def(key), "true").is_ok(), "{key}");
        }
        assert_eq!(def("imdb.regions").scopes, ALL);
        assert!(validate(def("imdb.regions"), "FR, BE, XWW").is_ok());
        assert!(validate(def("imdb.regions"), "France").is_err());
        assert!(validate(def("imdb.regions"), "").is_err());
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
