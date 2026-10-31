#!/usr/bin/env bash
# What the server holds in memory while a library is added to it, measured on
# a whole stack: the image as compose.yaml deploys it (512 MB ceiling,
# read-only root, /tmp on a tmpfs), PostgreSQL, and Sonarr and Radarr adding
# the most voted series and films on TMDB through it, its pictures fetched
# and served as a deployment's are. Every five seconds a probe reads what the
# kernel holds for the server and charges its container, and a scrape what
# the program holds itself; the summary is a markdown table, for
# docs/perf/memory.md.
#
#   ./scripts/load/memory.sh                                  # the published image
#   IMAGE=arr-metadata-server:dev ./scripts/load/memory.sh    # one built here
#   SERIES=100 MOVIES=150 QUIET=240 ./scripts/load/memory.sh
#   MEMORY_ENV=allocator.env ./scripts/load/memory.sh         # variables for the server
#   LIMIT=1G MEDIA=off ./scripts/load/memory.sh
#   KEEP=1 ./scripts/load/memory.sh                           # leave the stack up afterwards
#
# Needs Docker on cgroup v2 (Docker Desktop is), python3, curl, the internet,
# and TMDB's key — TMDB_API_KEY, and TVDB_API_KEY and FANARTTV_API_KEY if you
# have them — in the environment or in .env.local, handed to the server and
# never shown. The images an older server lacks the metrics of measure all
# the same, but for the heap. Everything is removed at the end, unless KEEP=1;
# the measures stay in the work directory it names.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${IMAGE:-ghcr.io/dim145/arr-metadata-server:latest}"
PROJECT=ams-memory
PORT="${PORT:-18480}"
WORK="${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/ams-memory.XXXXXX")}"
mkdir -p "$WORK"

fail() { printf 'FAILED: %s\n' "$1" >&2; exit 1; }
command -v docker >/dev/null || fail "docker is required"
docker info >/dev/null 2>&1 || fail "the Docker daemon does not answer"
command -v python3 >/dev/null || fail "python3 is required"
command -v curl >/dev/null || fail "curl is required"

if [ -z "${TMDB_API_KEY:-}" ] && [ -f "$ROOT/.env.local" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$ROOT/.env.local"
  set +a
fi
[ -n "${TMDB_API_KEY:-}" ] || fail "TMDB_API_KEY is required (in the environment or in .env.local)"
export AMS_TMDB_API_KEY="$TMDB_API_KEY" AMS_TVDB_API_KEY="${TVDB_API_KEY:-}" AMS_FANART_API_KEY="${FANARTTV_API_KEY:-}"
AMS_ADMIN_PASSWORD="$(openssl rand -hex 12)"
export AMS_ADMIN_PASSWORD AMS_IMAGE="$IMAGE" AMS_MEMORY_PORT="$PORT" AMS_MEMORY_LIMIT="${LIMIT:-512M}"
export AMS_MEDIA_STORAGE="${MEDIA:-filesystem}"
if [ -n "${MEMORY_ENV:-}" ]; then
  AMS_MEMORY_ENV="$(cd "$(dirname "$MEMORY_ENV")" && pwd)/$(basename "$MEMORY_ENV")"
  export AMS_MEMORY_ENV
fi
export AMS="http://127.0.0.1:${PORT}" SONARR="http://127.0.0.1:${SONARR_PORT:-18990}" RADARR="http://127.0.0.1:${RADARR_PORT:-17880}"

compose() { docker compose -p "$PROJECT" -f "$ROOT/compose.memory.yaml" "$@"; }

SCRAPER=
cleanup() {
  code=$?
  trap - EXIT
  [ -n "$SCRAPER" ] && kill "$SCRAPER" 2>/dev/null
  docker rm -f ams-memory-probe >/dev/null 2>&1 || true
  docker logs ams-memory-metadata > "$WORK/server.log" 2>&1 || true
  if [ "${KEEP:-0}" = 1 ]; then
    echo "kept: the stack (docker ps --filter name=ams-memory), and $WORK"
  else
    compose down -v --remove-orphans >/dev/null 2>&1 || true
    echo "the measures are in $WORK"
  fi
  exit "$code"
}
trap cleanup EXIT

echo "memory: $IMAGE, ceiling $AMS_MEMORY_LIMIT, media $AMS_MEDIA_STORAGE${MEMORY_ENV:+, with $MEMORY_ENV}"
compose down -v --remove-orphans >/dev/null 2>&1 || true
compose up -d >"$WORK/up.log" 2>&1 || { tail -20 "$WORK/up.log" >&2; fail "docker compose up failed"; }

# The probe: a busybox in the server's PID namespace, seeing the host's
# cgroups, writing a line every INTERVAL seconds into the work directory.
case "$(docker info --format '{{.CgroupDriver}}')" in
  systemd) cgroup() { echo "/sys/fs/cgroup/system.slice/docker-$(docker inspect -f '{{.Id}}' "$1").scope"; } ;;
  *) cgroup() { echo "/sys/fs/cgroup/docker/$(docker inspect -f '{{.Id}}' "$1")"; } ;;
esac
cat > "$WORK/probe.sh" <<'PROBE'
CG="$1"; PG="$2"; I="$3"
while :; do
  s=$(awk '/^(VmRSS|VmHWM|RssAnon|RssFile|Threads):/{sub(":", "", $1); printf "%s=%s ", $1, $2}' /proc/1/status)
  h=$(awk '/^AnonHugePages:/{printf "AnonHugePages=%s ", $2}' /proc/1/smaps_rollup 2>/dev/null)
  m=$(awk '/^(anon|active_file|inactive_file|shmem) /{printf "%s=%s ", $1, $2}' "$CG/memory.stat")
  e=$(awk '/^(max|oom_kill) /{printf "ev_%s=%s ", $1, $2}' "$CG/memory.events")
  u=$(awk '{printf "cpu_ticks=%d", $14 + $15}' /proc/1/stat)
  g=$(awk '/^anon /{printf "pg_anon=%s ", $2}' "$PG/memory.stat" 2>/dev/null)
  g="$g$(awk '/^usage_usec /{printf "pg_cpu_usec=%s", $2}' "$PG/cpu.stat" 2>/dev/null)"
  echo "t=$(date +%s) $s $h current=$(cat "$CG/memory.current") $m $e $g $u"
  sleep "$I"
done >> /work/probe.log
PROBE
docker run -d --name ams-memory-probe --pid container:ams-memory-metadata --cgroupns host --cap-add SYS_PTRACE \
  -v "$WORK:/work" busybox:1.37 sh /work/probe.sh "$(cgroup ams-memory-metadata)" "$(cgroup ams-memory-postgres)" "${INTERVAL:-5}" >/dev/null
python3 -I "$ROOT/scripts/load/memory.py" scrape "$WORK/scrape.log" "${INTERVAL:-5}" &
SCRAPER=$!
# Killed at the end without the shell saying so.
disown "$SCRAPER"

for c in sonarr radarr; do
  port="$([ "$c" = sonarr ] && echo "${SONARR_PORT:-18990}" || echo "${RADARR_PORT:-17880}")"
  for _ in $(seq 1 120); do curl -sf -o /dev/null "http://127.0.0.1:$port/ping" && break; sleep 2; done
  curl -sf -o /dev/null "http://127.0.0.1:$port/ping" || fail "ams-memory-$c did not come up"
  key="$(docker exec "ams-memory-$c" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml)"
  folder="$([ "$c" = sonarr ] && echo /config/tv || echo /config/movies)"
  docker exec "ams-memory-$c" sh -c "mkdir -p $folder && chown abc:abc $folder"
  if [ "$c" = sonarr ]; then export SONARR_KEY="$key"; else export RADARR_KEY="$key"; fi
done

started=$(date +%s)
python3 -I "$ROOT/scripts/load/memory.py" add | tee "$WORK/add.log"
echo "  waiting for the clients' commands, then ${QUIET:-240} s of quiet"
timeout "${WAIT:-900}" python3 -I "$ROOT/scripts/load/memory.py" wait || true
sleep "${QUIET:-240}"
echo "  $(( $(date +%s) - started )) s; the server: $(docker inspect -f 'oom-killed={{.State.OOMKilled}}, {{.State.Status}}' ams-memory-metadata)"
# How far the pictures got meanwhile: a faster preparation stores more of
# them in the same time.
echo "  pictures: $(docker exec ams-memory-postgres psql -U ams -d ams -At -F= -c \
  "SELECT status, count(*) FROM media_asset GROUP BY status ORDER BY status" | tr '\n' ' ')"
echo
python3 -I "$ROOT/scripts/load/memory.py" summary "$WORK/probe.log" "$WORK/scrape.log" | tee "$WORK/summary.md"
