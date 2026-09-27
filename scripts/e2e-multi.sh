#!/usr/bin/env bash
# Two instances of this server as one: started together against one
# PostgreSQL, one cache server and one bucket, then asked to agree on who
# leads, to pass on what one of them changed — a setting, a network rule, a
# medium kept, a certificate issued — to count together what they answered,
# and to hand the lead over when the leader stops.
#
# Needs Docker (for a PostgreSQL of its own, unless AMS_E2E_PG_URL names one),
# a Valkey or Redis (AMS_REDIS_URL, redis://127.0.0.1:6379 unless set), a
# built binary, and — for the media checks — an S3 bucket in the environment
# (AMS_S3_ENDPOINT, AMS_S3_BUCKET, AMS_S3_ACCESS_KEY, AMS_S3_SECRET_KEY, and
# AMS_S3_REGION / AMS_S3_PATH_STYLE as the bucket needs); without one the
# media are kept nowhere and those checks are skipped.
#
#   cargo build && AMS_S3_ENDPOINT=http://127.0.0.1:3900 … ./scripts/e2e-multi.sh
#   KEEP=1 ./scripts/e2e-multi.sh    # leave the database container and the logs behind
#
# Everything it makes is its own: a database in a container named
# ams-e2e-postgres, keys under the prefix e2e-multi: on the cache server,
# objects under the prefix e2e-multi in the bucket. Nothing reaches the
# internet: no provider key is given, and the upstreams point back at the
# instance itself, which refuses its own calls.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="${BINARY:-$ROOT/target/debug/arr-metadata-server}"
PG_IMAGE="${PG_IMAGE:-postgres:18-alpine}"
PG_NAME="${PG_NAME:-ams-e2e-postgres}"
PG_PORT="${PG_PORT:-15433}"
PORT_A="${PORT_A:-18481}"
PORT_B="${PORT_B:-18482}"
DOOR_A="${DOOR_A:-18443}"
DOOR_B="${DOOR_B:-18444}"
REDIS_URL="${AMS_REDIS_URL:-redis://127.0.0.1:6379}"
PREFIX="e2e-multi:"
PASSWORD="e2e-multi-password"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-e2e-multi.XXXXXX")"
A="http://127.0.0.1:${PORT_A}"
B="http://127.0.0.1:${PORT_B}"

[ -x "$BINARY" ] || { echo "no binary at $BINARY; run cargo build first" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }

MADE_PG=0
PID_A=
PID_B=
cleanup() {
  code=$?
  [ -n "$PID_A" ] && kill "$PID_A" 2>/dev/null || true
  [ -n "$PID_B" ] && kill "$PID_B" 2>/dev/null || true
  wait 2>/dev/null || true
  if [ "${KEEP:-0}" = 1 ] || [ "$code" -ne 0 ]; then
    echo "kept: work directory $WORK (a.log, b.log)$( [ "$MADE_PG" = 1 ] && echo ", container $PG_NAME" )"
  else
    [ "$MADE_PG" = 1 ] && docker rm -f "$PG_NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

step() { printf '  %-78s' "$1"; }
ok() { printf 'ok\n'; }
skip() { printf 'skipped: %s\n' "$1"; }
fail() { printf 'FAILED: %s\n' "$1" >&2; exit 1; }
json() { python3 -c "import sys, json; d = json.load(sys.stdin); print($1)"; }

# ── The database ────────────────────────────────────────────────────────────
if [ -z "${AMS_E2E_PG_URL:-}" ]; then
  command -v docker >/dev/null || { echo "docker is required unless AMS_E2E_PG_URL names a PostgreSQL" >&2; exit 1; }
  step "starting PostgreSQL ($PG_IMAGE) on $PG_PORT"
  docker rm -f "$PG_NAME" >/dev/null 2>&1 || true
  docker run -d --name "$PG_NAME" -e POSTGRES_USER=ams -e POSTGRES_PASSWORD=ams -e POSTGRES_DB=ams \
    -p "127.0.0.1:${PG_PORT}:5432" "$PG_IMAGE" >/dev/null
  MADE_PG=1
  for _ in $(seq 1 60); do
    docker exec "$PG_NAME" pg_isready -U ams -d ams >/dev/null 2>&1 && break
    sleep 1
  done
  docker exec "$PG_NAME" pg_isready -U ams -d ams >/dev/null 2>&1 || fail "PostgreSQL did not come up"
  AMS_E2E_PG_URL="postgres://ams:ams@127.0.0.1:${PG_PORT}/ams"
  ok
fi

# ── The instances ───────────────────────────────────────────────────────────
MEDIA=off
if [ -n "${AMS_S3_ENDPOINT:-}${AMS_S3_BUCKET:-}" ]; then
  MEDIA=s3
fi

# A key inherited from the shell would reach a provider: none is.
unset TMDB_API_KEY TVDB_API_KEY AMS_TVDB_API_KEY FANARTTV_API_KEY AMS_FANART_API_KEY

start() { # name web-port door-port — the server's pid in STARTED, a child of this shell
  AMS_MODE=multi \
  AMS_INSTANCE_NAME="$1" \
  AMS_DATABASE_URL="$AMS_E2E_PG_URL" \
  AMS_REDIS_URL="$REDIS_URL" \
  AMS_REDIS_PREFIX="$PREFIX" \
  AMS_BIND_ADDRESS="127.0.0.1:$2" \
  AMS_PUBLIC_URL="http://127.0.0.1:$2" \
  AMS_CLIENTS_BIND="127.0.0.1:$3" \
  AMS_CLIENTS_NAMES=localhost \
  AMS_TLS_DIR="$WORK/tls" \
  AMS_MEDIA_STORAGE="$MEDIA" \
  AMS_S3_PREFIX="${AMS_S3_PREFIX:-e2e-multi}" \
  AMS_PUBLIC_BROWSE=true \
  AMS_ALLOWLIST="127.0.0.0/8,::1/128" \
  AMS_TRUSTED_PROXIES="127.0.0.0/8" \
  AMS_RATE_LIMIT_PER_MINUTE=6000 \
  AMS_REFRESH_ENABLED=false \
  AMS_SKYHOOK_UPSTREAM="http://127.0.0.1:$2" \
  AMS_RADARR_METADATA_UPSTREAM="http://127.0.0.1:$2" \
  AMS_TMDB_UPSTREAM="http://127.0.0.1:$2" \
  AMS_ADMIN_USERNAME=admin \
  AMS_ADMIN_PASSWORD="$PASSWORD" \
  AMS_LOG="${AMS_LOG:-info}" \
    "$BINARY" >>"$WORK/$1.log" 2>&1 &
  STARTED=$!
}

wait_up() { # url
  for _ in $(seq 1 60); do
    curl -sf "$1/health" >/dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

step "starting two instances together"
start a "$PORT_A" "$DOOR_A"; PID_A=$STARTED
start b "$PORT_B" "$DOOR_B"; PID_B=$STARTED
wait_up "$A" || fail "instance a did not come up (see $WORK/a.log)"
wait_up "$B" || fail "instance b did not come up (see $WORK/b.log)"
ok

step "signing in on each"
sign_in() { # name url
  code="$(curl -s -c "$WORK/$1.jar" -H 'content-type: application/json' \
    -d "{\"username\":\"admin\",\"password\":\"$PASSWORD\"}" "$2/api/v1/auth/login" -o /dev/null -w '%{http_code}')"
  [ "$code" = 200 ] || fail "sign-in on $1 answered $code"
}
sign_in a "$A"
sign_in b "$B"
ok

as_a() { curl -s -b "$WORK/a.jar" "$@"; }
as_b() { curl -s -b "$WORK/b.jar" "$@"; }

# ── Who leads ───────────────────────────────────────────────────────────────
step "both know the mode, both instances, and one leader"
sleep 6
for _ in $(seq 1 10); do
  leader_a="$(as_a "$A/api/v1/admin/cache" | json "d['instances'].get('leader') or ''")"
  count_a="$(as_a "$A/api/v1/admin/cache" | json "len(d['instances']['all'])")"
  [ -n "$leader_a" ] && [ "$count_a" = 2 ] && break
  sleep 2
done
[ "$count_a" = 2 ] || fail "a lists $count_a instances"
[ -n "$leader_a" ] || fail "no leader after twenty seconds"
mode_b="$(as_b "$B/api/v1/admin/cache" | json "d['instances']['mode']")"
leader_b="$(as_b "$B/api/v1/admin/cache" | json "d['instances'].get('leader') or ''")"
[ "$mode_b" = multi ] || fail "b says mode $mode_b"
[ "$leader_a" = "$leader_b" ] || fail "a says $leader_a leads, b says $leader_b"
leads_a="$(as_a "$A/api/v1/admin/cache" | json "d['instances']['leads']")"
leads_b="$(as_b "$B/api/v1/admin/cache" | json "d['instances']['leads']")"
[ "$leads_a" != "$leads_b" ] || fail "both answer leads=$leads_a"
ok
echo "    $leader_a leads"

step "a session opened on one instance is good on the other"
code="$(as_a "$B/api/v1/auth/me" -o /dev/null -w '%{http_code}')"
[ "$code" = 200 ] || fail "b answered $code to a's session"
ok

# ── What one changes, the other sees ────────────────────────────────────────
step "a setting changed on a is read on b"
as_a -X PUT -H 'content-type: application/json' -d '{"key":"tmdb.language","value":"fr-FR"}' \
  "$A/api/v1/settings/server/-" -o /dev/null
sleep 1
lang_b="$(as_b "$B/api/v1/settings/server/-" | json "[s['value'] for s in d if s['key'] == 'tmdb.language'][0]")"
[ "$lang_b" = fr-FR ] || fail "b reads tmdb.language as $lang_b"
ok

step "a network rule added on b lets a peer through on a"
# The address-guarded surface, from an address the rules do not name yet:
# loopback is trusted to say who is calling, so the header stands.
as_peer() { curl -s -H 'X-Forwarded-For: 203.0.113.9' "$A/v1/tvdb/search/en?term=x" -o /dev/null -w '%{http_code}'; }
[ "$(as_peer)" = 403 ] || fail "a let an unknown peer through before the rule"
as_b -H 'content-type: application/json' -d '{"cidr":"203.0.113.0/24","name":"e2e-multi"}' \
  "$B/api/v1/network/rules" -o /dev/null
sleep 1
code="$(as_peer)"
[ "$code" != 403 ] || fail "a still refuses the peer the rule added on b allows"
ok

step "a work created on a is listed on b at once"
ID="$(as_a -H 'content-type: application/json' -d '{"kind":"movie","title":"Multi Test Film","year":2020}' \
  "$A/api/v1/items" | json "d['id']")"
[ -n "$ID" ] || fail "no id"
titles="$(as_b "$B/api/v1/items?term=Multi%20Test" | json "[i['title'] for i in d['items']]")"
case "$titles" in *"Multi Test Film"*) ;; *) fail "b lists $titles" ;; esac
ok

if [ "$MEDIA" = s3 ]; then
  step "a poster uploaded on b is served by a, and forgotten by b when a deletes it"
  python3 - "$WORK/poster.png" <<'EOF'
import struct, sys, zlib
w, h = 600, 900
raw = b''.join(b'\x00' + bytes([180, 40, 30]) * w for _ in range(h))
def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
open(sys.argv[1], 'wb').write(png)
EOF
  up="$(as_b -F "file=@$WORK/poster.png;type=image/png" -F coverType=poster "$B/api/v1/items/$ID/media")"
  url="$(echo "$up" | json "d['url']")"
  asset="$(echo "$up" | json "d['assetId']")"
  sleep 1
  code="$(curl -s -o /dev/null -w '%{http_code}' "$A$url")"
  [ "$code" = 200 ] || fail "a answered $code for the copy b kept"
  as_a -X DELETE "$A/api/v1/items/$ID/media/$asset" -o /dev/null
  sleep 1
  code="$(curl -s -o /dev/null -w '%{http_code}' "$B$url")"
  [ "$code" = 404 ] || fail "b still answers $code after a deleted the copy"
  ok
else
  step "media kept nowhere"
  skip "no AMS_S3_* in the environment"
fi

# ── Counted together ────────────────────────────────────────────────────────
step "calls answered by both are counted together"
before="$(as_a "$A/api/v1/admin/access" | json "[x['served'] for x in d['apis'] if x['api'] == 'sonarr'][0]")"
for _ in 1 2 3; do
  curl -s "$A/v1/tvdb/search/en?term=x" -o /dev/null
  curl -s "$B/v1/tvdb/search/en?term=x" -o /dev/null
done
sleep 7
after="$(as_b "$B/api/v1/admin/access" | json "[x['served'] for x in d['apis'] if x['api'] == 'sonarr'][0]")"
[ "$((after - before))" -ge 6 ] || fail "b counts $((after - before)) of the six calls"
ok

# ── The clients' certificate ────────────────────────────────────────────────
step "a certificate issued on one instance is served by both"
fp_before="$(as_a "$A/api/v1/admin/tls" | json "d['clients']['certificate']['fingerprint']")"
ca_a="$(as_a "$A/api/v1/admin/tls" | json "d['clients']['authority']['fingerprint']")"
ca_b="$(as_b "$B/api/v1/admin/tls" | json "d['clients']['authority']['fingerprint']")"
[ "$ca_a" = "$ca_b" ] || fail "the two instances trust different authorities"
follower="$B"; jar_f="$WORK/b.jar"
if [ "$leads_b" = True ]; then follower="$A"; jar_f="$WORK/a.jar"; fi
code="$(curl -s -b "$jar_f" -X POST "$follower/api/v1/admin/tls/renew" -o /dev/null -w '%{http_code}')"
[ "$code" = 202 ] || fail "the renewal answered $code"
sleep 4
fp_a="$(as_a "$A/api/v1/admin/tls" | json "d['clients']['certificate']['fingerprint']")"
fp_b="$(as_b "$B/api/v1/admin/tls" | json "d['clients']['certificate']['fingerprint']")"
[ "$fp_a" != "$fp_before" ] || fail "the certificate was not renewed"
[ "$fp_a" = "$fp_b" ] || fail "a serves $fp_a, b serves $fp_b"
if command -v openssl >/dev/null; then
  served_a="$(openssl s_client -connect "127.0.0.1:$DOOR_A" -servername skyhook.sonarr.tv </dev/null 2>/dev/null | openssl x509 -noout -fingerprint -sha256 | sed 's/.*=//')"
  served_b="$(openssl s_client -connect "127.0.0.1:$DOOR_B" -servername skyhook.sonarr.tv </dev/null 2>/dev/null | openssl x509 -noout -fingerprint -sha256 | sed 's/.*=//')"
  [ "$served_a" = "$fp_a" ] || fail "a's door shows $served_a"
  [ "$served_b" = "$fp_a" ] || fail "b's door shows $served_b"
fi
ok

# ── The lead handed over ────────────────────────────────────────────────────
step "the lead passes to the other when the leader stops"
if [ "$leads_a" = True ]; then
  kill "$PID_A"; wait "$PID_A" 2>/dev/null || true; PID_A=
  survivor="$B"; jar_s="$WORK/b.jar"
else
  kill "$PID_B"; wait "$PID_B" 2>/dev/null || true; PID_B=
  survivor="$A"; jar_s="$WORK/a.jar"
fi
led=False
for _ in $(seq 1 15); do
  led="$(curl -s -b "$jar_s" "$survivor/api/v1/admin/cache" | json "d['instances']['leads']")"
  [ "$led" = True ] && break
  sleep 2
done
[ "$led" = True ] || fail "the survivor did not take the lead within thirty seconds"
# The stopped instance's announcement outlives it by twenty seconds.
count=2
for _ in $(seq 1 15); do
  count="$(curl -s -b "$jar_s" "$survivor/api/v1/admin/cache" | json "len(d['instances']['all'])")"
  [ "$count" = 1 ] && break
  sleep 2
done
[ "$count" = 1 ] || fail "the survivor still lists $count instances after thirty seconds"
ok

step "the stopped instance comes back as a follower"
if [ -z "$PID_A" ]; then
  start a "$PORT_A" "$DOOR_A"; PID_A=$STARTED; wait_up "$A" || fail "a did not come back"
  back="$A"; jar_r="$WORK/a.jar"
else
  start b "$PORT_B" "$DOOR_B"; PID_B=$STARTED; wait_up "$B" || fail "b did not come back"
  back="$B"; jar_r="$WORK/b.jar"
fi
# Announced once its cache server is attached, and listed at the survivor's
# next round: a dozen seconds at most.
count=1
for _ in $(seq 1 15); do
  count="$(curl -s -b "$jar_s" "$survivor/api/v1/admin/cache" | json "len(d['instances']['all'])")"
  [ "$count" = 2 ] && break
  sleep 2
done
[ "$count" = 2 ] || fail "the survivor lists $count instances after thirty seconds"
leads_back="$(curl -s -b "$jar_r" "$back/api/v1/admin/cache" | json "d['instances']['leads']")"
[ "$leads_back" = False ] || fail "the instance that came back took the lead from the one that had it"
ok

echo
echo "all good: two instances agree on a leader, pass on what changes, count together and hand the lead over"
