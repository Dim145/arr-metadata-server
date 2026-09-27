# What the server answers under load

Measured with [`scripts/load/baseline.sh`](../../scripts/load/baseline.sh)
against [`scripts/load/serve.sh`](../../scripts/load/serve.sh): a release
build over a copy of the development catalogue (a few hundred works, The
Simpsons with its 800 episodes among them), 10 s at 32 connections per
scenario, on one laptop — the figures are for comparing one build with the
next, not for sizing a machine. `oha` reports the latency percentiles over
the whole run.

## Before the cache work (commit 5d721df, release build)

| Scenario | req/s | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| catalogue list (24 series) | 128 | 248.6 ms | 292.9 ms | 323.3 ms |
| one series (The Simpsons, whole) | 3 607 | 8.4 ms | 14.9 ms | 20.7 ms |
| one film | 23 299 | 1.3 ms | 2.2 ms | 3.7 ms |
| search "the" (`term=`) | — | — | — | — |
| Sonarr search | 2 738 | 10.2 ms | 18.3 ms | 24.7 ms |
| Sonarr show | 2 854 | 10.8 ms | 14.3 ms | 17.8 ms |
| interface document | 99 823 | 0.3 ms | 0.6 ms | 1.1 ms |

The search row of that first run measured a plain list (the parameter is
`term`, not `q`, and the script said `q`); the list row is the one to
compare. The catalogue list — every page of the interface's catalogue and
browse views — read the database on every request: a query with its
filters, a count with the same filters, the artwork of every card. The
search added a `LIKE '%term%'` over every title, twice.

## After the cache work, memory alone (release build)

| Scenario | req/s | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| catalogue list (24 series) | 82 712 | 0.3 ms | 0.6 ms | 1.2 ms |
| one series (The Simpsons, whole) | 4 024 | 7.7 ms | 13.1 ms | 19.1 ms |
| one film | 30 276 | 1.0 ms | 1.7 ms | 2.8 ms |
| search "the" | 98 365 | 0.3 ms | 0.5 ms | 1.0 ms |
| Sonarr search | 3 110 | 6.8 ms | 26.5 ms | 52.8 ms |
| Sonarr show | 3 098 | 10.0 ms | 12.9 ms | 14.9 ms |
| interface document | 123 513 | 0.2 ms | 0.5 ms | 0.9 ms |

## The same, with a Valkey behind the memory

| Scenario | req/s | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| catalogue list (24 series) | 82 003 | 0.3 ms | 0.6 ms | 1.2 ms |
| one series (The Simpsons, whole) | 3 961 | 7.7 ms | 13.4 ms | 20.1 ms |
| one film | 29 100 | 1.0 ms | 1.8 ms | 3.2 ms |
| search "the" | 98 274 | 0.3 ms | 0.5 ms | 1.0 ms |
| Sonarr search | 3 032 | 8.1 ms | 23.1 ms | 46.0 ms |
| Sonarr show | 3 199 | 9.5 ms | 12.5 ms | 18.2 ms |
| interface document | 122 701 | 0.2 ms | 0.5 ms | 0.9 ms |

What moved, and why:

- **The catalogue list and the search**, 128 requests a second at a quarter
  of a second each, are answered from the cache now — a page of the
  catalogue is kept under a key that every write and every settings change
  moves on, so a list never outlives the works it shows. The first read of
  a page still runs the query; the search's first read now runs on the
  trigram index rather than over every title.
- **A single work and the interface's document** gained a fifth to a
  third with no change of their own: the allocator (mimalloc) and the
  session read from memory rather than the database on every
  authenticated request.
- **A whole series** is still 8 ms: its 800 episodes are decoded from the
  cached document and encoded again for the answer on every request, which
  is where that time goes. Keeping the answer itself, by language and
  reader, is the next step if that page matters at scale.
- **Valkey costs nothing on a hit** — the memory answers first — and
  holds what the memory would lose at the next start, or what another
  instance would otherwise compute again.
- The percentiles are of a laptop serving itself over loopback; compare
  rows between runs, not with a production host.
