#!/usr/bin/env bash
# The catalogue the Playwright suite reads, put into an empty server.
#
# frontend/e2e does not make the works it reads: it reads what the server
# holds, and wants films and series, an anime and a Fan-Kai, a series with an
# episode on the air this week, one that has ended, one still to come, one of
# dozens of seasons, an entry made by hand. Those are the works the suite was
# written against — the 32 that data/e2e.db came to hold — put in the way an
# administrator would put them: the providers' own through the import the
# Discover page makes, the one made by hand through the editor's "new entry".
#
#   AMS_E2E_USER=admin AMS_E2E_PASSWORD=… ./scripts/e2e-seed.sh
#
# against the server the suite is about to run on — AMS_E2E_URL, as for the
# suite: http://127.0.0.1:8479 unless it is set — signed in as the
# administrator the suite signs in as. The imports ask TMDB, TheTVDB and
# Fan-Kai, so that server needs its provider keys: AMS_TMDB_API_KEY and
# AMS_TVDB_API_KEY, and AMS_FANART_API_KEY for the artwork. A new database has
# Fan-Kai and the anime sources off, and five of the works are Fan-Kai: start
# the server with AMS_FANKAI_ENABLED, AMS_FANKAI_WIKI_ENABLED,
# AMS_ANILIST_ENABLED, AMS_MAL_ENABLED, AMS_TVMAZE_ENABLED and
# AMS_IMDB_ENABLED set to true, or switch them on from the Sources page first.
#
# Safe to run again. A work the catalogue holds already is left as it is — an
# import would fetch it over again — so a second run is a few requests, and a
# first that was interrupted is finished by the second. A work that fails is
# asked for again twice, a little later: a provider that stumbles once is not
# a reason to start over. SEED_JOBS says how many are fetched at a time (4),
# SEED_ATTEMPTS how many times each is asked for (3).
#
# Frieren was entered by hand in the catalogue this list comes from, and filled
# in from its providers afterwards. A work entered by hand is refused a refresh
# now, so it is imported here like the others.

set -euo pipefail

BASE="${AMS_E2E_URL:-http://127.0.0.1:8479}"
USERNAME="${AMS_E2E_USER:?set AMS_E2E_USER to the administrator the suite signs in as}"
PASSWORD="${AMS_E2E_PASSWORD:?set AMS_E2E_PASSWORD to the password of that administrator}"
JOBS="${SEED_JOBS:-4}"
ATTEMPTS="${SEED_ATTEMPTS:-3}"

command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ams-e2e-seed.XXXXXX")"
COOKIES="$WORK/cookies"
HELD="$WORK/held"
trap 'rm -rf "$WORK"' EXIT

fail() { printf 'FAILED: %s\n' "$1" >&2; exit 1; }

# The works as `kind|how it is imported|its identifier|what it is`: a Fan-Kai
# by its own number, a series by TheTVDB's — the numbering Sonarr matches files
# against — and a film by TMDB's. One series has no TheTVDB entry, so TMDB's.
works() {
  cat <<'LIST'
series|fankaiId|1|Black Lagoon Henshū
series|fankaiId|4|Saint Seiya Kaï
series|fankaiId|12|Naruto Shippuden Yabai
series|fankaiId|27|Bleach Yabai
series|fankaiId|33|Horimiya Kaï
series|tvdbId|81189|Breaking Bad
series|tvdbId|305288|Stranger Things
series|tvdbId|121361|Game of Thrones
series|tvdbId|73739|Lost
series|tvdbId|71663|The Simpsons
series|tvdbId|70753|Covington Cross
series|tvdbId|71209|Yancy Derringer
series|tvdbId|81797|One Piece
series|tvdbId|79824|Naruto Shippūden
series|tvdbId|424536|Frieren: Beyond Journey’s End
series|tvdbId|433637|Harry Potter
series|tvdbId|464490|euphoria
series|tmdbId|124290|Slutty Immoral Older Sister
series|tvdbId|412806|More than a Married Couple, but Not Lovers
series|tvdbId|413578|Rurouni Kenshin, 2023
series|tvdbId|70863|Rurouni Kenshin, 1996
series|tvdbId|440056|Magical Explorer
series|tvdbId|471609|PSYREN
series|tvdbId|469231|Even Though I’m a Super Timid Noble Girl
movie|tmdbId|335984|Blade Runner 2049
movie|tmdbId|329865|Arrival
movie|tmdbId|27205|Inception
movie|tmdbId|157336|Interstellar
movie|tmdbId|129|Spirited Away
movie|tmdbId|550|Fight Club
movie|tmdbId|603|The Matrix
LIST
}

# The one work entered by hand: a film that does not exist, dated far enough on
# that no provider will ever hold it. Made as the editor makes it.
MANUAL_KIND=movie
MANUAL_TITLE="L’Atelier des Ombres"
MANUAL_YEAR=2031

# ── The server ──────────────────────────────────────────────────────────────
for _ in $(seq 1 60); do
  curl -sf -o /dev/null "$BASE/health" && break
  sleep 1
done
curl -sf -o /dev/null "$BASE/health" || fail "no server answers at $BASE"

# Through standard input, so that the password is in no process list.
code="$(
  AMS_SEED_USER="$USERNAME" AMS_SEED_PASSWORD="$PASSWORD" python3 -c '
import json, os
print(json.dumps({"username": os.environ["AMS_SEED_USER"], "password": os.environ["AMS_SEED_PASSWORD"]}))
' | curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIES" -X POST "$BASE/api/v1/auth/login" \
        -H 'content-type: application/json' --data-binary @- || true
)"
[ "$code" = 200 ] || fail "could not sign in at $BASE as $USERNAME (HTTP ${code:-none})"
echo "e2e seed: $BASE, signed in as $USERNAME"

# ── What the catalogue holds already ────────────────────────────────────────
# Each external identifier of each work as `kind:source:value`, and each work
# entered by hand as `kind:title:its title`.
read -r -d '' HOLDINGS <<'PY' || true
import json, sys
page = json.load(sys.stdin)
print("total %d" % page["total"])
for work in page["items"]:
    for source, value in (work.get("externalIds") or {}).items():
        if not isinstance(value, list):
            print("%s:%s:%s" % (work["kind"], source, value))
    if work.get("isManual"):
        print("%s:title:%s" % (work["kind"], work["title"]))
PY

: > "$HELD"
offset=0
total=1
while [ "$offset" -lt "$total" ]; do
  page="$WORK/page"
  curl -fsS -b "$COOKIES" -o "$page" \
    "$BASE/api/v1/items?limit=200&offset=$offset&includeAdult=true&includeDisabled=true" \
    || fail "could not list the catalogue"
  python3 -c "$HOLDINGS" < "$page" > "$WORK/holdings"
  total="$(sed -n 's/^total //p' "$WORK/holdings")"
  grep -v '^total ' "$WORK/holdings" >> "$HELD" || true
  offset=$((offset + 200))
done

holds() { grep -qxF -- "$1" "$HELD"; }

# ── The works the providers have ────────────────────────────────────────────
import_work() { # kind field value label
  local kind="$1" field="$2" value="$3" label="$4"
  local body="$WORK/answer.$kind.$value" attempt=1 status started=$SECONDS

  while :; do
    status="$(curl -sS --max-time 900 -o "$body" -w '%{http_code}' -b "$COOKIES" \
      -X POST "$BASE/api/v1/discover/import" -H 'content-type: application/json' \
      -d "{\"kind\":\"$kind\",\"$field\":$value}" 2>/dev/null)" || status=000

    case "$status" in
      200)
        printf '  %-60s imported, %ss\n' "$label" "$((SECONDS - started))"
        return 0
        ;;
      # Asking again would change nothing.
      400 | 401 | 403) break ;;
    esac

    [ "$attempt" -ge "$ATTEMPTS" ] && break
    sleep $((attempt * 5))
    attempt=$((attempt + 1))
  done

  printf '  %-60s FAILED, HTTP %s after %s attempt(s): %s\n' "$label" "$status" "$attempt" \
    "$(head -c 200 "$body" 2>/dev/null | tr '\n' ' ')" >&2
  echo "$label" >> "$WORK/failed"
  return 1
}

echo "importing, $JOBS at a time"
while IFS='|' read -r kind field value label; do
  # `tmdbId` is `tmdb` in a work's identifiers, `fankaiId` is `fankai`.
  if holds "$kind:${field%Id}:$value"; then
    printf '  %-60s held already\n' "$label"
    continue
  fi

  while [ "$(jobs -rp | wc -l | tr -d ' ')" -ge "$JOBS" ]; do
    sleep 0.3
  done
  import_work "$kind" "$field" "$value" "$label" &
done < <(works)
wait

# ── The work entered by hand ────────────────────────────────────────────────
if holds "$MANUAL_KIND:title:$MANUAL_TITLE"; then
  printf '  %-60s held already\n' "$MANUAL_TITLE"
else
  status="$(AMS_SEED_TITLE="$MANUAL_TITLE" AMS_SEED_KIND="$MANUAL_KIND" AMS_SEED_YEAR="$MANUAL_YEAR" python3 -c '
import json, os
print(json.dumps({"kind": os.environ["AMS_SEED_KIND"], "title": os.environ["AMS_SEED_TITLE"], "year": int(os.environ["AMS_SEED_YEAR"])}))
' | curl -sS -o "$WORK/manual" -w '%{http_code}' -b "$COOKIES" -X POST "$BASE/api/v1/items" \
      -H 'content-type: application/json' --data-binary @- || true)"
  if [ "$status" = 201 ]; then
    printf '  %-60s made by hand\n' "$MANUAL_TITLE"
  else
    printf '  %-60s FAILED, HTTP %s: %s\n' "$MANUAL_TITLE" "${status:-none}" \
      "$(head -c 200 "$WORK/manual" 2>/dev/null | tr '\n' ' ')" >&2
    echo "$MANUAL_TITLE" >> "$WORK/failed"
  fi
fi

# ── The result ──────────────────────────────────────────────────────────────
if [ -s "$WORK/failed" ]; then
  fail "$(wc -l < "$WORK/failed" | tr -d ' ') work(s) could not be put in: $(paste -sd ',' "$WORK/failed")"
fi

held="$(curl -fsS -b "$COOKIES" "$BASE/api/v1/items?limit=1&includeAdult=true&includeDisabled=true" \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["total"])')"
echo "e2e seed: done, the catalogue holds $held works"
