# Installing and running

The server is one binary that serves the interface, its own API and the surfaces its clients
call, with SQLite or PostgreSQL behind it. It is published as a container image; it can also
be built from source.

## Requirements

- Rust 1.94+ (only to build from source; a container image is provided)
- SQLite (bundled, nothing to install) or PostgreSQL 14+
- A TMDB API key, if you want it to fetch anything

## With Docker Compose

```bash
cp .env.example .env
# set AMS_TMDB_API_KEY, and AMS_ADMIN_USERNAME / AMS_ADMIN_PASSWORD
docker compose up -d                                            # SQLite
docker compose -f compose.yaml -f compose.postgres.yaml up -d   # PostgreSQL
```

[`compose.yaml`](../compose.yaml) runs one container, with its database, its certificates and,
when it keeps them on disk, the media, in one volume mounted at `/data`. The interface is then on
<http://localhost:8080> (`AMS_PORT` publishes it on another port): sign in with
`AMS_ADMIN_USERNAME` and `AMS_ADMIN_PASSWORD`, which open the first administrator. The password
must be twelve characters at least, or the server does not start; with neither variable set,
nobody can sign in to the interface. [`compose.postgres.yaml`](../compose.postgres.yaml) adds a
PostgreSQL and points the server at it, and wants `POSTGRES_PASSWORD` in `.env`.

The image is published at `ghcr.io/dim145/arr-metadata-server`, for amd64 and
arm64, when a version is tagged: `v1.2.3` is `1.2.3`, `1.2` and `latest`
(`.github/workflows/docker.yml`). The compose files pull it; `docker compose
build` builds it from the checkout instead.

The image is distroless: 67 MB, no shell, non-root, and it runs with a read-only
root filesystem and every capability dropped. It answers its own health check.
It runs as uid 65532: a named volume takes that ownership when it is made, and a
directory mounted from the host has to be given it (`chown 65532:65532 ./data`).

To put a whole stack behind it — Sonarr, Radarr and Jellyseerr all served from
here — see [Pointing your stack at this server](integration.md) and the runnable
[`compose.integration.yaml`](../compose.integration.yaml). To run several instances as
one, see [Several instances](instances.md); to publish the interface, see
[Behind a reverse proxy](reverse-proxy.md).

## From source

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

## Verifying the image

An image published from a tag carries what is needed to check it, so that none
of it has to be taken on trust:

- **A signature** by digest, made without a key by the publishing workflow
  (cosign, with the identity GitHub gives that run). One made anywhere else does
  not verify.
- **Where it was built**: GitHub's attestation of the build provenance, which
  ties the digest to the workflow run and the commit.
- **What it is made of**: an SBOM and BuildKit's provenance for each platform,
  kept in the registry beside the image. In the image's index they are the
  `unknown/unknown` entries next to each platform: attestations, not a missing
  platform.
- **The crates inside the binary**: it lists its own dependencies
  (`cargo auditable`), so a scanner finds them in a container that has no
  package database for Rust.

```bash
# Signed by this repository's publish workflow, from a tag, and by nothing else:
cosign verify ghcr.io/dim145/arr-metadata-server:<version> \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github\.com/Dim145/arr-metadata-server/\.github/workflows/docker\.yml@refs/tags/v'

# Built where it says, from the commit it says:
gh attestation verify oci://ghcr.io/dim145/arr-metadata-server:<version> --repo Dim145/arr-metadata-server

# What it is made of, as the registry holds it, per platform:
docker buildx imagetools inspect ghcr.io/dim145/arr-metadata-server:<version> --format '{{ json .SBOM }}'

# The crates inside the binary, against the advisory database:
id=$(docker create ghcr.io/dim145/arr-metadata-server:<version>)
docker cp "$id":/usr/local/bin/arr-metadata-server ./arr-metadata-server && docker rm "$id"
cargo audit bin ./arr-metadata-server
```

`<version>` is a tag such as `1.2.3`. Pinning the digest in a compose file
(`image: ghcr.io/dim145/arr-metadata-server@sha256:…`) is what keeps a check
true: a tag can be moved to another image, a digest cannot.

## Moving between databases

Changing `AMS_DATABASE_URL` points the server at a different database; it does
not carry your catalogue across. To move it:

```bash
arr-metadata-server transfer 'sqlite://data/ams.db' 'postgres://ams:secret@localhost/ams'
```

The target is migrated first and must be empty, unless you pass `--force` to add
to what is already there. An archive does the same: one made on SQLite restores on
PostgreSQL, and the other way round (see [Backups](#backups)).

## Backups

The server makes archives of itself: its database — every work, lock, account, key, setting,
destination and run — as one file that restores on SQLite and on PostgreSQL alike, and, when
asked, the media it keeps on disk. **Administration › Backups** lists them, makes one now,
downloads, deletes and restores them; [the command line](cli.md) does the same from a shell.

- **When.** Weekly, counted from the newest archive whoever made it, the schedule's four newest
  kept: `backup.schedule`, `backup.keep` and `backup.includeMedia` say otherwise (see
  [Settings](settings.md#backups)). Those made by hand, by the command line or before a restore
  stay until somebody deletes them; those made before an upgrade keep to a count of their own,
  below. The dashboard says so when the newest is older than twice
  the schedule, `/api/v1/admin/health` carries `lastBackupAt`, a scrape
  `ams_backup_age_seconds` ([Watching the server](monitoring/README.md)), and the webhooks are
  told `backup.completed` or `backup.failed`.
- **Where.** In `AMS_BACKUP_DIR`: `backups` beside the SQLite database by default, which is
  `/data/backups` in the image — on the volume the database is on, so copy that directory
  somewhere else as well: an archive on the disk it saves goes with it. Among several instances
  whose media are in a bucket, in that bucket, under `backups/`
  ([Several instances](instances.md)). An archive is put together beside the others and named
  only once whole, so one cut short by a full disk is never listed. One bound for a bucket is
  put together in `AMS_BACKUP_DIR` all the same: that is where the room for a copy of the
  database is wanted, not in the system's temporary directory, often a small tmpfs in a
  container.
- **Before an upgrade.** A start that finds migrations to apply first makes an archive of the
  database as the version before left it, without the media:
  `ams-20261019-020000-upgrade-m0037.tar.zst`, made before the migrations that bring it to 0037,
  listed as made *before an upgrade*. The two newest are kept (`backup.keepUpgrades`), counted
  apart: they never take the place of the schedule's. An SQLite file always is, a copy of the
  file in seconds; a PostgreSQL unless `backup.beforeUpgrade` is turned off — its rows are copied
  one by one through SQLite, which a large catalogue takes a while over, and an administrator
  who runs `pg_dump` before an upgrade may prefer that. An archive that cannot be made — a full
  disk, a directory the server may not write to — stops the start before any migration, and the
  log says so: give it room, or start once with `AMS_BACKUP_BEFORE_UPGRADE=false`. The way back
  to the version before is that archive, restored by that version into an empty database — a
  version older than the archives copies the archive's `database.sqlite` there with `transfer`
  instead. On SQLite, that file is itself the database, which can take the place of `ams.db`
  while the server is stopped, `ams.db-wal` and `ams.db-shm` removed with it.
- **What.** `ams-20261019-020000-scheduled.tar.zst`: a tar compressed with zstd that holds
  `manifest.json` — the archive's layout, the version and the last migration of the server that
  made it, when, from which engine, why, each file's size and SHA-256 — then
  `database.sqlite`, the whole database as one SQLite file whatever its engine, then, when
  asked, `media/<key>`, each medium under the SHA-256 of its bytes. Media kept in a bucket are
  not in it: back the bucket up as a bucket is. Nothing of the server is needed to read one:

  ```bash
  zstd -d < ams-20261019-020000-scheduled.tar.zst | tar -x
  sqlite3 database.sqlite 'SELECT COUNT(*) FROM media_item'
  ```

- **Encrypted** when `AMS_BACKUP_PASSPHRASE` is set: the whole archive, as
  [age](https://age-encryption.org) writes it with a passphrase, named `….tar.zst.age`, and
  opened without the server by `age -d ams-….tar.zst.age | zstd -d | tar -x`. This is what the
  passphrase is for: an archive holds every account's password hash, every key's hash, the
  webhooks' signing secrets and tokens, the identity provider's settings and, among several
  instances, the private key of the authority the clients trust — what lets somebody into this
  server and the services it calls. Keep the passphrase apart from the archives: nothing opens
  one without it.

**Restoring** puts the archive's database in the place of the one there. From the Backups page,
the server first makes an archive of the database as it stands — restoring that one undoes the
restore — and then answers every request with 503 until it is done: the interface, the API,
Sonarr's and Radarr's surfaces, the relays, the clients' door. `/health` alone still answers, so
that a container's health check does not restart it halfway. The rows are copied in one
transaction, so a restore that fails leaves the database as it was. Everybody is signed out but
those whose session the archive holds, and the settings, the network rules, the media's index and
the webhooks are read again; the clients' certificate, at the next start. An archive made by a
later version, or with a later migration, is refused before anything is touched; an older one is
brought up to this version's migrations first. Restored where the media are in a bucket, it says
how many media the database names that the bucket lacks — served from their providers until
fetched again.

From [the command line](cli.md#backups), the server stopped — a restore is refused while a server
holds the database:

```bash
docker compose stop metadata
docker compose run --rm metadata restore ams-20261019-020000-scheduled.tar.zst --replace
docker compose start metadata
```

Into an empty database, on a new machine, `--replace` is not needed. A database a server started
on once is not empty — it holds its settings — and needs it; the archive made of it first is then
what undoes the restore.

## Development

```bash
cargo test                      # the repository tests use an in-memory SQLite
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

CI runs both, plus the container image. [`CONTRIBUTING.md`](../CONTRIBUTING.md) says the rest:
the checks to run before a pull request, and the end-to-end suite.

This documentation is built from `docs/` and the README by the site under `site/`:
`npm ci && npm run build` there, and `docs/settings.md` is written again by
`UPDATE_DOCS=1 cargo test settings_reference` when a setting changes.
