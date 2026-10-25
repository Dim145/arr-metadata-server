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

### TheTVDB's characters

TheTVDB lists a series' **characters** — the part, the person who plays it, TheTVDB's id of that person and
their portrait. The first of them, in TheTVDB's order, are read into the cast, and how many is the setting
`tvdb.castLimit`:

| Setting | Default | Range | What it does |
|---|---|---|---|
| `tvdb.castLimit` | `25` when unset | 1 to 200 | How many of TheTVDB's characters a series keeps, in TheTVDB's order. A work already held takes the new figure at its next refresh |

Each is matched to the person of the cast another source gave — by TMDB's id where both give one, else by the
name, whatever its accents, its case and the order of its words (TheTVDB writes some Japanese names family name
first), or by the name in its own script TMDB gives — and gives that person TheTVDB's id, and a part or a
portrait where the cast had none. One nobody else listed is added after the cast. Somebody who plays two parts is
one credit, the parts put together. A guest of one episode is not in the series' cast.

### A guest is credited on the episode

TMDB's season documents, which a series is read with already, name each episode's **guests**, its **director**
and its **writers**. They are kept on that episode, not on the work: the work's cast and crew — what Sonarr,
Radarr, Plex and the NFO are given — are what they were. An episode is the work's own, as TheTVDB may number
it: TMDB's id of the episode places the credit on the episode the merge gave that id, and a credit of an episode
the work does not list is left out. Each episode keeps as many guests as `tmdb.castLimit` keeps of the cast, in
TMDB's order; never a producer, whom a series credits on every episode. A refresh that brings no episode credits
— TMDB not asked, or not answering — keeps those held. A person's page lists them under the series, with how
many episodes and which years.

### Each person a record

Everybody the credits name is a **person**, a record of their own: one, whatever the credits call them, with the
ids TMDB, TheTVDB and IMDb file them under, the other names they go by, their name in their own script, and —
once their page has been read — what TMDB says of them: born when and where, a biography, portraits, what they
are known for. That is asked of TMDB the first time the page is read and kept a month, never for everybody a
refresh credits; a biography in another language than the catalogue's is asked when a reader asks for it, and
kept a day.

A credit is filed under its person as it is written, by these rules, in this order:

1. **Kept.** A credit the sources give again — the same kind, on the same episode or on none, naming the same
   TMDB id or else the same name — keeps the person it had. Whatever was decided by hand outlives the next
   refresh.
2. **TMDB's id.** The person with that TMDB id, or the one a person with it was merged into; else the person
   TheTVDB's id names, who is given the TMDB id; else a new record.
3. **TheTVDB's id**, for a credit TMDB gives no id for.
4. **The name**, the person's own or one they go by: the one person known by a TMDB id who goes by it; else the
   first person known by that name alone; else a new record.

What a source says beyond the name — TheTVDB's id, the name in its own script, a portrait — completes a record
found by an id, never one found by a name, which is a guess. A credit added by hand is filed the same way within
seconds of being added.

**At the upgrade** (migration 0041), a record is made of everybody the credits held name: one for each TMDB id,
under the name most of its credits give it, its other spellings kept among its names; one for each name the
other credits give. A credit giving a name and no TMDB id where exactly one person known by a TMDB id is
credited under that name somewhere is taken for that person — the actor typed in by hand, the guest TheTVDB
lists, are far more often the actor TMDB credits elsewhere than somebody else. Two people known by TMDB ids
under one name: the credit by name alone is neither. The risk is a **homonym** — somebody else of the same
name, filed under the better-known one — and the way out is to split the credit off on the People page.

On **People** in the administration, whoever maintains the catalogue lists everybody with what they did and how
many works credit them, or those known by no TMDB id, or those who share a name with somebody else — the likely
homonyms and duplicates — and puts a record right: **merges** two records of one person (the credits filed under
the one kept, which goes by the other's names and takes the ids it lacks; the other kept as a pointer, so that its
ids and its address lead there), **splits** credits off onto a record of their own, gives a credit typed by name
**the person it is of**, and **enters** somebody by hand with their ids. A record put right by hand keeps what
was typed: TMDB fills only what is left empty. A credit a lock hides names nobody: a person whose every credit is
hidden is neither in the index nor found by the search. Each of these writes is on the audit trail, under the
record it was made to: `person.created`, `person.updated`, `person.merged`, `person.split` and
`person.credited`.

## Genres

A genre is a record of its own, under a slug that is the same in every language — `drama`,
`science-fiction` — with its name in each language it is known in. The sources name a genre in the
language they are asked in, and some in their own words whatever they are asked: TMDB says "Drama"
in English and "Drame" in French, AniList "Sci-Fi" where TMDB says "Science Fiction", TheTVDB
"Children" where TMDB says "Kids". Each of these leads to one genre: the server knows TMDB's,
TheTVDB's, AniList's, MyAnimeList's and Fankai's genres by every name they go by in English and in
French, and a genre it does not know is its name made into a slug — `Sword & Sorcery` is
`sword-sorcery` — named as a source gave it. A catalogue fetched in English and then in French,
after the server's language changed, lists one genre where it listed two, and counts each work once
under it.

A work keeps its genres as its sources gave them — what Sonarr, Radarr, Plex and the NFO files are
given — and is listed under their slugs, a lock on its genres applied, as its other values are; TMDB's
combined series genres are their parts, "Action & Adventure" being Action and Adventure. Every work is
listed again in the background the first time the server starts with this, and a genre is filed as
soon as a work is listed under it. Each genre has a page of its own (see
[The site](interface.md#a-genre-a-network-a-year)).

## Wikidata, AniList's format and tags

TMDB's **Wikidata item** is kept as an identifier — `Q1079`, whether it arrives so or as a Wikidata address —
linked from the page and editable among the others.

The **format** AniList gives an entry — `TV`, `TV_SHORT`, `MOVIE`, `SPECIAL`, `OVA`, `ONA` or `MUSIC` — is kept
and shown on the page as a chip where it says more than *series* or *film*: an OVA is filed as one or the
other, and the chip says what it is. AniList's **tags** stay beside TMDB's keywords: keywords are gathered
from every source, each once whatever its case, where the first source with any gave the whole list.

## IMDb's titles by region, and its episodes

With IMDb's titles read (`imdb.akas`, a weekly list — see [IMDb's titles by region, and its
episodes](sources.md#imdbs-titles-by-region-and-its-episodes)), each work held keeps the titles IMDb gives
it: one row a title, with its **region** as IMDb writes it — ISO 3166-1 (`FR`), or a code of IMDb's own
(`XWW`, the world in English), none for the original title — its language where IMDb names one, and which of
`imdbDisplay`, `original` and `alternative` it is. They are not the work's alternative titles, and nothing
shows them but what Radarr is sent:

- **Radarr** is sent, after a film's own `AlternativeTitles`, the titles IMDb gives it in the regions of the
  setting `imdb.regions` as it stands for that Radarr — on the server, on its key or on the rule of its
  address — or, unset, in the country of the language it is answered in. Each is sent once, a title Radarr
  is sent already (its title, its original title, a translation's or an alternative title) left out whatever
  its case, spacing and punctuation; fifty at most a film. `Type` is `Tmdb`, as from `api.radarr.video`: Radarr
  files every alternative title under TMDB whatever it is told, and reads no language for one.

| Setting | Default | What it does |
|---|---|---|
| `imdb.akas` | `false` | Read IMDb's titles weekly |
| `imdb.regions` | unset: the country of the Radarr's language | The regions whose titles Radarr is given, `FR, BE, CA`; per key and per address too |

With IMDb's episodes read (`imdb.episodes`, weekly too), each series held keeps the episodes IMDb numbers —
the episode's IMDb id, the series', the season and the number. An episode of the catalogue is the one of IMDb's
with the same season and number in the official order — never a special — save where the numbers are known to
part: numbers IMDb gives two of its own episodes match neither, and a season IMDb numbers fewer episodes of than
are held here, other than the series' last, matches nothing. Such a match is the less sure for resting on
numbers, and is said to be:

- **An episode's `.nfo` document** carries `<uniqueid type="imdb">tt0959621</uniqueid>` after TheTVDB's and
  TMDB's — the default only when it has neither — in the export of the whole library and in
  `GET /api/v1/items/{id}/nfo/{season}/{episode}`.
- **An episode's page** shows IMDb's rating of it beside its own, named *IMDb* and marked *by number*, with a
  link to its page at IMDb and, in its record, what the match rests on.
- **The native API** gives them by work, a season at a time or all at once:
  `GET /api/v1/items/{id}/imdb/episodes?season=1`, answered whoever may see the work.

| Field | Meaning |
|---|---|
| `on` | whether IMDb's episodes are read (`imdb.episodes`); while they are not, `episodes` is empty |
| `episodes[].episodeId` | the episode's id in the catalogue |
| `episodes[].seasonNumber`, `episodeNumber` | its numbers, the official order's |
| `episodes[].imdbId` | IMDb's id of the episode, `tt0959621` |
| `episodes[].rating`, `votes` | IMDb's rating of it out of ten, and its votes — while IMDb's ratings are on (`imdb.enabled`) and it has one |
| `episodes[].matchedBy` | `numbers`: matched by its season and number |

The ratings come from IMDb's daily ratings list, which keeps the rating of every episode IMDb lists of a series
held while the episodes are read, beside the works' — and is read again within the hour after the episodes are.
Sonarr is given none of it: IMDb's episodes stay on the page, in the `.nfo` documents and in this API.
