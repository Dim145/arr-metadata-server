//! `docs/settings.md`, the settings reference, written from the registry.
//!
//! The registry says what each setting is called, what it may hold and where
//! it may be set; the server, started on an environment that names none of
//! the variables below, says what each begins as. What neither holds is kept
//! here: the group a setting is described in, what it does, and the variable
//! that gives it its first value. A setting added to the registry without a
//! word here fails the test, as does a word about a setting that is gone, or
//! about a variable the configuration does not read.
//!
//! ```text
//! cargo test settings_reference                # compare docs/settings.md
//! UPDATE_DOCS=1 cargo test settings_reference  # write it again
//! ```

use std::{collections::HashMap, fmt::Write as _, path::PathBuf};

use super::registry::{self, Definition, Kind, REGISTRY, Scope};

/// One setting, as the reference describes it.
struct Entry {
    key: &'static str,
    /// What it does, in Markdown, as one paragraph.
    text: &'static str,
    /// The variables that give it its first value: the one to name, then the
    /// older names it still answers to.
    variables: &'static [&'static str],
    /// How the first value comes of them, when it is not simply theirs.
    first: Option<&'static str>,
}

/// A setting whose first value is the code's.
const fn set(key: &'static str, text: &'static str) -> Entry {
    Entry {
        key,
        text,
        variables: &[],
        first: None,
    }
}

/// A setting whose first value a variable gives, as it is written.
const fn seeded(
    key: &'static str,
    variables: &'static [&'static str],
    text: &'static str,
) -> Entry {
    Entry {
        key,
        text,
        variables,
        first: None,
    }
}

/// Settings described together, under the page of the interface they are on.
struct Group {
    title: &'static str,
    lead: &'static str,
    entries: &'static [Entry],
}

const GROUPS: &[Group] = &[
    Group {
        title: "Answering",
        lead: "On **Settings › Answering**: the language and the country answers are given \
               for, and how much they carry.",
        entries: &[
            seeded(
                "tmdb.language",
                &["AMS_TMDB_LANGUAGE", "TMDB_LANGUAGE"],
                "The language works are stored in and answered in, as a TMDB tag: `en-US`, \
                 `fr-FR`. Set for a key or an address rule, it is the language that caller is \
                 answered in instead; a language not held yet is fetched the first time it is \
                 asked for, and kept. Sonarr v4 always asks in English and Radarr names no \
                 language at all, so this is how either is answered in another: see \
                 [Languages](languages.md).",
            ),
            Entry {
                key: "tmdb.watchRegion",
                text: "The country whose streaming, rental and purchase services a work's page \
                       lists, as an ISO 3166-1 alpha-2 code: `FR`, `US`. Like the language, it can \
                       be set for a key or an address rule.",
                variables: &[],
                first: Some(
                    "Its first value is the country of the starting language's tag — `fr-FR` \
                     gives `FR` — or `US` when the tag names none.",
                ),
            },
            seeded(
                "tmdb.searchLimit",
                &["AMS_TMDB_SEARCH_LIMIT", "TMDB_SEARCH_LIMIT"],
                "How many works a search hands back.",
            ),
            set(
                "tmdb.castLimit",
                "How many of TMDB's actors a work keeps, top of the bill first. Each is a row, \
                 and a portrait to fetch and keep; a work already held takes a new figure at its \
                 next refresh.",
            ),
        ],
    },
    Group {
        title: "Other metadata services",
        lead: "On **Settings › Other metadata services**: Sonarr's and Radarr's own metadata \
               services, and TheTVDB's search, beside TMDB. *Enrichment* asks a service on every \
               fetch and merges what it says; a *fallback* asks it only when nothing else could \
               answer, so with enrichment on, the fallback never fires. See \
               [Where the data comes from](sources.md).",
        entries: &[
            seeded(
                "tvdb.searchFallback",
                &["AMS_TVDB_ENABLED"],
                "Search TheTVDB when what is stored and TMDB both found nothing: TheTVDB knows \
                 series TMDB has never heard of.",
            ),
            seeded(
                "skyhook.fallback",
                &["AMS_SKYHOOK_FALLBACK"],
                "Ask Skyhook, Sonarr's own metadata service, only when nothing else could answer: \
                 for a series no other source gave, and for a search TheTVDB found nothing for \
                 either.",
            ),
            seeded(
                "skyhook.enrich",
                &["AMS_SKYHOOK_ENRICH"],
                "Ask Skyhook on every fetch of a series as well, and merge what it says into \
                 TheTVDB's: one call more per refresh, and `skyhook.fallback` never fires.",
            ),
            seeded(
                "radarr.fallback",
                &["AMS_RADARR_METADATA_FALLBACK"],
                "Ask Radarr's own metadata service, `api.radarr.video`, only when nothing else \
                 could answer for a film.",
            ),
            seeded(
                "radarr.enrich",
                &["AMS_RADARR_METADATA_ENRICH"],
                "Ask Radarr's service on every fetch of a film as well, and merge what it says \
                 into TMDB's: one call more per refresh, and `radarr.fallback` never fires.",
            ),
            seeded(
                "sonarr.sceneMappings",
                &["AMS_SONARR_SCENE_MAPPINGS"],
                "Hand Sonarr's list of alternate titles on with this catalogue's titles added — in \
                 the language of the answers, in English, and romanised from the work's own, the \
                 ones Sonarr can match without mistaking one series for another — so that a \
                 release named in one of them is recognised. It needs `services.sonarr.tv` \
                 resolved to this server; off, the list is handed on as it is. See \
                 [Sonarr's alternate titles](integration.md#sonarrs-alternate-titles).",
            ),
            seeded(
                "sonarr.sceneMappingSearch",
                &["AMS_SONARR_SCENE_MAPPING_SEARCH"],
                "Have Sonarr search with a series' title in the language of the answers too: one \
                 query more per search, on an indexer that cannot be searched by id. The other \
                 titles added serve only to recognise releases.",
            ),
        ],
    },
    Group {
        title: "Further sources",
        lead: "On **Settings › Further sources**: sources beyond the four the server starts \
               with, each off until switched on, none needing a key. See \
               [Further sources](sources.md#further-sources).",
        entries: &[
            seeded(
                "tvmaze.enabled",
                &["AMS_TVMAZE_ENABLED"],
                "TVmaze, for the moment each episode aired, wherever its date agrees with \
                 TheTVDB's — so an episode moved to another slot reaches Sonarr on time. Never \
                 used for numbering. Its data is CC BY-SA, credited on every page.",
            ),
            seeded(
                "anilist.enabled",
                &["AMS_ANILIST_ENABLED"],
                "AniList, for anime: its score, and the romaji and native titles Radarr matches \
                 anime film releases against. While it or MyAnimeList is on, the anime \
                 identifier list is kept downloaded, which also lets Sonarr's AniList and \
                 MyAnimeList import lists find series nobody has added yet.",
            ),
            seeded(
                "mal.enabled",
                &["AMS_MAL_ENABLED"],
                "MyAnimeList, for anime: its score and its titles — through its own API when \
                 `AMS_MAL_CLIENT_ID` is set, through Tenrai otherwise, a mirror written to \
                 Jikan's schema that needs no key. Uses the same identifier list as AniList.",
            ),
            set(
                "mal.fallback",
                "When the way into MyAnimeList that was asked does not answer — no connection, an \
                 error of its own, a rate limit that outlasts the wait — ask the next: Tenrai \
                 after the official API, then Jikan, an instance of one's own \
                 (`AMS_JIKAN_UPSTREAM`; the public service closed on 2026-10-01). One that did \
                 not answer is passed over for five minutes; a 404 is an answer, and asks no \
                 other. Off, only the first is asked.",
            ),
            seeded(
                "imdb.enabled",
                &["AMS_IMDB_ENABLED"],
                "IMDb's own ratings, downloaded daily for the works held here; IMDb's rating is \
                 also the one Sonarr is given, as Skyhook does. For personal, non-commercial use \
                 only.",
            ),
            seeded(
                "fankai.enabled",
                &["AMS_FANKAI_ENABLED"],
                "The Fan-Kai productions, from Fankai's own metadata service: each recut anime as \
                 a series, its sagas as seasons and its films as episodes, numbered the way \
                 Fankai names its files. Fetched only when asked for; switched off, what was \
                 fetched stays as it is.",
            ),
            seeded(
                "fankai.wiki",
                &["AMS_FANKAI_WIKI_ENABLED"],
                "Each Fan-Kai's page on the Fankai community's wiki: the AniList and MyAnimeList \
                 entries it was cut from, and the Fan-Kai that follows it. One call per Fan-Kai \
                 fetch, and only while `fankai.enabled` is on. Its content is CC BY-SA, credited \
                 on every page.",
            ),
        ],
    },
    Group {
        title: "Order of the sources",
        lead: "Put in order on the **Sources** page, where the providers are the columns.",
        entries: &[seeded(
            "sources.priority",
            &["AMS_PROVIDER_PRIORITY"],
            "The order the merge takes the providers in, most trusted first: their names, \
             separated by commas, each at most once. A provider left out comes after the rest. A \
             work takes a new order at its next refresh. See \
             [Where the data comes from](sources.md).",
        )],
    },
    Group {
        title: "Automatic refresh",
        lead: "On **Settings › Automatic refresh**: stored works are fetched again as what is \
               known about them ages.",
        entries: &[
            seeded(
                "refresh.enabled",
                &["AMS_REFRESH_ENABLED"],
                "Fetch stored works again on a schedule. Off, nothing is fetched on the schedule's \
                 account; the housekeeping that runs beside it goes on.",
            ),
            seeded(
                "refresh.intervalSeconds",
                &["AMS_REFRESH_INTERVAL"],
                "How long the scheduler waits between two sweeps, in seconds.",
            ),
            seeded(
                "refresh.batchSize",
                &["AMS_REFRESH_BATCH_SIZE"],
                "How many works one sweep fetches again.",
            ),
            seeded(
                "refresh.ttlContinuing",
                &["AMS_REFRESH_CONTINUING_TTL"],
                "How long a work still changing — a series still airing, a film not out yet — \
                 keeps before it is fetched again, in seconds: `21600` is six hours. An episode \
                 about to air brings it back sooner. A work takes a change at its next refresh. \
                 TMDB's terms of use allow what it answered to be kept six months at the most, \
                 so past `15552000` — a hundred and eighty days — a work whose last refresh is \
                 older than that is asked of TMDB again anyway, every six months, by a light \
                 refresh: TMDB alone, the other sources keeping what they last gave, taken by \
                 the sweep after the works that are due.",
            ),
            seeded(
                "refresh.ttlEnded",
                &["AMS_REFRESH_ENDED_TTL"],
                "The same for a work done changing — an ended series, a released film: `604800` \
                 is a week, and ten years is the most. The six months TMDB allows hold here too: \
                 past `15552000`, the sweep asks TMDB again every six months and keeps the rest \
                 as it is.",
            ),
        ],
    },
    Group {
        title: "Adult titles",
        lead: "On **Settings › Adult titles**, and `adult.clientPolicy` on a key's or an address \
               rule's own settings. Off unless somebody turns them on; \
               [Adult titles](access.md#adult-titles) says in which order the answers are taken.",
        entries: &[
            Entry {
                key: "adult.mode",
                text: "`hidden`: nobody is served an adult title, whatever any client asks for. \
                       `visible`: one is served to a request that asks for it, within what its \
                       client's own policy allows.",
                variables: &["AMS_TMDB_INCLUDE_ADULT", "TMDB_INCLUDE_ADULT"],
                first: Some("`AMS_TMDB_INCLUDE_ADULT=true` makes its first value `visible`."),
            },
            set(
                "adult.clientPolicy",
                "What one client is served, within what the server allows: `allow` and `deny` \
                 decide for that key or that address rule alone, and `inherit` follows the \
                 server. Unset, it inherits.",
            ),
            set(
                "adult.force",
                "Discard the client's own `include_adult` and use this server's answer instead, \
                 both towards the providers and when filtering what goes back.",
            ),
        ],
    },
    Group {
        title: "Opening and APIs",
        lead: "On **Opening & APIs**: who may come in, and through which door. An API switched \
               off answers `503` with the code `surface_disabled`, which Sonarr and Radarr read \
               as \"try again later\". See [Accounts and access](access.md).",
        entries: &[
            Entry {
                key: "site.access",
                text: "Whether the catalogue can be read without signing in. `public` opens its \
                       reading pages to anyone — the works, the schedule, a person's credits, the \
                       feeds; `private` sends every page to the sign-in page. The APIs ask for \
                       their credential either way, and administration is never open. \
                       `AMS_PUBLIC_BROWSE=false` keeps the site private whatever this says.",
                variables: &["AMS_PUBLIC_BROWSE"],
                first: Some("`AMS_PUBLIC_BROWSE=true` makes its first value `public`."),
            },
            set(
                "registration.mode",
                "Who may open an account: `closed`, only an administrator; `invite`, whoever holds \
                 a code made on the Members page; `approval`, anybody, who then waits for an \
                 administrator; `open`, anybody. An open door only ever opens a member's account.",
            ),
            set(
                "registration.role",
                "The role an account that signed up for approval is given once it is approved: \
                 `member` or `editor`, never an administrator.",
            ),
            set(
                "keys.maxPerUser",
                "How many API keys a member or an editor may make for themselves. At 1, each has \
                 their own key and regenerates it; at 0, only administrators make keys. \
                 Administrators, and the server's own keys, are not counted.",
            ),
            set(
                "api.sonarr",
                "Sonarr's API: Skyhook's two editions, `/v1/tvdb/…` and `/v2/tvdb/…`, and the \
                 alternate-title list of `services.sonarr.tv`, `/v1/scenemapping`.",
            ),
            set(
                "api.radarr",
                "Radarr's API, in `api.radarr.video`'s place: `/v1/movie/…`, `/v1/search` and \
                 `/v1/list/…`.",
            ),
            set(
                "api.plex",
                "The metadata provider Plex Media Server is given by its address, `/plex`. See \
                 [Plex](plex.md).",
            ),
            set("api.tmdb", "The TMDB relay, `/3/…` and `/4/…`."),
            set("api.tvdb", "The TheTVDB relay, `/v4/…`."),
            set(
                "api.anilist",
                "The AniList relay, `graphql.anilist.co` on the clients' door.",
            ),
            set(
                "api.native",
                "The native API, `/api/v1/…`, called with a key. The interface's own session is \
                 never turned away, whatever this says: switching it off cannot lock out the \
                 person who did.",
            ),
            set(
                "api.tmdbMembers",
                "Let members — in person, or with their keys — use the relays: TMDB's, \
                 TheTVDB's and AniList's, which spend the operator's quotas. Editors and \
                 administrators may whenever a relay is on.",
            ),
            set(
                "tmdb.relayCredential",
                "Whose key the TMDB relay asks TMDB with: `server`, this server's \
                 (`AMS_TMDB_API_KEY`), in place of whatever the client sent; `client`, the \
                 client's own, handed on as it came — a key issued here is then refused, and \
                 the list of allowed addresses guards the relay. See \
                 [Whose key the service is asked with](relays.md#whose-key-the-service-is-asked-with).",
            ),
            set(
                "tvdb.relayCredential",
                "The same for the TheTVDB relay: `server` asks TheTVDB with this server's key \
                 (`AMS_TVDB_API_KEY`); `client` has every client sign in with its own TheTVDB \
                 key, and refuses a key issued here.",
            ),
        ],
    },
    Group {
        title: "Signing in through an identity provider",
        lead: "On **Opening & APIs**, and written together through `PUT /api/v1/admin/oidc`, \
               where they are checked as a whole. The provider is offered once `oidc.enabled` is \
               on and the issuer, the client ID and `AMS_PUBLIC_URL` are set. See \
               [Signing in through an identity provider](access.md#signing-in-through-an-identity-provider).",
        entries: &[
            set(
                "oidc.enabled",
                "Offer signing in through the identity provider.",
            ),
            set(
                "oidc.issuer",
                "The provider's issuer URL, which its discovery document is read from. HTTPS, \
                 unless it is a local address.",
            ),
            set("oidc.clientId", "The client ID registered at the provider."),
            set(
                "oidc.clientSecret",
                "The client's secret. `AMS_OIDC_CLIENT_SECRET` wins over it when it is set, and \
                 keeps it out of the database.",
            ),
            set(
                "oidc.scopes",
                "The scopes asked for, separated by spaces; `openid` is always asked for.",
            ),
            set(
                "oidc.buttonLabel",
                "What the sign-in page's button says. Empty, *Continue with the identity \
                 provider*.",
            ),
            set(
                "oidc.autoRegister",
                "Open an account for someone the provider vouches for and who has none here yet: \
                 a member's, unless the role claim says more — and waiting for approval when \
                 sign-ups do. Off, only an account already tied to the provider signs in through \
                 it.",
            ),
            set(
                "oidc.roleClaim",
                "The claim roles are read from: `groups`, or a path such as \
                 `realm_access.roles`. Named, the provider decides the role at every sign-in, but \
                 never takes away the last administrator.",
            ),
            set(
                "oidc.adminValues",
                "The values of the role claim that make an administrator, separated by commas.",
            ),
            set(
                "oidc.editorValues",
                "The values of the role claim that make an editor, separated by commas.",
            ),
            set(
                "auth.passwordLogin",
                "Offer the password form. It can be switched off only once the provider answers \
                 its discovery test and somebody could still be let back in. Off, only the \
                 provider signs people in — except the account `AMS_ADMIN_USERNAME` names, which \
                 keeps its password for the day the provider is down — and sign-ups by password \
                 close with it. It counts as on while no provider is set up, and \
                 `AMS_FORCE_PASSWORD_LOGIN=true` keeps it on whatever this says.",
            ),
        ],
    },
    Group {
        title: "Sonarr and Radarr",
        lead: "On the **Sonarr & Radarr** page: the operator's own Sonarr and Radarr, reached \
               the other way, to tell them of a change and to read what they hold. Written \
               through `PUT /api/v1/admin/arr`, which checks an address as an address before it \
               keeps it. See \
               [Connecting your Sonarr and Radarr](sonarr-radarr.md#connecting-your-sonarr-and-radarr).",
        entries: &[
            set(
                "arr.sonarrUrl",
                "Where this server reaches Sonarr, with its URL base when it has one: \
                 `http://sonarr:8989`. Empty, Sonarr is not connected.",
            ),
            set(
                "arr.sonarrKey",
                "Sonarr's API key, from its *Settings › General*.",
            ),
            set(
                "arr.radarrUrl",
                "Where this server reaches Radarr: `http://radarr:7878`. Empty, Radarr is not \
                 connected.",
            ),
            set("arr.radarrKey", "Radarr's API key."),
            set(
                "arr.pushRefresh",
                "Ask Sonarr or Radarr to read a work again as soon as it changes here — a lock, \
                 an edit, a sync, a refresh that changed it — rather than at their next refresh. \
                 See [Telling them of a change](sonarr-radarr.md#telling-them-of-a-change).",
            ),
        ],
    },
    Group {
        title: "Media",
        lead: "On the **Media** page, and read only when a store is configured \
               (`AMS_MEDIA_STORAGE`). See [Keeping the media](media.md).",
        entries: &[
            set(
                "media.store",
                "Fetch and keep the media a work points at, as the work is stored.",
            ),
            set(
                "media.serve",
                "How a copy kept in a bucket reaches a reader: `proxy`, through this server; \
                 `redirect`, sent to the bucket's own address, signed for an hour. A copy on \
                 disk is always served by this server.",
            ),
            set(
                "media.people",
                "The cast's photographs too: one per person, many per work.",
            ),
            set(
                "media.audio",
                "The themes: a Fan-Kai's, a few megabytes each.",
            ),
        ],
    },
    Group {
        title: "Cache",
        lead: "On the **Cache** page, one switch per space: off, a space answers nothing and \
               keeps nothing, and what it held expires. See [Caching](caching.md).",
        entries: &[
            set("cache.items", "Works, merged, as they are served."),
            set("cache.searches", "Searches, and pages of the catalogue."),
            set(
                "cache.lists",
                "Whole lists relayed from a metadata service — IMDb's Top 250 is thirteen \
                 megabytes — kept a day, apart from the searches.",
            ),
            set(
                "cache.relay",
                "What the TMDB, TheTVDB and AniList relays were answered with this server's \
                 credentials, shared between clients.",
            ),
            set(
                "cache.sessions",
                "Who a session belongs to, kept a few seconds rather than read from the database \
                 on every request. Memory alone.",
            ),
        ],
    },
    Group {
        title: "Updates",
        lead: "On **Settings › Everything else**.",
        entries: &[set(
            "updates.check",
            "Once a day, and a minute after the start, ask GitHub for this server's latest \
             release, so the dashboard can say when a newer one is out. What is sent is one \
             anonymous `GET` to `api.github.com`, with the name and version every request of \
             this server carries — nothing about this instance or what it holds. Unset, it is \
             off.",
        )],
    },
];

/// What comes before the groups.
const INTRO: &str = r#"# Settings reference

<!-- Written by `UPDATE_DOCS=1 cargo test settings_reference`, from src/settings/registry.rs
     and src/settings/reference.rs: change those, then run it, rather than editing this page. -->

A setting is how the server behaves: stored in its database, changed from the administration
without a restart, and recorded in the audit trail as `setting.changed` when it is. How the
server is deployed — its addresses, its keys, its database — is told by environment variables
instead: see [Configuration](configuration.md).

Each setting says what it holds, what it begins as, and where it may be set:

- **server**: for every caller;
- **client**: for one API key, over the server's value;
- **peer**: for one rule of the list of allowed addresses — which is how a client that sends no
  key, Sonarr or Radarr, is known — over the server's value and the key's.

The narrowest scope that sets a value wins. A setting is set for a key or a rule from that key's
or that rule's own settings, on **Keys & network**.

The default is what a server begins with when its environment names none of the variables
below. Where a variable is named, it gives the setting its first value instead: at the first
start, or at the first start of the version that added the setting. From then on the variable
is not read again, and the setting is changed in the interface. **Settings** marks a setting
changed from what it began as, and takes it back in one press.

Through the API, as an administrator: `GET /api/v1/settings/registry` lists every setting and
what it may hold; `GET /api/v1/settings/{scope}/{id}` says what applies at a scope — `server`
and `-`, `client` and a key's id, `peer` and a rule's id — and, at the server's, what each
value began as, `default`; `PUT` there with `{"key": …, "value": …}` sets one, and
`"value": null` stops setting it at that scope; `DELETE` there clears everything a key or a
rule set. The settings of signing in (`oidc.*`, `auth.*`) and of Sonarr's and Radarr's
connections (`arr.*`) are written through the pages that check them as a whole, and the
settings endpoint refuses them.
"#;

/// The width the prose is wrapped at, as the rest of docs/ is.
const WIDTH: usize = 92;

/// `text` wrapped at [`WIDTH`]: never inside a code span, and never so that a
/// line begins as a list item, a heading or a quote would.
fn wrap(text: &str) -> String {
    let marks = |word: &str| {
        matches!(word, "-" | "+" | "*" | ">" | "|")
            || word.starts_with('#')
            || ((word.ends_with('.') || word.ends_with(')'))
                && word[..word.len() - 1].bytes().all(|b| b.is_ascii_digit()))
    };
    // A code span with spaces in it is one word: `PUT /api/v1/admin/arr`.
    let mut words: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match words.last_mut() {
            Some(open) if open.matches('`').count() % 2 == 1 => {
                open.push(' ');
                open.push_str(word);
            }
            _ => words.push(word.to_string()),
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in &words {
        if !line.is_empty()
            && line.chars().count() + 1 + word.chars().count() > WIDTH
            && !marks(word)
        {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines.join("\n")
}

/// The anchor GitHub, and the site, give a heading made of `key` in code.
fn anchor(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// What a setting holds, as the table says it.
fn holds(def: &Definition) -> String {
    match def.kind {
        Kind::Bool => "`true` or `false`".into(),
        Kind::Int { min, max } => format!("a whole number, `{min}` to `{max}`"),
        Kind::Text if registry::validate(def, "").is_ok() => "text, or nothing".into(),
        Kind::Text => "text".into(),
        Kind::Choice { options } => match options {
            [] => "nothing".into(),
            [only] => format!("`{only}`"),
            [rest @ .., last] => format!(
                "{} or `{last}`",
                rest.iter()
                    .map(|o| format!("`{o}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
        Kind::Secret => "a secret, never read back".into(),
    }
}

/// Where a setting may be set, as the table says it.
fn scopes(def: &Definition) -> String {
    def.scopes
        .iter()
        .map(|scope| scope.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a setting begins as, as the table says it.
fn default(value: Option<&String>) -> String {
    match value {
        None => "unset".into(),
        Some(value) if value.is_empty() => "nothing".into(),
        Some(value) => {
            assert!(
                !value.contains(['`', '|', '\n']),
                "{value:?} cannot be written in a table cell as it is"
            );
            format!("`{value}`")
        }
    }
}

/// The page, from the registry, the groups above and what the server began with.
fn render(defaults: &HashMap<String, String>) -> String {
    let described: Vec<&str> = GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .map(|entry| entry.key)
        .collect();
    for def in REGISTRY {
        let times = described.iter().filter(|key| **key == def.key).count();
        assert!(
            times == 1,
            "{} is described {times} times in src/settings/reference.rs; it has to be described \
             once, in the group it is shown in",
            def.key
        );
    }
    for key in &described {
        assert!(
            registry::find(key).is_some(),
            "src/settings/reference.rs describes {key}, which the registry does not hold"
        );
    }

    let mut page = String::from(INTRO);
    for group in GROUPS {
        write!(page, "\n## {}\n\n{}\n\n", group.title, wrap(group.lead)).unwrap();
        page.push_str("| Setting | Holds | Default | Set at |\n| --- | --- | --- | --- |\n");
        for entry in group.entries {
            let def = registry::find(entry.key).expect("checked above");
            writeln!(
                page,
                "| [`{key}`](#{anchor}) | {holds} | {default} | {scopes} |",
                key = entry.key,
                anchor = anchor(entry.key),
                holds = holds(def),
                default = default(defaults.get(entry.key)),
                scopes = scopes(def),
            )
            .unwrap();
        }
        for entry in group.entries {
            for variable in entry.variables {
                assert!(
                    read_by_the_server(variable),
                    "src/settings/reference.rs says {variable} gives {} its first value, and \
                     the server reads no such variable",
                    entry.key
                );
            }
            write!(page, "\n### `{}`\n\n{}\n", entry.key, wrap(entry.text)).unwrap();
            let first = match (entry.first, entry.variables.first()) {
                (Some(first), _) => Some(first.to_string()),
                (None, Some(variable)) => Some(format!("`{variable}` gives it its first value.")),
                (None, None) => None,
            };
            if let Some(first) = first {
                write!(page, "\n{}\n", wrap(&first)).unwrap();
            }
        }
    }
    page
}

/// What reads the environment: the configuration, and the two modules that
/// read a variable of their own.
const READERS: [&str; 3] = [
    include_str!("../config.rs"),
    include_str!("../telemetry.rs"),
    include_str!("../outbound.rs"),
];

/// Whether `variable` is read by the server: named in one of [`READERS`], as
/// a word of its own rather than the start of a longer name.
fn read_by_the_server(variable: &str) -> bool {
    let part = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_';
    READERS.iter().any(|source| {
        source.match_indices(variable).any(|(at, _)| {
            !source[..at].chars().next_back().is_some_and(part)
                && !source[at + variable.len()..]
                    .chars()
                    .next()
                    .is_some_and(part)
        })
    })
}

/// Every setting's starting value, from a server started the way a new one is.
async fn starting_values() -> HashMap<String, String> {
    // The page documents the code's own defaults: a variable that gives a
    // setting its first value, set where the test runs, would write that
    // value into it instead.
    let named: Vec<&str> = GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .flat_map(|entry| entry.variables)
        .copied()
        .filter(|variable| std::env::var(variable).is_ok_and(|value| !value.trim().is_empty()))
        .collect();
    let (is, it) = if named.len() == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    assert!(
        named.is_empty(),
        "{} {is} set in this environment, and the reference says what a server begins with \
         when none is: run the test without {it}",
        named.join(", "),
    );

    let mut config = crate::config::Config::from_env().expect("a configuration");
    config.mode = crate::config::Mode::Single;
    config.database = crate::db::testing::database().await;
    config.security.bootstrap_admin = None;
    config.cache.redis_url = None;
    config.media.storage = crate::config::MediaStorage::Off;
    config.clients = None;
    config.tmdb.api_key = None;
    config.tvdb.api_key = None;

    let state = crate::state::AppState::bootstrap(config)
        .await
        .expect("a server");
    state
        .settings
        .effective(Scope::Server, "")
        .into_iter()
        .filter_map(|setting| Some((setting.key, setting.default?)))
        .collect()
}

#[tokio::test]
async fn the_settings_reference_says_what_the_registry_holds() {
    let page = render(&starting_values().await);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/settings.md");

    if std::env::var("UPDATE_DOCS").is_ok_and(|value| value == "1") {
        std::fs::write(&path, &page).expect("docs/settings.md written");
        return;
    }

    let written = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if written != page {
        let (line, (theirs, ours)) = written
            .lines()
            .chain(std::iter::repeat(""))
            .zip(page.lines().chain(std::iter::repeat("")))
            .enumerate()
            .find(|(_, (theirs, ours))| theirs != ours)
            .unwrap_or((0, ("", "")));
        panic!(
            "docs/settings.md is not what the registry says: line {} reads\n  {theirs:?}\nwhere \
             the registry gives\n  {ours:?}\nRun `UPDATE_DOCS=1 cargo test settings_reference` to \
             rewrite it.",
            line + 1,
        );
    }
}

#[test]
fn a_settings_reference_anchor_is_the_heading_github_makes() {
    assert_eq!(anchor("tmdb.watchRegion"), "tmdbwatchregion");
    assert_eq!(anchor("api.tmdbMembers"), "apitmdbmembers");
}

#[test]
fn the_settings_reference_wraps_no_line_into_a_list() {
    // Eighteen words and an `x` fill 91 columns: the dash would begin the
    // next line, where it would make one.
    let wrapped = wrap(&format!("{} x - not a list", "word ".repeat(18).trim_end()));
    assert!(wrapped.lines().count() > 1, "{wrapped}");
    assert!(
        wrapped.lines().all(|line| !line.starts_with('-')),
        "{wrapped}"
    );
    assert!(wrapped.contains(" x -\n"), "{wrapped}");
}

#[test]
fn the_settings_reference_breaks_no_code_span() {
    // The span begins in column 86: split at its space, it would read as two.
    let wrapped = wrap(&format!(
        "{} `PUT /api/v1/admin/arr` checks it",
        "word ".repeat(17).trim_end()
    ));
    assert!(
        wrapped.contains("\n`PUT /api/v1/admin/arr` checks it"),
        "{wrapped}"
    );
}

#[test]
fn the_configuration_page_names_only_variables_the_server_reads() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/configuration.md");
    let page = std::fs::read_to_string(&path).expect("docs/configuration.md");

    // Every name in capitals with an underscore in it: `AMS_MODE`, and the
    // older names it still answers to, `TMDB_API_KEY`.
    let part = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_';
    let named: Vec<&str> = page
        .split(|c: char| !part(c))
        .filter(|word| {
            word.len() > 3
                && word.contains('_')
                && !word.starts_with('_')
                && !word.ends_with('_')
                && word.chars().any(|c| c.is_ascii_uppercase())
        })
        .collect();
    assert!(
        named.len() > 50,
        "docs/configuration.md names {} variables",
        named.len()
    );

    let unread: Vec<&str> = named
        .into_iter()
        .filter(|variable| !read_by_the_server(variable))
        .collect();
    assert!(
        unread.is_empty(),
        "docs/configuration.md names {unread:?}, which the server does not read: \
         src/config.rs, src/telemetry.rs and src/outbound.rs are where it reads its variables"
    );
}

#[test]
fn a_variable_is_read_as_a_word_of_its_own() {
    assert!(read_by_the_server("AMS_TLS_DIR"));
    assert!(read_by_the_server("AMS_LOG"));
    // The start of AMS_TLS_DIR and AMS_TLS_CERT, and nothing on its own.
    assert!(!read_by_the_server("AMS_TLS"));
}

#[test]
fn the_refresh_intervals_say_what_tmdb_allows() {
    // TMDB's terms allow its answers to be kept six months at the most: both
    // intervals say so, and name the figure past which the sweep makes up
    // for the setting on its own.
    for key in ["refresh.ttlContinuing", "refresh.ttlEnded"] {
        let entry = GROUPS
            .iter()
            .flat_map(|group| group.entries)
            .find(|entry| entry.key == key)
            .expect("described");
        assert!(entry.text.contains("six months"), "{key}: {}", entry.text);
        assert!(entry.text.contains("`15552000`"), "{key}: {}", entry.text);
        assert!(entry.text.contains("TMDB"), "{key}: {}", entry.text);
    }
}
