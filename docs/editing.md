# Editing the catalogue

A field changed by hand is **locked**: it is served whatever the sources say, and no
refresh or sync moves it until the lock is lifted. The catalogue's filters, orders and
counts follow the edit too. This page covers what can be locked and how — a text for
one language alone, a credit, a picture or a title that a source gave — each work's
journal of what changed and the ways back, and the administration's tools for keeping a
catalogue in order: the register of every work, actions on many works at once, the order
of the sources, how long a work keeps before it is refreshed, and the dashboard's list of
what there is to do.

Locks are made in the work's editor (`/admin/catalogue`, then a work), or through the
native API, which asks for the right to write: an editor's or an administrator's session,
or a key with the `write` scope.

## Locking a text for one language

A title or an overview — of a work, a season or an episode — and a work's tagline can
be locked for the readers of **one language alone**, beside the text every other
language is given. A work served in a language is given, field by field:

1. what was locked in that language;
2. else what was locked in every language;
3. else the translation held in that language;
4. else the text the work is stored in.

The rule holds on every surface that serves its own documents: the native API
(`?language=`), Sonarr v4 and v5 — the title, the overview, each episode's, and every
entry of v5's `translations` — Radarr's `Translations`, Plex, and the `.nfo`. A title
locked in every language is every language's name for the work in the lists of
translations Sonarr and Radarr read; the original title Sonarr v5 is given stays the
one the work was made under, unless the original title is locked too.

The TMDB and TheTVDB [relays](relays.md#what-is-written-into-the-answers) write into a
service's own document, which has a text of its own in each language, and so follow a
rule of their own: into a document in the catalogue's language, the text locked in it or
in every language; into one in another language, only the text locked in that language
itself. A text locked in every language — every lock made before 0.8.0 — is the
catalogue's own: a German Jellyseerr is still answered TMDB's own German. Lock a text for
one language alone to have it reach the clients asking in that language.

A lock in the language the catalogue is stored in (`AMS_TMDB_LANGUAGE`) is the stored text
itself: what a client asking in no other language is given, and what a language with no
text of its own falls back to. A date, a runtime or a picture is the same in every
language and is locked without one.

**In the editor**, choose a language under **Show in** on the *Record* tab: a title, an
overview or a tagline edited there asks whether the edit holds for **this language only**
or for **every language**, and its lock says which — *Locked in French*, *Locked in every
language*. A season's and an episode's fields, on the *Seasons* tab, follow the same
choice.

**Through the API**, `language` is `fr`, `fr-FR` or `fra`, which are one. A lock in one
language is made, and lifted, apart from the lock in every language:

```http
PUT /api/v1/items/{id}/overrides
Content-Type: application/json
X-Api-Key: <a key with the write scope>

{"scope": "item", "field": "title", "language": "fr", "value": "Les Experts"}
```

```http
DELETE /api/v1/items/{id}/overrides/item/title?language=fra
X-Api-Key: <a key with the write scope>
```

`scope` is `item`, `season:3` or `episode:3x7`. With a `language`, `field` is `title` or
`overview`, or `tagline` on the work itself: any other field is the same in every language
and is refused with a `400`. A `value` of `null` is locked cleared. Without `language`, the
`DELETE` lifts the lock in every language. A lock in one language is listed with the
others (`GET /api/v1/items/{id}/overrides`, its `language` given). A work's
`lockedFields` names a text locked in another language than the one the work is served in
as `item/title@fra`.

## Hiding and correcting what the sources gave

A credit, a picture or an alternative title that a source gave can be **hidden** rather
than removed — it would come back at the next refresh — and a credit's character and
place can be **corrected**. Each is a lock that names what it holds by what it is, and
not by the row a refresh writes again, so it holds through every refresh:

| What | Named by | Locks |
|---|---|---|
| A credit | the person's TMDB id, or their name where the source gave none, and their role: `credit:actor:tmdb:17419` | `hidden`, `characterName`, `sortOrder` |
| A picture, the work's or a season's | its address as its source gives it: `image:https://…` | `hidden` |
| An alternative title | its language and text: `alternativeTitle:fr:Les Experts` | `hidden` |

Hidden, it is in no answer: Sonarr, Radarr, the `.nfo`, the site's pages and cards, a
person's own page, the titles Sonarr matches releases by, and the lists of people,
pictures and other titles that the [TMDB and TheTVDB relays](relays.md#what-is-taken-out)
hand on. It stays listed in the editor, faded and marked *hidden*, to be shown again. The
picture chosen to lead with is shown whatever is hidden. A credit, picture or title added
by hand is removed rather than hidden; a credit added by hand is corrected in its own row.

The editor's **People & titles** and **Artwork** tabs hide and show each row a source gave,
and correct a credit's character and place in place. Through the API each row is changed
by the id the work is read with:

```http
PATCH /api/v1/items/{id}/credits/{creditId}
Content-Type: application/json
X-Api-Key: <a key with the write scope>

{"hidden": true}
```

- `PATCH /api/v1/items/{id}/credits/{creditId}` takes `hidden`, `characterName` and
  `sortOrder`; `null` lifts a correction, and `{"hidden": false}` shows a credit again.
- `PATCH /api/v1/items/{id}/images/{imageId}` and
  `PATCH /api/v1/items/{id}/alternative-titles/{titleId}` take `hidden`.
- A work read whole by whoever maintains the catalogue carries what was hidden apart, under
  `hidden` (`credits`, `images`, `alternativeTitles`). Nobody else is given it.

These locks are counted with the rest, exported and imported with them, and lifted by
**Unlock everything**.

## Locks as a file

`GET /api/v1/admin/locks` downloads every lock made by hand as one JSON document, and
`POST /api/v1/admin/locks` imports one into another catalogue, each lock finding its work
of the same kind by the TMDB, TheTVDB or IMDb id it shares. A request takes two thousand
locks at most, and the dashboard sends a longer file in parts. The document is **version 2**
when it holds a lock in one language or a lock on a credit, a picture or a title that a
source gave, and version 1 otherwise; this server reads both.

## The journal

Every work keeps a **journal** of what changed in it, value by value: a field of the work,
of a season or of an episode, or a child a lock names. Each change says what the value held
and what it took, when, why, and who — a person, a key, or a source, or both when a person
asked the sources again:

| Why | Made by | What it is |
|---|---|---|
| `refresh` | a source | a refresh, asked for or the schedule's, the six-monthly one of TMDB alone included |
| `sync` | a source | some sources asked again, from the editor's *Sources* panel or *Ask the sources again…* |
| `fallback` | a source | the seasons and episodes moved when TMDB numbers a series in TheTVDB's place, or TheTVDB takes it back |
| `lock`, `unlock` | a person | a lock set, changed or lifted — in the editor, by *Change a field…*, through the API; a credit, a picture or a title a source gave hidden or shown again; the still or the theme an upload fills |
| `edit` | a person | a field of what was entered by hand, which has no source to be locked against; a season, an episode, a picture, a credit or an alternative title added or removed by hand, and a credit added by hand corrected |
| `revert` | a person | a value gone back to from the journal, or a change undone |
| `import` | a person | a lock brought in from a file (`POST /api/v1/admin/locks`) |
| `merge` | a person | another record merged into this one: a line naming it, and each field kept from it — see [Duplicates](#duplicates). Undone, the same lines go back as a `revert` |

A refresh writes everything it changed at once, in one transaction, and nothing when it
changed nothing — what most refreshes find. Of a season the journal follows the title, the
overview and the air date, and of an episode the title, the overview, the air date, the
runtime and the still: a series of a thousand episodes would otherwise write a thousand rows
of everything else. An episode is followed by what it is known by — TheTVDB's id, then
TMDB's, then its numbers — so one that moved is one change, *Numbers* `S01E05 → S02E01`, and
one that came or went is another. What a person added by hand is not a source's to change,
and is not followed by a refresh: its coming and going are the person's, journaled as they
add or remove it — a change of the row's own scope (`season:7`, `episode:7x1`, `image:…`,
`credit:…`, `alternativeTitle:…`) with a few words of it. A source's change made under a lock says so, *Under a lock*: it reached
the record, and not what is served.

The journal is the editor's **Journal** tab: newest first, the changes one write made in the
order the record lists them, filtered by field, by who — a person, a key or a source — and by
a period from one day to another, a page at a time. *CSV* and *JSON* hand over every change
the filters match, not only those shown; a CSV opens in a spreadsheet. A change is kept as
long as `history.keepDays` says — a year unless it is set, `0` for ever — and pruned by the
pass that prunes the audit trail; a work deleted takes its journal with it. The audit trail
says *that* a lock was set; the journal says what it held.

### Going back to a value

*Revert to this value*, beside a change of a field a lock can hold, locks the field to the
value it held before the change, through the locks' own route: what a source gave is never
written back over its copy. The lock, the audit trail's line (`override.set`, `revert`) and
the journal's own say so. Lifting the lock instead hands the field back to whatever the
sources say now.

### What the sources say under a lock

Under a field locked in every language, when the sources give it another value now, a line
in slate says what they give — *TMDB now says: …* — and, when a source changed it after the
lock was set, *changed since the lock*. *Take this value* lifts the lock, and the journal
says what it held and what is served without it. A lock in one language alone, the work's
identity — adult, its slug, its identifiers — and the pictures chosen to lead with have no
such line: what the sources give is in the stored language, and the identity is written to
the work as it is locked.

The dashboard counts the works with a lock a source moved under since it was set, and which
says something else than the lock now: *N works have a lock whose source has changed*. A lock
set again — kept as it was — is no longer counted, nor one whose source came round to it.

### What a refresh would change

The editor's refresh asks first. The sources are asked as a refresh asks them, merged as a
refresh merges them, and nothing is written — not the work, not its journal, not its orders,
not who numbers it; Sonarr and Radarr are not told. What a refresh would write over is listed,
each value with what it holds and would hold, and the locked ones are not, since no refresh
reaches them. *Lock before refreshing*, beside each field a lock can hold, locks it as it
stands first, and the refresh — run from the same dialog — leaves it alone.

### Undoing a change over many works

Once **Change a field…** is done over a selection, *Undo* is offered for eight seconds — for as
long as the pointer or the focus rests on it, besides. Each work the change was done for is
put back as its journal says it was: the lock it held, with its value, or no lock at all; the
journal and the audit trail call it a revert. A work whose field something else changed since
— a refresh, somebody else — is left as it is, and said so in the summary.

### Through the API

For whoever may edit the catalogue:

- `GET /api/v1/items/{id}/changes` reads the journal, newest first: `field`, `actor` (a person,
  a key or a source), `since` and `before` (an instant, or a day from its midnight UTC), fifty
  a page unless `limit` says otherwise, and `cursor` — the `next` the page before was answered
  with. `format=csv` or `format=json` hands over every change the filters match, as a file.
- `POST /api/v1/items/{id}/changes/undo`, with `{"scope": "item", "field": "genres"}`, undoes
  the caller's last change of that value: `409` when something else changed it since, and
  for a row added or removed by hand, which is removed or added again instead.
- `PUT /api/v1/items/{id}/overrides` with `"cause": "revert"`, and
  `DELETE /api/v1/items/{id}/overrides/{scope}/{field}?cause=revert`, are a lock set or lifted to
  go back to a value, which the journal and the audit trail say.
- `POST /api/v1/items/{id}/refresh?dryRun=1`, and `POST /api/v1/items/{id}/sync?dryRun=1` with
  the sources to ask, answer what would be written over — `changes`, each with its `scope`,
  `field`, `before`, `after` and the `provider` whose value it would be — and write nothing.
- `GET /api/v1/items/{id}/provenance` carries, under `diverging`, each lock whose sources say
  something else now: its `scope` and `field`, what they say (`value`), whose it is
  (`provider`), and whether a source changed it since the lock was set (`moved`).

## The catalogue

The administration's catalogue (`/admin/catalogue`) is a register of every work the server
holds, disabled ones included, one row each. A row leads with the work's poster, at the
width of a thumbnail, then its title, year and kind; its identifiers elsewhere; four marks
for what its record has; its state (disabled, entered by hand, locked fields); and when it
last changed. The row opens the editor. Its **⋯** button holds the rest — open the editor
or the public page, refresh, enable or disable, delete — as plain buttons and links, reached
with Tab and shut by Escape, which fits a phone.

### What a record lacks

The four marks are a poster, an overview, an IMDb id and a first refresh. Each says what it
means to the pointer that rests on it and to a screen reader; a missing one is struck
through, and the refresh mark is faded on a work entered by hand, which is never refreshed.

Four filters above the table — *No poster*, *No overview*, *No IMDb id*, *Never refreshed* —
narrow the list to the works lacking that, all of them when several are pressed. They are
kept in the address, so that a view can be sent as a link:

```
/admin/catalogue?missing=poster
/admin/catalogue?missing=poster,overview
/admin/catalogue?refreshFailed=1
```

The native API takes the same parameter, `GET /api/v1/items?missing=poster,overview`, comma
separated, each value narrowing further. It is for whoever may edit the catalogue, as
`includeDisabled` and `refreshFailed` are: anybody else is answered as if it had not been
sent, and a value the server does not know is refused with a `400`. Exactly:

| Value | A work lacking it |
|---|---|
| `poster` | has no poster of its own — a season's is the season's — and none chosen by hand |
| `overview` | has an empty overview in the language it is stored in: none from its sources, or one cleared by hand. An overview locked for one other language alone does not count |
| `imdb` | has no IMDb identifier |
| `refresh` | was never refreshed although it has an identifier to be refreshed by. A work entered by hand is never refreshed, and is not counted |
| `duplicates` | is one of a pair proposed as one work, not said to be two, the other still a work of its own: see [Duplicates](#duplicates) |

### Doing one thing to many works

Tick the rows — or the box in the header, for every row shown — and the bar offers:

- **Change a field…**: one field given to every selected work — the original language (a
  code such as `ja`, `en`, `fra`), the status, the network, the studio, the adult flag or the
  content rating set to one value; for the genres, one genre added or taken off. Or the lock
  on that field lifted, which hands it back to the sources at the next refresh. A value set is
  locked, as an edit in the editor is. The dialog says in a sentence what is about to happen
  before anything does, and a genre already there, or a lock that was not, is left alone and
  counted as such.
- **Refresh**: each work fetched again from its sources; one entered by hand is passed over.
- **Ask the sources again…**: some of the sources switched on, asked again about each work
  they describe, as the editor's *Sources* panel does for one work. The other sources keep
  their last answer, locked fields do not move, and the refresh schedule stands. A work
  entered by hand, or not refreshed in full since the server began recording where its values
  come from, is passed over.
- **Enable**, **Disable**, **Add to a list…**, and **Delete**, which asks first.

Each work is its own request, four at a time, and its own line in the audit trail, so one
that fails takes nothing else with it. A bar shows how far the run has gone, and it ends with
a count — done, already so, passed over, failed — and the reason for each failure. A field
changed so can be undone a moment after: see
[Undoing a change over many works](#undoing-a-change-over-many-works).

### Shifting a season's air dates

In the editor's **Seasons** tab, *Shift the air dates…* moves every dated episode of the
season shown by a number of days: positive for later, negative for earlier, from −3650 to
3650, and not 0. The episodes are laid out with their dates before and after, before anything
is done. The day each episode aired moves, and its broadcast instant (`airDateUtc`) with it
where there is one; each is locked at its new value, episode by episode, and stays so until
its lock is lifted.

### Orders

At the foot of the editor's **Seasons** tab, *Orders* lists every order the series' episodes
come in: the aired one, the work's own; TheTVDB's — its DVD and absolute orders, and the
further ones the series names, *Superfan Order*, *BluRay Order*; TMDB's episode groups, each
with the count TMDB lists it with until a refresh reads it; and those made here. Each says where it
comes from, how many seasons and episodes it holds, whether it is frozen, and whether clients
are offered it — the same order kept by TheTVDB and by TMDB is offered once. *See* opens a
season's page in it.

*Make an order of it* — or the form below the list — makes an order of the aired one, of any
other, or of nothing, under the name given: its key is `ams-` and the name's words,
`ams-my-dvd-order`, and Sonarr v5 and Plex are offered it at once. An order made here is
written in place, a season at a time:

- **Up** and **down** swap what two places hold, their numbers staying where they are: the
  quickest way to put two episodes the other way round;
- a place's season and number are typed in, and a season and a number are given once each;
- *Place an episode of the work* adds one the order does not place yet, at the end of the
  season shown; *Add an episode of its own* adds one the work does not hold — a cut, an extra —
  by its title: it is served in this order alone, under an id of its own;
- an episode of the order's own, or one the work no longer holds, is taken out with the bin.

No episode is cut in two or joined to another. TheTVDB's orders and TMDB's groups are read
again at every refresh and are not written here; make an order of one to change it. An order
made here is deleted with the bin beside it.

*Freeze* makes an order the series' aired numbering: what Sonarr asking for the aired order is
given from then on, and Sonarr v4, Plex with no order named, the TheTVDB relay and the
`.nfo` documents with it. The dialog says what follows — Sonarr renumbers at its next refresh,
its files then stand for other episodes, XEM's mappings no longer fit — before anything is
done; *Thaw* undoes it. See [Orders](sonarr-radarr.md#orders). While TMDB numbers the series
in TheTVDB's place, a banner at the top of the tab, and on the work's page, says since when,
with *Back to TheTVDB*: see [Who numbers a series](sonarr-radarr.md#who-numbers-a-series).

The *Other numberings* panel of the **Elsewhere** tab lists the same orders and leads here.
Through the API: `GET`/`POST /api/v1/items/{id}/orders`, `PUT`/`DELETE
/api/v1/items/{id}/orders/{key}`, `POST`/`DELETE /api/v1/items/{id}/orders/{key}/freeze`.

### Importing a work on purpose

The catalogue otherwise fills itself — a client asks for a work, this server
fetches it. **Import** searches the providers directly and pulls in the one you
meant, down the same path a client's request takes, so an imported work is
indistinguishable from one that arrived on its own.

### Entering a work by hand

*New entry* takes every field `POST /api/v1/items` does: the kind, the title, the year, the
overview, the status, the runtime, the first aired day (a series) or the in-cinemas day (a
film), the network (a series), the studio, the genres (separated by commas), and the TMDB,
TheTVDB and IMDb identifiers. The form says what is wrong beside each field before anything
is sent, and the server checks the same, refusing with a `400`:

- a status the kind does not have: `continuing`, `ended`, `upcoming` or `deleted` for a series;
  `tba`, `announced`, `inCinemas`, `released` or `deleted` for a film, in any case, stored as
  the clients spell them;
- a year outside 1800 to 2200, or a runtime outside 0 to 10,000 minutes;
- an identifier of nought, or an IMDb id that is not `tt` and seven to ten digits;
- a title over 500 characters, an overview over 20,000, a network or a studio over 200, more
  than 50 genres, or a genre over 200 characters. Text is trimmed, a blank field is dropped,
  and a genre named twice is kept once.

An entry made by hand is never refreshed: what was typed is what is served, until it is
edited. Its identifiers are what Sonarr and Radarr find it by — TheTVDB's for a series, TMDB's
for a film — and where its links lead. An identifier that another entry already claims is
refused with a `409`.

## Duplicates

Two records of one work turn up in a catalogue now and then: a film entered by hand before a
client asked for it, a series filed under TMDB's id and again under TheTVDB's, two entries TMDB
has since merged into one. The server looks for them, proposes them, folds two into one when
asked — and takes them apart again for two weeks.

### Found

Two works are proposed as one when:

| How sure | When |
|---|---|
| *certain* | both are films, or both series, and are filed under one identifier elsewhere — TMDB's, TheTVDB's, IMDb's, AniList's, MyAnimeList's or Fan-Kai's. An identifier is held by one work at a time, and a refresh of one may take it from the other: the other still counts as filed under it while its journal says it was, or a lock on its identifiers |
| *probable* | both are films, or both series, under one title as the search folds it — accents, case and punctuation aside — and a year apart at the most, or one of them without a year |
| *flag* | a film and a series carry one IMDb id: one of them is filed wrongly. Said, and never merged — put its identifiers right in its editor |

They are looked for again whenever the dashboard's list of errands, or the list of pairs, is
read. The dashboard counts the works in a pair — *N works look like another one* — and leads to
the register narrowed to them, `/admin/catalogue?missing=duplicates`, where each row carries
the other's title: pressed, it sets the two side by side. *Not the same work*, there, says the
two are not one work: the pair is remembered, and never proposed again whatever is found later.

### Merged

**Merge…**, in a row's **⋯** menu, merges the work with another found by its title; a pair's
mark opens the same dialog on the two. One record is kept, and the other is merged into it.
The rule keeps the one its sources fill over one entered by hand, which is never refreshed:
kept, a stub typed in by hand would hold what the sources said on the day of the merge, for
good. Then the one somebody put something into by hand — a lock, a season, an episode, a
picture, a credit, a title — else the one filed under more identifiers, else the older; *Keep
this one instead* keeps the other. Each field the two give
different values of is listed, the kept record's value chosen unless the other is: a value taken
from the merged record is locked on the kept one, as an edit is. The dialog says in a paragraph
what the merge does before anything is done, and one transaction does it:

- what the merged record holds moves onto the kept one, table by table — its seasons and
  episodes, pictures, credits, other titles, translations, ratings, its places in selections
  and its journal, and its orders, thawed: the kept one's numbering stands. Where the kept one
  holds the same — a season of the same number, an episode at the same place, a picture at the
  same address, a translation into the same language, a credit of the same person in the same
  part, an order under the same key — the kept one's stands, and the merged one's is set aside
  in the merge's record. What its sources last answered is not kept: the kept one has its own;
- its locks move where the kept one locks nothing of the same; where both lock a field, the
  choice made for it decides, and the kept one's lock stands without one;
- its identifiers become the kept one's where the kept one has none of their source;
- its id, its slug and each of its identifiers are filed as **aliases** of the kept one, for
  good: the site, the API, Sonarr, Radarr and Plex go on answering under them, with the kept
  work — see [After a merge](sonarr-radarr.md#after-a-merge). The site puts the kept work's
  address in place of an old one;
- the merged record stays, switched off, never refreshed and in no list, beside what the merge
  did.

The kept one's journal says so — a line naming the record merged in, and each field kept from
it, under *Merge* — and the audit trail has `item.merged`, with what the kept work was and
became. With [Tell them of a change](sonarr-radarr.md#telling-them-of-a-change) on, Sonarr or
Radarr is told of both works. A film and a series are never merged, whatever they share.

### Undone

For `merge.undoDays` days — 14 unless set, 365 at most, `0` for no undo at all — the kept
one's editor opens on a line saying what was merged into it, with *Undo the merge*. The merged
record comes back as it was: what moved over and is still as the merge left it moves back, what
was set aside is put back, its locks and identifiers with them, and the aliases go. What changed
on the kept one since — a lock set again, a refresh — stays the kept one's, and an identifier
another work has taken since stays with it, which the undo says. The record put back is due a
refresh, which asks its sources again — unless it was entered by hand. The two are not proposed
as one work again. The journal writes the way back as a `revert`, and the audit trail
`item.unmerged`. The routes are in [Two records of one work](api.md#two-records-of-one-work).

## The order of the sources

When several sources describe a work, the first with a value gives it, and the next fill in
what it lacks. That order is the setting `sources.priority`: the providers by name, most
trusted first, separated by commas —
`tmdb,tvdb,skyhook,radarr,fanart,tvmaze,anilist,mal,fankai,fankaiwiki` by default. A provider
left out comes after the rest. The setting refuses a name it does not know and a name given
twice. The numbering providers (TheTVDB, Skyhook) still lead the episode list whatever the
order says, and an authority (AniList and MyAnimeList, for a studio) still replaces the rest;
see [Whose episode numbering wins](sources.md#whose-episode-numbering-wins).

An administrator puts it in order on the **Sources** page: a source moves a rank with its
arrows, or is dragged onto another's place, and the matrix's columns follow. A run of moves is
saved as one change. *Back to the starting order* restores the order the server started with.
Every merge reads the setting when it runs, so a work takes a new order at its next refresh;
*Refresh everything*, on the Jobs page, applies it to every work at once.

`AMS_PROVIDER_PRIORITY` gives the setting its first value only — a name it does not know is
passed over then — and is not read again once the setting exists. `GET /api/v1/sources/rules`
gives the order the merge follows (`providers`) and the one the server started with
(`defaultOrder`).

## How long a work keeps

How long a work keeps before it is refreshed again is a setting, on the **Settings** page under
*Automatic refresh*:

| Setting | Default | Range | Meaning |
|---|---|---|---|
| `refresh.ttlContinuing` | `AMS_REFRESH_CONTINUING_TTL`, else 21600 (6 hours) | 60 to 31,536,000 seconds | A series still airing, or a film not out yet. An episode about to air brings it back sooner |
| `refresh.ttlEnded` | `AMS_REFRESH_ENDED_TTL`, else 604800 (7 days) | 60 to 315,360,000 seconds | An ended series or a released film |

The scheduler reads both each time it schedules a work, so a change applies to each work from
its next refresh. The variables give the settings their first value only, as
`AMS_REFRESH_INTERVAL` does for its own, and are held to the same ranges at start: a value
outside them stops the server, naming the variable.

## What there is to do

The administration's dashboard (`/admin`) opens on a list of errands, read from
`GET /api/v1/admin/todo` (administrators only). Each line is a count and the way to the works
it counts, and a figure of nought is no line; with nothing left, the card says so once.

| Line | Counted as | Leads to |
|---|---|---|
| Works whose last refresh failed | works with a refresh error | `/admin/catalogue?refreshFailed=1` |
| Works with no poster | `missing=poster` | `/admin/catalogue?missing=poster` |
| Works with no synopsis | `missing=overview` | `/admin/catalogue?missing=overview` |
| Works with no IMDb identifier | `missing=imdb` | `/admin/catalogue?missing=imdb` |
| Works never refreshed | `missing=refresh` | `/admin/catalogue?missing=refresh` |
| Works with a lock whose source has changed | a lock a source moved under since it was set, and which says something else now | `/admin/catalogue?missing=divergingLock` |
| API keys about to lapse | enabled, not yet expired, expiring within 14 days | `/admin/clients` |
| Pictures and themes given up on | media whose every fetch failed | `/admin/media` |
| Works that look like another one | `missing=duplicates`, the pairs looked for again first | `/admin/catalogue?missing=duplicates` |

The four gaps are counted through the catalogue filter's own definition, over the same works
its list shows — those switched off among them, and adult titles as the server's policy has
them — so that a figure and the list it leads to agree. The answer is camel-cased:
`refreshFailed`, `noPoster`, `noOverview`, `noImdb`, `neverRefreshed`, `expiringKeys`,
`keyHorizonDays`, `failedMedia`, `divergingLocks` and `duplicates`.

The banner of failed refreshes is the first line of the list. The count of locked fields is the
last, quiet figure among the others, and the explanation of how locking works folds to its
title and stays folded where the reader left it.
