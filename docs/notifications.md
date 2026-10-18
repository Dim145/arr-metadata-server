# Notifications

*Administration › System › Notifications* (`/admin/notifications`, an
administrator's page) is where the server tells somebody what happens: a work
imported or deleted, a refresh that started failing, a picture given up on, a
newer version out, any action of the audit trail. Each **destination** is an
address, the way it is posted to, and the events ticked for it, with a test and a
log. There can be as many as you like.

Nothing is sent from the request that did the thing. An event is written down once
for each destination that wants it, before anything is sent, and a worker of its own
sends it, tries it again when it is not taken, and keeps a log of what came of it. A
slow or failing destination costs the action nothing, and a restart loses nothing
owed.

## Kinds of destination

| Kind | Address | What is posted |
|---|---|---|
| **Standard Webhooks** (`standard`) | Your receiver's: `https://example.com/webhooks/catalogue` | `application/json`, `{"type", "timestamp", "data": {"actor", "target", "detail"}}`, signed (below) |
| **JSON, as before** (`json`) | Any address that takes JSON | The body the single webhook of earlier versions was posted: `event`, `actor`, `target`, `detail`, `at`, and a one-line `content` and `text`. Not signed |
| **Discord** (`discord`) | A channel webhook's: `https://discord.com/api/webhooks/…` | `{"content": "<the line>", "allowed_mentions": {"parse": []}}`: the line alone, cut to fit Discord's limit, with mentions switched off, so that an `@everyone` in a title is shown and not obeyed |
| **Slack** (`slack`) | An incoming webhook's: `https://hooks.slack.com/services/…` | `{"text": "<the line>"}`, with `&`, `<` and `>` escaped as Slack asks, so that nothing in a title is read as a mention |
| **ntfy** (`ntfy`) | The topic's: `https://ntfy.sh/your-topic`, or your own server's | The message as the body, with `Title`, `Tags` and `Priority` as headers (3, or 4 for a failure or a refused sign-in), and `Authorization: Bearer …` when a token is set. A title that is not ASCII is encoded as RFC 2047 says |
| **Gotify** (`gotify`) | The server's: `https://gotify.example.com`; `/message` is added | `{"title", "message", "priority"}` (5, or 8 for a failure), with the application's token in `X-Gotify-Key`. The token is required |
| **Apprise** (`apprise`) | An Apprise API's notify address with its key: `http://apprise:8000/notify/apprise` | `{"title", "body", "type"}`, the type being `info`, `warning` (a refused sign-in) or `failure` (a failed refresh, a medium given up on). Credentials, where the API asks for them, can be written in the address |

The line a chat shows reads `Cinémathèque · item.imported · series 01a0… · by admin`:
the event, what it says, and who did it.

The address must be an `http` or `https` one, at most 2000 bytes, and must not name
this machine or an address only it can reach — loopback, link-local, a cloud's
metadata service. It is checked when the destination is saved, and what its name
resolves to is checked again at every post. Addresses on your own network
(`192.168.x.x`, `10.x`, a Tailscale address) are fine: an ntfy or a Gotify beside
the server is the usual case. A name is at most 100 characters; a token is at most
500 visible characters. Webhooks are posted directly, whatever `HTTP_PROXY` says.

## Events

Each destination ticks its own events:

- **Every event** (`*`), those added in later versions included.
- **What happens on its own**, three facts nobody did: `refresh.failed`, once, when a
  work starts failing — not at each try while it goes on, and not for a supplement
  that did not answer (TVmaze, Fanart.tv, AniList, MyAnimeList): the work is
  refreshed all the same ([grouped](#failed-refreshes-grouped) for the kinds people
  read); `media.failed`, a picture or
  a theme whose every fetch failed; `update.available`, once per version, while the
  check for a newer version (`updates.check`) is on.
- **Any action of the audit trail**: `item.imported`, `item.created`, `item.deleted`,
  `item.refreshed`, `item.renumbered`, `override.set`, `list.updated`,
  `setting.changed`, `notification.created`… The names are on the destination's form
  and in `GET /api/v1/admin/webhooks/events`.

A new destination starts with what enters and leaves the catalogue: `item.imported`,
`item.created`, `item.deleted` and `dataset.imported`. An event name is lowercase
letters, digits, dots and underscores; a destination names 200 at the most.

Two things are never passed on: the name typed into a failed sign-in, which was
typed by whoever came to the door with no credential, and a destination's address or
token. Changes to the destinations are in the audit trail too —
`notification.created`, `notification.updated`, `notification.deleted` — by name, never
with the address or the token, and can be told to a destination like any other
action.

## Failed refreshes, grouped

A destination with **Group failed refreshes** on is not posted a message for each work
whose refresh failed. The failures wait five minutes from the first, then go as one
`refresh.failed` message: how many, counted by cause — the source and the kind of
failure — and the first works named.

```
34 refreshes failed since 2026-10-18T02:10:04Z
TheTVDB — timed out: 31
TMDB — 404 Not Found: 3
“Breaking Bad” (series 01…), “Dark” (series 01…), …, and 26 more
```

An outage of a source is one message rather than one per work it touched; a failure
alone in its five minutes goes as it came. The message's title carries the count
(`refresh.failed × 34`), and its JSON a `count` field, without a `target`.

It is on by default for the kinds people read — Discord, Slack, ntfy, Gotify,
Apprise — and off for `standard` and `json`, read by programs that want an event per
work; the destination's form switches it, and the API takes `grouped`. What waits is
held in memory: a restart within the five minutes loses it, and each of several
instances groups its own.

## Verifying a Standard Webhooks signature

A `standard` destination's **signing secret**, `whsec_…`, is made by the server and
shown once, when the destination is saved: copy it into the receiver. It cannot be
read again. *Make a new secret* in the destination's form replaces it, and the
receiver needs the new one before the next event. At another address, a destination
is given a new secret of its own when it is saved.

Every post carries the three headers the
[specification](https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md)
defines:

- `webhook-id`: the event's id, `msg_…`. It is the same for every try and every
  replay, so a receiver can keep the ones it handled for a few minutes and drop a
  repeat.
- `webhook-timestamp`: when this try was signed, in seconds since 1970. Refuse one
  more than five minutes away from your clock.
- `webhook-signature`: `v1,` and the base64 of the HMAC-SHA256 of
  `{webhook-id}.{webhook-timestamp}.{body}`, keyed with the secret's bytes — the
  base64 after `whsec_`, decoded. Compare in constant time, over the body exactly as
  it arrived.

In Python, with nothing but the standard library:

```python
import base64, hashlib, hmac, time

def verify(secret: str, headers: dict, body: bytes) -> bool:
    msg_id = headers["webhook-id"]
    timestamp = headers["webhook-timestamp"]
    if abs(time.time() - int(timestamp)) > 300:
        return False
    key = base64.b64decode(secret.removeprefix("whsec_"))
    signed = f"{msg_id}.{timestamp}.".encode() + body
    expected = base64.b64encode(hmac.new(key, signed, hashlib.sha256).digest()).decode()
    return any(
        hmac.compare_digest(expected, signature.split(",", 1)[1])
        for signature in headers["webhook-signature"].split()
        if signature.startswith("v1,")
    )
```

The Standard Webhooks libraries do the same in one call. The signature is checked in
this server's own tests against the example of the specification's reference
library.

## Tries, and the log

A post is taken when the address answers with a success (`2xx`) within 15 seconds; it
is given 10 to be reached at all. Anything else — another status, a timeout, an address
that cannot be reached, a redirect — is tried again after **5 seconds, 5 minutes, 30
minutes, 2 hours, 5 hours, 10 hours and 10 hours**: eight tries in all, over a day and
more, and then given up on.

- A redirect (`3xx`) is a failure, not something to follow: the address has moved, and
  the log says so, as Standard Webhooks asks.
- An answer with `Retry-After` is left alone that long when it is longer than the
  pause, up to ten hours.
- **`410 Gone`** switches the destination off at once, and the page says why. Switch
  it on again once the address is fixed.
- A destination switched off, by the server or by you, is owed nothing: what waited
  for it is marked given up, and can be replayed once it is on again.
- A destination with a thousand deliveries waiting is owed no more until it catches
  up, rather than queued for without end.
- An address that does not answer at all — it times out, or no connection is made — is
  posted to once for everything due to it, and what else is due to it is put off as
  that try was, each delivery by its own schedule. A destination is posted to one
  delivery at a time, in the order they are owed, and a dead address holds up no other
  destination.

**Log**, on a destination, lists what it was told, the newest first: when, the event
and what it was about, what came of it (*Delivered*, *Waiting*, *Given up*, with the
status answered or why the address was not reached, and how long it took), the tries
made and the next one. **Replay** sends a delivered or given-up one again, under the
same `webhook-id`. The log is kept **thirty days**, and **the newest thousand** of each
destination.

**Test** posts `webhook.test` to the destination at once, whatever is ticked and even
while it is switched off, and says what came of it: the status the address answered
and how long it took or, when it was not reached, why — and, when the address refused
the post, the start of what it answered, with the destination's secrets masked. A
test is written nowhere: neither in the audit trail nor in the log.

Among several instances, any instance sends what is due, each delivery once.

## Upgrading from the single webhook

Before 0.8.0 one webhook was set on the Settings page, in `webhooks.url` and
`webhooks.events`. At the first start of 0.8.0, if an address was set and no
destination existed, it became the destination **Webhook**, of the kind *JSON, as
before*: the same address, the same events — the default ones if none were set — and
the same body. Both settings were then removed. Nothing reads them any more, and the
settings API refuses them as unknown. To have it signed, change its kind to *Standard
Webhooks* and give the receiver the secret shown.

`POST /api/v1/admin/webhooks/test` is gone too, in favour of
`POST /api/v1/admin/notifications/{id}/test`, one test for each destination.
`GET /api/v1/admin/webhooks/events` stays.

## Secrets and addresses

A destination's address is never shown whole: the API and the page show its scheme,
host and port, and mask the rest, since the token of a Discord, Slack or ntfy address
is in its path. Signing secrets and tokens are write-only. None of them is written to
the log.

A token kept for an ntfy or Gotify destination goes to no other server and no other
kind of destination unless it is given again, and a Standard Webhooks receiver at a
new address is given a signing secret of its own.

An `http://` address is accepted, for a service on your own network, and is posted
to in clear: the body, and the token of an ntfy or Gotify destination in its
header, can be read on the way. Give an `https://` address to anything the post
has to cross a network you do not trust to reach. A Standard Webhooks signature
gives nothing of its secret away, but the body it signs travels in clear all the
same.

## API

All of it for administrators, under `/api/v1`, and in the OpenAPI document:

| Route | What it does |
|---|---|
| `GET /admin/webhooks/events` | The facts, the audit trail's actions, the default events and the kinds. |
| `GET /admin/notifications` | Every destination: `address` masked past the host, `secretSet`, `events`, `enabled`, `grouped`, `disabledReason`, and the counts of its log. |
| `POST /admin/notifications` | Add one: `{name, kind, address, token?, events?, enabled?, grouped?}` — `grouped` left out follows the kind. A `standard` one's `secret` is in this answer only. |
| `PATCH /admin/notifications/{id}` | Change one; each field left out stays. An empty `address` keeps it; `token` left out keeps it, empty removes it. Made `standard`, its new `secret` is in the answer. |
| `DELETE /admin/notifications/{id}` | Delete one, with its log. |
| `POST /admin/notifications/{id}/secret` | Make a standard destination's signing secret anew; shown in the answer only. |
| `POST /admin/notifications/{id}/test` | Post `webhook.test`: `{delivered, status?, error?, refused?, millis}`, where `refused` is `not_web`, `internal` or `unsent`. |
| `GET /admin/notifications/{id}/deliveries?limit=&offset=` | A page of its log, the newest first (50 by default, 200 at the most), and the total. |
| `POST /admin/notifications/{id}/deliveries/{delivery}/replay` | Tell a delivered or given-up one again; refused with `409` while the destination is off or the delivery still waits. |
