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
  order (`official`), then TheTVDB's — DVD, absolute, alternate, regional, and
  its two further alternate orders (`altdvd`, `alttwo` — a Blu-ray's, a
  streaming service's), which TheTVDB's own episodes endpoint answers empty for
  (thetvdb/v4-api#340) and this server reads season by season instead — then
  TMDB's episode groups and the orders made here, each under the series' own
  name for it. Asked in an order the series has, the episodes are numbered by
  it, an episode TheTVDB files in that order alone among them — a DVD's
  hour-long cut of two aired parts, the extended cuts that make up The Office
  (US)'s Superfan order — as the real service serves them; asked in one it has
  not, they are numbered as they aired, as Skyhook answers. See
  [Orders](#orders).
- **Seasons.** A season's title and year. A title that only says the season's
  number is kept back, as the real service keeps it.
- **Episodes.** An episode's ids under both names the service and the client
  use (`tvdbId` and `metadataId`).
- **Who numbers it.** `metadataSource` is `tvdb`, or `tmdb` for a series TMDB
  numbers — one TheTVDB does not list, or one it stopped answering for (see
  [Who numbers a series](#who-numbers-a-series)) — or `fankai`, or `local` for a
  series [entered by hand](#works-entered-by-hand) that nothing elsewhere names.

The language is three letters, as Sonarr writes it (`eng`, `fra`). The series is
answered in it when the catalogue holds it so; otherwise in the caller's own
`tmdb.language`, then in English, then as it is stored. A code that names no
language is read as `eng`. A title or an overview locked for one language alone is
what a Sonarr asking in that language is given, and a title locked for every
language is every language's name for the work in `translations`; the original
title stays the one the work was made under, unless the original title is locked
too. See [Editing](editing.md#locking-a-text-for-one-language).

**Not found, or not answered.** A series the catalogue holds is answered from
the catalogue, as it was, when the sources do not answer. An id nobody holds is
a `404` only when every source that could resolve it — TheTVDB, Skyhook, TMDB —
answered that there is no such series; when one of them did not answer (a
timeout, a 5xx, a refused connection) the answer is a `502`, never a `404`.
Sonarr takes a series for deleted on a `404` and keeps what it has on anything
else, so the doubt goes to the `502`, which it asks again at its next refresh. A
search is the same: nothing found while a source did not answer is a `502`, not
the empty list Sonarr reads as "no such series", and a name the catalogue holds
is answered whether or not the sources do. On both editions.

Sonarr v5 is a pre-release: this was checked against 5.0.0.2635
([Verified against](integration.md#verified-against)).
[`scripts/e2e-skyhook-v2.sh`](../scripts/e2e-skyhook-v2.sh) sets the real
service's second edition beside this server's for a few real series and tells
every difference apart as an addition or a failure.
[`.github/workflows/parity.yml`](../.github/workflows/parity.yml) runs it every
Monday morning, and on demand, against a server built from `main` with the
repository's TMDB and TheTVDB keys, and fails — a notification from GitHub — on
any difference that is not an addition: the real service's shape moved twice in
October 2026 without a word.

### Upgrading Sonarr to v5

A library migrated to v5 refreshes every series once: Sonarr's migration 208
resets the date each series was last synced, so its first scheduled refresh
after the upgrade asks for every series — one request each,
`/v2/tvdb/shows/{language}/{id}/{seasonType}`, at once. A series that is not
due a refresh here is answered from the catalogue straight away; one that is
due is fetched from the sources once, and two requests for it arriving together
cost one fetch: the second waits for the first and is answered with what it
stored. The wave costs the sources no more than the series that were due.

The limit per address (`AMS_RATE_LIMIT_PER_MINUTE`, 600 a minute by default;
see [Configuration](configuration.md)) can be reached by a large library on
that day: past it, Sonarr is answered `429` with a `Retry-After`, and the series
it was refused for wait for its next refresh. Raise it for the day —
`AMS_RATE_LIMIT_PER_MINUTE=6000` and a restart — and put it back after, or leave
it: the rest of the time Sonarr is nowhere near it.

A v5 search is answered with whole series documents, without their episodes —
every language the series is named in, its actors, its seasons and its orders
— as the real service answers one: some ten kilobytes a result, a few hundred
for a term many series match. The search cache (`cache.searches`) matters more
than it did: a search is answered from it for its lifetime, and only a complete
answer — one every source answered — is kept.

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

### Works entered by hand

A work [entered by hand](editing.md#entering-a-work-by-hand) is found by the ids
it was given elsewhere, as any other: TheTVDB's for a series — or a number made
of TMDB's or Fankai's, as for a series TheTVDB does not list — and TMDB's for a
film. One given none of them is handed an id of this server's own, and Sonarr
and Radarr find it by its title, add it and refresh it like any other work:

- **The id** is a number from 1 000 000 000 to 2 147 483 647, made of the work's
  id in this catalogue: the same at every answer, after a restart, on every
  instance, whatever is edited on the work — and above every number TheTVDB's
  series, TMDB's films and series, and this server's other ranges fill. Sonarr
  keeps it as the series' TheTVDB id, Radarr as the film's TMDB id. Both refuse
  an id of nought or below for a work they add, which is why it is not negative
  as an episode's own is (see [Orders](#orders)). `tvdb:` or `tmdb:` and the
  number find the work too.
- **Asked for by it**, on either edition of Sonarr's surface and on Radarr's
  (`/v1/movie/{id}`, `/v1/movie/bulk`), the work is answered from the catalogue
  alone, without asking a source, which none has: switched off or not, and
  merged into another or not — then with the work it went into, as
  [After a merge](#after-a-merge) says. A number whose work was deleted is a
  `404` at once, as nothing will answer for it again; Sonarr and Radarr then take
  it for deleted.
- Sonarr v5 is sent `metadataSource` `local`. A film entered by hand is named
  under it among the films changed (`/v1/movie/changed`) once one of its fields
  is edited, as any film is once a lock is put on it, so that Radarr's scheduled
  refresh reads it again; with [Tell them of a change](#telling-them-of-a-change)
  on, either is told under this number.
- Given an id elsewhere later — its TheTVDB entry, say — the work is handed that
  one from then on, and still answered by the number to the client that added it
  by it.

Two works of one kind entered by hand could make one number: the one entered
first keeps it, and the other is left out of the searches rather than offered
under a number that would add the first. A work entered by hand now is given an
id here whose number no other holds.

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
id that a TMDB-only series or a Fan-Kai is handed, for Sonarr; TMDB's for Radarr;
the number a [work entered by hand](#works-entered-by-hand) is handed, for either —
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

## After a merge

A [merge](editing.md#duplicates) here moves nothing in Sonarr or Radarr: each keeps every series
and film under the id it holds it by, and is answered under that id. The record merged away
answers with the one kept — its title, its seasons and episodes, its pictures — under the id the
client asked for: Sonarr asking for the merged series' TheTVDB id is given the kept series with
that id on it, and Radarr asking for the merged film's TMDB id, on `/v1/movie/{id}` or in
`/v1/movie/bulk`, the kept film with that `TmdbId`. Its next refresh reads the kept work's
episodes and titles.

A library that held both records holds two entries of one work afterwards, each under its own id
and both answered with the kept work: remove there the one it no longer needs. Plex keeps the
rating key it was handed — `movie-tmdb-604000` goes on answering, with the kept film — and a new
match is given the kept work's own key.

These ids answer for good: an id handed out once is not taken back, unless the merge is undone
within `merge.undoDays`. With **Tell them of a change** on, a merge tells Sonarr (a series) or
Radarr (a film) of both works at once, under the ids it holds them by, and so does an undo; the
page lists them under *Merge*.

## Filling the catalogue from their library

The first refresh after Sonarr and Radarr are pointed at this server would fetch
every series and film they hold at once, each while they wait for it. The task
**Seed from Sonarr and Radarr** (`arr.seed`) reads their libraries first and
imports what is missing the way the import page does:

- from Sonarr (`/api/v3/series`): by the id it holds the series under — TheTVDB's,
  or one this server made for a TMDB-only series or a Fan-Kai — then by IMDb's;
- from Radarr (`/api/v3/movie`): by TMDB's id, then by IMDb's.

A work held there under the number a [work entered by hand](#works-entered-by-hand)
is handed is looked for in this catalogue alone, since no source has it: *already
held here*, or *held there under no id to look it up by* once the catalogue no
longer holds it.

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

To keep a series numbered as it stands while TheTVDB renumbers it, make an order
from its aired one and freeze it: see
[Freezing an order](#freezing-an-order-as-the-aired-numbering).

## Orders

A series aired in one order, and every client is served that one unless it asks
for another: Sonarr v4 always, Sonarr v5 in the season type chosen when the series
was added. The catalogue keeps the series' other orders beside it:

- **TheTVDB's**: its DVD and absolute orders and its further ones — `alternate`,
  `regional`, `altdvd`, `alttwo` — read at each refresh of the series, the last
  two season by season, since TheTVDB's own episodes endpoint answers empty for
  them. Each is named as the series names it on TheTVDB — *Superfan Order*,
  *BluRay Order* — or as TheTVDB names the type.
- **TMDB's episode groups**, listed with the series at each refresh and read from
  TMDB by the refresh that lists one, or changes its count: see
  [Where the data comes from](sources.md#tmdbs-episode-groups).
- **Those made here**, in the editor's **Seasons** tab: see
  [Editing](editing.md#orders).

An order is a sequence of places, a season and a number each. A place names an
episode the series holds, or carries the record its source gave of one it does
not: TheTVDB files a DVD's hour-long cut of two aired parts, or the 194 extended
cuts of The Office (US), as episodes of their own, which the aired list does not
hold. Such an episode is served in the order, with its own title, date and
runtime, under the TheTVDB id it carries — or, for one written here or a TMDB
group's the series does not hold, under a negative id made of what names it, the
same at every answer, and for Sonarr v4 and v5 alike.

After an upgrade from 0.8.3 or before, the orders are read again at each series'
next refresh, which the upgrade brings forward: a series whose orders were kept
before is due at once, and the automatic refresh takes it among the first works it
finds due. Until then its orders are served as they were kept, under the names
TheTVDB gives its types and without the episodes TheTVDB files in one order alone.

### What Sonarr v5 is offered

`seasonTypes` lists the aired order (`official`), then each order the series has:
its `type` is the order's key, its `name` the order's own — with its source after
it where another order listed goes by the same name and this one is not from the
source numbering the series: The Office's *Superfan Order* is TheTVDB's, and
TMDB's group of that name is *Superfan Order (TMDB)*. Two of one source still
alike say how many episodes each holds as well. Plex's list names them the same
way; the order keeps its own name everywhere else.

| Key | Order |
|---|---|
| `dvd`, `absolute`, `alternate`, `regional`, `altdvd`, `alttwo` | TheTVDB's, as TheTVDB types them |
| `tmdb-5acf93e60e0a26346d0000ce` | a TMDB episode group, by its id |
| `ams-my-dvd-order` | an order made here: `ams-` and its name |

Sonarr writes the type into the address it asks for the series at —
`/v2/tvdb/shows/eng/73244/ams-my-dvd-order` — and is answered in that order. The
same order kept by TheTVDB and by TMDB — an absolute order both keep — is offered
once: the one from the source that numbers the series always is, and the other's
beside it only when it holds more episodes, another order then, a Blu-ray's with
its extras. One not offered is still answered when it is asked for by its key. A
TMDB group is read by the refresh that lists it, or changes its count; one that
could not be read then is offered with the count TMDB lists it with and no
seasons, and read when Sonarr first asks for the series in it, or at the next
refresh.

Before choosing one in Sonarr:

- **The order is chosen when the series is added.** Sonarr v5 changes it
  afterwards only while the series has no files (Sonarr's pull request #9055).
  To number a series that has files otherwise, freeze an order as its aired
  numbering instead.
- **An order other than the aired one skips XEM.** Sonarr maps scene numbering
  through XEM and the scene-mapping list for the aired order alone: a series set
  to another matches releases by that order's numbers only.
- **The key is free.** Sonarr v5 takes any word as a season type, one it was not
  built with too: [`scripts/e2e-stack.sh`](../scripts/e2e-stack.sh) adds a series
  in an order made here and checks that Sonarr numbers it so.

### Freezing an order as the aired numbering

An order — TheTVDB's, a TMDB group, one made here — can be frozen as the series'
aired numbering: what the aired order answers with from then on, for Sonarr v5
asking for `official`, for Sonarr v4, which knows no other, for Plex with no order
named, for the TheTVDB relay's `official` pages and the episodes a series' record
carries there (`meta=episodes`), and for the `.nfo` documents. One
order at the most is frozen; freezing another thaws the first. Sonarr follows at
its next refresh — at once when it is told of changes, see
[Telling them of a change](#telling-them-of-a-change) — and the files it matched
by the old numbers then stand for other episodes, as when TheTVDB renumbers.
XEM's mappings are made for TheTVDB's aired numbers: a frozen numbering that moves
episodes makes them point at others.

A frozen order does not keep what airs next from Sonarr: an episode the series
holds that the order does not place, and that airs after the order was last
written or frozen — or is not dated yet — is served under its aired numbers
wherever the order leaves them free. An order made here works so too when it is
asked for by its key. Thawing gives the aired numbering back. The audit trail
says each, `order.frozen` and `order.thawed`.

### Who numbers a series

TheTVDB numbers a series, and a refresh it does not answer keeps the numbering it
gave last. When it has not answered for a series for `numbering.fallbackAfter`
refreshes in a row — three unless set, counted twenty hours apart at the least so
that an outage of an afternoon counts once; `0`, never — and TMDB answers for the
series, TMDB numbers it in TheTVDB's place: its seasons and episodes, or its group
of the original air date where that one numbers the same episodes and puts more of
them where TheTVDB had them — Stranger Things' cuts its fifth season in three, by
the parts it aired in, where TMDB's seasons keep it whole as TheTVDB does — each
episode keeping the TheTVDB id it had. The audit trail says so
(`item.numbering_fallback`, by `system`), a
[notification destination](notifications.md) that wants `numbering.fallback` is
told, `metadataSource` says `tmdb` in what Sonarr v5 is sent, and the work's page
and its editor carry a banner, *Numbered by TMDB since …*, with
**Back to TheTVDB**.

It lasts until TheTVDB answers again, when its numbering is taken back of itself
(`item.numbering_restored`) — unless an order of the series was frozen as its
aired numbering meanwhile: that was TMDB's numbering, and stays so until it is
given back by hand. **Back to TheTVDB** clears the fallback and refreshes the
series at once; TheTVDB still silent, TMDB keeps it. Through the API:
`POST /api/v1/items/{id}/numbering/restore`.

Sonarr renumbers a series TMDB numbers otherwise than TheTVDB: TMDB cuts some
series' seasons its own way, One Piece's into more seasons than TheTVDB's. The
setting is how long TheTVDB is waited for before that.

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
what is served, adds a series in French, one in its DVD order and one in an order
made here on Sonarr 5, freezes an order of another and finds Sonarr following it,
locks a title and finds it in each client after a refresh, connects the server to Sonarr
and Radarr and finds the command a lock sends in their history, and seeds an empty
catalogue from their libraries.
