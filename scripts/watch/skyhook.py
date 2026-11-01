"""Skyhook, the metadata service Sonarr reads, against its snapshot: the keys
of four of its documents — not their values — and the last change to
Sonarr's own client of it.

This server imitates Skyhook for Sonarr, and Skyhook moves without notice:
its shape for Sonarr v5 changed twice in October 2026, and a route by TMDB
id, `/v2/tmdb`, answers since, which Sonarr v5 does not call yet. So:

- Breaking Bad as Sonarr v4 reads it (`/v1/tvdb`) and as v5 does
  (`/v2/tvdb`), by TMDB id (`/v2/tmdb`), and a search there. Every key of
  each document is read, at every depth: `episodes[].tvdbId` is the key
  `tvdbId` of the items of `episodes`. One added or gone is a move.
  Skyhook leaves out a key whose value is null, so a key read in a list
  that is empty today was not looked for, and is not counted gone.
- The last commit under src/NzbDrone.Core/MetadataSource on Sonarr's
  `develop`: the day Sonarr starts calling `/v2/tmdb`, it shows there
  first. A new one is a move.

Comparing this server with Skyhook is another script's work,
scripts/e2e-skyhook-v2.sh, which needs a running server; this one needs only
the network. SKYHOOK names another real service to read.
"""

from __future__ import annotations

import json
import os
import re

from common import Refused, Report, Unreadable, clean, fetch, github, keep, shown, snapshot, today

SKYHOOK = os.environ.get("SKYHOOK", "https://skyhook.sonarr.tv").rstrip("/")

# Breaking Bad: an ended series, whose documents change only when Skyhook
# does. TheTVDB 81189, TMDB 1396.
ROUTES = [
    "/v1/tvdb/shows/en/81189",
    "/v2/tvdb/shows/eng/81189/official",
    "/v2/tmdb/shows/eng/1396/default",
    "/v2/tmdb/search?term=breaking%20bad&language=eng",
]

SONARR_CLIENT = "/repos/Sonarr/Sonarr/commits?sha=develop&path=src/NzbDrone.Core/MetadataSource&per_page=1"

# Sonarr's own word for itself, which Skyhook may answer by.
HEADERS = {"Accept": "application/json", "User-Agent": "Sonarr/5.0.0.0 (arr-metadata-server upstream watch)"}


def run(update):
    report = Report("skyhook", "Skyhook and Sonarr's client of it")
    before = snapshot("skyhook") or {}
    taken = before.get("taken")
    if not before:
        report.move("there is no snapshot yet: take one.")

    routes = {}
    for route in ROUTES:
        try:
            document = json.loads(fetch(SKYHOOK + route, headers=HEADERS))
        except Refused as error:
            if error.status in (404, 410):
                report.move(
                    f"`{route}` answers {error.status}: the route is gone. Once that is understood, "
                    "take it out of `ROUTES` in scripts/watch/skyhook.py."
                )
            else:
                report.unreadable(f"`{route}` answers {error.status}.")
            continue
        except (Unreadable, ValueError) as error:
            report.unreadable(f"`{route}`: {clean(error)}")
            continue
        keys, filled = key_paths(document)
        routes[route] = sorted(keys)
        report.fact(f"`{SKYHOOK}{route}`: {len(keys)} keys.")

        if route not in before.get("routes", {}):
            if before:
                report.move(f"`{route}` is not in the snapshot: take it again.")
            continue
        old = set(before["routes"][route])
        for key in sorted(keys - old):
            report.move(f"`{route}`: `{clean(key)}` is new.")
        gone = sorted(old - keys)
        for key in gone:
            if any(other != key and key.startswith(other) and key[len(other)] in ".[" for other in gone):
                continue  # said of the key it sits in
            if not observed(key, filled):
                report.note(f"`{route}`: `{clean(key)}` not looked for: the list it sits in holds nothing today.")
                continue
            report.move(f"`{route}`: `{clean(key)}` is gone.")

    sonarr = None
    try:
        commits = github(SONARR_CLIENT)
        if commits:
            head = commits[0]
            sonarr = {
                "commit": head["sha"],
                "date": head["commit"]["committer"]["date"],
                "subject": head["commit"]["message"].splitlines()[0],
            }
            report.fact(
                f"Sonarr's client of it, src/NzbDrone.Core/MetadataSource on `develop`: last changed "
                f"{sonarr['date'][:10]}, `{sonarr['commit'][:8]}` `{clean(sonarr['subject'], 80)}`."
            )
            known = (before.get("sonarr") or {}).get("commit")
            if known and known != sonarr["commit"]:
                report.move(
                    f"Sonarr's client of Skyhook changed on `develop` ({sonarr['date'][:10]}, "
                    f"`{clean(sonarr['subject'], 100)}`): see whether it calls `/v2/tmdb` now, or reads another key."
                )
    except (Unreadable, Refused, ValueError, KeyError, IndexError) as error:
        report.unreadable(f"Sonarr's client on GitHub: {clean(error)}")

    if update and not report.unreadables:
        path = keep("skyhook", {"taken": today(), "skyhook": SKYHOOK, "routes": routes, "sonarr": sonarr})
        report.fact(f"Snapshot written to {shown(path)}.")
    return report.finish(update, taken)


def key_paths(value, prefix="", keys=None, filled=None):
    """Every key of a document, at every depth, and the lists that held
    something: `episodes[].tvdbId` is the key `tvdbId` of the items of
    `episodes`, `[].title` that of the items of a list at the top."""
    if keys is None:
        keys, filled = set(), set()
    if isinstance(value, dict):
        for key, item in value.items():
            path = f"{prefix}.{key}" if prefix else key
            keys.add(path)
            key_paths(item, path, keys, filled)
    elif isinstance(value, list):
        path = prefix + "[]"
        if value:
            filled.add(path)
        for item in value:
            key_paths(item, path, keys, filled)
    return keys, filled


def observed(key, filled):
    """Whether every list a key sits in held something today, so that the
    key was looked for."""
    return all(key[: match.end()] in filled for match in re.finditer(r"\[\]", key))

