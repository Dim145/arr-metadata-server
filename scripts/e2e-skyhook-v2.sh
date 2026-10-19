#!/usr/bin/env bash
# The real Skyhook's second edition beside this server's, for a few real
# series: each is fetched from skyhook.sonarr.tv and from the server given,
# as Sonarr v5 asks for it, and the two answers compared — the keys at the
# top and what kind of value each holds, the season types, the seasons and
# how many episodes each has, the languages the series is named in, the two
# names of an episode's ids — every difference an addition this server makes
# or a failure, but for an order made of episodes the aired list does not
# hold, which is a limit this server has yet and is said to be one.
#
# Reaches the internet, for Skyhook's side. The server is one already
# running, given by address, that admits this host on the Sonarr surface:
# that surface is guarded by address, not by key. Needs python3 and nothing
# else. A certificate the server made for itself is not checked.
#
#   ./scripts/e2e-skyhook-v2.sh http://127.0.0.1:8080
#   SERIES="412806 433637" LANGUAGE=fra ./scripts/e2e-skyhook-v2.sh https://skyhook.sonarr.tv
#   SKYHOOK=https://…    # another real service to compare with
#
# The first form is the web door; the second, the clients' door reached by
# the name Sonarr calls, resolved to the server on this host.

set -euo pipefail

SERVER="${1:-${SERVER:-}}"
[ -n "$SERVER" ] || { echo "usage: $0 <server address>" >&2; exit 1; }
SKYHOOK="${SKYHOOK:-https://skyhook.sonarr.tv}"
LANGUAGE="${LANGUAGE:-eng}"
# Stranger Things, Breaking Bad — which has a DVD order — an anime named in
# many languages, and The Office (US), whose Blu-ray and streaming orders
# (`altdvd`, `alttwo`) TheTVDB's own episodes endpoint answers empty for
# (thetvdb/v4-api#340) and the real service serves whole.
SERIES="${SERIES:-305288 81189 412806 73244}"

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }

echo "e2e skyhook v2: ${SKYHOOK} beside ${SERVER}, in ${LANGUAGE}"

SERVER="${SERVER%/}" SKYHOOK="${SKYHOOK%/}" LANGUAGE="$LANGUAGE" SERIES="$SERIES" python3 - <<'PY'
import json, os, ssl, sys, urllib.error, urllib.request

SKYHOOK, SERVER, LANGUAGE = os.environ["SKYHOOK"], os.environ["SERVER"], os.environ["LANGUAGE"]
SERIES = [int(s) for s in os.environ["SERIES"].split()]
failures, additions, limits = [], [], []

# The server's own certificate is one it made for itself; the real service's
# is checked as a browser would.
LENIENT = ssl.create_default_context()
LENIENT.check_hostname = False
LENIENT.verify_mode = ssl.CERT_NONE

def fetch(base, path, lenient):
    request = urllib.request.Request(base + path, headers={"User-Agent": "Sonarr/5.0.0.0 (e2e-skyhook-v2)"})
    try:
        with urllib.request.urlopen(request, timeout=120, context=LENIENT if lenient else None) as response:
            return json.loads(response.read().decode())
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"{base}{path}: {e.code} {e.read()[:300]!r}") from None

def fail(message):
    failures.append(message)
    print(f"    FAILED: {message}")

def add(message):
    additions.append(message)
    print(f"    + {message}")

def limit(message):
    limits.append(message)
    print(f"    limit: {message}")

def kind(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "list"
    return "object"

def per_season(show):
    counts = {}
    for episode in show.get("episodes") or []:
        counts[episode["seasonNumber"]] = counts.get(episode["seasonNumber"], 0) + 1
    return counts

for tvdb in SERIES:
    path = f"/v2/tvdb/shows/{LANGUAGE}/{tvdb}/official"
    try:
        real = fetch(SKYHOOK, path, lenient=False)
    except (RuntimeError, OSError) as e:
        fail(f"tvdb:{tvdb}: the real service did not answer: {e}")
        continue
    try:
        ours = fetch(SERVER, path, lenient=True)
    except (RuntimeError, OSError) as e:
        fail(f"tvdb:{tvdb}: this server did not answer: {e}")
        continue
    print(f"  tvdb:{tvdb} — ours “{ours.get('title')}”, real “{real.get('title')}”")

    # The keys at the top, and what each holds.
    for key in sorted(set(real) - set(ours)):
        if real[key] in (None, [], {}):
            continue
        fail(f"tvdb:{tvdb}: key missing here: {key} (real: {kind(real[key])})")
    for key in sorted(set(ours) - set(real)):
        add(f"tvdb:{tvdb}: key the real one lacks: {key}")
    for key in sorted(set(ours) & set(real)):
        if kind(ours[key]) != kind(real[key]) and real[key] is not None:
            fail(f"tvdb:{tvdb}: {key} is a {kind(ours[key])} here, a {kind(real[key])} there")
    for field in ("originalTitle", "originalLanguage", "metadataSource", "formats", "status", "firstAired"):
        if ours.get(field) != real.get(field):
            print(f"    differs: {field}: ours {ours.get(field)!r}, real {real.get(field)!r}")

    # The orders Sonarr may ask the series in. One the real service offers
    # and this server does not is a failure when the aired list holds any of
    # its episodes — this server places an order's episodes among the ones
    # it holds — and a limit when it holds none: TheTVDB files some orders'
    # episodes as records of their own (The Office (US)'s Superfan order is
    # 194 extended cuts), which the catalogue does not keep yet. See
    # docs/sonarr-radarr.md, "Orders".
    ours_types = {t["type"]: t for t in ours.get("seasonTypes") or []}
    real_types = {t["type"]: t for t in real.get("seasonTypes") or []}
    aired_ids = {e.get("tvdbId") for e in real.get("episodes") or []}
    for name in sorted(set(real_types) - set(ours_types)):
        try:
            order = fetch(SKYHOOK, f"/v2/tvdb/shows/{LANGUAGE}/{tvdb}/{name}", lenient=False)
        except (RuntimeError, OSError) as e:
            fail(f"tvdb:{tvdb}: season type missing here: {name} (and the real service did not answer for it: {e})")
            continue
        episodes = order.get("episodes") or []
        held = [e for e in episodes if e.get("tvdbId") in aired_ids]
        if held or not episodes:
            fail(f"tvdb:{tvdb}: season type missing here: {name} ({len(held)} of its {len(episodes)} episodes are in the aired list)")
        else:
            limit(f"tvdb:{tvdb}: season type {name} is {len(episodes)} episodes the aired list does not hold; not kept here yet")
    for name in sorted(set(ours_types) - set(real_types)):
        add(f"tvdb:{tvdb}: season type the real one lacks: {name}")
    for name in sorted(set(ours_types) & set(real_types)):
        mine, theirs = ours_types[name], real_types[name]
        if sorted(mine.get("seasonNumbers") or []) != sorted(theirs.get("seasonNumbers") or []):
            print(f"    differs: seasonTypes[{name}].seasonNumbers: ours {mine.get('seasonNumbers')}, real {theirs.get('seasonNumbers')}")
        if mine.get("episodeCount") != theirs.get("episodeCount"):
            print(f"    differs: seasonTypes[{name}].episodeCount: ours {mine.get('episodeCount')}, real {theirs.get('episodeCount')}")
    if "official" not in ours_types:
        fail(f"tvdb:{tvdb}: no official season type here")

    # The seasons, and the episodes of each.
    ours_seasons = {s["seasonNumber"]: s for s in ours.get("seasons") or []}
    real_seasons = {s["seasonNumber"]: s for s in real.get("seasons") or []}
    if len(ours_seasons) != len(ours.get("seasons") or []):
        fail(f"tvdb:{tvdb}: a season number is listed twice here")
    for number in sorted(set(real_seasons) - set(ours_seasons)):
        fail(f"tvdb:{tvdb}: season {number} missing here")
    for number in sorted(set(ours_seasons) - set(real_seasons)):
        add(f"tvdb:{tvdb}: season {number} the real one lacks")
    for number in sorted(set(ours_seasons) & set(real_seasons)):
        for field in ("title", "year"):
            if ours_seasons[number].get(field) != real_seasons[number].get(field):
                print(f"    differs: season {number} {field}: ours {ours_seasons[number].get(field)!r}, real {real_seasons[number].get(field)!r}")
    ours_counts, real_counts = per_season(ours), per_season(real)
    for number in sorted(set(real_counts) | set(ours_counts)):
        mine, theirs = ours_counts.get(number, 0), real_counts.get(number, 0)
        if mine < theirs:
            fail(f"tvdb:{tvdb}: season {number} has {mine} episodes here, {theirs} there")
        elif mine > theirs:
            add(f"tvdb:{tvdb}: season {number} has {mine - theirs} episodes the real one lacks")

    # The languages the series is named in: the one asked and the series'
    # own at least, since Sonarr reads the original title from the latter.
    ours_languages = {t["language"] for t in ours.get("translations") or [] if t.get("title")}
    real_languages = {t["language"] for t in real.get("translations") or [] if t.get("title")}
    for wanted in dict.fromkeys([LANGUAGE, ours.get("originalLanguage"), "eng"]):
        if wanted and wanted not in ours_languages:
            fail(f"tvdb:{tvdb}: no translation in {wanted} here")
    # The real service names every language TheTVDB holds; this server
    # holds the ones it was asked for, and fetches another the first time
    # it is asked — a difference, not a failure, once the asked and the
    # original are there.
    missing = sorted(real_languages - ours_languages)
    if missing:
        print(f"    differs: tvdb:{tvdb}: {len(missing)} languages the real one names that nobody asked this server for yet: {missing}")
    extra = sorted(ours_languages - real_languages)
    if extra:
        add(f"tvdb:{tvdb}: {len(extra)} languages the real one lacks: {extra}")

    # An episode's ids under both names: the real service's and the one
    # Sonarr v5 reads.
    episodes = ours.get("episodes") or []
    if not episodes:
        fail(f"tvdb:{tvdb}: no episodes here")
    for field in ("tvdbShowId", "metadataShowId", "tvdbId", "metadataId", "seasonNumber", "episodeNumber", "title"):
        lacking = [e for e in episodes if field not in e]
        if lacking:
            fail(f"tvdb:{tvdb}: {len(lacking)} episodes without {field} here")
    real_finales = sum(1 for e in real.get("episodes") or [] if e.get("finaleType"))
    ours_finales = sum(1 for e in episodes if e.get("finaleType"))
    if real_finales != ours_finales:
        print(f"    differs: episodes with a finaleType: ours {ours_finales}, real {real_finales}")

print(f"  {len(failures)} failures, {len(additions)} additions, {len(limits)} known limits")
sys.exit(1 if failures else 0)
PY

echo "e2e skyhook v2: all good"
