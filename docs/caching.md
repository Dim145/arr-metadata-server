# Caching, and a cache server

Everything the server answers often is kept under its hand: a merged work,
a search, a page of the catalogue, a relayed list, what the TMDB relay
answered Jellyseerr, the session behind a cookie. The first tier is the
process's own memory, sized in bytes and expired per kind
(`AMS_CACHE_MAX_ENTRIES`, `AMS_CACHE_ITEM_TTL`, `AMS_CACHE_SEARCH_TTL`,
`AMS_CACHE_SESSION_TTL`). Behind it, when `AMS_REDIS_URL` names one, a
**Valkey or Redis** server holds a second tier: shared between instances,
kept from one start to the next, and told by each instance what it forgot,
so an edit on one is seen by all. Every key it writes carries
`AMS_REDIS_PREFIX` (`ams:`), and that prefix is the one thing it deletes: a
server may be shared. Nothing durable lives there — sessions, keys, settings
and the audit trail are in the database — so it can be emptied at any time;
a server that is down at start is attached when it answers, and one that
stops answering is waited for while the memory carries on alone.

A page anyone may read, read by nobody in particular, carries
`Cache-Control: public, max-age=60, stale-while-revalidate=300` and a `Vary`
on the credentials, so a browser or a proxy in front — Caddy, nginx, a
CDN — absorbs the anonymous traffic without touching the server
(`AMS_PUBLIC_CACHE_SECONDS`; 0 keeps the interface's `no-cache`).

The administration's **Cache** page reads it all back: what each space
holds in each tier and how often it answered, the server's memory,
eviction policy and round trip, a switch and a *Flush* per space, and a
flush of everything under the prefix — each in the audit trail. The same
figures are scraped from `/api/v1/admin/metrics`; [`docs/monitoring/`](monitoring/README.md) holds
a Prometheus job and a Grafana dashboard to import.

Run the server yourself with `maxmemory` and `maxmemory-policy allkeys-lru`,
persistence off, and an ACL user confined to this server's keys and its
channel — it reads, writes, walks and unlinks under the prefix, publishes
and subscribes on `ams:events`, pings and reads `INFO`:

```
ACL SETUSER ams on >secret ~ams:* &ams:events +@read +@write +@keyspace +@pubsub +@connection +info
```

and then:

```yaml
  cache:
    image: valkey/valkey:8-alpine
    command: valkey-server --maxmemory 256mb --maxmemory-policy allkeys-lru --save "" --appendonly no
```

and `AMS_REDIS_URL=redis://cache:6379` on the metadata server. The server
trusts what it reads there and what it hears on its channel: give it a
database of its own, or one shared only with instances of itself. A
session revoked on one instance is forgotten on the others at once through
the channel, or within `AMS_CACHE_SESSION_TTL` when the server is away;
relayed lists of several megabytes stay in memory only. Title searches
read an index rather than every row since migration 0022: FTS5 trigrams on
SQLite, `pg_trgm` on PostgreSQL where the extension can be created.

[`scripts/load/serve.sh`](../scripts/load/serve.sh) starts a release build over a copy of a catalogue,
and [`scripts/load/baseline.sh`](../scripts/load/baseline.sh) measures the requests that matter with
[oha](https://github.com/hatoo/oha); [`docs/perf/`](perf/README.md) keeps the figures.

Each space's switch on the **Cache** page is a setting: see [Cache](settings.md#cache) in the
settings reference. Every variable of the cache is in
[Configuration](configuration.md#the-cache).
