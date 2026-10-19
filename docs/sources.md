# Where the data comes from

Four sources are asked at once and merged into one entity:

| Source | Brings |
| --- | --- |
| **TMDB** | overviews, stills and translations, in the configured language |
| **TheTVDB** v4 | the season and episode numbering, absolute numbers, air-order hints, broadcast time of day, per-country certifications |
| **Skyhook** (Sonarr) / **api.radarr.video** | the same view the arr stack would have got on its own, including ratings it carries and TMDB does not |
| **Fanart.tv** | transparent logos, clearart and banners — artwork only |

Only TMDB is really needed to start. The rest are optional and on by default;
each costs one extra call per refresh.

The merge is field by field, in the order of the setting `sources.priority` — put
in order on the Sources page, and given its first value by `AMS_PROVIDER_PRIORITY`:

- **Scalars** take the first provider that has anything, so a lower-priority
  source fills a gap rather than overwriting an answer.
- **Artwork, alternative titles, ratings and translations** are unioned.
- **Credits** come from one provider; nothing identifies a person across
  providers, so a union would list the same actor twice.
- **Episodes and seasons** are the exception, and the one rule worth knowing.

## Whose episode numbering wins

The **list** of episodes and seasons always comes from TheTVDB, or from Skyhook
which republishes its numbering. Every other provider may fill fields on those
episodes; none may add one. The one exception is a series TheTVDB stopped
answering for, refresh after refresh: TMDB then numbers it in TheTVDB's place,
until TheTVDB answers again — see
[Who numbers a series](sonarr-radarr.md#who-numbers-a-series).

This is not configurable, because it is not really a preference. A client
addresses a series by its TVDB id and expects the numbering that goes with it,
and the providers do not agree on where seasons end — so serving TMDB's
boundaries under a TVDB id makes Sonarr map files to the wrong episodes.

Taking the union of both is worse still: episodes that are the same hour of
television sit at different numbers in each scheme, so none of them match and
every one is kept twice. That is not hypothetical — it is what this server did
before the rule existed, and for *One Piece* it answered with 2352 episodes for
a show that has 1179.

Measured against the live APIs with all four providers configured:

| Series | Numbered episodes | With absolute numbers | With overviews |
| --- | --- | --- | --- |
| Breaking Bad | 62 | 62 | 62 |
| Attack on Titan | 89 | 89 | 89 |
| One Piece | 1179 | 1178 | 1178 |

Removing TMDB and leaving the other three changes none of those counts — that
is the point of the rule — but costs the artwork TMDB carries: Breaking Bad
drops from 82 images to 69.

A real Sonarr 4.0.20 served by this server stored those same counts with no gap
in the absolute numbering. A real Radarr 6.4.4 searched by id and by text and
added films with their studio, genres, certification and IMDb/TMDB/Trakt
ratings. Jellyseerr's ten TMDB endpoints all answer through the relay.

**Enrichment** is asking a provider on every fetch and merging what it says.
It is on by default and costs one extra call per refresh per provider. **Fallback**
is asking only when nothing else could answer, so with enrichment on it never
fires — the answer is already in hand, and a second call would buy it twice.

Both are settings, changed under **Settings** without a restart; the `AMS_*`
variables only seed them the first time the server starts.

A search is a ladder rather than a fan-out, because search results are cheap to
get and expensive to merge: what is already stored, then TMDB, then TheTVDB, then
Skyhook — each rung tried only while the ones above it found nothing. So a series
TMDB has never heard of is still found, and a deployment with no TMDB key at all
still answers.

> If you redirect `skyhook.sonarr.tv` or `api.radarr.video` at this server
> through your **resolver** rather than per container, it resolves those names
> to itself and would call itself forever. It detects that and answers `508`
> with the fix in the message — but point the upstreams elsewhere, or turn
> enrichment off, when that is your setup.

### TMDB's episode groups

The aired order is not the only one a series is numbered by: TheTVDB keeps a
DVD order and others beside it, and TMDB's members group a series' episodes
otherwise too — an original air date, an absolute order, a DVD's, a streaming
service's, a story's, a production order, a channel's: TMDB's types 1 to 7. The
groups come with the series at each refresh, in the same call
(`append_to_response=episode_groups`), each kept as an order still to be read,
under `tmdb-` and its id, with its name and the count of episodes TMDB lists it
with. A group is read — one more call to TMDB, once — by the refresh that lists
it, four at a time; read again when the count TMDB lists it with moves. One that
could not be read then is read the first time it is asked for, by Sonarr v5 in
its season type, by Plex in its episode order or by the TheTVDB relay, or at the
next refresh: the editor and a season's page show what is kept, and ask TMDB
nothing. A group of a type this server does not know is left out.

A group's episodes are TMDB's own: each names the episode the series holds by its
TMDB id, or is served as TMDB describes it where the series holds none. A group
cannot cut or join episodes, and neither can an order here.

TheTVDB and TMDB often keep the same order twice — a DVD order, an absolute one.
Clients are offered it once: the one from the source that numbers the series
(TheTVDB, unless TMDB numbers it in its place) always is, and the other's beside
it only when it holds more episodes; one of the aired kind is offered beside the
aired order only when it holds more episodes than the series does. TheTVDB's further orders —
`alternate`, `regional`, `altdvd`, `alttwo`, each the series' own — and the orders
made here are each offered whatever else there is. One not offered is still
answered when it is asked for by its key. See [Orders](sonarr-radarr.md#orders).

## Who gave what, and asking one source again

Every refresh records, field by field, which provider's value was kept, which
others said the same and which said otherwise; the pictures are counted by
source, and the episode list names whoever numbered it. The work editor shows
it beside each field: the provider's name, **Multi-sources** with the count
when several agree (Skyhook is not counted beside the TheTVDB it
republishes), **Locked** for an edit by hand, **Kept** for a value no source
gives any more, **No source** when nobody gave one. None of this reaches a
visitor. **Administration › Sources** lays out the rules themselves: who can
supply what, in the configured order, and who replaces the rest.

The editor's **Sources** panel asks chosen providers again, and only them. The
others are handed back what they gave last time, so the ordinary merge weighs
the fresh answers against them by the ordinary priority: a sync from TheTVDB
alone takes its new runtime without handing it a title TMDB outranks it on.
Locked fields do not move, the refresh schedule and the work's identifiers
stand, and the episode list stays whoever numbered it unless that provider
gives one anew. Some sources come along with the ones asked: TVmaze with the
provider that numbers the episodes, whose broadcast instants it corrects, and
that provider with any source that fills the episodes in, so the list is
merged as a refresh merges it. A sync that finds the work written by something
else meanwhile — a refresh, another sync — writes nothing and says so. A work
refreshed before this was recorded needs one full refresh first. The same through the API: `GET /api/v1/items/{id}/provenance`,
`POST /api/v1/items/{id}/sync` with `{"sources": ["tvdb", "fanart"]}`, and
`GET /api/v1/sources/rules`.

## Further sources

Six more, each off until switched on under **Settings → Further sources**.
None needs a key.

| Source | Brings | Costs |
| --- | --- | --- |
| **TVmaze** | the moment each episode aired | two calls per series fetch |
| **AniList** | its score, the romaji, native and English titles and synonyms, the main studio, an adult flag — for anime | one call per anime fetch |
| **MyAnimeList** | its score and titles — for anime; through its own API when `AMS_MAL_CLIENT_ID` is set, through [Tenrai](https://api.tenrai.org/documentation) otherwise, a mirror written to Jikan's schema (`AMS_TENRAI_UPSTREAM`), and through a Jikan of your own (`AMS_JIKAN_UPSTREAM`) when neither answers (`mal.fallback`, on) — Jikan's public API closed on 1 October 2026 | one call per anime fetch |
| **IMDb** | IMDb's rating, for every work with an IMDb id | one download a day |
| **Fankai** | the Fan-Kai productions — anime recut into films, each as a series with its sagas as seasons — from Fankai's own metadata service | one call per saga, and three more per production fetch |
| **Wiki Fankai** | for each Fan-Kai, the anime it was cut from and the Fan-Kai that follows it — only while Fankai is on | one call per Fan-Kai fetch |

TVmaze's servers take fifteen to twenty seconds, now and then, over what their
cache does not hold — the first lookup of a series — whatever the pace of the
calls. A refresh or a seed gives TVmaze a minute and asks once more after a
pause; a client waiting on this server gives it twenty seconds, and the fetch
goes without it. Its API keys are not asked for: they open its user API, a
premium member's own data, and change nothing of the public one's limit, which
is per address.

None of the first four changes the shape of an answer: Sonarr and Radarr are
served the same fields as before, some of them now more accurate. Fankai adds
works instead — ones no other source has.

**TVmaze corrects `airDateUtc`**, the moment after which Sonarr counts an
episode as aired. TheTVDB keeps one broadcast time per series and Skyhook stamps
it on every episode, so a show that changed slot is wrong for everything before
the change. TVmaze keeps a time per episode. Measured against Skyhook: all 62 of
Breaking Bad's episodes are placed at 21:00 ET, where 54 aired at 22:00 — the
last eight did air at 21:00, and on those it is TVmaze that is an hour late —
and 35 of The Big Bang Theory's move, season 3 having aired on Mondays at 21:30
rather than Thursdays at 20:00. Its time is taken only where its broadcast date
matches TheTVDB's for the same season and episode. It never adds, removes or
renumbers an episode.

**AniList and MyAnimeList** are asked about one entry: the one that stands for
the whole work, as the [Fribb anime-lists](https://github.com/Fribb/anime-lists)
id map places it — the first season's, for a series both sites split by cour.
The map is downloaded weekly while either source is on, and it also answers
Sonarr's `mal:` and `anilist:` searches, which is how its AniList and
MyAnimeList import lists find a series; those used to find only series already
stored here. Neither site is ever used for numbering. Sonarr ignores
alternative titles, so the romaji ones serve Radarr, which matches anime film
releases against them, and this interface. For anime, their main studio
replaces TMDB's, which is only the first production company TMDB lists.

**IMDb's rating** is what Sonarr is given for a series — Skyhook's rating is
IMDb's, republished — and otherwise the rating with the most votes behind it.
The daily list keeps that figure current and supplies it when Skyhook and
Radarr's service are off. Only the ratings of works stored here are kept, and
the list is fetched again within the hour when works are added.

**Fankai lists the Fan-Kai productions**: anime recut into films by the
[Fankai](https://fankai.fr) team, which neither TheTVDB nor TMDB carries. Each
production is a series, its sagas are its seasons, and its films are its
episodes, numbered as Fankai names its files — `Horimiya
Kaï.S01E02.MULTI.1080p.x265-FANKAI.mkv` is season 1, episode 2, and a second
saga carries on from the first: `S02E08` is the eighth film. A production turns
up in Sonarr's search under its own name — *Naruto Shippuden Yabai*, *Black
Lagoon Henshū* — or as `fankai:` and its id, and on the import page; nothing is
fetched until it is asked for, so Fankai's catalogue never lands here whole.
Sonarr is handed an id of this server's own, Fankai's plus 200 000 000, the way
a TMDB-only series gets one; Plex, Jellyfin and Kodi get the production through
the NFO export, with Fankai's id as a `uniqueid` and its theme music beside
it as `theme.mp3`, which a work's page also plays on demand. The kaïeur and the voice cast
are its credits; the kind of recut — Kaï, Yabai, Henshū — is a tag rather than
a genre. One call a second, and the listing is kept and revalidated by its ETag.

Fankai's id is its metadata service's. The website numbers its productions
otherwise — *Horimiya Kaï* is series 33 of the one and production 101 of the
other, where 33 is *Black Lagoon* — behind a sign-in, and nothing public says
which page is which. So the id is shown without a link, and a production's
homepage is its page on the Fankai wiki below, when that source is on.

**The Fankai wiki says what each Fan-Kai was cut from.** Fankai's service
leaves a production's ids blank; the community's wiki, at
[fan-kai.fandom.com](https://fan-kai.fandom.com/fr/), links each one's AniList
and MyAnimeList entries and names the Fan-Kai that follows it. With it on, a
Fan-Kai's page leads to its anime — to the anime's own page once Sonarr holds
it — and to the next Fan-Kai, and the anime's page leads back to every Fan-Kai
cut from it. The wiki keeps a page per cut, so a production is matched on its
name and on its kaïeur, never on a guess between two. With AniList on too, the
anime's title, year and poster come from there. The ids are never given to the
Fan-Kai itself: sharing one would merge it into the anime.

The lists and their last download are shown under the switches, with a button
to download one now rather than wait for its turn, and every download is a job.

## Terms of use

Each source answers under terms of its own, and the catalogue is built to stay
within them. What this server does with a source's data is the same for every
one: it keeps a copy of what the source answered for the works it holds — a
work is fetched when a client or a person asks for it, never a catalogue
wholesale — refreshes that copy on the schedule above, hands it to the clients
it serves, and never feeds it to a model or uses it to train one. Since 0.8.0
the TMDB, TheTVDB and AniList relays can also pass a client's own key through,
so a client that holds one answers under its own agreement rather than the
operator's. Every public page credits the sources that are on, with the
notices TMDB and IMDb require; **Administration › Sources** names each source's
terms with a link to them.

- **TMDB** allows non-commercial use with its notice shown to readers, and
  nothing kept for longer than six months. The refresh intervals can be set
  longer than that — an ended series is kept a week by default and may be kept
  ten years — so the sweep makes up the difference on its own: a work whose
  last refresh is more than 180 days old is asked of TMDB again, by a *light
  refresh* — TMDB alone, as a sync asks it, the other sources keeping what they
  last gave — taken after the works that are due, within the same batch. A work
  without a TMDB id, one entered by hand or one switched off is left alone, and
  one TMDB does not answer for is tried again, in full, after the usual backoff.
  The run's entries say which works were refreshed lightly, and the
  [`refresh.ttlContinuing`](settings.md#refreshttlcontinuing) and
  [`refresh.ttlEnded`](settings.md#refreshttlended) settings say so under the
  field. TMDB's logo, which its terms ask for beside the notice, is a brand
  asset TMDB distributes itself; the footer carries the notice and a link.
- **TheTVDB** is free below $50,000 a year of revenue, with a credit and a
  direct link shown to end users: the footer carries both.
- **TVmaze**'s data is CC BY-SA 4.0, credited on every page with the licence
  named.
- **AniList**'s API is free for non-commercial use, and forbids hoarding or mass
  collection, use as a backup or a data store, and use within a competing list
  or tracker service: this server asks about one work at a time, for the works
  its clients hold, and lists nothing of a person's.
- **MyAnimeList**'s API agreement forbids redistributing the API. Jikan, the
  unofficial mirror this server asked when no client id was set, closed its
  public API on 1 October 2026; the chain is now the official API, then
  [Tenrai](https://tenrai.org), a mirror written to Jikan's schema, then a Jikan
  address for whoever runs their own. Tenrai's terms allow caching and private
  proxies and forbid offering it to third parties as a competing API; the footer
  names it while it serves.
- **Fanart.tv** asks for a project key and credit where its pictures are shown.
- **IMDb**'s datasets are for personal, non-commercial use, with the sentence
  IMDb asks for on every page.
- **Fankai** publishes its metadata for its own productions, and is asked about
  one at a time; the Fankai wiki's content is CC BY-SA.

Each source has its own switch, and a source switched off is asked nothing and
credited nowhere; what it gave while on stays as it is. The further sources are
settings — `tvmaze.enabled`, `anilist.enabled`, `mal.enabled`, `imdb.enabled`,
`fankai.enabled` and `fankai.wiki`, under **Settings › Further sources** — as
are Skyhook's and Radarr's services (`skyhook.enrich`, `skyhook.fallback`,
`radarr.enrich`, `radarr.fallback`). TheTVDB and Fanart.tv are on while they
have a key and `AMS_TVDB_ENABLED` or `AMS_FANART_ENABLED` is not `false`; TMDB
is on while it has a key.
