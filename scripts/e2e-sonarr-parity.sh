#!/usr/bin/env bash
# Two Sonarrs side by side, given the same real series: one reaching this
# server for skyhook.sonarr.tv and services.sonarr.tv, one reaching the real
# ones. What each holds is then compared — the series, every episode, the
# alternate titles, and which series each takes a set of release names for —
# and every difference is either an addition this server makes or a failure.
#
# Unlike e2e-sonarr.sh this one reaches the internet: the series are real,
# fetched from TMDB and TheTVDB by this server and from Skyhook by the other
# Sonarr. It needs Docker, python3, a built binary, port 443 free on this host,
# and TMDB_API_KEY (TVDB_API_KEY too, for TheTVDB) in the environment.
#
#   cargo build && TMDB_API_KEY=… TVDB_API_KEY=… ./scripts/e2e-sonarr-parity.sh
#   KEEP=1 …    # leave both containers and the work directory behind
#   SERIES="412806 433637" …    # other TVDB ids
#
# The server answers in AMS_TMDB_LANGUAGE (fr-FR here), as a French
# deployment does: its French titles are the ones searched with.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="${BINARY:-$ROOT/target/debug/arr-metadata-server}"
SONARR_IMAGE="${SONARR_IMAGE:-lscr.io/linuxserver/sonarr:latest}"
WEB_PORT="${WEB_PORT:-18480}"
CLIENTS_BIND="${CLIENTS_BIND:-[::]:443}"
OURS_PORT="${OURS_PORT:-18990}"
REAL_PORT="${REAL_PORT:-18991}"
OURS="${OURS:-ams-parity-ours}"
REAL="${REAL:-ams-parity-real}"
LANGUAGE="${LANGUAGE:-fr-FR}"
# An anime with titles in many languages, an upcoming series with episodes
# nobody has named, and a western series with dozens of translations.
SERIES="${SERIES:-412806 433637 81189}"
MADE_WORK=0
if [ -z "${WORK:-}" ]; then
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-parity.XXXXXX")"
  MADE_WORK=1
fi
BASE="http://127.0.0.1:${WEB_PORT}"

[ -x "$BINARY" ] || { echo "no binary at $BINARY; run cargo build first" >&2; exit 1; }
[ -n "${TMDB_API_KEY:-}" ] || { echo "TMDB_API_KEY is required" >&2; exit 1; }
command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
mkdir -p "$WORK/tls" "$WORK/media"

SERVER=
cleanup() {
  code=$?
  if [ -n "$SERVER" ]; then kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true; fi
  if [ "${KEEP:-0}" = 1 ] || [ "$code" -ne 0 ]; then
    echo "kept: containers $OURS and $REAL, work directory $WORK (server.log)"
  else
    docker rm -f -v "$OURS" "$REAL" >/dev/null 2>&1 || true
    [ "$MADE_WORK" = 1 ] && rm -rf "$WORK"
  fi
}
trap cleanup EXIT

echo "e2e sonarr parity: server on ${WEB_PORT} and ${CLIENTS_BIND}, Sonarrs on ${OURS_PORT} (this server) and ${REAL_PORT} (the real services)"
echo "  work: $WORK"

AMS_DATABASE_URL="sqlite://${WORK}/catalogue.db?mode=rwc" \
AMS_BIND_ADDRESS="127.0.0.1:${WEB_PORT}" \
AMS_PUBLIC_URL="http://host.docker.internal:${WEB_PORT}" \
AMS_CLIENTS_BIND="$CLIENTS_BIND" \
AMS_TLS_DIR="$WORK/tls" \
AMS_ALLOWLIST="0.0.0.0/0,::/0" \
AMS_MEDIA_STORAGE=filesystem \
AMS_MEDIA_DIR="$WORK/media" \
AMS_REFRESH_ENABLED=false \
AMS_TMDB_LANGUAGE="$LANGUAGE" \
AMS_TVMAZE_ENABLED=true \
AMS_ANILIST_ENABLED=true \
AMS_MAL_ENABLED=true \
AMS_SONARR_SCENE_MAPPINGS=true \
AMS_LOG="${AMS_LOG:-info}" \
  "$BINARY" > "$WORK/server.log" 2>&1 &
SERVER=$!
for _ in $(seq 1 60); do curl -sf -o /dev/null "$BASE/health" && break; sleep 0.5; done
curl -sf -o /dev/null "$BASE/health" || { echo "the server did not come up; see $WORK/server.log" >&2; exit 1; }
curl -fsS -o "$WORK/ca.crt" "$BASE/ca.crt"

start() {
  local name=$1 port=$2
  shift 2
  docker rm -f -v "$name" >/dev/null 2>&1 || true
  # A volume of its own, not a directory of this host's: SQLite on a Docker
  # Desktop bind mount can come back "database disk image is malformed".
  docker run -d --name "$name" \
    -e PUID=1000 -e PGID=1000 -e TZ=UTC \
    -p "127.0.0.1:${port}:8989" \
    --mount type=volume,dst=/config \
    "$@" \
    "$SONARR_IMAGE" >/dev/null
}
start "$OURS" "$OURS_PORT" \
  -v "$WORK/ca.crt:/usr/local/share/ca-certificates/arr-metadata.crt:ro" \
  -v "$ROOT/docker/trust-ca.sh:/custom-cont-init.d/10-trust-arr-metadata-ca:ro" \
  --add-host "skyhook.sonarr.tv:host-gateway" \
  --add-host "services.sonarr.tv:host-gateway" \
  --add-host "host.docker.internal:host-gateway"
start "$REAL" "$REAL_PORT"

for port in "$OURS_PORT" "$REAL_PORT"; do
  for _ in $(seq 1 240); do curl -sf -o /dev/null "http://127.0.0.1:${port}/ping" && break; sleep 1; done
done
docker logs "$OURS" 2>&1 | grep "authority installed" >/dev/null || { echo "the authority was not installed in $OURS" >&2; exit 1; }
for name in "$OURS" "$REAL"; do
  docker exec "$name" sh -c 'mkdir -p /config/tv && chown abc:abc /config/tv'
done

OURS_KEY=$(docker exec "$OURS" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml)
REAL_KEY=$(docker exec "$REAL" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml)

OURS_URL="http://127.0.0.1:${OURS_PORT}" OURS_KEY="$OURS_KEY" \
REAL_URL="http://127.0.0.1:${REAL_PORT}" REAL_KEY="$REAL_KEY" \
SERIES="$SERIES" REPORT="$WORK/report.json" \
  python3 - <<'PY'
import json, os, sys, time, urllib.parse, urllib.request

SIDES = {
    "ours": (os.environ["OURS_URL"], os.environ["OURS_KEY"]),
    "real": (os.environ["REAL_URL"], os.environ["REAL_KEY"]),
}
SERIES = [int(s) for s in os.environ["SERIES"].split()]
failures, additions = [], []

def call(side, path, body=None, method=None, tries=30):
    base, key = SIDES[side]
    data = json.dumps(body).encode() if body is not None else None
    for attempt in range(tries):
        request = urllib.request.Request(base + path, data=data, method=method or ("POST" if data else "GET"))
        request.add_header("X-Api-Key", key)
        request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                text = response.read().decode()
                return json.loads(text) if text else None
        except urllib.error.HTTPError as e:
            if e.code < 500 and e.code != 404:
                raise RuntimeError(f"{side} {path}: {e.code} {e.read()[:300]!r}")
            if attempt == tries - 1:
                raise
        except (urllib.error.URLError, ConnectionError, TimeoutError):
            if attempt == tries - 1:
                raise
        time.sleep(2)

def command(side, name, **fields):
    started = call(side, "/api/v3/command", {"name": name, **fields})
    for _ in range(180):
        state = call(side, f"/api/v3/command/{started['id']}")
        if state["status"] in ("completed", "failed", "aborted"):
            return state["status"]
        time.sleep(1)
    return "timed out"

def fail(message):
    failures.append(message)
    print(f"    FAILED: {message}")

# Sonarr restarts itself once or twice after its first start, so a write may
# have landed before the answer was lost: each is made only if it is not there.
for side in SIDES:
    if not any(r.get("path") == "/config/tv" for r in call(side, "/api/v3/rootfolder")):
        try:
            call(side, "/api/v3/rootfolder", {"path": "/config/tv"})
        except RuntimeError as e:
            if "already configured" not in str(e):
                raise

held = {}
for tvdb in SERIES:
    for side in SIDES:
        found = [s for s in call(side, f"/api/v3/series/lookup?term=tvdb:{tvdb}") if s.get("tvdbId") == tvdb]
        if not found:
            fail(f"{side} could not look up tvdb:{tvdb}")
            continue
        series = found[0]
        series.update(qualityProfileId=1, rootFolderPath="/config/tv", monitored=False, seasonFolder=True,
                      addOptions={"monitor": "none", "searchForMissingEpisodes": False})
        existing = [s for s in call(side, "/api/v3/series") if s.get("tvdbId") == tvdb]
        if existing:
            held[(side, tvdb)] = existing[0]["id"]
            continue
        try:
            added = call(side, "/api/v3/series", series)
        except RuntimeError as e:
            if "already" not in str(e):
                raise
            added = [s for s in call(side, "/api/v3/series") if s.get("tvdbId") == tvdb][0]
        held[(side, tvdb)] = added["id"]

# Episodes land with the add's refresh: each series is refreshed again, and
# waited for, so both sides are compared once they are done.
def episodes(side, tvdb):
    return call(side, f"/api/v3/episode?seriesId={held[(side, tvdb)]}")
for (side, tvdb), series_id in held.items():
    status = command(side, "RefreshSeries", seriesId=series_id)
    if status != "completed":
        fail(f"{side}: refreshing tvdb:{tvdb} ended {status}")

for side in SIDES:
    status = command(side, "UpdateSceneMapping")
    if status != "completed":
        fail(f"{side}: Update Scene Mapping ended {status}")

def key(e):
    return (e["seasonNumber"], e["episodeNumber"])

for tvdb in SERIES:
    if ("ours", tvdb) not in held or ("real", tvdb) not in held:
        continue
    ours = call("ours", f"/api/v3/series/{held[('ours', tvdb)]}")
    real = call("real", f"/api/v3/series/{held[('real', tvdb)]}")
    print(f"  tvdb:{tvdb} — ours “{ours['title']}”, real “{real['title']}”")
    for field in ("year", "status", "seriesType", "network", "runtime", "certification", "originalLanguage", "tvMazeId", "imdbId"):
        if ours.get(field) != real.get(field):
            print(f"    differs: {field}: ours {ours.get(field)!r}, real {real.get(field)!r}")
    ours_seasons = sorted(s["seasonNumber"] for s in ours["seasons"])
    real_seasons = sorted(s["seasonNumber"] for s in real["seasons"])
    if set(real_seasons) - set(ours_seasons):
        fail(f"tvdb:{tvdb}: seasons missing here: {sorted(set(real_seasons) - set(ours_seasons))}")

    ours_eps = {key(e): e for e in episodes("ours", tvdb)}
    real_eps = {key(e): e for e in episodes("real", tvdb)}
    missing = sorted(set(real_eps) - set(ours_eps))
    extra = sorted(set(ours_eps) - set(real_eps))
    if missing:
        fail(f"tvdb:{tvdb}: {len(missing)} episodes missing here, first {missing[:5]}")
    if extra:
        print(f"    {len(extra)} episodes here the real one lacks, first {extra[:5]}")
    blank = [k for k, e in ours_eps.items() if not (e.get("title") or "").strip()]
    if blank:
        fail(f"tvdb:{tvdb}: {len(blank)} episodes without a title here, first {blank[:5]}")
    for field in ("title", "airDate", "absoluteEpisodeNumber"):
        diffs = [(k, ours_eps[k].get(field), real_eps[k].get(field)) for k in sorted(set(ours_eps) & set(real_eps))
                 if ours_eps[k].get(field) != real_eps[k].get(field)]
        if diffs:
            print(f"    {len(diffs)} episodes differ in {field}, first {diffs[:3]}")
    tba_real = sum(1 for e in real_eps.values() if e.get("title") == "TBA")
    tba_ours = sum(1 for e in ours_eps.values() if e.get("title") == "TBA")
    if tba_real or tba_ours:
        print(f"    TBA: ours {tba_ours}, real {tba_real}")

    ours_alt = {a["title"] for a in ours.get("alternateTitles") or []}
    real_alt = {a["title"] for a in real.get("alternateTitles") or []}
    if real_alt - ours_alt:
        fail(f"tvdb:{tvdb}: alternate titles missing here: {sorted(real_alt - ours_alt)}")
    for title in sorted(ours_alt - real_alt):
        additions.append((tvdb, title))
        print(f"    + alternate title: {title}")

# Release names, and the series each Sonarr takes them for.
NAMES = {
    412806: [
        "[SubsPlease] Fuufu Ijou, Koibito Miman. - 01 (1080p) [ABCDEF12].mkv",
        "Presque Maries Loin d Etre Amoureux S01E01 VOSTFR 1080p WEB x264-GRP",
        "More.than.a.Couple.Less.than.Lovers.S01E02.1080p.WEB.x264-GRP",
    ],
    433637: ["Harry.Potter.S01E01.1080p.WEB.h264-GRP"],
    81189: ["Breaking.Bad.S01E01.1080p.BluRay.x264-GRP", "Breaking Bad S01E02 FRENCH 1080p BluRay x264-GRP"],
}
for tvdb, names in NAMES.items():
    if tvdb not in SERIES:
        continue
    for name in names:
        seen = {}
        for side in SIDES:
            parsed = call(side, "/api/v3/parse?title=" + urllib.parse.quote(name))
            seen[side] = ((parsed or {}).get("series") or {}).get("tvdbId")
        verdict = "same" if seen["ours"] == seen["real"] else ("added here" if seen["ours"] == tvdb else "REGRESSION")
        print(f"  parse {name!r}: ours {seen['ours']}, real {seen['real']} — {verdict}")
        if verdict == "REGRESSION":
            fail(f"parse {name!r}: ours {seen['ours']}, real {seen['real']}")

json.dump({"failures": failures, "additions": additions}, open(os.environ["REPORT"], "w"), indent=1)
print(f"  {len(failures)} failures, {len(additions)} alternate titles added")
sys.exit(1 if failures else 0)
PY

echo "e2e sonarr parity: all good"
