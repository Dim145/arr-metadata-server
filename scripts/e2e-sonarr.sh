#!/usr/bin/env bash
# A Sonarr of its own, reaching this server the way a real one does: by the
# name it has compiled in, in TLS, trusting the authority this server made —
# then asked to search, to add a series, and to fetch its episodes and its
# poster from here.
#
# Needs Docker, a built binary, and port 443 free on this host: Sonarr calls
# skyhook.sonarr.tv on 443 and nothing else. On Linux, binding 443 as a user
# takes `sudo setcap cap_net_bind_service=+ep target/debug/arr-metadata-server`
# (macOS lets any user bind it).
#
#   cargo build && ./scripts/e2e-sonarr.sh
#   KEEP=1 ./scripts/e2e-sonarr.sh    # leave the container and the work directory behind
#
# Everything it makes is its own: a catalogue in a temporary directory, an
# authority of its own, a Sonarr container named ams-e2e-sonarr. Nothing
# reaches the internet: the upstreams point back at this server.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="${BINARY:-$ROOT/target/debug/arr-metadata-server}"
SONARR_IMAGE="${SONARR_IMAGE:-lscr.io/linuxserver/sonarr:latest}"
WEB_PORT="${WEB_PORT:-18479}"
CLIENTS_BIND="${CLIENTS_BIND:-[::]:443}"
SONARR_PORT="${SONARR_PORT:-18989}"
NAME="${NAME:-ams-e2e-sonarr}"
MADE_WORK=0
if [ -z "${WORK:-}" ]; then
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-e2e-sonarr.XXXXXX")"
  MADE_WORK=1
fi
PASSWORD="e2e-sonarr-password"
BASE="http://127.0.0.1:${WEB_PORT}"
SONARR="http://127.0.0.1:${SONARR_PORT}"
COOKIES="$WORK/cookies"
TVDB=997701

[ -x "$BINARY" ] || { echo "no binary at $BINARY; run cargo build first" >&2; exit 1; }
command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
mkdir -p "$WORK/tls" "$WORK/sonarr" "$WORK/media"

SERVER=
cleanup() {
  code=$?
  if [ -n "$SERVER" ]; then kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true; fi
  if [ "${KEEP:-0}" = 1 ] || [ "$code" -ne 0 ]; then
    echo "kept: container $NAME (docker logs $NAME), work directory $WORK (server.log)"
  else
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    # Only what this run made: a directory somebody chose is theirs to keep.
    [ "$MADE_WORK" = 1 ] && rm -rf "$WORK"
  fi
}
trap cleanup EXIT

step() { printf '  %-80s' "$1"; }
ok() { printf 'ok\n'; }
fail() { printf 'FAILED: %s\n' "$1" >&2; exit 1; }
py() { python3 -c "import sys, json; $1"; }

echo "e2e sonarr: server on ${WEB_PORT} (web) and ${CLIENTS_BIND} (clients), Sonarr on ${SONARR_PORT}"
echo "  work: $WORK"

# ── 1. The server, with its two doors ──────────────────────────────────────
AMS_DATABASE_URL="sqlite://${WORK}/catalogue.db?mode=rwc" \
AMS_BIND_ADDRESS="127.0.0.1:${WEB_PORT}" \
AMS_PUBLIC_URL="http://host.docker.internal:${WEB_PORT}" \
AMS_CLIENTS_BIND="$CLIENTS_BIND" \
AMS_TLS_DIR="$WORK/tls" \
AMS_ALLOWLIST="0.0.0.0/0,::/0" \
AMS_MEDIA_STORAGE=filesystem \
AMS_MEDIA_DIR="$WORK/media" \
AMS_ADMIN_USERNAME=e2e \
AMS_ADMIN_PASSWORD="$PASSWORD" \
AMS_REFRESH_ENABLED=false \
AMS_SKYHOOK_UPSTREAM="$BASE" \
AMS_RADARR_METADATA_UPSTREAM="$BASE" \
AMS_LOG="${AMS_LOG:-info}" \
  "$BINARY" > "$WORK/server.log" 2>&1 &
SERVER=$!

step "the server is up, with its two doors"
for _ in $(seq 1 60); do curl -sf -o /dev/null "$BASE/health" && break; sleep 0.5; done
curl -sf -o /dev/null "$BASE/health" || fail "the server did not come up; see $WORK/server.log"
grep -q "listening for the clients" "$WORK/server.log" || fail "no clients' door; see $WORK/server.log"
[ -s "$WORK/tls/ca.crt" ] || fail "no authority was made in $WORK/tls"
ok

step "the authority is served, and verifies the clients' door"
curl -fsS -o "$WORK/ca.crt" "$BASE/ca.crt"
cmp -s "$WORK/ca.crt" "$WORK/tls/ca.crt" || fail "/ca.crt differs from the authority on disk"
PORT443="${CLIENTS_BIND##*:}"
curl -fsS --cacert "$WORK/ca.crt" --resolve "skyhook.sonarr.tv:${PORT443}:127.0.0.1" \
  -o /dev/null "https://skyhook.sonarr.tv:${PORT443}/health" \
  || fail "the clients' door does not verify against the authority"
ok

# ── 2. A series of this server's own, for Sonarr to find ───────────────────
step "the administrator makes a series with two episodes and a poster"
curl -fsS -c "$COOKIES" -o /dev/null -X POST "$BASE/api/v1/auth/login" \
  -H 'content-type: application/json' \
  -d "{\"username\":\"e2e\",\"password\":\"${PASSWORD}\"}"
ID=$(curl -fsS -b "$COOKIES" -X POST "$BASE/api/v1/items" -H 'content-type: application/json' \
  -d "{\"kind\":\"series\",\"title\":\"Cinematheque E2E\",\"year\":2026,\"overview\":\"A series that exists to be found by a Sonarr.\",\"externalIds\":{\"tvdb\":${TVDB}}}" \
  | py 'print(json.load(sys.stdin)["id"])')
[ -n "$ID" ] || fail "no series was created"
curl -fsS -b "$COOKIES" -o /dev/null -X POST "$BASE/api/v1/items/${ID}/seasons" \
  -H 'content-type: application/json' -d '{"seasonNumber":1,"title":"Season 1"}'
for n in 1 2; do
  curl -fsS -b "$COOKIES" -o /dev/null -X POST "$BASE/api/v1/items/${ID}/episodes" \
    -H 'content-type: application/json' \
    -d "{\"seasonNumber\":1,\"episodeNumber\":${n},\"title\":\"Episode ${n}\",\"airDate\":\"2026-01-0${n}\",\"runtime\":24}"
done
# A 600×900 PNG made here: a poster this server keeps, and Sonarr fetches
# from the web door under AMS_PUBLIC_URL.
python3 - "$WORK/poster.png" <<'PY'
import struct, sys, zlib
def chunk(kind, data):
    body = kind + data
    return struct.pack('>I', len(data)) + body + struct.pack('>I', zlib.crc32(body) & 0xffffffff)
w, h = 600, 900
row = b'\x00' + bytes([180, 40, 40]) * w
png = (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
       + chunk(b'IDAT', zlib.compress(row * h)) + chunk(b'IEND', b''))
open(sys.argv[1], 'wb').write(png)
PY
POSTER=$(curl -fsS -b "$COOKIES" -F "file=@${WORK}/poster.png;type=image/png" -F coverType=poster \
  "$BASE/api/v1/items/${ID}/media" | py 'print(json.load(sys.stdin)["url"])')
case "$POSTER" in /media/*) ;; *) fail "the poster was not kept here: $POSTER" ;; esac
ok

step "Sonarr's surface hands out the series, its episodes and the poster's public address"
SHOW=$(curl -fsS --cacert "$WORK/ca.crt" --resolve "skyhook.sonarr.tv:${PORT443}:127.0.0.1" \
  "https://skyhook.sonarr.tv:${PORT443}/v1/tvdb/shows/en/${TVDB}/")
printf '%s' "$SHOW" | py "d = json.load(sys.stdin); assert d['title'] == 'Cinematheque E2E', d.get('title'); assert len(d['episodes']) == 2, len(d['episodes']); assert any(i['url'].startswith('http://host.docker.internal:${WEB_PORT}/media/') for i in d.get('images', [])), [i['url'] for i in d.get('images', [])]"
ok

# ── 3. Sonarr, trusting the authority, pointed at this server by name ──────
served() {
  curl -fsS -b "$COOKIES" "$BASE/api/v1/admin/access" \
    | py "print(next(a['served'] for a in json.load(sys.stdin)['apis'] if a['api'] == 'sonarr'))"
}
BEFORE=$(served)

step "Sonarr starts, and installs the authority"
docker rm -f "$NAME" >/dev/null 2>&1 || true
chmod 777 "$WORK/sonarr"
docker run -d --name "$NAME" \
  -e PUID=1000 -e PGID=1000 -e TZ=UTC \
  -p "127.0.0.1:${SONARR_PORT}:8989" \
  -v "$WORK/sonarr:/config" \
  -v "$WORK/ca.crt:/usr/local/share/ca-certificates/arr-metadata.crt:ro" \
  -v "$ROOT/docker/trust-ca.sh:/custom-cont-init.d/10-trust-arr-metadata-ca:ro" \
  --add-host "skyhook.sonarr.tv:host-gateway" \
  --add-host "host.docker.internal:host-gateway" \
  "$SONARR_IMAGE" >/dev/null
for _ in $(seq 1 240); do curl -sf -o /dev/null "$SONARR/ping" && break; sleep 1; done
curl -sf -o /dev/null "$SONARR/ping" || fail "Sonarr did not come up; docker logs $NAME"
# grep reads everything: a -q that stops early would fail the pipeline under pipefail.
docker logs "$NAME" 2>&1 | grep "authority installed" >/dev/null || fail "the authority was not installed; docker logs $NAME"
APIKEY=$(docker exec "$NAME" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml)
[ -n "$APIKEY" ] || fail "no API key in Sonarr's config.xml"
ok
# Sonarr restarts itself once or twice after its first start; a call that
# finds nobody listening is tried again, an answer of any kind is kept.
sonarr() {
  for _ in $(seq 1 30); do
    if curl -sS -H "X-Api-Key: ${APIKEY}" "$@"; then return 0; fi
    sleep 2
  done
  return 1
}

step "Sonarr's search reaches this server by the name it has compiled in"
LOOKUP=
for _ in $(seq 1 30); do
  LOOKUP=$(sonarr "$SONARR/api/v3/series/lookup?term=Cinematheque%20E2E" || true)
  printf '%s' "$LOOKUP" | py "sys.exit(0 if any(s.get('tvdbId') == ${TVDB} for s in json.load(sys.stdin)) else 1)" 2>/dev/null && break
  sleep 2
done
printf '%s' "$LOOKUP" | py "assert any(s.get('tvdbId') == ${TVDB} for s in json.load(sys.stdin)), 'the series was not found: ' + sys.stdin.read()[:300]" \
  || fail "Sonarr could not look the series up; docker logs $NAME"
ok

step "the series is added; its episodes and its poster come from this server"
docker exec "$NAME" sh -c 'mkdir -p /config/tv && chown abc:abc /config/tv'
sonarr -o /dev/null -X POST "$SONARR/api/v3/rootfolder" -H 'content-type: application/json' -d '{"path":"/config/tv"}'
BODY=$(printf '%s' "$LOOKUP" | py "s = [x for x in json.load(sys.stdin) if x.get('tvdbId') == ${TVDB}][0]; s.update(qualityProfileId=1, rootFolderPath='/config/tv', monitored=False, seasonFolder=True, addOptions={'monitor': 'none', 'searchForMissingEpisodes': False}); print(json.dumps(s))")
ADDED=$(sonarr -X POST "$SONARR/api/v3/series" -H 'content-type: application/json' -d "$BODY")
SID=$(printf '%s' "$ADDED" | py 'd = json.load(sys.stdin); print(d["id"] if isinstance(d, dict) and "id" in d else "")')
[ -n "$SID" ] || fail "Sonarr did not add the series: ${ADDED:0:300}"
COUNT=0
for _ in $(seq 1 120); do
  COUNT=$(sonarr "$SONARR/api/v3/episode?seriesId=${SID}" | py 'print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0)
  [ "$COUNT" = "2" ] && break
  sleep 1
done
[ "$COUNT" = "2" ] || fail "expected 2 episodes, Sonarr has ${COUNT}; docker logs $NAME"
sonarr "$SONARR/api/v3/series/${SID}" | py "d = json.load(sys.stdin); assert any('host.docker.internal:${WEB_PORT}/media/' in (i.get('remoteUrl') or '') for i in d.get('images', [])), [i.get('remoteUrl') for i in d.get('images', [])]" \
  || fail "the poster Sonarr holds is not this server's"
code=
for _ in $(seq 1 90); do
  code=$(sonarr -o "$WORK/sonarr-poster.jpg" -w '%{http_code}' "$SONARR/api/v3/mediacover/${SID}/poster.jpg" || true)
  [ "$code" = "200" ] && break
  sleep 1
done
[ "$code" = "200" ] || fail "Sonarr did not fetch the poster (got ${code}); docker logs $NAME"
ok

step "this server counted Sonarr's calls on Sonarr's surface"
# Docker Desktop hands a container's traffic to the host from loopback, so the
# callers list may show one address for curl and Sonarr alike; the surface's
# own counter says what came in since the container started.
AFTER=$(served)
[ "$AFTER" -ge $((BEFORE + 3)) ] || fail "Sonarr's surface answered ${AFTER} calls, ${BEFORE} of them before Sonarr started"
ok

echo "e2e sonarr: all good"
