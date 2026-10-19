# The relays: TMDB, TheTVDB and AniList

Clients that ask TMDB, TheTVDB or AniList themselves — Jellyseerr and Overseerr,
Yamtrack, Jellyfin's TheTVDB plugin, the anime trackers — reach those services
through this server once their names resolve here. Each request is handed on to
the real service, and its answer is handed back with the fields locked in this
catalogue written in. The routes are the TMDB relay (`/3/*`, and the public lists of
`/4/list/*`), the TheTVDB relay (`/v4/*`) and the AniList relay
(`graphql.anilist.co`, on the clients' door).

[Pointing your stack at this server](integration.md#tmdb-clients) says how to
redirect a client and make it trust the certificate. This page says whose key the
services are asked with, what is written into their answers, and what is taken out
of them.

The TMDB and TheTVDB relays read and do not write: a request that is not a `GET`
or a `HEAD` is refused with `403` whoever asks, TheTVDB's sign-in aside.

## Whose key the service is asked with

Which key asks TMDB or TheTVDB is a choice on **Opening & APIs › Whose key the
relays ask with**, one for each relay, written the moment it is made:

| Setting | Values | Default |
|---|---|---|
| `tmdb.relayCredential` | `server`, `client` | `server` |
| `tvdb.relayCredential` | `server`, `client` | `server` |

**`server`** is what the relays always did. The operator's key serves every client.

- *TMDB.* This server's key (`AMS_TMDB_API_KEY`) is used in place of whatever the
  client sent. A key issued here, given to the client as its TMDB key,
  authenticates here and never reaches TMDB. With no key of its own, the relay
  answers `503`.
- *TheTVDB.* A client signs in (`POST /v4/login`) with a key issued here and is
  answered that same key as its token; TheTVDB is asked with this server's own
  token (`AMS_TVDB_API_KEY`, `AMS_TVDB_PIN`). A client's own TheTVDB key, let
  through by address (`AMS_TVDB_AUTH=allowlist`), is signed in with at TheTVDB and
  its token travels as it came.

**`client`** hands each client's own key on, as it came, and never uses the
server's:

- *TMDB.* The client's own v3 `api_key` in the query, or its v4 read token in
  `Authorization`.
- *TheTVDB.* Every client signs in with its own TheTVDB key; the sign-in and the
  token travel as they came.

With `client`:

- **A key issued here is refused.** Wherever it is presented — in the query
  (`api_key`, `apikey`), in `Authorization` under any scheme and in any case, in
  `X-Api-Key`, or in a TheTVDB sign-in — the answer is `401` with
  `{"error": "own_key_required", "message": …}`, before the service is asked
  anything. It is never handed on beside a client's own token. A request with no
  key at all is passed on, and the service answers it as it answers any.
- **Nothing is kept for another client.** What a client's own key was answered is
  that client's: the relay's cache holds only what the server's key was answered.
- **The surface is guarded by address.** A key issued here being refused there,
  `AMS_TMDB_AUTH=apikey` or `AMS_TVDB_AUTH=apikey` is read as `allowlist` for that
  relay: the addresses listed on **Keys & network** guard it. `allowlist` and `open`
  stay what they are. The Opening & APIs page says so under the choice, and the
  relay's row, the configuration page and the dashboard show the policy that
  applies.
- **The locks are written in all the same.** The choice is about whose key asks,
  not about what is answered.

`AMS_TVDB_AUTH` keeps saying how the TheTVDB surface is guarded. Under `server`,
`apikey` has a key issued here sign in and `allowlist` lets a client's own key
travel too, with a key issued here still standing in; `server` is what both do, and
what every deployment starts on. `tvdb.relayCredential = client` refuses a key
issued here and guards the surface by address, whatever the variable says.

Which to choose: with `server`, the operator's key serves every client and the
operator answers to TMDB and TheTVDB for that use — their terms, their quotas. With
`client`, each client uses its own key and its own quota, and the relay only writes
the corrections in. A member's key is kept off every relay until **Members too** is
on, on the same page (`api.tmdbMembers`).

## What is written into the answers

Only what a person **locked** here, and only into fields the service's own
document carries; a field nobody locked is the service's.

A text — a title, an overview — is written as it is locked for the language the
client asked (TMDB's `language=`, `en-US` when none is; TheTVDB's language in the
path or in the translation):

- in the language the catalogue is stored in (`AMS_TMDB_LANGUAGE`, by its language
  whatever the region: `fr` and `fr-CA` are French), the text locked in that
  language, else the text locked in every language;
- in another language, only a text locked in that language itself. A text locked in
  every language — which is every lock made without a language, so every lock made
  before 0.8.0 — is the catalogue's own, and the service's document in another
  language keeps the service's text: a German Jellyseerr is still answered TMDB's own
  German.

A date, a runtime or a chosen picture is written into every document. TMDB's
homepage and the genres' names are written only for a client asking in the
catalogue's language, since TMDB names genres in the language asked; the genres keep
TMDB's own ids, and when a genre locked here is not among TMDB's for that title,
TMDB's are left as they are.

**TMDB** (`/3/*`):

| Path | Written |
|---|---|
| `/3/movie/{id}`, `/3/tv/{id}` | title, original title, overview, dates, status, runtime, chosen poster and background; the homepage and the genres as above |
| `/3/tv/{id}/season/{n}` | the season's name, overview, date and chosen poster; each episode's title, overview, date, runtime and still |
| `/3/tv/{id}/season/{n}/episode/{e}` | the episode's title, overview, date, runtime and still |
| `/3/search/{movie,tv,multi}`, `/3/trending/{movie,tv,all}/{window}`, `/3/discover/{movie,tv}` | each result the catalogue holds with a lock: title, original title, overview, date, poster, background. People are left alone |

A regular episode is paired with the catalogue's by its numbers, a special by the
TMDB id the catalogue gave it, and neither when the catalogue knows it under another
TMDB id. Pictures are written by TMDB's own path, so only a picture TMDB has can be
named. TMDB's answers are kept a while and shared by every client asking with the
server's key; see
[TMDB clients](integration.md#tmdb-clients).

**TheTVDB** (`/v4/*`):

| Path | Written |
|---|---|
| `series/{id}`, `series/{id}/extended`, `movies/{id}[/extended]` | name, overview, year, dates, runtime, status, chosen poster. With `meta=translations`, every language's entry, and an entry for the catalogue's language and for each language a text was locked in alone where TheTVDB has none, as Jellyfin's plugin reads them; with `meta=episodes`, the episodes; the seasons of the aired order, with their locked titles |
| `series/{id}/translations/{lang}`, `movies/{id}/translations/{lang}` | name and overview, as locked for that language |
| `series/{id}/episodes/{order}[/{lang}]` | each episode's title, overview, date, runtime and still, told by its TheTVDB id in every order (`official`, `default`, `dvd`, `absolute`…), and by its numbers too in the aired order. This is what Seerr's TheTVDB option reads |
| `seasons/{id}`, `seasons/{id}/extended` | the season's locked title as its name, its chosen poster, its episodes |
| `episodes/{id}[/extended]`, `episodes/{id}/translations/{lang}` | the episode's title, overview, date, runtime and still, and its translations |

A TheTVDB record names a work in its original language, which no client asks for by
name: its own `name` and `overview` take the lock as the catalogue holds the work,
whatever language that is in, and the translations it carries take each what was
locked for their language. `seasons/{id}/translations/{lang}` is not written: it
does not say whose season it is.

A series' episodes in an order the catalogue keeps and TheTVDB does not — a TMDB
group's, `tmdb-…`, one made here, `ams-…` — are answered here, as TheTVDB writes a
page of them, every episode on the first; so are the aired order's (`official`,
`default`) and the episodes a series' record carries with `meta=episodes`, while
an order is frozen as the aired numbering; and an order of TheTVDB's own it
answers with no episode — `altdvd`, `alttwo` (thetvdb/v4-api#340) — is filled with
what was read of it season by season. See
[Orders](sonarr-radarr.md#orders).

**AniList** (`graphql.anilist.co`): the title, description, genres and pictures of
each entry the answer carries. AniList has no key to choose.

## What is taken out

A credit, a picture or an alternative title that a source gave and a person
[hid](editing.md#hiding-and-correcting-what-the-sources-gave) is taken out of the
lists the relays hand on, each told by what the service itself names it by:

| Relay | List | Told by |
|---|---|---|
| TMDB | `cast` and `crew` of `/credits`, a series' `/aggregate_credits` and of both asked with `append_to_response`; a series' `created_by` | the person's TMDB id and the kind of credit: the cast are actors; of the crew, a `Director` job is a director, a `Producer` job a producer, any job of the `Writing` department a writer, as the catalogue files them; a creator is a writer |
| TMDB | `posters`, `backdrops`, `logos` of `/images`, a work's or a season's, and of `images` asked with it | the path TMDB files the picture under |
| TMDB | `titles` (a film's) or `results` (a series') of `/alternative_titles`, and of `alternative_titles` asked with it | the text, case and blanks aside, and the country TMDB files it for |
| TheTVDB | `artworks` of a series' or a film's record, and a series' `/artworks`; a season's `artwork` | the picture's address |
| TheTVDB | `aliases` | the text, case and blanks aside, in whatever language |
| TheTVDB | `characters` of a series' or a film's record | the person's name, case and blanks aside, and the kind of credit TheTVDB gives |

What cannot be told apart safely is left as the service gives it, rather than
guessed at:

- a credit a source named by name alone — one of Skyhook's, Fan-Kai's or Radarr's
  service's, which give no TMDB id — is not looked for in TMDB's lists: TMDB names
  every person by an id, and a name could be somebody else's;
- TheTVDB's people share no id with TMDB's, so they are told by name: a namesake
  in the same role on the same work goes with the person hidden;
- a season's or an episode's people — TMDB's season credits, an episode's guest
  stars and crew, TheTVDB's episode characters — are not the work's credits, which
  are all a person can hide: they stay;
- the picture a document leads with (TMDB's `poster_path` and `backdrop_path`,
  TheTVDB's `image`) stays when it names a hidden picture, since another in its
  place would be a choice nobody made: choose the picture to lead with, and the
  relays write it;
- a credit that was corrected — its character, its place — is written into no
  relayed list: hiding is all the relays carry of a credit;
- a title TMDB lists for several countries is kept once by the catalogue, under the
  first; hidden, it goes from TMDB's list under that country alone.

## Answers worth knowing

| Answer | When |
|---|---|
| `401`, `own_key_required` | With `client`, a key issued here was presented. |
| `401` | Under a key policy, no key this server takes was presented. |
| `503` | The relay is switched off (`api.tmdb`, `api.tvdb`, `api.anilist`), or with `server` this server holds no key of its own to stand in with. |
| `403` | A member's key while **Members too** is off (`relay_not_for_members`), or a write, or TheTVDB's operator account asked for with a key issued here. |
| `508` | The relay was asked by an instance of this server: the service's name resolves back here. Set `AMS_TMDB_UPSTREAM` or `AMS_TVDB_UPSTREAM`; see [If you redirect at the resolver](integration.md#if-you-redirect-at-the-resolver). |
