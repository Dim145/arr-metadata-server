# Pointing your stack at this server

Sonarr, Radarr and most TMDB clients **cannot be configured** to use a different
metadata source. The URLs are compiled into the applications:

| Client | Hostname it calls | Where that is set |
|---|---|---|
| Sonarr | `skyhook.sonarr.tv` | `NzbDrone.Common/Cloud/SonarrCloudRequestBuilder.cs` |
| Sonarr, for its alternate titles (optional) | `services.sonarr.tv` | the same file |
| Radarr | `api.radarr.video` | `NzbDrone.Common/Cloud/RadarrCloudRequestBuilder.cs` |
| Jellyseerr / Overseerr | `api.themoviedb.org` | the TMDB SDK it bundles |
| Yamtrack, Jellyfin's TheTVDB plugin, Kodi's scraper | `api4.thetvdb.com` | the TheTVDB client each bundles |
| Yamtrack's imports, the anime trackers | `graphql.anilist.co` | the AniList client each bundles |

So substituting this server is done at the network layer: resolve those
hostnames to it, and make the client trust the certificate it presents. That is
the whole technique. Everything below is detail.

The server has **two doors**, and they are independent:

- `AMS_BIND_ADDRESS` (8080) — the web UI and the native API. Plain HTTP, or TLS
  from your own files (`AMS_TLS_CERT` / `AMS_TLS_KEY`), or behind whatever
  reverse proxy you already run. Nothing below touches it.
- `AMS_CLIENTS_BIND` (unset by default; `0.0.0.0:443` opens it) — Sonarr's,
  Radarr's and the relays' surfaces (TMDB, TheTVDB, AniList), and nothing
  else, always in TLS, under the names above. Its certificate is issued by
  the server itself, from an authority it makes on first start, and renewed
  before it runs out. No reverse proxy, no certificate tooling.

---

## 1. Open the clients' door

```bash
AMS_CLIENTS_BIND=0.0.0.0:443
```

On first start the server creates, under `AMS_TLS_DIR` (`data/tls`; `/data/tls`
in the image):

- `ca.crt` / `ca.key` — its authority, what the clients must trust;
- `server.crt` / `server.key` — the certificate the door shows, for all six
  hostnames (and any you add with `AMS_CLIENTS_NAMES=ams.lan,192.168.1.7`),
  good for a year and issued anew when fewer than thirty days remain — loaded
  without a restart, by the daily `tls.renew` task.

The authority is **constrained to those names** (RFC 5280 name constraints): a
machine that trusts it trusts this server for `skyhook.sonarr.tv` and its like,
and for nothing else on the web. `ca.key` is still a key your stack trusts:
keep it off shared storage. Adding names later that the constraints do not
cover leaves them out of the certificate, with a warning; the Opening & APIs
page lists them. An authority made before this server answered for
`services.sonarr.tv`, or before it answered for TheTVDB's and AniList's
names, is one such case. To cover them, set
`AMS_TLS_REPLACE_AUTHORITY` to the authority's fingerprint and restart: a new
authority is made for every name, once, the old one kept beside it
(`ca.crt.replaced-…` in the directory, or in the database with several
instances), and every client has to trust the new one — see
[Upgrading a server that is already running](#upgrading-a-server-that-is-already-running).

Bringing your own certificate instead — a wildcard, an internal CA of yours —
is `AMS_CLIENTS_TLS_CERT` / `AMS_CLIENTS_TLS_KEY`; no authority is made then.

The authority's certificate is served at `/ca.crt` on both doors, and the
script that installs it at `/trust-ca.sh`; the administration's **Opening &
APIs** page shows the door, its names, the certificate's end and the
fingerprints, with both files to download and a **Renew now**.

## 2. Port 443

The clients call `https://` on the default port, so the door has to be on it.
In a container that is nothing special: Docker lets a container's process bind
any port, non-root included (`net.ipv4.ip_unprivileged_port_start=0` in its
namespace, since 20.10), so `AMS_CLIENTS_BIND=0.0.0.0:443` and `"443:443"` do.
On an older engine, bind 8443 and map `"443:8443"`. Outside a container on
Linux, `setcap cap_net_bind_service=+ep` on the binary; macOS asks nothing.

With several instances (`AMS_MODE=multi`, see the README's *Several
instances*), every instance has a door of its own on 443, and all of them
serve the one certificate the authority in the database issued: point the
names at any of them, at a TCP balancer in front of them, or — in Compose —
give every instance the three names as network aliases, as
`compose.multi.yaml` does, so Docker's DNS hands a client either address.
`/ca.crt` is the same on each.

## 3. Redirect each client

### Docker Compose

Give the metadata server a fixed address on a shared network and override the
hostname per client:

```yaml
services:
  metadata:
    networks:
      stack:
        ipv4_address: 172.31.0.10

  sonarr:
    extra_hosts:
      - "skyhook.sonarr.tv:172.31.0.10"
      # Optional: Sonarr's alternate titles from this catalogue, below.
      - "services.sonarr.tv:172.31.0.10"

  yamtrack:
    extra_hosts:
      - "api.themoviedb.org:172.31.0.10"
      - "api4.thetvdb.com:172.31.0.10"
      - "graphql.anilist.co:172.31.0.10"
```

A complete, runnable example is in
[`compose.integration.yaml`](../compose.integration.yaml).

### Outside Docker

Add the same mapping to `/etc/hosts` on each client host, or create the records
on your local DNS resolver. A resolver is the better choice for more than one or
two machines — it is one place to change when the address moves.

## 4. Trust the CA

The redirect is useless until the client accepts the certificate. A client that
does not trust it fails with `unable to get local issuer certificate`.

**Sonarr and Radarr.** Installing a certificate is *not* a matter of mounting
it: the linuxserver.io images do not rebuild their trust bundle on their own.
They do run anything executable in `/custom-cont-init.d/` before starting,
which is what [`docker/trust-ca.sh`](../docker/trust-ca.sh) is for. It takes
the authority from the first of: a file mounted at
`/usr/local/share/ca-certificates/arr-metadata.crt`; the server's data volume
mounted read-only at `/arr-metadata` (`tls/ca.crt` in it); or `AMS_CA_URL`,
fetched from the server's web door, with retries while the server starts. In a
compose stack the URL is the simplest, and the only one that shares nothing
else — the data volume holds `ca.key` and the database, which a client's root
could read:

```yaml
environment:
  AMS_CA_URL: http://172.31.0.10:8080/ca.crt
volumes:
  - ./docker/trust-ca.sh:/custom-cont-init.d/10-trust-arr-metadata-ca:ro
depends_on:
  metadata:
    condition: service_healthy
```

That fetch is plain HTTP on the stack's own network. Once the authority exists,
its SHA-256 is on the Opening & APIs page: set `AMS_CA_FINGERPRINT` to it and
the script refuses anything else.

Deploying from the image alone, without this repository? The script is what the
server serves at `/trust-ca.sh`: `curl -o trust-ca.sh
http://<server>:8080/trust-ca.sh`. The container logs `[trust-ca] authority
installed` when it worked, and fails loudly when it found nothing to install.

**Jellyseerr and Overseerr.** Node merges a PEM file with its built-in roots,
so nothing in the image needs modifying — download `ca.crt` once from the page
(or `/ca.crt`), put it beside the compose file, and mount it:

```yaml
environment:
  NODE_EXTRA_CA_CERTS: /etc/arr-metadata-ca.crt
volumes:
  - ./arr-metadata-ca.crt:/etc/arr-metadata-ca.crt:ro
```

**Yamtrack, and other Python clients.** `requests` verifies against its own
bundle and ignores the system store, and `REQUESTS_CA_BUNDLE` *replaces* that
bundle rather than adding to it — set to `ca.crt` alone, every other call the
application makes (MyAnimeList, IGDB, its identity provider) fails. Make a
bundle of the roots the image carries and the authority, and mount that:

```bash
curl -o arr-metadata-ca.crt http://172.31.0.10:8080/ca.crt
docker run --rm --entrypoint python ghcr.io/fuzzygrim/yamtrack:latest \
  -c 'import certifi, sys; sys.stdout.write(open(certifi.where()).read())' > yamtrack-ca-bundle.crt
cat arr-metadata-ca.crt >> yamtrack-ca-bundle.crt
```

```yaml
environment:
  REQUESTS_CA_BUNDLE: /etc/yamtrack-ca-bundle.crt
  # A key issued on Opening & APIs › Keys, in place of its TheTVDB key and
  # of its TMDB key: this server signs it in, and stands in with its own.
  # Every name redirected to this server wants the key — a redirected
  # api.themoviedb.org with Yamtrack's own TMDB key still in place is a 401
  # on every TMDB call.
  TVDB_API: ams_…
  TMDB_API: ams_…
volumes:
  - ./yamtrack-ca-bundle.crt:/etc/yamtrack-ca-bundle.crt:ro
```

**Windows, or a client outside Docker.** Download `ca.crt` from the
administration page and import it into the trusted root store (`certmgr`, or
`Import-Certificate -CertStoreLocation Cert:\LocalMachine\Root`); on Linux, drop
it into `/usr/local/share/ca-certificates/` and run `update-ca-certificates`.

**Any other .NET client on Alpine or Debian**: mount into
`/usr/local/share/ca-certificates/` and make sure `update-ca-certificates` runs
before the application starts.

## 5. Allow the clients through

Sonarr and Radarr cannot attach an API key — there is nowhere in the protocol to
put one. Those surfaces are guarded by address instead:

```bash
AMS_ALLOWLIST=172.31.0.0/24
```

Set this to the network your stack runs on. It defaults to loopback plus the
RFC 1918 ranges, which is right for a single Docker host and too permissive for
a shared network.

If this server sits behind a reverse proxy, the allowlist is only as good as the
address resolution behind it — set `AMS_TRUSTED_PROXIES` to the proxy's network,
or `X-Forwarded-For` is ignored and every client looks like the proxy.

### TMDB clients

These *do* send an `api_key`, so in principle a key issued here works. Whether
you can use that depends on the client:

- **If it lets you set the TMDB API key** — some do — put a key issued in the web
  UI there and leave `AMS_TMDB_AUTH=apikey`. This server substitutes its own
  credentials before relaying upstream, so the client's copy never reaches TMDB.
- **If the key is compiled in** — Jellyseerr's is, as of this writing — there is
  nothing to configure, and the surface has to be guarded by address like the
  others:

  ```bash
  AMS_TMDB_AUTH=allowlist
  ```

Check before assuming: look for a TMDB API key field in the client's settings.

### TheTVDB and AniList clients

Some clients ask TheTVDB and AniList themselves, whatever they get from TMDB:
Yamtrack asks TheTVDB to place a Jellyfin or Plex episode it knows only by its
TheTVDB id, and AniList to import somebody's lists. Resolve `api4.thetvdb.com`
and `graphql.anilist.co` to this server as well, and those calls come through
it: each is handed on to the real service, and its answer handed back with the
fields a person **locked** here written in — a series' or a film's title and
overview, its year, dates, status, runtime and chosen poster, an episode's
title, overview, date, runtime and still; for AniList, the title, description,
genres and pictures of each entry the answer carries. Nothing unlocked is
touched: the services' own data reaches the client as it is, in the language
it asked for.

**TheTVDB** clients sign in first, `POST /v4/login` with a key, for a token
they carry on every call after. Two ways in:

- **A key issued here as the client's TheTVDB key** (`TVDB_API` in Yamtrack),
  with `AMS_TVDB_AUTH=apikey`, the default. The sign-in is answered with that
  same key as the token, so every call after carries a credential this server
  knows; TheTVDB is asked with this server's own key (`AMS_TVDB_API_KEY`, the
  one its TheTVDB source uses) in the client's place, and the client's copy
  never reaches it. Without a key of its own, the relay answers those clients
  `503`. The documents answered with that key are the same for every caller,
  and are kept a while, as the TMDB relay keeps TMDB's; the operator's own
  account — `user`, its favourites — is nobody else's, and refused with it.
- **The client's own TheTVDB key**, with `AMS_TVDB_AUTH=allowlist`: the sign-in
  is relayed to TheTVDB as it came, and the token TheTVDB answers travels on
  every call after, as the client's own. Nothing of a client's own token is
  kept. A key issued here still stands in under this policy, and is judged as
  under the other: a key revoked, run out or a member's is refused. Under the
  key policy, a client that brings its own TheTVDB token sends this server's
  key in `X-Api-Key`, since `Authorization` is the token's.

Reads only: TheTVDB's one write, a user's favourites, is refused whoever asks.

**AniList** has no key at all, so nothing a client sends can be one of this
server's: `AMS_ANILIST_AUTH=allowlist` is the default, and a key issued here
would only serve a script of your own. A client reading somebody's private
lists carries that person's own AniList token, which travels with the query as
it came; an answer to a query that carried one, to a mutation, or about
somebody's lists or account, is never kept — a public list is still
somebody's, read for its latest state. An entry is recognised by AniList's
id or by MyAnimeList's, whichever the client asked for: Yamtrack's import
asks only the latter. AniList's rate limit — ninety queries a minute, thirty while it runs
degraded — is handed back as AniList answers it, `429` with `Retry-After`, and
counts this server's own calls to AniList among them.

Both relays are switches on **Opening & APIs** (`api.tvdb`, `api.anilist`),
listed with the others; a member's key is kept off every relay until *Members
too* is on there. Each is also a start-up flag, `AMS_TVDB_PASSTHROUGH` and
`AMS_ANILIST_PASSTHROUGH`, on by default. This server calls both services as
sources of its own, by the same names: see
[If you redirect at the resolver](#if-you-redirect-at-the-resolver).

---

## Sonarr's alternate titles

Sonarr recognises a release only by the series' own title and the titles of
two lists it downloads every three hours: its own, from
`services.sonarr.tv/v1/scenemapping`, and TheXEM's. It never reads the
alternative titles of a Skyhook answer — neither the real one's nor this
server's — so a release named in French, or in a romaji spelling nobody put
in those lists, is an *unknown series* however many titles this catalogue
holds.

Resolve `services.sonarr.tv` to this server as well, and turn on
`sonarr.sceneMappings` (**Settings › Other metadata services**, or
`AMS_SONARR_SCENE_MAPPINGS=true` as its starting value). Sonarr's list is then
handed on whole, with a mapping added for each title of this catalogue that is:

- in a language releases of that series are named in: the language of the
  answers, English, or the work's own original language, romanised. The other
  translations — thirty of them for a popular show — are names that releases of
  series outside this catalogue go by too, which no list here can tell, and an
  alternative title its source gave no language to is left out for the same
  reason;
- written in the Latin alphabet, as release names are;
- not already known to Sonarr for that series, compared the way Sonarr
  compares titles (its `CleanSeriesTitle`, ported and tested against its own
  cases), nor one of those titles followed by more words — a season, a
  special, a spin-off like *Breaking Bad: Original Minisodes*, whose releases
  are not the series' episodes;
- claimed by no other series in Sonarr's list, in TheXEM's names, or in this
  catalogue. A title two series answer to makes Sonarr throw for every release
  by that name, and in an interactive search that one failure empties the whole
  list, so such a title is left out rather than risked. A work hidden from the
  catalogue gains no title, and keeps its own from every other series.

Sonarr also searches indexers that cannot be searched by id with every title of
its list written in Latin-1, one query each. Of the titles added, only the
series' title in the language of the answers is searched with
(`sonarr.sceneMappingSearch`, on by default); every other one serves to
recognise releases and is searched under the series' own title, which Sonarr
searches anyway. A popular show's thirty translations would otherwise be thirty
queries an episode.

Everything else Sonarr asks of that host — its updates, the list of daily
series, the server's notices, the clock and proxy checks, the MyAnimeList
import's sign-in — is relayed to the real service with the headers that
describe the request, never a key, a cookie or a proxy's forwarding headers,
and its answer handed back as it came, a redirect included. A request that
carries this server's own mark has come round a loop and is refused with a
`508`: set `AMS_SONARR_SERVICES_UPSTREAM` to the real service's address when
the instances themselves resolve the name here — a resolver-level redirect, or
network aliases the instances share, as in `compose.multi.yaml`. With the setting off, the list is relayed as it is too. When the real
list cannot be had, the answer is a `502` and nothing else: Sonarr keeps the
list it holds, where this catalogue's titles alone would have replaced it.

Sonarr shows the titles on the series' page, with `arr-metadata-server` beside
each one it got from here; **System › Tasks › Update Scene Mapping** takes them
at once, and `/api/v3/parse?title=…` says which series a release name goes to.

### Upgrading a server that is already running

The certificate of a server set up before a name existed — `services.sonarr.tv`
since 0.4.0, `api4.thetvdb.com` and `graphql.anilist.co` since 0.6.0 — does
not cover it, and cannot: the authority's constraints were fixed when it was
made. Nothing changes until you choose to, in this order:

1. Deploy the new version. Nothing else moves: the names added are listed
   under **Opening & APIs** as names the certificate does not carry, and the
   others are served as before.
2. Copy the authority's SHA-256 fingerprint from that page, set
   `AMS_TLS_REPLACE_AUTHORITY` to it and restart (with several instances,
   restart them all: the first replaces it, the others take the new one). The
   log says `replaced the authority`; the old one is kept aside.
3. Make every client trust the new authority. A container that runs
   `trust-ca.sh` with `AMS_CA_URL` takes it when it restarts — update
   `AMS_CA_FINGERPRINT` first where it is set. A mounted `ca.crt`, as for
   Jellyseerr, is downloaded again; a Windows store imports it again. Until
   then, those clients refuse the certificate: do this step right after the
   previous one.
4. Remove `AMS_TLS_REPLACE_AUTHORITY`. Left set, it does nothing — it names an
   authority that is no longer there — but it would replace this one too if the
   old pair were ever restored.
5. Add `services.sonarr.tv` to Sonarr's `extra_hosts` (or to the resolver) and
   recreate the container, then turn `sonarr.sceneMappings` on and run
   **Update Scene Mapping** in Sonarr. For the TheTVDB and AniList relays,
   add `api4.thetvdb.com` and `graphql.anilist.co` to the client's
   `extra_hosts` the same way, with its trust bundle in place (step 3).

To go back, turn the setting off: the list is relayed untouched, and Sonarr
replaces the titles it got from here at its next update. Removing the redirect
does the same. The old authority is kept aside, named after the first sixteen
digits of its fingerprint, and put back with the server stopped:

- alone, `ca.crt.replaced-…` and `ca.key.replaced-…` in `AMS_TLS_DIR`, both
  copied back over `ca.crt` and `ca.key`;
- with several instances, `tls.ca.replaced-…` in the `keystore` table, its
  value copied over `tls.ca`'s:
  `UPDATE keystore SET value = (SELECT value FROM keystore WHERE name = 'tls.ca.replaced-…') WHERE name = 'tls.ca'`.

Every client then trusts the old authority again, as it did before step 3.

Episodes nobody has named yet reach Sonarr as `TBA`, as Skyhook sends them,
rather than as an empty title Sonarr lists as a row with nothing to click;
Sonarr rewrites the titles it holds at its next refresh of the series.

---

## Verifying it worked

From inside a client container:

```bash
# Sonarr
curl -s https://skyhook.sonarr.tv/v1/tvdb/shows/en/81189 | head -c 200

# Radarr
curl -s https://api.radarr.video/v1/movie/329865 | head -c 200

# A TMDB client
curl -s "https://api.themoviedb.org/3/tv/1396?api_key=<your ams_ key>" | head -c 200

# Sonarr's alternate titles: the added ones say where they come from
curl -s https://services.sonarr.tv/v1/scenemapping | grep -c arr-metadata-server

# A TheTVDB client: signed in with a key issued here, the token is that key
curl -s https://api4.thetvdb.com/v4/login -H 'content-type: application/json' \
  -d '{"apikey":"<your ams_ key>"}'
curl -s https://api4.thetvdb.com/v4/series/81189 -H 'Authorization: Bearer <your ams_ key>' | head -c 200

# An AniList client
curl -s https://graphql.anilist.co -H 'content-type: application/json' \
  -d '{"query":"{ Media(id: 16498) { id title { english } } }"}'
```

A TLS error means the CA is not trusted yet — check the container logged
`[trust-ca] authority installed`. A connection refused means the hostname override did
not take; check `getent hosts skyhook.sonarr.tv` inside the client. A `403` means
the client's address is not in `AMS_ALLOWLIST`, and a `401` means the surface
wants a key the client did not send.

The server's own logs name which of those it was.

## Verified against

The compatibility surfaces were exercised against real clients, not only against
recorded responses:

| Client | Version | What was checked |
|---|---|---|
| Sonarr | 4.x (linuxserver) | lookup, add, 71 episodes imported, refresh picking up a locked title, network and episode title |
| Radarr | 6.4 | lookup, add, 26 credits stored, certification, refresh picking up a locked title, studio and runtime |
| Jellyseerr | latest | `/3/movie`, `/3/tv`, `/3/search/movie` and `/3/configuration` relayed, with local overrides applied |
| Yamtrack | latest | signed in to TheTVDB with a key issued here through its own provider, read a series and an episode with the titles locked here written in, placed the series by its TMDB id, queried AniList through its request helper with the locked title written in — all by the services' names, in TLS, trusting the authority |

That run turned up several things this document had wrong, and several the
server had wrong — see the commit history.

[`scripts/e2e-sonarr.sh`](../scripts/e2e-sonarr.sh) repeats the Sonarr part
on demand: it starts the server with a catalogue of its own and both doors,
starts a Sonarr container that resolves `skyhook.sonarr.tv` to this host and
installs the authority through `trust-ca.sh`, then has Sonarr search, add the
series, and fetch its episodes and its poster from here. It needs Docker,
python3, a built binary and a free port 443 — which on Linux a user binds only
after `setcap cap_net_bind_service=+ep` on the binary.

[`scripts/e2e-sonarr-parity.sh`](../scripts/e2e-sonarr-parity.sh) compares:
two Sonarrs are given the same real series, one reaching this server for
`skyhook.sonarr.tv` and `services.sonarr.tv`, the other the real services, and
what they hold is set side by side — the series, every episode, `TBA` included,
the alternate titles, and the series each takes a set of release names for.
A difference is either an addition of this server's, or a failure. It reaches
the internet, and wants `TMDB_API_KEY` (and `TVDB_API_KEY`) in the environment.

[`scripts/e2e-relays.sh`](../scripts/e2e-relays.sh) exercises the TheTVDB and
AniList relays against a stand-in for both services
([`scripts/e2e-relays-upstream.py`](../scripts/e2e-relays-upstream.py)) that
records what reaches it: the sign-in with a key issued here and with a
client's own, which token travels and which never does, the locks written
into a series, its translation, its episodes and an AniList entry, the cache,
AniList's rate limit handed back, a loop refused, a relay switched off — under
the default policies and with them swapped. It reaches nothing outside this
host, and needs a built binary and python3.

[`scripts/e2e-yamtrack.sh`](../scripts/e2e-yamtrack.sh) repeats the Yamtrack
row of the table above on demand: it starts the server with both doors and
the real services behind the relays, starts a Yamtrack container that resolves
`api4.thetvdb.com` and `graphql.anilist.co` to this host with the authority
appended to its trust bundle, and runs Yamtrack's own TheTVDB provider and
request helper inside it. It needs Docker, a built binary, a free port 443,
the internet, and this server's TheTVDB and TMDB keys in the environment.

---

## If you redirect at the resolver

Everything above redirects per container, with `extra_hosts`. If instead you add
the records to your local DNS resolver, **this server resolves them too** — and
it calls `skyhook.sonarr.tv`, `api.radarr.video`, `api4.thetvdb.com` and
`graphql.anilist.co` itself, because they are providers as well as protocols
it speaks.

It recognises the loop and answers `508 Loop Detected` rather than recursing, so
nothing hangs. But the enrichment is then dead — and so are the TheTVDB and
AniList relays, which ask the real services. Either point the upstreams at
the real services by address:

```bash
AMS_SKYHOOK_UPSTREAM=https://<real-skyhook-address>
AMS_RADARR_METADATA_UPSTREAM=https://<real-api.radarr.video-address>
AMS_SONARR_SERVICES_UPSTREAM=https://<real-services.sonarr.tv-address>
AMS_TVDB_UPSTREAM=https://<real-api4.thetvdb.com-address>/v4
AMS_ANILIST_UPSTREAM=https://<real-graphql.anilist.co-address>
```

or turn enrichment off:

```bash
AMS_SKYHOOK_ENRICH=false
AMS_RADARR_METADATA_ENRICH=false
```

## A note on languages

Sonarr's request builder pins the language segment to `en` and nothing in Sonarr
changes it, so asking this server for French through Sonarr is not possible from
Sonarr's side. Set `AMS_TMDB_LANGUAGE` instead: entries are stored in that
language and every client gets it.

Radarr has no language in its protocol at all, and the same applies.

TMDB clients do send `language=`, and it is forwarded upstream, so those get
whatever they ask for with your edits patched in.

## Series of the same name

TheTVDB names the second of two homonymous series with what tells it apart
— *Rurouni Kenshin (2023)*, *The Office (US)* — and Skyhook passes that on,
so it is the title Sonarr files, names folders after and matches releases
by. TMDB never does, and its name is usually the title here. The Sonarr
surface therefore gives back what TheTVDB adds, and only there: the site,
the native API and the TMDB relay keep the title as TMDB names it, the year
shown beside it. A work TheTVDB has no entry for is given its year when
another series of the catalogue is served the same title, since two series
Sonarr knows by one title make its lookup by title fail. A locked title is
sent exactly as it was locked.

## What about Plex?

Plex has no configurable metadata source. Since the legacy agents were removed,
the server talks to `metadata.provider.plex.tv` over a pinned connection tied to
your Plex account, and that cannot be substituted the way the others can.

The supported route is the **Personal Media / XBMCnfo** agent reading `.nfo`
files and local artwork. Sonarr and Radarr already write those, so a practical
setup is: this server feeds Sonarr and Radarr, they write the `.nfo` files, and
Plex reads them. Your corrections reach Plex, just indirectly.

If nothing else manages the library, `POST /api/v1/export/nfo` writes the whole
catalogue under `AMS_NFO_EXPORT_PATH`, pictures included:

```
series/breaking-bad-2008/tvshow.nfo
series/breaking-bad-2008/poster.jpg      fanart.jpg  banner.jpg  clearlogo.png
series/breaking-bad-2008/season01-poster.jpg
series/breaking-bad-2008/.actors/Bryan Cranston.jpg
series/naruto-shippuden-yabai-2007/theme.mp3             where the provider keeps a theme
series/breaking-bad-2008/Season 01/S01E01.nfo
series/breaking-bad-2008/Season 01/S01E01-thumb.jpg
movies/arrival-2016/movie.nfo            poster.jpg  fanart.jpg  …
```

Those are the names Kodi defined and Plex's agent adopted, which is the point:
a `.nfo` that only carries image *URLs* leaves the fetching to the consumer, and
Plex often will not. Copy or link this tree next to your media. Re-running the
export rewrites the documents and leaves existing pictures alone, so it is cheap
to repeat. Set `AMS_NFO_EXPORT_ARTWORK=false` for documents only.

The call answers at once, `202` with the job it started, and the export is
written in the background: a library's artwork is thousands of downloads,
more than a request can wait for. The run is listed under **Jobs** as
`export.nfo`, with what it wrote once it is done; a second export asked for
while one is running is refused with `409`.

A Fan-Kai comes out the same way once the Fankai source is on and the
production has been imported or asked for by Sonarr: `series/{slug}/tvshow.nfo`
with `<uniqueid type="fankai">`, and one document per film under the season
its saga is, numbered as Fankai names the files — `Season 01/S01E02.nfo` beside
`Horimiya Kaï.S01E02.MULTI.1080p.x265-FANKAI.mkv`. Plex reads it through the
same agent; Jellyfin and Kodi read it as they read any `.nfo`. Its theme music
lands beside it as `theme.mp3`, the name Plex's local assets, Jellyfin and
Kodi's theme add-ons play as the show's theme.

Jellyfin and Emby are a different matter — both accept metadata plugins, so a
direct provider for them is possible. It is not built yet.
