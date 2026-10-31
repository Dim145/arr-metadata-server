# What the server holds in memory

Measured with [`scripts/load/memory.sh`](../../scripts/load/memory.sh): the
image as [`compose.yaml`](../../compose.yaml) deploys it — a 512 MB ceiling, a
read-only root, `/tmp` on a tmpfs — on PostgreSQL 18, while Sonarr and Radarr
add the hundred most voted series and the hundred and fifty most voted films
on TMDB through it, four at a time, and then four quiet minutes; its pictures
are fetched and served as a deployment's are. Docker Desktop on one laptop:
8 CPUs, 8 GB, and a kernel that hands every process transparent huge pages.
Every five seconds a probe reads what the kernel holds for the server and
charges its container, and the server's metrics what the program holds.

An import is the heaviest a server sees, and the figures are for comparing
one build with the next, not for sizing a machine. Runs of the same build
come within some 5 % of each other.

## The import

| Run | The allocator | `docker stats`, mean / max | Its own memory, mean | The program's, mean | Its CPU | PostgreSQL's own memory, mean / max |
|---|---|---:|---:|---:|---:|---:|
| 0.11.1 | as it ships | 399 / 498 MB | 300 MB | 70 MB | — | 4 309 / 5 577 MB |
| fixed | as it ships | 381 / 512 MB | 296 MB | 67 MB | — | 108 / 158 MB |
| fixed, again | as it ships | 364 / 510 MB | 302 MB | 65 MB | 164 s | 110 / 243 MB |
| fixed | huge pages off | 341 / 512 MB | 302 MB | 65 MB | 176 s | 107 / 158 MB |
| fixed | given back at once | 338 / 488 MB | 262 MB | 67 MB | 178 s | 106 / 150 MB |
| fixed | huge pages off, given back after 100 ms | 248 / 332 MB | 219 MB | 70 MB | 183 s | 109 / 247 MB |
| fixed | huge pages off, given back at once | 180 / 258 MB | 148 MB | 66 MB | — | 106 / 162 MB |
| fixed, again | huge pages off, given back at once | 181 / 267 MB | 148 MB | 69 MB | 190 s | 108 / 161 MB |

*`docker stats`* is the container's working set
(`ams_cgroup_memory_working_set_bytes`), page cache included; *its own
memory*, the process's anonymous memory (`ams_memory_resident_bytes`); *the
program's*, what it holds from its allocator (`ams_memory_heap_bytes`), of
which the caches 18 MB; *its CPU*, the server's over the run's 410 s.
*Fixed* is what changed below; the allocator's two settings are mimalloc's,
`MIMALLOC_ALLOW_THP=0` and `MIMALLOC_PURGE_DELAY=0`, which the image now sets.

The published 0.11.1 adding twice as many — two hundred series and four
hundred films — was killed by the kernel for its memory after six minutes,
its own memory having gone from 330 MB to 503 MB in ten seconds.

## What held it

**PostgreSQL's sessions.** The statement cache is off on PostgreSQL — the way
around a fault of sqlx's `Any` driver, which binds a missing number as a
single-precision one — and sqlx then names every statement it prepares and
never closes one. Each query left its plan in the session for as long as the
connection lived: ninety thousand in one connection after a few minutes of
import, some 800 MB of PostgreSQL's memory for each connection of the pool.
PostgreSQL took all eight CPUs, the simplest statements waited minutes to be
parsed, and its connections were what the kernel killed first when the
virtual machine ran short. The statements are let
go of as each connection goes back to the pool (`DEALLOCATE ALL`, in the
simple protocol, which prepares nothing).

**The allocator.** The program held some 70 MB; the process four times as
much. The pictures' copies are made in buffers of tens of megabytes, freed
and made again all day: mimalloc kept what was freed for a second before
giving it back, and with huge pages, gave back nothing of the two megabytes
a block still sat in. Either alone changes little — what is given back at
once still sits in huge pages, and pages of the system's own size are still
kept a second — and both together halve what the server holds, for some 15 %
more of its CPU: the pages given back are asked for again.

**A long series' answers.** Storing a work held each provider's answer twice
while it was merged and written: parsed, and a parsed copy for the snapshots,
which are written as text. They are kept as that text now, and the answers
let go of. What the program held, one series at a time:

| Series | Episodes | Before | After |
|---|---:|---:|---:|
| Breaking Bad | 80 | 17 MB | 14 MB |
| One Piece | 1 242 | 49 MB | 32 MB |
| Coronation Street | 9 973 | 210 MB | 123 MB |

**The pictures' placeholders.** With the WebP copies off, as they are
unless `media.webp` turns them on, a picture is decoded for its placeholder
alone — a sketch of a hundred pixels — and it was decoded whole, then held
again as RGBA. A JPEG is now drawn at an eighth of its sides or a quarter by
the decoder's inverse transform (`jpeg-decoder`; `image`'s own draws whole).
TMDB's originals, one at a time, in a release build:

| Picture | Whole | Small |
|---|---:|---:|
| Poster, 2000×3000 | 20–26 ms, 42 MB | 13–20 ms, 0.7 MB |
| Backdrop, 1920×1080 | 8.8 ms, 14.5 MB | 6.6 ms, 0.3 MB |
| Backdrop, 3840×2160, progressive | 46 ms, 58 MB | 50 ms, 26 MB |

A progressive JPEG has every coefficient read before any of it is drawn,
which keeps half of what it took; the others keep under a megabyte. The
sketch is the same within what a JPEG's rounding moves.

**The anime list.** Read whole as documents before its elements were, its
seven and a half megabytes took the server to 150 MB at a start where the list
was due; read an element at a time, to 105.

## What a deployment can change

- The image sets `MIMALLOC_PURGE_DELAY=0` and `MIMALLOC_ALLOW_THP=0`. Setting
  `MIMALLOC_PURGE_DELAY=1000` and `MIMALLOC_ALLOW_THP=1` on the container
  undoes them, for the CPU they cost; a server run from a binary sets them in
  its own environment. Where the kernel hands out huge pages only to whoever
  asks (`/sys/kernel/mm/transparent_hugepage/enabled` says `[madvise]` or
  `[never]`), the second changes nothing.
- The caches are measured in bytes, `AMS_CACHE_MAX_ENTRIES` of 16 KiB each for
  the works (see [Configuration](../configuration.md)): what they hold is the
  administration's **Cache** page, and `ams_cache_bytes`.
- A ceiling well above what the server holds leaves room for the page cache of
  the pictures it serves, which the kernel takes back first when the container
  nears it: `ams_cgroup_memory_ceiling_hits_total` climbing says it does.

## Measuring again

```bash
./scripts/load/memory.sh
```

```bash
IMAGE=arr-metadata-server:dev SERIES=100 MOVIES=150 QUIET=240 ./scripts/load/memory.sh
```

`MEMORY_ENV` names a file of variables for the server — the allocator's, a
cache's size — and `LIMIT` and `MEDIA` the ceiling and the media store. The
summary is a table like the ones above, the probe's and the scrape's lines
beside it in the work directory.
