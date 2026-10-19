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
