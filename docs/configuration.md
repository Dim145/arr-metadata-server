# Configuration

The server is configured in two places: its environment, and its settings. This page covers
the environment, every variable it reads, and where the two meet; every setting is in the
[settings reference](settings.md).

## What is a setting, and what is not

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

A setting added by a later version is given its first value the same way, at the first start
of that version: from its variable when the environment sets one, otherwise from the code. The
[settings reference](settings.md) says which variable gives which setting its first value.

Three variables overrule a setting for as long as they are set: `AMS_PUBLIC_BROWSE=false` keeps
the site private whatever **Opening & APIs** says, `AMS_FORCE_PASSWORD_LOGIN=true` keeps sign-in
by password on, and `AMS_OIDC_CLIENT_SECRET` wins over the client secret stored with the
identity provider's settings.

## The environment

Every option is an environment variable prefixed `AMS_`. See
[`.env.example`](../.env.example) for the annotated list, whose notes are in French. Variable
names from the two predecessor projects (`TMDB_API_KEY`, `BIND_ADDRESS`, `SKYHOOK_BASE_URL`, …)
are still accepted, so an existing `.env` keeps working; where one is, the table below gives
it in brackets after the `AMS_` name.

A variable set to nothing counts as unset. A value the server cannot use — a number out of its
range, a flag that is neither `true` nor `false`, a policy it does not know — stops the start,
naming the variable. Flags take `true`, `false`, `1`, `0`, `yes`, `no`, `on` and `off`.
`AMS_MODE=multi` is checked as a whole: see [Several instances](instances.md).

## Environment variables

> This list is written by hand from what the server reads — `src/config.rs`, and
> `src/telemetry.rs` and `src/outbound.rs` for the three variables they read themselves. A test
> checks that every variable named on this page is one the server reads; it does not check the
> defaults. The container image sets `AMS_DATABASE_URL`, `AMS_TLS_DIR` and `AMS_MEDIA_DIR` to
> places under `/data`, given beside the defaults below.

### The server and its doors

| Variable | Default | What it does |
| --- | --- | --- |
| `AMS_MODE` | `single` | `single`, one instance; `multi`, several as one: see [Several instances](instances.md). |
| `AMS_INSTANCE_NAME` | the host's name | What an instance is called among the others. |
| `AMS_BIND_ADDRESS` (`BIND_ADDRESS`, `LISTEN_ADDR`) | `0.0.0.0:8080` | The interface's door: the interface, the native API, and the surfaces the clients call, as an address and a port. |
| `AMS_PUBLIC_URL` | unset | The address the interface is reached at from outside, such as `https://metadata.example.com`: where the media kept are addressed for the clients, where a sign-in and an invitation lead back to, and an origin a change may be asked from. See [Behind a reverse proxy](reverse-proxy.md). |
| `AMS_TLS_CERT`, `AMS_TLS_KEY` | unset | A certificate and its key, in PEM files: the interface's door then speaks TLS itself. Both or neither. |
| `AMS_TRUSTED_PROXIES` | unset | The proxies whose forwarded headers are believed, as addresses or CIDR blocks separated by commas. See [Behind a reverse proxy](reverse-proxy.md). |
| `AMS_ALLOWED_HOSTS` | unset, any name | The host names the interface's door answers to, separated by commas; another is answered `421`, but for `/health`, `/ready`, `/ca.crt` and `/trust-ca.sh`. |
| `AMS_CORS_ORIGINS` | unset, none | Origins whose pages may call the native API from a browser, separated by commas. `*` is ignored, with a warning. |
| `AMS_REQUEST_TIMEOUT` | `60` | Seconds a request may take before it is answered `504`; 1 to 86400. |
| `AMS_MAX_CONNECTIONS` | `1024` | Connections each door holds at once; past it, a new one waits to be accepted. |
| `AMS_HEADER_READ_TIMEOUT` | `30` | Seconds a connection has to send a request's head, and may sit idle between two requests; 1 to 600. |
| `AMS_LOG` (`RUST_LOG`) | `info` | What is logged, as a `tracing` filter: `debug`, `info,arr_metadata_server=debug`. |
| `AMS_LOG_FORMAT` | unset | `json` writes one JSON object per line, for a log shipper. |

### The clients' door

| Variable | Default | What it does |
| --- | --- | --- |
| `AMS_CLIENTS_BIND` | unset, closed | The second door, for the names Sonarr, Radarr and the TMDB, TheTVDB and AniList clients have compiled in: `0.0.0.0:443`. Always in TLS. See [Pointing your stack at this server](integration.md). |
| `AMS_CLIENTS_NAMES` | unset | More names that door answers to, and that its certificate carries, separated by commas: `ams.lan,192.168.1.7`. |
| `AMS_CLIENTS_TLS_CERT`, `AMS_CLIENTS_TLS_KEY` | unset | Your own certificate and key for that door, instead of the authority the server makes. Both or neither. |
| `AMS_TLS_DIR` | `data/tls` (`/data/tls` in the image) | Where the authority and the certificate it issues are kept. |
| `AMS_TLS_REPLACE_AUTHORITY` | unset | The SHA-256 fingerprint of the authority to replace, once, with one made for every name. |

### The database

| Variable | Default | What it does |
| --- | --- | --- |
| `AMS_DATABASE_URL` (`DATABASE_URL`) | `sqlite://data/ams.db?mode=rwc` (`sqlite:///data/ams.db?mode=rwc` in the image) | The database: `sqlite://…` or `postgres://user:password@host:5432/name`. |
| `AMS_DATABASE_MAX_CONNECTIONS` | `10` | Connections kept open to it; 1 to 10000. |
| `AMS_DATABASE_ACQUIRE_TIMEOUT` | `30` | Seconds a request waits for one of them; 1 to 3600. |

### Accounts and access

| Variable | Default | What it does |
| --- | --- | --- |
| `AMS_ADMIN_USERNAME`, `AMS_ADMIN_PASSWORD` | unset | The first administrator, and a way back in: when no active administrator is left, a start makes this account one again, with this password. Both or neither; the password is twelve characters at least. |
| `AMS_NATIVE_AUTH` | `apikey` | What the native API asks for: `apikey`, `allowlist` or `open`. `open` grants administrator. |
| `AMS_TMDB_AUTH` | `apikey` | The same for the TMDB relay. |
| `AMS_TVDB_AUTH` | `apikey` | The same for the TheTVDB relay. |
| `AMS_ANILIST_AUTH` | `allowlist` | The same for the AniList relay. |
| `AMS_ARR_AUTH` | `allowlist` | The same for Sonarr's and Radarr's surfaces. |
| `AMS_PLEX_AUTH` | `allowlist` | The same for the Plex provider; `apikey` takes a key in its address. See [Plex](plex.md). |
| `AMS_AUTH_DISABLED` | `false` | Opens every surface. For closed networks and a first setup only. |
| `AMS_ALLOWLIST` (`AMS_ARR_ALLOWLIST`) | loopback and the private ranges | The first value of the list of allowed addresses, separated by commas; read against an empty list only. The default is `127.0.0.0/8`, `::1/128`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` and `fc00::/7`. |
| `AMS_PUBLIC_BROWSE` | unset | `true` gives `site.access` the first value `public`; `false` keeps the site private whatever the setting says. |
| `AMS_RATE_LIMIT_PER_MINUTE` | `600` | Requests a minute, per address — an IPv6 address by its /64. |
| `AMS_SIGNIN_FAILURES_PER_ACCOUNT` | `10` | Failed sign-ins an account takes in fifteen minutes before it is refused unchecked until the window closes. |
| `AMS_SIGNUPS_PER_HOUR` | `5` | Sign-ups without an invitation, per address and per hour. |
| `AMS_SIGNUPS_PER_HOUR_TOTAL` | `100` | The same, for the whole server. |
| `AMS_OIDC_CLIENT_SECRET` | unset | The identity provider's client secret, which wins over the one stored in the settings. |
| `AMS_FORCE_PASSWORD_LOGIN` | `false` | Keeps sign-in by password on, whatever the settings say. |
| `AMS_AUDIT_RETENTION_DAYS` | `90` | How long the audit trail is kept, in days; `0` keeps everything. |

### The providers

| Variable | Default | What it does |
| --- | --- | --- |
| `AMS_TMDB_API_KEY` (`TMDB_API_KEY`) | unset | TMDB's v3 key, or its v4 read token (`eyJ…`). Without one, TMDB is not asked — a search goes on to TheTVDB and Skyhook — and the TMDB relay, when it asks with this server's key, answers `503`. |
| `AMS_TVDB_API_KEY` (`TVDB_API_KEY`) | unset | TheTVDB's key. Without one, TheTVDB is not asked, and the TheTVDB relay, when it asks with this server's key, answers `503`. |
| `AMS_TVDB_PIN` | unset | The PIN of a subscriber key; a project key sends none. |
| `AMS_TVDB_ENABLED` | `true` | TheTVDB as a source; also the first value of `tvdb.searchFallback`. |
| `AMS_FANART_API_KEY` (`FANARTTV_API_KEY`) | unset | Fanart.tv's key, for logos, clearart and banners. |
| `AMS_FANART_ENABLED` | `true` | Fanart.tv as a source, once it has a key. |
| `AMS_MAL_CLIENT_ID` (`MAL_CLIENT_ID`) | unset | MyAnimeList's client id: its own API is asked rather than Jikan. |
| `AMS_TMDB_PASSTHROUGH` | `true` | The TMDB relay, `/3/` and `/4/`: `false` answers `503` to all of it. |
| `AMS_TVDB_PASSTHROUGH` | `true` | The TheTVDB relay, `/v4/`: `false` answers `503` to all of it. |
| `AMS_ANILIST_PASSTHROUGH` | `true` | The AniList relay, `graphql.anilist.co`: `false` answers `503` to all of it. |

These give a setting its first value, and are not read again once it has one — see the
[settings reference](settings.md): `AMS_TMDB_LANGUAGE` (`TMDB_LANGUAGE`, `en-US`),
`AMS_TMDB_INCLUDE_ADULT` (`TMDB_INCLUDE_ADULT`, `false`), `AMS_TMDB_SEARCH_LIMIT`
(`TMDB_SEARCH_LIMIT`, `10`), `AMS_SKYHOOK_FALLBACK` and `AMS_SKYHOOK_ENRICH` (`true`),
`AMS_RADARR_METADATA_FALLBACK` and `AMS_RADARR_METADATA_ENRICH` (`true`),
`AMS_SONARR_SCENE_MAPPINGS` (`false`), `AMS_SONARR_SCENE_MAPPING_SEARCH` (`true`),
`AMS_TVMAZE_ENABLED`, `AMS_ANILIST_ENABLED`, `AMS_MAL_ENABLED`, `AMS_IMDB_ENABLED`,
`AMS_FANKAI_ENABLED` and `AMS_FANKAI_WIKI_ENABLED` (`false`), `AMS_PROVIDER_PRIORITY`,
`AMS_REFRESH_ENABLED` (`true`), `AMS_REFRESH_INTERVAL` (`900`), `AMS_REFRESH_BATCH_SIZE` (`25`),
`AMS_REFRESH_CONTINUING_TTL` (`21600`) and `AMS_REFRESH_ENDED_TTL` (`604800`).

### Where the providers are asked

Each provider is asked at its own address unless one is given. An address given may be
anywhere — a mirror on the same machine included; where it redirects is judged as any other
address the server fetches. See
[If you redirect at the resolver](integration.md#if-you-redirect-at-the-resolver).

| Variable | Default |
| --- | --- |
| `AMS_TMDB_UPSTREAM` (`TMDB_UPSTREAM`) | `https://api.themoviedb.org` |
| `AMS_TVDB_UPSTREAM` | `https://api4.thetvdb.com/v4` |
| `AMS_SKYHOOK_UPSTREAM` (`SKYHOOK_BASE_URL`) | `https://skyhook.sonarr.tv` |
| `AMS_SONARR_SERVICES_UPSTREAM` | `https://services.sonarr.tv` |
| `AMS_THEXEM_UPSTREAM` | `https://thexem.info` |
| `AMS_RADARR_METADATA_UPSTREAM` | `https://api.radarr.video` |
| `AMS_FANART_UPSTREAM` | `https://webservice.fanart.tv/v3` |
| `AMS_TVMAZE_UPSTREAM` | `https://api.tvmaze.com` |
| `AMS_ANILIST_UPSTREAM` | `https://graphql.anilist.co` |
| `AMS_MAL_UPSTREAM` | `https://api.myanimelist.net/v2` |
| `AMS_JIKAN_UPSTREAM` | `https://api.jikan.moe/v4` |
| `AMS_TENRAI_UPSTREAM` | `https://api.tenrai.org/v1` |
| `AMS_IMDB_DATASETS` | `https://datasets.imdbws.com` |
| `AMS_FANKAI_UPSTREAM` | `https://metadata.fankai.fr` |
| `AMS_FANKAI_WIKI_UPSTREAM` | `https://fan-kai.fandom.com/fr/api.php` |
| `AMS_ANIME_MAPPING_URL` | `https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-full.json` |

### The cache

| Variable | Default | What it does |
| --- | --- | --- |
| `AMS_CACHE_MAX_ENTRIES` | `10000` | The memory tier's size, in entries of 16 KiB: the works' space holds this many, the searches' half as many. |
| `AMS_CACHE_ITEM_TTL` (`REDIS_TTL`) | `3600` | Seconds a merged work is kept. |
| `AMS_CACHE_SEARCH_TTL` (`REDIS_SEARCH_TTL`) | `1800` | Seconds a search is kept. |
| `AMS_CACHE_SESSION_TTL` | `30` | Seconds a session is remembered before the database is asked again. |
| `AMS_REDIS_URL` (`REDIS_URL`) | unset | A Valkey or Redis server for the second tier: `redis://[user:password@]host:port[/db]`, or `rediss://` over TLS. |
| `AMS_REDIS_PREFIX` | `ams:` | What every key written there begins with, and the one thing the server deletes there. |
| `AMS_REDIS_TIMEOUT_MS` | `150` | Milliseconds one command to it may take; 1 to 60000. |
| `AMS_PUBLIC_CACHE_SECONDS` | `60` | How long a browser or a proxy may keep a page anyone may read; `0` keeps the interface's `no-cache`. |

See [Caching](caching.md).

### The media and the exports

| Variable | Default | What it does |
| --- | --- | --- |
| `AMS_MEDIA_STORAGE` | `off` | Where the media are kept: `off`, `filesystem` or `s3`. See [Keeping the media](media.md). |
| `AMS_MEDIA_DIR` | `data/media` (`/data/media` in the image) | The directory, for `filesystem`. |
| `AMS_S3_ENDPOINT` | unset, Amazon's | A server that speaks S3 — Garage, MinIO, RustFS, Ceph — as `http://…` or `https://…`. |
| `AMS_S3_REGION` | `us-east-1` | The region the bucket signs for. |
| `AMS_S3_BUCKET`, `AMS_S3_ACCESS_KEY`, `AMS_S3_SECRET_KEY` | unset | The bucket and its credentials; required with `s3`. |
| `AMS_S3_PATH_STYLE` | `true` with an endpoint, `false` without | Addresses the bucket in the path rather than as a subdomain. |
| `AMS_S3_PREFIX` | unset | A directory within the bucket. |
| `AMS_MEDIA_PRIVATE_NETWORKS` | `true` | `false` keeps the media store and the `.nfo` export out of private networks too. |
| `AMS_NFO_EXPORT_PATH` | unset | Where `POST /api/v1/export/nfo` writes; unset, that export is off. See [Exports](exports.md). |
| `AMS_NFO_EXPORT_ARTWORK` | `true` | Downloads the artwork beside the documents. |
