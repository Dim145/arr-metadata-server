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

## The ranked search, over fifty thousand works (migration 0039)

The development catalogue is too small for a search to cost anything, so
the search index was measured over a made-up one: 50 000 works (30 000
films, 20 000 series with 345 214 episodes), 133 294 alternative titles,
74 856 translations and 300 000 credits among 39 976 people, the words of
every title drawn from the system word list by Zipf's law — a word's chance
falls as one over its rank — and three in ten from a short list of common
ones, `the`, `love`, `night`… Both builds are release builds with thin LTO
and 16 codegen units (fat LTO takes minutes to link, and the comparison is
what counts): *before* is fae9bf4, *after* the tree with the search index.
The two servers ran side by side over copies of the catalogue, the after
one's index written first, each scenario 10 s at 32 connections against one
and then the other. A scenario of random terms (`oha --rand-regex-url`)
meets most of its terms cold; a kept one is asked once before it is timed.

| Scenario | before req/s | p50 | p99 | after req/s | p50 | p99 |
|---|---:|---:|---:|---:|---:|---:|
| catalogue list (24 series), kept | 54 488 | 0.5 ms | 2.6 ms | 60 334 | 0.4 ms | 2.1 ms |
| search "love", kept | 56 011 | 0.5 ms | 2.5 ms | 60 367 | 0.4 ms | 2.3 ms |
| search, a common word + 0–2 letters | 44 | 587.3 ms | 3 638.0 ms | 19 | 1 259.6 ms | 6 299.1 ms |
| search, five random letters | 41 | 805.5 ms | 1 069.2 ms | 235 | 103.3 ms | 359.8 ms |
| series search, a common word + 0–2 letters | 54 | 505.6 ms | 2 988.9 ms | 40 | 665.8 ms | 4 377.2 ms |
| facets, a common word + 0–2 letters | 53 | 633.0 ms | 1 816.4 ms | 11 | 4 234.1 ms | 9 053.6 ms |
| Sonarr search "love", kept | 4 472 | 7.0 ms | 10.0 ms | 4 393 | 7.2 ms | 10.3 ms |

The command palette's own search, `/api/v1/search` (works, people and
episodes at once), which the before build does not have: 18 requests a
second for a common word and a letter or two (p50 1 906.2 ms), 121 for five
random letters (p50 141.5 ms).

What moved, and why:

- **A kept answer costs what it did**: the cache answers before anything is
  searched. The gap between the two columns is the spread between runs of a
  laptop serving itself.
- **A term no name holds** — five random letters, which is what a
  misspelling looks like to the index — is answered six times as often. The
  before build matched it against every work's titles and found nothing in
  800 ms at this load; the after build finds nothing in a few rows of its
  two reads by the letters, then asks for the names sharing most of the
  term's trigrams, each trigram's rows counted.
- **A common word, cold, costs more.** To rank, the after build reads the
  best-known 200 of the names that start with the term and of those that
  hold it — for `love`, 2 254 and 5 879 names, each joined to its work and
  sorted by popularity — where the before build read the titles once; a
  common word with a letter or two after it mostly finds nothing by its
  letters, and pays for the misspelling read as well.
- **The facets** of a term read its works three times: for the genres and
  the rest, then for the countries and the ratings, which are new. When
  first measured they read more: the whole search first, only to learn
  whether a misspelling was forgiven (they now glance at fifty rows of it),
  and the ratings' subquery once for each branch of their `UNION` (now one
  common table).
- **A search of series** read every series through the index of their kind
  in the first measurement — without statistics, SQLite takes
  `kind = 'series'` for a handful of rows — and took 8 s at p50; when the
  search index names the works, those columns are now written `+kind`,
  SQLite's way of keeping a column out of its choice of index.
- **At 32 connections the limit is SQLite's, not the search's**: one
  connection answers a cold search of a common word in tens of
  milliseconds, but thirty-two share the bundled SQLite's memory-statistics
  mutex, which every allocation takes, and read a 596 MB file through page
  caches of 2 MB each. Two settings lift both; they are measured below, and
  are not part of this change.

### The same, with two settings of SQLite's (proposed, not applied)

Both builds again, each with `PRAGMA mmap_size = 1073741824` on every
connection (`src/db/mod.rs`) and the bundled SQLite compiled with
`SQLITE_DEFAULT_MEMSTATUS=0` (`LIBSQLITE3_FLAGS`), which SQLite's own list of
recommended options carries:

| Scenario | before req/s | p50 | p99 | after req/s | p50 | p99 |
|---|---:|---:|---:|---:|---:|---:|
| catalogue list (24 series), kept | 71 185 | 0.4 ms | 1.5 ms | 63 130 | 0.4 ms | 2.2 ms |
| search "love", kept | 68 765 | 0.4 ms | 1.6 ms | 48 359 | 0.5 ms | 3.5 ms |
| search, a common word + 0–2 letters | 861 | 1.5 ms | 334.3 ms | 2 219 | 0.4 ms | 89.2 ms |
| search, five random letters | 332 | 94.3 ms | 144.1 ms | 2 511 | 9.4 ms | 40.9 ms |
| series search, a common word + 0–2 letters | 1 029 | 1.4 ms | 247.0 ms | 1 438 | 0.6 ms | 126.2 ms |
| facets, a common word + 0–2 letters | 376 | 85.9 ms | 167.2 ms | 362 | 37.7 ms | 319.4 ms |
| Sonarr search "love", kept | 3 969 | 7.9 ms | 11.9 ms | 3 205 | 8.9 ms | 24.8 ms |

The palette's search: 1 187 a second for a common word and a letter or two
(p50 0.6 ms), 2 199 for five random letters (p50 12.1 ms). Apart, the map
multiplies the after build's cold scenarios by two to eight and the
statistics left out by one and a half to four; together, by ten (five
random letters) to more than a hundred (a common word), and its facets by
thirty. With both, it answers a cold search one and a half to seven times
as often as the before build with the same settings, and the facets as
often.

### The first start after the upgrade

On this catalogue the first start answers after 10 s — the archive the
server makes of the database before it migrates, 9 s for 336 MB; the
migrations themselves take 2 — then writes the search index in the
background: whole after 255 s (258 080 names of works, 39 976 people,
345 214 episodes), the search answering from what is written meanwhile. The
file grows from 336 MB to 596 MB: the index's table, its FTS5 trigrams and
its three indexes weigh 283 MB, less the 0022 index the migration drops.
