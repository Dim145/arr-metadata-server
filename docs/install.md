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
to what is already there.

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
