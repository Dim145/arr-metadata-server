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

The redirect is useless until the client accepts the certificate.

**Sonarr and Radarr** (linuxserver images run `update-ca-certificates` at start):

```yaml
volumes:
  - ./docker/certs/ca.crt:/usr/local/share/ca-certificates/arr-metadata.crt:ro
```

**Jellyseerr and Overseerr** — Node merges this file with its built-in roots, so
nothing in the image needs modifying:

```yaml
environment:
  NODE_EXTRA_CA_CERTS: /etc/arr-metadata-ca.crt
volumes:
  - ./docker/certs/ca.crt:/etc/arr-metadata-ca.crt:ro
```

**Any other .NET client**: mount into `/usr/local/share/ca-certificates/` and
run `update-ca-certificates`.

## 5. Allow the clients through

Sonarr and Radarr cannot attach an API key to a request — there is nowhere in
the protocol to put one. Those surfaces are therefore guarded by address:

```bash
AMS_ARR_ALLOWLIST=172.31.0.0/24
```

Set this to the network your stack runs on. It defaults to loopback plus the
RFC 1918 ranges, which is right for a single Docker host and too permissive for
a shared network.

If this server sits behind a reverse proxy, the allowlist is only as good as the
address resolution behind it — set `AMS_TRUSTED_PROXIES` to the proxy's network,
or `X-Forwarded-For` will be ignored and every client will look like the proxy.

TMDB clients *can* send a key: they already send `api_key`. Issue one in the web
UI and configure it as the client's "TMDB API key". This server substitutes its
own credentials before relaying upstream, so the client's copy is never used
against TMDB.

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

A TLS error means the CA is not trusted yet. A connection refused means the
hostname override did not take. A `403` means the client's address is not in
`AMS_ARR_ALLOWLIST`.

The server's own logs name which of those it was.

---

## What about Plex?

Plex has no configurable metadata source. Since the legacy agents were removed,
the server talks to `metadata.provider.plex.tv` over a pinned connection tied to
your Plex account, and that cannot be substituted the way the others can.

The supported route is the **Personal Media / XBMCnfo** agent reading `.nfo`
files and local artwork. Sonarr and Radarr already write those, so a practical
setup is: this server feeds Sonarr and Radarr, they write the `.nfo` files, and
Plex reads them. Your corrections reach Plex, just indirectly.

Jellyfin and Emby are a different matter — both accept metadata plugins, so a
direct provider for them is possible. It is not built yet.
