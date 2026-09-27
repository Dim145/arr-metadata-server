#!/usr/bin/env bash
# What the server answers under load, in numbers to keep: requests a second
# and the latency at p50 / p95 / p99, for the handful of requests that matter
# — the catalogue's pages, a search, Sonarr's two calls, a thumbnail, the
# interface's own document. Run before and after a change, on a release
# build; a debug build measures the compiler, not the server.
#
#   cargo build --release
#   ./scripts/load/serve.sh &          # a release server on 18479, catalogue of its own
#   ./scripts/load/baseline.sh         # against http://127.0.0.1:18479
#   BASE=http://host:8080 ./scripts/load/baseline.sh
#
# Needs oha (https://github.com/hatoo/oha): `brew install oha` / `cargo install oha`.
# Every scenario runs DURATION seconds at CONNECTIONS connections; the
# summary is a markdown table on stdout, ready for docs/perf/.

set -euo pipefail

BASE="${BASE:-http://127.0.0.1:18479}"
DURATION="${DURATION:-10s}"
CONNECTIONS="${CONNECTIONS:-32}"
TERM_="${TERM_:-the}"

command -v oha >/dev/null || { echo "oha is required (brew install oha / cargo install oha)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
curl -sf -o /dev/null "$BASE/health" || { echo "nothing answers at $BASE" >&2; exit 1; }
case "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/items?limit=1")" in
  200) ;;
  401|403) echo "the catalogue at $BASE is private: the scenarios read it as a visitor (AMS_PUBLIC_BROWSE=true)" >&2; exit 1 ;;
  *) echo "the catalogue at $BASE does not answer as expected" >&2; exit 1 ;;
esac
TERM_ENC="$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$TERM_")"

py() { python3 -c "import sys, json; $1"; }

# One series and one film, whatever the catalogue holds, and a thumbnail if
# any picture is kept here.
SERIES=$(curl -sf "$BASE/api/v1/items?kind=series&limit=1" | py 'd = json.load(sys.stdin); print(d["items"][0]["id"] if d.get("items") else "")')
MOVIE=$(curl -sf "$BASE/api/v1/items?kind=movie&limit=1" | py 'd = json.load(sys.stdin); print(d["items"][0]["id"] if d.get("items") else "")')
TVDB=$(curl -sf "$BASE/api/v1/items/${SERIES}" 2>/dev/null | py 'd = json.load(sys.stdin); print((d.get("externalIds") or {}).get("tvdb") or "")' || true)
THUMB=$(curl -sf "$BASE/api/v1/items?kind=series&limit=40" | py 'd = json.load(sys.stdin); urls = [i.get("poster") or "" for i in d.get("items", [])]; k = [u for u in urls if "/media/" in u]; print(k[0].replace(".jpg", "-t.jpg").replace(".png", "-t.jpg").replace(".webp", "-t.jpg") if k else "")' || true)

scenarios=()
scenarios+=("catalogue list|$BASE/api/v1/items?kind=series&limit=24")
[ -n "$SERIES" ] && scenarios+=("one series|$BASE/api/v1/items/$SERIES")
[ -n "$MOVIE" ] && scenarios+=("one film|$BASE/api/v1/items/$MOVIE")
scenarios+=("search '$TERM_'|$BASE/api/v1/items?term=$TERM_ENC&limit=24")
scenarios+=("sonarr search|$BASE/v1/tvdb/search/en/?term=$TERM_ENC")
[ -n "$TVDB" ] && scenarios+=("sonarr show|$BASE/v1/tvdb/shows/en/$TVDB")
[ -n "$THUMB" ] && scenarios+=("thumbnail|$BASE$THUMB")
scenarios+=("interface document|$BASE/")

echo "load: $BASE · ${DURATION} × ${CONNECTIONS} connections · $(date -u +%FT%TZ)"
echo
echo "| Scenario | req/s | p50 | p95 | p99 | non-2xx |"
echo "|---|---:|---:|---:|---:|---:|"
for entry in "${scenarios[@]}"; do
  name="${entry%%|*}"; url="${entry#*|}"
  if ! out="$(oha -z "$DURATION" -c "$CONNECTIONS" --no-tui --output-format json --disable-compression "$url" 2>&1)"; then
    printf '| %s | failed: %s |\n' "$name" "$(printf '%s' "$out" | tail -1)"
    continue
  fi
  # The name reaches Python as an environment variable, never as source.
  NAME="$name" printf '%s' "$out" | py "import os; d = json.load(sys.stdin); s = d['summary']; lp = d['latencyPercentiles']; codes = d.get('statusCodeDistribution', {}); bad = sum(v for k, v in codes.items() if not k.startswith('2')); ms = lambda x: f'{x*1000:.1f} ms'; print(f\"| {os.environ['NAME']} | {s['requestsPerSec']:.0f} | {ms(lp['p50'])} | {ms(lp['p95'])} | {ms(lp['p99'])} | {bad} |\")"
done
