# arr-metadata-server

A self-hosted metadata server for your media stack. It speaks the protocols
Sonarr, Radarr and TMDB clients already use, keeps its own canonical copy of
everything it serves, and lets you correct or invent entries by hand — with
manual edits permanently protected from automatic refreshes.

It grew out of two earlier, private tools — a TMDB relay and a stand-in for
Sonarr's metadata service — and does what both did, from one catalogue.

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
- **Related works, and who people are.** What AniList files beside an anime
  — its sequels, prequels, side stories, what it was drawn from — is kept
  with the work and shown on its page, leading to the work here when the
  catalogue holds it and to AniList when it does not; the seasonal chart says
  what a series is the sequel of. A person's page adds who they are as
  TMDB has it: born when and where, known for what, a biography in the
  reader's language and a few portraits.
- **The other orders, and a lamp.** Where TheTVDB numbers a series another
  way — its DVDs, straight through, an alternate or a regional order — the
  season page offers that numbering too (`GET /api/v1/items/{id}/orders`),
  without touching what Sonarr is served. And the interface has a light
  theme: the reader's choice, kept in the browser, or the system's.
- **A scrape, and the locks as a file.** `GET /api/v1/admin/metrics` is the
  server counting itself in the text Prometheus reads — requests by surface
  and status, calls upstream by provider and outcome with their latency,
  what the caches keep, what the catalogue holds, what the jobs did — for an
  administrator or a key with the `admin` scope, which a scraper sends as
  `Authorization: Bearer`. Every edit made by hand can be downloaded as one
  JSON document and imported into another catalogue (`GET`/`POST
  /api/v1/admin/locks`), each lock finding its work of the same kind by the
  TMDB, TheTVDB or IMDb id it shares; a request takes two thousand locks at
  most, and the dashboard sends a longer file in parts.
- **Languages of the interface.** English and French ship complete; a
  third language is one file and a registry entry, may start partial — what
  it lacks is shown in English — and the build refuses a key English does
  not have. See `frontend/src/lib/lang/README.md`.
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

### Who gave what, and asking one source again

Every refresh records, field by field, which provider's value was kept, which
others said the same and which said otherwise; the pictures are counted by
source, and the episode list names whoever numbered it. The work editor shows
it beside each field: the provider's name, **Multi-sources** with the count
when several agree (Skyhook is not counted beside the TheTVDB it
republishes), **Locked** for an edit by hand, **Kept** for a value no source
gives any more, **No source** when nobody gave one. None of this reaches a
visitor. **Administration › Sources** lays out the rules themselves: who can
supply what, in the configured order, and who replaces the rest.

The editor's **Sources** panel asks chosen providers again, and only them. The
others are handed back what they gave last time, so the ordinary merge weighs
the fresh answers against them by the ordinary priority: a sync from TheTVDB
alone takes its new runtime without handing it a title TMDB outranks it on.
Locked fields do not move, the refresh schedule and the work's identifiers
stand, and the episode list stays whoever numbered it unless that provider
gives one anew. Some sources come along with the ones asked: TVmaze with the
provider that numbers the episodes, whose broadcast instants it corrects, and
that provider with any source that fills the episodes in, so the list is
merged as a refresh merges it. A sync that finds the work written by something
else meanwhile — a refresh, another sync — writes nothing and says so. A work
refreshed before this was recorded needs one full refresh first. The same through the API: `GET /api/v1/items/{id}/provenance`,
`POST /api/v1/items/{id}/sync` with `{"sources": ["tvdb", "fanart"]}`, and
`GET /api/v1/sources/rules`.

### Further sources

Six more, each off until switched on under **Settings → Further sources**.
None needs a key.

| Source | Brings | Costs |
| --- | --- | --- |
| **TVmaze** | the moment each episode aired | two calls per series fetch |
| **AniList** | its score, the romaji, native and English titles and synonyms, the main studio, an adult flag — for anime | one call per anime fetch |
| **MyAnimeList** | its score and titles — for anime; through its own API when `AMS_MAL_CLIENT_ID` is set, through [Jikan](https://jikan.moe) otherwise | one call per anime fetch |
| **IMDb** | IMDb's rating, for every work with an IMDb id | one download a day |
| **Fankai** | the Fan-Kai productions — anime recut into films, each as a series with its sagas as seasons — from Fankai's own metadata service | one call per saga, and three more per production fetch |
| **Wiki Fankai** | for each Fan-Kai, the anime it was cut from and the Fan-Kai that follows it — only while Fankai is on | one call per Fan-Kai fetch |

None of the first four changes the shape of an answer: Sonarr and Radarr are
served the same fields as before, some of them now more accurate. Fankai adds
works instead — ones no other source has.

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

**Fankai lists the Fan-Kai productions**: anime recut into films by the
[Fankai](https://fankai.fr) team, which neither TheTVDB nor TMDB carries. Each
production is a series, its sagas are its seasons, and its films are its
episodes, numbered as Fankai names its files — `Horimiya
Kaï.S01E02.MULTI.1080p.x265-FANKAI.mkv` is season 1, episode 2, and a second
saga carries on from the first: `S02E08` is the eighth film. A production turns
up in Sonarr's search under its own name — *Naruto Shippuden Yabai*, *Black
Lagoon Henshū* — or as `fankai:` and its id, and on the import page; nothing is
fetched until it is asked for, so Fankai's catalogue never lands here whole.
Sonarr is handed an id of this server's own, Fankai's plus 200 000 000, the way
a TMDB-only series gets one; Plex, Jellyfin and Kodi get the production through
the NFO export, with Fankai's id as a `uniqueid` and its theme music beside
it as `theme.mp3`, which a work's page also plays on demand. The kaïeur and the voice cast
are its credits; the kind of recut — Kaï, Yabai, Henshū — is a tag rather than
a genre. One call a second, and the listing is kept and revalidated by its ETag.

Fankai's id is its metadata service's. The website numbers its productions
otherwise — *Horimiya Kaï* is series 33 of the one and production 101 of the
other, where 33 is *Black Lagoon* — behind a sign-in, and nothing public says
which page is which. So the id is shown without a link, and a production's
homepage is its page on the Fankai wiki below, when that source is on.

**The Fankai wiki says what each Fan-Kai was cut from.** Fankai's service
leaves a production's ids blank; the community's wiki, at
[fan-kai.fandom.com](https://fan-kai.fandom.com/fr/), links each one's AniList
and MyAnimeList entries and names the Fan-Kai that follows it. With it on, a
Fan-Kai's page leads to its anime — to the anime's own page once Sonarr holds
it — and to the next Fan-Kai, and the anime's page leads back to every Fan-Kai
cut from it. The wiki keeps a page per cut, so a production is matched on its
name and on its kaïeur, never on a guess between two. With AniList on too, the
anime's title, year and poster come from there. The ids are never given to the
Fan-Kai itself: sharing one would merge it into the anime.

The lists and their last download are shown under the switches, with a button
to download one now rather than wait for its turn, and every download is a job. Their terms: TVmaze's data is CC BY-SA; IMDb's datasets are
for personal, non-commercial use; AniList's API is free for non-commercial use
and asks not to be crawled or stored wholesale — this server asks only about
works a client requested; Fankai publishes its metadata for its own productions,
and is asked about one at a time; the Fankai wiki's content is CC BY-SA. Every public page credits the sources that
are on, with the notices TMDB and IMDb require.

## Keeping the media

Every picture and theme a work points at can be fetched once and kept, so
the catalogue reads without its providers — and keeps reading when they are
gone. Set `AMS_MEDIA_STORAGE` to `filesystem` (files under `AMS_MEDIA_DIR`,
`data/media` by default) or `s3` (a bucket, Amazon's or anything that speaks
S3 — Garage, MinIO, RustFS, Ceph — with `AMS_S3_ENDPOINT`, `AMS_S3_REGION`,
`AMS_S3_BUCKET`, `AMS_S3_ACCESS_KEY` and `AMS_S3_SECRET_KEY`). Off by
default: nothing is kept until it is asked for.

- **What is kept.** The work's posters, backdrops, banners and logos, its
  seasons', its episodes' stills, its cast's photographs and a Fan-Kai's
  theme music. Never a trailer or a video. The cast's photographs and the
  themes are each a switch on **Administration › Media**, where the store's
  figures, the failures and the tasks are.
- **When.** As a work is stored — a refresh, a sync, an import — its media
  go in line, and a worker fetches them a few at a time; the work is served
  the moment its details are, and its pictures switch to the copies kept as
  they land. **Store everything** fetches what a catalogue held before the
  store was switched on. An address that fails is tried again a few hours
  later, five times; one that answers with something else is given up on at
  once. Both are listed on the page, to be tried again.
- **How they are served.** Every copy is addressed by the hash of its bytes
  under `/media/`, and served immutable — with a thumbnail beside each
  picture, so a page of cards weighs what it did. The rows keep the
  providers' addresses: a work is rewritten as it is read, never as it is
  written, and asking for a copy to be forgotten puts the provider's address
  back. Sonarr, Radarr and the NFO documents are given the copies' addresses
  when `AMS_PUBLIC_URL` is set, and the providers' when it is not, since
  only this server's own pages can follow a path. A bucket's media go
  through this server, or, with **Sent to the bucket's own address**, a
  reader is redirected to a presigned address good for an hour.
- **By hand.** From a work's page, a picture or a theme can be uploaded —
  read for what it is, never for what it is called; SVG is refused, since a
  document that can carry a script must not be served from here — and a
  copy taken away. An upload's file goes with it; a provider's copy is
  fetched again at the next refresh.
- **The sweep.** Daily, and on request: what no work points at any more —
  a work removed, a picture a provider dropped, an upload taken off — is
  forgotten and its file deleted, and a file no row names is removed once it
  has had its hour.
- **Moving the store.** The rows say which copies exist, not where: pointed
  at another bucket or directory, they name files that are not there.
  Either copy the files across first, or **Forget every copy** on the Media
  page and store everything again. A database transfer carries the rows,
  not the files.

The providers' terms still apply to what is kept: TMDB, TheTVDB and
Fanart.tv each say what may be done with their pictures.

## Caching, and a cache server

Everything the server answers often is kept under its hand: a merged work,
a search, a page of the catalogue, a relayed list, what the TMDB relay
answered Jellyseerr, the session behind a cookie. The first tier is the
process's own memory, sized in bytes and expired per kind
(`AMS_CACHE_MAX_ENTRIES`, `AMS_CACHE_ITEM_TTL`, `AMS_CACHE_SEARCH_TTL`,
`AMS_CACHE_SESSION_TTL`). Behind it, when `AMS_REDIS_URL` names one, a
**Valkey or Redis** server holds a second tier: shared between instances,
kept from one start to the next, and told by each instance what it forgot,
so an edit on one is seen by all. Every key it writes carries
`AMS_REDIS_PREFIX` (`ams:`), and that prefix is the one thing it deletes: a
server may be shared. Nothing durable lives there — sessions, keys, settings
and the audit trail are in the database — so it can be emptied at any time;
a server that is down at start is attached when it answers, and one that
stops answering is waited for while the memory carries on alone.

A page anyone may read, read by nobody in particular, carries
`Cache-Control: public, max-age=60, stale-while-revalidate=300` and a `Vary`
on the credentials, so a browser or a proxy in front — Caddy, nginx, a
CDN — absorbs the anonymous traffic without touching the server
(`AMS_PUBLIC_CACHE_SECONDS`; 0 keeps the interface's `no-cache`).

The administration's **Cache** page reads it all back: what each space
holds in each tier and how often it answered, the server's memory,
eviction policy and round trip, a switch and a *Flush* per space, and a
flush of everything under the prefix — each in the audit trail. The same
figures are scraped from `/api/v1/admin/metrics`; `docs/monitoring/` holds
a Prometheus job and a Grafana dashboard to import.

Run the server yourself with `maxmemory` and `maxmemory-policy allkeys-lru`,
persistence off, and an ACL user confined to this server's keys and its
channel — it reads, writes, walks and unlinks under the prefix, publishes
and subscribes on `ams:events`, pings and reads `INFO`:

```
ACL SETUSER ams on >secret ~ams:* &ams:events +@read +@write +@keyspace +@pubsub +@connection +info
```

and then:

```yaml
  cache:
    image: valkey/valkey:8-alpine
    command: valkey-server --maxmemory 256mb --maxmemory-policy allkeys-lru --save "" --appendonly no
```

and `AMS_REDIS_URL=redis://cache:6379` on the metadata server. The server
trusts what it reads there and what it hears on its channel: give it a
database of its own, or one shared only with instances of itself. A
session revoked on one instance is forgotten on the others at once through
the channel, or within `AMS_CACHE_SESSION_TTL` when the server is away;
relayed lists of several megabytes stay in memory only. Title searches
read an index rather than every row since migration 0022: FTS5 trigrams on
SQLite, `pg_trgm` on PostgreSQL where the extension can be created.

`scripts/load/serve.sh` starts a release build over a copy of a catalogue,
and `scripts/load/baseline.sh` measures the requests that matter with
[oha](https://github.com/hatoo/oha); `docs/perf/` keeps the figures.

## Several instances

One instance is the default, and stays as simple as it is: `AMS_MODE=single`
— SQLite or PostgreSQL, the media on disk or in a bucket, every job
scheduled by the process itself. `AMS_MODE=multi` runs several instances
as one, behind whatever balances the interface's port between them:

* **PostgreSQL** is required (`AMS_DATABASE_URL=postgres://…`): several
  processes cannot share an SQLite file.
* **The media in a bucket** (`AMS_MEDIA_STORAGE=s3`), or kept nowhere
  (`off`): a directory is one instance's own.
* **A cache server** is required (`AMS_REDIS_URL`): it is the bus the
  instances coordinate on, besides being the second tier.

The server refuses to start in `multi` mode without the three, and says
which is missing. With them, the instances agree on a **leader** through a
lease on the cache server — renewed every five seconds, lost within
fifteen when the leader goes, taken by another within five more — and the leader
alone runs the schedules: the refresh sweep, the dataset imports, the
listing, the media sweep, the certificate's renewal. Every instance answers
requests, fetches the media in line (each medium claimed by one worker),
and can run a task by hand: a task is held on the cache server while it
runs, so two instances never run the same. What one instance changes, the
others are told through the channel: a setting, a network rule, a session
revoked, a medium kept or forgotten, a run to stop, a certificate issued.
The counters on *Opening & APIs* add up what every instance answered, and
the rate limits — requests per minute, sign-ups per hour — are one quota
across them all, counted on the cache server.

Two things move into the database in `multi` mode, so an instance started
afresh serves what the others do: the key the identity provider's cookies
are sealed with, and — for the clients' door — the authority the clients
trust and the certificate it issued. An authority already kept in
`AMS_TLS_DIR` is taken into the database the first time, so the clients
need not trust a new one; back the database up accordingly, since it holds
the authority's key from then on. Each instance's clients' door serves
that one certificate, so a client may reach any of them; the operator's
own certificate (`AMS_CLIENTS_TLS_CERT` / `_KEY`) works as before, from
files every instance has.

`AMS_INSTANCE_NAME` names an instance among the others — set it: the
hostname is the fallback, and a container's hostname is its id unless
`hostname:` says otherwise, so two instances or two lives of one would not
be told apart. The administration's **Cache** page lists them with who
leads, and the dashboard says how many there are. The metrics carry
`ams_leader` and `ams_instances`. The lease, the holds and the windows
live on the cache server beside the cache: with `allkeys-lru` they are
what is touched most and evicted last, but a server that runs out of
memory is a server that may forget who leads — size it so that it never
does, or give the instances a server of their own for coordination.
`compose.multi.yaml` is a complete stack — PostgreSQL, Valkey, a MinIO
bucket, two instances, Caddy in front — and `scripts/e2e-multi.sh` starts
two instances against a PostgreSQL of its own and checks that they agree,
pass on what changes, count together and hand the lead over.

What it does not do: a `multi` deployment scales the requests, not the
database or the bucket, which become the ceiling; with the cache server
away, the caches keep to memory as before, but no instance leads and
nothing scheduled runs until it answers — a request is never held up by
it. A task is held on the cache server for a minute at a time, renewed
while it runs: a cache server restarted or evicting keys can lose the hold,
and another instance asked to start the same task in that minute would run
it a second time — wasteful, never wrong.

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
| Public browsing | a fixed subset of `/api/v1/*` | **off**, chosen on *Opening & APIs* |
| TMDB-compatible | `/3/*`, and the public lists of `/4/list/*` | API key (`api_key` query parameter) |
| Sonarr / Radarr compatible | `/v1/*`, including the IMDb lists Radarr imports from | IP allowlist |

### Letting anyone browse

Whether the catalogue can be read without signing in is chosen on the
administration's **Opening & APIs** page (the `site.access` setting). *Public*
opens it to a reader with no credential:
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

*Private* sends every page to the sign-in page, turns link previews off, and
leaves the feeds and the import lists to whoever holds a key.

It is private by default, because a public catalogue publishes what this server
knows to whoever can reach the port. `AMS_PUBLIC_BROWSE` gives the setting its
first value on a new deployment and is not read for anything else — except that
`AMS_PUBLIC_BROWSE=false` keeps the site private whatever the page says, so that
closing the catalogue from the environment cannot be undone by a value stored
before. Remove the variable to let the page decide.

### Accounts, sign-ups and API switches

Every account has a role — *member* (browses, and holds keys of their own),
*editor* (corrects the catalogue), *administrator* (settles everything) — and a
status: active, pending (waiting for approval), disabled. A key a person makes
acts with no more than their role grants, and stops with them. Members reach
what a visitor may, plus their own account page; the Sonarr and Radarr surfaces
are never theirs, and the TMDB relay only when *Opening & APIs* lets members use
it, since it spends the operator's TMDB quota.

Sign-ups are *closed* (an administrator opens accounts), *by invitation* (a code
made on the Members page, shown once, which also says the role), *with approval*
(anybody signs up and waits), or *open*. An open door only ever opens a member's
account, and no sign-up ever makes an administrator. Uninvited sign-ups are
limited to `AMS_SIGNUPS_PER_HOUR` (5) per address — an IPv6 address counts by
its /64 — and `AMS_SIGNUPS_PER_HOUR_TOTAL` (100) for the whole server.

The same page switches each API off — Sonarr's, Radarr's, the TMDB relay, and
the native API's keys (the interface's own session is never cut). A switched-off
API answers `503`, which Sonarr and Radarr read as "try again later".

`AMS_ADMIN_USERNAME` and `AMS_ADMIN_PASSWORD` open the first administrator. After
that they are a way back in: when no active administrator is left, a restart
makes the account they name an active administrator again, with that password.

### Signing in through an identity provider

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

The image is published at `ghcr.io/dim145/arr-metadata-server`, for amd64 and
arm64, when a version is tagged: `v1.2.3` is `1.2.3`, `1.2` and `latest`
(`.github/workflows/docker.yml`). The compose files pull it; `docker compose
build` builds it from the checkout instead.

The image is distroless: 67 MB, no shell, non-root, and it runs with a read-only
root filesystem and every capability dropped. It answers its own health check.

To put a whole stack behind it — Sonarr, Radarr and Jellyseerr all served from
here — see [`docs/integration.md`](docs/integration.md) and the runnable
[`compose.integration.yaml`](compose.integration.yaml).

## The clients' door

Sonarr, Radarr and the TMDB clients call `https://skyhook.sonarr.tv`,
`https://api.radarr.video` and `https://api.themoviedb.org` — compiled in, on
443, and nothing else. Resolve those names to this server and it answers them
on a **second door**, `AMS_CLIENTS_BIND=0.0.0.0:443`: those surfaces alone,
always in TLS, with a certificate the server issues itself from an authority it
makes on first start, constrained to those names, and renews before it runs
out. The interface's own door, `AMS_BIND_ADDRESS`, is untouched by it — plain,
in TLS from your files, or behind your reverse proxy, as you decide. Each
client trusts the authority once (`/ca.crt`, and `/trust-ca.sh` for a
linuxserver.io container); the administration's **Opening & APIs** page shows
the door, its certificate and the files. [`docs/integration.md`](docs/integration.md)
walks through it, and [`scripts/e2e-sonarr.sh`](scripts/e2e-sonarr.sh) proves
it against a real Sonarr.

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

MIT — see [`LICENSE`](LICENSE).
