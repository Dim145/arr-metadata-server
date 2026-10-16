# Editing the catalogue

A field changed by hand is **locked**: it is served whatever the sources say, and no
refresh or sync moves it until the lock is lifted. The catalogue's filters, orders and
counts follow the edit too. This page covers what can be locked and how — a text for
one language alone, a credit, a picture or a title that a source gave — and the
administration's tools for keeping a catalogue in order: the register of every work,
actions on many works at once, the order of the sources, how long a work keeps before it
is refreshed, and the dashboard's list of what there is to do.

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
a count — done, already so, passed over, failed — and the reason for each failure.

### Shifting a season's air dates

In the editor's **Seasons** tab, *Shift the air dates…* moves every dated episode of the
season shown by a number of days: positive for later, negative for earlier, from −3650 to
3650, and not 0. The episodes are laid out with their dates before and after, before anything
is done. The day each episode aired moves, and its broadcast instant (`airDateUtc`) with it
where there is one; each is locked at its new value, episode by episode, and stays so until
its lock is lifted.

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

## The order of the sources

When several sources describe a work, the first with a value gives it, and the next fill in
what it lacks. That order is the setting `sources.priority`: the providers by name, most
trusted first, separated by commas —
`tmdb,tvdb,skyhook,radarr,fanart,tvmaze,anilist,mal,fankai,fankaiwiki` by default. A provider
left out comes after the rest. The setting refuses a name it does not know and a name given
twice. The numbering providers (TheTVDB, Skyhook) still lead the episode list whatever the
order says, and an authority (AniList and MyAnimeList, for a studio) still replaces the rest;
see *Whose episode numbering wins* in the [README](../README.md#whose-episode-numbering-wins).

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
| API keys about to lapse | enabled, not yet expired, expiring within 14 days | `/admin/clients` |
| Pictures and themes given up on | media whose every fetch failed | `/admin/media` |

The four gaps are counted through the catalogue filter's own definition, over the same works
its list shows — those switched off among them, and adult titles as the server's policy has
them — so that a figure and the list it leads to agree. The answer is camel-cased:
`refreshFailed`, `noPoster`, `noOverview`, `noImdb`, `neverRefreshed`, `expiringKeys`,
`keyHorizonDays` and `failedMedia`.

The banner of failed refreshes is the first line of the list. The count of locked fields is the
last, quiet figure among the others, and the explanation of how locking works folds to its
title and stays folded where the reader left it.
