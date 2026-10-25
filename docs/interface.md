# The site and the administration

The interface has two sides: a catalogue anyone allowed to may browse, and an
administration for the people who maintain it. This page covers how they are laid out
and how to move about them: the bar along the foot of a phone, the catalogue as a
list, the pages of a genre, a network and a year, two works side by side, the
administration's tables, its palette and its keys, the Settings page, and the Jobs page.
What the administration's catalogue and
dashboard do is in [Editing](editing.md).

## On a phone

The site's places are along the foot of the screen, where a thumb rests: **Home**,
**Browse**, **Schedule**, **Seasons**, and a fifth, **More**. The place you are in is
marked by a rule as well as a colour, and exposed to a screen reader as the current
page; *More* is marked when you are in one of the places it holds. **More** opens a sheet
with what the four leave out: the collections, the selections (only while there is one to
show), the catalogue in numbers, the light or dark theme, the language, and the account —
or the way in for somebody who has not signed in, and the way out for somebody who has.

From a tablet's width up the same places are the tabs of the bar at the top, with the
catalogue's shortcuts hanging from the Browse tab, and the theme and the language beside
the search. The bar along the foot leaves its own height free at the end of every page,
the footer included, and honours the device's safe area where the browser reports one.

The administration has had such a bar of its own already.

Two smaller things belong to a phone too. The rows that scroll sideways — a work's
seasons, a person's portraits and, on a phone, the related, similar and front-page rows —
fade at the edge where they go on, with two arrows to walk them. And a running series says
on its plate what airs next — the episode, its day, how far off — and how far its season has
got.

## The catalogue as a list

**Browse** shows its results as a grid of posters, to know a work by its cover, or as a
list, to read and compare. The switch sits beside the order. A row is a small poster, the
title, the year and whether it is a series or a film, the score, the status, up to three
genres and two lines of synopsis. The title is the row's only link, and the whole row is its
target. *Load more* is the same button in both views.

The list is `?view=list` in the address, so a view can be sent as a link; `?view=grid` is the
grid. What the reader chose last is kept in the browser, in `localStorage` under
`ams.browse.view`, and used for an address that says neither. An address that says one wins.
Clearing the filters never changes the view, and storage that cannot be read means the grid.
The browser skips a row until it is near the screen (`content-visibility: auto`), so a list of
a thousand works scrolls as lightly as a page of thirty.

## A genre, a network, a year

A genre, a network or a studio, a year and a decade each have a page: `/genre/drama`,
`/network/hbo`, `/year/1999`, `/decade/1990`. The genres, the network and the year a work's page
shows lead there. A page opens on its name — a genre's over a band in the colour its chips wear —
and how many works it holds that you may see; then the programmer's note on it, where somebody
wrote one (see [Editing](editing.md#the-programmers-note)); the five works rated highest; and then
every one of them in the catalogue's own list, with **Browse**'s filters, orders and views and the
page's subject always applied, never offered as a choice. A year's and a decade's page lead to the
ones on either side, and a year's to its decade.

A genre is the same genre whatever language its works were fetched in (see
[Genres](data.md#genres)): its page answers to any name it goes by — `/genre/drame` becomes
`/genre/drama` — and is named in the reader's language where the server knows it. A network's page
is the slug of its name, `canal` for *Canal+*; a decade's answers to any of its years, which
becomes its first. A link to one of these pages carries a preview of it — its name, its note or how
many works it holds, and the poster of the one rated highest — whenever a reader with no credential
may see the catalogue, as a work's and a selection's do.

## Two works side by side

`/compare?a=…&b=…` puts two works side by side: their scores source by source, their years, how
long they run, how many seasons and episodes a series has, their networks or studios and their
genres, and the people both credit — those who made them first, then by their billing — with what
each did on each. Either work is chosen, or changed, by searching the catalogue in its column; the
address keeps both, so a comparison can be sent as a link.

## The administration's tables

Every register of the administration — the catalogue, the members, the keys, the audit
trail, the selections, the archives, what was sent to Sonarr and Radarr, the callers seen,
the invitations, a destination's deliveries, the runs of the tasks, the media by work and those
given up on — is one table, drawn the same way:

- **Its head stays at the top** while the rows scroll under it. Where the table is wider
  than its frame, the frame scrolls both ways and the head stays at its own top.
- **A column whose head is a button orders the rows**; pressed again, the other way. The
  head says which way to a screen reader (`aria-sort`). The catalogue's order is the
  server's, and lives in the address; the media by work are ordered by the server too, the
  most first; the other complete lists are ordered where they are drawn. A history read a
  page at a time — the audit trail, the deliveries, the sends, the runs of the tasks — keeps
  its own order: one page put in another order would say nothing of the others.
- **Columns and density**, the button beside each table: a box per column that can be
  hidden, two densities (*Comfortable*, *Compact*), and the way back to the table's own.
  What is chosen is kept with the account, so it follows its person from one browser to the
  next; without an account (a trusted address, authentication switched off) it is kept in
  the browser. A column added by a later version shows until somebody hides it.
- **Under 600 pixels of its own width**, a table is a column of cards: the row's name and
  its box at the head of each card, its actions at the end of that line, and each value
  after its column's name. It is the table's width that decides, not the screen's: a narrow
  panel beside a wide one gets cards too. A column a phone can do without (the catalogue's
  identifiers, a key's prefix) stays off the cards.
- **The rows are walked one at a time with the keyboard**: Tab lands on one row, the arrow
  keys move to the next or the previous, Home and End to the first and the last; Enter opens
  the row where a row opens something (a work's editor, a member's page, what a run did), and
  the space bar ticks its box.

## The administration's palette and keys

⌘K, or Ctrl K, opens the palette everywhere in the administration. Besides the places and
the search, it offers what can be done where it was opened:

- in the **catalogue**, to the works selected, or else to the row the focus is on: open its
  editor or its public page, refresh it, lock its overview as it is, enable or disable it,
  add it to a selection; and, for the catalogue itself, search it, start a new entry, select
  every work shown, or open one of its views;
- in a **work's editor**: save the changes not saved yet, and save then open the next work
  of the catalogue's list; refresh from the sources (the review first), lock or unlock the
  overview, enable or disable the work, add it to a selection, show the preview, open the
  public page, or the previous and next works;
- everywhere, for an administrator, **Run a task…**, which lists the tasks — those that
  cannot run now greyed out, with the reason — and, once one has started, opens the Tasks
  page, where it is followed.

A command that asks for a choice — which selection, which task — opens a page of its own in
the palette; Backspace in the empty field comes back. Each command shows its keys, which
work without the palette, though not while something is being typed unless ⌘ or Ctrl is
held:

| Keys | Where | What |
|---|---|---|
| `g` then `d`, `c`, `p`, `i`, `l` | everywhere | the dashboard, the catalogue, the people, the import, the selections |
| `g` then `t`, `m`, `b`, `a`, `n`, `u`, `k`, `s` | everywhere, an administrator | the tasks, the media, the backups, the audit trail, the notifications, the members, the keys, the settings |
| `/` | the catalogue | the search |
| `n` | the catalogue | a new entry |
| `r` | the catalogue | refresh the selection, or the row the focus is on |
| `r` | an editor | refresh from the sources, after the review |
| `p` | an editor | the preview |
| `j`, `k` | an editor | the next and the previous work of the catalogue's list |
| ⌘S or Ctrl S | an editor | save the changes, and stay |
| ⇧⌘↵ or Ctrl ⇧ ↵ | an editor | save the changes, and open the next work |

## The server log

*Administration › Server log* (`/admin/logs`, for administrators) shows the last two thousand
lines the server wrote, every secret in them masked: by level and by words, followed as they come,
the lines of one request a press away, downloaded as text or as JSON lines. The sidebar and the
palette lead there. [Monitoring](monitoring/README.md) says how the same lines reach
Loki, and the traces Tempo.

## The Settings page

*Administration › Access › Settings* (`/admin/settings`, for administrators) lists the server's
settings — every one is described in the [settings reference](settings.md) — apart from those
that have a page of their own: signing in, the APIs and whose key the
relays ask with are on **Opening & APIs**, the order of the sources on **Sources**, the media
store on **Media**, the cache's spaces on **Cache**, Sonarr's and Radarr's connections on
**Sonarr & Radarr**, and the destinations of notifications on **Notifications**.

- **Search.** A setting is found by its title, its key, a word of what explains it or the name
  of its group; every word typed has to be found. Accents and the ligatures œ and æ are folded:
  *oeuvres* finds *œuvres*. The index of the page follows what is left.
- **Modified.** A setting changed from what it began as is marked *Modified*, says what that
  was (*Default: …*), and goes back to it in one press, *Back to the default*. *Changed only*
  lists those alone. What a setting began as is the value the environment or the code gave it
  when the server started. `GET /api/v1/settings/server/-` carries it as `default`; it is absent
  for a secret and for a setting that begins unset, which counts as modified once it is set, and
  whose reversal is to set nothing.
- **How it was started.** What the server was started with — its version, its database, its
  public address, whether a TMDB key is configured, the policy at each door — is a section of its
  own at the foot of the page. It is read from the environment once, and cannot be changed from
  there: changing it means changing its variable and restarting.

## The Jobs page

*Administration › System › Jobs* (`/admin/jobs`, for administrators; some labels call it the
Tasks page) lists the **tasks** the server runs on its own and the history of their runs.

Each task shows when it runs, how it last went, and when it runs next. Any of them can be run
by hand — *Run now*, or *Run again* once it has run — and a run somebody started says whose it
was. *Log* opens what the run under way, or the last one, did: each work it took, and how that
went. A task that works through many works or files — refreshing everything, storing the media,
seeding from Sonarr and Radarr, a batch, the import rules — can be cancelled, and stops after
the work in hand.

| Task | Id | What it does |
|---|---|---|
| Refresh what is due | `refresh.sweep` | Works whose details have aged, asked of their sources again, a batch at a time |
| Refresh everything | `refresh.all` | Every work that has an identifier elsewhere, one after the other; can be stopped |
| Anime identifier list | `import.anime` | The list that ties AniList and MyAnimeList to TheTVDB and TMDB, downloaded whole |
| IMDb ratings | `import.imdb` | IMDb's ratings file, downloaded whole and kept for the works held here |
| Import a batch | `import.batch` | The works of a batch started on the *Import* page, one after the other; see [Importing many works at once](editing.md#importing-many-works-at-once) |
| Import rules | `import.rules` | What the import rules bring in, each on its own schedule; see [Import rules](exports.md#filled-on-its-own-import-rules) |
| NFO export | `export.nfo` | A `.nfo` document for every work; see [Exports](exports.md#nfo-documents-and-their-artwork) |
| Store the media | `media.store` | Every picture and theme not kept yet, fetched and kept; can be stopped |
| Sweep the media | `media.sweep` | The media nobody points at any more, forgotten and deleted |
| Prepare the pictures | `media.variants` | The pictures kept before there were copies, prepared; by hand only, see [Media](media.md#preparing-what-was-kept-before) |
| Make the AVIF copies | `media.avif` | The posters' and backgrounds' AVIF copies, at night once switched on; see [Media](media.md#avif) |
| Renew the clients' certificate | `tls.renew` | The certificate of the clients' door, issued anew when fewer than thirty days remain |
| Check for a newer version | `update.check` | One anonymous request to GitHub, and only with `updates.check` on |
| Seed from Sonarr and Radarr | `arr.seed` | The catalogue filled from their libraries; see [Sonarr and Radarr](sonarr-radarr.md#filling-the-catalogue-from-their-library) |
| Back up the database | `backup.run` | An archive of the database, on the schedule *Backups* sets; run by hand, one kept until deleted |
| Rebuild the search index | `search.rebuild` | Every name a work, a person or an episode is found by, written again from the catalogue |
| Keep the catalogue's order current | `listing.relist` | The values works are sorted and filtered by, written again where they are behind — after an upgrade, after IMDb's daily list; every half minute, and a look that finds nothing behind leaves no run |

A task that cannot run says why: its source is switched off, a variable is not set, no media
store is configured, Sonarr and Radarr are not connected, or no import rule is switched on.
Through the API, `GET /api/v1/tasks` lists them and `POST /api/v1/tasks/{id}/run` starts one in
the background (`202`; `409` while it is running already). A batch is started from the *Import*
page alone, with its lines.

### Followed as they go

The page follows the tasks live: a run that starts, says how far it has got, or ends is on its
card within a second or two, with a bar that fills as it works — or that only says it is
working, for a run that does not know how many there are. It is the same whichever instance
makes the run ([Several instances](instances.md)): what the page is sent is read from the
history every instance writes and the holds they share, never from one process's memory. A line
under the title says how it is followed: *Followed as it goes*; *Asked for every few seconds*,
where the stream cannot be opened — a proxy that holds an answer back until it is whole, which
`X-Accel-Buffering: no` asks nginx not to; or *Paused while this tab is hidden*, when it is not
followed at all, and neither streamed nor asked for.

The stream is `GET /api/v1/admin/tasks/stream`: server-sent events named `tasks`, each the
whole board — every task as `GET /api/v1/tasks` gives it, and `newestRun` — sent at once and
then whenever it changes. It ends after five minutes and the browser opens another, which asks
for the credential again; a session that ended is followed no longer than that.

### When it runs

The sweep, the two lists downloaded whole and the archive run on a cadence of their own. *When
it runs* on their card keeps that cadence to **a window of the day** — `01:00` to `06:00`: a run
that comes due outside it waits for the window to open; an end before the start crosses
midnight — or replaces it with **the moments a cron expression names** — `0 4 * * 0`, Sundays at
four. An expression is five fields, minute, hour, day of the month, month and day of the week,
each `*`, a number, a range, a list or a step (`*/15`), with the English names of months and
days and `@hourly`, `@daily`, `@weekly`, `@monthly`, `@yearly` for theirs; when both day fields
are set, a day that matches either is one. Both are in UTC, the time the server keeps: the
dialog says what a window is where you are, and lists the next three runs in your own time as
you write. A moment missed while the server was down is made up once it is back, and after a
failure the task's own wait before trying again comes first. The other tasks keep their own
cadence.

Through the API: `PUT /api/v1/tasks/{id}/schedule` with `{"kind": "window", "from": "01:00",
"to": "06:00"}` or `{"kind": "cron", "cron": "0 4 * * 0"}`, `DELETE` on the same path to give the
task its own cadence back, and `POST /api/v1/tasks/schedules/preview` with the same body for the
next three moments — or a `400` that says why an expression names none.

**The history** is a page at a time — fifty runs and *Load more* — read under the day each ran
on (*Today*, *Yesterday*, then the date, in the browser's own time zone), with a day's failures
counted beside its name. Each run is a row of the
[administration's tables](#the-administrations-tables): its name, or Enter on the row, opens
what it did under it. It can be narrowed by task, by outcome and by who started the run.
*Failures only* at its head asks for the failed runs alone, and a history with nothing failed
says so. While the newest page is on screen it is read again whenever the stream says a run
started or ended — or, where it is asked for instead, every 20 seconds, every 3 while a task
runs; once older pages are loaded it stops, since a refresh would ask for every one of them. The
API is `GET /api/v1/jobs?limit=50&offset=N`, with `status` (`running`, `succeeded` or
`failed`), `kind` and `by` as filters; `total` says how many runs match.

## The dashboard

The administration opens on a dashboard that says what the server is, holds, keeps and did last,
and opens on **what there is to do**: works to put right, keys about to lapse, pictures given up
on. With a media store, a tile says how many media are kept, what they and their copies weigh,
how many were given up on — with, when any were, the way to the works they were for, the
catalogue narrowed to them (`?missing=mediaFailed`) — and which three works weigh most (see
[Media](media.md)). It is described with the catalogue's register that its lines lead into, in
[Editing](editing.md#what-there-is-to-do).
