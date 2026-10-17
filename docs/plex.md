# Plex

Plex Media Server 1.43 and later can take a library's metadata from a *custom
metadata provider*: an HTTP API that Plex is given by its address. This server
is one. Plex asks it what each file it scanned is, then for the whole of what
it matched — a film; a series with its seasons and its episodes — and shows
what the catalogue holds: its titles, its summaries, its chosen posters, and
every correction locked here.

The provider is at `/plex` on the interface's door (`AMS_BIND_ADDRESS`). It is
not on the clients' door (`AMS_CLIENTS_BIND`): Plex is given an address to call,
and no name is compiled into it for the door to answer under.

Plex is a client of this server and never one of its sources. Two other routes
reach Plex as well, and this page says when to take them instead:
[the `.nfo` export and Kometa's metadata file](exports.md).

## Adding the provider to Plex

1. In the administration, open **Keys & network** (`/admin/clients`). Its
   **Plex** panel shows the address to give Plex — `/plex` after `AMS_PUBLIC_URL`
   when that is set, otherwise after the address the page was opened at — with a
   copy button. Set `AMS_PUBLIC_URL` to the address your Plex server reaches this
   server at, when that is not the one your browser uses.
2. Allow Plex's address. Plex sends no credential to a provider, so it is let
   in by the address it calls from, as Sonarr and Radarr are: add the address of
   your Plex Media Server — `192.168.1.20`, or its container network, such as
   `172.18.0.0/16` — under **Allowed addresses**, on the same page, just below
   the panel. A Plex that was refused is listed under **Who has been calling**,
   with a button that allows it. Behind a reverse proxy, name the proxy in
   `AMS_TRUSTED_PROXIES`, as
   [for Sonarr and Radarr](integration.md#5-allow-the-clients-through).
3. In Plex, under **Settings › Metadata Agents**, add a provider with that
   address, make an agent with it as the primary provider, and choose that agent
   in a library's advanced settings.

The list of allowed addresses is one list, shared by every route that is guarded
by address: an address allowed for Plex may call Sonarr's and Radarr's routes too.

The panel's switch is the setting `api.plex`, on by default. Off, the provider
answers `503` with the code `surface_disabled` to everything Plex asks, which
Plex takes for "try again later". The **Opening & APIs** page lists the provider
with what it asks for and how many calls it has answered and refused since the
start; `ams_http_requests_total` counts it under `surface="plex"`.

## Keeping it on your network

Plex cannot yet prove who it is to a provider, so whatever the list of allowed
addresses lets in can read the catalogue through `/plex`. Do not publish the path
beyond your local network: put only your Plex server's address on the list, and
keep a reverse proxy from forwarding `/plex` to the internet. A Plex elsewhere —
on another network, or behind an address that changes — is given a key instead
([below](#a-key-in-the-address)).

`AMS_PLEX_AUTH` sets what the provider asks for, as `AMS_ARR_AUTH` does for
Sonarr's and Radarr's routes:

| Value | Meaning |
|---|---|
| `allowlist` (default) | The caller's address must be on the list of allowed addresses. An address with a key in it, `/plex/<key>`, is refused with `403` and the code `plex_key_unused`. |
| `open` | Anything that reaches the address is answered, an address with a key in it too. The panel says so. |
| `apikey` | A key issued here is asked for, carried in the address Plex is given: `/plex/<key>`. |

`AMS_AUTH_DISABLED=true` opens every surface, this one included. The provider is
rate limited like the others (`AMS_RATE_LIMIT_PER_MINUTE`, per address); a
refusal says when to come back in `Retry-After`.

## A key in the address

Plex sends no credential to a provider and keeps no query string, but it calls
the paths of the manifest under whatever address it was given. With
`AMS_PLEX_AUTH=apikey`, that address carries a key issued here:

```
https://ams.example.org/plex/ams_…
```

and Plex calls `/plex/ams_…/library/metadata/…` under it. The key is checked as
any key is — enabled, not expired, its owner active — and Plex is answered from
wherever it calls: no address of it has to be on the list. The settings a key can
carry apply to Plex as to any client, its language and its country among them.

1. In **Keys & network**, the **Plex** panel shows the address with `<key>` in it
   and a button, **Make a key for Plex**: it makes a key that only reads, named
   Plex, and shows the whole address once — only a hash of the key is kept, so
   copy it then. A read-only key of your own can go in place of `<key>` instead.
2. Give Plex that address, as in [step 3 above](#adding-the-provider-to-plex).
3. To shut Plex out, revoke the key in the list on the same page.

A Plex that already has the provider — added at `/plex`, or with a key since
revoked — is given the new address in place: Plex refuses to remove a provider an
agent uses, but its API changes the address,
`PUT /media/providers/metadata/{id}?uri=<the new address>` (the provider's id is in
`GET /media/providers/metadata`). Then restart Plex Media Server: until it is
restarted, its agents keep calling the address the provider had when Plex started,
and a refresh there is refused.

`/plex` with no key is refused (`401`), and a key in a header (`X-Api-Key`) is
still taken, for a caller that can send one. Logs never hold the key: the
server writes the path as `/plex/***/…`. Plex does not do as much — it keeps the
address in its database and may write it in its own logs — so keep the key for
Plex alone, and give Plex an `https` address when it reaches this server across a
network you do not trust.

## What is served

| Plex asks | Route | Answer |
|---|---|---|
| The manifest | `GET /plex` | The identifier `tv.plex.agents.custom.arrmetadataserver`; films (1), series (2), seasons (3) and episodes (4); the `metadata` and `match` features. |
| A match | `POST /plex/library/metadata/matches` | The best match, or with `manual` every likely one, best first. The body is JSON, sent as `application/json`: any other type is refused with `415`. |
| An item | `GET /plex/library/metadata/{ratingKey}` | A film, a series, a season or an episode. With `includeChildren=1`, a series' seasons or a season's episodes come with it. |
| Its children | `GET /plex/library/metadata/{ratingKey}/children` | A series' seasons, or a season's episodes, a page at a time. |
| Its grandchildren | `GET /plex/library/metadata/{ratingKey}/grandchildren` | A series' episodes, every season's, a page at a time. |
| Its pictures | `GET /plex/library/metadata/{ratingKey}/images` | Posters (`coverPoster`), backgrounds (`background`), logos (`clearLogo`) — the one the work leads with first — and an episode's still (`snapshot`). |

All of it is in the OpenAPI document under *Plex provider*.

### Rating keys

A rating key names a work by the identifier it is best known by, so that it
survives the work being fetched again, or a new database: a film by TMDB's id,
then IMDb's; a series by TheTVDB's, then TMDB's, then Fankai's; either by the
catalogue's own id when it has none of those, as an entry made by hand may.

```
movie-tmdb-603                  show-tvdb-81189
season-tvdb-81189-s1            episode-tvdb-81189-s1e3
season-tvdb-81189-s1-dvd        movie-id-…   (an entry made by hand)
```

A season or an episode of another order ends with the order's name (`-dvd`).
The GUID Plex keeps is `tv.plex.agents.custom.arrmetadataserver://{type}/{ratingKey}`.

### Paging

`/children` and `/grandchildren` read `X-Plex-Container-Start`, counted from 1
(a `0` is taken for the first item), and `X-Plex-Container-Size`, 20 when left
out and 5000 at the most, as headers or as query parameters.

## How a file is matched

A file Plex scanned is matched by an id Plex gives first: this provider's own
GUID, or an IMDb, TMDB or TheTVDB id — the `com.plexapp.agents.imdb://…`,
`…themoviedb://…` and `…thetvdb://…` GUIDs of the old agents among them. At most
ten ids are tried, each once. Then by title and year:

1. among the works the catalogue holds, one named exactly so, in any language it
   is held in, case and accents aside, from a year either side of Plex's;
2. otherwise through the search that Sonarr's and Radarr's lookups use, which asks
   TMDB and the other sources.

A match by title fetches and keeps nothing: only the work Plex then asks for by
its rating key is fetched and kept, as one that Sonarr asks for is. A match by an
id does reach the work that id names, fetched and kept, except for a caller who
may not see works for adults: for them a match looks among the works the
catalogue already holds, since what a fetch brought back could be a work they may
not see.

A season is matched by its series and its number; an episode by its series and
its season and number, or by the day it aired.

## What Plex is given

- **Language.** `X-Plex-Language` (a header, else a query parameter), then the
  caller's `tmdb.language` — set on the allowlist rule Plex comes in by, or on the
  server — then English, then the text as it is stored: the chain Sonarr v5 is
  answered by. What names no language is taken for none asked. A title, an
  overview or a tagline locked for one language alone is what Plex is given when
  it asks in that language; see [Editing](editing.md#locking-a-text-for-one-language).
  A season or an episode nobody named is called *Season 2*, *Saison 2*,
  *Staffel 2* — in French, German, Spanish, Italian, Portuguese or Dutch, else in
  English. The tagline is held in the language the catalogue is stored in alone,
  so it is given only to a Plex that asks in that language.
- **Country.** `X-Plex-Country`, else `tmdb.watchRegion` as set for the caller,
  picks the rating — `PG-13` for the United States, `fr/12` for another country,
  as Plex writes them — and a film's release day: in cinemas, else online, else on
  disc, in that country. A date fixed by hand is the film's in every country. See
  [Data](data.md#release-dates-and-certifications-by-country).
- **Episode orders.** A series offers the orders the catalogue keeps for it
  (`SeasonType`): the aired one (`official`), then each order TheTVDB keeps for
  it — `dvd`, `absolute`, `alternate`, `regional`. It is laid out in the one Plex
  asks for with `episodeOrder`; an order the series does not have gives no season
  at all.
- **Pictures** are given as absolute addresses, since Plex fetches every picture
  itself: a picture this server keeps is under `AMS_PUBLIC_URL`, or under the host
  Plex called when that is unset.
- **Ratings** are IMDb's and TMDB's. Rotten Tomatoes' are not sent: Plex takes
  them as fresh or rotten only.

## When Plex cannot add it

From the machine Plex runs on, `GET /plex` at the address the panel gave answers the
manifest. If it does not:

| Answer | Why |
|---|---|
| `403` | The address Plex calls from is not on the list of allowed addresses, or it comes through a proxy that is not in `AMS_TRUSTED_PROXIES`. The server's log names which. |
| `503` | The provider is switched off (`api.plex`). |
| `429` | The per-address rate limit was reached; `Retry-After` says when to come back. |
| nothing | The address does not reach the interface's door: check `AMS_PUBLIC_URL`, the port and any firewall. |

## Which route to take

| | Plex provider | `.nfo` export | Kometa metadata file |
|---|---|---|---|
| What reaches Plex | the whole catalogue, live | the whole catalogue, as files | only what was locked |
| When | whenever Plex asks | as of the last export | at each Kometa run |
| Needs | Plex 1.43 or later, able to call this server | files beside the media | Kometa |

Use the provider for a Plex that can call this server. Use the `.nfo` export for
Kodi and Jellyfin, or for a Plex that must not call it. Use Kometa's file when
Plex keeps its own agent and only your corrections are to be laid over what it
found.

## Settings and variables

| Name | Default | What it does |
|---|---|---|
| `api.plex` | `true` | Answers Plex. Off, every call gets a `503`. A setting, switched on **Keys & network** and on **Opening & APIs**. |
| `AMS_PLEX_AUTH` | `allowlist` | What the provider asks for: `allowlist`, `open` or `apikey` — a key in the address Plex is given, `/plex/<key>`. Read at start. |
| `tmdb.language` | the server's | The language Plex is answered in when it asks in none the work is named in; per key or per allowlist rule. |
| `tmdb.watchRegion` | the language's country | The country of the rating and of the release day when Plex names none; per key or per allowlist rule. |
| `AMS_PUBLIC_URL` | unset | What pictures and the panel's address are given under. |

[`scripts/e2e-stack.sh`](../scripts/e2e-stack.sh) runs a real Plex Media Server
against this one, has it add the provider and make an agent of it, and scans a
film and a series; [Pointing your stack at this server](integration.md#verified-against)
lists what was checked.
