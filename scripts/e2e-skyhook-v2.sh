#!/usr/bin/env bash
# The real Skyhook's second edition beside this server's, for a few real
# series: each is fetched from skyhook.sonarr.tv and from the server given,
# as Sonarr v5 asks for it, and the two answers compared — the keys at the
# top and what kind of value each holds, the season types, the seasons and
# how many episodes each has, the languages the series is named in, the two
# names of an episode's ids — every difference an addition this server makes
# or a failure. A season type the real service offers is one this server
# offers too, an order made of episodes the aired list does not hold among
# them: it keeps the records TheTVDB files in one order alone.
#
# Then the same series by TMDB's id, as the real service's TMDB edition
# serves them (`/v2/tmdb/…`), which Sonarr v5 does not call yet: numbered as
# TMDB numbers them, each episode by its TMDB id, TMDB's episode groups as
# season types under their bare ids — and a search for each by `tmdb:`.
#
# Reaches the internet, for Skyhook's side. The server is one already
# running, given by address, that admits this host on the Sonarr surface:
# that surface is guarded by address, not by key. Needs python3 and nothing
# else. A certificate the server made for itself is not checked.
#
#   ./scripts/e2e-skyhook-v2.sh http://127.0.0.1:8080
#   SERIES="412806 433637" LANGUAGE=fra ./scripts/e2e-skyhook-v2.sh https://skyhook.sonarr.tv
#   TMDB_SERIES="1396 66732"   # by TMDB's id: the real service's for SERIES otherwise
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
# By TMDB's id: those the real service names the series above by, unless
# given. Breaking Bad has a TMDB group, its DVD order.
TMDB_SERIES="${TMDB_SERIES:-}"

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }

echo "e2e skyhook v2: ${SKYHOOK} beside ${SERVER}, in ${LANGUAGE}"

SERVER="${SERVER%/}" SKYHOOK="${SKYHOOK%/}" LANGUAGE="$LANGUAGE" SERIES="$SERIES" TMDB_SERIES="$TMDB_SERIES" python3 - <<'PY'
import http.client, json, os, ssl, sys, time, urllib.error, urllib.request

SKYHOOK, SERVER, LANGUAGE = os.environ["SKYHOOK"], os.environ["SERVER"], os.environ["LANGUAGE"]
SERIES = [int(s) for s in os.environ["SERIES"].split()]
TMDB_SERIES = [int(s) for s in os.environ["TMDB_SERIES"].split()]
failures, additions = [], []

# The server's own certificate is one it made for itself; the real service's
# is checked as a browser would.
LENIENT = ssl.create_default_context()
LENIENT.check_hostname = False
LENIENT.verify_mode = ssl.CERT_NONE

def fetch(base, path, lenient):
    request = urllib.request.Request(base + path, headers={"User-Agent": "Sonarr/5.0.0.0 (e2e-skyhook-v2)"})
    # A body cut short, which the real service sends now and then to a
    # client that closes its connection, is asked for again.
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=120, context=LENIENT if lenient else None) as response:
                return json.loads(response.read().decode())
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"{base}{path}: {e.code} {e.read()[:300]!r}") from None
        except http.client.IncompleteRead:
            if attempt == 2:
                raise RuntimeError(f"{base}{path}: the body was cut short three times") from None
            time.sleep(2)

def fail(message):
    failures.append(message)
    print(f"    FAILED: {message}")

def add(message):
    additions.append(message)
    print(f"    + {message}")

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

def keys_at_the_top(label, ours, real):
    """The keys at the top, and what each holds."""
    for key in sorted(set(real) - set(ours)):
        if real[key] in (None, [], {}):
            continue
        fail(f"{label}: key missing here: {key} (real: {kind(real[key])})")
    for key in sorted(set(ours) - set(real)):
        add(f"{label}: key the real one lacks: {key}")
    for key in sorted(set(ours) & set(real)):
        if kind(ours[key]) != kind(real[key]) and real[key] is not None:
            fail(f"{label}: {key} is a {kind(ours[key])} here, a {kind(real[key])} there")
    for field in ("originalTitle", "originalLanguage", "metadataSource", "formats", "status", "firstAired"):
        if ours.get(field) != real.get(field):
            print(f"    differs: {field}: ours {ours.get(field)!r}, real {real.get(field)!r}")

def season_types(label, ours, real, default):
    """The orders Sonarr may ask the series in. One the real service offers
    and this server does not is a failure; one this server adds is not."""
    ours_types = {t["type"]: t for t in ours.get("seasonTypes") or []}
    real_types = {t["type"]: t for t in real.get("seasonTypes") or []}
    for name in sorted(set(real_types) - set(ours_types)):
        fail(f"{label}: season type missing here: {name} ({real_types[name].get('episodeCount')} episodes there)")
    for name in sorted(set(ours_types) - set(real_types)):
        add(f"{label}: season type the real one lacks: {name}")
    for name in sorted(set(ours_types) & set(real_types)):
        mine, theirs = ours_types[name], real_types[name]
        if mine.get("name") != theirs.get("name"):
            print(f"    differs: seasonTypes[{name}].name: ours {mine.get('name')!r}, real {theirs.get('name')!r}")
        if sorted(mine.get("seasonNumbers") or []) != sorted(theirs.get("seasonNumbers") or []):
            print(f"    differs: seasonTypes[{name}].seasonNumbers: ours {mine.get('seasonNumbers')}, real {theirs.get('seasonNumbers')}")
        if mine.get("episodeCount") != theirs.get("episodeCount"):
            print(f"    differs: seasonTypes[{name}].episodeCount: ours {mine.get('episodeCount')}, real {theirs.get('episodeCount')}")
    if default not in ours_types:
        fail(f"{label}: no {default} season type here")

def seasons(label, ours, real):
    """The seasons, and the episodes of each."""
    ours_seasons = {s["seasonNumber"]: s for s in ours.get("seasons") or []}
    real_seasons = {s["seasonNumber"]: s for s in real.get("seasons") or []}
    if len(ours_seasons) != len(ours.get("seasons") or []):
        fail(f"{label}: a season number is listed twice here")
    for number in sorted(set(real_seasons) - set(ours_seasons)):
        fail(f"{label}: season {number} missing here")
    for number in sorted(set(ours_seasons) - set(real_seasons)):
        add(f"{label}: season {number} the real one lacks")
    for number in sorted(set(ours_seasons) & set(real_seasons)):
        for field in ("title", "year"):
            if ours_seasons[number].get(field) != real_seasons[number].get(field):
                print(f"    differs: season {number} {field}: ours {ours_seasons[number].get(field)!r}, real {real_seasons[number].get(field)!r}")
    ours_counts, real_counts = per_season(ours), per_season(real)
    for number in sorted(set(real_counts) | set(ours_counts)):
        mine, theirs = ours_counts.get(number, 0), real_counts.get(number, 0)
        if mine < theirs:
            fail(f"{label}: season {number} has {mine} episodes here, {theirs} there")
        elif mine > theirs:
            add(f"{label}: season {number} has {mine - theirs} episodes the real one lacks")

def languages(label, ours, real):
    """The languages the series is named in: the one asked and the series'
    own at least, since Sonarr reads the original title from the latter.
    The real service names every language its source holds; this server
    holds the ones it was asked for, and fetches another the first time it
    is asked — a difference, not a failure, once the asked and the original
    are there."""
    ours_languages = {t["language"] for t in ours.get("translations") or [] if t.get("title")}
    real_languages = {t["language"] for t in real.get("translations") or [] if t.get("title")}
    for wanted in dict.fromkeys([LANGUAGE, ours.get("originalLanguage"), "eng"]):
        if wanted and wanted not in ours_languages:
            fail(f"{label}: no translation in {wanted} here")
    missing = sorted(real_languages - ours_languages)
    if missing:
        print(f"    differs: {label}: {len(missing)} languages the real one names that nobody asked this server for yet: {missing}")
    extra = sorted(ours_languages - real_languages)
    if extra:
        add(f"{label}: {len(extra)} languages the real one lacks: {extra}")

def episode_ids(label, ours, real, fields):
    """Every episode carries the ids it is known by."""
    episodes = ours.get("episodes") or []
    if not episodes:
        fail(f"{label}: no episodes here")
    for field in fields:
        lacking = [e for e in episodes if field not in e]
        if lacking:
            fail(f"{label}: {len(lacking)} episodes without {field} here")
    real_finales = sum(1 for e in real.get("episodes") or [] if e.get("finaleType"))
    ours_finales = sum(1 for e in episodes if e.get("finaleType"))
    if real_finales != ours_finales:
        print(f"    differs: episodes with a finaleType: ours {ours_finales}, real {real_finales}")

def both(label, path):
    """The real service's answer and this server's, or none when either did
    not answer, which is a failure."""
    try:
        real = fetch(SKYHOOK, path, lenient=False)
    except (RuntimeError, OSError) as e:
        fail(f"{label}: the real service did not answer: {e}")
        return None
    try:
        ours = fetch(SERVER, path, lenient=True)
    except (RuntimeError, OSError) as e:
        fail(f"{label}: this server did not answer: {e}")
        return None
    return ours, real

# By TheTVDB's id, as Sonarr v5 asks today.
tmdb_ids = []
for tvdb in SERIES:
    label = f"tvdb:{tvdb}"
    answered = both(label, f"/v2/tvdb/shows/{LANGUAGE}/{tvdb}/official")
    if answered is None:
        continue
    ours, real = answered
    print(f"  {label} — ours “{ours.get('title')}”, real “{real.get('title')}”")
    if real.get("tmdbId"):
        tmdb_ids.append(real["tmdbId"])

    keys_at_the_top(label, ours, real)
    # An order made of episodes the aired list does not hold is one as much
    # as any: TheTVDB files some orders' episodes as records of their own
    # (The Office (US)'s Superfan order is 194 extended cuts), which the
    # catalogue keeps with the order and serves as episodes. See
    # docs/sonarr-radarr.md, "Orders". A TMDB group or an order made here is
    # one this server adds.
    season_types(label, ours, real, "official")
    seasons(label, ours, real)
    languages(label, ours, real)
    # An episode's ids under both names: the real service's and the one
    # Sonarr v5 reads.
    episode_ids(label, ours, real, ("tvdbShowId", "metadataShowId", "tvdbId", "metadataId", "seasonNumber", "episodeNumber", "title"))

# By TMDB's id, as the real service's TMDB edition serves a series: TMDB's
# own numbering under `default`, TMDB's groups by their bare ids, every
# episode by its TMDB id. See docs/sonarr-radarr.md, "Sonarr v5 by TMDB's id".
for tmdb in TMDB_SERIES or tmdb_ids:
    label = f"tmdb:{tmdb}"
    answered = both(label, f"/v2/tmdb/shows/{LANGUAGE}/{tmdb}/default")
    if answered is None:
        continue
    ours, real = answered
    print(f"  {label} — ours “{ours.get('title')}”, real “{real.get('title')}”")

    keys_at_the_top(label, ours, real)
    season_types(label, ours, real, "default")
    seasons(label, ours, real)
    languages(label, ours, real)
    episode_ids(label, ours, real, ("tmdbShowId", "tmdbId", "seasonNumber", "episodeNumber", "title"))
    if ours.get("metadataSource") != "tmdb":
        fail(f"{label}: metadataSource is {ours.get('metadataSource')!r} here")
    # TMDB's episodes, as both name them: one the real service numbers and
    # this server does not is told, not failed — TMDB may have moved since
    # the series was last read here.
    real_ids = {e.get("tmdbId") for e in real.get("episodes") or []}
    ours_ids = {e.get("tmdbId") for e in ours.get("episodes") or []}
    unknown = sorted(i for i in real_ids - ours_ids if i is not None)
    if unknown:
        print(f"    differs: {label}: {len(unknown)} of TMDB's episodes not served here: {unknown[:10]}")

    # Found by its id, as one search result without its episodes.
    answered = both(f"{label} search", f"/v2/tmdb/search?term=tmdb:{tmdb}&language={LANGUAGE}")
    if answered is None:
        continue
    ours, real = answered
    if not isinstance(ours, list) or len(ours) != 1 or ours[0].get("tmdbId") != tmdb:
        fail(f"{label}: the search for tmdb:{tmdb} found {[s.get('tmdbId') for s in ours or []]} here")
        continue
    if real:
        keys_at_the_top(f"{label} search", ours[0], real[0])
    if "episodes" in ours[0]:
        fail(f"{label}: a search result carries its episodes here")

print(f"  {len(failures)} failures, {len(additions)} additions")
sys.exit(1 if failures else 0)
PY

echo "e2e skyhook v2: all good"
