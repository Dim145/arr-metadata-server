# arr-metadata-server

A self-hosted metadata server for your media stack. It speaks the protocols
Sonarr, Radarr and TMDB clients already use, keeps its own canonical copy of
everything it serves, and lets you correct or invent entries by hand — with
manual edits permanently protected from automatic refreshes.

It replaces and merges two earlier projects: `the earlier TMDB relay` and
`the earlier Skyhook stand-in`.

> **Status: working, not yet battle-tested.** Every surface is implemented and
> exercised by tests on both database engines, and the container runs read-only
> as a non-root user. What has *not* happened yet is a live TMDB call or a run
> against a real Sonarr/Radarr — see `docs/integration.md` for how to try it.

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

## Authentication

Authentication is configured **per API surface**, because the clients differ in
what they can send:

| Surface | Path | Default policy |
|---|---|---|
| Native API and web UI | `/api/v1/*` | API key (header or query) |
| TMDB-compatible | `/3/*` | API key (`api_key` query parameter) |
| Sonarr / Radarr compatible | `/v1/*` | IP allowlist |

Sonarr and Radarr have their metadata URLs compiled in and cannot attach an API
key to a request, so those surfaces are guarded by network policy instead. Set
`AMS_ARR_ALLOWLIST` to the networks your stack runs on.

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
cargo test                      # 114 unit tests, no database required
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

## Licence

MIT
