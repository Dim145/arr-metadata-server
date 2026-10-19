# Data: releases by country, ratings and what a work carries

The sources give a work more than a title and a release date: the days it came out in
each country, the rating it carries there, the line it was sold on, every country and
company that made it, the people who wrote it, its Wikidata item. This page says what is
kept of it, where it is shown, and what it changes in the answers.
[Where the data comes from](sources.md) says which
source brings what, and in which order the sources are merged.

A work takes the data on this page at its next refresh; **Refresh everything**, on the Jobs
page, fills them all at once.

## Release dates and certifications by country

Every work keeps, country by country, what its sources say of it there:

- for a **film**, each release TMDB lists — a premiere, a limited run, cinemas, digital,
  disc, television — with its day, its rating and TMDB's note (a festival, a service, a
  re-release);
- for a **series**, every rating TMDB and TheTVDB give it, where only one of each was
  kept before;
- Radarr's own service gives a film every country's certification as well.

Per country, the releases come whole from the first source that lists any for that country,
in the order the sources are merged: two sources' days for one country cannot be told apart
from two releases. A later source's rating fills a country the first gave days without one.
A rating from Radarr's service is filed under the United States when it has one there.

The native API gives them on a work as `releases`, each with:

| Field | Meaning |
|---|---|
| `country` | ISO 3166-1 alpha-2, upper case: `FR`, `US` |
| `releaseType` | `premiere`, `limited`, `theatrical`, `digital`, `physical` or `tv`. Absent for a rating given on its own, as a series' are |
| `date` | the day, `YYYY-MM-DD` |
| `certification` | the rating there: `TP`, `PG-13`, `12A` |
| `note` | what TMDB notes beside it |
| `source` | `tmdb`, `tvdb` or `radarr` |

The work's own `contentRating` and `contentRatingCountry` stay one rating, the United
States' where there is one, as Sonarr and an NFO's `<mpaa>` read it. A film's first day in a
country is the earliest of its days there: in cinemas (a limited run counts, a premiere does
not), online, or on disc.

**What reads them.**

- **Radarr** is sent every country's certification (`Certifications`), in the order of their
  codes, and takes the one of the country it is set to, as it does from `api.radarr.video`. A
  rating locked in the editor is the one sent for its country.
- **The site** dates and rates a work for its reader. The plate shows the rating of the
  reader's country, and a panel — *Releases* for a film, *Certifications* for a series — lists
  that country's days and rating, every other country a press away. Where the reader's country
  has none, the record's own rating is shown with the country it is of.
- **Plex** is given the rating and the release day of the country it names
  (`X-Plex-Country`). See [Plex](plex.md#what-plex-is-given).
- **The schedule** places a film on the day it comes out in the reader's country, below.

**The reader's country** is the one picked under *Where to watch* on a work's page, else the
setting `tmdb.watchRegion` as it stands for them — on the server, on their key or on the rule
of the address they call from — else the country of their language (`fr-FR` is `FR`). It is
what `GET /api/v1/auth/me` says as `region`.

## The schedule

`GET /api/v1/calendar` lists what airs and what comes out in a window of at most 62 days
(`from` and `to`, RFC 3339 instants): the episodes of every series, and the films in cinemas,
digitally or on disc. `entries` holds both, earliest first, each tagged with its `kind`:

- an `episode`, at the moment it airs in UTC, or at midnight UTC on its date where no provider
  knew the time;
- a `release`, at midnight UTC on its day, with `workId`, `date` and `releaseTypes` (`cinema`,
  `digital`, `physical`, cinemas first). The kinds that fall on one day are one release.

`works` carries every work the entries belong to, and `truncated` says that the window held more
than one answer carries and the latest are missing: ask for a shorter one. The episodes alone,
which 0.8 also listed apart as `episodes`, are the entries whose `kind` is `episode`.

A film comes out on the day it comes out **in the reader's country**, where that is known, and on
its own dates otherwise; a date fixed by hand stays where it was put. This holds in `/calendar`
on the site, the week and the month, in `calendar.ics` and in the airing feed. `region` asks for
another country (`region=FR`; anything but two letters is a `400`).

`calendar.ics` carries each release as an all-day event, named in the language asked — *Interstellar
— sortie en salles* — and the airing feed (`airing.atom`) has an entry for it. A film's own calendar,
`/api/v1/items/{id}/calendar.ics`, lists every release, a day each, under the UIDs
`release-{id}-{cinema|digital|physical}`: an app subscribed to it replaces its one event once. The
feeds are described with the others in [Calendars and feeds](exports.md#calendars-and-feeds). The site lists the
films beside the episodes on each day one comes out — in cinemas, digitally, on disc — in the week
with its poster and a chip a kind, in the month's cells after the day's episodes, and on the front
page's strip, counted apart: *15 episodes · 2 releases*.

## The tagline, the countries and the companies

TMDB's **tagline** is kept in every language TMDB gives it in — the catalogue's own, and each of the
work's translations — and shown under the title in the language a page or a client asks for, as the
title and the overview are; a tagline locked for one language stands over TMDB's in that language.
Every **country** a work was made in and every **company** that made it are kept, where only the first
of each was; the studio is the first company, or the one an anime source names instead. They are on the
work's page, and in its NFO as `<tagline>`, a `<country>` each, by its English name, and a `<studio>`
each after the studio itself. The tagline, the production countries, the production companies and the
AniList format can each be locked in the editor.

## The people

A work keeps TMDB's **writers** — the screenplay, the story, the novel — a series' **creators** and a
film's **producers** beside its directors, each with the job TMDB gives them: up to three directors, and
up to six of each of the others. Radarr is sent those jobs, and the page names each job once with its
people. How many of TMDB's **actors** a work keeps is the setting `tmdb.castLimit`, on the Settings page:

| Setting | Default | Range | What it does |
|---|---|---|---|
| `tmdb.castLimit` | `25` | 1 to 200 | How many of TMDB's actors a work keeps, top of the bill first. Each is a row, and a portrait to fetch and keep. A work already held takes the new figure at its next refresh |

A film made from Radarr's service alone no longer lists its composer and its editor among the actors: its
directors, writers and producers are kept, and the rest left out.

## Wikidata, AniList's format and tags

TMDB's **Wikidata item** is kept as an identifier — `Q1079`, whether it arrives so or as a Wikidata address —
linked from the page and editable among the others.

The **format** AniList gives an entry — `TV`, `TV_SHORT`, `MOVIE`, `SPECIAL`, `OVA`, `ONA` or `MUSIC` — is kept
and shown on the page as a chip where it says more than *series* or *film*: an OVA is filed as one or the
other, and the chip says what it is. AniList's **tags** stay beside TMDB's keywords: keywords are gathered
from every source, each once whatever its case, where the first source with any gave the whole list.
