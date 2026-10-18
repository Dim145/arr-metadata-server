# The site and the administration

The interface has two sides: a catalogue anyone allowed to may browse, and an
administration for the people who maintain it. This page covers how they are laid out
and how to move about them: the bar along the foot of a phone, the catalogue as a
list, the Settings page, and the Jobs page. What the administration's catalogue and
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
by hand, and a run somebody started says whose it was. A task that works through many works or
files — refreshing everything, storing the media, seeding from Sonarr and Radarr — can be
stopped, and stops after the work in hand.

| Task | Id | What it does |
|---|---|---|
| Refresh what is due | `refresh.sweep` | Works whose details have aged, asked of their sources again, a batch at a time |
| Refresh everything | `refresh.all` | Every work that has an identifier elsewhere, one after the other; can be stopped |
| Anime identifier list | `import.anime` | The list that ties AniList and MyAnimeList to TheTVDB and TMDB, downloaded whole |
| IMDb ratings | `import.imdb` | IMDb's ratings file, downloaded whole and kept for the works held here |
| NFO export | `export.nfo` | A `.nfo` document for every work; see [Exports](exports.md#nfo-documents-and-their-artwork) |
| Store the media | `media.store` | Every picture and theme not kept yet, fetched and kept; can be stopped |
| Sweep the media | `media.sweep` | The media nobody points at any more, forgotten and deleted |
| Renew the clients' certificate | `tls.renew` | The certificate of the clients' door, issued anew when fewer than thirty days remain |
| Check for a newer version | `update.check` | One anonymous request to GitHub, and only with `updates.check` on |
| Seed from Sonarr and Radarr | `arr.seed` | The catalogue filled from their libraries; see [Sonarr and Radarr](sonarr-radarr.md#filling-the-catalogue-from-their-library) |

A task that cannot run says why: its source is switched off, a variable is not set, no media
store is configured, or Sonarr and Radarr are not connected. Through the API,
`GET /api/v1/tasks` lists them and `POST /api/v1/tasks/{id}/run` starts one in the background
(`202`; `409` while it is running already).

**The history** is a page at a time — fifty runs and *Load more* — read under the day each ran
on (*Today*, *Yesterday*, then the date, in the browser's own time zone), with a day's failures
counted beside its name. It can be narrowed by task, by outcome and by who started the run.
*Failures only* at its head asks for the failed runs alone, and a history with nothing failed
says so. While the newest page is on screen it is refreshed every 20 seconds, every 3 while a
task runs; once older pages are loaded it stops, since a refresh would ask for every one of
them. The API is `GET /api/v1/jobs?limit=50&offset=N`, with `status` (`running`, `succeeded` or
`failed`), `kind` and `by` as filters; `total` says how many runs match.

## The dashboard

The administration opens on a dashboard that says what the server is, holds, keeps and did last,
and opens on **what there is to do**: works to put right, keys about to lapse, pictures given up
on. It is described with the catalogue's register that its lines lead into, in
[Editing](editing.md#what-there-is-to-do).
