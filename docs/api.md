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
`write`, `admin`. What each surface asks for, and how a key is made and limited, is in
[Accounts and access](access.md).

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

## Searching the catalogue

`GET /api/v1/search?term=…` is the search box's: the works, the people credited on them and the
episodes a term finds, each kind ranked on its own — `kinds=work,person,episode` to ask for fewer,
`limit` (six by default, twenty at most) for more of each, `language` for the titles. A work comes
under the title it is shown by, with `matched` naming the other title it was found by when that is
not the one; a person with the TMDB id their page is filed under, when the credits give one, and
how many works credit them; an episode with its series, its season and its number.

`GET /api/v1/items?term=…` searches the same index for a page of works, in the order of how well
each answers — the term itself, then a name that starts with it, then a word that does, then one
that holds it, the better known first among those alike — unless `sort` asks for another. Both
fold the term as the index folds every name: accents, case and punctuation aside. A term whose
letters find nothing it was typed to find is taken for a misspelling, and the names sharing most
of its trigrams are what it finds; Sonarr's and Radarr's lookups and Plex's match read the letters
alone, which TMDB's own search follows with its own forgiveness.

The list also takes `person` (a TMDB id, or a name whole, whatever its case and accents),
`runtimeMin` and `runtimeMax` (minutes), `country` (where a work was made, `FR`),
`certification` (a rating as the reader's country writes it, read in `region` — by default the
country the caller is answered for) and `collection`, and sorts by `runtime`, `lastAired` and
`episodes` besides. `GET /api/v1/facets` takes the same filters and adds the countries and the
ratings they leave to choose from.

The index is kept by the server: a work is written into it as it is stored, and anything else
that changes what it is found by — a title locked, an alternative title added by hand — has it
written again within seconds. The task `search.rebuild` writes all of it again. On PostgreSQL it
reads `pg_trgm`, which the migration installs where it may; without it a search reads every row
and forgives no misspelling, and the server says so when it starts.

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
