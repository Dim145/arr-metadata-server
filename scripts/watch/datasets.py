"""The anime list this server downloads every week, and the database it is
made from: their age, and the keys of the list's entries against the
snapshot.

The list is Fribb/anime-lists' anime-list-full.json, which maps AniList and
MyAnimeList ids to TheTVDB's and TMDB's. Fribb builds it from
cedya77/anime-offline-database, which took over from manami's
anime-offline-database when that one was archived in July 2026, and
publishes a release a week. Either standing still for more than three weeks
is the sign of a project losing its maintainer, and no snapshot excuses it.

The keys are read from the first 64 KB of the list, its first hundred and
fifty entries or so, which held every key of the whole list when this was
written. The list once changed the shape of one of them already
(`themoviedb_id`); a key added or gone is a move, and one this server reads
(src/jobs/datasets.rs) gone is the one that matters.
"""

from __future__ import annotations

import datetime
import json
import re

from common import Refused, Report, Unreadable, clean, fetch, github, keep, moment, now, shown, snapshot, today

MAX_AGE = datetime.timedelta(days=21)

FRIBB = "Fribb/anime-lists"
FRIBB_FILE = "anime-list-full.json"
# The address this server downloads by default (`AMS_ANIME_MAPPING_URL`, in
# src/config.rs).
FRIBB_URL = f"https://raw.githubusercontent.com/{FRIBB}/master/{FRIBB_FILE}"
CEDYA = "cedya77/anime-offline-database"

# What src/jobs/datasets.rs reads of an entry (`AnimeRecord`).
READ_HERE = {"type", "mal_id", "anilist_id", "tvdb_id", "season.tvdb", "episode_offset.tvdb", "themoviedb_id"}

SAMPLE = 64 * 1024
BETWEEN = re.compile(r"[\s,]*")


def run(update):
    report = Report("datasets", "The anime list and its source")
    before = snapshot("datasets") or {}
    taken = before.get("taken")
    if not before:
        report.move("there is no snapshot yet: take one.")
    found = {}

    try:
        commits = github(f"/repos/{FRIBB}/commits?path={FRIBB_FILE}&per_page=1")
        head = commits[0]
        found["fribb"] = {"commit": head["sha"], "date": head["commit"]["committer"]["date"]}
        aged(report, f"`{FRIBB}`'s `{FRIBB_FILE}`", found["fribb"]["date"], f"`{head['sha'][:8]}`")
    except (Unreadable, Refused, ValueError, KeyError, IndexError) as error:
        report.unreadable(f"`{FRIBB}` on GitHub: {clean(error)}")

    try:
        release = github(f"/repos/{CEDYA}/releases/latest")
        found["cedya77"] = {"release": release["tag_name"], "date": release["published_at"]}
        aged(report, f"`{CEDYA}`'s latest release", release["published_at"], f"`{clean(release['tag_name'], 40)}`")
        known = (before.get("cedya77") or {}).get("release")
        if known and known != release["tag_name"]:
            report.fact(f"New since the snapshot: release `{clean(release['tag_name'], 40)}` (was `{clean(known, 40)}`).")
    except (Unreadable, Refused, ValueError, KeyError) as error:
        report.unreadable(f"`{CEDYA}`'s latest release on GitHub: {clean(error)}")

    try:
        head = fetch(FRIBB_URL, headers={"Range": f"bytes=0-{SAMPLE - 1}"}, limit=SAMPLE)
        entries = first_entries(head.decode("utf-8", "replace"))
        if not entries:
            raise ValueError("no entry could be read at its start")
        keys = set()
        for entry in entries:
            keys |= key_paths(entry)
        found["keys"] = sorted(keys)
        report.fact(f"`{FRIBB_URL}` answers; {len(keys)} keys in its first {len(entries)} entries.")
        for key in sorted(READ_HERE - keys):
            report.move(f"`{key}`, which this server reads, is in none of the list's first {len(entries)} entries.")
        if "keys" in before:
            old = set(before["keys"])
            for key in sorted(keys - old):
                report.move(f"The list's entries have a new key: `{clean(key)}`.")
            for key in sorted(old - keys - READ_HERE):
                report.move(f"`{clean(key)}` is in none of the list's first {len(entries)} entries.")
    except Refused as error:
        report.stale(f"`{FRIBB_URL}`, which this server downloads, answers {error.status}.")
    except (Unreadable, ValueError) as error:
        report.unreadable(f"`{FRIBB_URL}`: {clean(error)}")

    if update and not report.unreadables:
        path = keep("datasets", {"taken": today(), **found})
        report.fact(f"Snapshot written to {shown(path)}.")
    return report.finish(update, taken)


def aged(report, what, date, which):
    age = now() - moment(date)
    text = f"{what}: {which}, {date[:10]}, {age.days} days ago."
    if age > MAX_AGE:
        report.stale(f"{text} More than {MAX_AGE.days} days.")
    else:
        report.fact(text)


def first_entries(text):
    """The entries a cut-off JSON array starts with, each one whole."""
    decoder = json.JSONDecoder()
    start = text.find("[")
    if start < 0:
        return []
    position, entries = start + 1, []
    while True:
        position = BETWEEN.match(text, position).end()
        try:
            entry, position = decoder.raw_decode(text, position)
        except ValueError:
            return entries
        if isinstance(entry, dict):
            entries.append(entry)


def key_paths(value, prefix=""):
    """The keys of an entry, at every depth: `season.tvdb`."""
    keys = set()
    if isinstance(value, dict):
        for key, item in value.items():
            path = f"{prefix}.{key}" if prefix else key
            keys.add(path)
            keys |= key_paths(item, path)
    return keys
