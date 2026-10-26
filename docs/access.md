# Accounts and access

Authentication is configured **per API surface**, because the clients differ in
what they can send:

| Surface | Path | Default policy |
|---|---|---|
| Native API and web UI | `/api/v1/*` | API key (header or query) |
| Public browsing | a fixed subset of `/api/v1/*` | **off**, chosen on *Opening & APIs* |
| TMDB-compatible | `/3/*`, and the public lists of `/4/list/*` | API key (`api_key` query parameter) |
| TheTVDB-compatible | `/v4/*` | API key (a key issued here, as the client's TheTVDB key) |
| AniList-compatible | `graphql.anilist.co`, on the clients' door | IP allowlist |
| Sonarr / Radarr compatible | `/v1/*`, and Sonarr v5's `/v2/*`, including the IMDb lists Radarr imports from | IP allowlist |
| Plex metadata provider | `/plex/*`, on the interface's door | IP allowlist, or a key in the address, `/plex/<key>` (`AMS_PLEX_AUTH`) |

Each policy is a variable — `AMS_NATIVE_AUTH`, `AMS_TMDB_AUTH`, `AMS_TVDB_AUTH`,
`AMS_ANILIST_AUTH`, `AMS_ARR_AUTH`, `AMS_PLEX_AUTH` — set to `apikey`, `allowlist` or `open`;
see [Configuration](configuration.md#accounts-and-access). A key is presented as the
[API](api.md#a-key) says.

## Letting anyone browse

Whether the catalogue can be read without signing in is chosen on the
administration's **Opening & APIs** page (the `site.access` setting). *Public*
opens it to a reader with no credential:
the list of works, one work, the totals, what the filters offer
(`/api/v1/facets`), the schedule (`/api/v1/calendar`), a season chart
(`/api/v1/seasons/{year}/{season}`), a person's credits
(`/api/v1/people/{tmdbId}`) and the feeds ([Calendars and feeds](exports.md#calendars-and-feeds)). Nothing else — not what TMDB lists for a season
(`…/candidates`), which is asked of TMDB on this server's key. Which works failed their last
refresh is not among them: that filter is ignored for anyone but an
administrator. It is an allowlist
rather than a denylist, so the settings, the job history, the audit trail, the
raw provider payloads and the record of who edited what all stay behind a
credential — and an endpoint added later is closed until somebody decides
otherwise. Administration is never reachable this way.

*Private* sends every page to the sign-in page, turns link previews off, and
leaves the feeds and the import lists to whoever holds a key.

It is private by default, because a public catalogue publishes what this server
knows to whoever can reach the port. `AMS_PUBLIC_BROWSE` gives the setting its
first value on a new deployment and is not read for anything else — except that
`AMS_PUBLIC_BROWSE=false` keeps the site private whatever the page says, so that
closing the catalogue from the environment cannot be undone by a value stored
before. Remove the variable to let the page decide.

## Accounts, sign-ups and API switches

Every account has a role — *member* (browses, and holds keys of their own),
*editor* (corrects the catalogue), *administrator* (settles everything) — and a
status: active, pending (waiting for approval), disabled. A key a person makes
acts with no more than their role grants, and stops with them. Members reach
what a visitor may, plus their own account page; the Sonarr and Radarr surfaces
are never theirs, and the relays — TMDB's, TheTVDB's, AniList's — only when
*Opening & APIs* lets members use them (`api.tmdbMembers`), since they spend the
operator's quotas.

Sign-ups are *closed* (an administrator opens accounts), *by invitation* (a code
made on the Members page, shown once, which also says the role), *with approval*
(anybody signs up and waits), or *open*. An open door only ever opens a member's
account, and no sign-up ever makes an administrator. Uninvited sign-ups are
limited to `AMS_SIGNUPS_PER_HOUR` (5) per address — an IPv6 address counts by
its /64 — and `AMS_SIGNUPS_PER_HOUR_TOTAL` (100) for the whole server.

The same page switches each API off — Sonarr's, Radarr's, the Plex provider, the
TMDB, TheTVDB and AniList relays, and the native API's keys (the interface's own
session is never cut). A switched-off API answers `503`, which Sonarr and Radarr
read as "try again later". It also says whose key the TMDB and TheTVDB relays ask
their service with: this server's, or each client's own
(`tmdb.relayCredential`, `tvdb.relayCredential`; see
[The relays](relays.md)). Every switch is in the [settings reference](settings.md#opening-and-apis).

`AMS_ADMIN_USERNAME` and `AMS_ADMIN_PASSWORD` open the first administrator. After
that they are a way back in: when no active administrator is left, a restart
makes the account they name an active administrator again, with that password.

## Signing in through an identity provider

Any OpenID Connect provider with a discovery document — Authentik, Keycloak,
Authelia, Kanidm, Google — can sign people in, set up on *Opening & APIs*:
the issuer, a client ID and its secret (or `AMS_OIDC_CLIENT_SECRET`, which wins
and keeps it out of the database), the scopes. Register
`{AMS_PUBLIC_URL}/api/v1/auth/oidc/callback` at the provider as the return
address; without `AMS_PUBLIC_URL` there is none, and the button is not offered.

The flow is the authorization code with PKCE, a state and a nonce, carried in
a cookie sealed with a key only the running server holds (`__Host-` over
HTTPS); the ID token's signature — asymmetric only — issuer, audience, expiry
and `at_hash` are checked. The issuer, and every endpoint it publishes, must be
HTTPS unless it is a local address. A returning person is found by the
provider's own identifier. Someone who already has an account here ties it to
theirs at the provider from their account page, signed in, which proves both
sides; nothing is ever tied by e-mail, since an address typed into a profile
proves nothing. Otherwise, if allowed, an account is opened — a member's, or
the role a claim gives (`groups`, a path such as `realm_access.roles`, or a
namespaced `https://…/roles`, with the values that make an administrator or an
editor). With a claim named, the provider decides the role at every sign-in,
but never takes away the last administrator, and a sign-in whose claims did
not carry it changes nobody's role.

Password sign-in can then be switched off — only once the provider answers its
discovery test, and only when somebody could still be let back in: the
administrator switching it off is tied to the provider, or `AMS_ADMIN_USERNAME`
names an active administrator, whose password keeps working for the day the
provider is down. Sign-ups by password close with it. `AMS_FORCE_PASSWORD_LOGIN=true`
turns passwords back on whatever the settings say.

## Adult titles

Off unless somebody turns them on. Four things decide who sees one, in order:

1. **the server** — `hidden` means nobody, whatever anything else says;
2. **the client's own policy** — `allow`, `deny`, or inherit the server's;
3. **whether the request gets a say** — `adult.force` discards the client's own
   `include_adult` and uses this server's answer instead, both towards the
   providers and when filtering the reply;
4. **what the request asked for**, where it still has a say.

Silence means no unless somebody said otherwise: a client that never mentions
adult titles is not asking for them. The three settings are in the
[settings reference](settings.md#adult-titles).

Sonarr and Radarr cannot say either way, so their searches are taken to ask for
what the server shows. With `visible`, they are listed the works for adults the
catalogue holds — one entered by hand among them — as TMDB's half of the search
lists those it knows; an address rule or a key whose policy is `deny` is listed
none, TMDB's included. A work for adults one of them holds already is answered
by its id all the same: a refusal would have it taken for deleted.

## Who may call, and who has tried

Sonarr and Radarr have their metadata URLs compiled in and cannot attach an API
key, so those surfaces are guarded by address. That list lives in the database
and is edited under **Keys & network** in the administration, beside the API keys — it is the
same decision said a different way — and a change applies from the next request
without a restart. `AMS_ALLOWLIST` seeds it once, the first time the server
starts against an empty table, and is ignored afterwards; a start with the
variable still set, and different from the list, says so in the log.

The same screen shows **who has been calling**: every address that has reached a
guarded route, refused ones included. A refused client leaves no other trace,
and its address is the one thing needed to let it in — so each row carries the
name the hosts file or the system resolver gives that address (inside a compose
network, the container's name), what the caller called itself (`Sonarr/4.0.20`),
how many times it has called, how many times it was turned away, and a button
that allows it. Some TMDB clients
compile their key in too — Jellyseerr does — and need the same treatment. Allow
the clients' own addresses rather than the networks they are on: a Docker
network's gateway is on it, and whatever reaches the host's published ports
through Docker's proxy arrives from there. Behind a reverse proxy, name it in
`AMS_TRUSTED_PROXIES`: a request forwarded by a proxy that is not there is
refused on these surfaces, since every client behind it would look like it. See
[Behind a reverse proxy](reverse-proxy.md).

`AMS_AUTH_DISABLED=true` opens every surface. It exists for closed networks and
first-run setup; do not use it on anything reachable from outside. The same goes
for `AMS_NATIVE_AUTH=open`, which grants administrator rather than reader — key
issuance, the allowlist and the settings included.
