# Changelog

Every release of arr-metadata-server, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
[Semantic Versioning](https://semver.org/): while the major is 0, a minor
may change what the API or the configuration means, and says so here.

## [Unreleased]

### Changed

- A series near an air date is fetched again sooner. One that has not
  started — upcoming, or with no episodes yet — every two hours in the week
  of its premiere or of its next episode, and every hour in the day of it,
  after the premiere as before it; any series still running, no later than
  an hour after its next episode airs, at the instant a provider gave or
  else at midnight UTC of its day. Every other work keeps its interval.
  *Magical Explorer*'s episodes reached TheTVDB and TMDB hours after it
  premiered, and six hours from its last refresh kept them from Sonarr for
  longer still. A refresh that fails near an air date is tried again on the
  same cadence rather than six hours later. A series takes it from its next
  refresh.
- Sonarr's request for a series with no episodes yet, or one that premieres
  within two days either side, is answered with a copy fetched there and
  then when the one held is more than an hour old. Sonarr asks again only
  every few hours, and kept the copy from before the episodes were listed.

### Fixed

- A series with an episode Skyhook has no name for yet is read whole.
  Skyhook leaves `title` out for such an episode, and the whole answer was
  refused for it, so a new series lost what only Skyhook gives: *Magical
  Explorer*'s first episode reached Sonarr at midnight UTC of its Japanese
  day, nine hours after it aired, rather than at the instant Skyhook knows.
  Whatever else Skyhook leaves out, or a mirror sends as `null`, is read as
  nothing, and an entry that cannot be read is left out rather than the
  series. Sonarr is still sent `TBA` for the episode. A series shows it from
  its next refresh; Sonarr takes it at its own next refresh of the series.
- A series due a refresh that no provider answers for is served to Sonarr
  as it is held, rather than as a 404. The attempt counts as a failed
  refresh, so the requests that follow are answered at once instead of each
  waiting on the providers again.
- A series TheTVDB tells apart from a homonym reaches Sonarr under the
  title Skyhook gives it — *Rurouni Kenshin (2023)*, *The Office (US)* —
  where TMDB's name, which the title usually is, carries no such mark. A
  work TheTVDB has no entry for is given its year instead when another
  series here goes by the same title. Two series Sonarr knows by one title
  make its lookup by title fail, and the releases by that name were dropped.
  A locked title still goes as it was locked. A new column holds what
  TheTVDB adds, filled in as each series is next refreshed, and kept through
  a refresh neither TheTVDB nor Skyhook answers or a sync that asks only
  other sources; Sonarr takes the title at its own next refresh of the
  series.
- A special reaches Sonarr with its own name, synopsis and still where TMDB
  numbers specials differently from TheTVDB. TMDB's were taken by number:
  *Rurouni Kenshin*'s first special, a 1997 film on TheTVDB, carried the
  still and TMDB id of TMDB's first, the series' last episode, and, on a
  server set to another language than Sonarr's, that episode's name.
  Another provider's special now fills one of TheTVDB's only when both date
  it the same day: the only special either has that day, or one of several
  that both sides number alike. One without a date fills nothing, and the
  regular seasons are matched by number as before. An episode TMDB has no
  text for in the language asked is now asked of TheTVDB even when
  everything TMDB sent was complete: that last episode, which TheTVDB counts
  in season 3, reached an English Sonarr in French. A series shows it from
  its next refresh; Sonarr takes it at its own next refresh of the series.
- An episode TMDB has no name for reaches Sonarr as `TBA`, as Skyhook sends
  it. TMDB calls such an episode by its number in the language asked —
  `Épisode 3`, `Folge 3`, `第3話` — and that went to Sonarr as its title:
  Sonarr named files after it, and its check that an episode is named
  before it is imported let it through. *Reincarnated as a Sword*'s second
  season reached a Sonarr from a server set to French as `Épisode 3` to
  `Épisode 12`. TMDB's stand-in, in any of the forms its languages give it
  and with the episode's own number, is now no name, in the catalogue as in
  the text fetched for another language; another provider's title takes its
  place where there is one. A series shows it from its next refresh; Sonarr
  takes it at its own next refresh of the series.
- Sonarr's `mal:` and `anilist:` lookups, which its MyAnimeList and AniList
  import lists search by, find a series only TMDB lists while it is due a
  refresh. The lookup went through the series' TheTVDB id, which such a
  series has none of, and the anime identifier list seldom files a new one:
  the series was not found, and the import list passed it over. It is now
  looked up by the id Sonarr keeps it under, fetched again, and served as
  held when no provider answers.
- A refresh asked for by hand that no provider answers is recorded as such.
  When the providers came back with nothing, the series was looked up as
  Sonarr looks it up, and the copy held — current, or kept for want of an
  answer — was taken for a fresh one: the run read "refreshed from a
  provider" with every provider out of reach.
- A work switched off in the catalogue is served to Sonarr and Radarr from
  the store, as it is held, and no provider is asked for it on their
  requests. It was taken for one the store did not hold: each request for
  it fetched it again from every provider and was answered with the copy
  written all the same — or with a 404 when no provider answered, on which
  Sonarr takes a series for deleted, as a Fan-Kai switched off while the
  Fankai source is off always was. The sweep still passes such a work by,
  and a refresh asked for by hand still fetches it.
- A film due a refresh that no provider answers for is served to Radarr as
  it is held, as a series is to Sonarr. Radarr was answered with a 404 by
  its TMDB id, with nothing by its IMDb id — or an error, when TMDB could
  not be reached — and its bulk request left the film out. The attempt
  counts as a failed refresh, so the requests that follow are answered at
  once instead of each waiting on the providers again.
- Sonarr's request for a series switched off in the catalogue, in a
  language its episodes' text was never fetched in, asks no provider
  either: it is served what is held in that language. That text was still
  fetched from TMDB and TheTVDB, once for each new language asked for.
- The dialog that disables a work says what that does now: the work is
  hidden from the site, the catalogue and the native API, and Sonarr and
  Radarr keep getting the copy held here, which is no longer refreshed on
  its own. It said the work stopped being served to every client, as the
  administration's catalogue said of the works it lists.
- A series' episode text in a language survives a request in that language
  that no provider answers. A series asked for in a language for the first
  time since its last refresh has its episodes' text in it fetched again,
  and the text held was deleted before the answer was written: with TMDB
  and TheTVDB out of reach nothing was written back, the language was
  marked fetched all the same, and Sonarr was given the episodes in the
  server's own language until the series' next refresh. An episode's text
  is now replaced only by what a provider gives for it, and the language's
  text as a whole only by an answer from every provider asked. A fetch that
  comes to less — a season TMDB did not answer for, TheTVDB out of reach,
  or nothing at all, as for a language neither has — is tried again when a
  failed refresh would be, six hours later or sooner near an air date,
  rather than at the series' next refresh; the requests in between are
  served what is held without waiting on the providers. A new column holds
  when.

## [0.4.0] — 2026-09-29

### Added

- Sonarr's alternate titles, from this catalogue. Sonarr never reads the
  alternative titles of a Skyhook answer: it recognises releases only by a
  series' own title and the lists it downloads from `services.sonarr.tv` and
  TheXEM. With `services.sonarr.tv` resolved to this server and
  `sonarr.sceneMappings` on (`AMS_SONARR_SCENE_MAPPINGS`, off by default), the
  real list reaches Sonarr with a mapping added for each title of this
  catalogue in the language of the answers, in English or romanised from the
  work's own language, written in the Latin alphabet, that Sonarr does not
  know yet and that no other series answers to — so a release named in French
  or in romaji is recognised. Only the series' title in the language of the answers is also
  searched with (`sonarr.sceneMappingSearch`); the others serve to recognise
  releases. Everything else Sonarr asks of that host is relayed as it is, and
  when the real list cannot be had Sonarr keeps the one it holds.
- `AMS_TLS_REPLACE_AUTHORITY`: set to the authority's fingerprint, it replaces
  the authority once with one made for every name, the old one kept aside, in
  the files as in the database. It is how a server set up before
  `services.sonarr.tv` covers it; every client then trusts the new authority.
  See "Upgrading a server that is already running" in `docs/integration.md`.

### Fixed

- An episode nobody has named yet reaches Sonarr as `TBA`, as Skyhook sends
  it. An empty title made Sonarr list the episode as a row with nothing in it
  to click; Sonarr rewrites the titles it holds at its next refresh.
- A series whose first episode is still to come reaches Sonarr as upcoming,
  as on Skyhook, where TMDB's "In Production" had it continuing. A work shows
  it from its next refresh.

## [0.3.2] — 2026-09-29

### Fixed

- Radarr's interactive search failed for a film once its metadata came from
  this server ("Object reference not set to an instance of an object"). A
  translation with a synopsis but no title of its own was sent without one,
  which Radarr's release matching does not survive. Every translation now
  carries a title: its own, or else the film's original title, as Radarr's
  own metadata service does. Radarr rewrites the translations it holds when
  it next refreshes the film.
- A film whose original language is not known is sent as undetermined
  (`und`) rather than with none, which Radarr does not survive either: it
  could not refresh the film, and a search that found it failed whole.
- A series' first and last air dates, and an episode's air date, reach
  Sonarr as plain days. A date field here also takes a date-time, and Sonarr
  reads these three strictly as year-month-day: a date-time failed the
  series' refresh, and a search that found it failed whole; on an episode,
  it broke the search of a daily series. A date-time is sent as the day it
  names in its own zone.

## [0.3.1] — 2026-09-28

### Fixed

- The adult flag said it was kept from an earlier answer, whatever the
  sources said: it was left out of the record of where each value comes
  from. It is traced again, so the editor names the source that says a work
  is adult; a flag left off is the default and names none. A work shows its
  source from its next refresh.
- The slug no longer claims to be kept from a source: it is made here, from
  the title and the year.

## [0.3.0] — 2026-09-28

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
- The same for TVmaze: a work's own TVmaze identifier is enough to ask it,
  where TheTVDB's was required.
- A page's preview picture follows the poster Sonarr is given: the chosen
  one, else one added by hand, else the best the sources offered.

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

[Unreleased]: https://github.com/Dim145/arr-metadata-server/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Dim145/arr-metadata-server/releases/tag/v0.1.0
