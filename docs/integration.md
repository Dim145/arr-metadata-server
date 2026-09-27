# Pointing your stack at this server

Sonarr, Radarr and most TMDB clients **cannot be configured** to use a different
metadata source. The URLs are compiled into the applications:

| Client | Hostname it calls | Where that is set |
|---|---|---|
| Sonarr | `skyhook.sonarr.tv` | `NzbDrone.Common/Cloud/SonarrCloudRequestBuilder.cs` |
| Radarr | `api.radarr.video` | `NzbDrone.Common/Cloud/RadarrCloudRequestBuilder.cs` |
| Jellyseerr / Overseerr | `api.themoviedb.org` | the TMDB SDK it bundles |

So substituting this server is done at the network layer: resolve those
hostnames to it, and make the client trust the certificate it presents. That is
the whole technique. Everything below is detail.

The server has **two doors**, and they are independent:

- `AMS_BIND_ADDRESS` (8080) — the web UI and the native API. Plain HTTP, or TLS
  from your own files (`AMS_TLS_CERT` / `AMS_TLS_KEY`), or behind whatever
  reverse proxy you already run. Nothing below touches it.
- `AMS_CLIENTS_BIND` (unset by default; `0.0.0.0:443` opens it) — Sonarr's,
  Radarr's and the TMDB relay's surfaces, and nothing else, always in TLS,
  under the names above. Its certificate is issued by the server itself, from
  an authority it makes on first start, and renewed before it runs out. No
  reverse proxy, no certificate tooling.

---

## 1. Open the clients' door

```bash
AMS_CLIENTS_BIND=0.0.0.0:443
```

On first start the server creates, under `AMS_TLS_DIR` (`data/tls`; `/data/tls`
in the image):

- `ca.crt` / `ca.key` — its authority, what the clients must trust;
- `server.crt` / `server.key` — the certificate the door shows, for all three
  hostnames (and any you add with `AMS_CLIENTS_NAMES=ams.lan,192.168.1.7`),
  good for a year and issued anew when fewer than thirty days remain — loaded
  without a restart, by the daily `tls.renew` task.

The authority is **constrained to those names** (RFC 5280 name constraints): a
machine that trusts it trusts this server for `skyhook.sonarr.tv` and its like,
and for nothing else on the web. `ca.key` is still a key your stack trusts:
keep it off shared storage. Adding names later that the constraints do not
cover leaves them out of the certificate, with a warning — remove `ca.crt` and
`ca.key` to make the authority anew, and trust it again everywhere.

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

That run turned up several things this document had wrong, and several the
server had wrong — see the commit history.

[`scripts/e2e-sonarr.sh`](../scripts/e2e-sonarr.sh) repeats the Sonarr part
on demand: it starts the server with a catalogue of its own and both doors,
starts a Sonarr container that resolves `skyhook.sonarr.tv` to this host and
installs the authority through `trust-ca.sh`, then has Sonarr search, add the
series, and fetch its episodes and its poster from here. It needs Docker,
python3, a built binary and a free port 443 — which on Linux a user binds only
after `setcap cap_net_bind_service=+ep` on the binary.

---

## If you redirect at the resolver

Everything above redirects per container, with `extra_hosts`. If instead you add
the records to your local DNS resolver, **this server resolves them too** — and
it calls `skyhook.sonarr.tv` and `api.radarr.video` itself, because they are
providers as well as protocols it speaks.

It recognises the loop and answers `508 Loop Detected` rather than recursing, so
nothing hangs. But the enrichment is then dead. Either point the upstreams at
the real services by address:

```bash
AMS_SKYHOOK_UPSTREAM=https://<real-skyhook-address>
AMS_RADARR_METADATA_UPSTREAM=https://<real-api.radarr.video-address>
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
