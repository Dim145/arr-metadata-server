# What is closed, and how

A few things worth knowing before this is reachable from anywhere. Who may call each surface
is in [Accounts and access](access.md), and publishing the interface in
[Behind a reverse proxy](reverse-proxy.md).

- **Nothing a caller presents here is relayed upstream.** The TMDB relay strips
  the session cookie and every spelling of an API key before forwarding, so a
  credential for this server never reaches TMDB's access logs.
- **The relay reads and does not write.** It exists for Jellyseerr, Overseerr and
  Plex, which only read; a forwarded `POST` would let any key issued here rate a
  film, or empty a list, using the operator's TMDB credentials.
- **Of TMDB's v4 API, only the public lists are relayed** (`/4/list/{id}`), and
  only with a v4 read token configured: Radarr's TMDb list imports read them
  there. An account's own lists, ratings and watchlist need that account's
  token, which the relay carries for nobody.
- **The settings, the job history and the audit trail need an administrator**,
  not merely a credential. A key issued to Jellyseerr cannot read which surface
  takes which credential or whether authentication is off.
- **The session cookie** is `HttpOnly`, `SameSite=Strict`, and `Secure` when this
  server terminates TLS, `AMS_PUBLIC_URL` says `https`, or a trusted proxy says
  the request reached it in `https`. Two session cookies on one request — a
  second can only have been planted by a sibling under the same domain — sign
  that request in as nobody.
- **A change has to be asked from this server's own pages.** SameSite stops
  other sites, but every other application on the same host or domain is the
  same *site*. So a write carried by the session cookie — or, on the native API
  under `allowlist` or `open`, by the browser's address — is refused when the
  browser's `Sec-Fetch-Site` says it came from another page, or an older
  browser's `Origin` is not this server's (`AMS_PUBLIC_URL` and
  `AMS_CORS_ORIGINS` count as this server's). Keys, Sonarr, Radarr and scripts
  are not concerned.
- **Every response carries a content security policy** with `frame-ancestors
  'none'`, so no page elsewhere can frame this one and borrow an administrator's
  clicks, and `Referrer-Policy: no-referrer`, so the ids of what you are looking
  at do not travel to the artwork hosts; a `Permissions-Policy` that denies the
  camera, the microphone, the location and payments; on pages, a
  `Cross-Origin-Opener-Policy`; and, over TLS — this server's own or a trusted
  proxy's — `Strict-Transport-Security` for a year.
- **Every surface is rate limited**, the arr and TMDB ones included
  (`AMS_RATE_LIMIT_PER_MINUTE`, 600 by default, per address — an IPv6 address
  by its /64); a refusal says when to come back in `Retry-After`, and the
  per-address limit its quota in `RateLimit` and `RateLimit-Policy`. Signing
  in has two brakes of its own: an account that failed
  `AMS_SIGNIN_FAILURES_PER_ACCOUNT` times (10) in fifteen minutes is refused
  without being checked until the window closes, from wherever the attempts
  come; and passwords are checked a few at a time — as many as the machine has
  cores, two to eight — so a burst of sign-ins waits, or is told to come back,
  rather than taking the memory argon2 needs for each. With passwords off, the
  name `AMS_ADMIN_USERNAME` keeps its door, and takes as long to refuse as any
  other name.
- **Each door holds `AMS_MAX_CONNECTIONS` connections at once** (1024); past
  that, a new one waits to be accepted. A connection has
  `AMS_HEADER_READ_TIMEOUT` seconds (30) to send a request's head, and may sit
  idle between two requests no longer.
- **What is logged has its secrets masked:** the values of `api_key`, `apikey`,
  `token` and their like in any address, what follows `Bearer` or `Basic`, the
  password of an address, the configured keys and passwords, the keys kept for
  your own Sonarr and Radarr, and the addresses, tokens and signing secrets of
  notification destinations. A failed sign-in's username is logged quoted, its
  line breaks escaped.
- **An image URL may not name an address only this server can reach** — loopback,
  link-local, `169.254.169.254`. The NFO export downloads stored artwork, which
  makes an image URL a request this server makes on somebody else's say-so.
  A private LAN address is still allowed — a picture mirror at home is a
  reasonable thing to own — unless `AMS_MEDIA_PRIVATE_NETWORKS=false` refuses
  those too.
- **A notification destination's address is an administrator's setting**, and
  is refused the addresses an image URL is refused — loopback, link-local, the
  cloud's metadata — and allowed a LAN one, since a relay at home is as
  reasonable as a picture mirror. Webhooks are posted without `HTTP_PROXY`, and
  a redirect is not followed. A test posts to that destination alone, and says
  back the status and how long it took and, when the address refused it, the
  start of what it answered, with the destination's secrets masked.
- **`AMS_CORS_ORIGINS` names origins.** `*` is ignored with a warning rather than
  honoured — this server sends credentials, and the two cannot be combined.
- **`AMS_ALLOWED_HOSTS` names the hostnames this server answers to**, and is the
  only defence against DNS rebinding — a name whose TTL is one second, pointing
  first at an attacker and then at this server, makes a victim's own browser
  treat the attacker's page as same-origin with it, and CORS never gets a say.
  Empty by default, because there is no safe guess: this is reached by container
  name, by LAN address, by whatever the router calls it. Worth setting on
  anything using `AMS_NATIVE_AUTH=allowlist`, where a browser calling from an
  allowed address is all the credential an attacker needs. `/health`, `/ready`,
  `/ca.crt` and `/trust-ca.sh` answer under any name: the container's health
  check, a proxy's and a client's trust script ask by address, and none of them
  holds anything of anyone's.

## The audit trail

Every action that changes what the server serves is recorded: who did it, to
what, and from where. Reads are not — one row per metadata request would bury
everything that matters under Sonarr's refresh traffic. Failed sign-ins are.

Browse it under **Audit** in the web UI, or at `GET /api/v1/audit`.
`AMS_AUDIT_RETENTION_DAYS` bounds how much is kept (90 days by default; `0`
keeps everything).
