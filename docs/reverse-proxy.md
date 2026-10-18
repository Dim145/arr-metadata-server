# Behind a reverse proxy

The interface can be published under a name of its own, `https://metadata.example.com`, by
the reverse proxy that already publishes the rest of a server: nginx, Traefik, Caddy. This page
says what the server needs to know about that proxy, what it reads from it, what to keep the
proxy from forwarding, and where TLS is done. Every address and name below is an example.

## The two doors

The server listens on two doors, and only the first belongs behind a reverse proxy.

- **The interface's door**, `AMS_BIND_ADDRESS` (`0.0.0.0:8080`). The interface, the native
  API (`/api/v1/…`) and its documentation (`/api/docs`), the media kept (`/media/…`), the
  probes (`/health`, `/ready`) and the authority's files (`/ca.crt`, `/trust-ca.sh`) — and,
  beside them, the surfaces the clients call: Sonarr's and Radarr's (`/v1/…`, `/v2/…`), the
  TMDB relay (`/3/…`, `/4/…`), the TheTVDB relay (`/v4/…`) and the Plex provider (`/plex`),
  which is on this door only. Plain HTTP, or TLS from `AMS_TLS_CERT` and `AMS_TLS_KEY`.
- **The clients' door**, `AMS_CLIENTS_BIND` (unset; `0.0.0.0:443` opens it). Sonarr's,
  Radarr's and the relays' surfaces, and nothing of the interface, always in TLS, under the
  names the clients have compiled in — `skyhook.sonarr.tv`, `services.sonarr.tv`,
  `api.radarr.video`, `api.themoviedb.org`, `api4.thetvdb.com`, `graphql.anilist.co` — and
  those of `AMS_CLIENTS_NAMES`. A request for any other name is answered `421`. It serves a
  certificate of its own, which the clients trust, and stays off the proxy: see
  [The clients' door](#the-clients-door).

## What the server needs to know

Two variables, both read at the start.

### `AMS_PUBLIC_URL`

The address the interface is reached at, as a browser outside types it:

```bash
AMS_PUBLIC_URL=https://metadata.example.com
```

It is the root of a name — the interface asks for `/api/v1/…` and its files from `/`, so it
cannot be served under a path such as `https://example.com/metadata/`. The server reads it as:

- the address the identity provider sends a browser back to,
  `{AMS_PUBLIC_URL}/api/v1/auth/oidc/callback` — without it, signing in through a provider is
  not offered;
- the address of an invitation's link, of the links in a feed, of the picture a shared link
  unfurls with, and of the Plex provider that **Keys & network** shows;
- the address of the media kept, in what Sonarr, Radarr and the `.nfo` documents are given:
  those clients have to reach this name too, or are better given the providers' own addresses,
  which is what they get when it is unset;
- an origin a change may be asked from (see [Changes need this server's own
  pages](integration.md#changes-need-this-servers-own-pages));
- with `https`, a reason to mark the session cookie `Secure`.

### `AMS_TRUSTED_PROXIES`

The proxy's address, as the server sees it: one or more addresses or CIDR blocks, separated by
commas. Empty by default.

```bash
AMS_TRUSTED_PROXIES=172.30.0.10
```

A request from one of these addresses is believed when it says who it came from. Without the
proxy listed, the proxy's address is taken for every client's, and:

- every client shares one rate limit, `AMS_RATE_LIMIT_PER_MINUTE` for them all;
- sign-ups count against one address, and the audit trail and **Who has been calling** record
  the proxy rather than who did what;
- on a surface guarded by the list of allowed addresses — Sonarr's and Radarr's, the AniList
  relay, the Plex provider, and any other whose policy is `allowlist` — a request the proxy
  forwarded is refused (`403`), with a warning in the log that names the proxy and says to add
  it to `AMS_TRUSTED_PROXIES`.

That last one is deliberate. A proxy usually has a private address, and the list of allowed
addresses starts as the private ranges: taken for the client, the proxy would let the whole
internet through to those surfaces. A forwarded request is told from a direct one by the
headers a proxy adds — `X-Forwarded-For`, `Forwarded` or `X-Real-IP` — which Sonarr and Radarr
calling directly never send.

The server warns at the start when `AMS_PUBLIC_URL` says `https` while it holds no TLS of its
own and `AMS_TRUSTED_PROXIES` is empty: something in front of it terminates TLS, and nobody
declared it.

### The headers it reads

From a trusted proxy, and from nobody else:

| Header | Read as |
| --- | --- |
| `X-Forwarded-For` | The client's address. Its lines are read as one list, in the order they arrived, and walked from the right: the first address that is not itself a trusted proxy is the client. A hop that is no address leaves the client unknown: refused by every list of allowed addresses, and counted with the other unknowns. |
| `X-Forwarded-Proto` | The scheme the request reached the proxy by, `https` or `http`: its first value. |
| `X-Forwarded-Host` | The name the request reached the proxy by: its first value. It counts as this server's own origin when a change is asked, and names the links of a feed when `AMS_PUBLIC_URL` is unset. |

`Forwarded` and `X-Real-IP` are only recognised, never read: a proxy that sends either without
`X-Forwarded-For` leaves every client looking like the proxy. With several proxies in a row,
every one between the client and the server has to be in `AMS_TRUSTED_PROXIES`. A request from
an address that is not trusted keeps its own address, whatever it claims in these headers.

The `Host` header is read as it arrives. With `AMS_ALLOWED_HOSTS` set, it has to be one of
those names, or the request is answered `421` — so the proxy passes the client's `Host` on, and
the list names every name the interface's door is reached by: the public one, and the one Plex,
a client or a script on the network uses (`metadata`, `192.168.1.20`). `/health`, `/ready`,
`/ca.crt` and `/trust-ca.sh` answer under any name.

Nothing else is needed of the proxy: no WebSocket, no streamed answer. A request is answered
within `AMS_REQUEST_TIMEOUT` (60 seconds), and anything longer — an export, storing every
picture — runs as a job. A request's body is held to 1 MiB, but for a picture or a theme
uploaded from a work's page: 25 MiB for a picture, 60 MiB for a theme.

## Keeping the clients' surfaces off the internet

Sonarr, Radarr, Plex and the relays' clients call this server from the local network: on the
clients' door, or on the interface's door by its address there. None of them needs the public
name, and the paths they call are best not forwarded by it at all:

| Path | Who calls it |
| --- | --- |
| `/plex`, `/plex/…` | Plex Media Server |
| `/v1/…`, `/v2/…` | Sonarr and Radarr |
| `/3/…`, `/4/…` | TMDB clients |
| `/v4/…` | TheTVDB clients |

They are guarded either way — by the list of allowed addresses, or by a key — but Plex sends
a provider no credential, so the Plex provider is guarded by address alone unless its address
carries a key ([Keeping it on your network](plex.md#keeping-it-on-your-network)). The
interface asks for none of these paths. A Plex that is elsewhere and reaches this server
through the proxy is given a key in its address instead (`AMS_PLEX_AUTH=apikey`,
[A key in the address](plex.md#a-key-in-the-address)), and `/plex` is then forwarded.

The media kept, `/media/…`, are another matter: when the server keeps them and
`AMS_PUBLIC_URL` is set, Sonarr and Radarr are given their addresses under it, and fetch them
there.

## Examples

Each example publishes the interface at `metadata.example.com`, forwards everything but the
paths above, and expects the proxy at `172.30.0.10` on a network it shares with the server.
In Compose, that is a network with a fixed subnet and a fixed address for the proxy, so the
server can be told which it is — as [`compose.multi.yaml`](../compose.multi.yaml) does:

```yaml
services:
  metadata:
    image: ghcr.io/dim145/arr-metadata-server:latest
    environment:
      AMS_PUBLIC_URL: https://metadata.example.com
      AMS_TRUSTED_PROXIES: 172.30.0.10
    networks: [web]

  proxy:
    # nginx, Traefik or Caddy, below.
    ports: ["80:80", "443:443"]
    networks:
      web:
        ipv4_address: 172.30.0.10

networks:
  web:
    ipam:
      config:
        - subnet: 172.30.0.0/24
```

The server's own port is not published: the proxy reaches it on the network. A proxy on the
host that reaches a published port instead arrives through Docker's own proxy, from the
network's gateway — which would then be the address to trust, and with it anything else that
reaches that port.

### nginx

nginx sends no `X-Forwarded-For` unless told to, and replaces the `Host` with the upstream's
name unless told otherwise. It also refuses a body over a megabyte, which an upload exceeds.

```nginx
server {
    listen 443 ssl;
    server_name metadata.example.com;

    ssl_certificate     /etc/nginx/tls/fullchain.pem;
    ssl_certificate_key /etc/nginx/tls/privkey.pem;

    # A theme uploaded from a work's page: 60 MiB, and the form around it.
    client_max_body_size 61m;

    # The surfaces the clients call stay on the local network.
    location ~ ^/(plex|v1|v2|v4|3|4)(/|$) {
        return 404;
    }

    location / {
        proxy_pass http://metadata:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-Host $host;
    }
}
```

### Traefik

Traefik passes the `Host` on and sets the three `X-Forwarded-*` headers itself; a router on the
server's container says which requests reach it. With an entry point `websecure` on 443 and a
certificate resolver `letsencrypt` in Traefik's own configuration:

```yaml
  metadata:
    labels:
      traefik.enable: "true"
      traefik.http.routers.metadata.rule: >-
        Host(`metadata.example.com`)
        && !PathPrefix(`/plex`) && !PathPrefix(`/v1/`) && !PathPrefix(`/v2/`)
        && !PathPrefix(`/v4/`) && !PathPrefix(`/3/`) && !PathPrefix(`/4/`)
      traefik.http.routers.metadata.entrypoints: websecure
      traefik.http.routers.metadata.tls.certresolver: letsencrypt
      traefik.http.services.metadata.loadbalancer.server.port: "8080"
```

A request the router does not take is answered `404` by Traefik.

### Caddy

Caddy obtains the certificate for the name, passes the `Host` on and sets the
`X-Forwarded-*` headers itself:

```
metadata.example.com {
	@clients path /plex /plex/* /v1/* /v2/* /v4/* /3/* /4/*
	respond @clients 404

	reverse_proxy metadata:8080
}
```

## TLS

**The interface's door.** The proxy terminates TLS for the public name and speaks plain HTTP to
the server, on a network nothing else reaches; that is what the examples do. The server may
hold a certificate itself as well (`AMS_TLS_CERT`, `AMS_TLS_KEY`, PEM files) for a proxy that
connects in `https`, or to be reached without a proxy at all.

Either way the server knows the request came in TLS — from its own, or from a trusted proxy's
`X-Forwarded-Proto: https` — and then answers with `Strict-Transport-Security` for a year,
without the subdomains, marks the session cookie `Secure`, and gives the cookie that carries a
sign-in through the identity provider the `__Host-` prefix. A browser that has been answered
over `https` once keeps asking that name over `https` for that year.

**The clients' door.** Always TLS, with a certificate for the names the clients call, which no
public authority would issue: the server's own, from an authority it makes and the clients are
made to trust, or one of yours (`AMS_CLIENTS_TLS_CERT`, `AMS_CLIENTS_TLS_KEY`). See
[Pointing your stack at this server](integration.md).

## The clients' door

Keep the clients' door out of the reverse proxy, and off the internet: the clients call it by
the services' names, with the certificate the server issues for those names and renews by
itself, and nothing outside your network has any business there.

- In Compose, the clients reach the server's own address on their network
  (`extra_hosts`, [Redirect each client](integration.md#3-redirect-each-client)), and the
  door's port need not be published at all. That also leaves the host's 443 to the proxy. A
  client outside Docker needs the door published: on an address of the host the proxy does not
  listen on, since the two cannot share one port.
- A TCP balancer in front of several instances — as `compose.multi.yaml` has — passes the TLS
  through untouched, so it can add no header: every client reaches the server from the
  balancer's address, which the list of allowed addresses then has to allow, for every client
  that reaches the balancer. The server reads no PROXY protocol. Keep such a balancer on the
  clients' network.

## Checking it

From outside, through the proxy:

```bash
curl -sI https://metadata.example.com/health                     # 200
curl -sI https://metadata.example.com/plex                       # 404, from the proxy
curl -s  https://metadata.example.com/api/v1/settings/registry   # 401: a credential is asked for
```

Then sign in through the proxy and open **Audit**: the sign-in is recorded from your own
address, not the proxy's. A forwarded request that reaches an address-guarded surface from a
proxy nobody declared leaves `refused a forwarded request from a proxy outside
AMS_TRUSTED_PROXIES` in the server's log, with the proxy's address.
