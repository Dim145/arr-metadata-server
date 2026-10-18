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
