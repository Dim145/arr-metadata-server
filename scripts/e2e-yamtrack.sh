#!/usr/bin/env bash
# A Yamtrack of its own, reaching TheTVDB and AniList through this server the
# way the real one does: by the names it has compiled in, in TLS, trusting
# the authority this server made — then asked, through its own code, to sign
# in to TheTVDB with a key issued here, read a series and an episode this
# catalogue holds locks on, place the series by its TMDB id, and query AniList.
#
# Needs Docker, a built binary, port 443 free on this host (Yamtrack calls
# api4.thetvdb.com and graphql.anilist.co on 443 and nothing else; on Linux a
# user binds it after `sudo setcap cap_net_bind_service=+ep` on the binary),
# and the internet: the services behind the relays are the real ones, so this
# server wants a TheTVDB key and a TMDB key of its own in the environment
# (AMS_TVDB_API_KEY or TVDB_API_KEY, AMS_TMDB_API_KEY or TMDB_API_KEY).
#
#   cargo build && (set -a; source .env.local; set +a; ./scripts/e2e-yamtrack.sh)
#   KEEP=1 ./scripts/e2e-yamtrack.sh    # leave the containers and the work directory behind
#
# Everything it makes is its own: a catalogue in a temporary directory, an
# authority of its own, two containers named ams-e2e-yamtrack and
# ams-e2e-yamtrack-redis on a network of their own.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="${BINARY:-$ROOT/target/debug/arr-metadata-server}"
YAMTRACK_IMAGE="${YAMTRACK_IMAGE:-ghcr.io/fuzzygrim/yamtrack:latest}"
REDIS_IMAGE="${REDIS_IMAGE:-redis:8-alpine}"
WEB_PORT="${WEB_PORT:-18479}"
CLIENTS_BIND="${CLIENTS_BIND:-[::]:443}"
YAMTRACK_PORT="${YAMTRACK_PORT:-18000}"
NAME="${NAME:-ams-e2e-yamtrack}"
REDIS_NAME="${NAME}-redis"
NETWORK="${NAME}-net"
MADE_WORK=0
if [ -z "${WORK:-}" ]; then
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-e2e-yamtrack.XXXXXX")"
  MADE_WORK=1
fi
PASSWORD="e2e-yamtrack-password"
BASE="http://127.0.0.1:${WEB_PORT}"
COOKIES="$WORK/cookies"
# Breaking Bad, as TheTVDB and TMDB number it; Attack on Titan, as AniList does.
SERIES_TVDB=81189
SERIES_TMDB=1396
ANIME_ANILIST=16498
LOCKED_SERIES="Breaking Bad (Cinematheque)"
LOCKED_EPISODE="Pilot (Cinematheque)"
LOCKED_ANIME="Attack on Titan (Cinematheque)"

[ -x "$BINARY" ] || { echo "no binary at $BINARY; run cargo build first" >&2; exit 1; }
command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
[ -n "${AMS_TVDB_API_KEY:-${TVDB_API_KEY:-}}" ] || { echo "a TheTVDB key is wanted: AMS_TVDB_API_KEY or TVDB_API_KEY" >&2; exit 1; }
[ -n "${AMS_TMDB_API_KEY:-${TMDB_API_KEY:-}}" ] || { echo "a TMDB key is wanted: AMS_TMDB_API_KEY or TMDB_API_KEY" >&2; exit 1; }
mkdir -p "$WORK/tls"

SERVER=
cleanup() {
  code=$?
  if [ -n "$SERVER" ]; then kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true; fi
  if [ "${KEEP:-0}" = 1 ] || [ "$code" -ne 0 ]; then
    echo "kept: containers $NAME and $REDIS_NAME (docker logs $NAME), work directory $WORK (server.log)"
  else
    docker rm -f "$NAME" "$REDIS_NAME" >/dev/null 2>&1 || true
    docker network rm "$NETWORK" >/dev/null 2>&1 || true
    [ "$MADE_WORK" = 1 ] && rm -rf "$WORK"
  fi
}
trap cleanup EXIT

step() { printf '  %-104s' "$1"; }
ok() { printf 'ok\n'; }
fail() { printf 'FAILED: %s\n' "$1" >&2; exit 1; }
py() { python3 -c "import sys, json; $1"; }

echo "e2e yamtrack: server on ${WEB_PORT} (web) and ${CLIENTS_BIND} (clients), Yamtrack on ${YAMTRACK_PORT}"
echo "  work: $WORK"

# ── 1. The server, with its two doors, the real services behind it ─────────
AMS_DATABASE_URL="sqlite://${WORK}/catalogue.db?mode=rwc" \
AMS_BIND_ADDRESS="127.0.0.1:${WEB_PORT}" \
AMS_PUBLIC_URL="http://host.docker.internal:${WEB_PORT}" \
AMS_CLIENTS_BIND="$CLIENTS_BIND" \
AMS_TLS_DIR="$WORK/tls" \
AMS_ALLOWLIST="0.0.0.0/0,::/0" \
AMS_ADMIN_USERNAME=e2e \
AMS_ADMIN_PASSWORD="$PASSWORD" \
AMS_REFRESH_ENABLED=false \
AMS_RATE_LIMIT_PER_MINUTE=6000 \
AMS_SKYHOOK_UPSTREAM="$BASE" \
AMS_RADARR_METADATA_UPSTREAM="$BASE" \
AMS_TVDB_ENABLED=true \
AMS_LOG="${AMS_LOG:-info}" \
  "$BINARY" > "$WORK/server.log" 2>&1 &
SERVER=$!

step "the server is up, with its two doors"
for _ in $(seq 1 60); do curl -sf -o /dev/null "$BASE/health" && break; sleep 0.5; done
curl -sf -o /dev/null "$BASE/health" || fail "the server did not come up; see $WORK/server.log (is 443 free?)"
grep -q "listening for the clients" "$WORK/server.log" || fail "no clients' door; see $WORK/server.log"
PORT443="${CLIENTS_BIND##*:}"
for name in api4.thetvdb.com graphql.anilist.co; do
  curl -fsS --cacert "$WORK/tls/ca.crt" --resolve "${name}:${PORT443}:127.0.0.1" -o /dev/null "https://${name}:${PORT443}/health" \
    || fail "the clients' door does not answer ${name}"
done
ok

# ── 2. A series TheTVDB knows, an anime AniList knows, and locks on both ──
step "the administrator signs in, stores Breaking Bad from the providers and locks its title and its pilot's"
curl -fsS -c "$COOKIES" -o /dev/null -X POST "$BASE/api/v1/auth/login" \
  -H 'content-type: application/json' -d "{\"username\":\"e2e\",\"password\":\"${PASSWORD}\"}"
# Asked for the way Sonarr asks, so it is fetched from TMDB and TheTVDB and
# stored with its episodes' TheTVDB ids.
curl -fsS -o /dev/null "$BASE/v1/tvdb/shows/en/${SERIES_TVDB}" || fail "the series could not be fetched from the providers"
ITEM=$(curl -fsS -b "$COOKIES" "$BASE/api/v1/items?term=Breaking%20Bad&kind=series" \
  | py "items = json.load(sys.stdin)['items']; print(next(i['id'] for i in items if i.get('externalIds', {}).get('tvdb') == ${SERIES_TVDB}))")
[ -n "$ITEM" ] || fail "the series is not in the catalogue"
EPISODE_TVDB=$(curl -fsS -b "$COOKIES" "$BASE/api/v1/items/${ITEM}" \
  | py "eps = json.load(sys.stdin)['episodes']; print(next(e['tvdbId'] for e in eps if e['seasonNumber'] == 1 and e['episodeNumber'] == 1))")
[ -n "$EPISODE_TVDB" ] || fail "the pilot carries no TheTVDB id"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/items/${ITEM}/overrides" \
  -H 'content-type: application/json' -d "{\"field\":\"title\",\"value\":\"${LOCKED_SERIES}\"}"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/items/${ITEM}/overrides" \
  -H 'content-type: application/json' -d "{\"scope\":\"episode:1x1\",\"field\":\"title\",\"value\":\"${LOCKED_EPISODE}\"}"
ANIME=$(curl -fsS -b "$COOKIES" -X POST "$BASE/api/v1/items" -H 'content-type: application/json' \
  -d "{\"kind\":\"series\",\"title\":\"Attack on Titan\",\"year\":2013,\"externalIds\":{\"anilist\":[${ANIME_ANILIST}]}}" \
  | py 'print(json.load(sys.stdin)["id"])')
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/items/${ANIME}/overrides" \
  -H 'content-type: application/json' -d "{\"field\":\"title\",\"value\":\"${LOCKED_ANIME}\"}"
KEY=$(curl -fsS -b "$COOKIES" -X POST "$BASE/api/v1/clients" \
  -H 'content-type: application/json' -d '{"name":"yamtrack","scopes":["read"]}' \
  | py 'print(json.load(sys.stdin)["key"])')
[ -n "$KEY" ] || fail "no key was issued"
ok

# ── 3. Yamtrack, trusting the authority, pointed at this server by name ────
served() {
  curl -fsS -b "$COOKIES" "$BASE/api/v1/admin/access" \
    | py "print(next(a['served'] for a in json.load(sys.stdin)['apis'] if a['api'] == '$1'))"
}
TVDB_BEFORE=$(served tvdb)
ANILIST_BEFORE=$(served anilist)

step "Yamtrack starts, with the authority appended to its own trust bundle"
docker rm -f "$NAME" "$REDIS_NAME" >/dev/null 2>&1 || true
docker network rm "$NETWORK" >/dev/null 2>&1 || true
docker network create "$NETWORK" >/dev/null
docker run -d --name "$REDIS_NAME" --network "$NETWORK" "$REDIS_IMAGE" >/dev/null
# Python's requests verifies against certifi's bundle, and REQUESTS_CA_BUNDLE
# replaces it: the bundle mounted is certifi's with the authority appended,
# so MyAnimeList and the rest keep verifying too.
docker run --rm --entrypoint python "$YAMTRACK_IMAGE" \
  -c 'import certifi, sys; sys.stdout.write(open(certifi.where()).read())' > "$WORK/ca-bundle.crt"
[ -s "$WORK/ca-bundle.crt" ] || fail "could not read certifi's bundle from the image"
cat "$WORK/tls/ca.crt" >> "$WORK/ca-bundle.crt"
docker run -d --name "$NAME" --network "$NETWORK" \
  -e TZ=UTC -e SECRET=e2e-yamtrack-secret -e "REDIS_URL=redis://${REDIS_NAME}:6379" \
  -e "TVDB_API=${KEY}" \
  -e REQUESTS_CA_BUNDLE=/etc/ams-ca-bundle.crt \
  -v "$WORK/ca-bundle.crt:/etc/ams-ca-bundle.crt:ro" \
  --add-host "api4.thetvdb.com:host-gateway" \
  --add-host "graphql.anilist.co:host-gateway" \
  -p "127.0.0.1:${YAMTRACK_PORT}:8000" \
  "$YAMTRACK_IMAGE" >/dev/null
# Its sign-in page: the root redirects there, and it is the one page that
# answers before anybody has an account.
for _ in $(seq 1 180); do curl -sf -o /dev/null "http://127.0.0.1:${YAMTRACK_PORT}/accounts/login/" && break; sleep 1; done
curl -sf -o /dev/null "http://127.0.0.1:${YAMTRACK_PORT}/accounts/login/" || fail "Yamtrack did not come up; docker logs $NAME"
MANAGE_DIR=$(docker exec "$NAME" sh -c 'dirname "$(find / -maxdepth 3 -name manage.py 2>/dev/null | head -1)"')
[ -n "$MANAGE_DIR" ] && [ "$MANAGE_DIR" != "." ] || fail "no manage.py in the image"
ok

# Yamtrack's own code, run inside it: its TheTVDB and AniList providers and
# the request helper every provider goes through — the session, its trust
# bundle, its rate limiters.
yamtrack() { docker exec -w "$MANAGE_DIR" "$NAME" python manage.py shell -c "$1" 2>"$WORK/yamtrack-stderr.log"; }

step "Yamtrack signs in to TheTVDB with the key issued here, and is answered that key as its token"
OUT=$(yamtrack "
from app.providers import tvdb
token = tvdb.get_access_token()
print('TOKEN_IS_KEY', token == '${KEY}')
") || fail "Yamtrack's sign-in failed: $(tail -5 "$WORK/yamtrack-stderr.log")"
printf '%s' "$OUT" | grep -q 'TOKEN_IS_KEY True' || fail "the token is not the key: $OUT"
ok

step "Yamtrack reads Breaking Bad and its pilot from TheTVDB through this server, titles locked here written in"
OUT=$(yamtrack "
from app.providers import tvdb, services
token = tvdb.get_access_token()
auth = {'Authorization': 'Bearer ' + token}
series = services.api_request('TVDB', 'GET', tvdb.BASE_URL + '/series/${SERIES_TVDB}/extended', headers=auth)
print('SERIES_NAME', series['data']['name'])
print('SERIES_OVERVIEW_IS_THETVDBS', bool(series['data'].get('overview')) and 'Cinematheque' not in (series['data'].get('overview') or ''))
episode = services.api_request('TVDB', 'GET', tvdb.BASE_URL + '/episodes/${EPISODE_TVDB}', headers=auth)
print('EPISODE_NAME', episode['data']['name'])
print('TMDB_ID', tvdb.series_tmdb_id(${SERIES_TVDB}))
") || fail "Yamtrack's reads failed: $(tail -5 "$WORK/yamtrack-stderr.log")"
printf '%s' "$OUT" | grep -q "SERIES_NAME ${LOCKED_SERIES}" || fail "the series' locked title did not reach Yamtrack: $OUT"
printf '%s' "$OUT" | grep -q "SERIES_OVERVIEW_IS_THETVDBS True" || fail "the overview, not locked, is not TheTVDB's: $OUT"
printf '%s' "$OUT" | grep -q "EPISODE_NAME ${LOCKED_EPISODE}" || fail "the pilot's locked title did not reach Yamtrack: $OUT"
printf '%s' "$OUT" | grep -q "TMDB_ID ${SERIES_TMDB}" || fail "Yamtrack did not place the series by its TMDB id: $OUT"
ok

step "Yamtrack queries AniList through this server, the locked title written into the entry"
OUT=$(yamtrack "
from app.providers import services
answer = services.api_request('ANILIST', 'POST', 'https://graphql.anilist.co',
    params={'query': '{ Media(id: ${ANIME_ANILIST}) { id title { english romaji userPreferred } } }'},
    headers={'Accept': 'application/json'})
title = answer['data']['Media']['title']
print('ANILIST_ENGLISH', title['english'])
print('ANILIST_ROMAJI', title['romaji'])
") || fail "Yamtrack's AniList query failed: $(tail -5 "$WORK/yamtrack-stderr.log")"
printf '%s' "$OUT" | grep -q "ANILIST_ENGLISH ${LOCKED_ANIME}" || fail "the anime's locked title did not reach Yamtrack: $OUT"
printf '%s' "$OUT" | grep -q "ANILIST_ROMAJI Shingeki no Kyojin" || fail "the romaji title, not locked, is not AniList's: $OUT"
ok

step "this server counted Yamtrack's calls on both relays"
TVDB_AFTER=$(served tvdb)
ANILIST_AFTER=$(served anilist)
[ "$TVDB_AFTER" -ge $((TVDB_BEFORE + 4)) ] || fail "the TheTVDB relay answered ${TVDB_AFTER} calls, ${TVDB_BEFORE} before Yamtrack started"
[ "$ANILIST_AFTER" -ge $((ANILIST_BEFORE + 1)) ] || fail "the AniList relay answered ${ANILIST_AFTER} calls, ${ANILIST_BEFORE} before Yamtrack started"
ok

echo "e2e yamtrack: all good"
