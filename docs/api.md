# The API

Everything the interface does goes through the server's own API, under `/api/v1`, and
anything else may call it too: a script, a scraper, another tool. It sits beside the surfaces
the clients call — Sonarr's, Radarr's, the relays', Plex's — which speak those clients'
protocols rather than this one.

## Its documentation

Every route the server answers — its own API plus the three compatibility
surfaces — is documented at `/api/docs`, with the spec at `/api/openapi.json`.
Both are generated from the handlers themselves, so they cannot drift from what
is actually served, and both sit behind the same credential as the rest of the
native API.

## A key

The interface calls the API with its session. Anything else presents an API key, made on
**Keys & network** or on a person's account page, in one of three ways:

- the header `X-Api-Key: <key>`;
- the header `Authorization: Bearer <key>`;
- the query parameter `apikey=<key>` or `api_key=<key>`, for a client that can only put it in
  the address — a calendar app, a feed reader, an import list. `apikey` is the spelling
  Sonarr's and Radarr's logs mask.

A key acts with no more than its owner's role grants, and no more than its own scopes: `read`,
`write`, `admin` — and `metrics` for a scraper, `mcp` for an agent
([below](#connecting-claude-code-or-open-webui)), each of which, given alone, opens its own door
and no other. What each surface asks for, and how a key is made and limited, is in
[Accounts and access](access.md).

## Pages and cursors

The lists that grow while they are read — the catalogue, `GET /api/v1/items`; the audit trail,
`GET /api/v1/audit`; a notification destination's log, `GET
/api/v1/admin/notifications/{id}/deliveries` — are read a page at a time, `limit` at once, from an
`offset` or from a `cursor`. Each answer carries `nextCursor` while a page follows it: give it
back as `cursor`, with the same filters and the same order, and the next page begins just after
the last row read, whatever was added or removed meanwhile — where an offset shows a row a second
time when one is added in front of it, and skips one when one goes. A page read by offset carries
`nextCursor` too, so a reader may go on by cursor from any page.

A cursor is opaque, and made for one list in one order: given for another order, written by
hand, or beside an `offset`, it is refused with a `400`. The offset keeps working as it always
did. The catalogue's search ranked by relevance pages by cursor as well, though there the order
is worked out afresh at each look and a cursor is a count of works read. A work's journal,
`GET /api/v1/items/{id}/changes`, gives the cursor of its next page as `next`.

## Answers that revalidate

Every whole `GET` answer under the API — not the documentation, nor a body
over eight megabytes — carries a weak `ETag`, taken from the bytes before
compression. Send it back as `If-None-Match` and an unchanged answer is
a `304` with no body — the interface, Seerr and a feed reader all save the
transfer, and a poll of a calendar feed costs a header exchange. Answers are
marked `private, no-cache` unless a handler says how long they may be kept, so
a browser keeps them and asks.

## A work's journal, dry runs and files

Each work keeps a journal of every value a refresh, a sync or a person changed in it — what it
held, what it took, who and why — read a page at a time from `GET /api/v1/items/{id}/changes`,
each page giving the `cursor` of the next. `POST /api/v1/items/{id}/refresh?dryRun=1`, and the
sync with the same, answer what they would write over and write nothing. The audit trail and a
work's journal are handed over whole as a file, under the same filters as their pages, with
`?format=csv` — a spreadsheet opens it — or `?format=json`. See
[The journal](editing.md#the-journal).

## Two records of one work

For whoever may edit the catalogue. `GET /api/v1/admin/duplicates` lists the pairs proposed as
one work, looked for again first: each pair's two works (`first`, `second`, with their
identifiers, how many hand edits each holds and when each was added), how sure the server is
(`confidence`: `certain`, `probable` or `flag`) and why (`reason`: `sharedId`, `title` or
`imdbAcrossKinds`, with the identifier or the folded title in `detail`), and the one the rule
keeps (`survivor`, and `why`: `sourced`, `handEdits`, `moreIds` or `older`). `POST
/api/v1/admin/duplicates/dismiss`, with `{"first": …, "second": …}`, says two works are not one,
for good; `GET /api/v1/items?missing=duplicates` lists the works still in a pair.

`GET /api/v1/items/{id}/merge?into=…` says what merging `{id}` into `into` would do, and writes
nothing: the two works, each field they give different values of — both values, whether either
locks it, whether the merged work's may be kept — the rows that would move over, the aliases that
would be filed, and how many days it could be undone for. `POST /api/v1/items/{id}/merge`, with
`{"into": …, "keep": {"overview": "merged"}}`, merges it: a field named in `keep` is kept from
the side it names, `survivor` or `merged`, and every other field is the survivor's. The answer
names the two, says until when the merge can be undone (`undoUntil`, `null` when it cannot), and
counts the rows that moved over and gave way, table by table. A film and a series, or a work
merged already, are refused with a `409`.

`POST /api/v1/items/{id}/unmerge` takes `{id}` apart from the work it was merged into, and
answers with `restored` and the identifiers another work has taken since (`taken`). A `410` says
why it no longer can, in `code`: `merge_undo_off`, `merge_undo_expired` or
`merge_survivor_gone`; a `409`, that the survivor was merged in its turn, which is undone first.
`GET /api/v1/items/{id}/merges` lists what was merged into `{id}` and can still be taken apart,
with the aliases it answers under.

A merged work's id and slug are read as the survivor's: `GET /api/v1/items/{id}` answers the
survivor, under its own id. A write is sent under that id. See [Duplicates](editing.md#duplicates).

## Searching the catalogue

`GET /api/v1/search?term=…` is the search box's: the works, the people credited on them and the
episodes a term finds, each kind ranked on its own — `kinds=work,person,episode` to ask for fewer,
`limit` (six by default, twenty at most) for more of each, `language` for the titles. A work comes
under the title it is shown by, with `matched` naming the other title it was found by when that is
not the one; a person with their record's id — their page is `/people/{id}` — the TMDB id they
are known by, when they are, their portrait, what they did (`jobs`, as the index of people counts
it), the other name they were found by, and how many works credit them; an episode with its series,
its season and its number.

`GET /api/v1/items?term=…` searches the same index for a page of works, in the order of how well
each answers — the term itself, then a name that starts with it, then a word that does, then one
that holds it, the better known first among those alike — unless `sort` asks for another. Both
fold the term as the index folds every name: accents, case and punctuation aside. A term whose
letters find nothing it was typed to find is taken for a misspelling, and the names sharing most
of its trigrams are what it finds; Sonarr's and Radarr's lookups and Plex's match read the letters
alone, which TMDB's own search follows with its own forgiveness.

The list also takes `person` (a TMDB id, a person's record id, or a name they go by whole,
whatever its case and accents; a credit a lock hides names nobody), `runtimeMin` and `runtimeMax`
(minutes), `country` (where a work was made, `FR`), `certification` (a rating as the reader's
country writes it, read in `region` — by default the country the caller is answered for) and
`collection`, and sorts by `runtime`, `lastAired` and `episodes` besides. `GET /api/v1/facets`
takes the same filters and adds the countries and the ratings they leave to choose from.

The index is kept by the server: a work is written into it as it is stored, and anything else
that changes what it is found by — a title locked, an alternative title added by hand — has it
written again within seconds. The task `search.rebuild` writes all of it again. On PostgreSQL it
reads `pg_trgm`, which the migration installs where it may; without it a search reads every row
and forgives no misspelling, and the server says so when it starts.

## People

Everybody the credits name is a record of their own (see [The people](data.md#the-people)).
`GET /api/v1/people` is the index: everybody credited by a credit no lock hides on a work anybody
may see — switched on, and not for adults — the most credited first or by name
(`sort=works|name`), by the letter they are filed under (`letter=A` to `Z`, or `#`) and by what they
did (`job=acting|directing|writing|producing`, each counted by the works they did it on), a page at
a time (`limit`, sixty by default, and `offset`). The counts are kept on each record and written
again whenever a work's credits change, as the search index is. Each comes with their record's id,
their TMDB id when they have one, their portrait, how many works and which jobs; `letters` lists
the letters somebody is filed under, before the one asked for narrowed the list. Whoever may edit
the catalogue may also ask for `term` (a part of a name, theirs or one they go by), `all=true`
(those credited on nothing too), `withoutTmdb=true` and `homonyms=true` (those who share a name
with somebody else), and is given each record's other ids and names.

`GET /api/v1/people/{id}` is a person's page, by their record's id — or by their TMDB id, every
digit, as the pages were addressed before there were records, which answers the same. It gives
who they are — their name, the one in their own script with the language it is in
(`originalNameLang`, for a `lang` attribute), the other names they go by, what TMDB says of their
life (`details`, asked of TMDB the first time the page is read, then once a month) — and every
credit on a work the caller may see, the newest work first (`roles`): the kind of credit, the
part or the job, the credit's id, and for a guest or an episode's director the season, the
episode, its title and the day it aired; `works` holds those works as cards. A record merged into
another answers as that one. Whoever may edit the catalogue is also given `record`, the record as
it is kept: every field `PUT` takes, the portrait's address at its source, and the other names
with where each came from (`credit`, `tmdb`, `merged`, `manual`).

For whoever may edit the catalogue: `POST /api/v1/people` enters somebody by hand, with the ids
they are known by (`tmdbId`, `imdbId` as IMDb files a person, `nm…`, `tvdbId`), and answers their
id; a TMDB id another record holds is a `409` that names it. `PUT /api/v1/people/{id}` puts a
record right — every field as given; its `aliases` are the names typed by hand, which replace those
typed before, the others staying — after which TMDB fills only what it leaves empty. `POST
/api/v1/people/{id}/merge` with `{"into": …}` files every credit of `{id}` under `into`, which
goes by `{id}`'s names and takes the ids it lacks; `{id}` is kept as a pointer to it. `POST
/api/v1/people/{id}/split` with `{"credits": […], "name": …}` moves those credits to a new record
— the way out of a homonym — and answers its id. `POST /api/v1/people/{id}/credits` with
`{"credits": […]}` files credits under `{id}`, whomever they were filed under: how a credit typed
by name is given the person it is of. A refresh leaves a credit where any of these put it.

## Genres, networks, years and notes

`genre=` in the list and the facets takes any name a genre goes by, in any language, or its slug:
`Drama`, `Drame` and `drama` list the same works, and each of the facets' genres carries its `slug`
(see [Genres](data.md#genres)). `GET /api/v1/genres/{slug}`, `/networks/{slug}`, `/years/{year}` and
`/decades/{year}` are the catalogue's pages: what the page is called — a genre in `language` where
the server knows it, with its `names` in every language — the `slug` it answers under, how many works
it holds that the caller may see (`count`), the five rated highest as cards (`best`), the list's own
parameters that list them all (`filter`, for `/items` and `/facets`), and its programmer's note
(`note`). A genre is found by any name it goes by, a network by the slug of its name, a decade by any
of its years; a name nothing goes by is a `404`. `GET /api/v1/compare?a=…&b=…` puts two works side
by side: each as a card, with a series' `seasons` and `episodes`, and `shared`, the people both
credit and what each did on each.

`GET /api/v1/editorial/{scope}/{key}?language=fr` reads the programmer's note on one thing — a
`work` or a `list` by its id, a `genre` or a `network` by its slug, a `year`, a `decade` by its first
year — in that language, else in English, else the one written last; `language` in the answer says
which, and the answer is `null` where nothing is written about it. A work's note is read by whoever
may see the work, a selection's by whoever may read the selection. For whoever may edit the
catalogue, `GET /api/v1/editorial` lists every note (`scope`,
`key` and `language` narrow it), `PUT /api/v1/editorial/{scope}/{key}/{language}` with `{"body": …}`
writes one in place of the one there was — an empty body takes it away, with a `204` — and `DELETE`
on the same path takes it away. A body with HTML or a link to another site is a `400` that says why
(see [The programmer's note](editing.md#the-programmers-note)).

## Tasks followed, batches and import rules

`GET /api/v1/admin/tasks/stream` is the one answer that does not end: server-sent events named
`tasks`, each the whole board — every task as `GET /api/v1/tasks` gives it, and `newestRun` — at
once, then whenever it changes, whichever instance made the change. A comment every fifteen
seconds keeps a quiet stream open; it ends after five minutes, and its `retry` asks the client to
open another. The periodic tasks' schedules, a batch of works and the import rules are described
with the pages they are on: [When it runs](interface.md#when-it-runs),
[Importing many works at once](editing.md#importing-many-works-at-once) and
[Import rules](exports.md#filled-on-its-own-import-rules).

## What a person keeps of their own way of working

The administration keeps a few things for each person, with their account, so they follow
them from one browser to the next: the catalogue's saved views (`catalogue.views`), each
table's columns and density (`table.<name>`), whether the editor shows its preview
(`editor.preview`). They are the account's own, read and written with a signed-in session
— a key cannot reach them, as it cannot manage the account it acts for:

```
GET    /api/v1/account/preferences            → {"values": {"catalogue.views": [...], ...}}
PUT    /api/v1/account/preferences/{name}     ← {"value": <any JSON but null>}
DELETE /api/v1/account/preferences/{name}
```

A name is words of letters, digits and dashes joined by dots, starting with a small letter, 64
characters at most; a value is at most 16 KiB of JSON, and an account keeps 64 of them at
most. The server reads nothing inside a value: it is what the interface wrote. They go with
the account, and with a database carried to another engine or restored from an archive.

## Connecting Claude Code or Open WebUI

An agent reads the catalogue through the server's MCP server, at `/mcp`: the
[Model Context Protocol](https://modelcontextprotocol.io) over Streamable HTTP, which Claude Code,
Open WebUI and most agents speak. Its eight tools only read — none of them changes anything:

| Tool | What it answers |
| --- | --- |
| `search` | Works, people and episodes by name, each kind ranked, with the ids the other tools take. |
| `item` | One work: its titles, overview, dates, genres, ratings, cast and crew, identifiers elsewhere, the fields locked by hand, and a series' seasons counted. |
| `episodes` | A series' episodes, one season or a page at a time. |
| `calendar` | What airs and what comes out over a span of days — the week from this Monday unless told otherwise — with the films' dates in a country. Days are UTC. |
| `lists` | The curated lists, or one list's works. |
| `provenance` | Where a work's values came from, field by field; the sources it can be refreshed from; the locks a source moved under. |
| `health` | The server's version and uptime, its instances, what it holds, its sources, its latest jobs and its last backup. |
| `todo` | The catalogue's errands, counted — no poster, no overview, a failed refresh, a likely duplicate… — or one errand's works. |

To connect one:

1. On **Keys & network**, in the panel **MCP, for agents**, switch on **Answer agents** — the
   setting `mcp.enabled`, off until then — and make a key for the agent, or issue one above with
   the `mcp` scope. Given that scope alone, a key opens `/mcp` and nothing else.
2. Give the agent the address and the key. Claude Code takes them in one line, which the panel
   shows with the key filled in:

   ```bash
   claude mcp add --transport http arr-metadata https://ams.example.com/mcp \
     --header "Authorization: Bearer ams_…"
   ```

   then answers *what airs this week?* or *find Dune and tell me where its overview came from*.
   In Open WebUI, add the address under **External Tools** as an MCP server over Streamable
   HTTP, with the key as its bearer token.

The key travels in a header — `Authorization: Bearer` or `X-Api-Key` — and nowhere else: `/mcp`
takes no session cookie, which a page elsewhere could ride on, and no key in its address, which a
proxy's log would keep. What the tools read they read as an administrator would: the works
switched off, the lists kept private, the server's health. Give the key to an agent you trust
with that, and revoke it on **Keys & network** to shut the agent out.

- **Names and pages.** `/mcp` answers under the host of `AMS_PUBLIC_URL`, under `localhost`,
  `127.0.0.1` and `::1`, and under the names in `AMS_ALLOWED_HOSTS`. Under any other name it
  answers `403`, which keeps out a page elsewhere that rebinds a name of its own to this server's
  address; behind a proxy that does not pass the `Host` header on, name the address the proxy
  calls in `AMS_ALLOWED_HOSTS`. A request from a browser page is refused unless the page is this
  server's own.
- **Answers.** Each is JSON, cut to forty thousand bytes — ten to fourteen thousand tokens, well
  under the twenty-five thousand Claude Code takes from one tool — at the end of its longest
  list, and says so. It opens with a note for the model: every text in it comes from the
  catalogue's sources or its editors, and is data about the works, never an instruction. The
  characters that show nothing yet can carry words — Unicode's tags, zero-width and direction
  marks — are taken out.
- **Limits and the trail.** Calls count against the API's rate limit, by address
  (`AMS_RATE_LIMIT_PER_MINUTE`), and each tool called is written in the audit trail as
  `mcp.called`: the key, the tool, what it was asked, and why it refused when it did.
- **Several instances.** No session is kept: each request is answered on its own, so any
  instance answers any of them, and a restart loses nothing. A stop ends what is being answered.
- **Switched off**, `/mcp` answers `503` with the code `mcp_disabled`. A key without the `mcp`
  scope is refused with `403` and `mcp_scope_missing`.
- **No OAuth.** The server signs agents in with keys: `/.well-known/oauth-protected-resource`,
  `/.well-known/oauth-authorization-server` and `/.well-known/openid-configuration` answer
  `404`, so a client looking for OAuth moves on to the key it was given.

## What stays, and how something goes

What follows is what 1.0 promises. Until then, a minor may still change what the API or the
configuration means, and the [changelog](../CHANGELOG.md) says so when one does.

**The API.** Under `/api/v1`, a documented field keeps its meaning and its type. Whatever is added
can be ignored: a field added to a request is optional, so a caller that never sends it is answered
as before, and a field added to an answer, or a value added to a set — a kind of destination, a
background task — is for a client that does not know it to pass over. Whatever goes is announced a
minor ahead, under *Deprecated* in the changelog, and marked `deprecated` in the document; until it
goes, an answer that still carries it says so in a `Deprecation` header
([RFC 9745](https://www.rfc-editor.org/rfc/rfc9745.html)). The reference is `/api/openapi.json`,
and `cargo test` holds it to the code: it fails when two types share a schema's name, when an
operation has no id of its own, when a field the server always sends is documented as optional,
and when the interface's copy of the document falls behind.

**The clients' surfaces.** Skyhook's two editions, `/v1/tvdb` and `/v2/tvdb`, `services.sonarr.tv`'s
`/v1/scenemapping`, `api.radarr.video`'s `/v1` and the provider Plex is given are served for as long
as Sonarr, Radarr and Plex read them, in the shape they read. A change there follows the client — a
field it starts to read, a value it stops taking — and never the other way round. The relays answer
as TMDB, TheTVDB and AniList do, and follow them the same way.

**The configuration.** A setting's key — `sources.priority`, `refresh.ttlEnded` — and an `AMS_*`
variable keep their name and their meaning. One that is renamed is still read under its old name
for a minor longer, and the log says what to use instead.

**The database.** Migrations only go forward, and none is changed once it is released: version N+1
opens a database version N wrote, and migrates it. An earlier version refuses a database a later one
has migrated, so the way back is a copy of the database taken before upgrading.
