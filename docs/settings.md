# Settings reference

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

## Answering

On **Settings › Answering**: the language and the country answers are given for, and how
much they carry.

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`tmdb.language`](#tmdblanguage) | text | `en-US` | server, client, peer |
| [`tmdb.watchRegion`](#tmdbwatchregion) | text | `US` | server, client, peer |
| [`tmdb.searchLimit`](#tmdbsearchlimit) | a whole number, `1` to `50` | `10` | server |
| [`tmdb.castLimit`](#tmdbcastlimit) | a whole number, `1` to `200` | `25` | server |

### `tmdb.language`

The language works are stored in and answered in, as a TMDB tag: `en-US`, `fr-FR`. Set for a
key or an address rule, it is the language that caller is answered in instead; a language
not held yet is fetched the first time it is asked for, and kept. Sonarr v4 always asks in
English and Radarr names no language at all, so this is how either is answered in another:
see [Languages](languages.md).

`AMS_TMDB_LANGUAGE` gives it its first value.

### `tmdb.watchRegion`

The country whose streaming, rental and purchase services a work's page lists, as an ISO
3166-1 alpha-2 code: `FR`, `US`. Like the language, it can be set for a key or an address
rule.

Its first value is the country of the starting language's tag — `fr-FR` gives `FR` — or `US`
when the tag names none.

### `tmdb.searchLimit`

How many works a search hands back.

`AMS_TMDB_SEARCH_LIMIT` gives it its first value.

### `tmdb.castLimit`

How many of TMDB's actors a work keeps, top of the bill first. Each is a row, and a portrait
to fetch and keep; a work already held takes a new figure at its next refresh.

## Other metadata services

On **Settings › Other metadata services**: Sonarr's and Radarr's own metadata services, and
TheTVDB's search, beside TMDB. *Enrichment* asks a service on every fetch and merges what it
says; a *fallback* asks it only when nothing else could answer, so with enrichment on, the
fallback never fires. See [Where the data comes from](sources.md).

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`tvdb.searchFallback`](#tvdbsearchfallback) | `true` or `false` | `true` | server |
| [`skyhook.fallback`](#skyhookfallback) | `true` or `false` | `true` | server |
| [`skyhook.enrich`](#skyhookenrich) | `true` or `false` | `true` | server |
| [`radarr.fallback`](#radarrfallback) | `true` or `false` | `true` | server |
| [`radarr.enrich`](#radarrenrich) | `true` or `false` | `true` | server |
| [`sonarr.sceneMappings`](#sonarrscenemappings) | `true` or `false` | `false` | server |
| [`sonarr.sceneMappingSearch`](#sonarrscenemappingsearch) | `true` or `false` | `true` | server |

### `tvdb.searchFallback`

Search TheTVDB when what is stored and TMDB both found nothing: TheTVDB knows series TMDB
has never heard of.

`AMS_TVDB_ENABLED` gives it its first value.

### `skyhook.fallback`

Ask Skyhook, Sonarr's own metadata service, only when nothing else could answer: for a
series no other source gave, and for a search TheTVDB found nothing for either.

`AMS_SKYHOOK_FALLBACK` gives it its first value.

### `skyhook.enrich`

Ask Skyhook on every fetch of a series as well, and merge what it says into TheTVDB's: one
call more per refresh, and `skyhook.fallback` never fires.

`AMS_SKYHOOK_ENRICH` gives it its first value.

### `radarr.fallback`

Ask Radarr's own metadata service, `api.radarr.video`, only when nothing else could answer
for a film.

`AMS_RADARR_METADATA_FALLBACK` gives it its first value.

### `radarr.enrich`

Ask Radarr's service on every fetch of a film as well, and merge what it says into TMDB's:
one call more per refresh, and `radarr.fallback` never fires.

`AMS_RADARR_METADATA_ENRICH` gives it its first value.

### `sonarr.sceneMappings`

Hand Sonarr's list of alternate titles on with this catalogue's titles added — in the
language of the answers, in English, and romanised from the work's own, the ones Sonarr can
match without mistaking one series for another — so that a release named in one of them is
recognised. It needs `services.sonarr.tv` resolved to this server; off, the list is handed
on as it is. See [Sonarr's alternate titles](integration.md#sonarrs-alternate-titles).

`AMS_SONARR_SCENE_MAPPINGS` gives it its first value.

### `sonarr.sceneMappingSearch`

Have Sonarr search with a series' title in the language of the answers too: one query more
per search, on an indexer that cannot be searched by id. The other titles added serve only
to recognise releases.

`AMS_SONARR_SCENE_MAPPING_SEARCH` gives it its first value.

## Further sources

On **Settings › Further sources**: sources beyond the four the server starts with, each off
until switched on, none needing a key. See [Further sources](sources.md#further-sources).

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`tvmaze.enabled`](#tvmazeenabled) | `true` or `false` | `false` | server |
| [`anilist.enabled`](#anilistenabled) | `true` or `false` | `false` | server |
| [`mal.enabled`](#malenabled) | `true` or `false` | `false` | server |
| [`mal.fallback`](#malfallback) | `true` or `false` | `true` | server |
| [`imdb.enabled`](#imdbenabled) | `true` or `false` | `false` | server |
| [`fankai.enabled`](#fankaienabled) | `true` or `false` | `false` | server |
| [`fankai.wiki`](#fankaiwiki) | `true` or `false` | `false` | server |

### `tvmaze.enabled`

TVmaze, for the moment each episode aired, wherever its date agrees with TheTVDB's — so an
episode moved to another slot reaches Sonarr on time. Never used for numbering. Its data is
CC BY-SA, credited on every page.

`AMS_TVMAZE_ENABLED` gives it its first value.

### `anilist.enabled`

AniList, for anime: its score, and the romaji and native titles Radarr matches anime film
releases against. While it or MyAnimeList is on, the anime identifier list is kept
downloaded, which also lets Sonarr's AniList and MyAnimeList import lists find series nobody
has added yet.

`AMS_ANILIST_ENABLED` gives it its first value.

### `mal.enabled`

MyAnimeList, for anime: its score and its titles — through its own API when
`AMS_MAL_CLIENT_ID` is set, through Tenrai otherwise, a mirror written to Jikan's schema
that needs no key. Uses the same identifier list as AniList.

`AMS_MAL_ENABLED` gives it its first value.

### `mal.fallback`

When the way into MyAnimeList that was asked does not answer — no connection, an error of
its own, a rate limit that outlasts the wait — ask the next: Tenrai after the official API,
then Jikan, an instance of one's own (`AMS_JIKAN_UPSTREAM`; the public service closed on
2026-10-01). One that did not answer is passed over for five minutes; a 404 is an answer,
and asks no other. Off, only the first is asked.

### `imdb.enabled`

IMDb's own ratings, downloaded daily for the works held here; IMDb's rating is also the one
Sonarr is given, as Skyhook does. For personal, non-commercial use only.

`AMS_IMDB_ENABLED` gives it its first value.

### `fankai.enabled`

The Fan-Kai productions, from Fankai's own metadata service: each recut anime as a series,
its sagas as seasons and its films as episodes, numbered the way Fankai names its files.
Fetched only when asked for; switched off, what was fetched stays as it is.

`AMS_FANKAI_ENABLED` gives it its first value.

### `fankai.wiki`

Each Fan-Kai's page on the Fankai community's wiki: the AniList and MyAnimeList entries it
was cut from, and the Fan-Kai that follows it. One call per Fan-Kai fetch, and only while
`fankai.enabled` is on. Its content is CC BY-SA, credited on every page.

`AMS_FANKAI_WIKI_ENABLED` gives it its first value.

## Order of the sources

Put in order on the **Sources** page, where the providers are the columns.

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`sources.priority`](#sourcespriority) | text | `tmdb,tvdb,skyhook,radarr,fanart,tvmaze,anilist,mal,fankai,fankaiwiki` | server |

### `sources.priority`

The order the merge takes the providers in, most trusted first: their names, separated by
commas, each at most once. A provider left out comes after the rest. A work takes a new
order at its next refresh. See [Where the data comes from](sources.md).

`AMS_PROVIDER_PRIORITY` gives it its first value.

## Automatic refresh

On **Settings › Automatic refresh**: stored works are fetched again as what is known about
them ages.

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`refresh.enabled`](#refreshenabled) | `true` or `false` | `true` | server |
| [`refresh.intervalSeconds`](#refreshintervalseconds) | a whole number, `60` to `604800` | `900` | server |
| [`refresh.batchSize`](#refreshbatchsize) | a whole number, `1` to `500` | `25` | server |
| [`refresh.ttlContinuing`](#refreshttlcontinuing) | a whole number, `60` to `31536000` | `21600` | server |
| [`refresh.ttlEnded`](#refreshttlended) | a whole number, `60` to `315360000` | `604800` | server |

### `refresh.enabled`

Fetch stored works again on a schedule. Off, nothing is fetched on the schedule's account;
the housekeeping that runs beside it goes on.

`AMS_REFRESH_ENABLED` gives it its first value.

### `refresh.intervalSeconds`

How long the scheduler waits between two sweeps, in seconds.

`AMS_REFRESH_INTERVAL` gives it its first value.

### `refresh.batchSize`

How many works one sweep fetches again.

`AMS_REFRESH_BATCH_SIZE` gives it its first value.

### `refresh.ttlContinuing`

How long a work still changing — a series still airing, a film not out yet — keeps before it
is fetched again, in seconds: `21600` is six hours. An episode about to air brings it back
sooner. A work takes a change at its next refresh. TMDB's terms of use allow what it
answered to be kept six months at the most, so past `15552000` — a hundred and eighty days —
a work whose last refresh is older than that is asked of TMDB again anyway, every six
months, by a light refresh: TMDB alone, the other sources keeping what they last gave, taken
by the sweep after the works that are due.

`AMS_REFRESH_CONTINUING_TTL` gives it its first value.

### `refresh.ttlEnded`

The same for a work done changing — an ended series, a released film: `604800` is a week,
and ten years is the most. The six months TMDB allows hold here too: past `15552000`, the
sweep asks TMDB again every six months and keeps the rest as it is.

`AMS_REFRESH_ENDED_TTL` gives it its first value.

## Adult titles

On **Settings › Adult titles**, and `adult.clientPolicy` on a key's or an address rule's own
settings. Off unless somebody turns them on; [Adult titles](access.md#adult-titles) says in
which order the answers are taken.

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`adult.mode`](#adultmode) | `hidden` or `visible` | `hidden` | server |
| [`adult.clientPolicy`](#adultclientpolicy) | `inherit`, `allow` or `deny` | unset | client, peer |
| [`adult.force`](#adultforce) | `true` or `false` | `false` | server, client, peer |

### `adult.mode`

`hidden`: nobody is served an adult title, whatever any client asks for. `visible`: one is
served to a request that asks for it, within what its client's own policy allows.

`AMS_TMDB_INCLUDE_ADULT=true` makes its first value `visible`.

### `adult.clientPolicy`

What one client is served, within what the server allows: `allow` and `deny` decide for that
key or that address rule alone, and `inherit` follows the server. Unset, it inherits.

### `adult.force`

Discard the client's own `include_adult` and use this server's answer instead, both towards
the providers and when filtering what goes back.

## Opening and APIs

On **Opening & APIs**: who may come in, and through which door. An API switched off answers
`503` with the code `surface_disabled`, which Sonarr and Radarr read as "try again later".
See [Accounts and access](access.md).

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`site.access`](#siteaccess) | `private` or `public` | `private` | server |
| [`registration.mode`](#registrationmode) | `closed`, `invite`, `approval` or `open` | `closed` | server |
| [`registration.role`](#registrationrole) | `member` or `editor` | `member` | server |
| [`keys.maxPerUser`](#keysmaxperuser) | a whole number, `0` to `50` | `5` | server |
| [`api.sonarr`](#apisonarr) | `true` or `false` | `true` | server |
| [`api.radarr`](#apiradarr) | `true` or `false` | `true` | server |
| [`api.plex`](#apiplex) | `true` or `false` | `true` | server |
| [`api.tmdb`](#apitmdb) | `true` or `false` | `true` | server |
| [`api.tvdb`](#apitvdb) | `true` or `false` | `true` | server |
| [`api.anilist`](#apianilist) | `true` or `false` | `true` | server |
| [`api.native`](#apinative) | `true` or `false` | `true` | server |
| [`api.tmdbMembers`](#apitmdbmembers) | `true` or `false` | `false` | server |
| [`tmdb.relayCredential`](#tmdbrelaycredential) | `server` or `client` | `server` | server |
| [`tvdb.relayCredential`](#tvdbrelaycredential) | `server` or `client` | `server` | server |

### `site.access`

Whether the catalogue can be read without signing in. `public` opens its reading pages to
anyone — the works, the schedule, a person's credits, the feeds; `private` sends every page
to the sign-in page. The APIs ask for their credential either way, and administration is
never open. `AMS_PUBLIC_BROWSE=false` keeps the site private whatever this says.

`AMS_PUBLIC_BROWSE=true` makes its first value `public`.

### `registration.mode`

Who may open an account: `closed`, only an administrator; `invite`, whoever holds a code
made on the Members page; `approval`, anybody, who then waits for an administrator; `open`,
anybody. An open door only ever opens a member's account.

### `registration.role`

The role an account that signed up for approval is given once it is approved: `member` or
`editor`, never an administrator.

### `keys.maxPerUser`

How many API keys a member or an editor may make for themselves. At 1, each has their own
key and regenerates it; at 0, only administrators make keys. Administrators, and the
server's own keys, are not counted.

### `api.sonarr`

Sonarr's API: Skyhook's two editions, `/v1/tvdb/…` and `/v2/tvdb/…`, and the alternate-title
list of `services.sonarr.tv`, `/v1/scenemapping`.

### `api.radarr`

Radarr's API, in `api.radarr.video`'s place: `/v1/movie/…`, `/v1/search` and `/v1/list/…`.

### `api.plex`

The metadata provider Plex Media Server is given by its address, `/plex`. See
[Plex](plex.md).

### `api.tmdb`

The TMDB relay, `/3/…` and `/4/…`.

### `api.tvdb`

The TheTVDB relay, `/v4/…`.

### `api.anilist`

The AniList relay, `graphql.anilist.co` on the clients' door.

### `api.native`

The native API, `/api/v1/…`, called with a key. The interface's own session is never turned
away, whatever this says: switching it off cannot lock out the person who did.

### `api.tmdbMembers`

Let members — in person, or with their keys — use the relays: TMDB's, TheTVDB's and
AniList's, which spend the operator's quotas. Editors and administrators may whenever a
relay is on.

### `tmdb.relayCredential`

Whose key the TMDB relay asks TMDB with: `server`, this server's (`AMS_TMDB_API_KEY`), in
place of whatever the client sent; `client`, the client's own, handed on as it came — a key
issued here is then refused, and the list of allowed addresses guards the relay. See [Whose
key the service is asked with](relays.md#whose-key-the-service-is-asked-with).

### `tvdb.relayCredential`

The same for the TheTVDB relay: `server` asks TheTVDB with this server's key
(`AMS_TVDB_API_KEY`); `client` has every client sign in with its own TheTVDB key, and
refuses a key issued here.

## Signing in through an identity provider

On **Opening & APIs**, and written together through `PUT /api/v1/admin/oidc`, where they are
checked as a whole. The provider is offered once `oidc.enabled` is on and the issuer, the
client ID and `AMS_PUBLIC_URL` are set. See [Signing in through an identity
provider](access.md#signing-in-through-an-identity-provider).

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`oidc.enabled`](#oidcenabled) | `true` or `false` | `false` | server |
| [`oidc.issuer`](#oidcissuer) | text, or nothing | unset | server |
| [`oidc.clientId`](#oidcclientid) | text, or nothing | unset | server |
| [`oidc.clientSecret`](#oidcclientsecret) | a secret, never read back | unset | server |
| [`oidc.scopes`](#oidcscopes) | text | `openid profile email` | server |
| [`oidc.buttonLabel`](#oidcbuttonlabel) | text, or nothing | unset | server |
| [`oidc.autoRegister`](#oidcautoregister) | `true` or `false` | `false` | server |
| [`oidc.roleClaim`](#oidcroleclaim) | text, or nothing | unset | server |
| [`oidc.adminValues`](#oidcadminvalues) | text, or nothing | unset | server |
| [`oidc.editorValues`](#oidceditorvalues) | text, or nothing | unset | server |
| [`auth.passwordLogin`](#authpasswordlogin) | `true` or `false` | `true` | server |

### `oidc.enabled`

Offer signing in through the identity provider.

### `oidc.issuer`

The provider's issuer URL, which its discovery document is read from. HTTPS, unless it is a
local address.

### `oidc.clientId`

The client ID registered at the provider.

### `oidc.clientSecret`

The client's secret. `AMS_OIDC_CLIENT_SECRET` wins over it when it is set, and keeps it out
of the database.

### `oidc.scopes`

The scopes asked for, separated by spaces; `openid` is always asked for.

### `oidc.buttonLabel`

What the sign-in page's button says. Empty, *Continue with the identity provider*.

### `oidc.autoRegister`

Open an account for someone the provider vouches for and who has none here yet: a member's,
unless the role claim says more — and waiting for approval when sign-ups do. Off, only an
account already tied to the provider signs in through it.

### `oidc.roleClaim`

The claim roles are read from: `groups`, or a path such as `realm_access.roles`. Named, the
provider decides the role at every sign-in, but never takes away the last administrator.

### `oidc.adminValues`

The values of the role claim that make an administrator, separated by commas.

### `oidc.editorValues`

The values of the role claim that make an editor, separated by commas.

### `auth.passwordLogin`

Offer the password form. It can be switched off only once the provider answers its discovery
test and somebody could still be let back in. Off, only the provider signs people in —
except the account `AMS_ADMIN_USERNAME` names, which keeps its password for the day the
provider is down — and sign-ups by password close with it. It counts as on while no provider
is set up, and `AMS_FORCE_PASSWORD_LOGIN=true` keeps it on whatever this says.

## Sonarr and Radarr

On the **Sonarr & Radarr** page: the operator's own Sonarr and Radarr, reached the other
way, to tell them of a change and to read what they hold. Written through
`PUT /api/v1/admin/arr`, which checks an address as an address before it keeps it. See
[Connecting your Sonarr and Radarr](sonarr-radarr.md#connecting-your-sonarr-and-radarr).

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`arr.sonarrUrl`](#arrsonarrurl) | text, or nothing | unset | server |
| [`arr.sonarrKey`](#arrsonarrkey) | a secret, never read back | unset | server |
| [`arr.radarrUrl`](#arrradarrurl) | text, or nothing | unset | server |
| [`arr.radarrKey`](#arrradarrkey) | a secret, never read back | unset | server |
| [`arr.pushRefresh`](#arrpushrefresh) | `true` or `false` | `false` | server |

### `arr.sonarrUrl`

Where this server reaches Sonarr, with its URL base when it has one: `http://sonarr:8989`.
Empty, Sonarr is not connected.

### `arr.sonarrKey`

Sonarr's API key, from its *Settings › General*.

### `arr.radarrUrl`

Where this server reaches Radarr: `http://radarr:7878`. Empty, Radarr is not connected.

### `arr.radarrKey`

Radarr's API key.

### `arr.pushRefresh`

Ask Sonarr or Radarr to read a work again as soon as it changes here — a lock, an edit, a
sync, a refresh that changed it — rather than at their next refresh. See [Telling them of a
change](sonarr-radarr.md#telling-them-of-a-change).

## Media

On the **Media** page, and read only when a store is configured (`AMS_MEDIA_STORAGE`). See
[Keeping the media](media.md).

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`media.store`](#mediastore) | `true` or `false` | `true` | server |
| [`media.serve`](#mediaserve) | `proxy` or `redirect` | `proxy` | server |
| [`media.people`](#mediapeople) | `true` or `false` | `true` | server |
| [`media.audio`](#mediaaudio) | `true` or `false` | `true` | server |

### `media.store`

Fetch and keep the media a work points at, as the work is stored.

### `media.serve`

How a copy kept in a bucket reaches a reader: `proxy`, through this server; `redirect`, sent
to the bucket's own address, signed for an hour. A copy on disk is always served by this
server.

### `media.people`

The cast's photographs too: one per person, many per work.

### `media.audio`

The themes: a Fan-Kai's, a few megabytes each.

## Cache

On the **Cache** page, one switch per space: off, a space answers nothing and keeps nothing,
and what it held expires. See [Caching](caching.md).

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`cache.items`](#cacheitems) | `true` or `false` | `true` | server |
| [`cache.searches`](#cachesearches) | `true` or `false` | `true` | server |
| [`cache.lists`](#cachelists) | `true` or `false` | `true` | server |
| [`cache.relay`](#cacherelay) | `true` or `false` | `true` | server |
| [`cache.sessions`](#cachesessions) | `true` or `false` | `true` | server |

### `cache.items`

Works, merged, as they are served.

### `cache.searches`

Searches, and pages of the catalogue.

### `cache.lists`

Whole lists relayed from a metadata service — IMDb's Top 250 is thirteen megabytes — kept a
day, apart from the searches.

### `cache.relay`

What the TMDB, TheTVDB and AniList relays were answered with this server's credentials,
shared between clients.

### `cache.sessions`

Who a session belongs to, kept a few seconds rather than read from the database on every
request. Memory alone.

## Updates

On **Settings › Everything else**.

| Setting | Holds | Default | Set at |
| --- | --- | --- | --- |
| [`updates.check`](#updatescheck) | `true` or `false` | unset | server |

### `updates.check`

Once a day, and a minute after the start, ask GitHub for this server's latest release, so
the dashboard can say when a newer one is out. What is sent is one anonymous `GET` to
`api.github.com`, with the name and version every request of this server carries — nothing
about this instance or what it holds. Unset, it is off.
