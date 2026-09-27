#!/usr/bin/env bash
# A release server to measure, on 18479, over a copy of a catalogue: nothing
# it does touches the database it was given. Public browsing on, Sonarr's
# surface open to this host, refresh off so the numbers are the requests'
# alone. Stop it with Ctrl-C; the copy is removed.
#
#   DB=data/e2e.db MEDIA_DIR=data/media ./scripts/load/serve.sh
#   AMS_REDIS_URL=redis://127.0.0.1:6379 ./scripts/load/serve.sh   # with a second level

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="${BINARY:-$ROOT/target/release/arr-metadata-server}"
DB="${DB:-$ROOT/data/e2e.db}"
PORT="${PORT:-18479}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-load.XXXXXX")"

[ -x "$BINARY" ] || { echo "no binary at $BINARY; run cargo build --release first" >&2; exit 1; }
[ -f "$DB" ] || { echo "no catalogue at $DB" >&2; exit 1; }

# A copy, WAL included, so the measured server writes nothing back.
cp "$DB" "$WORK/catalogue.db"
[ -f "$DB-wal" ] && cp "$DB-wal" "$WORK/catalogue.db-wal"
trap 'rm -rf "$WORK"' EXIT

echo "load server: $BINARY on 127.0.0.1:$PORT over a copy of $DB (work: $WORK)"
# Run as a child rather than exec'd, so the trap above removes the copy once
# the server ends — a Ctrl-C reaches it through the shell.
SERVER=
forward() { [ -n "$SERVER" ] && kill "$SERVER" 2>/dev/null; }
trap 'forward' INT TERM
AMS_DATABASE_URL="sqlite://$WORK/catalogue.db?mode=rwc" \
AMS_BIND_ADDRESS="127.0.0.1:$PORT" \
AMS_PUBLIC_URL="http://127.0.0.1:$PORT" \
AMS_PUBLIC_BROWSE=true \
AMS_ALLOWLIST="127.0.0.0/8,::1/128" \
AMS_RATE_LIMIT_PER_MINUTE=0 \
AMS_REFRESH_ENABLED=false \
AMS_ADMIN_USERNAME="${AMS_ADMIN_USERNAME:-load}" \
AMS_ADMIN_PASSWORD="${AMS_ADMIN_PASSWORD:-load-test-password}" \
AMS_MEDIA_STORAGE="${AMS_MEDIA_STORAGE:-${MEDIA_DIR:+filesystem}}" \
AMS_MEDIA_DIR="${MEDIA_DIR:-$ROOT/data/media}" \
AMS_LOG="${AMS_LOG:-warn}" \
  "$BINARY" &
SERVER=$!
wait "$SERVER"
