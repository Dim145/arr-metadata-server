# Changelog

Every release of arr-metadata-server, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
[Semantic Versioning](https://semver.org/): while the major is 0, a minor
may change what the API or the configuration means, and says so here.

## [Unreleased]

### Added

- A poster and a background can be chosen as the ones a work leads with —
  the star beside each image in the editor's artwork; none is chosen by
  default. A choice is a lock: kept through every refresh, carried with the
  locks, written in the journal. Sonarr and Radarr are given it, this site
  shows it, and the TMDB relay names it to Jellyseerr when it is one of
  TMDB's own images. A chosen image its source stops listing is kept for the
  choice.

### Changed

- Sonarr and Radarr are given one image a kind, as Skyhook and Radarr's own
  service give them: both write every image of a kind to one file, so the
  last one they were sent was the one they showed. Without a choice, an
  image added by hand leads, then the best the sources offered.

### Fixed

- Marking a work adult by hand failed on PostgreSQL: the flag was written as
  a boolean where the column holds an integer.
- A work's own AniList or MyAnimeList identifier now reaches those sources:
  the sources panel offers them, and a sync or a refresh asks them — the
  entry locked by hand first, then the identifier list's, then the one the
  work goes by. They were reached only through TheTVDB for a series, TMDB
  for a film, so a series TheTVDB does not know could not be asked at all.

## [0.2.0] — 2026-09-28

### Added

- A work's identity can be set by hand and locked like any other field:
  whether it is adult, its address (slug), and the identifiers it goes by
  elsewhere — edited from the work's page, written to the row as well as
  locked so the lists, the addresses and the clients' lookups follow, kept
  through every refresh, and refused when another work already goes by the
  same address or identifier.

### Changed

- A manual entry has no sources: its page no longer offers a refresh, a
  sync from sources or TMDB's suggestions, and the server refuses to
  refresh or sync one — the schedules never took them.

## [0.1.1] — 2026-09-28

### Changed

- The image is built on Debian 13 (trixie) — the Node and Rust build
  stages and the distroless runtime alike — and HAProxy 3.2 fronts the
  clients' door in `compose.multi.yaml`.
- Every dependency is at its newest: the Rust crates within their ranges,
  the frontend's `react-router` 8.4 and `@tanstack/react-query` 5.103, the
  workflows' actions at their latest releases, pinned by commit.

## [0.1.0] — 2026-09-28

The first public release: everything the server does today, as it went
public. Its image is `ghcr.io/dim145/arr-metadata-server:0.1.0`, for amd64
and arm64.

### The server

- Answers Sonarr as Skyhook (`/v1/tvdb/*`), Radarr as its metadata service
  (`/v1/movie/*`, `/v1/search`, `/v1/list/*`) and TMDB clients such as
  Jellyseerr as TMDB itself (`/3/*`, `/4/list/*`), relaying with its own
  credentials and patching every answer with the local edits — and offers
  its own API (`/api/v1/*`), documented from the handlers themselves at
  `/api/docs`.
- Keeps a canonical copy of every series and film it serves, merged from
  several sources — TMDB, TheTVDB, Skyhook, Radarr's service, Fanart.tv,
  and optionally TVmaze, AniList, MyAnimeList, IMDb's datasets and the
  Fan-Kai productions — with a provider priority, a record of where every
  value came from, and a refresh on a schedule that never touches a value
  set by hand.
- Serves the catalogue in the language a client asks for, translations
  included; exports `.nfo` documents and their artwork; carries the manual
  edits as a file between catalogues; runs on SQLite or PostgreSQL, and
  moves between them with `transfer`.
- Keeps the media works point at — pictures, cast photographs, themes — on
  disk or in an S3 bucket, with thumbnails, so the catalogue reads without
  its providers.

### The interface

- A catalogue anyone allowed may browse: works, seasons, episodes and
  people each with a page, filters, a search as the title is typed, a
  calendar of what airs, feeds, curated lists served to Sonarr and Radarr,
  where a work can be watched, what goes with it, the other orders a series
  comes in; installable, and reachable from the keyboard.
- An administration: an editor that locks what is set by hand, a sources
  matrix, a settings page in scopes, the tasks and their history, the audit
  trail, the network rules and who knocked, the media kept, the cache, and
  a dashboard with the server's health and figures.
- In English and French.

### Accounts and access

- Accounts with roles — administrator, editor, member — with API keys of
  their own, sessions, invitations, sign-ups by invitation, approval or an
  open door, and sign-in through an OpenID Connect provider with a role
  mapping.
- Each API surface switched and guarded on its own: by key, by network
  rule, or open; a public site if the administrators say so; rate limits
  and sign-up limits; a journal of every change, with who made it.

### The clients' door

- A second listener, in TLS, on the names Sonarr, Radarr and the TMDB
  clients have compiled in, with a certificate the server issues itself
  from an authority it makes on first start — constrained to those names,
  renewed before it runs out, served for the clients to trust at `/ca.crt`
  with a script at `/trust-ca.sh` — so no reverse proxy is needed in front
  of them. The operator's own certificate works instead.

### Caching, and several instances

- Two tiers of cache — the process's memory and, when configured, a Valkey
  or Redis server shared between instances and kept across restarts — with
  a page to read them back, switch each space and flush; public pages carry
  `Cache-Control` for a proxy in front; a search index on both engines;
  Prometheus metrics with a Grafana dashboard to import.
- `AMS_MODE=multi` runs several instances as one over PostgreSQL, a bucket
  and the cache server: a leader chosen by lease runs the schedules, the
  media are fetched by whichever instance is idle, what one instance
  changes the others are told, the counters and the limits are shared, and
  the clients' authority lives in the database so every door serves one
  certificate. `AMS_MODE=single`, the default, is as simple as it was.

### Running it

- A distroless container image, non-root, read-only, with its own health
  check; compose files for SQLite, PostgreSQL, a whole stack with Sonarr,
  Radarr and Jellyseerr redirected to it, and several instances behind
  Caddy and a TCP door.
- End-to-end checks against a real Sonarr, against two instances, and a
  Playwright suite over the whole interface.

[Unreleased]: https://github.com/Dim145/arr-metadata-server/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Dim145/arr-metadata-server/releases/tag/v0.1.0
