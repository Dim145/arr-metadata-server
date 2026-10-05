# Changelog

Every release of arr-metadata-server, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
[Semantic Versioning](https://semver.org/): while the major is 0, a minor
may change what the API or the configuration means, and says so here.

## [Unreleased]

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

[Unreleased]: https://github.com/Dim145/arr-metadata-server/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Dim145/arr-metadata-server/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Dim145/arr-metadata-server/releases/tag/v0.1.0
