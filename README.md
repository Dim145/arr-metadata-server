# arr-metadata-server

A self-hosted metadata server for your media stack. It speaks the protocols
Sonarr, Radarr and TMDB clients already use, keeps its own canonical copy of
everything it serves, and lets you correct or invent entries by hand — with
manual edits permanently protected from automatic refreshes.

It replaces and merges two earlier projects: `the earlier TMDB relay` and
`the earlier Skyhook stand-in`.

> **Status: working.** Every surface is implemented, exercised by tests on both
> database engines, and verified against the live TMDB API and against real
> Sonarr, Radarr and Jellyseerr containers — including that a refresh leaves
> manually locked fields alone. See [`docs/integration.md`](docs/integration.md).

## What it does

- **One metadata source for the whole stack.** Sonarr's Skyhook API, Radarr's
  metadata API and the TMDB v3 API are all served from the same store.
- **Several sources behind it.** TMDB, plus Sonarr's and Radarr's own metadata
  services, merged field by field. They carry what TMDB has no field for: air
  time, TVMaze and AniList ids, absolute episode numbering, certifications by
  country, and IMDb/Metacritic/Rotten Tomatoes ratings.
- **Its own canonical database.** Provider payloads are kept verbatim as
  snapshots and merged into a canonical entity; nothing downstream depends on a
  provider being reachable.
- **Manual entries.** Create a work that exists on no provider at all.
- **Edits that stick.** Change a title, an air date, an episode order — the
  field is locked and every later refresh leaves it alone. The catalogue's
  filters, orders and counts follow the edit too: a work locked to Animation
  is found under Animation.
- **Automatic refresh.** Any entry carrying at least one external id is
  refreshed on a schedule that adapts to its status.
- **Client management.** Named API keys, scopes, expiry, per-surface policy, and
  an escape hatch to turn authentication off entirely.
- **Per-language answers.** Sonarr asks in its URL, the native API takes
  `?language=`. Translations are fetched the first time a language is asked for
  and kept — from TMDB, then TheTVDB for the many languages TMDB does not carry.
  A locked field stays locked in every language.
- **Curated lists.** Selections composed by hand or by a filter, shown on
  the site and served to Sonarr and Radarr as the custom lists their import
  lists read.
- **The catalogue in numbers, the server's health, and webhooks.** A public
  page counts what the catalogue holds by decade, genre, network, language
  and score; the administrator's dashboard says what the server is, keeps and
  did last; and what happens can be posted to a webhook — Discord, Slack or
  anything that takes JSON — for the actions chosen (`webhooks.url`,
  `webhooks.events`). To begin with, the actions are what enters and leaves
  the catalogue: `item.imported`, `item.created`, `item.deleted` and
  `dataset.imported`; `*` is every one. A failed sign-in is posted without
  the name that was typed, and the webhook address itself is never posted.
- **A command palette, and actions on many works at once.** ⌘K or Ctrl K
  anywhere finds a place to go or a work by its title; in the catalogue,
  whoever maintains it selects rows and refreshes, switches off or on,
  deletes, or adds them to a list, all at once — each work its own request
  and its own line in the audit trail.
- **What goes with a work.** A work's page offers the catalogue's own works
  in the same vein — its kind, sharing its genres, keywords, network or
  decade — and a film's whole collection, held or not, on a page of its own;
  whoever maintains the catalogue is shown what TMDB recommends beside a
  work, to import in one click.
- **Where to watch.** A work's page lists the services carrying it in a
  country — streaming, rent, buy — from the JustWatch data TMDB serves, named
  beside it as TMDB's terms ask; the country is a setting (`tmdb.watchRegion`,
  per key or address too), or the one the language of the answers names.
- **A page that installs, links that unfurl, answers that revalidate.** The
  site is a web app a phone installs; a link to a work or a list carries its
  title, line and poster for whoever unfurls it, while the catalogue is open;
  API answers carry a validator, so a client holding one is told "unchanged"
  rather than sent it again.
- **Calendars and feeds.** The schedule, and any one work, as an iCalendar a
  phone subscribes to; what arrives and what airs as Atom feeds.
- **`.nfo` export with the artwork beside it**, in the layout Kodi defined and
  Plex's Personal Media agent reads. It is the only route to Plex.
- **Two interfaces, in English or French.** A catalogue anyone can browse —
  posters, seasons, cast, the record — and an administration side for the
  people who maintain it. Both switch language from the bar and remember the
  choice.
- **A catalogue to read, not only to feed clients.** Every season and every
  episode has its page, every credited person a filmography of what is held
  here, and a schedule lists what airs each week in the reader's own timezone.
  The list is filtered by genre, years, score, status, language and network,
  and sorted by popularity, score, date, title or when a work arrived — all of
  it kept in the address, so a view can be sent as a link — by the values each
  work's page shows, IMDb's latest score included when that list is on. A
  series' combined genres count as the film genres they stand for: "Action &
  Adventure" is found under Action and under Adventure, beside the films.
  The search lists what it finds as it is typed — `/` puts the cursor in it —
  and the front page opens on one of the most followed works, its own logo
  over its backdrop, a different one each day. Each identifier opens the
  work's page on the site it came from; the artwork opens in a viewer; a
  trailer plays from YouTube's no-cookie domain, and only once somebody
  presses play.
- **A season chart, past and to come.** One page for each quarter of the
  calendar — winter from January, spring from April, summer from July, autumn
  from October — as anime season charts have it, for series and films alike:
  the new series, the series back for another season, the films released and
  what carries on from before, each placed by its episodes' dates (corrected
  ones included) or its release. A strip of the season's weeks leads to each;
  kind, trailer, genre and original language narrow it, all kept in the
  address. For whoever maintains the catalogue, the most popular of what TMDB
  lists for the same quarter that the catalogue lacks follows, each a button
  away from being imported.
- **Maintenance where the problem is.** Season and episode fields are locked
  one by one from the editor, which opens on the episode a public page came
  from; each provider's last answer can be read as it arrived; the works whose
  last refresh failed are one filter away, and flagged on the dashboard.

## Requirements

- Rust 1.94+ (only to build from source; a container image is provided)
- SQLite (bundled, nothing to install) or PostgreSQL 14+
- A TMDB API key, if you want it to fetch anything

## Quick start

```bash
cp .env.example .env
# set AMS_TMDB_API_KEY, and AMS_ADMIN_USERNAME / AMS_ADMIN_PASSWORD
cargo run --release
```

The server listens on `0.0.0.0:8080` and creates `data/ams.db` on first start.

### PostgreSQL instead of SQLite

```bash
AMS_DATABASE_URL=postgres://ams:password@localhost:5432/ams cargo run --release
```

The schema is maintained separately for each engine, under `migrations/`.
Migrations run automatically at startup.

## Configuration

Every option is an environment variable prefixed `AMS_`. See
[`.env.example`](.env.example) for the annotated list. Variable names from the
two predecessor projects (`TMDB_API_KEY`, `BIND_ADDRESS`, `SKYHOOK_BASE_URL`, …)
are still accepted, so an existing `.env` keeps working.

## API documentation

Every route the server answers — its own API plus the three compatibility
surfaces — is documented at `/api/docs`, with the spec at `/api/openapi.json`.
Both are generated from the handlers themselves, so they cannot drift from what
is actually served, and both sit behind the same credential as the rest of the
native API.

Every whole `GET` answer under the API — not the documentation, nor a body
over eight megabytes — carries a weak `ETag`, taken from the bytes before
compression. Send it back as `If-None-Match` and an unchanged answer is
a `304` with no body — the interface, Seerr and a feed reader all save the
transfer, and a poll of a calendar feed costs a header exchange. Answers are
marked `private, no-cache` unless a handler says how long they may be kept, so
a browser keeps them and asks.

## Where the data comes from

Four sources are asked at once and merged into one entity:

| Source | Brings |
| --- | --- |
| **TMDB** | overviews, stills and translations, in the configured language |
| **TheTVDB** v4 | the season and episode numbering, absolute numbers, air-order hints, broadcast time of day, per-country certifications |
| **Skyhook** (Sonarr) / **api.radarr.video** | the same view the arr stack would have got on its own, including ratings it carries and TMDB does not |
| **Fanart.tv** | transparent logos, clearart and banners — artwork only |

Only TMDB is really needed to start. The rest are optional and on by default;
each costs one extra call per refresh.

The merge is field by field, in `AMS_PROVIDER_PRIORITY` order:

- **Scalars** take the first provider that has anything, so a lower-priority
  source fills a gap rather than overwriting an answer.
- **Artwork, alternative titles, ratings and translations** are unioned.
- **Credits** come from one provider; nothing identifies a person across
  providers, so a union would list the same actor twice.
- **Episodes and seasons** are the exception, and the one rule worth knowing.

### Whose episode numbering wins

The **list** of episodes and seasons always comes from TheTVDB, or from Skyhook
which republishes its numbering. Every other provider may fill fields on those
episodes; none may add one.

This is not configurable, because it is not really a preference. A client
addresses a series by its TVDB id and expects the numbering that goes with it,
and the providers do not agree on where seasons end — so serving TMDB's
boundaries under a TVDB id makes Sonarr map files to the wrong episodes.

Taking the union of both is worse still: episodes that are the same hour of
television sit at different numbers in each scheme, so none of them match and
every one is kept twice. That is not hypothetical — it is what this server did
before the rule existed, and for *One Piece* it answered with 2352 episodes for
a show that has 1179.

Measured against the live APIs with all four providers configured:

| Series | Numbered episodes | With absolute numbers | With overviews |
| --- | --- | --- | --- |
| Breaking Bad | 62 | 62 | 62 |
| Attack on Titan | 89 | 89 | 89 |
| One Piece | 1179 | 1178 | 1178 |

Removing TMDB and leaving the other three changes none of those counts — that
is the point of the rule — but costs the artwork TMDB carries: Breaking Bad
drops from 82 images to 69.

A real Sonarr 4.0.20 served by this server stored those same counts with no gap
in the absolute numbering. A real Radarr 6.4.4 searched by id and by text and
added films with their studio, genres, certification and IMDb/TMDB/Trakt
ratings. Jellyseerr's ten TMDB endpoints all answer through the relay.

**Enrichment** is asking a provider on every fetch and merging what it says.
It is on by default and costs one extra call per refresh per provider. **Fallback**
is asking only when nothing else could answer, so with enrichment on it never
fires — the answer is already in hand, and a second call would buy it twice.

Both are settings, changed under **Settings** without a restart; the `AMS_*`
variables only seed them the first time the server starts.

A search is a ladder rather than a fan-out, because search results are cheap to
get and expensive to merge: what is already stored, then TMDB, then TheTVDB, then
Skyhook — each rung tried only while the ones above it found nothing. So a series
TMDB has never heard of is still found, and a deployment with no TMDB key at all
still answers.

> If you redirect `skyhook.sonarr.tv` or `api.radarr.video` at this server
> through your **resolver** rather than per container, it resolves those names
> to itself and would call itself forever. It detects that and answers `508`
> with the fix in the message — but point the upstreams elsewhere, or turn
> enrichment off, when that is your setup.

### Further sources

Four more, each off until switched on under **Settings → Further sources**.
None needs a key.

| Source | Brings | Costs |
| --- | --- | --- |
| **TVmaze** | the moment each episode aired | two calls per series fetch |
| **AniList** | its score, the romaji, native and English titles and synonyms, the main studio, an adult flag — for anime | one call per anime fetch |
| **MyAnimeList** | its score and titles — for anime; through its own API when `AMS_MAL_CLIENT_ID` is set, through [Jikan](https://jikan.moe) otherwise | one call per anime fetch |
| **IMDb** | IMDb's rating, for every work with an IMDb id | one download a day |

None of them changes the shape of an answer: Sonarr and Radarr are served the
same fields as before, some of them now more accurate.

**TVmaze corrects `airDateUtc`**, the moment after which Sonarr counts an
episode as aired. TheTVDB keeps one broadcast time per series and Skyhook stamps
it on every episode, so a show that changed slot is wrong for everything before
the change. TVmaze keeps a time per episode. Measured against Skyhook: all 62 of
Breaking Bad's episodes are placed at 21:00 ET, where 54 aired at 22:00 — the
last eight did air at 21:00, and on those it is TVmaze that is an hour late —
and 35 of The Big Bang Theory's move, season 3 having aired on Mondays at 21:30
rather than Thursdays at 20:00. Its time is taken only where its broadcast date
matches TheTVDB's for the same season and episode. It never adds, removes or
renumbers an episode.

**AniList and MyAnimeList** are asked about one entry: the one that stands for
the whole work, as the [Fribb anime-lists](https://github.com/Fribb/anime-lists)
id map places it — the first season's, for a series both sites split by cour.
The map is downloaded weekly while either source is on, and it also answers
Sonarr's `mal:` and `anilist:` searches, which is how its AniList and
MyAnimeList import lists find a series; those used to find only series already
stored here. Neither site is ever used for numbering. Sonarr ignores
alternative titles, so the romaji ones serve Radarr, which matches anime film
releases against them, and this interface. For anime, their main studio
replaces TMDB's, which is only the first production company TMDB lists.

**IMDb's rating** is what Sonarr is given for a series — Skyhook's rating is
IMDb's, republished — and otherwise the rating with the most votes behind it.
The daily list keeps that figure current and supplies it when Skyhook and
Radarr's service are off. Only the ratings of works stored here are kept, and
the list is fetched again within the hour when works are added.

The lists and their last download are shown under the switches, with a button
to download one now rather than wait for its turn, and every download is a job. Their terms: TVmaze's data is CC BY-SA; IMDb's datasets are
for personal, non-commercial use; AniList's API is free for non-commercial use
and asks not to be crawled or stored wholesale — this server asks only about
works a client requested. Every public page credits the sources that are on,
with the notices TMDB and IMDb require.

## Languages

Entries are stored in whatever `AMS_TMDB_LANGUAGE` is set to. **If you want your
stack in French, set that** — it is what every client gets by default, and it is
almost certainly the setting you want:

```bash
AMS_TMDB_LANGUAGE=fr-FR
```

Any *other* language is fetched the first time something asks for it and kept
from then on:

```bash
curl 'http://localhost:8080/v1/tvdb/shows/fr/81189'       # the Skyhook form
curl 'http://localhost:8080/api/v1/items/{id}?language=ja'
```

One caveat worth knowing before you rely on the per-request form: **Sonarr does
not use it.** Its request builder sets the language segment to `en` once and
never changes it, so every Sonarr request arrives as English whatever its own UI
language says. Per-request language is therefore for the web UI, for anything
calling this server directly, and for TMDB clients — which do send `language=`
and get it forwarded. For Sonarr, `AMS_TMDB_LANGUAGE` is the knob.

A translation never overwrites a field someone locked. That would undo a rename
the moment a client asked in another language, which is the one thing locking
exists to prevent. Lock a field per language if you want different text in each.

## Audit trail

Every action that changes what the server serves is recorded: who did it, to
what, and from where. Reads are not — one row per metadata request would bury
everything that matters under Sonarr's refresh traffic. Failed sign-ins are.

Browse it under **Audit** in the web UI, or at `GET /api/v1/audit`.
`AMS_AUDIT_RETENTION_DAYS` bounds how much is kept (90 days by default; `0`
keeps everything).

## Authentication

Authentication is configured **per API surface**, because the clients differ in
what they can send:

| Surface | Path | Default policy |
|---|---|---|
| Native API and web UI | `/api/v1/*` | API key (header or query) |
| Public browsing | a fixed subset of `/api/v1/*` | **off** (`AMS_PUBLIC_BROWSE`) |
| TMDB-compatible | `/3/*`, and the public lists of `/4/list/*` | API key (`api_key` query parameter) |
| Sonarr / Radarr compatible | `/v1/*`, including the IMDb lists Radarr imports from | IP allowlist |

### Letting anyone browse

`AMS_PUBLIC_BROWSE=true` opens the catalogue to a reader with no credential:
the list of works, one work, the totals, what the filters offer
(`/api/v1/facets`), the schedule (`/api/v1/calendar`), a season chart
(`/api/v1/seasons/{year}/{season}`), a person's credits
(`/api/v1/people/{tmdbId}`) and the feeds (below). Nothing else — not what TMDB lists for a season
(`…/candidates`), which is asked of TMDB on this server's key. Which works failed their last
refresh is not among them: that filter is ignored for anyone but an
administrator. It is an allowlist
rather than a denylist, so the settings, the job history, the audit trail, the
raw provider payloads and the record of who edited what all stay behind a
credential — and an endpoint added later is closed until somebody decides
otherwise. Administration is never reachable this way.

It is off by default, because turning it on publishes what this server knows to
whoever can reach the port.

### Feeds

The schedule, and one work's dates, as calendars a phone or a desktop
subscribes to; what arrives and what airs as Atom feeds a reader follows.
Read under the same rule as the pages they mirror: open under public
browsing, otherwise with a key, which a calendar app carries in the address
(`?apikey=…`).

| Feed | Path |
|---|---|
| The schedule: seven days back and twenty-eight ahead unless asked otherwise, at most sixty-two together | `/api/v1/calendar.ics?pastDays=7&futureDays=28&language=fr` |
| One work: a series' dated episodes, or a film's release day | `/api/v1/items/{id}/calendar.ics` |
| The fifty works most recently added | `/api/v1/feed/added.atom` |
| The episodes airing from yesterday to a week ahead | `/api/v1/feed/airing.atom` |

An episode whose time a provider knew is a timed event, as long as its runtime;
one with only a date is an all-day event on it. Under public browsing the
interface offers them — *Subscribe* on the schedule, *Add to my calendar* on a
work, the feeds in the footer — as `webcal://` addresses where a calendar app
expects one; otherwise it does not, since the app would arrive without a
credential, and the addresses take a key as above. Links in a feed point at
`AMS_PUBLIC_URL` when it is set — set it behind a reverse proxy — otherwise at
the host and scheme the request came to, as the proxy forwarded them if it is
among `AMS_TRUSTED_PROXIES`, or as the socket saw them. A window holding more
episodes than one answer carries is cut short and says so in an
`X-AMS-Truncated` header.

### Lists

Whoever maintains the catalogue composes lists under *Lists* in the
administration: a name, a description, what the list holds (series, films or
both), and either its members by hand, in an order, or a filter — genres,
keyword, years, score, status, language, network, order, how many — evaluated
whenever the list is read. Each is a page on the site, under *Selections*,
unless it is marked private; a work's own page says which hand-made
selections hold it.

Each list is also served in the shapes the clients import, so a selection
made here becomes what those clients add on their own:

| Client | Import list to add | Address | Shape |
|---|---|---|---|
| Sonarr | Settings → Import Lists → *Custom List* | `/api/v1/lists/{slug}/sonarr.json` | `[{ "title", "tvdbId" }]` — the series with a TheTVDB id |
| Radarr | Settings → Lists → *Custom Lists* | `/api/v1/lists/{slug}/radarr.json` | `[{ "id", "title" }]` — the films with a TMDB id, `id` being it |
| Radarr | Settings → Lists → *StevenLu Custom* | `/api/v1/lists/{slug}/stevenlu.json` | `[{ "title", "imdb_id" }]` — the films with an IMDb id |

The addresses are on each list's page, with a button to copy them. They are
read under the same rule as the page: open under public browsing, otherwise
with a key appended (`?apikey=…` — that spelling, which Sonarr's and Radarr's
logs redact; `api_key` works too but would be logged). A private list answers
nobody but a caller who may write, so importing one takes a write-scoped key:
make the list public instead where a client's key should not be able to edit
the catalogue. The whole list API is under `/api/v1/lists` and in the OpenAPI
document.

### What is a setting, and what is not

The environment configures the **deployment** — the port, the database, the
provider keys — and belongs beside the compose file. How the server **behaves**
is a setting, stored in the database and changed under **Settings** without a
restart: the language answers are given in, whether Sonarr's and Radarr's own
metadata services are used, whether refresh runs and how often, and whether
adult titles are served at all. The matching `AMS_*` variables seed those once,
the first time the server starts against an empty table, and are ignored after.

A setting can be answered at three scopes, narrowest first:

```
peer      an allowlist rule — how a client that sends no credential is known
 ↳ client an API key
    ↳ server
```

That is how one Radarr answers in French while everything else answers in
English. A client that presents a key is identified by it; Sonarr and Radarr
present nothing, so they are identified by the address they call from — which is
what an allowlist rule already records. Name the rule and it can carry settings;
the narrowest matching rule wins, so one container can differ from the network
around it. A name is used rather than an address because a container takes a new
address whenever it restarts.

### Adult titles

Off unless somebody turns them on. Four things decide who sees one, in order:

1. **the server** — `hidden` means nobody, whatever anything else says;
2. **the client's own policy** — `allow`, `deny`, or inherit the server's;
3. **whether the request gets a say** — `adult.force` discards the client's own
   `include_adult` and uses this server's answer instead, both towards the
   providers and when filtering the reply;
4. **what the request asked for**, where it still has a say.

Silence means no unless somebody said otherwise: a client that never mentions
adult titles is not asking for them.

### Putting something in the catalogue on purpose

The catalogue otherwise fills itself — a client asks for a work, this server
fetches it. **Import** searches the providers directly and pulls in the one you
meant, down the same path a client's request takes, so an imported work is
indistinguishable from one that arrived on its own.

### Who may call, and who has tried

Sonarr and Radarr have their metadata URLs compiled in and cannot attach an API
key, so those surfaces are guarded by address. That list lives in the database
and is edited under **Access** in the web UI, beside the API keys — it is the
same decision said a different way — and a change applies from the next request
without a restart. `AMS_ALLOWLIST` seeds it once, the first time the server
starts against an empty table, and is ignored afterwards.

The same screen shows **who has been calling**: every address that has reached a
guarded route, refused ones included. A refused client leaves no other trace,
and its address is the one thing needed to let it in — so each row carries the
name the hosts file or the system resolver gives that address (inside a compose
network, the container's name), what the caller called itself (`Sonarr/4.0.20`),
how many times it has called, how many times it was turned away, and a button
that allows it. Some TMDB clients
compile their key in too — Jellyseerr does — and need the same treatment. Set
`AMS_ALLOWLIST` to the networks your stack runs on.

`AMS_AUTH_DISABLED=true` opens every surface. It exists for closed networks and
first-run setup; do not use it on anything reachable from outside. The same goes
for `AMS_NATIVE_AUTH=open`, which grants administrator rather than reader — key
issuance, the allowlist and the settings included.

### What is closed, and how

A few things worth knowing before this is reachable from anywhere:

- **Nothing a caller presents here is relayed upstream.** The TMDB relay strips
  the session cookie and every spelling of an API key before forwarding, so a
  credential for this server never reaches TMDB's access logs.
- **The relay reads and does not write.** It exists for Jellyseerr, Overseerr and
  Plex, which only read; a forwarded `POST` would let any key issued here rate a
  film, or empty a list, using the operator's TMDB credentials.
- **Of TMDB's v4 API, only the public lists are relayed** (`/4/list/{id}`), and
  only with a v4 read token configured: Radarr's TMDb list imports read them
  there. An account's own lists, ratings and watchlist need that account's
  token, which the relay carries for nobody.
- **The settings, the job history and the audit trail need an administrator**,
  not merely a credential. A key issued to Jellyseerr cannot read which surface
  takes which credential or whether authentication is off.
- **The session cookie** is `HttpOnly`, `SameSite=Strict`, and `Secure` when this
  server terminates TLS or `AMS_PUBLIC_URL` says `https`.
- **Every response carries a content security policy** with `frame-ancestors
  'none'`, so no page elsewhere can frame this one and borrow an administrator's
  clicks, and `Referrer-Policy: no-referrer`, so the ids of what you are looking
  at do not travel to the artwork hosts.
- **Every surface is rate limited**, the arr and TMDB ones included
  (`AMS_RATE_LIMIT_PER_MINUTE`, 600 by default, per address).
- **An image URL may not name an address only this server can reach** — loopback,
  link-local, `169.254.169.254`. The NFO export downloads stored artwork, which
  makes an image URL a request this server makes on somebody else's say-so.
  A private LAN address is still allowed: a picture mirror at home is a
  reasonable thing to own.
- **`AMS_CORS_ORIGINS` names origins.** `*` is ignored with a warning rather than
  honoured — this server sends credentials, and the two cannot be combined.
- **`AMS_ALLOWED_HOSTS` names the hostnames this server answers to**, and is the
  only defence against DNS rebinding — a name whose TTL is one second, pointing
  first at an attacker and then at this server, makes a victim's own browser
  treat the attacker's page as same-origin with it, and CORS never gets a say.
  Empty by default, because there is no safe guess: this is reached by container
  name, by LAN address, by whatever the router calls it. Worth setting on
  anything using `AMS_NATIVE_AUTH=allowlist`, where a browser calling from an
  allowed address is all the credential an attacker needs.

## Running it in containers

```bash
docker compose up -d                                        # SQLite
docker compose -f compose.yaml -f compose.postgres.yaml up -d   # PostgreSQL
```

The image is distroless: 67 MB, no shell, non-root, and it runs with a read-only
root filesystem and every capability dropped. It answers its own health check.

To put a whole stack behind it — Sonarr, Radarr and Jellyseerr all served from
here — see [`docs/integration.md`](docs/integration.md) and the runnable
[`compose.integration.yaml`](compose.integration.yaml).

## Development

```bash
cargo test                      # 175 tests; the repository ones use an in-memory SQLite
cargo clippy --all-targets
cargo fmt --check

cd frontend && npm ci && npm run dev   # UI on :5173, proxying to :8080
```

`scripts/smoke.sh` starts the server against a database URL and walks the paths
that matter — sign-in, a manual entry, the Sonarr surface, locking, key
authentication — then stops it:

```bash
cargo build
./scripts/smoke.sh 'sqlite://data/smoke.db?mode=rwc'
./scripts/smoke.sh 'postgres://ams:ams@127.0.0.1:5432/ams'
```

CI runs both, plus the container image.

## Moving between databases

Changing `AMS_DATABASE_URL` points the server at a different database; it does
not carry your catalogue across. To move it:

```bash
arr-metadata-server transfer 'sqlite://data/ams.db' 'postgres://ams:secret@localhost/ams'
```

The target is migrated first and must be empty, unless you pass `--force` to add
to what is already there.

## Licence

MIT
