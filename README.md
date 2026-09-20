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
  and kept. A locked field stays locked in every language.
- **`.nfo` export**, which is the only route to Plex.

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

## Languages

Entries are stored in `AMS_TMDB_LANGUAGE`. Any other language is fetched the
first time a client asks for it and kept from then on:

```bash
curl 'http://localhost:8080/v1/tvdb/shows/fr/81189'      # Sonarr's own form
curl 'http://localhost:8080/api/v1/items/{id}?language=ja'
```

A translation never overwrites a field someone locked — that would undo a rename
the moment a client asked in another language. Lock the field again per language
if you want different text in each.

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
| TMDB-compatible | `/3/*` | API key (`api_key` query parameter) |
| Sonarr / Radarr compatible | `/v1/*` | IP allowlist |

Sonarr and Radarr have their metadata URLs compiled in and cannot attach an API
key, so those surfaces are guarded by network policy instead. Some TMDB clients
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
