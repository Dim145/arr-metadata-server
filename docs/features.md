# What it does

A self-hosted metadata server for your media stack. It speaks the protocols
Sonarr, Radarr and TMDB clients already use, keeps its own canonical copy of
everything it serves, and lets you correct or invent entries by hand — with
manual edits permanently protected from automatic refreshes.

It grew out of two earlier, private tools — a TMDB relay and a stand-in for
Sonarr's metadata service — and does what both did, from one catalogue.

> **Status: working.** Every surface is implemented, exercised by tests on both
> database engines, and verified against the live TMDB API and against real
> Sonarr, Radarr and Jellyseerr containers — including that a refresh leaves
> manually locked fields alone. See [Pointing your stack at this server](integration.md).

- **One metadata source for the whole stack.** Sonarr's Skyhook API — its
  first edition, which Sonarr v4 reads, and its second, which Sonarr v5
  reads — Radarr's metadata API and the TMDB v3 API are all served from the
  same store.
- **Several sources behind it.** TMDB, plus Sonarr's and Radarr's own metadata
  services, merged field by field. They carry what TMDB has no field for: air
  time, TVMaze and AniList ids, absolute episode numbering, certifications by
  country, and IMDb/Metacritic/Rotten Tomatoes ratings.
- **Its own canonical database.** Provider payloads are kept verbatim as
  snapshots and merged into a canonical entity; nothing downstream depends on a
  provider being reachable.
- **Manual entries.** Create a work that exists on no provider at all.
- **Edits that stick.** Change a title, an air date, an episode order — the
  field is locked and every later refresh leaves it alone. The catalogue's
  filters, orders and counts follow the edit too: a work locked to Animation
  is found under Animation.
- **Automatic refresh.** Any entry carrying at least one external id is
  refreshed on a schedule that adapts to its status.
- **Client management.** Named API keys, scopes, expiry, per-surface policy, and
  an escape hatch to turn authentication off entirely.
- **Per-language answers.** Sonarr asks in its URL, the native API takes
  `?language=`. Translations are fetched the first time a language is asked for
  and kept — from TMDB, then TheTVDB for the many languages TMDB does not carry.
  A locked field stays locked in every language; a title, an overview or a
  tagline can also be locked for one language alone
  ([Editing](editing.md)).
- **Sonarr's alternate titles.** Sonarr recognises releases only by the
  titles of the list it downloads from `services.sonarr.tv`, never by those of
  a Skyhook answer. Resolve that name here too and the list reaches it with
  this catalogue's titles added — in the language of the answers, in English
  and romanised from the work's own, the ones Sonarr can match without
  mistaking one series for another — so a release named in French or in
  romaji is recognised (`sonarr.sceneMappings`). Everything else Sonarr asks
  of that host is relayed to the real service.
- **TheTVDB and AniList relayed too.** The clients that ask those services
  themselves — Yamtrack, Jellyfin's TheTVDB plugin, the anime trackers — reach
  them through this server once `api4.thetvdb.com` and `graphql.anilist.co`
  resolve here: each request is handed on and its answer handed back with the
  fields locked in this catalogue written in. A TheTVDB client signs in with
  a key issued here, and this server stands in with its own.
- **Curated lists.** Selections composed by hand or by a filter, shown on
  the site and served to Sonarr and Radarr as the custom lists their import
  lists read.
- **The catalogue in numbers, the server's health, and notifications.** A public
  page counts what the catalogue holds by decade, genre, network, language
  and score; the administrator's dashboard says what the server is, keeps and
  did last — and, when `updates.check` is on, whether a newer release is
  out, learnt by one anonymous GET to GitHub a day and nothing else; and
  what happens can be told to as many destinations as you like, on the
  Notifications page — a receiver of your own, signed as Standard Webhooks
  says, Discord, Slack, ntfy, Gotify, Apprise, or anything that takes JSON —
  each for the events ticked for it. To begin with, the events are what enters
  and leaves the catalogue: `item.imported`, `item.created`, `item.deleted` and
  `dataset.imported`; `*` is every one. Three facts nobody did are among them:
  `refresh.failed`, once when a work starts failing — grouped into one
  message every five minutes for Discord, Slack, ntfy, Gotify and Apprise;
  `media.failed`, a picture or theme whose every try failed;
  `update.available`. What a destination does
  not take is tried again for a day and more, and what it was told is kept
  thirty days in its log, with a test and a replay for each. A failed sign-in is
  posted without the name that was typed, and a destination's address and
  token are never shown whole, posted or logged
  ([Notifications](notifications.md)).
- **Related works, and who people are.** What AniList files beside an anime
  — its sequels, prequels, side stories, what it was drawn from — is kept
  with the work and shown on its page, leading to the work here when the
  catalogue holds it and to AniList when it does not; the seasonal chart says
  what a series is the sequel of. A person's page adds who they are as
  TMDB has it: born when and where, known for what, a biography in the
  reader's language and a few portraits — kept with their record, and asked
  again once a month.
- **The other orders, and a lamp.** Where TheTVDB numbers a series another
  way — its DVDs, straight through, an alternate or a regional order — the
  season page offers that numbering too (`GET /api/v1/items/{id}/orders`),
  without touching what Sonarr is served. And the interface has a light
  theme: the reader's choice, kept in the browser, or the system's.
- **A scrape, and the locks as a file.** `GET /api/v1/admin/metrics` is the
  server counting itself in the text Prometheus reads — requests by surface
  and status, calls upstream by provider and outcome with their latency,
  what the caches keep, what the catalogue holds, what the jobs did — for an
  administrator or a key with the `metrics` scope, a scraper's, which opens
  nothing else and is sent as `Authorization: Bearer`. Every edit made by
  hand can be downloaded as one
  JSON document and imported into another catalogue (`GET`/`POST
  /api/v1/admin/locks`), each lock finding its work of the same kind by the
  TMDB, TheTVDB or IMDb id it shares; a request takes two thousand locks at
  most, and the dashboard sends a longer file in parts.
- **Languages of the interface.** English and French ship complete; a
  third language is one file and a registry entry, may start partial — what
  it lacks is shown in English — and the build refuses a key English does
  not have. See [`frontend/src/lib/lang/README.md`](../frontend/src/lib/lang/README.md).
- **A command palette, and actions on many works at once.** ⌘K or Ctrl K
  anywhere finds a place to go or a work by its title; in the catalogue,
  whoever maintains it selects rows and refreshes, switches off or on,
  deletes, or adds them to a list, all at once — each work its own request
  and its own line in the audit trail.
- **What goes with a work.** A work's page offers the catalogue's own works
  in the same vein — its kind, sharing its genres, keywords, network or
  decade — and a film's whole collection, held or not, on a page of its own;
  whoever maintains the catalogue is shown what TMDB recommends beside a
  work, to import in one click.
- **Where to watch.** A work's page lists the services carrying it in a
  country — streaming, rent, buy — from the JustWatch data TMDB serves, named
  beside it as TMDB's terms ask; the country is a setting (`tmdb.watchRegion`,
  per key or address too), or the one the language of the answers names.
- **A page that installs, links that unfurl, answers that revalidate.** The
  site is a web app a phone installs; a link to a work or a list carries its
  title, line and poster for whoever unfurls it, while the catalogue is open;
  API answers carry a validator, so a client holding one is told "unchanged"
  rather than sent it again.
- **Calendars and feeds.** The schedule — episodes and film releases — and
  any one work, as an iCalendar a phone subscribes to; what arrives, airs
  and comes out as Atom feeds.
- **`.nfo` export with the artwork beside it**, in the layout Kodi defined and
  Plex's Personal Media agent reads: for Kodi, for Jellyfin, and for a Plex that
  does not call this server. A Plex Media Server 1.43 or later can take its
  metadata from this server directly instead, as a custom metadata provider
  (`/plex`), and Kometa can lay what is locked here over a Plex library
  (`/api/v1/export/kometa.yml`). See [Plex](plex.md) and
  [Exports](exports.md).
- **Two interfaces, in English or French.** A catalogue anyone can browse —
  posters, seasons, cast, the record — and an administration side for the
  people who maintain it. Both switch language from the bar and remember the
  choice.
- **A catalogue to read, not only to feed clients.** Every season and every
  episode has its page, every credited person a filmography of what is held
  here, and a schedule lists what airs and what comes out each week —
  episodes in the reader's own timezone, films on the day they come out.
  The list is filtered by genre, years, score, status, language and network
  — and, a sheet away, by a person credited, a keyword, a running time, the
  country a work was made in, the rating the reader's country gives it and the
  collection a film belongs to — and sorted by popularity, score, date, title,
  when a work arrived, running time, when a series last aired or how many
  episodes it holds — all of it kept in the address, so a view can be sent as
  a link — by the values each work's page shows, IMDb's latest score included
  when that list is on. What is applied heads the filters as chips, each taken
  off with a press, and the next page arrives as the end of the list comes
  near. A series' combined genres count as the film genres they stand for:
  "Action & Adventure" is found under Action and under Adventure, beside the
  films.
  The search lists what it finds as it is typed — `/` puts the cursor in it —
  and the front page opens on one of the most followed works, its own logo
  over its backdrop, a different one each day. Each identifier opens the
  work's page on the site it came from; the artwork opens in a viewer; a
  trailer plays from YouTube's no-cookie domain, and only once somebody
  presses play.
- **A search that ranks, and forgives.** A title is found by any name the
  work goes by — original, alternative, translated, locked by hand — whatever
  its accents, case and punctuation (`ete` finds *Été 85*, `greys` *Grey's
  Anatomy*, while `が` stays apart from `か`), and the people its credits name
  and its episodes' titles are found too. What is found comes in order: a
  name that is the term, then one that starts with it, then one with a word
  that does, then one that holds it, the better known first among those alike;
  a year at the end of a term (`dune 2021`) puts that year's work first. When
  the letters find nothing a term was typed to find, the names sharing most of
  its trigrams are offered — `breking bad` finds Breaking Bad. The command
  palette groups the works, the people (`@` alone looks for them) and the
  episodes (`S05E14 — Ozymandias`), each leading to its page; Sonarr's and
  Radarr's searches find the work held here by that very name before
  TMDB's. One index serves both engines, kept by the server as works are
  written ([The API](api.md#searching-the-catalogue)).
- **People, each a record of their own.** Everybody the credits name is one
  person, whatever the credits call them: a record with the ids TMDB, TheTVDB and
  IMDb file them under and the other names they go by, which every credit is filed
  under as it is written, and which a refresh leaves as somebody put it. A
  person's page opens on what they did — acting, directing, writing, producing,
  each with how many works — in the order of the years, the titles or the scores,
  films or series apart, a guest's episodes under their series with how many and
  which years; their name in their own script is marked as in its language for a
  screen reader. What TMDB says of them is asked the first time their page is read
  and kept a month. An index of everybody (`/people`) goes by letter and by job,
  the most credited first, and finds a name as the search does. TheTVDB's
  characters join TMDB's cast, matched by name, and each episode's guests, director
  and writers are credited on it rather than on the series. Whoever maintains the
  catalogue merges two records of one person, splits a homonym off another, gives
  a credit typed by name the person it is of, and enters somebody by hand with
  their ids ([The people](data.md#the-people), [The API](api.md#people)).
- **A season chart, past and to come.** One page for each quarter of the
  calendar — winter from January, spring from April, summer from July, autumn
  from October — as anime season charts have it, for series and films alike:
  the new series, the series back for another season, the films released and
  what carries on from before, each placed by its episodes' dates (corrected
  ones included) or its release. A strip of the season's weeks leads to each;
  kind, trailer, genre and original language narrow it, all kept in the
  address. For whoever maintains the catalogue, the most popular of what TMDB
  lists for the same quarter that the catalogue lacks follows, each a button
  away from being imported.
- **Maintenance where the problem is.** The editor is a record in five tabs
  — the fields, the seasons and their episodes, the artwork, the people and
  titles, the work elsewhere — and the address carries which is open. Season
  and episode fields are locked one by one from it, and it opens on the
  episode a public page came from; each provider's last answer can be read
  as it arrived; the works whose
  last refresh failed are one filter away, and flagged on the dashboard.
