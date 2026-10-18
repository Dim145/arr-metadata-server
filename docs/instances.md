# Several instances

One instance is the default, and stays as simple as it is: `AMS_MODE=single`
— SQLite or PostgreSQL, the media on disk or in a bucket, every job
scheduled by the process itself. `AMS_MODE=multi` runs several instances
as one, behind whatever balances the interface's port between them:

* **PostgreSQL** is required (`AMS_DATABASE_URL=postgres://…`): several
  processes cannot share an SQLite file.
* **The media in a bucket** (`AMS_MEDIA_STORAGE=s3`), or kept nowhere
  (`off`): a directory is one instance's own.
* **A cache server** is required (`AMS_REDIS_URL`): it is the bus the
  instances coordinate on, besides being the second tier.

The server refuses to start in `multi` mode without the three, and says
which is missing. With them, the instances agree on a **leader** through a
lease on the cache server — renewed every five seconds, lost within
fifteen when the leader goes, taken by another within five more — and the leader
alone runs the schedules: the refresh sweep, the dataset imports, the
listing, the media sweep, the certificate's renewal. Every instance answers
requests, fetches the media in line (each medium claimed by one worker),
and can run a task by hand: a task is held on the cache server while it
runs, so two instances never run the same. What one instance changes, the
others are told through the channel: a setting, a network rule, a session
revoked, a medium kept or forgotten, a run to stop, a certificate issued.
The counters on *Opening & APIs* add up what every instance answered, and
the rate limits — requests per minute, sign-ups per hour — are one quota
across them all, counted on the cache server.

Two things move into the database in `multi` mode, so an instance started
afresh serves what the others do: the key the identity provider's cookies
are sealed with, and — for the clients' door — the authority the clients
trust and the certificate it issued. An authority already kept in
`AMS_TLS_DIR` is taken into the database the first time, so the clients
need not trust a new one; back the database up accordingly, since it holds
the authority's key from then on. Each instance's clients' door serves
that one certificate, so a client may reach any of them; the operator's
own certificate (`AMS_CLIENTS_TLS_CERT` / `_KEY`) works as before, from
files every instance has.

`AMS_INSTANCE_NAME` names an instance among the others — set it: the
hostname is the fallback, and a container's hostname is its id unless
`hostname:` says otherwise, so two instances or two lives of one would not
be told apart. The administration's **Cache** page lists them with who
leads, and the dashboard says how many there are. The metrics carry
`ams_leader` and `ams_instances`. The lease, the holds and the windows
live on the cache server beside the cache: with `allkeys-lru` they are
what is touched most and evicted last, but a server that runs out of
memory is a server that may forget who leads — size it so that it never
does, or give the instances a server of their own for coordination.
[`compose.multi.yaml`](../compose.multi.yaml) is a complete stack — PostgreSQL, Valkey, a Garage
bucket, two instances, Caddy in front — and [`scripts/e2e-multi.sh`](../scripts/e2e-multi.sh) starts
two instances against a PostgreSQL of its own and checks that they agree,
pass on what changes, count together and hand the lead over.

What it does not do: a `multi` deployment scales the requests, not the
database or the bucket, which become the ceiling; with the cache server
away, the caches keep to memory as before, but no instance leads and
nothing scheduled runs until it answers — a request is never held up by
it. A task is held on the cache server for a minute at a time, renewed
while it runs: a cache server restarted or evicting keys can lose the hold,
and another instance asked to start the same task in that minute would run
it a second time — wasteful, never wrong.

The interface's port of every instance is published by whatever balances them — Caddy, in
`compose.multi.yaml`. What that proxy has to send, and what the instances need to know of it,
is in [Behind a reverse proxy](reverse-proxy.md).
