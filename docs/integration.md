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

---

## 1. Issue a certificate

```bash
./docker/gen-certs.sh
```

This creates `docker/certs/` containing:

- `ca.crt` / `ca.key` — a local certificate authority
- `server.crt` / `server.key` — one certificate covering all three hostnames

`ca.key` is the private key of an authority your whole stack will trust. Keep it
off shared storage; it is in `.gitignore` already.

## 2. Serve TLS on port 443

The clients call `https://` on the default port, so the server has to answer
there:

```bash
AMS_BIND_ADDRESS=0.0.0.0:443
AMS_TLS_CERT=/certs/server.crt
AMS_TLS_KEY=/certs/server.key
```

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

**Sonarr and Radarr.** Mounting the certificate is *not* enough: the
linuxserver.io images do not rebuild their trust bundle on their own. They do run
anything executable in `/custom-cont-init.d/` before starting, which is what
[`docker/trust-ca.sh`](../docker/trust-ca.sh) is for:

```yaml
volumes:
  - ./docker/certs/ca.crt:/usr/local/share/ca-certificates/arr-metadata.crt:ro
  - ./docker/trust-ca.sh:/custom-cont-init.d/10-trust-arr-metadata-ca:ro
```

The container logs `[trust-ca] CA installed` when it worked.

**Jellyseerr and Overseerr.** Node merges this file with its built-in roots, so
nothing in the image needs modifying:

```yaml
environment:
  NODE_EXTRA_CA_CERTS: /etc/arr-metadata-ca.crt
volumes:
  - ./docker/certs/ca.crt:/etc/arr-metadata-ca.crt:ro
```

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
`[trust-ca] CA installed`. A connection refused means the hostname override did
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
