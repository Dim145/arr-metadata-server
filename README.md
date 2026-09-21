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
  field is locked and every later refresh leaves it alone.
- **Automatic refresh.** Any entry carrying at least one external id is
  refreshed on a schedule that adapts to its status.
- **Client management.** Named API keys, scopes, expiry, per-surface policy, and
  an escape hatch to turn authentication off entirely.
- **Per-language answers.** Sonarr asks in its URL, the native API takes
  `?language=`. Translations are fetched the first time a language is asked for
  and kept — from TMDB, then TheTVDB for the many languages TMDB does not carry.
  A locked field stays locked in every language.
- **`.nfo` export with the artwork beside it**, in the layout Kodi defined and
  Plex's Personal Media agent reads. It is the only route to Plex.
- **Two interfaces, in English or French.** A catalogue anyone can browse —
  posters, seasons, cast, the record — and an administration side for the
  people who maintain it. Both switch language from the bar and remember the
  choice.

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

Enrichment is on by default and costs one extra call per refresh per provider.
Turn it off with `AMS_SKYHOOK_ENRICH=false`, `AMS_RADARR_METADATA_ENRICH=false`,
`AMS_TVDB_ENABLED=false` or `AMS_FANART_ENABLED=false`.

> If you redirect `skyhook.sonarr.tv` or `api.radarr.video` at this server
> through your **resolver** rather than per container, it resolves those names
> to itself and would call itself forever. It detects that and answers `508`
> with the fix in the message — but point the upstreams elsewhere, or turn
> enrichment off, when that is your setup.

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
| TMDB-compatible | `/3/*` | API key (`api_key` query parameter) |
| Sonarr / Radarr compatible | `/v1/*` | IP allowlist |

### Letting anyone browse

`AMS_PUBLIC_BROWSE=true` opens the catalogue to a reader with no credential:
the list of works, one work, and the totals. Nothing else. It is an allowlist
rather than a denylist, so the settings, the job history, the audit trail, the
raw provider payloads and the record of who edited what all stay behind a
credential — and an endpoint added later is closed until somebody decides
otherwise. Administration is never reachable this way.

It is off by default, because turning it on publishes what this server knows to
whoever can reach the port.

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
first-run setup; do not use it on anything reachable from outside.

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
