# arr-metadata-server

A self-hosted metadata server for your media stack. It speaks the protocols
Sonarr, Radarr and TMDB clients already use, keeps its own canonical copy of
everything it serves, and lets you correct or invent entries by hand — with
manual edits permanently protected from automatic refreshes.

**Documentation: <https://dim145.github.io/arr-metadata-server/>**, built from
[`docs/`](docs/) and this README.

## What it serves

| Who asks | What it is served | Where |
| --- | --- | --- |
| Sonarr v4 and v5 | Skyhook, in `skyhook.sonarr.tv`'s place, and Sonarr's alternate titles, in `services.sonarr.tv`'s | the clients' door |
| Radarr | its metadata API, in `api.radarr.video`'s place | the clients' door |
| Jellyseerr, Overseerr and other TMDB clients | the TMDB API, relayed from `api.themoviedb.org` with the corrections made here written in | the clients' door |
| Yamtrack, Jellyfin's TheTVDB plugin, Kodi's scraper, the anime trackers | TheTVDB's and AniList's APIs, relayed the same way | the clients' door |
| Plex Media Server 1.43 or later | a custom metadata provider, `/plex` | the interface's door |
| Kometa, Kodi, Jellyfin, calendar apps, feed readers | Kometa's metadata file, lists, iCalendar and Atom feeds; `.nfo` documents with their artwork, written to a directory | the interface's door |
| You | the catalogue to browse, its administration, and the API behind both (`/api/v1`) | the interface's door |

The **interface's door** is `AMS_BIND_ADDRESS` (`0.0.0.0:8080`). The **clients' door**,
`AMS_CLIENTS_BIND`, answers on 443 in TLS under the names the clients have compiled in, with a
certificate from an authority the server makes; resolving those names to the server is what
points a client at it. Behind both is one catalogue, filled from TMDB, TheTVDB, Sonarr's and
Radarr's own services and Fanart.tv, and — when switched on — TVmaze, AniList, MyAnimeList,
IMDb and Fankai.

## Quick start

From a checkout of this repository, with Docker Compose:

```bash
cp .env.example .env
# set AMS_TMDB_API_KEY, and AMS_ADMIN_USERNAME / AMS_ADMIN_PASSWORD (twelve characters at least)
docker compose up -d
```

The interface is then on <http://localhost:8080>; sign in with the administrator the
environment names. The image is `ghcr.io/dim145/arr-metadata-server`, for amd64 and arm64.
PostgreSQL instead of SQLite, building from source and checking the image are in
[Installing and running](docs/install.md).

## Pointing Sonarr, Radarr and Plex at it

Sonarr and Radarr cannot be given another metadata address: it is compiled in. So the names
they call are made to lead here, as
[Pointing your stack at this server](docs/integration.md) walks through in full:

1. Open the clients' door: `AMS_CLIENTS_BIND=0.0.0.0:443`, with the server at a fixed address
   on the stack's network.
2. Resolve `skyhook.sonarr.tv` (and `services.sonarr.tv`, for the alternate titles) in Sonarr,
   and `api.radarr.video` in Radarr, to that address — `extra_hosts` in Compose, or your
   resolver.
3. Make them trust the server's authority: `/ca.crt`, and
   [`docker/trust-ca.sh`](docker/trust-ca.sh) for a linuxserver.io container.
4. Allow their addresses on **Keys & network** in the administration; they send no key.
5. For Plex Media Server, add a metadata provider at `http://<this server>:8080/plex` under
   *Settings › Metadata Agents*, and allow Plex's address: see [Plex](docs/plex.md).

From then on a correction made here reaches them at their next refresh, or at once with
[Telling them of a change](docs/sonarr-radarr.md#telling-them-of-a-change) on.

## Documentation

On the site, or in [`docs/`](docs/):

- [What it does](docs/features.md) — everything it serves and keeps, in one list.
- [Installing and running](docs/install.md) — Compose, PostgreSQL, from source, verifying the
  image, moving between databases, development.
- [Configuration](docs/configuration.md) — what is a setting and what is an environment
  variable, and every variable with its default.
- [Pointing your stack at this server](docs/integration.md) — Sonarr, Radarr and the TMDB,
  TheTVDB and AniList clients: the clients' door, trusting its authority, allowing the clients
  through.
- [Sonarr and Radarr](docs/sonarr-radarr.md) — what Sonarr v4, Sonarr v5 and Radarr are given,
  and connecting your own Sonarr and Radarr: telling them of a change, filling the catalogue
  from their libraries.
- [Plex](docs/plex.md) — Plex Media Server taking its metadata from this server, as a custom
  metadata provider.
- [The relays](docs/relays.md) — the TMDB, TheTVDB and AniList relays: whose key TMDB and
  TheTVDB are asked with, what is written into the answers and what is taken out of them.
- [Where the data comes from](docs/sources.md) — the sources, how they are merged, whose
  episode numbering wins, and the further sources.
- [Languages](docs/languages.md) — what each client is answered in.
- [Notifications](docs/notifications.md) — telling what happens to Standard Webhooks
  receivers, Discord, Slack, ntfy, Gotify and Apprise: tries, log and replay.
- [Exports](docs/exports.md) — Kometa's metadata file, selections as text files, lists and
  feeds, calendars, and `.nfo` documents with their artwork.
- [Editing the catalogue](docs/editing.md) — locks per language, hiding what the sources gave,
  the catalogue's register and its actions on many works, the order of the sources, and the
  dashboard's list of what there is to do.
- [Data](docs/data.md) — release dates and ratings by country, the schedule, and what a work
  carries besides: tagline, writers, Wikidata, AniList's format.
- [The site and the administration](docs/interface.md) — the bar along the foot of a phone, the
  catalogue as a list, the Settings page and the Jobs page.
- [Keeping the media](docs/media.md) — pictures and themes fetched once and kept, on disk or in
  a bucket.
- [Accounts and access](docs/access.md) — what each surface asks for, browsing without an
  account, sign-ups, the identity provider, adult titles, the list of allowed addresses.
- [What is closed, and how](docs/security.md) — what to know before the server is reachable
  from anywhere, and the audit trail.
- [Behind a reverse proxy](docs/reverse-proxy.md) — nginx, Traefik and Caddy in front of the
  interface, `AMS_PUBLIC_URL` and `AMS_TRUSTED_PROXIES`, and what to keep off the internet.
- [Caching, and a cache server](docs/caching.md) and [Several instances](docs/instances.md) —
  Valkey or Redis behind the memory, and several instances run as one.
- [The API](docs/api.md) — where it is documented, and how a key is presented.
- [Settings reference](docs/settings.md) — every setting, what it holds and what it starts as,
  written from the code.

## Verifying the image

An image published from a tag is signed, carries its build provenance and an SBOM, and lists
the crates inside its binary. The commands that check each are in
[Installing and running](docs/install.md#verifying-the-image).

## Development

```bash
cargo test                      # the repository tests use an in-memory SQLite
cargo clippy --all-targets
cargo fmt --check

cd frontend && npm ci && npm run dev   # UI on :5173, proxying to :8080
```

`scripts/smoke.sh` starts the server against a database URL and walks the paths
that matter, then stops it:

```bash
cargo build
./scripts/smoke.sh 'sqlite://data/smoke.db?mode=rwc'
./scripts/smoke.sh 'postgres://ams:ams@127.0.0.1:5432/ams'
```

[`CONTRIBUTING.md`](CONTRIBUTING.md) says the rest.

## Licence and credits

MIT — see [`LICENSE`](LICENSE).

It grew out of two earlier, private tools — a TMDB relay and a stand-in for
Sonarr's metadata service — and does what both did, from one catalogue.

The metadata comes from [TMDB](https://www.themoviedb.org), [TheTVDB](https://thetvdb.com),
Sonarr's and Radarr's own metadata services, [Fanart.tv](https://fanart.tv), and, when they are
switched on, [TVmaze](https://www.tvmaze.com), [AniList](https://anilist.co),
[MyAnimeList](https://myanimelist.net) through its API or through
[Tenrai](https://api.tenrai.org/documentation), a mirror written to Jikan's schema (a Jikan of
your own last, the public one having closed), IMDb's datasets, [Fankai](https://fankai.fr)
and its community's wiki; the [Fribb anime-lists](https://github.com/Fribb/anime-lists) map
anime between them. Each provider's terms apply to what is fetched and kept, and every public
page credits the sources that are on. This product uses the TMDB API but is not endorsed or
certified by TMDB.
