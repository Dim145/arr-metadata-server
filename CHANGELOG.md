# Changelog

Every release of arr-metadata-server, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
[Semantic Versioning](https://semver.org/): while the major is 0, a minor
may change what the API or the configuration means, and says so here.

## [Unreleased]

### Added

- A documentation site, https://dim145.github.io/arr-metadata-server/, built
  with Starlight from `docs/` and the README and published on GitHub Pages by
  `.github/workflows/docs.yml`. Each page is written once and stays readable
  on GitHub: the build rewrites the links between pages and stops on one that
  leads nowhere.
- `docs/settings.md`: every setting — what it holds, its default, where it
  may be set, the variable that seeds it — written from the settings registry
  by `UPDATE_DOCS=1 cargo test settings_reference`; `cargo test` fails when
  the page and the code disagree. `docs/reverse-proxy.md`: nginx, Traefik and
  Caddy in front of the server — `AMS_PUBLIC_URL`, `AMS_TRUSTED_PROXIES`, the
  headers read, the paths to keep off the internet.

### Changed

- The OpenAPI document (`/api/openapi.json`) describes what the server sends.
  Ten schema names stood for two types or more — `Role`, `Status`, `Counts`,
  `Mode`, `Entry`, `Kind`, `Source`, and Radarr's `RatingResource`,
  `ImageResource` and `AlternativeTitleResource`, which carried Skyhook's
  shape — so twenty-five fields were documented with another type's shape:
  each type has its own name now (`AccountRole` and `PersonRole`,
  `AccountStatus`, `SkyhookRatingResource`…). Every operation has an id of its
  own, `<module>_<handler>` (`users_list`), where fifty-three shared sixteen;
  and the sixty-seven fields the native API always sends, `null` when empty,
  are listed as required. Nothing the server sends has changed; a client
  generated from the document gets the new names. `cargo test` fails when two
  types share a name, when an operation lacks its id or shares one, or when a
  field always sent is documented as optional; the interface's generated
  types are a `.ts` the compiler checks.
- The README is down to what the server is, what it serves, a quick start
  and the first steps for Sonarr, Radarr and Plex. The rest moved, unchanged
  in substance, to new pages under `docs/`: installing, configuration (every
  environment variable), sources, languages, media, caching, several
  instances, accounts and access, security, the API.
- The interface loads only the language it is read in: English and French
  are separate files, the reader's fetched before the first paint and the
  other only when chosen. The first download is a third lighter: 489 kB
  instead of 746 (148 kB compressed instead of 235).
- An API's access policy reads the same, in the same colour, on the
  dashboard, the configuration page and the access page; the public bar's
  language switch takes the administration's segmented look.
- Internal, with nothing to see from outside: the interface's types are
  generated from the server's own OpenAPI document (`frontend/openapi.json`,
  kept current by `cargo test`, `UPDATE_OPENAPI=1` rewriting it, and turned
  into types by `npm run types`), and CI lints the interface (ESLint with
  typescript-eslint, react-hooks and jsx-a11y), runs its unit tests (Vitest)
  and fails when the generated types fall behind. The works' repository, one
  file of some five thousand lines, is six, each with its tests; the native
  API's handlers guard with `Identity::require_admin` and a new
  `Identity::require_write` instead of ten private copies of either.

### Fixed

- A film's releases in a country listed those with no date first, and an
  undated rating could stand for the country instead of the dated cinema
  release's.
- A field locked to an object shows its JSON in the editor, not
  `[object Object]`.

## [0.8.2] — 2026-10-18

### Added

- **Failed refreshes, grouped.** A notification destination can be told of
  failed refreshes together: they wait five minutes from the first, then go
  as one `refresh.failed` message — how many, counted by cause (the source
  and the kind of failure: `TheTVDB — timed out: 31`), the first works
  named — rather than one message per work; an outage of a source is one
  message. On by default for Discord, Slack, ntfy, Gotify and Apprise, and
  for the destinations of those kinds already made; off for `standard` and
  `json`, read by programs that want an event per work. Switched on the
  destination's form, `grouped` in the API; the message's JSON carries a
  `count`.

### Changed

- A supplement that does not answer — TVmaze, Fanart.tv, AniList,
  MyAnimeList, the Fankai wiki — no longer makes a work's refresh a failed
  one. What it gave before is kept, as it was, and the log says it did not
  answer; but the work is refreshed: no refresh error, not counted among the
  works whose refresh failed, and no `refresh.failed`. A hiccup of TVmaze
  flagged and told of every series refreshed meanwhile.

### Fixed

- TVmaze is given a minute in a background task — a refresh, a seed — and
  asked once more, after a pause, for what it never answered. Its servers
  take fifteen to twenty seconds, now and then, over what their cache does
  not hold, whatever the pace of the calls, and a refresh logged
  `TVmaze lookup failed … timed out` past the twenty it was given. A client
  waiting on this server still gives it twenty seconds, and no second try.
  TVmaze's API keys are not asked for: they open its user API, a premium
  member's own data, and change nothing of the public one's limit, which is
  per address.

## [0.8.1] — 2026-10-17

### Added

- **Plex is given a key in its address.** With `AMS_PLEX_AUTH=apikey`, the
  provider answers `/plex/<key>` and the paths under it, which is how Plex —
  sending no credential and keeping no query string — calls a provider: a
  Plex on another network, or behind an address that changes, is let in by
  the key rather than by a list of allowed addresses. The **Plex** panel of
  **Keys & network** makes a key for Plex, read only, and shows the address
  that carries it, once; revoking the key shuts Plex out. The key is checked
  as any other and carries its settings, language and country among them.
  Logs write the path as `/plex/***/…`. Asked by address, the provider
  refuses an address with a key in it (`403`, `plex_key_unused`) rather than
  answer as though the key mattered; `/plex` itself is unchanged. A Plex
  that already has the provider is given the new address through its API
  and uses it once restarted. `scripts/e2e-stack.sh` checks all of it
  against a real Plex Media Server (`plexkey`).

### Changed

- The web UI is type-checked by TypeScript 7, the native compiler, with the
  same diagnostics as 5.9. Dependabot no longer offers a new major of Node
  or of `@types/node`: the UI is built on Node's LTS line, moved by hand
  with ci.yml's `node-version`.

## [0.8.0] — 2026-10-17

### Added

- **Sonarr v5 is served.** It reads the second edition of the Skyhook API —
  `/v2/tvdb/shows/{language}/{id}/{seasonType}` and `/v2/tvdb/search` —
  answered from the catalogue beside the first, which Sonarr v4 keeps
  reading: the same series, with what v5 reads besides. Every language the
  series is named in (`translations`, its own among them, where Sonarr takes
  the original title from); the orders its episodes come in (`seasonTypes`:
  the aired order, then the DVD, absolute, alternate and regional orders
  TheTVDB keeps for it); a season's title and year — a title that only says
  the season's number is kept back, as the real service keeps it; and an
  episode's ids under both names the service and the client use. Asked in an
  order the series has, the episodes are numbered by it; in one it has not,
  as they aired, as Skyhook answers. The language is three letters, as Sonarr
  writes it: the series is answered in it when held so, else in the caller's
  `tmdb.language`, then in English, then as stored; a code that names no
  language is read as `eng`. Nothing changes in how `skyhook.sonarr.tv` is
  resolved. `scripts/e2e-skyhook-v2.sh` sets the real service's second
  edition beside this server's for a few real series and tells every
  difference apart as an addition or a failure.
- The schedule lists the films coming out beside the episodes, on each day
  one comes out — in cinemas, digitally, on disc; the kinds that fall on one
  day as one release — in the week with its poster and a chip a kind, in the
  month's cells after the day's episodes, on the front page's strip, and
  counted apart: "15 episodes · 2 releases". `GET /api/v1/calendar` answers
  both in `entries`, earliest first, each with its `kind`; `episodes` stays
  as it was. `calendar.ics` carries each release as an all-day event named
  in the language asked — *Interstellar — sortie en salles* — and
  `airing.atom` an entry for it.
- Radarr is sent each film's collection and the films recommended with it,
  as `api.radarr.video` sends them — `Collection` by id and name,
  `Recommendations` by id and title, twenty at most — on `/v1/movie/{id}`,
  `/imdb/{id}` and `/bulk`: read from what is held, never fetched for.
  Radarr's collections fill, and its Discover page has something to offer.
- MyAnimeList through [Tenrai](https://api.tenrai.org/documentation), a
  mirror written to Jikan's schema, when Jikan does not answer, and through
  Jikan when the official API does not: a way in that fails is passed over
  for five minutes, a 404 asks no other (`mal.fallback`, on;
  `AMS_TENRAI_UPSTREAM`). The Sources page says which way in serves.
- The webhook is told of three facts nobody did — `refresh.failed`, once
  when a work starts failing; `media.failed`, a picture or theme whose every
  try failed; `update.available` — its events are ticked rather than typed,
  and each destination can be tested from the Notifications page, which
  posts it `webhook.test`.
- A check for a newer version, off until turned on (`updates.check`): one
  anonymous GET to GitHub a day, said on the dashboard, by the version in the
  administration's bar, in the scrape (`ams_update_available`) and to the
  webhook once per version.
- A running series says on its plate what airs next — the episode, its day,
  how far off — and how far its season has got, in the season chart's words.
  The rows that scroll sideways — a work's seasons, a person's portraits, and
  on a phone the related, similar and front-page rows — fade at the edge
  where they go on, with two arrows to walk them.
- The published image says what it is and where it came from: an SBOM and
  the build's provenance beside each platform's image in the registry, a
  keyless signature by digest, GitHub's attestation of the build, and the
  binary lists its own dependencies for scanners and `cargo audit bin`. A
  security policy, a contributing guide, a code of conduct, issue forms and a
  pull request template; Dependabot proposes updates weekly, grouped.
- Plex can take its metadata from this server. Plex Media Server 1.43 and
  later accept a custom metadata provider, and this server is one, at
  `/plex` on the interface's door: add `{your address}/plex` in Plex under
  Settings, Metadata Agents, make an agent of it, and choose it in a
  library's advanced settings. Films and series are served, seasons and
  episodes included: what is locked here is what Plex shows, in the language
  Plex names (`X-Plex-Language`, else the caller's `tmdb.language`, else
  English), with the rating and release day of `X-Plex-Country`.
- A file Plex scans is matched by an id Plex gives — this provider's own, or
  an IMDb, TMDB or TheTVDB id, the old agents' `com.plexapp.agents.*` among
  them — then by title and year, among the works the catalogue holds and
  then through the search Sonarr's and Radarr's lookups use. A match by title
  fetches nothing, and one by id follows a few of the ids given, each once —
  for a caller who may not see works for adults, among the works the
  catalogue holds alone; the work Plex then asks for is fetched and kept. A
  series
  offers the orders the catalogue keeps for it (`SeasonType`) and is laid
  out in the one Plex asks for (`episodeOrder`); the pictures kept here are
  given under `AMS_PUBLIC_URL`, or the host Plex called.
- A Plex panel on the Keys & network page gives the address to paste, with a
  copy button, the switch that answers Plex (`api.plex`, on by default) and
  a reminder that Plex sends no credential: its server's address must be on
  the list of allowed addresses just below. `AMS_PLEX_AUTH` sets what the
  provider asks for, as `AMS_ARR_AUTH` does for Sonarr's and Radarr's routes
  (`allowlist` by default); the Opening & APIs page lists the provider, and
  `ams_http_requests_total` counts it under `surface="plex"`.
- `GET /api/v1/export/kometa.yml?kind=movie` (or `series`) writes what is
  locked here as a Kometa metadata file, to correct a Plex library: an entry
  for each work with a lock, matched by TMDB id for a film and TheTVDB id
  for a series, else by IMDb id, else by title and year, carrying only what
  was locked — titles, summary, tagline, first release, rating, studio,
  genres, the chosen poster and background, a series' seasons and episodes.
  `language=` asks for another language's locked texts. It needs an editor's
  or an administrator's key — one limited to reading will do; the settings
  page offers it as a download.
- A selection is served as a text file of identifiers for Kometa's "Text
  File" builder — `/api/v1/lists/{key}/ids.txt`, a work a line, `tmdb:949`
  for a film, `tvdb:348303` for a series, the title after a `#`, and
  `?kind=movie` or `?kind=series` for the works one Kometa library holds —
  and as an Atom feed (`feed.atom`), the latest added first, each work dated
  by the day it joined. Both are read as the selection's page is, a private
  selection being an editor's or an administrator's — through a key of
  theirs that only reads, too — and their addresses are on the selection's
  page and in its editor beside Sonarr's and Radarr's.
- A selection has a cover, chosen in the administration among the
  backgrounds of its works or by any picture's address, with the dialog
  every other picture is chosen with. It is shown above the selection on the
  selections' page, at the head of its own page, and as its preview where it
  is shared (`cover` on `PUT /api/v1/lists/{key}`: empty, it takes it away;
  left out, it stays).
- The `.nfo` export can be asked for one selection's works alone
  (`POST /api/v1/export/nfo?list=<slug or id>`, with a button in the
  selection's editor). A film that belongs to a TMDB collection carries
  `<set>`, as Kodi reads it, so that a library groups its films; a season
  with a title of its own is a `<namedseason>` in the series' document, and
  its poster a `<thumb>` of that season.
- Your own Sonarr and Radarr can be connected the other way, on a new Sonarr
  & Radarr page (`/admin/arr`, administrators): the address each answers at
  and its API key, which is written once, never shown again and kept out of
  the log. A test asks each who it is (`/api/v3/system/status`) and says its
  version, or why it did not answer. A private network and this very machine
  are reached; link-local and cloud-metadata addresses are refused, and a
  redirect to another origin is never followed, so the key goes nowhere else.
- With `arr.pushRefresh` on (off by default), a correction made here reaches
  Sonarr and Radarr at once: a lock put on a work, a season or an episode,
  changed or taken off, an edit of its seasons or episodes, a sync, or a
  refresh that changed what the work is served as asks Sonarr (for a series)
  or Radarr (for a film) to read it again, rather than at its own refresh
  twelve hours on. A burst of changes is one command, ten seconds after the
  last of them; a refresh that found everything as it was tells nobody. What
  was sent lately, and how it went, is listed on the page.
- A new task, *Seed from Sonarr and Radarr* (`arr.seed`, by hand on the
  Jobs page or on the new page), reads every series Sonarr holds and every
  film Radarr holds and imports what is missing the way the import page
  does, one work after another, gently. A work held already is passed over,
  so it can run again at any time and fetch only what came since, and it can
  be stopped; each work is written down in the run as imported, already held
  or not found.
- Renumbered episodes are told. When a refresh or a sync finds that TheTVDB
  moved episodes of a series — a season cut in two, absolute numbers
  shifted — the audit trail says so (`item.renumbered`), with how many moved
  and the first of them (`S01E05 → S02E01`), and a notification destination
  can be told of it. A numbering kept because TheTVDB did not answer moves
  nothing, and an episode added or gone is no move.
- The TMDB and TheTVDB relays can hand each client's own key on instead of
  the server's: *Whose key the relays ask with*, on Opening & APIs
  (`tmdb.relayCredential`, `tvdb.relayCredential`: `server`, the default, as
  the relays always did, or `client`). With `client` the server's key is
  never used; a key issued here is refused with a `401` before the service
  is asked; nothing asked with a client's key is kept for another; and where
  `AMS_TMDB_AUTH` or `AMS_TVDB_AUTH` asks for a key, the listed addresses
  guard the relay instead. The locks apply either way.
- The TMDB relay writes the locks into more of TMDB's documents: a season
  (`/3/tv/{id}/season/{n}`) — its name, overview, date and poster, and its
  episodes' titles, overviews, dates, runtimes and stills — one episode, and
  the results of `/3/search`, `/3/trending` and `/3/discover` — each
  result's title, overview, date and pictures, for every work of the page
  the catalogue holds with a lock. A regular episode is paired by its
  numbers, a special by the TMDB id the catalogue gave it, and neither when
  the catalogue knows it under another TMDB id.
- The TheTVDB relay writes the locks into more of its documents: the
  translations a record carries when they are asked for
  (`meta=translations`), with an entry added for the catalogue's language
  and for each language a text was locked in alone where TheTVDB has none,
  as Jellyfin's plugin reads them; a season's record (`/v4/seasons/{id}`,
  `/extended`) and the seasons a series lists; the episodes a record carries
  (`meta=episodes`); and a page of a series' episodes in any of its orders,
  which is what Seerr's TheTVDB option reads.
- Webhooks and other notifications get a page of their own, Notifications
  (`/admin/notifications`): any number of destinations, each with the events
  ticked for it, a test and a log. A destination is a Standard Webhooks
  receiver, Discord, Slack, ntfy, Gotify, Apprise or JSON, as before. Each
  is told what is ticked for it — the audit trail's actions and the three
  facts nobody does (`refresh.failed`, `media.failed`, `update.available`) —
  or every event with `*`; changes to destinations are in the audit trail
  too (`notification.created`, `.updated`, `.deleted`).
- A Standard Webhooks destination is posted `{type, timestamp, data}` with
  the `webhook-id`, `webhook-timestamp` and `webhook-signature` headers the
  specification defines: `v1,` and the base64 HMAC-SHA256 of
  `{id}.{timestamp}.{body}` under a `whsec_` secret the server makes, shown
  once when the destination is saved and made anew on request. The
  `webhook-id` is the event's, the same for every try and every replay, so a
  receiver can drop one it has already handled.
- An event is written down for each destination that wants it before
  anything is sent, and a worker of its own sends it: a slow or failing
  destination costs the action nothing, and a restart loses nothing owed.
  What is not taken is tried again after 5 seconds, 5 minutes, 30 minutes, 2
  hours, then 5, 10 and 10 hours — longer when the address asks so with
  `Retry-After` — and given up on after the eighth try; `410 Gone` switches
  the destination off, and the page says why. A destination with a thousand
  deliveries waiting is owed no more until it catches up.
- Each destination has a log: every delivery with its event, what came of
  it — the status answered, or why the address could not be reached, and how
  long it took — its tries and its next one. A delivered or given-up one can
  be replayed, under the same `webhook-id`. The log is kept thirty days, the
  newest thousand of each destination.
- A title or an overview — of a work, a season or an episode — and a work's
  tagline can be locked for the readers of one language alone. A work served in a
  language is given what was locked in that language, else what was locked
  in every language, else the translation, else the text it is stored in: by
  the native API, Sonarr, Radarr and the `.nfo`, and by the relays as
  *Changed* below says.
  `PUT /api/v1/items/{id}/overrides` takes `language` (`fr`, `fr-FR` and
  `fra` are one); in the editor, a field shown in a language through *Show
  in* asks whether an edit is for this language alone or for every one.
- A credit, a picture or an alternative title a source gave can be hidden
  rather than removed, and a credit's character and place corrected. Each is
  a lock naming what it holds by what it is — a credit by the person's TMDB
  id (or name) and role, a picture by its address, a title by its text and
  language — so it holds through every refresh, and a hidden one is in no
  answer: Sonarr, Radarr, the `.nfo`, the site, the lists of people,
  pictures and other titles the TMDB and TheTVDB relays hand on. The People
  & titles and
  Artwork tabs hide, show and correct each row in place; a locks document
  holding such a lock is version 2, and version 1 still imports.
- The administration's catalogue says what each record lacks. Every row
  leads with its poster and carries four marks — a poster, an overview, an
  IMDb id, a first refresh — said in words to the pointer and a screen
  reader, struck through where missing. Four filters — *No poster*, *No
  overview*, *No IMDb id*, *Never refreshed* — narrow the list together and
  are kept in the address (`/admin/catalogue?missing=poster`);
  `GET /api/v1/items?missing=poster,overview` lists the works lacking both,
  for those who may edit the catalogue.
- A row's actions — refresh, enable or disable, delete, the public page, the
  editor — are behind one button, which fits a phone, where they used to be
  in the editor only.
- Rows ticked in the administration's catalogue can be given one field at
  once with *Change a field…*: the original language, the status, the
  network, the studio, the adult flag or the content rating set to one
  value, a genre added or taken off, or the lock on any of them lifted. Each
  work is its own request and its own line in the audit trail; a sentence
  says first what is about to happen, and the run ends with how many were
  done, left as they were, passed over or refused, and why.
- *Ask the sources again…*, on the same rows, asks some of the sources
  switched on about each work they describe, as the editor's sync does for
  one: the other sources keep their last answer, and a work entered by hand
  or never refreshed in full is passed over.
- The editor's seasons tab shifts every dated episode of a season by a
  number of days — a season pushed back a week — with the episodes, their
  dates now and after, laid out before anything is done. The day each aired
  and its broadcast instant move together, and each is locked at its new
  value.
- An entry by hand takes every field the server does. The form asks for the
  status, the runtime, the first aired or in-cinemas day, the network, the
  studio, the genres and the IMDb id beside the title, year, overview and
  the TMDB and TheTVDB ids, and says what is wrong beside each field before
  anything is sent. The server checks the same: a status the kind does not
  have, a year not between 1800 and 2200, a runtime past 10,000 minutes, an
  identifier of nought or an IMDb id not shaped `tt` and seven to ten digits
  is refused with a `400`.
- The order of the sources is a setting, `sources.priority`, put in order on
  the Sources page: a source moves a rank with its arrows, or is dragged
  onto another's place, and the matrix's columns follow. Every refresh
  merges the sources in that order, so a work takes a new order at its next
  one; *Back to the starting order* restores the one the server started
  with, and `GET /api/v1/sources/rules` gives it as `defaultOrder`.
- How long a work keeps before it is refreshed is a setting:
  `refresh.ttlContinuing` for a running series or a film not out yet,
  `refresh.ttlEnded` for an ended series or a released film, in seconds, on
  the Settings page beside the other refresh settings. The scheduler reads
  them each time it schedules a work.
- Release dates and ratings are kept country by country. Every country TMDB
  lists for a film is kept — each premiere, run in cinemas, digital and
  physical release, with its day, its rating and TMDB's note (a festival, a
  service) — and every rating TMDB and TheTVDB give a series, where only one
  of each was kept (`releases` on a work in the native API). Radarr is sent
  every country's certification, as api.radarr.video sends them, and takes
  the one of the country it is set to. Filled in as each work is next
  refreshed.
- A work's page dates and rates it for its reader. The plate shows the
  rating of the reader's country, and a new panel, *Releases* for a film and
  *Certifications* for a series, lists that country's days and rating, every
  other country a press away. The reader's country is the one picked under
  *Where to watch*, else `tmdb.watchRegion` as set for them, else the
  language's own; where it has none, the record's own rating is shown with
  the country it is of.
- The schedule places a film on its reader's day. In `/calendar`, the week,
  the month, `calendar.ics` and the airing feed, a film comes out on the day
  it comes out in the reader's country — in cinemas, digitally, on disc —
  where that is known, and on its own dates otherwise; a date fixed by hand
  stays where it was put. `GET /api/v1/calendar` takes `region` to ask for
  another country.
- The tagline, the countries and the companies are kept. TMDB's tagline is
  shown under the title; every country a work was made in and every company
  that made it are kept, where only the first of each was. They are on the
  work's page and in its NFO (`<tagline>`, a `<country>` each, a `<studio>`
  each), and all four can be locked in the editor.
- A work keeps TMDB's writers (the screenplay, the story, the novel), a
  series' creators and a film's producers beside its directors, each with
  the job TMDB gives them; Radarr is sent those jobs, and the page names
  each job once with its people. How many of TMDB's actors a work keeps is a
  setting, `tmdb.castLimit`: 25 as before, up to 200.
- TMDB's Wikidata item is kept as an identifier, linked from the page and
  editable among the others. The format AniList gives an entry — OVA, ONA,
  TV shorts, special, music video — is shown on the page as a chip where it
  says more than *series* or *film*.
- The site has a bar along the foot of a phone's screen — Home, Browse,
  Schedule, Seasons and More — where a thumb rests, in place of a menu
  button at the top that took two gestures to change section. The place the
  reader is in is marked by a rule as well as a colour, and *More* opens a
  sheet with what the four leave out: the collections, the selections while
  there is one, the figures, the lamp, the language, and the account or the
  way in. The page keeps room under its own end for the bar, the footer
  included.
- The browse page lists as well as it grids. A switch beside the order lays
  the results out as rows — a small poster, the title, the year and the
  kind, the score, the status, three genres at most, two lines of synopsis —
  each with the one link, which is the whole row. `?view=list` is a link to
  it; what the reader chose last is remembered in the browser for an address
  that says nothing, and clearing the filters keeps the view. A long list is
  not laid out until it is scrolled to.
- The settings page can be searched. A setting is found by its title, its
  key or a word of what explains it, accents and ligatures folded (*oeuvres*
  finds *œuvres*), and the index of the page follows what is left. A setting
  changed from what it began as is marked *Modified*, says what that was,
  and goes back to it in one press; *Changed only* lists those. What the
  server was started with — its version, its database, its public address,
  the policy at each door — is a section of its own at the foot of the page.
  `GET /api/v1/settings/server/-` carries each setting's `default`.
- The dashboard opens on what there is to do (`GET /api/v1/admin/todo`):
  works whose last refresh failed, with no poster, no synopsis, no IMDb
  identifier, or never refreshed; API keys that lapse within fourteen days;
  pictures and themes given up on. Each line leads into the catalogue
  filtered to those works (`/admin/catalogue?missing=poster`) or into the
  page that settles it, and a figure of nought is no line. Each counts the
  works the list it leads to shows, those switched off among them, and adult
  titles as the server's policy has them.
- `scripts/e2e-stack.sh` runs the whole stack end to end with the real
  software, from the tree's own binary on a catalogue of its own:
  Sonarr 4, Sonarr 5 (a pre-release, 5.0.0.2635), Radarr and a Plex Media
  Server in Docker (`compose.e2e-stack.yaml`), each resolving
  `skyhook.sonarr.tv`, `services.sonarr.tv` and `api.radarr.video` to the
  server and trusting its authority. It searches and adds real series and
  films, compares seasons, episodes, classifications and collections with
  what the server serves, adds a series in French and in its DVD order on
  Sonarr 5, locks a title and sees it in each client after a refresh,
  connects the server to Sonarr and Radarr and finds the command a lock
  sends in their history, seeds an empty catalogue from their libraries,
  and has Plex add the provider, make an agent of it and scan a film and a
  series. Plex's own API needs no token on a server nobody has claimed; the
  script uses none and documents the two ways to a claimed one.

### Changed

- A 429 says when to come back (`Retry-After`), and for the per-address
  limit its quota (`RateLimit`, `RateLimit-Policy`).
- Where to watch shows four services a group, the rest behind a count. On a
  streaming platform, an episode timed at midnight UTC is "available on" its
  day, with no clock.
- A person's page agrees in French with TMDB's gender (`gender` in the API);
  a biography is set a paragraph to a line, its *emphasis* as emphasis. A
  keyword wears a link's chip.
- A film's own calendar lists every release, a day each, under UIDs
  `release-{id}-{cinema|digital|physical}`: an app subscribed to it replaces
  its one event once.
- CI checks the dependency tree with `cargo deny`, audits `Cargo.lock`
  weekly and whenever it changes, builds with the minimum Rust version, and
  scans the image with Trivy, reported to code scanning without failing the
  run.
- A picture's address, typed or pasted into the picker, is kept as the
  browser reads it: parsed, of `http` or `https`, and written back whole —
  the same address the preview shows. A line that parses as no address, or
  as one of another scheme, is not offered for keeping.
- The TMDB and TheTVDB relays write a text locked for one language alone
  (*this language only* in the editor, `language` on the API) into the
  documents in that language. A text locked in every language — every lock
  made before this version — is written only into those in the catalogue's
  language, as the TMDB relay did: a German Jellyseerr is still answered
  TMDB's own German. TheTVDB's relay, which wrote it into a translation in
  any language, does the same now.
- The webhook settings are gone: at the first start of this version,
  `webhooks.url` and `webhooks.events` become a destination of the
  Notifications page — *Webhook*, of the kind *JSON, as before* (`json`),
  with the same address, events and body — if an address was set and no
  destination exists, and are then removed; the settings API refuses them as
  unknown. `POST /api/v1/admin/webhooks/test` goes too, in favour of
  `POST /api/v1/admin/notifications/{id}/test`, one test per destination;
  `GET /api/v1/admin/webhooks/events` stays.
- A title locked in every language is every language's name for the work in
  the lists of translations Sonarr and Radarr read as well. The original
  title Sonarr v5 is given stays the one the work was made under, or a
  locked original title.
- The pictures beside the `.nfo` documents go by the names Kodi and Plex
  both read. Where the two differ — a season's poster (`season01-poster.jpg`
  for Kodi, `season01.jpg` for Plex) and an episode's still
  (`S01E01-thumb.jpg`, `S01E01.jpg`) — the picture is written once and the
  second name is a hard link to it, the same bytes on the disk; a disk that
  cannot link, a network share or FAT, is given a copy.
- Skyhook is read in its second edition, the one Sonarr v5 reads
  (`/v2/tvdb/shows/eng/{id}/official`, `/v2/tvdb/search`), and in its first
  only when the second answers with an error: a series it holds now brings
  every language TheTVDB names it in, each season's own title and the title
  in the series' own language. Episode ids are read under either name the
  editions use (`tvdbId` or `metadataId`), so a mirror of this server is
  read as well as the real service.
- `AMS_PROVIDER_PRIORITY`, `AMS_REFRESH_CONTINUING_TTL` and
  `AMS_REFRESH_ENDED_TTL` only give `sources.priority`,
  `refresh.ttlContinuing` and `refresh.ttlEnded` their first value, as
  `AMS_REFRESH_INTERVAL` does for its setting: once a setting exists, the
  variable is not read again. The two intervals are held to the settings'
  ranges at start — a minute to a year for the first, a minute to ten years
  for the second — and a value outside them stops the server, naming the
  variable.
- AniList's tags stay beside TMDB's keywords. Keywords are gathered from
  every source, each once whatever its case, where the first source with any
  gave the whole list, so AniList's tags went the moment TMDB answered.
- A rating from Radarr's service is filed under the United States when it
  has one there, rather than under the first country it lists, which was
  Argentina.
- `episodes` in `GET /api/v1/calendar` is deprecated: it repeats the
  episodes `entries` lists beside the films, the site no longer reads it,
  and a later version will drop it.
- CI runs the Playwright suite: a job builds the interface and the server,
  starts it on a new SQLite database, fills the catalogue with
  `scripts/e2e-seed.sh` — the 32 works the suite was written against,
  imported the way the Discover page does — and runs the desktop project and
  the server-wide specs. A failed run keeps Playwright's report and prints
  the server's log. It needs `TMDB_API_KEY` and `TVDB_API_KEY` as repository
  secrets (`FANARTTV_API_KEY` adds the artwork) and stands aside, saying so,
  on a run given none, a fork's pull request.
- CI runs the tests that open a database against PostgreSQL as well as
  SQLite, each on a database of its own, and with them a test that says the
  two migration sets build the same schema — tables, columns, keys, indexes,
  foreign keys, the values a `CHECK` allows, triggers. What the engines
  differ in on purpose (SQLite's FTS5 search table, PostgreSQL's `pg_trgm`
  indexes) is listed in the test with its reason, and fails it once it stops
  being true.
- Every compose file gives the server twenty seconds to stop
  (`stop_grace_period: 20s`): a stop takes up to fifteen — the doors drain,
  the background tasks end, the database's connections are handed back, five
  seconds each — and `docker stop` waits ten before it kills.
- The tasks page's history is a page at a time — fifty runs, and *Load
  more* — read under the day each ran on (*Today*, *Yesterday*, then the
  date), with a day's failures counted beside its name, instead of a hundred
  runs in one block after which the older ones were gone. *Failures only* at
  its head asks for those, and a history with nothing failed says so.
- The dashboard counts the locked fields last and small, among the other
  figures, instead of as the largest thing on the page. The explanation of
  locking folds to its title and stays folded where the reader left it, and
  the banner of failed refreshes is the first line of the list of what there
  is to do.

### Fixed

- A background loop that panics or returns is said so and started again,
  after a wait that doubles up to five minutes, rather than stopping in
  silence until a restart.
- `/v1/movie/changed` names the films changed here too — fetched again, a
  lock put on, changed or lifted — with or without TMDB. The parts of
  `/v1/movie/collection/{id}` name their collection, which carries its
  pictures.
- A long series' row of season numbers scrolls back to its first seasons on
  a wide screen.
- A season's poster was never written beside the `.nfo` documents: the
  export looked for the seasons' pictures among the work's own, so no season
  poster was written and no season thumb named in the series' document. The
  specials' poster is named `season-specials-poster`, as Kodi and Plex name
  it, and not `season00-poster`, which neither reads.
- A `.nfo` marks exactly one identifier as the default, the one Plex's NFO
  agent builds a work's identity from. A series known by IMDb alone, a film
  known by TheTVDB or Fankai alone and an episode known by TMDB alone had
  none.
- A work added to a hand-made selection keeps the day it joined through
  every later save; each save used to date every member afresh.
- The form for an entry by hand no longer says that an external identifier
  makes the entry refreshable, which an entry made by hand never is. It says
  what the identifiers are for: what Sonarr and Radarr find the entry by,
  and where its links lead.
- Radarr's crew is no longer filed as cast. A film made from Radarr's
  service alone listed its composer and its editor among the actors; its
  directors, writers and producers are kept, and the rest left out.
- A migration added between two builds is read into the binary.
  `cargo build` and `cargo test` went on with the schema from before it
  until something else in the crate changed.
- Plex is given the day in cinemas a person fixed, whichever country it asks
  in (`X-Plex-Country`, `tmdb.watchRegion`): the day that country's releases
  give came in its place, and a lock on the date did nothing there. The day
  online and the day on disc are read the same way.
- A picture sent up and chosen as a selection's cover is kept when it is
  taken off the work it was sent for, and by the sweep. A cover did not count
  as naming it, so the picture was deleted and the selection shown without
  one.
- A destination that does not answer no longer holds up the others. An
  address that times out, or is never connected, is posted to once, and what
  else is due to it is put off as that try was, each delivery by its own
  schedule; one destination is posted to a delivery at a time, in the order
  they are owed. Four posts at once, fifteen seconds each, kept every other
  destination waiting behind a dead address.
- A Sonarr or a Radarr that does not answer is asked once when works are to
  be told of it, not once for each: the first is written down as failed, the
  rest wait for the next pass, and the log says so once. It had a timeout, a
  failed row and a warning for every work.
- A title or an overview somebody wrote for the language Sonarr 5 or Plex
  fall back to is the one they are given, and is said locked, as it is for
  the language asked; the fallback no longer takes the translations to have
  been made to carry the locks already.
- A selection's cover is refused at an `http://` address, and says why: the
  site loads no picture over http, so the cover was kept and shown as a gap.
- A `.nfo` leaves out U+FFFE and U+FFFF, as the feeds and the Kometa file do:
  a title carrying one made the whole document unreadable to a strict XML
  reader.
- Sonarr 5 takes this catalogue's titles into its scene mappings. Each
  mapping the server adds to `services.sonarr.tv`'s list now carries a
  `mappingId` — `ams-` and twenty-four hexadecimal digits, the same for the
  same series and title on every answer. Sonarr 5.0.0.2635 keeps its list by
  that field and skipped, with a warning, every mapping without one: all of
  this catalogue's. Sonarr 4 reads no such field. Found by running a real
  Sonarr 5 against the server.
- A series asked for in a language whose translation has an overview and no
  title — TMDB leaves the name out where it is the one the series already
  goes by, as Breaking Bad's and Stranger Things' in French — is listed in
  that language among its `translations` for Sonarr 5, under the title the
  answer gives. The entry was left out, and the overview with it, where the
  real Skyhook lists one.

### Security

- A key kept for your own Sonarr or Radarr goes to the address it was given
  with and to no other: a test or a save at another scheme, host or port
  needs the key typed again, and an address removed takes its key along.
- A redirect from your own Sonarr or Radarr is followed only to the same
  origin — scheme, host and port — and so is one from MyAnimeList's client.
  Another port or another scheme of the same host was followed, with the key
  (`X-Api-Key`, `X-MAL-CLIENT-ID`) along.
- Kometa's metadata file, and the text file and the feed of a private
  selection, are read with a key limited to reading when it is an editor's or
  an administrator's: the address given to a tool no longer has to carry a key
  that could edit the catalogue. A key of the server's still needs the write
  scope, and a member's key has no such reading to lend.
- A token kept for an ntfy or Gotify destination goes to no other server and
  no other kind of destination unless it is given again, and a Standard
  Webhooks receiver at a new address is given a signing secret of its own.
- The Plex provider reads a match only from a body sent as
  `application/json`, follows a few of the ids it names, each once, and
  fetches and keeps nothing for a caller who may not see works for adults.
- With each client's own key relayed, a key issued here is refused wherever
  `Authorization` carries it, whatever its scheme or case, and is never
  handed on beside a client's own token.
- A notification destination's address is never shown whole — the API and
  the page show its scheme, host and port, since the token of a Discord,
  Slack or ntfy address is in its path — and signing secrets and tokens are
  write-only; none is written to the log, and the audit trail names a
  destination, never its address or token. Webhooks are posted without
  `HTTP_PROXY`, through the resolver that refuses loopback, link-local and
  cloud-metadata addresses (a home network's own are fine), and a redirect
  is not followed: an address that answers 3xx has moved.
- rustls 0.23.46, for RUSTSEC-2026-0346 (a session's `Debug` showed its TLS
  secrets), -0347 (data sent on after an ECH rejection), -0348 (a compressed
  certificate inflated to 1 GiB before authentication) and -0349 (an
  unauthenticated ServerHello downgrading later key exchanges). The web UI's
  build takes source-map-js 1.2.2 (GHSA-68fv-2mgg-jv7q); nothing of it is
  shipped.
- `cargo audit` and `cargo deny` set aside RUSTSEC-2026-0367 to -0369, in
  `zip`'s extraction: `zip` 8 serves only the build of the Swagger UI, from
  an archive vendored in a crate Cargo.lock pins by checksum, written out
  without the functions at fault. No `utoipa-swagger-ui` takes the fixed
  `zip` 9 yet.

## [0.7.0] — 2026-10-05

### Added

- The schedule has a month view (`/calendar?view=month`): a grid of the
  month's days, Monday first, each with what airs on it — three at most, the
  rest a step into the week — and today marked. On a phone the days carry a
  mark an episode and a tap opens the week at that day. The week stays the
  default; a switch in the heading goes between the two.
- A work at random. The browse page draws one under the filters as they
  stand, from the count it already holds; the command palette offers one from
  the whole catalogue.
- Recently viewed. The front page keeps a shelf of the works opened lately,
  and the command palette lists them before anything is typed — kept in the
  browser alone, sent nowhere, cleared with one button.
- A work's page offers to share its address: the system's share sheet where
  there is one and a finger, the clipboard elsewhere, with the copying said.
- A poster that cannot be shown — no picture, or one whose address no longer
  answers — gives way to a drawn stand-in: the title's initial in the display
  face, the kind, the title. Wherever a poster can be missing, at every size.
- Opening a work from its card is drawn, where the browser draws view
  transitions: the poster travels onto the page's plate and the rest crosses
  over. A reader who asked for less motion, or a browser without the API,
  arrives as before. A card asks for its work as soon as a pointer rests on
  it, so the page is there by the time it is opened.
- The web app manifest names three shortcuts an installed app offers from its
  icon: the catalogue, the schedule, the season chart.
- `original_language` on `/api/v1/items` and `/api/v1/facets` takes several
  codes, comma-separated (`ja,jpn`): the works made in any of them.
- A season's poster is chosen: `season:N / primaryPoster`, a lock like the
  work's own, by the address of one of the season's pictures — or of any
  picture, brought back under the season when its sources no longer list
  it. The season's page, the season chart and Sonarr lead with it. Set from
  the season's own panel in the editor, through the one picker every picture
  is now chosen with: the work's own pictures at the shape wanted, filed by
  this season, the work and the other seasons; a file, dropped, pasted or
  picked, kept by this server; or an address, previewed first. The same
  picker sets an episode's picture, the work's poster and background from
  the editor's masthead, and adds a picture to the work.

### Changed

- The site's navigation offers *Selections* — in the bar, the catalogue's
  panel, the phone's menu, the footer and the command palette — only while
  there is a selection to show. A way to an empty page was no way.
- The figures page says genres in the reader's language, counts a language
  once however many ways its providers spell it (`ja` from TMDB, `jpn` from
  TheTVDB were two bars of *Japanese*), and every bar leads into the
  catalogue narrowed to what it counts.
- The browse page's language filter offers each language once too, asking for
  every spelling at once.
- The dashboard's health card no longer repeats the last runs the panel above
  it lists.
- The lock on a public card shows only where a source could be overruled, as
  the administration already had it; captions under posters are set at 12px.
- The editor's artwork tab keeps to the work's own pictures: a season's are
  decided on the season's panel, and one line counts them and leads there.
  The forms that added a picture by address or by file give way to the
  picker.
- The calendar reads the episode text held in the language asked for in
  one query for every series in the window, not one each; the figures page
  is kept a while, as the facets are.
- `AMS_REQUEST_TIMEOUT`, `AMS_REDIS_TIMEOUT_MS`,
  `AMS_DATABASE_MAX_CONNECTIONS` or `AMS_DATABASE_ACQUIRE_TIMEOUT` at zero,
  and `AMS_TMDB_SEARCH_LIMIT`, `AMS_REFRESH_INTERVAL` or
  `AMS_REFRESH_BATCH_SIZE` outside what their settings hold, are refused at
  start, naming the variable — as is a surface policy that is none.
- `AMS_ALLOWLIST` still set, and different from the list on the Access page,
  is said at every start: it only ever gave the list its first value.
  `compose.yaml` and `compose.multi.yaml` no longer set it, and
  `compose.integration.yaml` names its clients' addresses rather than the
  network Docker's gateway is on.
- `compose.multi.yaml` keeps its media in Garage: the MinIO images it pulled
  are no longer published.

### Fixed

- A work read while an edit was being put on it from another request could
  be kept in the cache a moment too old, and be answered without the edit
  until the cache let it go. What is read across a write is now answered
  but not kept. Among several instances, a work read while another instance
  wrote it could reach the cache server after that instance's forgetting, and
  be served as it was before for the cache's hour. The copy is kept only once
  the cache server has it and nothing was written meanwhile, and taken back
  otherwise; the word of a work written elsewhere forgets the cache server's
  copy as well as this memory's.
- Un-starring a picture uploaded here, unlocking it, or unlocking every
  field deleted the file while the work still showed it — or while a
  season, an episode or another work still named it. An upload is now
  deleted only once nothing names it: no picture of any work, no theme, no
  lock. Taking a copy away by its id takes only one of that work's own.
- A sweep, a reset or the removal of a copy could delete the file of a
  picture being stored at that very moment — after a reset and "store
  everything", most of them — leaving it broken until the next reset. A
  picture's row names its file before the file is written, a file taken in
  between is put back, and the sweep asks again, of the rows and of the
  store, just before it deletes anything. A file the store will not delete
  is counted and passed by instead of ending the sweep, and a removed
  copy's row goes before its file, never leaving a row that names nothing.
- `/media/{key}` answers a range in another unit, one that does not parse,
  or several at once with the whole file rather than `416`, and a range
  that runs past the end with what there is; its `304` says the `ETag` and
  the caching a `200` would; `If-None-Match` compares weakly, `If-Range` is
  honoured, and `HEAD` reads nothing of the file. A file is served as its
  key's extension says.
- "Forget every copy" holds off fetching while it forgets — on this
  instance, and "store everything" on any other — where a fetch that began
  in between could bring back copies only the index knew of.
- A search for `_` or `%` found every work, and `a_c` found `abc`: a term is
  matched as typed. One with nothing a slug could be made of no longer
  finds every work called *Untitled*. A term, a keyword, a status or a
  network is at most 200 characters, and genres or original languages at
  most twenty, rather than a statement the engine refuses.
- The schedule, `calendar.ics` and `airing.atom` compare an episode's time
  as an instant: a time locked by hand with its zone (`21:00:00+02:00`) or
  its fraction landed on the wrong day, in the wrong order.
- A season, an episode or a work entered by hand is checked as the same
  fields are when locked: a day as a day, a runtime and an absolute number
  not negative, a picture's season one the series has, a language the shape
  of one, at most fifty genres and fifty AniList or MyAnimeList entries. A
  work entered by hand with an identifier another work goes by is refused
  where it is written, so two made at once can no longer both have it; and
  a list is written with its members in one go.
- The health check reads the server's own configuration — every name the
  bind address goes by, an empty variable as unset, the address it is bound
  to when it listens on one interface — and `/health`, `/ready`, `/ca.crt`
  and `/trust-ca.sh` answer under whatever name they are asked: with
  `AMS_ALLOWED_HOSTS` set, the container no longer turns unhealthy, nor a
  client's trust script fails, for asking by address.
- A fingerprinted bundle no longer built answers `404` rather than the page,
  which the browser then refused as a script; `/api/`, `/v1/`, `/3/` and the
  other API roots answer the API's `404`, not the interface.
- A setting the database could not store is a `500`, logged, rather than a
  `400` that handed the driver's message to the administrator.
- Reading one's own keys (`GET /api/v1/account/keys`) no longer empties the
  session cache of every instance.
- The request metrics count the TheTVDB and AniList relays, the media kept
  and the authority's files under names of their own, and
  services.sonarr.tv's list as Sonarr's, where they were the interface's or
  Radarr's.
- The TMDB relay writes into a title's document only the fields a person
  locked, as the TheTVDB and AniList relays do. One lock on a work — a
  poster, an episode's title — replaced TMDB's whole document with the
  catalogue's text, dates, runtime and status, and every genre with one
  whose id was `null`. The text — the title, the overview, the homepage, the
  genres' names — is written only for a client that asked in the
  catalogue's language: a German Jellyseerr is answered TMDB's German, not a
  lock written in French. A locked genre keeps TMDB's id; one TMDB has no id
  for leaves TMDB's genres as they came. Only a whole document (`200`) is
  patched, and only the fields it carries.
- The AniList relay matches an entry by MyAnimeList's id only when the entry
  says it is an anime — its type, its format, or a count of episodes, a
  season or a duration. MyAnimeList numbers anime and manga apart, in
  numbers that overlap, and Yamtrack's import asks both lists in one query
  without either: Death Note's manga took One Piece's locked title and
  poster, and Yamtrack kept them. A media's tags, which say whether they
  are for adults as a media does, are no longer taken for entries.
- A refresh that some providers did not answer — an error, a timeout, a
  `429`, a queue that was full — was written as complete, for six hours or
  for a week: TMDB's numbering under a TheTVDB id when TheTVDB or Skyhook
  failed, a season TMDB could not fetch written with no episodes (which
  Sonarr then deletes, files and all), the adult flag cleared when AniList
  or MyAnimeList did not answer. What each one gave the work before is now
  put back in its place — values, pictures, translations, ratings, the
  episode list it numbers, a season's episodes — the adult flag stands, and
  the work is written as refreshed in part: the providers that failed are
  its refresh error, and it is tried again within six hours, or sooner
  near an air date. TheTVDB's answer counts as failed when its episodes
  could not be read, rather than as a series with none.
- A season or an episode added by hand was written over by the next
  refresh once a provider listed the same number: its title, its date, its
  still. It now stays as it was typed, on SQLite and on PostgreSQL.
- TheTVDB's season pictures were never kept: they name their season by
  `seasonId`, which was not read. They are now filed under the aired
  order's season it names, and those of the DVD or absolute orders left
  out.
- A run this process started in its first half-minute — the anime list's
  import, at twenty seconds — was closed as failed by the cleanup of the
  runs a crash left open, then succeeded. Only runs opened before the start
  are closed now.
- The scores, the runtimes and the order of pictures a provider gives can no
  longer overflow: a sum past what they hold panicked a debug build and
  wrapped a release one.
- A page that throws no longer takes the whole app with it. A value that was
  not what the page expected — an answer that was not a work, or a file that
  is gone after the server was updated under an open tab — left a blank
  window, bar and all. The page now says something went wrong and offers to
  try again, with the bar or the sidebar where it was; where the cause is a
  file that is gone, trying again reads the page afresh.
- *Collections*, in the browse panel and the phone's menu, leads to a page
  and not to the 404 it led to. The collections have a page of their own at
  `/collections`; the selections' page keeps its section of them, and a
  collection's way back leads to the new page.
- The command palette keeps the keyboard inside while it is open. It was a
  `div` that said it was a modal: Tab walked on to the links behind it. It
  is now the browser's own modal dialog, so the page behind is out of reach
  and Tab stays on the field; Escape and a press on the dark around it close
  it as before. An editor is offered the catalogue's pages and no longer
  those of the administration it cannot open.
- A key or a password shown while another is on screen is not already marked
  *Copied*. The panel kept what it had last copied across to the next
  secret, so a key issued beside the first looked copied when it was not,
  and could be dismissed unread: it is shown once, and a lost key has to be
  issued again. A second password reset on a member did the same.
- Copying an import address the browser refuses to copy says so and leaves
  the address selected. The fallback read the click's target once the click
  was over and threw, so the address Sonarr and Radarr import from was
  neither copied nor selected.
- The identity provider's settings say so when they cannot be read. A failed
  read left a skeleton on the page for as long as it stayed open.
- The draw on the browse page no longer does nothing without saying so. It
  drew from the count of the last filters while the new ones were loading,
  and an offset past the new count found no work; it now waits for the list,
  counts again when it comes up short, and says when there is nothing to
  draw.
- Saving a text list in the editor no longer splits an entry that has a
  comma in it: a keyword like *Hello, World* came back from the box as two,
  whether or not it had been touched. Such an entry is shown in quotes, and
  one typed in quotes stays whole.
- An identifier typed on the editor's *Elsewhere* tab has to be a whole
  number. `81189x` locked 81189 and `1e5` locked 1, without a word.
- The work editor's tabs point only at the panel that is open. Each named a
  panel that was not in the page unless it was the selected one.

### Security

- An address a picture or a theme is fetched from is judged at every
  redirect: one to this machine, to a cloud's metadata service, or to any
  address only the server can reach is not followed — written as an address
  too, in any of its spellings (`2130706433`, `[::ffff:7f00:1]`, NAT64,
  6to4), which never reached the resolver that refused such names. The
  addresses refused now take in AWS's metadata over IPv6 (`fd00:ec2::254`),
  Alibaba's `100.100.100.200`, Azure's `168.63.129.16`, multicast and
  `0.0.0.0/8`, and these fetches no longer go through `HTTP_PROXY`, a proxy
  resolving the names past the check. The NFO export fetches through the same
  client, writes a picture or a theme only when the bytes are one, and reads
  no more than one may weigh. The providers' client judges every redirect as
  the media client does: a hop written as an address only this server can
  reach is refused in any of its spellings, and so is a name that resolves to
  one, unless it is an upstream the configuration names. MyAnimeList's client
  id, a header of its own, is no longer carried by a redirect to another
  host.
- A private network stays reachable for a picture mirror kept at home;
  `AMS_MEDIA_PRIVATE_NETWORKS=false` refuses `10/8`, `172.16/12`,
  `192.168/16`, `100.64/10` and IPv6's unique-local `fc00::/7` — where
  Tailscale and OpenWrt number a network — to the media store and the NFO
  export.
- What an address answered when a picture could not be fetched — a status,
  a type, a connection refused — is shown to administrators alone; an
  editor is told what kind of trouble it was. Any address a writer cares to
  type was otherwise a way to map the network around the server.
- A thumbnail is made one picture at a time, and none of a picture past 36
  megapixels, which is served as it is; a picture past 16,384 pixels a side
  or 120 megapixels is not kept at all. Four large pictures decoded at once
  could take more memory than a small server has. A fetch is counted as it
  starts, so a picture that brings the server down is given up after five
  tries rather than taken again at every start; one a decoder panics on is
  kept without a thumbnail.
- A `language` is a language tag — two or three letters, then at most a
  region or a script: `fr`, `fra`, `pt-BR` — wherever it is asked for: a
  work, the catalogue, the calendar, a person, the collections, a list, the
  season chart, Sonarr's shows and search. Anything else is answered `400`;
  the feeds, which a calendar app polls for years, read it as none. It was
  the key episode text is filed under, a segment of the path TheTVDB was
  asked on with the operator's token, and a line in the log, so a path or a
  query in its place reached other TheTVDB addresses, and every new string
  was a season's worth of calls and a row kept for good: anybody who could
  open a work's page could spend the operator's TMDB and TheTVDB quota on it,
  one new language at a time. What was filed under a key that is not two or
  three letters goes on upgrade. A work's episode text is fetched only in a
  language the code tables know, and in at most eight between two of its
  refreshes; a ninth is served what is held in it.
- The collections and a person's page ask TMDB in the language asked for
  only as far as it is one TMDB has, by its two letters — the reader's own
  otherwise — so `en-AA`, `en-AB`… are one answer kept, not one call to TMDB
  for every collection held on every request; and an answer that failed is
  not asked for again for five minutes.
- A lock on a work's identity — adult, its slug, its identifiers — is
  written to its row in the lock's own transaction, by an import of locks
  as by an edit by hand, so a work locked adult leaves every list, the
  calendar, the feeds and the clients' lookups at once; a slug or an
  identifier another work holds refuses that lock. The row now follows the
  adult lock whatever wrote it last — a sync from chosen sources included —
  and every work is listed again once, which puts right a work locked adult
  before its row followed. Lifting such a lock brings the work's refresh
  forward to now.
- `/stats` counts for a reader what the lists show them — works switched on
  and, as their adult policy has it, not for adults — and tells keys,
  locks, the journal, the jobs and the caches only to whoever maintains the
  catalogue. A list's count of works leaves out, for a reader kept from
  them, those for adults.
- A work's raw provider documents, its locks and its `.nfo` are refused,
  as the work is, to a reader it is switched off for or kept from by the
  adult policy; and the locks no longer name who set them to anybody but
  whoever maintains the catalogue.
- Clearing every setting of a key or of a rule is written in the journal,
  by the keys it cleared: dropping a key's `adult.clientPolicy` hands it
  the adult titles back.
- A burst of sign-ins no longer takes the memory argon2 needs for each at
  once: passwords are checked a few at a time — as many as the machine has
  cores, from two to eight — and an attempt left waiting five seconds for
  its turn is told to come back (`429`), nothing recorded. An account that
  failed `AMS_SIGNIN_FAILURES_PER_ACCOUNT` times (10) in fifteen minutes is
  refused without being checked, from wherever the attempts come, until a
  sign-in goes through or the window closes. With passwords off, any other
  name takes as long to refuse as the one `AMS_ADMIN_USERNAME` names.
- The rate limit, the record of callers and the lookups of their names count
  an IPv6 client by its /64, as sign-ups already did; four lookups at most
  run at once, and past that a name is not looked up.
- Every `X-Forwarded-For` line is read, in order, as one list — HAProxy adds
  a line of its own after the client's, and only the client's was read — and
  a hop that is not an address, or not text, leaves the client unknown
  instead of making it the proxy, whose own address is usually allowed: an
  unknown client is refused by every allowlist, counted in one rate-limit
  bucket with the others, and journaled as the proxy's address with a note.
  A request forwarded by a proxy missing from `AMS_TRUSTED_PROXIES` is
  refused on the address-guarded surfaces, with a warning naming the proxy.
- A change carried by the session cookie — or, on the native API under
  `allowlist` or `open`, by the browser's address — is refused (`403
  cross_site_request`) when the browser says another page asked for it: a
  `Sec-Fetch-Site` other than `same-origin` or `none`, or, from a browser too
  old to send one, an `Origin` that is not this server's, `AMS_PUBLIC_URL` or
  one of `AMS_CORS_ORIGINS`. Every other application on the same host or
  domain is the same site, and its pages could post here with the cookie.
- Each door holds `AMS_MAX_CONNECTIONS` connections at once (1024), and
  gives a connection `AMS_HEADER_READ_TIMEOUT` seconds (30) to say what it
  wants — its first bytes, a request's head — and to sit idle between two
  requests; an idle HTTP/2 connection is pinged. A connection that sent
  nothing was held for ever, and a few thousand of them took every
  descriptor the process had. A stop drains every door at once, the
  clients' included, for five seconds at most.
- Release builds unwind a panic rather than abort: a handler that panics
  answers `500`, as the panic layers were there for, instead of taking the
  whole server down, and a panic while refreshing one work fails that work,
  not the sweep.
- `Permissions-Policy` on every answer, `Cross-Origin-Opener-Policy` on
  pages, and `Strict-Transport-Security` (a year) where a request came in TLS
  — this server's own, or a trusted proxy's word; the security headers also
  on the `504` and `500` the timeout and the panic layers make, and on the
  documentation's redirects. The clients' door answers with a content
  security policy that lets nothing run. The interface's lets a picked file
  be previewed (`blob:`).
- The session cookie is `Secure` too where a trusted proxy says the request
  reached it in `https`; two session cookies on one request — a second can
  only have been planted beside this server's — sign it in as nobody.
- What is logged has its secrets masked: the values of `api_key`, `apikey`,
  `token` and their like in any address an error repeats, what follows
  `Bearer` or `Basic`, the password of an address, the keys and passwords the
  configuration holds. The address of a webhook — the secret that lets anyone
  post to a Discord or Slack channel — is no longer written to the log when a
  delivery fails. The request span records no query value; a failed sign-in's
  username is logged quoted, its line breaks escaped; the configuration's
  debug print shows no secret.
- `trust-ca.sh` refuses an authority fetched over plain HTTP unless
  `AMS_CA_FINGERPRINT` pins it or `AMS_CA_INSECURE=1` accepts the risk.
- A person's key paused by an administrator stays paused until an
  administrator resumes it: its owner can no longer resume it from their
  account.
- A key's `expiresAt` is a time — RFC 3339, or a date — refused when it
  cannot be read or has passed, kept in UTC and compared as a time:
  `31/12/2026` sorted after every date and never ran out.
- Changing or resetting a password, closing a session, and an account
  taken out of service forget the sessions remembered after the rows go as
  well as before: a request read in between could keep a closed session
  alive in the cache for its lifetime.
- A member can sign out. The way out was in the administration alone, so a
  reader with a plain account — which is what everybody who registers or is
  invited has — had none: on a shared machine their session, their account
  page and their keys stayed open to whoever sat down next. It is now in the
  catalogue's bar and its phone menu for anybody signed in. The account link
  in the bar keeps its word for wide screens, to leave it room.
- Signing out, or changing a password, forgets the shelf of recently viewed
  works. It stayed in the browser, so a work an editor had opened — switched
  off, or kept from the public — showed on the front page and in the command
  palette of whoever used the browser next, anonymous visitors included.
- The members' page acts on what it shows. A selection was kept across
  searches and filters and went on to the accounts the page no longer
  showed: picking one account, searching for another and setting the role to
  *Administrator* made both administrators, and approving, disabling and
  deleting did the same. An account a new search or filter takes off the
  page now leaves the selection, the delete dialog names the accounts it
  deletes, and the role is given by *Apply* — a select that acted on
  `change` gave it on a single arrow key.
- Choices that are written the moment they are made are not made by an arrow
  key. On *Opening & APIs* (the site private or public, how people sign up,
  the role a new account is given) and on a member's role and status, the
  arrow keys move along the options and Space, Enter or a press chooses. The
  browser's own radios chose with every arrow, and one stroke made a private
  site public or a member an administrator.
- A crafted address no longer reaches the API as a path of its own.
  `/work/..%2Fstats` asked for `/api/v1/stats` and showed what came back as
  a work, which threw while the page rendered. The ids and slugs the router
  hands over — a work's, a collection's, a selection's, a member's — are
  encoded wherever they go into an API path, and a segment of dots is
  refused.
- A genre named like an object's own property — `constructor`, `toString`,
  `__proto__` — no longer breaks the catalogue for a French reader. Looked
  up in the table of French genre names it came back as a function, and the
  genre filter threw as it folded and sorted the names, which took the page
  with it. The tables are asked for their own keys only: those of
  `labels.ts`, the footer's credits and the interface's dictionaries — the
  kinds in a gallery, a relation's type, a status.
- Signing in goes back only to a page of this site.
  `?next=/.//evil.example/x` was let through as `//evil.example/x`: the
  router refused to follow it, but only once the session was opened, so the
  form reported a failure for a sign-in that had worked. The address must be
  one path — a single leading slash, no backslash or control character, no
  `.` or `..` segment — and anything else lands on the usual page, as it
  does for the single sign-on button.
- The TMDB relay asks TMDB with the client's `User-Agent`, `Accept` and
  `Accept-Language` and nothing else it sent, as the other relays do. Every
  header but a short list travelled: a `Range`, which TMDB's edge answers
  with a fragment — and any `2xx` was kept, under a key shared through the
  cache server, so one caller could have every Jellyseerr served a byte of
  `/3/configuration` for a day — `X-Forwarded-For`, `Referer`, `Origin`.
  Only a whole document (`200`) is kept now, under a key that holds the
  headers that may change it, and the caller's validators go to TMDB only for
  an answer neither kept nor patched. TMDB's word on cross-origin reads, and
  an address of its own that would name the operator's key, are no longer
  handed back.
- What mints or reads a TMDB session — a request token, a guest session, an
  account's pages, anything asked with a `session_id` — is relayed but never
  kept: every caller was handed the same request token for six hours, and a
  person's session id was part of a key kept in the cache server.
- A parameter this server sets on the TMDB relay's request (`api_key`,
  `include_adult`) is told by its decoded name: `include%5Fadult=true` was
  left out of the key the answer was kept under but sent to TMDB ahead of
  this server's own. A parameter given twice keeps its order in the key, as
  the services read it.
- The TMDB relay reads TMDB's answer as the other relays read theirs, at
  most 16 MiB, and a body cut short no longer writes the address asked —
  the operator's TMDB key in its query — to the log. It marks its calls as
  this server's, and a call that came round a loop through the resolver is
  answered `508 Loop Detected` instead of asking itself again until the
  rate limit or the timeout ended it.
- TheTVDB, Radarr's service and Fanart.tv are asked with each value put
  into an address encoded as one segment of it, whatever the value holds.
- What a provider says when it answers with an error is read to 64 KiB at
  most, and kept to its first 300 characters: the body was read whole,
  bounded only by the timeout, and stored whole as the work's refresh
  error.
- A provider that answers `429`, or `503` with a `Retry-After`, is left alone
  for as long as it asks — a minute at most — by every request at once, and
  the request it refused is sent again once after a short wait. TheTVDB,
  Skyhook, Radarr's service and Fanart.tv are asked eight requests at a time
  at most; a Radarr bulk refresh of a hundred films was a hundred at once to
  each.
- When TheTVDB refuses to sign this server in — a wrong or rotated key, a
  missing PIN, TheTVDB down — the sign-in is not tried again for a minute,
  and the TheTVDB relay answers the calls made with the operator's key `503`
  for a minute, then two, up to ten while the refusals go on, rather than
  signing in again on each one: every call a client made was another sign-in
  with the operator's key, one after another, which TheTVDB rate-limits and
  may lock the key out for. A refused token is renewed once a minute at most.
- Among several instances, the settings and the network rules are read
  again every minute, with the caches' generation and epoch: a word lost on
  the cache server's channel left an instance on the old policy — the site
  public, an address range let in — until it restarted.
- What comes off the cache server is weighed first: a relayed document kept
  there is served only as JSON or plain text with a status that is one; a
  generation or an epoch far ahead of this instance's, a key no instance
  writes, a sender or a run that is not an id, a message longer than any
  instance sends, are not taken; and another instance's word of a picture
  kept is believed only for a key of this server's and the type its extension
  stands for.
- An original only the Fankai wiki names, which anybody can edit, is taken
  for one for adults — kept from readers who may not see those — unless
  AniList or the work the catalogue holds under that entry says otherwise.

## [0.6.0] — 2026-10-04

### Added

- TheTVDB's v4 API is relayed under `/v4/*`, in `api4.thetvdb.com`'s place,
  for the clients that ask TheTVDB themselves — Yamtrack, Jellyfin's plugin,
  Kodi's scraper. A request is handed on to TheTVDB and its answer handed
  back with the fields locked here written in: a series' or a film's name,
  overview, year, dates, status, runtime and chosen poster on its record,
  its name and overview on its translations, an episode's title, overview,
  date, runtime and still in the aired order and on its own. A client signs
  in with a key issued here (`POST /v4/login`) and is answered that key as
  its token; TheTVDB is then asked with this server's own key in its place,
  and the client's copy never leaves. With `AMS_TVDB_AUTH=allowlist`, a
  client's own TheTVDB key is signed in with as it came, and its token
  travels on. Reads only, and nothing of the operator's own account; what is
  answered with this server's key is kept a while, as the TMDB relay keeps
  TMDB's. `AMS_TVDB_PASSTHROUGH=false` turns it off; `api.tvdb` switches it
  on the access page.
- AniList's GraphQL is relayed in `graphql.anilist.co`'s place, on the
  clients' door, for the clients that import or track through it. A query is
  handed on and its answer handed back with the title, description, genres
  and pictures locked here written into every entry it carries — found by
  AniList's id or by MyAnimeList's, whichever was asked. Somebody's own
  AniList token travels with the query; a key of this server never does; an
  answer to a signed query, to a mutation, or about somebody's lists is
  never kept. AniList's rate limit is handed back as it answers it. By
  address (`AMS_ANILIST_AUTH=allowlist`), since AniList has no key a client
  could present; `AMS_ANILIST_PASSTHROUGH=false` and `api.anilist` as above.
- The clients' certificate carries `api4.thetvdb.com` and
  `graphql.anilist.co`. An authority made before does not cover them, and
  is left serving the others until it is replaced
  (`AMS_TLS_REPLACE_AUTHORITY`; docs/integration.md, *Upgrading a server
  that is already running*).

- A run in the tasks' history opens onto what it did: each work a refresh
  took, in order, with how it went — refreshed, failed and why, gone
  meanwhile — and a link to its record. The run's whole summary, its start
  and end, and the instance that ran it are read there too. Written down as
  the run goes (`job_entry`), kept and pruned with the runs; a run older
  than this says so.

### Changed

- A work with no identifier elsewhere — one made by hand, that no refresh
  ever touches — shows none of its edits as locks: not on its record, its
  seasons and episodes, nor in the catalogue's list. Nothing is locked
  against nothing. The server files the edits as it did, which is what
  keeps them the day a source is given.
- *Members too*, on the access page, lets members' keys onto every relay —
  TMDB's, TheTVDB's, AniList's — rather than TMDB's alone; the setting keeps
  its key, `api.tmdbMembers`. The *Relays* cache space holds what every
  relay answered.
- This server's own calls to TheTVDB and AniList carry its mark, so a
  resolver that sends those names here is met with a `508` rather than a
  loop.

### Fixed

- A refresh asked for a work made by hand left its run open in the
  history, as if still running, until a restart closed it with the runs a
  crash abandons. It is closed at once, as the refusal it is, and says why.

## [0.5.0] — 2026-10-04

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
- The work editor is a record in five tabs — *Record*, *Seasons &
  episodes*, *Artwork*, *People & titles*, *Elsewhere* — in place of one
  page of a dozen panels. The fields are grouped as a card groups them
  (identity, synopsis, broadcast or release, classification, links), can be
  narrowed to the locked or the empty ones, and are shown as what they are:
  a date as a date, a genre in its colour, an address as a link, a still as
  the picture, with the stored form beside it. The seasons stand in a rail
  beside the one chosen, each episode opening onto every field of its own;
  the artwork is a light table by kind, the seasons' own below; the
  identifiers, every rating a source gave, the related works, TheTVDB's other
  numberings and TMDB's suggestions have a tab of their own, and the
  translations held are listed beside the credits. The address carries the
  tab (`?tab=`), and the anchors the public pages link with still land on
  it. Enabling the record, unlocking everything and deleting it sit beside
  the sources rather than across the top.

### Added

- Editable, and locked like any field: a work's TheTVDB qualifier, the
  country of its rating and its TMDB collection; a season's TMDB and TheTVDB
  ids; an episode's TMDB and TheTVDB ids, and where a special belongs — after
  a season, or before an episode. The editor also offers what the API always
  took: an episode's synopsis, runtime, still and absolute number when it is
  added by hand, a season's synopsis, a credit's photograph, an alternative
  title's kind and language, and an image's language and the season it is for.

### Security

- A homepage, a theme or an episode's still locked by hand is refused unless
  it is an address a client can follow — `http` or `https`, or an upload's
  own origin — so a `javascript:` or `data:` scheme locked in by someone who
  may edit is never served as a link to a visitor or a client. The interface
  draws any other scheme as words rather than a link, wherever a field is
  linked. A YouTube trailer id is checked for its shape, and a rating's
  country for its two capital letters.

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

[Unreleased]: https://github.com/Dim145/arr-metadata-server/compare/v0.8.2...HEAD
[0.8.2]: https://github.com/Dim145/arr-metadata-server/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Dim145/arr-metadata-server/releases/tag/v0.1.0
