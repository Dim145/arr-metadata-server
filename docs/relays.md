# The relays: TMDB, TheTVDB and AniList

Clients that ask TMDB, TheTVDB or AniList themselves — Jellyseerr and Overseerr,
Yamtrack, Jellyfin's TheTVDB plugin, the anime trackers — reach those services
through this server once their names resolve here. Each request is handed on to
the real service, and its answer is handed back with the fields locked in this
catalogue written in. The routes are the TMDB relay (`/3/*`, and the public lists of
`/4/list/*`), the TheTVDB relay (`/v4/*`) and the AniList relay
(`graphql.anilist.co`, on the clients' door) — and, where an administrator chose it,
TMDB's pictures under `/tmdb-images/*`, on the interface's door.

[Pointing your stack at this server](integration.md#tmdb-clients) says how to
redirect a client and make it trust the certificate. This page says whose key the
services are asked with, what is written into their answers, what is taken out
of them, what answers when a service fails, and how TMDB's pictures can come from
this server.

The TMDB and TheTVDB relays read and do not write: a request that is not a `GET`
or a `HEAD` is refused with `403` whoever asks, TheTVDB's sign-in aside.

## Whose key the service is asked with

Which key asks TMDB or TheTVDB is a choice on **Opening & APIs › Whose key the
relays ask with**, one for each relay, written the moment it is made:

| Setting | Values | Default |
|---|---|---|
| `tmdb.relayCredential` | `server`, `client` | `server` |
| `tvdb.relayCredential` | `server`, `client` | `server` |

**`server`** is what the relays always did. The operator's key serves every client.

- *TMDB.* This server's key (`AMS_TMDB_API_KEY`) is used in place of whatever the
  client sent. A key issued here, given to the client as its TMDB key,
  authenticates here and never reaches TMDB. With no key of its own, the relay
  answers `503`.
- *TheTVDB.* A client signs in (`POST /v4/login`) with a key issued here and is
  answered that same key as its token; TheTVDB is asked with this server's own
  token (`AMS_TVDB_API_KEY`, `AMS_TVDB_PIN`). A client's own TheTVDB key, let
  through by address (`AMS_TVDB_AUTH=allowlist`), is signed in with at TheTVDB and
  its token travels as it came.

**`client`** hands each client's own key on, as it came, and never uses the
server's:

- *TMDB.* The client's own v3 `api_key` in the query, or its v4 read token in
  `Authorization`.
- *TheTVDB.* Every client signs in with its own TheTVDB key, handed on with its
  PIN alone, and its token travels as it came.

With `client`:

- **A key issued here is refused.** Wherever it is presented — in the query
  (`api_key`, `apikey`), in `Authorization` under any scheme and in any case, in
  `X-Api-Key`, or in a TheTVDB sign-in — the answer is `401` with
  `{"error": "own_key_required", "message": …}`, before the service is asked
  anything. It is never handed on beside a client's own token. A request with no
  key at all is passed on, and the service answers it as it answers any.
- **Nothing is kept for another client.** What a client's own key was answered is
  that client's: the relay's cache holds only what the server's key was answered,
  and a client's own request is never shared with another caller asking the same,
  nor answered with what was kept when the service fails.
- **The surface is guarded by address.** A key issued here being refused there,
  `AMS_TMDB_AUTH=apikey` or `AMS_TVDB_AUTH=apikey` is read as `allowlist` for that
  relay: the addresses listed on **Keys & network** guard it. `allowlist` and `open`
  stay what they are. The Opening & APIs page says so under the choice, and the
  relay's row, the configuration page and the dashboard show the policy that
  applies.
- **The locks are written in all the same.** The choice is about whose key asks,
  not about what is answered.

`AMS_TVDB_AUTH` keeps saying how the TheTVDB surface is guarded. Under `server`,
`apikey` has a key issued here sign in and `allowlist` lets a client's own key
travel too, with a key issued here still standing in; `server` is what both do, and
what every deployment starts on. `tvdb.relayCredential = client` refuses a key
issued here and guards the surface by address, whatever the variable says.

A TheTVDB sign-in names its key `apikey`, as TheTVDB documents it, or `apiKey`, as
Seerr writes it; one that names both is refused with `400`. A client's own key is
signed in with at TheTVDB as the key and its PIN alone: whatever else a client
writes beside them — Kodi's scraper adds its user's gender, year of birth and a
device id — stays here.

Which to choose: with `server`, the operator's key serves every client and the
operator answers to TMDB and TheTVDB for that use — their terms, their quotas. With
`client`, each client uses its own key and its own quota, and the relay only writes
the corrections in. A member's key is kept off every relay until **Members too** is
on, on the same page (`api.tmdbMembers`).

## What is written into the answers

Only what a person **locked** here, and only into fields the service's own
document carries; a field nobody locked is the service's.

A text — a title, an overview — is written as it is locked for the language the
client asked (TMDB's `language=`, `en-US` when none is; TheTVDB's language in the
path or in the translation):

- in the language the catalogue is stored in (`AMS_TMDB_LANGUAGE`, by its language
  whatever the region: `fr` and `fr-CA` are French), the text locked in that
  language, else the text locked in every language;
- in another language, only a text locked in that language itself. A text locked in
  every language — which is every lock made without a language, so every lock made
  before 0.8.0 — is the catalogue's own, and the service's document in another
  language keeps the service's text: a German Jellyseerr is still answered TMDB's own
  German.

A date, a runtime or a chosen picture is written into every document. TMDB's
homepage and the genres' names are written only for a client asking in the
catalogue's language, since TMDB names genres in the language asked; the genres keep
TMDB's own ids, and when a genre locked here is not among TMDB's for that title,
TMDB's are left as they are.

**TMDB** (`/3/*`):

| Path | Written |
|---|---|
| `/3/movie/{id}`, `/3/tv/{id}` | title, original title, overview, dates, status, runtime, chosen poster and background; the homepage and the genres as above |
| `/3/tv/{id}/season/{n}` | the season's name, overview, date and chosen poster; each episode's title, overview, date, runtime and still |
| `/3/tv/{id}/season/{n}/episode/{e}` | the episode's title, overview, date, runtime and still |
| `/3/search/{movie,tv,multi}`, `/3/trending/{movie,tv,all}/{window}`, `/3/discover/{movie,tv}` | each result the catalogue holds with a lock: title, original title, overview, date, poster, background. People are left alone |

A series' own document carries what a client asked with it, and each part takes
what the same path takes when it is asked alone: `season/{n}` the season's locks
and its episodes', `season/{n}/episode/{e}` the episode's, and `season/{n}/images`
loses the posters a person hid. This is how Kodi's TV scraper and Yamtrack read
seasons (`append_to_response=season/1,season/1/images,…`). What else is asked with
it — a season's or an episode's people, an episode's stills, ids, videos, where it
can be watched — is TMDB's, there as on its own.

`/3/movie/changes` and `/3/tv/changes` list, on their first page and after TMDB's
own, the TMDB ids of the works changed here in the window asked — from the start of
`start_date` to the end of `end_date`, the last day when no start is asked, fourteen
days at the most: a lock put on or lifted, something added or taken out by hand, a
refresh that changed a value. Each is written as TMDB lists one,
`{"id": …, "adult": …}`, none twice, a title for adults only where this server shows
them, and `total_results` counts them. TMDB's page is kept a quarter of an hour and
the works changed here are laid on it at each answer, so a change shows at once: a
client that keeps itself in step by changes, as Yamtrack does, fetches a work
corrected here again without waiting for its own full refresh. When TMDB fails, the
feed answers with TMDB's failure, never with what was kept nor with this server's
changes alone: the client asks the same window again.

A regular episode is paired with the catalogue's by its numbers, a special by the
TMDB id the catalogue gave it, and neither when the catalogue knows it under another
TMDB id. Pictures are written by TMDB's own path, so only a picture TMDB has can be
named. TMDB's answers are kept a while and shared by every client asking with the
server's key; see
[TMDB clients](integration.md#tmdb-clients).

**TheTVDB** (`/v4/*`):

| Path | Written |
|---|---|
| `series/{id}`, `series/{id}/extended`, `movies/{id}[/extended]` | name, overview, year, dates, runtime, status, chosen poster. With `meta=translations`, every language's entry, and an entry for the catalogue's language and for each language a text was locked in alone where TheTVDB has none, as Jellyfin's plugin reads them; with `meta=episodes`, the episodes; the seasons of the aired order, with their locked titles |
| `series/{id}/translations/{lang}`, `movies/{id}/translations/{lang}` | name and overview, as locked for that language |
| `series/{id}/episodes/{order}[/{lang}]` | each episode's title, overview, date, runtime and still, told by its TheTVDB id in every order (`official`, `default`, `dvd`, `absolute`…), and by its numbers too in the aired order. This is what Seerr's TheTVDB option reads |
| `seasons/{id}`, `seasons/{id}/extended` | the season's locked title as its name, its chosen poster, its episodes |
| `episodes/{id}[/extended]`, `episodes/{id}/translations/{lang}` | the episode's title, overview, date, runtime and still, and its translations |
| `updates` | on the first page (`page=0`, or none), after TheTVDB's own records, an `update` record for each series and film of the catalogue that changed here since `since` — a lock set or lifted, something added or taken out by hand, a refresh that changed a value — by its TheTVDB id, under `entityType` `series` or `movies`, written as TheTVDB documents an `EntityUpdate`; none TheTVDB lists already, none for another `type`, for `action=delete` or on a later page; fourteen days back at the most. This is what Jellyfin's plugin syncs by |

A TheTVDB record names a work in its original language, which no client asks for by
name: its own `name` and `overview` take the lock as the catalogue holds the work,
whatever language that is in, and the translations it carries take each what was
locked for their language. `seasons/{id}/translations/{lang}` is not written: it
does not say whose season it is.

A series' episodes in an order the catalogue keeps and TheTVDB does not — a TMDB
group's, `tmdb-…`, one made here, `ams-…` — are answered here, as TheTVDB writes a
page of them, every episode on the first; so are the aired order's (`official`,
`default`) and the episodes a series' record carries with `meta=episodes`, while
an order is frozen as the aired numbering; and an order of TheTVDB's own it
answers with no episode — `altdvd`, `alttwo` (thetvdb/v4-api#340) — is filled with
what was read of it season by season. Those pages need no TheTVDB key of the
server's own: without one, a client signs in with a key issued here all the same
and reads them, `altdvd` and `alttwo` from what was read; whatever only TheTVDB can
answer is `503`. See [Orders](sonarr-radarr.md#orders).

**AniList** (`graphql.anilist.co`): the title, description, genres and pictures of
each entry the answer carries. AniList has no key to choose.

An AniList answer is kept an hour, and AniList's genre and tag lists, asked alone,
half a day. One to a query signed with a person's token, to a mutation, or about
somebody — their lists, their account, who they follow, what they liked, posted or
reviewed — is never kept, never shared with another caller and never answered
stale (see [TheTVDB and AniList clients](integration.md#thetvdb-and-anilist-clients)).

## What is taken out

A credit, a picture or an alternative title that a source gave and a person
[hid](editing.md#hiding-and-correcting-what-the-sources-gave) is taken out of the
lists the relays hand on, each told by what the service itself names it by:

| Relay | List | Told by |
|---|---|---|
| TMDB | `cast` and `crew` of `/credits`, a series' `/aggregate_credits` and of both asked with `append_to_response`; a series' `created_by` | the person's TMDB id and the kind of credit: the cast are actors; of the crew, a `Director` job is a director, a `Producer` job a producer, any job of the `Writing` department a writer, as the catalogue files them; a creator is a writer |
| TMDB | `posters`, `backdrops`, `logos` of `/images`, a work's or a season's, of `images` asked with it, and of `season/{n}/images` asked with a series | the path TMDB files the picture under |
| TMDB | `titles` (a film's) or `results` (a series') of `/alternative_titles`, and of `alternative_titles` asked with it | the text, case and blanks aside, and the country TMDB files it for |
| TheTVDB | `artworks` of a series' or a film's record, and a series' `/artworks`; a season's `artwork` | the picture's address |
| TheTVDB | `aliases` | the text, case and blanks aside, in whatever language |
| TheTVDB | `characters` of a series' or a film's record | the person's name, case and blanks aside, and the kind of credit TheTVDB gives |

What cannot be told apart safely is left as the service gives it, rather than
guessed at:

- a credit a source named by name alone — one of Skyhook's, Fan-Kai's or Radarr's
  service's, which give no TMDB id — is not looked for in TMDB's lists: TMDB names
  every person by an id, and a name could be somebody else's;
- TheTVDB's people share no id with TMDB's, so they are told by name: a namesake
  in the same role on the same work goes with the person hidden;
- a season's or an episode's people — TMDB's season credits, an episode's guest
  stars and crew, TheTVDB's episode characters — are not the work's credits, which
  are all a person can hide: they stay;
- the picture a document leads with (TMDB's `poster_path` and `backdrop_path`,
  TheTVDB's `image`) stays when it names a hidden picture, since another in its
  place would be a choice nobody made: choose the picture to lead with, and the
  relays write it;
- a credit that was corrected — its character, its place — is written into no
  relayed list: hiding is all the relays carry of a credit;
- a title TMDB lists for several countries is kept once by the catalogue, under the
  first; hidden, it goes from TMDB's list under that country alone.

## When a service fails

A relay keeps what TMDB, TheTVDB and AniList answered this server's own keys, and
that answer stands in when the service fails: no answer in time, a connection
refused, too many requests (`429`), an error of its own (`500`, `502`, `503`,
`504`); for AniList a `403` too, which is how it says its API is off; for TheTVDB,
a refusal to sign this server in. The client is answered the last good answer to
the same request, with an `Age` header that says how many seconds old it is, and
with the locks written in as they are into any other.

| Service | A kept answer stands in while it is under | Reference lists kept in the database too |
|---|---|---|
| TMDB | a week | `/3/configuration` and what is under it, `/3/genre/{movie,tv}/list`, `/3/certification/{movie,tv}/list`, `/3/watch/providers/regions` |
| TheTVDB | a week | `languages`, `countries`, `genres`, `content/ratings`, `entities`, `genders`, and the lists of types and statuses (`artwork/types`, `series/statuses`, `people/types`…) |
| AniList | a day | none: its terms want nothing stored |

Nothing else stands in:

- a refusal of the request itself — `400`, `401`, `404`, and TMDB's or TheTVDB's
  `403` — is handed on as it came;
- so is any failure of a feed of changes — TMDB's `/3/movie/changes`,
  `/3/tv/changes` and a work's own `…/changes`, TheTVDB's `/v4/updates`: a client
  that moves a cursor along one asks the same window again, rather than miss what
  changed while the service was down;
- what a client asked with its own key or token, what mints or reads a TMDB
  session, and an AniList query signed with a person's token, that changes
  something or is about somebody, is never kept: nothing stands in for it, and
  nothing it was answered stands in for anybody else.

The reference lists are what a client cannot start without. Each is kept in the
database too, written again each time the service answers it, and read when the
service fails and the memory holds nothing — after a restart, for instance. One
older than 180 days is neither served nor kept, and the server deletes it as it
goes; a database transfer carries the others.

`relays.staleIfError`, on unless an administrator turns it off, decides all of
this: off, the service's failure is handed on as it came, and the reference lists,
still written, are not read. It is set on **Opening & APIs**, under *When a service
fails*, beside whose key the relays ask with; see
[Accounts and access](access.md#accounts-sign-ups-and-api-switches). What stood in
is counted in `ams_relay_stale_total`, by relay and by where it came from
(`memory`, `database`), and each is said in the log.

Requests asked the same at once share one exchange with the service, on each
instance, and the relays go through the door the server's own fetches from each
service take: as many calls at once between them — sixteen to TMDB, eight to
TheTVDB, four to AniList — and the same pause when the service answers `429`, or
`503` with a `Retry-After`, for as long as it asks, a minute at the most. A relay
never waits for that pause to end, nor longer than ten seconds for a place: it
answers with what it kept, or `429` with the seconds left in `Retry-After`. A
client's AniList query is never spaced out as the server's own fetches from AniList
are.

A client's `If-None-Match` and `If-Modified-Since` reach the service only with a
request answered for that client alone, and handed back as the service wrote it: a
request kept or shared with others is asked without them, since a "not modified" is
one caller's answer. TheTVDB's sign-in goes through the same door as its reads:
while the door is closed, a client signing in with its own key is answered `429`
with `Retry-After`. A key issued here is answered here, door or no door.

## TMDB's pictures, from this server

Jellyfin, Kodi, Kometa and Emby build every TMDB picture's address from
`/3/configuration`: its `images.secure_base_url`, then a size and the picture's
path. **Opening & APIs** chooses that address, under *Where TMDB's clients fetch
the pictures* (`tmdb.images`):

| Value | `images.base_url` and `images.secure_base_url` |
|---|---|
| `tmdb` (default) | TMDB's own, as TMDB answers them |
| `server` | `{AMS_PUBLIC_URL}/tmdb-images/`, this server's gate |

`server` takes the media store (`AMS_MEDIA_STORAGE`, see
[Keeping the media](media.md)) and `AMS_PUBLIC_URL`; without either it reads as
`tmdb`, and the page says which is missing. The configuration is written as it is
served — fresh, kept, or standing in for TMDB — so a change shows at once; but
Kodi keeps the address it was given for a month, and the gate answers whatever the
setting says now. Seerr and Yamtrack have the reader's browser fetch TMDB's
pictures, at addresses of their own, and are not sent here.

The gate, `GET /tmdb-images/{size}/{file}`, is on the interface's door:

- a picture the media store keeps — TMDB's original, fetched for a work of the
  catalogue — is served from the store, whatever the size asked: this version makes
  no smaller copy of it for a TMDB client;
- any other is fetched from image.tmdb.org at the size asked and handed on as it
  comes, kept nowhere: `404` when TMDB has no such picture at that size, `502` when
  TMDB cannot be reached;
- only TMDB's sizes are taken — those its configuration lists, `original`, `w45`
  to `w1280` and `h632`, and the crops of its site (`w600_and_h900_bestv2`,
  `w300_and_h450_face`, `w1920_and_h800_multi_faces`…) — and only a picture's name
  as TMDB gives it, ending in `.jpg` or `.png`; anything else is `400`. Nothing but
  TMDB's pictures can be had through it.

It is guarded as the TMDB relay is — `api.tmdb` off or `AMS_TMDB_PASSTHROUGH=false`
answer `503` there too — except that a key policy (`AMS_TMDB_AUTH=apikey`) reads as
`allowlist`: a picture's address carries no key. A client has to reach
`AMS_PUBLIC_URL` from an address on **Keys & network**, where one turned away is
listed among those who have called, with the button that allows it. An address the
gate lets in is not held to `AMS_RATE_LIMIT_PER_MINUTE` — a library's scan asks for
pictures in bursts — while one it turns away is counted as on any surface. A
picture is no call to TMDB's API: **Opening & APIs** does not count it among the
relay's calls, and the metrics file it under the `media` surface. Keep the gate to
the operator's own clients: the pictures are TMDB's and their owners'.

## Answers worth knowing

| Answer | When |
|---|---|
| `200` with `Age` | The service failed, and what was kept answered instead (see [When a service fails](#when-a-service-fails)). |
| `400` | A TheTVDB sign-in that is none — no key, or a key named both `apikey` and `apiKey` — or, under `/tmdb-images/`, a size or a name that is not TMDB's. |
| `401`, `own_key_required` | With `client`, a key issued here was presented. |
| `401` | Under a key policy, no key this server takes was presented. |
| `403` | A member's key while **Members too** is off (`relay_not_for_members`), or a write, or TheTVDB's operator account asked for with a key issued here. |
| `403`, `status_code` 3 | A path of TMDB's v4 API other than a public list (`/4/list/{id}`): an account's lists, ratings or watchlist, TMDB's sign-in. In TMDB's own shape (`success`, `status_code`, `status_message`), the message naming this server, its version and `AMS_PUBLIC_URL`. Radarr's *TMDb User* import list asks for these and imports nothing: nothing is made up in an account's place, since an empty list would read as one emptied. |
| `429` with `Retry-After` | The service answered `429` itself, or asked to be left alone a moment ago (a `429`, or a `503` with `Retry-After`), or every call this server makes to it at once is under way — and nothing was kept to answer with. The same status, with `RateLimit` fields, when the caller's address spent its `AMS_RATE_LIMIT_PER_MINUTE`. |
| `502` | The service could not be reached or said nothing in time, and nothing was kept to answer with. |
| `503` | The relay is switched off — `api.tmdb`, `api.tvdb`, `api.anilist`, or `AMS_TMDB_PASSTHROUGH` and its like at start, the TMDB one answering `503` to `/tmdb-images/` too — or with `server` this server holds no key of its own to stand in with: for TheTVDB, for a document it does not answer itself. Also TheTVDB's relay for a minute or more after TheTVDB refused to sign this server in, when nothing was kept. |
| `508` | The relay was asked by an instance of this server: the service's name resolves back here. Set `AMS_TMDB_UPSTREAM` or `AMS_TVDB_UPSTREAM`; see [If you redirect at the resolver](integration.md#if-you-redirect-at-the-resolver). |
