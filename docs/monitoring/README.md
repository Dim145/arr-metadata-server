# Watching the server

The server counts what it does and offers it at `/api/v1/admin/metrics`, in
Prometheus's text format: requests by surface and status class with their
latency, calls made to each provider and how they ended, what every cache
answered from each tier, the cache server's memory, evictions and round
trip, the catalogue's size, the jobs' runs. The endpoint is administrative
— it says which surfaces are open and how the caches are doing — so the
scrape carries a key: a scraper's own, or an administrator's.

1. Make a key under **Keys & network** for the scraper, with the `metrics`
   scope and no other, and put it in a file Prometheus can read, mode 600.
   A key made with `metrics` alone reads the figures and nothing else: not
   the catalogue, not the settings, not the other keys. It sits in a file
   more eyes read than a person's key ever is, and what it opens, should
   it be read, is the figures. (A key with the `admin` scope scrapes too,
   and opens everything; a person's own key cannot carry `metrics`.)
2. Add the job in [`prometheus.yml`](prometheus.yml) to your Prometheus
   configuration; it sends the key as a bearer token, which the server takes
   like `X-Api-Key`.
3. Import [`grafana-dashboard.json`](grafana-dashboard.json) in Grafana
   (*Dashboards → New → Import*) and pick your Prometheus data source. The
   dashboard reads every series the server offers, by instance.

The same figures are on the administration's **Cache** page and its
dashboard, read from the server directly; Prometheus keeps the history.

With several instances (`AMS_MODE=multi`), scrape each one: the caches'
first tier, the request latencies and the uptime are each instance's own,
while the cache server's figures are the same from every instance.
`ams_leader{ams_instance="…"}` is 1 on the one that runs the schedules — the
label is not called `instance`, which Prometheus reserves for the target — and
`ams_instances` says how many were heard of lately — an alert on
`sum(ams_leader) != 1` for more than a minute catches a cluster with no
leader, or two.

`ams_backup_age_seconds` is how long ago the newest archive of the database was made
([Backups](../install.md#backups)), offered while the schedule is on and once an archive
exists. An alert for backups that stopped, at the weekly default:

```yaml
- alert: ArrMetadataBackupStale
  # Two schedules missed, or none ever made while the schedule is on.
  expr: max(ams_backup_age_seconds) > 2 * 7 * 86400 or absent(ams_backup_age_seconds)
  for: 1h
```

Drop the `absent()` half where the schedule is off on purpose; the webhook's `backup.failed`
says why a run failed, as it happens.
