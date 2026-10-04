#!/usr/bin/env bash
# The TheTVDB and AniList relays, end to end: this server with both its
# doors, a stand-in for both services that records what it is asked, and a
# client reaching the relays the way Yamtrack does — by the services' names,
# in TLS, trusting the authority this server made.
#
#   cargo build && ./scripts/e2e-relays.sh
#   KEEP=1 ./scripts/e2e-relays.sh    # leave the work directory behind
#
# Nothing reaches the internet: both upstreams are the stand-in on this
# host (scripts/e2e-relays-upstream.py). Two runs of the server: one under
# the default policies — TheTVDB by key, AniList by address — and one with
# them swapped.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="${BINARY:-$ROOT/target/debug/arr-metadata-server}"
WEB_PORT="${WEB_PORT:-18479}"
CLIENTS_PORT="${CLIENTS_PORT:-18443}"
FAKE_PORT="${FAKE_PORT:-18555}"
MADE_WORK=0
if [ -z "${WORK:-}" ]; then
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-e2e-relays.XXXXXX")"
  MADE_WORK=1
fi
PASSWORD="e2e-relays-password"
OPERATOR_TVDB_KEY="operator-tvdb-key"
BASE="http://127.0.0.1:${WEB_PORT}"
FAKE="http://127.0.0.1:${FAKE_PORT}"
COOKIES="$WORK/cookies"
TVDB_ID=997701
ANILIST_ID=16498

[ -x "$BINARY" ] || { echo "no binary at $BINARY; run cargo build first" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
mkdir -p "$WORK/tls"

SERVER=
UPSTREAM=
cleanup() {
  code=$?
  if [ -n "$SERVER" ]; then kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true; fi
  if [ -n "$UPSTREAM" ]; then kill "$UPSTREAM" 2>/dev/null || true; wait "$UPSTREAM" 2>/dev/null || true; fi
  if [ "${KEEP:-0}" = 1 ] || [ "$code" -ne 0 ]; then
    echo "kept: work directory $WORK (server.log, server-2.log)"
  else
    [ "$MADE_WORK" = 1 ] && rm -rf "$WORK"
  fi
}
trap cleanup EXIT

step() { printf '  %-98s' "$1"; }
ok() { printf 'ok\n'; }
fail() { printf 'FAILED: %s\n' "$1" >&2; exit 1; }
py() { python3 -c "import sys, json; $1"; }

echo "e2e relays: server on ${WEB_PORT} (web) and ${CLIENTS_PORT} (clients, TLS), stand-in on ${FAKE_PORT}"
echo "  work: $WORK"

# ── The stand-in for TheTVDB and AniList ──────────────────────────────────
python3 "$ROOT/scripts/e2e-relays-upstream.py" "$FAKE_PORT" "$OPERATOR_TVDB_KEY" &
UPSTREAM=$!
for _ in $(seq 1 40); do curl -sf -o /dev/null "$FAKE/_fake/requests" && break; sleep 0.25; done
curl -sf -o /dev/null "$FAKE/_fake/requests" || fail "the stand-in did not come up"

# What the stand-in was asked: how many times a path was, and the last
# request's field for a path.
# A path names itself and what hangs off it — `/v4/series` its episodes and
# translations too; the root, AniList's one path, itself alone.
asked_count() { curl -fsS "$FAKE/_fake/requests" | py "print(sum(1 for r in json.load(sys.stdin) if (r['path'].split('?')[0] == '/' if '$1' == '/' else r['path'].startswith('$1'))))"; }
asked_last() { curl -fsS "$FAKE/_fake/requests" | py "rs = [r for r in json.load(sys.stdin) if (r['path'].split('?')[0] == '/' if '$1' == '/' else r['path'].startswith('$1'))]; print(json.dumps(rs[-1]['$2']) if rs else 'none')"; }
asked_reset() { curl -fsS -o /dev/null -X POST "$FAKE/_fake/reset"; }

# ── The server, started with the policies given ────────────────────────────
start_server() {
  local log="$1"; shift
  env "$@" \
    AMS_DATABASE_URL="sqlite://${WORK}/catalogue.db?mode=rwc" \
    AMS_BIND_ADDRESS="127.0.0.1:${WEB_PORT}" \
    AMS_PUBLIC_URL="$BASE" \
    AMS_CLIENTS_BIND="127.0.0.1:${CLIENTS_PORT}" \
    AMS_TLS_DIR="$WORK/tls" \
    AMS_ALLOWLIST="0.0.0.0/0,::/0" \
    AMS_ADMIN_USERNAME=e2e \
    AMS_ADMIN_PASSWORD="$PASSWORD" \
    AMS_REFRESH_ENABLED=false \
    AMS_RATE_LIMIT_PER_MINUTE=6000 \
    AMS_SKYHOOK_UPSTREAM="$BASE" \
    AMS_RADARR_METADATA_UPSTREAM="$BASE" \
    AMS_TVDB_UPSTREAM="${FAKE}/v4" \
    AMS_TVDB_API_KEY="$OPERATOR_TVDB_KEY" \
    AMS_TVDB_ENABLED=false \
    AMS_ANILIST_UPSTREAM="$FAKE" \
    AMS_ANILIST_ENABLED=false \
    AMS_LOG="${AMS_LOG:-info}" \
    "$BINARY" > "$WORK/$log" 2>&1 &
  SERVER=$!
  for _ in $(seq 1 60); do curl -sf -o /dev/null "$BASE/health" && break; sleep 0.5; done
  curl -sf -o /dev/null "$BASE/health" || fail "the server did not come up; see $WORK/$log"
  grep -q "listening for the clients" "$WORK/$log" || fail "no clients' door; see $WORK/$log"
}
stop_server() {
  kill "$SERVER" 2>/dev/null || true
  wait "$SERVER" 2>/dev/null || true
  SERVER=
}

# A call through the clients' door, by a service's name: what a client with
# the name compiled in does, once the name resolves here and the authority is
# trusted.
tvdb() { curl -sS --cacert "$WORK/tls/ca.crt" --resolve "api4.thetvdb.com:${CLIENTS_PORT}:127.0.0.1" "$@"; }
anilist() { curl -sS --cacert "$WORK/tls/ca.crt" --resolve "graphql.anilist.co:${CLIENTS_PORT}:127.0.0.1" "$@"; }
TVDB_DOOR="https://api4.thetvdb.com:${CLIENTS_PORT}"
ANILIST_DOOR="https://graphql.anilist.co:${CLIENTS_PORT}"
QUERY='{"query":"query { Media(id: 16498) { id title { english userPreferred romaji } description } }"}'

start_server server.log

step "the authority covers TheTVDB's and AniList's names"
tvdb -f -o /dev/null "$TVDB_DOOR/health" || fail "the clients' door does not answer api4.thetvdb.com"
anilist -f -o /dev/null "$ANILIST_DOOR/health" || fail "the clients' door does not answer graphql.anilist.co"
ok

# ── The catalogue: two works with locks, a key, a member ──────────────────
step "the administrator makes a series known to TheTVDB and an anime known to AniList, and locks fields"
curl -fsS -c "$COOKIES" -o /dev/null -X POST "$BASE/api/v1/auth/login" \
  -H 'content-type: application/json' -d "{\"username\":\"e2e\",\"password\":\"${PASSWORD}\"}"
SERIES=$(curl -fsS -b "$COOKIES" -X POST "$BASE/api/v1/items" -H 'content-type: application/json' \
  -d "{\"kind\":\"series\",\"title\":\"Relay Series\",\"year\":2026,\"overview\":\"Held here.\",\"externalIds\":{\"tvdb\":${TVDB_ID}}}" \
  | py 'print(json.load(sys.stdin)["id"])')
curl -fsS -b "$COOKIES" -o /dev/null -X POST "$BASE/api/v1/items/${SERIES}/seasons" \
  -H 'content-type: application/json' -d '{"seasonNumber":1,"title":"Season 1"}'
for n in 1 2; do
  curl -fsS -b "$COOKIES" -o /dev/null -X POST "$BASE/api/v1/items/${SERIES}/episodes" \
    -H 'content-type: application/json' \
    -d "{\"seasonNumber\":1,\"episodeNumber\":${n},\"title\":\"Episode ${n}\",\"airDate\":\"2026-01-0${n}\",\"runtime\":24}"
done
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/items/${SERIES}/overrides" \
  -H 'content-type: application/json' -d '{"field":"title","value":"Locked Series Title"}'
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/items/${SERIES}/overrides" \
  -H 'content-type: application/json' -d '{"scope":"episode:1x2","field":"title","value":"Locked Episode Title"}'
ANIME=$(curl -fsS -b "$COOKIES" -X POST "$BASE/api/v1/items" -H 'content-type: application/json' \
  -d "{\"kind\":\"series\",\"title\":\"Relay Anime\",\"year\":2013,\"externalIds\":{\"anilist\":[${ANILIST_ID}],\"mal\":[${ANILIST_ID}]}}" \
  | py 'print(json.load(sys.stdin)["id"])')
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/items/${ANIME}/overrides" \
  -H 'content-type: application/json' -d '{"field":"title","value":"Locked Anime Title"}'
KEY=$(curl -fsS -b "$COOKIES" -X POST "$BASE/api/v1/clients" \
  -H 'content-type: application/json' -d '{"name":"yamtrack","scopes":["read"]}' \
  | py 'print(json.load(sys.stdin)["key"])')
[ -n "$KEY" ] || fail "no key was issued"
MEMBER_PASSWORD=$(curl -fsS -b "$COOKIES" -X POST "$BASE/api/v1/users" \
  -H 'content-type: application/json' -d '{"username":"e2e_member","role":"member"}' \
  | py 'print(json.load(sys.stdin)["password"])')
curl -fsS -c "$WORK/member-cookies" -o /dev/null -X POST "$BASE/api/v1/auth/login" \
  -H 'content-type: application/json' -d "{\"username\":\"e2e_member\",\"password\":\"${MEMBER_PASSWORD}\"}"
MEMBER_KEY=$(curl -fsS -b "$WORK/member-cookies" -X POST "$BASE/api/v1/account/keys" \
  -H 'content-type: application/json' -d '{"name":"relay"}' | py 'print(json.load(sys.stdin)["secret"])')
[ -n "$MEMBER_KEY" ] || fail "no member key was issued"
ok

# ── TheTVDB, under the default policy: by key ─────────────────────────────
echo "TheTVDB relay, AMS_TVDB_AUTH=apikey (default)"

step "a client signing in with a key issued here is answered that key as its token"
TOKEN=$(tvdb -X POST "$TVDB_DOOR/v4/login" -H 'content-type: application/json' -d "{\"apikey\":\"${KEY}\"}" \
  | py 'd = json.load(sys.stdin); assert d["status"] == "success", d; print(d["data"]["token"])') \
  || fail "no token was answered"
[ "$TOKEN" = "$KEY" ] || fail "the token is not the key: $TOKEN"
[ "$(asked_count /v4/login)" = "0" ] || fail "the sign-in reached TheTVDB"
ok

step "another key is refused under this policy, before TheTVDB is asked"
code=$(tvdb -o /dev/null -w '%{http_code}' -X POST "$TVDB_DOOR/v4/login" -H 'content-type: application/json' -d '{"apikey":"somebody-elses"}')
[ "$code" = "401" ] || fail "expected 401, got $code"
[ "$(asked_count /v4/login)" = "0" ] || fail "the sign-in reached TheTVDB"
ok

step "a sign-in that is not one is a 400"
code=$(tvdb -o /dev/null -w '%{http_code}' -X POST "$TVDB_DOOR/v4/login" -H 'content-type: application/json' -d '{"pin":"1234"}')
[ "$code" = "400" ] || fail "expected 400, got $code"
ok

step "a member's key is kept off the relays until members are let in"
ANSWER=$(tvdb -w '\n%{http_code}' -X POST "$TVDB_DOOR/v4/login" -H 'content-type: application/json' -d "{\"apikey\":\"${MEMBER_KEY}\"}")
[ "${ANSWER##*$'\n'}" = "403" ] || fail "expected 403, got ${ANSWER##*$'\n'}"
printf '%s' "${ANSWER%$'\n'*}" | grep -q relay_not_for_members || fail "not the members' refusal: $ANSWER"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/settings/server/-" \
  -H 'content-type: application/json' -d '{"key":"api.tmdbMembers","value":"true"}'
code=$(tvdb -o /dev/null -w '%{http_code}' -X POST "$TVDB_DOOR/v4/login" -H 'content-type: application/json' -d "{\"apikey\":\"${MEMBER_KEY}\"}")
[ "$code" = "200" ] || fail "expected 200 once members are let in, got $code"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/settings/server/-" \
  -H 'content-type: application/json' -d '{"key":"api.tmdbMembers","value":"false"}'
ok

step "the series is read with the operator's token, and the locked title written into TheTVDB's record"
asked_reset
DOC=$(tvdb -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/series/${TVDB_ID}/extended")
printf '%s' "$DOC" | py 'd = json.load(sys.stdin)["data"]; assert d["name"] == "Locked Series Title", d["name"]; assert d["overview"] == "Upstream overview", d["overview"]; assert d["status"]["name"] == "Continuing"' \
  || fail "the record was not patched as expected: ${DOC:0:300}"
[ "$(asked_last /v4/series authorization)" = "\"Bearer fake-token-for-${OPERATOR_TVDB_KEY}\"" ] || fail "TheTVDB was not asked with the operator's token: $(asked_last /v4/series authorization)"
[ "$(asked_last /v4/series x-api-key)" = "null" ] || fail "a key of this server reached TheTVDB"
[ "$(asked_count /v4/login)" = "1" ] || fail "the operator was signed in $(asked_count /v4/login) times"
ok

step "the same document is served from the cache the second time"
tvdb -o /dev/null -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/series/${TVDB_ID}/extended"
[ "$(asked_count /v4/series/${TVDB_ID}/extended)" = "1" ] || fail "TheTVDB was asked again"
ok

step "the aired order's episodes take the locked episode title; the translation takes the locked name"
tvdb -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/series/${TVDB_ID}/episodes/official/eng" \
  | py 'eps = json.load(sys.stdin)["data"]["episodes"]; assert eps[0]["name"] == "Upstream E1", eps[0]; assert eps[1]["name"] == "Locked Episode Title", eps[1]' \
  || fail "the episodes were not patched as expected"
tvdb -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/series/${TVDB_ID}/translations/fra" \
  | py 'd = json.load(sys.stdin)["data"]; assert d["name"] == "Locked Series Title", d; assert d["language"] == "fra"' \
  || fail "the translation was not patched as expected"
ok

step "a key in the query authenticates too, and is not handed on"
tvdb -o /dev/null -f "$TVDB_DOOR/v4/search?query=relay&apikey=${KEY}" || fail "the search was refused"
[ "$(asked_last /v4/search path)" = "\"/v4/search?query=relay\"" ] || fail "the key travelled: $(asked_last /v4/search path)"
[ "$(asked_last /v4/search authorization)" = "\"Bearer fake-token-for-${OPERATOR_TVDB_KEY}\"" ] || fail "not the operator's token"
ok

step "the operator's own account is refused with a key issued here, however it is spelt"
for path in "/v4/user" "/v4/user/favorites" "/v4/%75ser/favorites"; do
  code=$(tvdb -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR${path}")
  [ "$code" = "403" ] || fail "expected 403 for ${path}, got $code"
done
[ "$(asked_count /v4/user)" = "0" ] || fail "the operator's account was asked for"
ok

step "no credential at all is a 401, and a write is refused"
code=$(tvdb -o /dev/null -w '%{http_code}' "$TVDB_DOOR/v4/series/${TVDB_ID}")
[ "$code" = "401" ] || fail "expected 401, got $code"
code=$(tvdb -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/user/favorites" -d '{"series":1}')
[ "$code" = "403" ] || fail "expected 403 for a write, got $code"
ok

step "a path that climbs is refused; a request that came round a loop is a 508"
code=$(tvdb -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/series/%2e%2e/login")
[ "$code" = "400" ] || fail "expected 400, got $code"
code=$(tvdb -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ${TOKEN}" -H 'x-ams-instance: elsewhere' "$TVDB_DOOR/v4/series/${TVDB_ID}")
[ "$code" = "508" ] || fail "expected 508, got $code"
ok

step "when TheTVDB refuses the operator's token, the relay signs in anew and asks again"
asked_reset
curl -fsS -o /dev/null -X POST "$FAKE/_fake/expire-token"
tvdb -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/series/${TVDB_ID}/translations/eng" \
  | py 'd = json.load(sys.stdin)["data"]; assert d["name"] == "Locked Series Title", d' \
  || fail "the translation was not answered, or not patched"
[ "$(asked_count /v4/login)" = "1" ] || fail "the relay signed in $(asked_count /v4/login) times"
[ "$(asked_count /v4/series/${TVDB_ID}/translations/eng)" = "2" ] || fail "the translation was asked $(asked_count /v4/series/${TVDB_ID}/translations/eng) times"
ok

step "an episode this catalogue holds no TheTVDB id for passes through as TheTVDB said it"
# A series fed by TheTVDB carries its episodes' ids, and its locks are written
# into /v4/episodes/{id}; one made by hand, as this one, has none to match by.
tvdb -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/episodes/$((TVDB_ID * 10 + 2))" \
  | py 'd = json.load(sys.stdin)["data"]; assert d["name"] == "Upstream E2", d' \
  || fail "the episode was not answered as TheTVDB said it"
ok

step "the relay answers on the interface's door too"
curl -fsS -H "x-api-key: ${KEY}" "$BASE/v4/series/${TVDB_ID}" \
  | py 'd = json.load(sys.stdin)["data"]; assert d["name"] == "Locked Series Title", d' \
  || fail "the interface's door did not relay"
ok

step "switched off on the access page, the relay answers 503 and asks nothing"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/settings/server/-" \
  -H 'content-type: application/json' -d '{"key":"api.tvdb","value":"false"}'
ANSWER=$(tvdb -w '\n%{http_code}' -H "Authorization: Bearer ${TOKEN}" "$TVDB_DOOR/v4/series/${TVDB_ID}/translations/eng")
[ "${ANSWER##*$'\n'}" = "503" ] || fail "expected 503, got ${ANSWER##*$'\n'}"
printf '%s' "$ANSWER" | grep -q surface_disabled || fail "not the switched-off refusal"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/settings/server/-" \
  -H 'content-type: application/json' -d '{"key":"api.tvdb","value":"true"}'
ok

# ── AniList, under the default policy: by address ─────────────────────────
echo "AniList relay, AMS_ANILIST_AUTH=allowlist (default)"

step "a query is relayed by the name graphql.anilist.co, and the locked title written into the entry"
asked_reset
anilist -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -d "$QUERY" \
  | py 'm = json.load(sys.stdin)["data"]["Media"]; assert m["title"]["english"] == "Locked Anime Title", m; assert m["title"]["userPreferred"] == "Locked Anime Title", m; assert m["title"]["romaji"] == "Upstream Romaji", m; assert m["description"] == "Upstream description", m' \
  || fail "the entry was not patched as expected"
[ "$(asked_last / authorization)" = "null" ] || fail "an Authorization reached AniList: $(asked_last / authorization)"
[ "$(asked_last / x-ams-instance)" != "null" ] || fail "the relay did not mark its request"
ok

step "the same anonymous query is served from the cache the second time"
anilist -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -d "$QUERY"
[ "$(asked_count /)" = "1" ] || fail "AniList was asked $(asked_count /) times"
ok

step "a list read the way Yamtrack reads one — entries named by MyAnimeList's id — is patched, and never kept"
asked_reset
LIST='{"query":"query ($userName: String) { MediaListCollection(userName: $userName, type: ANIME) { lists { entries { status media { title { userPreferred } coverImage { large } idMal episodes } } } } }","variables":{"userName":"somebody"}}'
anilist -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -d "$LIST" \
  | py 'm = json.load(sys.stdin)["data"]["MediaListCollection"]["lists"][0]["entries"][0]["media"]; assert m["title"]["userPreferred"] == "Locked Anime Title", m; assert m["idMal"] == 16498, m' \
  || fail "the list's entry was not patched by its MyAnimeList id"
anilist -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -d "$LIST"
[ "$(asked_count /)" = "2" ] || fail "somebody's list was served from the cache"
ok

step "somebody's own AniList token travels with the query, and the answer is theirs alone"
asked_reset
anilist -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -H 'Authorization: Bearer their-anilist-token' -d "$QUERY"
anilist -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -H 'Authorization: Bearer their-anilist-token' -d "$QUERY"
[ "$(asked_count /)" = "2" ] || fail "a signed query was served from the cache"
[ "$(asked_last / authorization)" = "\"Bearer their-anilist-token\"" ] || fail "their token did not travel: $(asked_last / authorization)"
ok

step "a key of this server, as a bearer or a header, never leaves"
asked_reset
anilist -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -H "Authorization: Bearer ${KEY}" -H "x-api-key: ${KEY}" -d '{"query":"query { Media(id: 1) { id title { english } } }"}'
[ "$(asked_last / authorization)" = "null" ] || fail "the key travelled as a bearer"
[ "$(asked_last / x-api-key)" = "null" ] || fail "the key travelled as a header"
ok

step "a mutation is relayed every time, never from the cache"
asked_reset
MUTATION='{"query":"mutation { SaveMediaListEntry(mediaId: 1, status: CURRENT) { id } }"}'
anilist -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -H 'Authorization: Bearer their-anilist-token' -d "$MUTATION"
anilist -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -H 'Authorization: Bearer their-anilist-token' -d "$MUTATION"
[ "$(asked_count /)" = "2" ] || fail "a mutation was served from the cache"
ok

step "AniList's rate limit is handed back with its headers"
curl -fsS -o /dev/null -X POST "$FAKE/_fake/ratelimit"
HEADERS=$(anilist -D - -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -d '{"query":"query { Media(id: 2) { id } }"}')
printf '%s' "$HEADERS" | grep -q "^HTTP/.* 429" || fail "expected 429: ${HEADERS:0:200}"
printf '%s' "$HEADERS" | grep -qi "^retry-after: 3" || fail "no Retry-After"
printf '%s' "$HEADERS" | grep -qi "^x-ratelimit-remaining: 0" || fail "no X-RateLimit-Remaining"
ok

step "a GET with the query in the address is relayed as one"
asked_reset
anilist -f -o /dev/null "$ANILIST_DOOR/?query=%7B%20Media(id%3A%203)%20%7B%20id%20%7D%20%7D" || fail "the GET was refused"
[ "$(asked_last / method)" = "\"GET\"" ] || fail "not relayed as a GET"
ok

step "the root of any other name is nothing; a looped query is a 508; switched off, a 503"
code=$(curl -sS -o /dev/null -w '%{http_code}' --cacert "$WORK/tls/ca.crt" --resolve "skyhook.sonarr.tv:${CLIENTS_PORT}:127.0.0.1" -X POST "https://skyhook.sonarr.tv:${CLIENTS_PORT}/" -d "$QUERY")
[ "$code" = "404" ] || fail "expected 404 for skyhook.sonarr.tv's root, got $code"
code=$(anilist -o /dev/null -w '%{http_code}' -X POST "$ANILIST_DOOR/" -H 'x-ams-instance: elsewhere' -d "$QUERY")
[ "$code" = "508" ] || fail "expected 508, got $code"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/settings/server/-" \
  -H 'content-type: application/json' -d '{"key":"api.anilist","value":"false"}'
code=$(anilist -o /dev/null -w '%{http_code}' -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -d "$QUERY")
[ "$code" = "503" ] || fail "expected 503, got $code"
curl -fsS -b "$COOKIES" -o /dev/null -X PUT "$BASE/api/v1/settings/server/-" \
  -H 'content-type: application/json' -d '{"key":"api.anilist","value":"true"}'
ok

step "the access page counts both relays' calls"
curl -fsS -b "$COOKIES" "$BASE/api/v1/admin/access" \
  | py 'apis = {a["api"]: a for a in json.load(sys.stdin)["apis"]}; assert apis["tvdb"]["served"] >= 8, apis["tvdb"]; assert apis["tvdb"]["refused"] >= 2, apis["tvdb"]; assert apis["anilist"]["served"] >= 7, apis["anilist"]; assert apis["tvdb"]["policy"] == "apikey" and apis["anilist"]["policy"] == "allowlist", apis' \
  || fail "the counts are not what the run made"
ok

# ── The policies swapped: TheTVDB by address, AniList by key ──────────────
stop_server
start_server server-2.log AMS_TVDB_AUTH=allowlist AMS_ANILIST_AUTH=apikey
echo "the policies swapped: AMS_TVDB_AUTH=allowlist, AMS_ANILIST_AUTH=apikey"

step "a client's own TheTVDB key is signed in with at TheTVDB, and its token travels as it came"
asked_reset
THEIRS=$(tvdb -X POST "$TVDB_DOOR/v4/login" -H 'content-type: application/json' -d '{"apikey":"their-tvdb-key","pin":"1234"}' \
  | py 'print(json.load(sys.stdin)["data"]["token"])') \
  || fail "TheTVDB's token was not handed back"
[ "$THEIRS" = "fake-token-for-their-tvdb-key" ] || fail "not TheTVDB's token: $THEIRS"
[ "$(asked_last /v4/login body)" = "\"{\\\"apikey\\\":\\\"their-tvdb-key\\\",\\\"pin\\\":\\\"1234\\\"}\"" ] || fail "the sign-in did not travel whole: $(asked_last /v4/login body)"
tvdb -H "Authorization: Bearer ${THEIRS}" "$TVDB_DOOR/v4/series/${TVDB_ID}" \
  | py 'd = json.load(sys.stdin)["data"]; assert d["name"] == "Locked Series Title", d' \
  || fail "the record was not patched for a client with its own token"
[ "$(asked_last /v4/series authorization)" = "\"Bearer ${THEIRS}\"" ] || fail "their token did not travel: $(asked_last /v4/series authorization)"
tvdb -o /dev/null -H "Authorization: Bearer ${THEIRS}" "$TVDB_DOOR/v4/series/${TVDB_ID}"
[ "$(asked_count /v4/series)" = "2" ] || fail "an answer to a client's own token was served from the cache"
ok

step "a key issued here still stands in for the operator's token under this policy, and is judged"
asked_reset
tvdb -o /dev/null -f -H "Authorization: Bearer ${KEY}" "$TVDB_DOOR/v4/series/${TVDB_ID}/translations/eng" || fail "refused"
[ "$(asked_last /v4/series authorization)" = "\"Bearer fake-token-for-${OPERATOR_TVDB_KEY}\"" ] || fail "not the operator's token"
code=$(tvdb -o /dev/null -w '%{http_code}' -H "Authorization: Bearer ams_not_a_key_of_this_server" "$TVDB_DOOR/v4/series/${TVDB_ID}")
[ "$code" = "401" ] || fail "expected 401 for a key this server never issued, got $code"
ANSWER=$(tvdb -w '\n%{http_code}' -H "Authorization: Bearer ${MEMBER_KEY}" "$TVDB_DOOR/v4/series/${TVDB_ID}")
[ "${ANSWER##*$'\n'}" = "403" ] || fail "expected 403 for a member's key, got ${ANSWER##*$'\n'}"
printf '%s' "${ANSWER%$'\n'*}" | grep -q relay_not_for_members || fail "not the members' refusal: $ANSWER"
[ "$(asked_count /v4/series)" = "1" ] || fail "TheTVDB was asked for a key this server refused"
ok

step "with no token at all, TheTVDB's own refusal is handed back"
code=$(tvdb -o /dev/null -w '%{http_code}' "$TVDB_DOOR/v4/series/${TVDB_ID}")
[ "$code" = "401" ] || fail "expected 401, got $code"
ok

step "AniList by key: nothing without one, and a key as a header or a bearer stays here"
code=$(anilist -o /dev/null -w '%{http_code}' -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -d "$QUERY")
[ "$code" = "401" ] || fail "expected 401, got $code"
asked_reset
anilist -f -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -H "x-api-key: ${KEY}" -d "$QUERY" || fail "refused with a key"
[ "$(asked_last / x-api-key)" = "null" ] || fail "the key travelled"
anilist -f -o /dev/null -X POST "$ANILIST_DOOR/" -H 'content-type: application/json' -H "Authorization: Bearer ${KEY}" -d '{"query":"query { Media(id: 4) { id } }"}' || fail "refused with a bearer key"
[ "$(asked_last / authorization)" = "null" ] || fail "the key travelled as a bearer"
ok

echo "e2e relays: all good"
