# Watching the server

The server counts what it does and offers it at `/api/v1/admin/metrics`, in
Prometheus's text format: requests by surface and status class with their
latency, calls made to each provider and how they ended, what every cache
answered from each tier, the cache server's memory, evictions and round
trip, the catalogue's size, the jobs' runs. The endpoint is administrative
— it says which surfaces are open and how the caches are doing — so the
scrape carries an administrator's key.

1. Make a key under **Keys & network** for the scraper — with the `admin`
   scope, since the endpoint is administrative — and put it in a file
   Prometheus can read, mode 600.
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
