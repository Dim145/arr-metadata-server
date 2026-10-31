# Watching the server

Four ways to see what the server does, each for its own question, and each off or quiet until
you ask for it:

- **figures**, for Prometheus: how much, how fast, how full — [below](#figures);
- **alerts** on those figures, for when something needs a person — [Alerts](#alerts);
- **traces**, for one request followed from the door to the provider, in Tempo or Jaeger —
  [Traces](#traces);
- **the log**, in the administration and on standard output, every line naming the trace of its
  request, and **error reports** sent to a Sentry or a GlitchTip — [The log](#the-log) and
  [Error reports](#error-reports).

## Figures

The server counts what it does and offers it at `/api/v1/admin/metrics`, in Prometheus's text
format: requests by surface and by status, with their latency; calls made to each provider and
how they ended; what every cache answered from each tier; the cache server's memory, evictions
and round trip; the database's pool of connections and how long a connection was waited for;
what waits in each queue; the catalogue's size, the works whose last refresh failed, the jobs'
runs; when the certificates end and how old the newest archive is. The endpoint is
administrative — it says which surfaces are open and how the caches are doing — so the scrape
carries a key: a scraper's own, or an administrator's.

1. Make a key under **Keys & network** for the scraper, with the `metrics` scope and no other,
   and put it in a file Prometheus can read, mode 600. A key made with `metrics` alone reads the
   figures and nothing else: not the catalogue, not the settings, not the other keys. It sits in
   a file more eyes read than a person's key ever is, and what it opens, should it be read, is
   the figures. (A key with the `admin` scope scrapes too, and opens everything; a person's own
   key cannot carry `metrics`.)
2. Add the job in [`prometheus.yml`](prometheus.yml) to your Prometheus configuration; it sends
   the key as a bearer token, which the server takes like `X-Api-Key`.
3. Import [`grafana-dashboard.json`](grafana-dashboard.json) in Grafana (*Dashboards → New →
   Import*) and pick your Prometheus data source. The dashboard reads every series the server
   offers, by instance.

The same figures are on the administration's **Cache** page and its dashboard, read from the
server directly; Prometheus keeps the history.

| Series | What it says |
| --- | --- |
| `ams_http_requests_total{surface,status}` | Requests answered, by surface and status class: `2xx`, `5xx`. |
| `ams_http_responses_total{surface,status}` | The same by status, `200`, `502`: those answered at least once. |
| `ams_http_request_duration_seconds` | How long requests took, by surface; a histogram. |
| `ams_upstream_requests_total{provider,outcome}` | Calls to each provider: `ok` (a 404 is an answer), `refused`, `failed`, `unreachable`. |
| `ams_upstream_duration_seconds` | How long each provider took; a histogram. |
| `ams_db_pool_size`, `ams_db_pool_idle`, `ams_db_pool_max` | The database's connections: open, idle, and the most the pool opens. |
| `ams_db_pool_wait_seconds` | How long each connection was waited for; a histogram. |
| `ams_queue_depth{kind}` | What waits: `refresh`, works due for a refresh; `media`, pictures and themes to fetch; `webhooks`, deliveries to send. |
| `ams_refresh_failed` | Works whose last refresh failed. |
| `ams_tls_expiry_seconds{certificate}` | Seconds left before a certificate ends — `interface`, `clients`, `authority` — below nought once it has. |
| `ams_backup_age_seconds` | How long ago the newest archive was made, while the schedule is on and one exists. |
| `ams_leader{ams_instance}`, `ams_instances` | Which instance runs the schedules, and how many were heard of lately. |
| `ams_cache_*`, `ams_redis_*` | Each cache's entries, bytes, hits and misses by tier — `copies` is the pictures' copies made in the last minute; the cache server's state. |
| `ams_memory_heap_bytes` | What the program holds from its allocator: its own data, without what the allocator keeps beside it for reuse. |
| `ams_memory_resident_bytes{kind}`, `ams_memory_resident_peak_bytes`, `ams_process_threads` | What the kernel keeps resident for the process — `anonymous` (the heap, what the allocator keeps for reuse, the stacks), `file` (the binary's pages), `shared` — the most it ever held, and its threads. On Linux. |
| `ams_cgroup_memory_bytes`, `ams_cgroup_memory_working_set_bytes`, `ams_cgroup_memory_limit_bytes` | What the container is charged, page cache included; the working set, which is what `docker stats` shows; and its ceiling, when it has one. On Linux. |
| `ams_cgroup_memory_stat_bytes{kind}`, `ams_cgroup_memory_ceiling_hits_total` | The charge by kind, as `memory.stat` counts it (`anon`, `active_file`, `inactive_file`, `shmem`, `kernel`, `sock`, `file`), and how many times the container reached its ceiling and the kernel took memory back to stay under it. |
| `ams_works`, `ams_episodes`, `ams_locks`, `ams_job_runs` | What the catalogue holds, and the jobs' runs. |

With several instances (`AMS_MODE=multi`), scrape each one: the caches' first tier, the request
latencies, the pool and the uptime are each instance's own, while the cache server's figures and
what the database holds are the same from every instance. `ams_leader{ams_instance="…"}` is 1 on
the one that runs the schedules — the label is not called `instance`, which Prometheus reserves
for the target — and `ams_instances` says how many were heard of lately.

## Alerts

[`alerts.yml`](alerts.yml) holds the alerts, to load beside the scrape job
(`rule_files: [alerts.yml]`) and route where you read them:

| Alert | When |
| --- | --- |
| `ArrMetadataDown`, `ArrMetadataNotScraped` | The scrape has failed for five minutes, or there is no target to scrape. |
| `ArrMetadataNoLeader`, `ArrMetadataTwoLeaders` | Among several instances, none or more than one runs the schedules. |
| `ArrMetadataServerErrors` | More than two answers in a hundred were 5xx, over ten minutes. |
| `ArrMetadataSourceFailing` | A provider fails more than half of what it is asked, over fifteen minutes. |
| `ArrMetadataRefreshFailuresRising` | More than five works failed their refresh in the last hour. |
| `ArrMetadataValkeyUnreachable` | The cache server has not answered for two minutes. |
| `ArrMetadataDatabasePoolExhausted` | One request in twenty waits more than half a second for a connection. |
| `ArrMetadataCertificateExpiring` | A certificate ends in less than thirty days. |
| `ArrMetadataBackupStale` | The newest archive is more than eight days old. |

They expect the scrape job to be called `arr-metadata-server`, as `prometheus.yml` calls it:
rename it in the two `up` rules if yours is not. The thresholds are a home server's; move them.
Before loading a changed file, check it, and run the tests in
[`alerts.test.yml`](alerts.test.yml) that say what each rule does with figures shaped as the
server writes them:

```sh
docker run --rm -v "$PWD/docs/monitoring:/rules:ro" -w /rules --entrypoint promtool \
  prom/prometheus:v3.5.0 check rules alerts.yml
docker run --rm -v "$PWD/docs/monitoring:/rules:ro" -w /rules --entrypoint promtool \
  prom/prometheus:v3.5.0 test rules alerts.test.yml
```

`ArrMetadataBackupStale` reads `ams_backup_age_seconds`, which is offered while the schedule is on
and once an archive exists; to be told also of a schedule that never made one, add
`or absent(ams_backup_age_seconds)` to it — and drop that half where the schedule is off on
purpose. The webhook's `backup.failed` says why a run failed, as it happens.

## Traces

A trace follows one request: from the door — `GET /v1/tvdb/shows/{language}/{id}`, as Sonarr
asked — to each call it made to a provider, `GET tvdb`, `GET tmdb`, with how long each took and
how it ended. Off until the environment names a collector, in OpenTelemetry's own variables;
then the server sends its spans over OTLP, HTTP and protobuf, to Tempo, Jaeger, Grafana Alloy or
an OpenTelemetry collector:

```yaml
services:
  metadata:
    environment:
      OTEL_EXPORTER_OTLP_ENDPOINT: http://tempo:4318
      # One trace in ten, and a proxy's choice for a request it traced.
      OTEL_TRACES_SAMPLER: parentbased_traceidratio
      OTEL_TRACES_SAMPLER_ARG: "0.1"
```

The rest are OpenTelemetry's too: `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` for a whole address,
`OTEL_EXPORTER_OTLP_HEADERS` for a token, `OTEL_SERVICE_NAME` (`arr-metadata-server` unless set),
`OTEL_RESOURCE_ATTRIBUTES`, the batch's `OTEL_BSP_*`. See [Configuration](../configuration.md#traces).

- **A trace is named by the request's id.** Each request is answered with `x-request-id`, which
  is sixteen random bytes — the shape of a trace id — and a trace the server starts takes it as
  its own: the id a user reads off an error finds its trace in Tempo as it is.
- **A proxy's trace is joined.** A request from a proxy in `AMS_TRUSTED_PROXIES` that carries a
  W3C `traceparent` — Traefik, Caddy and nginx with OpenTelemetry send one — is a span of that
  trace, and keeps its decision to be kept or not; with no `x-request-id` of the proxy's own, the
  request's id is the trace's. Anybody else's `traceparent` is dropped at the door.
- **The trace goes on to the providers.** Each call made to one carries the trace in its own
  `traceparent`, so a service that traces too — another instance of this server, a provider
  behind a proxy that traces — continues it.
- **Only spans leave.** Each with the attributes the server gives it — method, route, status,
  the surface, the provider, the address with its keys masked — and nothing of the lines written
  meanwhile: those stay in the log, which names the trace on each. A call made by a background
  task, with no request to belong to, is no trace of its own.

The traces are separate from `AMS_LOG`: `AMS_LOG=warn` quietens the log and leaves the traces
whole.

### From a line of the log to its trace

With `AMS_LOG_FORMAT=json` every line written while a request was served starts with its
`trace_id`. In Grafana, give the Loki data source a derived field that reads it and links to
Tempo:

- **Name** `trace_id`, **Regex** `"trace_id":"(\w+)"`, **Query** `${__value.raw}`, **Internal
  link** to the Tempo data source.

And on the Tempo data source, *Trace to logs* with the Loki data source and the query
`{container="metadata"} |= "${__trace.traceId}"` leads back from a span to its lines.

## The log

Every line the server writes goes to standard output, as text or, with `AMS_LOG_FORMAT=json`,
as one JSON object per line. `AMS_LOG` decides what is written: `info` by default, `debug` for
more, `warn` for less — the lines a request's handler writes name the request's ids whatever the
level.

The administration's **Server log** keeps the last two thousand of the same lines in memory, to
read without `docker logs`: by level, by words — a trace id finds every line of one request —
followed as they come, and downloaded as text or as JSON lines. Every text in them is masked as
the log masks a secret, whatever wrote it: keys and tokens in an address, passwords, the values
of the secret settings. Among several instances each keeps its own, and the page says whose it
is. `GET /api/v1/admin/logs` answers the same, for an administrator.

## Error reports

Each error the server writes — a request that failed, a panic — can be sent to a Sentry or a
GlitchTip as it happens. Off until a DSN is set under **Settings › Error reports**, and taken at
once, on every instance, when it is: no restart. The DSN is a secret, written and never shown
again; `sentry.environment` says what the reports are filed under and `sentry.sampleRate` what
share of them is sent (see [Settings](../settings.md#error-reports)).

What is sent is the error line and nothing around it: its message and fields, the module that
wrote it, the request it was written in, its method and its address masked, and the
`trace_id` and `request_id` that find it in the log and the traces. No breadcrumb of the lines
before it, no address of the caller, no request body; every text masked before it leaves.
