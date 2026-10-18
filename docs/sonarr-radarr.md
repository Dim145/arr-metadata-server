# Sonarr and Radarr

[Pointing your stack at this server](integration.md) makes Sonarr and Radarr ask
this server for their metadata, and this server answer. This page covers what
they are given besides the basics — Sonarr v5's edition of the Skyhook API, and
what Radarr is sent — and the other direction, which is optional: connected to
your own Sonarr and Radarr, this server also asks them. It tells them when a work
changes here, it reads their libraries to fill the catalogue before they are
pointed at it, and it says in the audit trail when TheTVDB renumbers episodes
under them.

## What they are given

### Sonarr v4

Sonarr v4 asks `skyhook.sonarr.tv` under `/v1/`: `/v1/tvdb/shows/{language}/{id}`
and `/v1/tvdb/search/{language}`. Its request builder pins the language to `en`
whatever its own interface says: see *Languages* in the
[Languages](languages.md) for what it is answered in, and
[A note on languages](integration.md#a-note-on-languages).

### Sonarr v5

Sonarr v5 reads the second edition of the Skyhook API, under `/v2/`:
`/v2/tvdb/shows/{language}/{id}/{seasonType}` and `/v2/tvdb/search`. Both are
answered from the same catalogue as the first edition, which Sonarr v4 keeps
reading: the same series, with what v5 reads besides.

- **Languages.** Every language the series is named in travels with it
  (`translations`), its own among them — where Sonarr takes the original title
  from. A language whose translation has an overview and no title, as TMDB leaves
  the name out where it is the one the series already goes by, is listed under the
  title the answer gives.
- **Orders.** `seasonTypes` lists the orders the episodes come in: the aired
  order (`official`), then the DVD, absolute, alternate and regional orders
  TheTVDB keeps for the series. Asked in an order the series has, the episodes are
  numbered by it; asked in one it has not, they are numbered as they aired, as
  Skyhook answers.
- **Seasons.** A season's title and year. A title that only says the season's
  number is kept back, as the real service keeps it.
- **Episodes.** An episode's ids under both names the service and the client
  use (`tvdbId` and `metadataId`).

The language is three letters, as Sonarr writes it (`eng`, `fra`). The series is
answered in it when the catalogue holds it so; otherwise in the caller's own
`tmdb.language`, then in English, then as it is stored. A code that names no
language is read as `eng`. A title or an overview locked for one language alone is
what a Sonarr asking in that language is given, and a title locked for every
language is every language's name for the work in `translations`; the original
title stays the one the work was made under, unless the original title is locked
too. See [Editing](editing.md#locking-a-text-for-one-language).

Sonarr v5 is a pre-release: this was checked against 5.0.0.2635
([Verified against](integration.md#verified-against)).
[`scripts/e2e-skyhook-v2.sh`](../scripts/e2e-skyhook-v2.sh) sets the real
service's second edition beside this server's for a few real series and tells
every difference apart as an addition or a failure.

### Scene mappings

Sonarr recognises a release by the titles of the list it downloads from
`services.sonarr.tv`. Each mapping this server adds to that list carries a
`mappingId`, `ams-` and twenty-four hexadecimal digits, the same for the same
series and title on every answer: Sonarr v5 keeps its list by that field and
skips a mapping without one. Sonarr v4 reads no such field. See
[Sonarr's alternate titles](integration.md#sonarrs-alternate-titles).

### Radarr

Radarr is sent, on `/v1/movie/{id}`, `/v1/movie/imdb/{id}` and `/v1/movie/bulk`:

- the film's **collection**, by id and name (`Collection`), and the films
  **recommended** with it, by id and title (`Recommendations`), twenty at most.
  Both are read from what the catalogue holds and never fetched for. Radarr's
  collections fill, and its Discover page has something to offer;
- every country's **certification** (`Certifications`), as `api.radarr.video`
  sends them; Radarr takes the one of the country it is set to. See
  [Data](data.md#release-dates-and-certifications-by-country);
- the **jobs** TMDB gives the writers (the screenplay, the story, the novel) and
  the producers a film keeps beside its directors.

`/v1/movie/changed` names the films changed here too — fetched again, a lock put
on, changed or lifted — with or without TMDB.

## Connecting your Sonarr and Radarr

Open **Sonarr & Radarr** in the administration (`/admin/arr`; administrators
only). For each of the two:

- **Address.** Where this server reaches it, with its URL base when it has one:
  `http://sonarr:8989` on a shared Docker network, `http://192.168.1.20:8989` on
  the LAN, `http://localhost:8989` when it runs on this machine outside a
  container. A private network and this very machine are reached. Link-local,
  cloud-metadata, multicast and unspecified addresses are refused, however they
  are written or resolved. The address carries no name, password or query: the
  key has its own field. The certificate of an `https` address is always checked.
- **API key.** Sonarr's or Radarr's own, from *Settings › General*. It is written
  once, never shown again, and kept out of the log like every secret setting.
  *Replace* and *Remove* change it.

A key goes to the address it was given with and to no other. A test or a save at
another scheme, host or port needs the key typed again, and an address removed
takes its key along. A redirect is followed only to the same origin — scheme,
host and port — and never to another, so the key goes nowhere else. These
requests are made directly, whatever `HTTP_PROXY` says.

**Test** asks each who it is (`/api/v3/system/status`) and says its version, or
why it did not answer: the key refused, nothing at that address, or a Radarr where
Sonarr was expected.

| Setting | Default | What it does |
|---|---|---|
| `arr.sonarrUrl`, `arr.radarrUrl` | unset | Where each is reached. Empty, that one is not connected. |
| `arr.sonarrKey`, `arr.radarrKey` | unset | Its API key. A secret: written once, never read back. |
| `arr.pushRefresh` | `false` | Tell them of a change, below. |

These settings are set on the page, which checks an address as an address before
it keeps it. The generic settings endpoint refuses them. The page is the API's
too, for administrators:

- `GET /api/v1/admin/arr` — both connections (`url`, `keySet`, `ready`), whether
  telling is on, how many works wait to be told of, and what was told lately.
- `PUT /api/v1/admin/arr` — `sonarrUrl`, `sonarrKey`, `radarrUrl`, `radarrKey`,
  `pushRefresh`: a field left out stays, an empty one removes. Every change is in
  the audit trail as `setting.changed`; a key's value never is.
- `POST /api/v1/admin/arr/test` — `app` (`sonarr` or `radarr`), and optionally
  `url` and `key`, to try what is typed before it is kept.

## Telling them of a change

A correction made here reaches Sonarr and Radarr at their next refresh, twelve
hours on, unless somebody presses refresh there. With **Tell them of a change** on
(`arr.pushRefresh`, off by default), Sonarr (for a series) or Radarr (for a film)
is asked to read the work again at once, after:

- a lock put on the work, on one of its seasons or on one of its episodes,
  changed or taken off — by hand, or by an imported locks document;
- an edit of its seasons or episodes, or the work switched on or off;
- a sync from some of its sources;
- a refresh, the schedule's or one asked for by hand, that changed what the work
  is served as. A refresh that found everything as it was tells nobody, and nor
  does the fetch that a client's own request makes of a work gone stale: that
  client has the answer already.

The work is found there by the id it is served under — TheTVDB's, or the synthetic
id that a TMDB-only series or a Fan-Kai is handed, for Sonarr; TMDB's for Radarr —
through `/api/v3/series?tvdbId=` or `/api/v3/movie?tmdbId=`. One command then asks
for every work found, up to a hundred at a time: `RefreshSeries` with `seriesIds`,
`RefreshMovie` with `movieIds` (`POST /api/v3/command`).

Changes are gathered by work, so that a burst is one command: a work is told of
ten seconds after the last change to it, or a minute after the first, however long
the burst goes on. A work that Sonarr or Radarr does not hold is noted as *Not held
there*. An app that is not connected is not told. One that does not answer is asked
once, not once for each work: the first is written down as failed, the rest wait for
the next pass, and the log says so once. Each instance tells of the changes made on
it.

The page lists what was told lately — the work, to which app, when, why (*Lock*,
*Edit*, *Sync*, *Refresh*) and how it went (*Sent*, *Not held there*, *Failed*,
with the reason). The latest fifty are shown; the newest 300 are kept.

## Filling the catalogue from their library

The first refresh after Sonarr and Radarr are pointed at this server would fetch
every series and film they hold at once, each while they wait for it. The task
**Seed from Sonarr and Radarr** (`arr.seed`) reads their libraries first and
imports what is missing the way the import page does:

- from Sonarr (`/api/v3/series`): by the id it holds the series under — TheTVDB's,
  or one this server made for a TMDB-only series or a Fan-Kai — then by IMDb's;
- from Radarr (`/api/v3/movie`): by TMDB's id, then by IMDb's.

It starts by hand, from the panel *Fill the catalogue from their library* of the
Sonarr & Radarr page or from the tasks on the **Jobs** page (`/admin/jobs`), and
only while one of the two is connected. It imports one work after another, with half a second between two
fetches. A work held already is passed over, so it can run again at any time and
fetch only what came since, and it can be stopped between two works.

Each work is written down in the run as *imported*, *already held here*, *no
provider could resolve it* or *held there under no id to look it up by*, and the run
ends with how many of each: `12 already here, 30 imported, 2 not found, 0 failed,
of 44`. A library that cannot be read is said at the end; the other is seeded all
the same.

Run it before pointing Sonarr and Radarr at this server, so that their first
refresh finds the catalogue ready.

## Renumbered episodes

TheTVDB renumbers: a special becomes the first episode of a season, a season is cut
in two, an anime's absolute numbers shift by one. Sonarr follows at its next
refresh, and the files it had matched by the old numbers then stand for other
episodes, or for none.

At every write of a series — a refresh, the fetch of a work gone stale, a sync —
the episodes are compared with the stored ones by TheTVDB's own id. Episodes that
moved to another season, number or absolute number are recorded in the audit trail
as `item.renumbered`, by `system`, with how many moved and the first three:

```
“Show”: 2 episodes renumbered — S01E05 → S02E01, S01E06 → S02E02
```

A [notification destination](notifications.md) can be told of `item.renumbered`
like any action of the trail. An episode added or gone, one made by hand, and a
numbering kept because TheTVDB did not answer are not moves.

## Skyhook as a source

This server also asks Skyhook, as a source of its own. It reads the second
edition — `/v2/tvdb/shows/eng/{id}/official` and `/v2/tvdb/search` — and the first
only when the second answers with an error, a `404` included, which is what a
mirror that only knows the first edition answers. From the second it takes every
language TheTVDB names a series in, each season's own title, and the title in the
series' own language. Episode ids are read under either name the editions use
(`tvdbId` or `metadataId`), so a mirror of this server is read as well as the real
service.

If you resolve `skyhook.sonarr.tv` to this server at your resolver rather than per
container, the loop rule in
[If you redirect at the resolver](integration.md#if-you-redirect-at-the-resolver)
applies.

## Checking the whole of it

[`scripts/e2e-stack.sh`](../scripts/e2e-stack.sh) runs real Sonarr 4, Sonarr 5,
Radarr and Plex against this server, from the tree's binary on a catalogue of its
own: it searches and adds real series and films and compares what they hold with
what is served, adds a series in French and in its DVD order on Sonarr 5, locks a
title and finds it in each client after a refresh, connects the server to Sonarr
and Radarr and finds the command a lock sends in their history, and seeds an empty
catalogue from their libraries.
