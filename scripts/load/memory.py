#!/usr/bin/env python3
"""The memory stack's driver: the library added through Sonarr and Radarr,
the server's memory gauges scraped, and what was measured summed up.

    memory.py add                      add SERIES series and MOVIES films, the most voted on TMDB
    memory.py wait                     wait for the clients' commands to settle
    memory.py scrape <file> [secs]     the server's memory gauges, a line a scrape, until killed
    memory.py summary <probe> <scrape> min / mean / max of each figure, in MB

Run by scripts/load/memory.sh, which says where everything is through the
environment: AMS, SONARR, RADARR, SONARR_KEY, RADARR_KEY, AMS_ADMIN_PASSWORD.
Standard library only.
"""

import bisect
import concurrent.futures
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

AMS = os.environ.get("AMS", "http://127.0.0.1:18480")
SONARR = os.environ.get("SONARR", "http://127.0.0.1:18990")
RADARR = os.environ.get("RADARR", "http://127.0.0.1:17880")
# Everything here is on this host: no proxy of the environment's is asked.
DIRECT = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def call(method, url, body=None, headers=None, timeout=600, raw=False):
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(url, data=data, method=method, headers=dict(headers or {}))
    if data is not None:
        request.add_header("Content-Type", "application/json")
    with DIRECT.open(request, timeout=timeout) as response:
        payload = response.read()
    if raw:
        return payload
    return json.loads(payload) if payload else None


class Arr:
    """Sonarr's or Radarr's v3 API, with its key."""

    def __init__(self, base, key):
        self.base, self.key = base, key

    def get(self, path):
        return call("GET", f"{self.base}/api/v3{path}", headers={"X-Api-Key": self.key})

    def post(self, path, body):
        return call("POST", f"{self.base}/api/v3{path}", body, headers={"X-Api-Key": self.key})


def clients():
    return Arr(SONARR, os.environ["SONARR_KEY"]), Arr(RADARR, os.environ["RADARR_KEY"])


def most_voted(kind, count):
    """TMDB's most voted series or films, asked through the server's relay:
    the works a library is likeliest to hold, long runs among them."""
    ids, page = [], 1
    while len(ids) < count:
        found = call("GET", f"{AMS}/3/discover/{kind}?sort_by=vote_count.desc&page={page}&language=en-US")
        ids += [r["id"] for r in found.get("results", []) if r["id"] not in ids]
        page += 1
        if page > found.get("total_pages", 1):
            break
    return ids[:count]


def ensure_root(arr, path):
    if not any(folder["path"] == path for folder in arr.get("/rootfolder")):
        arr.post("/rootfolder", {"path": path})


def add_series(sonarr, tmdb):
    """Looked up by TMDB id — the server resolves it — and added unmonitored,
    with nothing searched for. A problem is said, not raised."""
    try:
        found = sonarr.get(f"/series/lookup?term=tmdb:{tmdb}")
    except urllib.error.HTTPError as e:
        return f"series tmdb:{tmdb}: lookup {e.code}"
    if not found:
        return f"series tmdb:{tmdb}: not found"
    body = found[0]
    if body.get("id"):
        return None
    body.update(
        qualityProfileId=1, rootFolderPath="/config/tv", monitored=False, seasonFolder=True,
        addOptions={"monitor": "none", "searchForMissingEpisodes": False, "searchForCutoffUnmetEpisodes": False},
    )
    try:
        sonarr.post("/series", body)
    except urllib.error.HTTPError as e:
        return f"series tmdb:{tmdb}: add {e.code}"
    return None


def add_movie(radarr, tmdb):
    try:
        found = [m for m in radarr.get(f"/movie/lookup?term=tmdb:{tmdb}") if m.get("tmdbId") == tmdb]
    except urllib.error.HTTPError as e:
        return f"film tmdb:{tmdb}: lookup {e.code}"
    if not found:
        return f"film tmdb:{tmdb}: not found"
    body = found[0]
    if body.get("id"):
        return None
    body.update(
        qualityProfileId=1, rootFolderPath="/config/movies", monitored=False, minimumAvailability="released",
        addOptions={"monitor": "none", "searchForMovie": False},
    )
    try:
        radarr.post("/movie", body)
    except urllib.error.HTTPError as e:
        return f"film tmdb:{tmdb}: add {e.code}"
    return None


def add():
    sonarr, radarr = clients()
    ensure_root(sonarr, "/config/tv")
    ensure_root(radarr, "/config/movies")
    series = most_voted("tv", int(os.environ.get("SERIES", "150")))
    movies = most_voted("movie", int(os.environ.get("MOVIES", "250")))
    workers = int(os.environ.get("WORKERS", "4"))
    print(f"  adding {len(series)} series and {len(movies)} films, {workers} at a time", flush=True)
    began = time.time()
    failures = []
    with concurrent.futures.ThreadPoolExecutor(workers) as pool:
        jobs = [pool.submit(add_series, sonarr, t) for t in series]
        jobs += [pool.submit(add_movie, radarr, t) for t in movies]
        for done, job in enumerate(concurrent.futures.as_completed(jobs), 1):
            if problem := job.result():
                failures.append(problem)
            if done % 50 == 0:
                print(f"  {time.strftime('%H:%M:%S')} {done}/{len(jobs)} ({len(failures)} failed)", flush=True)
    print(f"  added in {time.time() - began:.0f} s; {len(failures)} failed"
          f"{': ' + ', '.join(failures[:10]) if failures else ''}", flush=True)


def wait(limit=3600):
    """Until neither client has a command queued or running."""
    arrs = dict(zip(("sonarr", "radarr"), clients()))
    started = time.time()
    while time.time() - started < limit:
        left = {name: sum(c.get("status") in ("queued", "started") for c in arr.get("/command")) for name, arr in arrs.items()}
        if not any(left.values()):
            return
        time.sleep(15)


# ── the server's gauges ──────────────────────────────────────────────────────

GAUGES = {
    "heap": r"^ams_memory_heap_bytes (\S+)",
    "anon": r'^ams_memory_resident_bytes\{kind="anonymous"\} (\S+)',
    "ws": r"^ams_cgroup_memory_working_set_bytes (\S+)",
}


def scrape(path, interval):
    """The memory gauges, a line a scrape: what the program holds can only be
    read from the server itself."""
    cookie = None
    with open(path, "a") as out:
        while True:
            try:
                if cookie is None:
                    request = urllib.request.Request(
                        f"{AMS}/api/v1/auth/login",
                        data=json.dumps({"username": "memory", "password": os.environ["AMS_ADMIN_PASSWORD"]}).encode(),
                        method="POST",
                        headers={"Content-Type": "application/json"},
                    )
                    with DIRECT.open(request, timeout=30) as response:
                        cookie = response.headers.get("Set-Cookie", "").split(";")[0]
                text = call("GET", f"{AMS}/api/v1/admin/metrics", headers={"Cookie": cookie}, raw=True).decode()
                values = {name: m.group(1) for name, pattern in GAUGES.items() if (m := re.search(pattern, text, re.M))}
                caches = sum(float(x) for x in re.findall(r"^ams_cache_bytes\{[^}]*\} (\S+)", text, re.M))
                out.write(f"t={int(time.time())} " + " ".join(f"{k}={v}" for k, v in values.items()) + f" caches={caches:.0f}\n")
            except Exception as e:  # restarting, a session gone, a gauge an older image lacks
                cookie = None
                out.write(f"t={int(time.time())} error={type(e).__name__}\n")
            out.flush()
            time.sleep(interval)


def lines(path):
    rows = []
    if os.path.exists(path):
        for line in open(path):
            if line.startswith("t="):
                row = {}
                for pair in line.split():
                    key, _, value = pair.partition("=")
                    try:
                        row[key] = float(value)
                    except ValueError:
                        pass
                rows.append(row)
    return rows


def summary(probe_path, scrape_path):
    probe = [row for row in lines(probe_path) if "VmRSS" in row]
    scraped = [row for row in lines(scrape_path) if "heap" in row]
    times = [row["t"] for row in scraped]

    def near(t):
        i = bisect.bisect_left(times, t)
        close = [j for j in (i - 1, i) if 0 <= j < len(times) and abs(times[j] - t) <= 6]
        return scraped[min(close, key=lambda j: abs(times[j] - t))] if close else {}

    nan = float("nan")
    mb = 2**20
    columns = [
        ("docker", "what docker stats shows", lambda p, s: (p["current"] - p["inactive_file"]) / mb),
        ("anon", "the process's own memory", lambda p, s: p["RssAnon"] / 1024),
        ("huge", "of which huge pages", lambda p, s: p.get("AnonHugePages", nan) / 1024),
        ("heap", "what the program holds", lambda p, s: s.get("heap", nan) / mb),
        ("caches", "the caches' bytes", lambda p, s: s.get("caches", nan) / mb),
        ("pgcache", "page cache charged", lambda p, s: (p["active_file"] + p["inactive_file"]) / mb),
        ("peak", "the process's peak", lambda p, s: p["VmHWM"] / 1024),
        ("pg", "PostgreSQL's own memory", lambda p, s: p.get("pg_anon", nan) / mb),
    ]
    rows = [[f(p, near(p["t"])) for _, _, f in columns] for p in probe]
    if not rows:
        print("no samples")
        return
    # The kernel counts a process's time in ticks of a hundredth of a second.
    cpu = (probe[-1].get("cpu_ticks", 0) - probe[0].get("cpu_ticks", 0)) / 100
    pg_cpu = (probe[-1].get("pg_cpu_usec", 0) - probe[0].get("pg_cpu_usec", 0)) / 1e6
    print(f"{len(rows)} samples over {int(probe[-1]['t'] - probe[0]['t'])} s; "
          f"{cpu:.0f} s of the server's CPU and {pg_cpu:.0f} s of PostgreSQL's; "
          f"the ceiling reached {int(probe[-1].get('ev_max', 0))} times, "
          f"{int(probe[-1].get('ev_oom_kill', 0))} killed for memory")
    print()
    print("| MB | " + " | ".join(name for name, _, _ in columns) + " |")
    print("|---|" + "---:|" * len(columns))
    for label, fn in (("min", min), ("mean", lambda xs: sum(xs) / len(xs)), ("max", max)):
        cells = []
        for i in range(len(columns)):
            xs = [row[i] for row in rows if row[i] == row[i]]
            cells.append(f"{fn(xs):.0f}" if xs else "—")
        print(f"| {label} | " + " | ".join(cells) + " |")
    print()
    print(", ".join(f"{name}: {what}" for name, what, _ in columns))


def main():
    what = sys.argv[1] if len(sys.argv) > 1 else ""
    if what == "add":
        add()
    elif what == "wait":
        wait()
    elif what == "scrape":
        scrape(sys.argv[2], float(sys.argv[3]) if len(sys.argv) > 3 else 5)
    elif what == "summary":
        summary(sys.argv[2], sys.argv[3])
    else:
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()
