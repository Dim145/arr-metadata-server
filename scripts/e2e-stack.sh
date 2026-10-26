#!/usr/bin/env bash
# The whole stack, end to end, with the real software: this server, started
# from the tree's own binary on a catalogue of its own, and in Docker Sonarr 4,
# Sonarr 5 (a pre-release), Radarr and — if ffmpeg is there to make it a few
# small videos — a Plex Media Server. Each client reaches the server the way a
# deployment's do: by the names it has compiled in, resolved here, in TLS,
# trusting the authority the server made.
#
#   cargo build && ./scripts/e2e-stack.sh
#   PHASES="sonarr5" ./scripts/e2e-stack.sh      # a part of it
#   KEEP=1 ./scripts/e2e-stack.sh                # leave everything running afterwards
#   PHASES=none ./scripts/e2e-stack.sh           # remove what a KEEP=1 run left
#
# The three names every client resolves to this host, the ones the server
# answers: skyhook.sonarr.tv, services.sonarr.tv and api.radarr.video.
# radarr.servarr.com, Radarr's own services host (its updates), carries no
# metadata and is left alone.
#
# What it checks, each check named when it fails (scripts/e2e-stack.py):
#
#   sonarr4, sonarr5   a search by text and by tvdb:, tmdb: and imdb:; real
#                      series added (an anime, a series still airing, two with
#                      a DVD order); as many seasons and episodes as the server
#                      serves; and on Sonarr 5 a series' metadata language
#                      (French: its title, overview and episodes), the DVD order
#                      chosen at the add, the seasons' titles; a title locked
#                      through the native API appears after a refresh; the
#                      catalogue's scene-mapping titles take a release to its
#                      series; and a series entered by hand, one nothing
#                      elsewhere names — made through the native API, with an
#                      alternative title and a season of two episodes: found by
#                      its title and by tvdb: under a number of the server's
#                      own (from 1 000 000 000, made of its catalogue id), a
#                      number no work was handed finding nothing; added, its
#                      season and episodes as served; a locked title after a
#                      refresh; its alternative title in the scene mapping, a
#                      release named by it taken for it; on Sonarr 5 its
#                      metadataSource, local
#   radarr             a search by text and by tmdb:; two films added (one in a
#                      collection); title, year, runtime, studio and
#                      classification as served; the collection; the
#                      recommendations; a locked title after a refresh; and a
#                      film entered by hand, one with no TMDB id: found by its
#                      title and by tmdb: under such a number; added, as
#                      served; named among the films changed once edited; a
#                      locked title after a refresh
#   push               the server connected to each of them with its API key
#                      (PUT /api/v1/admin/arr), a lock made here sends the
#                      client a RefreshSeries / RefreshMovie, found in its own
#                      history of commands — for the series and the film
#                      entered by hand too, which the client holds under their
#                      numbers
#   seed               the server restarted on a new, empty catalogue, then
#                      Seed from Sonarr and Radarr: what their libraries hold
#                      arrives, and a second run fetches nothing; the works
#                      entered by hand that they hold under their numbers are
#                      reported held under no id to look them up by, none made
#   plex               the provider added to Plex over its own API, an agent
#                      made of it, a library of films, one of series, one of
#                      series in French; the scan; the film, the series, its
#                      season and its episodes as the catalogue has them; and a
#                      series and a film entered by hand, in libraries of their
#                      own on the same agent, matched as show-id-… and
#                      movie-id-…, with the catalogue's title, summary, season
#                      and episodes, and no id elsewhere
#   plexkey            the server restarted with AMS_PLEX_AUTH=apikey: /plex
#                      refused without a key; Plex given /plex/<key> instead,
#                      a lock reaching it there, a key never issued answered
#                      nothing, and the key written nowhere in the log
#
# Needs: Docker (and its compose plugin), python3, curl, a built binary
# (BUILD=1 builds it), the internet (the series are real: TMDB's and TheTVDB's
# keys are TMDB_API_KEY and TVDB_API_KEY, read from the environment or from
# .env.local, and never shown), and port 443 free on this host — Sonarr calls
# skyhook.sonarr.tv on 443 and nothing else. On Linux, a user binds 443 only
# after `sudo setcap cap_net_bind_service=+ep target/debug/arr-metadata-server`
# (macOS lets any user bind it). The server's web door listens on loopback on
# macOS, where Docker Desktop shows a container's calls to the host as from
# loopback, and on every address elsewhere (WEB_BIND).
#
# Everything it makes is its own and is removed at the end — the containers,
# their volumes, the server, its catalogue and its authority — unless KEEP=1.
# After a failure the work directory stays, with the server's log and the
# containers' logs.
#
# ── Plex ──────────────────────────────────────────────────────────────────
# Plex's own API needs no token on a server nobody has claimed, and this
# script uses none, never a Plex account's: the provider, the agent and the
# libraries are made over that API as a user does it in Settings › Metadata
# Agents. To run it against a server that is claimed, which refuses a call
# without a token from a network it does not allow without authorization, two
# ways, neither giving this script a token:
#
#   PLEX_CLAIM=claim-…   a token from https://plex.tv/claim, good for four
#                        minutes, handed to the first start of Plex (a new
#                        volume): the server is then claimed by that account
#                        — and refuses these calls, unless
#   PLEX_ALLOWED_NETWORKS=172.16.0.0/12,192.168.0.0/16
#                        names the networks it allows without authorization
#                        (the preference allowedNetworks, set at the first
#                        start — Docker Desktop shows the host to Plex from
#                        its gateway, the address `docker network inspect`
#                        gives)
#
# or claim it by hand — KEEP=1, open http://localhost:32400/web, sign in —
# and then add the Docker network to Settings › Network › "List of IP
# addresses and networks that are allowed without auth", and run
# PHASES=plex against the stack kept. Without either, the plex checks say
# they were skipped, and why.
#
# Knobs: PHASES, KEEP, BUILD, PULL=1 (pull the images again), SERIES="81189 …"
# (a subset), SONARR4_IMAGE, SONARR5_IMAGE, RADARR_IMAGE, PLEX_IMAGE,
# SONARR4_PORT, SONARR5_PORT, RADARR_PORT, PLEX_PORT, WEB_PORT, WEB_BIND,
# CLIENTS_BIND, BINARY, WORK (a directory of your own, kept).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="${BINARY:-$ROOT/target/debug/arr-metadata-server}"
PROJECT="${PROJECT:-ams-e2e-stack}"
WEB_PORT="${WEB_PORT:-18479}"
CLIENTS_BIND="${CLIENTS_BIND:-[::]:443}"
DOOR_PORT="${CLIENTS_BIND##*:}"
case "$(uname -s)" in
  Darwin) DEFAULT_WEB_BIND="127.0.0.1:${WEB_PORT}" ;;
  # A container reaches the host by the bridge's address on Linux, not by
  # loopback as Docker Desktop does.
  *) DEFAULT_WEB_BIND="0.0.0.0:${WEB_PORT}" ;;
esac
WEB_BIND="${WEB_BIND:-$DEFAULT_WEB_BIND}"
SONARR4_PORT="${SONARR4_PORT:-18989}"
SONARR5_PORT="${SONARR5_PORT:-18990}"
RADARR_PORT="${RADARR_PORT:-17878}"
PLEX_PORT="${PLEX_PORT:-32400}"
DEFAULT_PHASES="sonarr4 sonarr5 radarr push seed"
if command -v ffmpeg >/dev/null 2>&1; then DEFAULT_PHASES="$DEFAULT_PHASES plex plexkey"; fi
PHASES="${PHASES:-$DEFAULT_PHASES}"
BASE="http://127.0.0.1:${WEB_PORT}"
USER_NAME=e2e
MADE_WORK=0
if [ -z "${WORK:-}" ]; then
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-e2e-stack.XXXXXX")"
  MADE_WORK=1
fi
# The administrator of a server that lives for the length of this run — and of
# the next, on a WORK kept: the account is made when the catalogue is.
mkdir -p "$WORK"
if [ -s "$WORK/admin-password" ]; then
  PASSWORD="$(cat "$WORK/admin-password")"
else
  PASSWORD="$(openssl rand -hex 12)"
  (umask 077 && printf '%s' "$PASSWORD" > "$WORK/admin-password")
fi
FAILED=""

compose() {
  AMS_E2E_CA="$WORK/tls/ca.crt" AMS_E2E_MEDIA="$WORK/media" \
  SONARR4_PORT="$SONARR4_PORT" SONARR5_PORT="$SONARR5_PORT" RADARR_PORT="$RADARR_PORT" PLEX_PORT="$PLEX_PORT" \
    docker compose -p "$PROJECT" -f "$ROOT/compose.e2e-stack.yaml" --profile plex "$@"
}

has_phase() { case " $PHASES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

step() { printf '  %-96s' "$1"; }
ok() { printf 'ok\n'; }
fail() { printf 'FAILED: %s\n' "$1" >&2; exit 1; }

[ -x "$BINARY" ] || { [ "${BUILD:-0}" = 1 ] || fail "no binary at $BINARY; run cargo build first, or BUILD=1"; }
command -v docker >/dev/null || fail "docker is required"
docker info >/dev/null 2>&1 || fail "the Docker daemon does not answer"
docker compose version >/dev/null 2>&1 || fail "docker compose is required"
command -v python3 >/dev/null || fail "python3 is required"
command -v curl >/dev/null || fail "curl is required"
command -v openssl >/dev/null || fail "openssl is required (for the administrator's password)"
if has_phase plex && ! command -v ffmpeg >/dev/null 2>&1; then fail "the plex phase makes its videos with ffmpeg, which is not installed: PHASES without plex, or install it"; fi
if has_phase plexkey && ! has_phase plex; then fail "the plexkey phase runs on the libraries the plex phase makes: PHASES=\"plex plexkey\""; fi

# The providers' keys, from the environment or .env.local: handed to the
# server and to nothing else.
if [ -z "${TMDB_API_KEY:-}" ] && [ -f "$ROOT/.env.local" ]; then
  set +x
  set -a
  # shellcheck disable=SC1091
  . "$ROOT/.env.local"
  set +a
fi
[ -n "${TMDB_API_KEY:-}" ] || fail "TMDB_API_KEY is required (in the environment or in .env.local)"
[ -n "${TVDB_API_KEY:-}" ] || echo "  note: no TVDB_API_KEY: the DVD orders and the seasons' titles come from TheTVDB, and these checks will fail without it" >&2

SERVER=
cleanup() {
  code=$?
  trap - EXIT
  if [ -n "$SERVER" ]; then kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true; fi
  mkdir -p "$WORK/logs"
  for c in ams-e2e-sonarr4 ams-e2e-sonarr5 ams-e2e-radarr ams-e2e-plex; do
    docker logs "$c" > "$WORK/logs/$c.log" 2>&1 || rm -f "$WORK/logs/$c.log"
  done
  if [ "${KEEP:-0}" = 1 ]; then
    echo "kept: containers (docker ps --filter name=ams-e2e), their volumes, work directory $WORK"
    echo "      the server is stopped. Again, on the same containers, catalogue and authority:"
    echo "      WORK=$WORK KEEP=1 PHASES=\"…\" $0"
    echo "      and to remove the containers and their volumes: PHASES=none $0 (then rm -rf $WORK)"
  else
    compose down -v --remove-orphans >/dev/null 2>&1 || true
    if [ "$code" -ne 0 ] || [ -n "$FAILED" ]; then
      echo "kept: work directory $WORK (server.log, logs/ of each container); containers and volumes removed"
    elif [ "$MADE_WORK" = 1 ]; then
      rm -rf "$WORK"
    fi
  fi
  exit "$code"
}
trap cleanup EXIT

mkdir -p "$WORK/tls" "$WORK/media-store" "$WORK/logs"

# Nothing to run: only what an earlier run kept is removed, by the trap.
if [ "$PHASES" = none ]; then
  echo "e2e stack: nothing to run; the containers and volumes of a run kept with KEEP=1 are removed"
  exit 0
fi

echo "e2e stack: server on ${WEB_BIND} (web) and ${CLIENTS_BIND} (clients); phases: ${PHASES}"
echo "  work: $WORK"

if [ "${BUILD:-0}" = 1 ]; then
  echo "  building the server (cargo build)…"
  (cd "$ROOT" && cargo build) >"$WORK/build.log" 2>&1 || { tail -20 "$WORK/build.log" >&2; fail "cargo build failed; see $WORK/build.log"; }
fi
echo "  binary: $BINARY ($(date -r "$BINARY" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo '?'); tree at $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')$(git -C "$ROOT" diff --quiet 2>/dev/null || echo ' + uncommitted changes'))"

# ── 1. The server, with its two doors ──────────────────────────────────────
start_server() {
  AMS_DATABASE_URL="sqlite://${WORK}/catalogue.db?mode=rwc" \
  AMS_BIND_ADDRESS="$WEB_BIND" \
  AMS_PUBLIC_URL="http://host.docker.internal:${WEB_PORT}" \
  AMS_CLIENTS_BIND="$CLIENTS_BIND" \
  AMS_TLS_DIR="$WORK/tls" \
  AMS_ALLOWLIST="0.0.0.0/0,::/0" \
  AMS_MEDIA_STORAGE=filesystem \
  AMS_MEDIA_DIR="$WORK/media-store" \
  AMS_ADMIN_USERNAME="$USER_NAME" \
  AMS_ADMIN_PASSWORD="$PASSWORD" \
  AMS_REFRESH_ENABLED=false \
  AMS_RATE_LIMIT_PER_MINUTE="${RATE_LIMIT:-100000}" \
  AMS_TVMAZE_ENABLED=true \
  AMS_ANILIST_ENABLED=true \
  AMS_MAL_ENABLED=true \
  AMS_SONARR_SCENE_MAPPINGS=true \
  AMS_PLEX_AUTH="${PLEX_AUTH:-allowlist}" \
  AMS_LOG="${AMS_LOG:-info}" \
    "$BINARY" >> "$WORK/server.log" 2>&1 &
  SERVER=$!
  for _ in $(seq 1 120); do curl -sf -o /dev/null "$BASE/health" && break; kill -0 "$SERVER" 2>/dev/null || break; sleep 0.5; done
  curl -sf -o /dev/null "$BASE/health" || { tail -15 "$WORK/server.log" >&2; fail "the server did not come up; see $WORK/server.log"; }
}
stop_server() {
  if [ -n "$SERVER" ]; then kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true; SERVER=; fi
}

step "port ${DOOR_PORT} is free for the clients' door"
if (exec 3<>"/dev/tcp/127.0.0.1/${DOOR_PORT}") 2>/dev/null; then fail "something listens on ${DOOR_PORT} already: another e2e script, or a server with AMS_CLIENTS_BIND set"; fi
ok

step "the server is up, with its two doors"
start_server
grep -q "listening for the clients" "$WORK/server.log" || fail "no clients' door; see $WORK/server.log"
[ -s "$WORK/tls/ca.crt" ] || fail "no authority was made in $WORK/tls"
ok

step "the authority is served, and verifies the clients' door under the names the clients call"
curl -fsS -o "$WORK/ca.crt" "$BASE/ca.crt"
cmp -s "$WORK/ca.crt" "$WORK/tls/ca.crt" || fail "/ca.crt differs from the authority on disk"
for name in skyhook.sonarr.tv services.sonarr.tv api.radarr.video; do
  curl -fsS --cacert "$WORK/ca.crt" --resolve "${name}:${DOOR_PORT}:127.0.0.1" -o /dev/null "https://${name}:${DOOR_PORT}/health" \
    || fail "the clients' door does not verify against the authority as ${name}"
done
ok

# ── 2. The clients ─────────────────────────────────────────────────────────
SERVICES=""
has_phase sonarr4 && SERVICES="$SERVICES sonarr4"
has_phase sonarr5 && SERVICES="$SERVICES sonarr5"
has_phase radarr && SERVICES="$SERVICES radarr"
# push and seed read the clients' libraries: every one the stack has.
if has_phase push || has_phase seed; then
  for s in sonarr4 sonarr5 radarr; do case " $SERVICES " in *" $s "*) ;; *) SERVICES="$SERVICES $s" ;; esac; done
fi
if has_phase plex; then
  SERVICES="$SERVICES plex"
  # Small, valid videos, named as Plex wants: a film, and a series of two
  # episodes — a few kilobytes each.
  mkdir -p "$WORK/media/Movies/The Matrix (1999)" "$WORK/media/TV Shows/Breaking Bad (2008)/Season 01"
  make_video() {
    ffmpeg -v error -y -f lavfi -i "testsrc=duration=3:size=320x180:rate=10" -f lavfi -i "sine=frequency=440:duration=3" \
      -c:v libx264 -preset ultrafast -pix_fmt yuv420p -c:a aac -b:a 32k -shortest "$1" || fail "ffmpeg could not make $1"
  }
  make_video "$WORK/media/Movies/The Matrix (1999)/The Matrix (1999).mp4"
  make_video "$WORK/media/TV Shows/Breaking Bad (2008)/Season 01/Breaking Bad (2008) - S01E01 - Pilot.mp4"
  make_video "$WORK/media/TV Shows/Breaking Bad (2008)/Season 01/Breaking Bad (2008) - S01E02 - Cat's in the Bag....mp4"
  # And a series of two episodes and a film entered by hand, under the titles
  # scripts/e2e-stack.py makes them with (HAND_SERIES, HAND_FILM, HAND_EPISODES).
  # Their own folders, for libraries of their own: the checks of the real works
  # count what theirs hold.
  hand_series="$WORK/media/By hand/TV Shows/Quenzabar Tides (2024)/Season 01"
  hand_film="$WORK/media/By hand/Movies/Brindlemoss Without Source (2019)"
  mkdir -p "$hand_series" "$hand_film"
  make_video "$hand_series/Quenzabar Tides (2024) - S01E01 - The Tide Goes Out.mp4"
  make_video "$hand_series/Quenzabar Tides (2024) - S01E02 - The Tide Comes Back.mp4"
  make_video "$hand_film/Brindlemoss Without Source (2019).mp4"
fi

step "the containers start (${SERVICES# })"
# shellcheck disable=SC2086
if [ "${PULL:-0}" = 1 ]; then compose pull $SERVICES >"$WORK/logs/pull.log" 2>&1 || fail "pulling the images failed; see $WORK/logs/pull.log"; fi
# shellcheck disable=SC2086
compose up -d $SERVICES >"$WORK/logs/up.log" 2>&1 || { tail -20 "$WORK/logs/up.log" >&2; fail "docker compose up failed; see $WORK/logs/up.log"; }
ok

port_of() {
  case "$1" in
    sonarr4) echo "$SONARR4_PORT" ;; sonarr5) echo "$SONARR5_PORT" ;; radarr) echo "$RADARR_PORT" ;; plex) echo "$PLEX_PORT" ;;
  esac
}
KEYS_DIR="$WORK/keys"
(umask 077 && mkdir -p "$KEYS_DIR")
for s in $SERVICES; do
  [ "$s" = plex ] && continue
  c="ams-e2e-$s"
  step "$c is up, trusts the authority, and has an API key"
  for _ in $(seq 1 300); do curl -sf -o /dev/null "http://127.0.0.1:$(port_of "$s")/ping" && break; sleep 1; done
  curl -sf -o /dev/null "http://127.0.0.1:$(port_of "$s")/ping" || { docker logs "$c" 2>&1 | tail -15 >&2; fail "$c did not come up; docker logs $c"; }
  # grep reads everything: a -q that stops early would fail the pipeline under pipefail.
  docker logs "$c" 2>&1 | grep "authority installed" >/dev/null || fail "the authority was not installed in $c; docker logs $c"
  key="$(docker exec "$c" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml)"
  [ -n "$key" ] || fail "no API key in $c's config.xml"
  (umask 077 && printf '%s' "$key" > "$KEYS_DIR/$s")
  case "$s" in
    radarr) docker exec "$c" sh -c "mkdir -p /config/movies && chown abc:abc /config/movies" ;;
    *) docker exec "$c" sh -c "mkdir -p /config/tv && chown abc:abc /config/tv" ;;
  esac
  ok
done

# What each client resolves, and what it makes of the door under each name.
for s in $SERVICES; do
  [ "$s" = plex ] && continue
  c="ams-e2e-$s"
  step "$c resolves the three names to this host and trusts the server under each"
  want="$(docker exec "$c" getent hosts host.docker.internal | awk '{print $1; exit}')"
  [ -n "$want" ] || fail "$c cannot resolve host.docker.internal"
  for name in skyhook.sonarr.tv services.sonarr.tv api.radarr.video; do
    got="$(docker exec "$c" getent hosts "$name" | awk '{print $1; exit}')"
    [ "$got" = "$want" ] || fail "$c resolves $name to '${got:-nothing}', not $want"
  done
  for name in skyhook.sonarr.tv services.sonarr.tv api.radarr.video; do
    code="$(docker exec "$c" curl -sS -m 20 -o /dev/null -w '%{http_code}' "https://${name}/health" 2>&1 || true)"
    [ "$code" = 200 ] || fail "$c: https://${name}/health answers '$code' from inside the container"
  done
  ok
done

# ── 3. The checks ──────────────────────────────────────────────────────────
run_phase() {
  local phase="$1"
  echo
  local sonarr4_url="" sonarr5_url="" radarr_url="" plex_url=""
  [ -f "$KEYS_DIR/sonarr4" ] && sonarr4_url="http://127.0.0.1:${SONARR4_PORT}"
  [ -f "$KEYS_DIR/sonarr5" ] && sonarr5_url="http://127.0.0.1:${SONARR5_PORT}"
  [ -f "$KEYS_DIR/radarr" ] && radarr_url="http://127.0.0.1:${RADARR_PORT}"
  has_phase plex && plex_url="http://127.0.0.1:${PLEX_PORT}"
  # The providers' keys stay with the server.
  env -u TMDB_API_KEY -u TVDB_API_KEY -u FANARTTV_API_KEY \
    E2E_AMS_URL="$BASE" E2E_AMS_USER="$USER_NAME" E2E_AMS_PASSWORD="$PASSWORD" \
    E2E_DOOR_CA="$WORK/tls/ca.crt" E2E_DOOR_PORT="$DOOR_PORT" \
    E2E_SONARR4_URL="$sonarr4_url" E2E_SONARR4_KEY="$(cat "$KEYS_DIR/sonarr4" 2>/dev/null || true)" E2E_SONARR4_VERSION="4." \
    E2E_SONARR5_URL="$sonarr5_url" E2E_SONARR5_KEY="$(cat "$KEYS_DIR/sonarr5" 2>/dev/null || true)" E2E_SONARR5_VERSION="5." \
    E2E_RADARR_URL="$radarr_url" E2E_RADARR_KEY="$(cat "$KEYS_DIR/radarr" 2>/dev/null || true)" E2E_RADARR_VERSION="6." \
    E2E_PLEX_URL="$plex_url" E2E_PLEX_PROVIDER="http://host.docker.internal:${WEB_PORT}/plex" \
    E2E_SERVER_LOG="$WORK/server.log" \
    E2E_SERIES="${SERIES:-}" \
    python3 -I "$ROOT/scripts/e2e-stack.py" "$phase" || FAILED="$FAILED $phase"
}

for phase in sonarr4 sonarr5 radarr push; do
  if has_phase "$phase"; then run_phase "$phase"; fi
done
if has_phase seed; then
  echo
  step "the server is restarted on a new, empty catalogue (the authority stays: the clients still trust it)"
  stop_server
  rm -f "$WORK"/catalogue.db "$WORK"/catalogue.db-shm "$WORK"/catalogue.db-wal
  start_server
  ok
  run_phase seed
fi
if has_phase plex; then run_phase plex; fi
if has_phase plexkey; then
  echo
  step "the server is restarted asking Plex for a key (AMS_PLEX_AUTH=apikey)"
  stop_server
  PLEX_AUTH=apikey start_server
  ok
  run_phase plexkey
fi

echo
if [ -n "$FAILED" ]; then
  echo "e2e stack: FAILED in:${FAILED}"
  exit 1
fi
echo "e2e stack: all good"
