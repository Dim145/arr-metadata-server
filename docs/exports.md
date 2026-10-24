# Exports: Kometa, selections, feeds and `.nfo` documents

This server hands its catalogue to other tools in files and feeds. Four exports are
documented here:

- **Kometa's metadata file**, for a Plex library run by [Kometa](https://kometa.wiki):
  what has been locked here, to lay over what Plex's own agent found;
- **a selection as a list**, as a text file of identifiers, an Atom feed or one of
  the import lists Sonarr and Radarr read;
- **calendars and feeds**: the schedule and a work's dates for a calendar app, and what
  arrives, airs and comes out for a feed reader;
- **`.nfo` documents with their artwork**, for Plex, Kodi and Jellyfin.

For a Plex Media Server that can call this server, the live route is the
[Plex provider](plex.md); the `.nfo` export and Kometa's file serve the Plex that
cannot, and every other player.

## A Kometa metadata file

`GET /api/v1/export/kometa.yml?kind=movie` (or `kind=series`) writes what is locked
here as a Kometa metadata file: an entry for each work of that kind with something
locked, carrying only what was locked — never what a provider said, which Plex has
already. A library that applies it shows the catalogue's corrections on every Kometa
run, whatever Plex's agent finds.

A metadata file belongs to one library, and a library holds one kind, so `kind` is
required. Settings → *Plex and other .nfo readers* → *Kometa metadata files* offers
the file as a download, one button for the films and one for the series, and says so
rather than hand over a file with nothing in it. Kometa reads it by address as well:

```yaml
libraries:
  Films:
    metadata_files:
      - url: https://catalogue.example/api/v1/export/kometa.yml?kind=movie&apikey=<key>
  Series:
    metadata_files:
      - url: https://catalogue.example/api/v1/export/kometa.yml?kind=series&apikey=<key>
```

**The key.** The file is read with the key of an editor or an administrator: one
limited to the `read` scope will do, since Kometa only reads, so the address given
to Kometa need not carry a key that could edit the catalogue. A key that belongs to
nobody — one of the server's — needs the `write` scope, and a member's key has no
such reading to lend. The key travels as `apikey` (or `api_key`) in the query,
which Kometa's address can hold, or in `X-Api-Key` or as a bearer token.

**The language.** `language=` (`fr` or `fra`) asks for the texts locked for a library
in another language than the catalogue's: those locked for that language, and those
locked for every language, are written; those locked for another are not. Left out,
it is the language the catalogue is kept in.

What is written, for each work that has something locked:

| Locked here | Written as |
|---|---|
| title, sort title, original title | `title`, `sort_title`, `original_title` |
| overview, tagline | `summary`, `tagline` |
| first aired (a series), in cinemas (a film) | `originally_available` |
| content rating | `content_rating` |
| studio (a film), network or studio (a series) | `studio` |
| genres | `genre.sync`: the whole list, not an addition to Plex's |
| chosen poster, chosen background | `url_poster`, `url_background` |
| a season's title, overview, chosen poster | `seasons: N:` then `title`, `summary`, `url_poster` |
| an episode's title, overview, air date | `seasons: N: episodes: M:` then `title`, `summary`, `originally_available` |

Each entry says how to find the work:

```yaml
metadata:
  "Heat (1995)":
    match:
      mapping_id: 949
      title: "Heat"
      year: 1995
    title: "Heat — the director's cut"
    genre.sync:
      - "Crime"
      - "Drama"
```

`mapping_id` is TMDB's id for a film and TheTVDB's for a series, or IMDb's
(`"tt0113277"`) where the work has not that one; a work with none of them is matched
by its title and year alone. Every string is written double-quoted and escaped, so a
title with a colon, quotes, a line break or an emoji reads back as it was. A field
locked cleared writes nothing, since Kometa has no way to say "no value" that does
not also fail the entry. A work with no correction is not in the file.

A picture is an address Kometa can fetch: a copy the catalogue keeps is named under
`AMS_PUBLIC_URL`, so a picture that was *uploaded* needs it set. The answer is never
kept by a cache (`Cache-Control: no-store`), and its `X-AMS-Works` header says how
many works it speaks of.

## A selection as a list

A selection — what *Lists* composes in the administration — is read as its page is:
open under public browsing, otherwise with a key (`?apikey=…`). A private selection
answers only an editor or an administrator, in person or through a key of theirs that
only reads, and is no list at all to anybody else. These are the shapes it is served
in, under `/api/v1/lists/{slug or id}/`:

| Address | For | Shape |
|---|---|---|
| `sonarr.json` | Sonarr, *Custom List* | `[{ "title", "tvdbId" }]`, the series |
| `radarr.json` | Radarr, *Custom Lists* | `[{ "id", "title" }]`, the films, `id` being the TMDB id |
| `stevenlu.json` | Radarr, *StevenLu Custom* | `[{ "title", "imdb_id" }]`, the films |
| `ids.txt` | Kometa, the *Text File* builder | a work a line |
| `ids.txt?kind=movie` or `?kind=series` | one Kometa library | only the films, or only the series |
| `feed.atom` | a feed reader | Atom, the works that joined the selection most recently first |

The first three are described with how a selection is composed, just below. The addresses
of all of them are on the selection's page and in its editor, with a button to copy each.

### Composing a selection, and importing it

Whoever maintains the catalogue composes lists under *Lists* in the
administration: a name, a description, what the list holds (series, films or
both), and either its members by hand, in an order, or a filter — genres,
keyword, years, score, status, language, network, order, how many — evaluated
whenever the list is read. Each is a page on the site, under *Selections*,
unless it is marked private; a work's own page says which hand-made
selections hold it.

Each list is also served in the shapes the clients import, so a selection
made here becomes what those clients add on their own:

| Client | Import list to add | Address | Shape |
|---|---|---|---|
| Sonarr | Settings → Import Lists → *Custom List* | `/api/v1/lists/{slug}/sonarr.json` | `[{ "title", "tvdbId" }]` — the series with a TheTVDB id |
| Radarr | Settings → Lists → *Custom Lists* | `/api/v1/lists/{slug}/radarr.json` | `[{ "id", "title" }]` — the films with a TMDB id, `id` being it |
| Radarr | Settings → Lists → *StevenLu Custom* | `/api/v1/lists/{slug}/stevenlu.json` | `[{ "title", "imdb_id" }]` — the films with an IMDb id |

The addresses are on each list's page, with a button to copy them. They are
read under the same rule as the page: open under public browsing, otherwise
with a key appended (`?apikey=…` — that spelling, which Sonarr's and Radarr's
logs redact; `api_key` works too but would be logged). A private list answers
only whoever maintains the catalogue: an editor or an administrator, in person
or through a key of theirs — a key limited to the `read` scope will do, so
importing one asks for no key that could edit the catalogue — and a key that
belongs to nobody, only with the `write` scope. Make the list public instead
where neither is wanted. The whole list API is under `/api/v1/lists` and in
the OpenAPI document.

### The text file for Kometa

A line is one work: `tmdb:949` for a film, `tvdb:348303` for a series, a bare `tt…` for
an IMDb id, and the title after a `#` for whoever reads the file. The lines follow the
selection's own order, and the file begins with comments, which Kometa skips.

```
# Autumn 2026
# For Kometa's Text File builder: tmdb: is a film's TMDB id, tvdb: a series' TheTVDB id, tt… an IMDb id.
tmdb:949  # Heat (1995)
tmdb:603  # The Matrix (1999)
```

Kometa reads a bare number as a TMDB id in a movie library and as a TheTVDB id in a
show library, and the two kinds' numbers are not each other's. That is why each line
says which kind of id it is, and why a selection that holds films and series is asked
for once per library, with `?kind=movie` or `?kind=series`. A film is named by TMDB's
id, then by IMDb's; a series by TheTVDB's, then IMDb's, then TMDB's. A work none of
them names is left out. `kind` is `movie` or `series`: another value is a `400`.

```yaml
collections:
  Autumn 2026:
    text_file: https://catalogue.example/api/v1/lists/autumn-2026/ids.txt?kind=movie
```

### The Atom feed

`feed.atom` lists the selection's works as a feed, the latest added first. Each is
dated by the day it joined the selection — or, for a selection composed by a filter,
whose members are whatever matches and have no such day, by the day the catalogue
received it — and a work added to a hand-made selection keeps that day through every
later save. `?language=` asks for the titles in another language. An entry's identifier
names the selection as well as the work, so that a work in two selections is two
entries.

### A cover

A selection has a **cover**, chosen in the administration — *Lists*, a list,
*Cover* — among the backgrounds of its works or by any picture's address, with the
dialog every other picture is chosen with. It is shown above the selection on the
selections' page, at the head of its own page, and as its preview where the page is
shared.

Through the API, `cover` on `POST /api/v1/lists` and `PUT /api/v1/lists/{key}` takes an
`https://` address of at most 2048 characters, or the `upload:` origin of a picture
sent up; empty, it takes the cover away; left out, it stays. An `http://` address is
refused with a `400` and says why: the site loads no picture over `http`.

## Calendars and feeds

The schedule — episodes and film releases — and one work's dates, as
calendars a phone or a desktop subscribes to; what arrives, airs and comes
out as Atom feeds a reader follows.
Read under the same rule as the pages they mirror: open under public
browsing, otherwise with a key, which a calendar app carries in the address
(`?apikey=…`).

| Feed | Path |
|---|---|
| The schedule: seven days back and twenty-eight ahead unless asked otherwise, at most sixty-two together | `/api/v1/calendar.ics?pastDays=7&futureDays=28&language=fr` |
| One work: a series' dated episodes, or a film's releases, a day each | `/api/v1/items/{id}/calendar.ics` |
| The fifty works most recently added | `/api/v1/feed/added.atom` |
| The episodes airing and the films coming out from yesterday to a week ahead | `/api/v1/feed/airing.atom` |

An episode whose time a provider knew is a timed event, as long as its runtime;
one with only a date is an all-day event on it. Under public browsing the
interface offers them — *Subscribe* on the schedule, *Add to my calendar* on a
work, the feeds in the footer — as `webcal://` addresses where a calendar app
expects one; otherwise it does not, since the app would arrive without a
credential, and the addresses take a key as above. Links in a feed point at
`AMS_PUBLIC_URL` when it is set — set it behind a [reverse proxy](reverse-proxy.md) — otherwise at
the host and scheme the request came to, as the proxy forwarded them if it is
among `AMS_TRUSTED_PROXIES`, or as the socket saw them. A window holding more
episodes or releases than one answer carries is cut short and says so in an
`X-AMS-Truncated` header.

## `.nfo` documents and their artwork

`POST /api/v1/export/nfo` writes a `.nfo` document for every work, and the pictures
beside it, under `AMS_NFO_EXPORT_PATH`: in the layout Kodi defined, which Plex's
Personal Media agent and Jellyfin read. The export is also on the Settings page, under
*Plex and other .nfo readers*, and the run is listed on the **Jobs** page as
`export.nfo`.

```
series/breaking-bad-2008/tvshow.nfo
series/breaking-bad-2008/Season 01/S01E01.nfo
movies/arrival-2016/movie.nfo
```

The layout is this server's own: nothing here can know how your library is laid out,
so the files land somewhere predictable, for you to copy or link next to your media.
Re-running the export rewrites the documents and leaves existing pictures alone.
`AMS_NFO_EXPORT_ARTWORK=false` writes documents only. The call needs the right to write.
It answers at once with `202` and the job it started, since a library's artwork is
thousands of downloads; a second export asked for while one is running is refused with
`409`, and with `AMS_NFO_EXPORT_PATH` unset it is refused with `503`
(`export_not_configured`).

One work's document alone is `GET /api/v1/items/{id}/nfo`, and one episode's
`GET /api/v1/items/{id}/nfo/{season}/{episode}`.

### One selection's works

`POST /api/v1/export/nfo?list=<slug or id>` writes the documents and pictures of that
selection's works alone — those switched on and not for adults, as the whole export
has them. The selection's editor has a button for it, *Write the documents*. It suits a
Plex or Kodi library that holds those works alone.

### What a document carries

- `<uniqueid type="tmdb|tvdb|imdb|fankai">`, and **exactly one** of them marked
  `default="true"`: the identifier Plex's NFO agent builds the work's identity from. It
  is TheTVDB's for a series, then TMDB's, then Fankai's; TMDB's for a film, then IMDb's;
  and the first the work has where none of those is known. An episode's is TheTVDB's,
  then TMDB's.
- `<tagline>`, a `<country>` for each country the work was made in (by its English
  name) and a `<studio>` for each company that made it.
- For a film that belongs to a TMDB collection, `<set>` with the collection's name and
  what it is about, as Kodi reads it, so that a library groups its films. Nothing is
  written where TMDB cannot be asked.
- For a season with a title of its own — not the *Season 2* a provider falls back on —
  a `<namedseason number="N">` in the series' document, and for a season's poster
  `<thumb aspect="poster" type="season" season="N">`.
- The usual `<thumb>`, `<fanart>` and `<actor>`. U+FFFE and U+FFFF are left out of every
  document, since one made the whole document unreadable to a strict XML reader.
- Each `<actor>` with the ids the person's record is known by — `<tmdbid>`, and
  `<imdbid>` once TMDB has been asked about them (their page read) — which Jellyfin and
  Emby file the person under. A credit TMDB names its person on gives its TMDB id even
  where nobody has looked them up; a credit by name alone filed under somebody TMDB
  knows gives theirs (see [The people](data.md#the-people)).
- For a series with an order frozen as its aired numbering (see
  [Orders](sonarr-radarr.md#orders)), its episodes' documents — and the files they are
  written to — are numbered by that order, as Sonarr then names the files, and the
  series' document names the order to Jellyfin in a `<displayorder>`: TheTVDB's type
  for one of TheTVDB's (`dvd`, `alternate`, `altdvd`…), which its TheTVDB plugin
  reads; for a TMDB episode group, the order its TMDb provider maps to the group's type
  (`originalAirDate`, `absolute`, `dvd`, `digital`, `storyArc`, `production`, `tv`),
  with the group itself beside it as `<uniqueid type="TmdbEpisodeGroupId">`. An order
  made here is no order Jellyfin knows: nothing names it, and the numbers carry it.
- For a special, `<displayseason>` and `<displayepisode>`: where Kodi shows it among
  the regular seasons — the season, and the episode it comes before. TheTVDB's own
  word on where it aired first; else where the first other order that places it among
  the seasons puts it, the DVDs' first, the episode it comes before there named by its
  numbers here; at the end of a season, the season alone.

### Pictures

The pictures are written beside the documents under the names both programs read. Where
the two differ, the picture is written once, under Kodi's name, and the second name is a
hard link to it: the same bytes on the disk. A disk that cannot link, a network share or
FAT, is given a copy.

| | Kodi | Plex (local assets, read by its NFO agent) | Written |
|---|---|---|---|
| poster, background, logo | `poster.jpg`, `fanart.jpg`, `clearlogo.png` | the same | once |
| a season's poster | `season01-poster.jpg` | `season01.jpg` | `season01-poster.jpg` and a hard link `season01.jpg` |
| the specials' poster | `season-specials-poster.jpg` | the same | once |
| an episode's still | `S01E01-thumb.jpg` | `S01E01.jpg` | `S01E01-thumb.jpg` and a hard link `S01E01.jpg` |
| banner, clear art, landscape | `banner.jpg`, `clearart.png`, `landscape.jpg` | not read | once |
| key art | `keyart.jpg` | not read | once |
| disc art | `discart.png` | not read | `discart.png` and a hard link `disc.png` |
| character art | `characterart.png` | not read | once |

Key art is a poster without its lettering, which Kodi and tinyMediaManager show where a
title is written beside it: a textless poster from Fanart.tv (filed under no language,
`00`), or one put on by hand as key art. Disc art is Fanart.tv's `moviedisc`; Emby
reads it as `disc.png`, which Jellyfin reads too. Character art is Fanart.tv's
`characterart`: one character of a series cut out, where clear art carries the title
too. None of the three is named in a document: Kodi reads them from the files, and
Jellyfin takes any `<thumb>` it does not know for the work's poster. Each episode has a document of its own, as Kodi 22 requires: a file
holding several `<episodedetails>` is no longer read.

Cast photographs go in `.actors/`, and a Fan-Kai's theme beside the show as `theme.mp3`.
The pictures are fetched as the media store fetches them, since the addresses are
whatever somebody who may edit put on a work: never from this machine, a cloud's
metadata service or another address only the server can reach, every redirect judged
on its way, never through `HTTP_PROXY`. A picture mirror on your own network is
reached; `AMS_MEDIA_PRIVATE_NETWORKS=false` keeps the export and the media store out of
private networks too. For the background of the layout and the Fan-Kai documents, see
[What about Plex?](integration.md#what-about-plex).
