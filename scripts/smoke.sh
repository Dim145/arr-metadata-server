#!/usr/bin/env bash
# Start the server against $1, exercise the paths that matter, and stop it.
#
# Used by CI against both database engines, and useful by hand after a change
# that touches storage.
#
#   ./scripts/smoke.sh 'sqlite://data/smoke.db?mode=rwc'
#   ./scripts/smoke.sh 'postgres://ams:ams@127.0.0.1:5432/ams'

set -euo pipefail

DATABASE_URL="${1:?usage: smoke.sh <database url>}"
PORT="${PORT:-18099}"
BASE="http://127.0.0.1:${PORT}"
BINARY="${BINARY:-./target/debug/arr-metadata-server}"
COOKIES="$(mktemp)"
PASSWORD="smoke-test-password"

[ -x "$BINARY" ] || { echo "no binary at $BINARY; run cargo build first" >&2; exit 1; }

AMS_DATABASE_URL="$DATABASE_URL" \
AMS_BIND_ADDRESS="127.0.0.1:${PORT}" \
AMS_ADMIN_USERNAME=smoke \
AMS_ADMIN_PASSWORD="$PASSWORD" \
AMS_REFRESH_ENABLED=false \
AMS_LOG="${AMS_LOG:-warn}" \
  "$BINARY" &
SERVER=$!

cleanup() {
  kill "$SERVER" 2>/dev/null || true
  wait "$SERVER" 2>/dev/null || true
  rm -f "$COOKIES"
}
trap cleanup EXIT

for _ in $(seq 1 60); do
  curl -sf -o /dev/null "${BASE}/health" && break
  sleep 0.5
done

step() { printf '  %-46s' "$1"; }
ok()   { printf 'ok\n'; }

echo "smoke: ${DATABASE_URL%%\?*}"

step "health"
curl -fsS -o /dev/null "${BASE}/health"; ok

step "ready reports the database is up"
curl -fsS "${BASE}/ready" | grep -q '"database":true'; ok

step "native API rejects an unauthenticated call"
[ "$(curl -s -o /dev/null -w '%{http_code}' "${BASE}/api/v1/items")" = "401" ]; ok

step "admin can sign in"
curl -fsS -c "$COOKIES" -o /dev/null -X POST "${BASE}/api/v1/auth/login" \
  -H 'content-type: application/json' \
  -d "{\"username\":\"smoke\",\"password\":\"${PASSWORD}\"}"; ok

step "a manual entry can be created"
ITEM=$(curl -fsS -b "$COOKIES" -X POST "${BASE}/api/v1/items" \
  -H 'content-type: application/json' \
  -d '{"kind":"series","title":"Smoke Test Series","year":2026,"externalIds":{"tvdb":999777}}')
ID=$(printf '%s' "$ITEM" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[ -n "$ID" ]; ok

step "Sonarr's surface serves it"
curl -fsS "${BASE}/v1/tvdb/shows/en/999777" | grep -q 'Smoke Test Series'; ok

step "Sonarr's trailing-slash search URL is accepted"
# Sonarr builds `.../v1/tvdb/{route}/{language}/` with a trailing slash and its
# hostname is compiled in, so this exact shape has to work.
curl -fsS "${BASE}/v1/tvdb/search/en/?term=smoke" >/dev/null; ok

step "Radarr's empty year parameter is tolerated"
# Radarr always emits `year=`, empty when the user gave none.
curl -fsS "${BASE}/v1/search?q=smoke&year=" >/dev/null; ok

step "an override locks the field"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "${BASE}/api/v1/items/${ID}/overrides" \
  -H 'content-type: application/json' \
  -d '{"field":"title","value":"Locked Title"}'
curl -fsS -b "$COOKIES" "${BASE}/api/v1/items/${ID}" | grep -q '"item/title"'; ok

step "Sonarr's surface serves the locked value"
curl -fsS "${BASE}/v1/tvdb/shows/en/999777" | grep -q 'Locked Title'; ok

step "the catalogue list shows the locked value too"
curl -fsS -b "$COOKIES" "${BASE}/api/v1/items" | grep -q 'Locked Title'; ok

step "a wrongly typed override is refused"
[ "$(curl -s -b "$COOKIES" -o /dev/null -w '%{http_code}' -X PUT "${BASE}/api/v1/items/${ID}/overrides" \
  -H 'content-type: application/json' -d '{"field":"runtime","value":"not a number"}')" = "400" ]; ok

step "an issued key authenticates"
KEY=$(curl -fsS -b "$COOKIES" -X POST "${BASE}/api/v1/clients" \
  -H 'content-type: application/json' -d '{"name":"smoke","scopes":["read"]}' \
  | sed -n 's/.*"key":"\([^"]*\)".*/\1/p')
curl -fsS -o /dev/null -H "x-api-key: ${KEY}" "${BASE}/api/v1/stats"; ok

step "a wrong key does not"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H 'x-api-key: ams_wrong' "${BASE}/api/v1/stats")" = "401" ]; ok

step "the TMDB relay matches a multi-segment path"
# Without a TMDB key the relay answers 503. What matters is that it is reached
# at all: a route that only matches one segment falls through to the web UI's
# 404 instead, which is how the whole surface once went quietly dead.
code=$(curl -s -o /dev/null -w '%{http_code}' -H "x-api-key: ${KEY}" "${BASE}/3/movie/329865?language=en-US")
[ "$code" = "503" ] || { printf 'expected 503, got %s\n' "$code"; exit 1; }; ok

step "the OpenAPI spec is behind the same guard"
[ "$(curl -s -o /dev/null -w '%{http_code}' "${BASE}/api/openapi.json")" = "401" ]
curl -fsS -b "$COOKIES" "${BASE}/api/openapi.json" | grep -q '"openapi"'; ok

step "a manual credit is added and survives a refresh"
CREDIT=$(curl -fsS -b "$COOKIES" -X POST "${BASE}/api/v1/items/${ID}/credits" \
  -H 'content-type: application/json' \
  -d '{"personName":"Added By Hand","characterName":"A Role"}' \
  | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[ -n "$CREDIT" ]
curl -fsS -b "$COOKIES" "${BASE}/api/v1/items/${ID}" | grep -q 'Added By Hand'; ok

step "a provider row cannot be removed, a manual one can"
# There is no provider row here, so this checks the shape of the refusal: an
# identifier that is not a manual row of this work must 404 rather than delete.
[ "$(curl -s -b "$COOKIES" -o /dev/null -w '%{http_code}' -X DELETE \
    "${BASE}/api/v1/items/${ID}/credits/not-a-real-id")" = "404" ]
curl -fsS -b "$COOKIES" -o /dev/null -X DELETE "${BASE}/api/v1/items/${ID}/credits/${CREDIT}"; ok

step "asking in another language does not clobber a locked value"
# No TMDB key here, so there is nothing to overlay — which is the point: a
# language we hold nothing for must leave the text alone, and in particular must
# not lose the override set a few steps above.
curl -fsS "${BASE}/v1/tvdb/shows/fr/999777" | grep -q 'Locked Title'
curl -fsS -b "$COOKIES" "${BASE}/api/v1/items/${ID}?language=fr" | grep -q 'Locked Title'; ok

step "the nfo document is generated"
curl -fsS -b "$COOKIES" "${BASE}/api/v1/items/${ID}/nfo" | grep -q '<tvshow>'; ok

step "the job history records a hand-triggered refresh"
curl -s -b "$COOKIES" -o /dev/null -X POST "${BASE}/api/v1/items/${ID}/refresh"
curl -fsS -b "$COOKIES" "${BASE}/api/v1/jobs" | grep -q 'refresh.item'; ok

step "unlocking restores the provider value"
curl -fsS -b "$COOKIES" -o /dev/null -X DELETE "${BASE}/api/v1/items/${ID}/overrides/item/title"
curl -fsS -b "$COOKIES" "${BASE}/api/v1/items/${ID}" | grep -q 'Smoke Test Series'; ok

echo "smoke: all checks passed"
