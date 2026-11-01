"""TheTVDB's API description, against its snapshot: the version its swagger
gives, the last commit of thetvdb/v4-api on `main`, and the pull requests
open there.

The swagger stood at 4.7.10 from May 2024 until, on 2026-10-31, a pull
request (#386) proposed 4.8.0 — a `primaryLanguage` filter for the search,
its `language` deprecated — to be merged "together with the API deploy".
This server relays TheTVDB's API and imitates parts of it: the day that
version lands, the version here moves, and so does `main`. A pull request
opened or closed is a move too, since that is where the next one shows
first.
"""

from __future__ import annotations

import re

from common import Refused, Report, Unreadable, clean, fetch, github, keep, shown, snapshot, today

REPOSITORY = "thetvdb/v4-api"
SWAGGER = f"https://raw.githubusercontent.com/{REPOSITORY}/main/docs/swagger.yml"


def run(update):
    report = Report("thetvdb", "TheTVDB's API description")
    before = snapshot("thetvdb") or {}
    taken = before.get("taken")
    if not before:
        report.move("there is no snapshot yet: take one.")
    now = {}

    try:
        version = info_version(fetch(SWAGGER).decode("utf-8", "replace"))
        if version is None:
            report.unreadable(f"`{SWAGGER}` gives no `info.version`.")
        else:
            now["version"] = version
            report.fact(f"The swagger on `main` is version `{clean(version)}`.")
            if before.get("version") and before["version"] != version:
                report.move(f"The swagger is version `{clean(version)}` now, `{before['version']}` in the snapshot.")
    except (Unreadable, Refused) as error:
        report.unreadable(f"the swagger: {clean(error)}")

    try:
        head = github(f"/repos/{REPOSITORY}/commits/main")
        now["main"] = {
            "commit": head["sha"],
            "date": head["commit"]["committer"]["date"],
            "subject": head["commit"]["message"].splitlines()[0],
        }
        report.fact(
            f"Last commit on `main`: {now['main']['date'][:10]}, `{head['sha'][:8]}` "
            f"`{clean(now['main']['subject'], 100)}`."
        )
        known = (before.get("main") or {}).get("commit")
        if known and known != head["sha"]:
            report.move(
                f"`main` moved: {now['main']['date'][:10]}, `{clean(now['main']['subject'], 100)}` "
                f"(was `{known[:8]}`)."
            )
    except (Unreadable, Refused, ValueError, KeyError, IndexError) as error:
        report.unreadable(f"the last commit on `main`: {clean(error)}")

    try:
        pulls = github(f"/repos/{REPOSITORY}/pulls?state=open&per_page=100")
        now["pulls"] = {str(p["number"]): clean(p["title"], 120) for p in pulls}
        open_now = ", ".join(f"{REPOSITORY}#{n}" for n in sorted(now["pulls"], key=int))
        report.fact(f"{len(pulls)} pull requests open: {open_now}.")
        known = before.get("pulls")
        if known is not None:
            for number in sorted(now["pulls"].keys() - known.keys(), key=int):
                report.move(f"Pull request {REPOSITORY}#{number} is open: `{now['pulls'][number]}`.")
            for number in sorted(known.keys() - now["pulls"].keys(), key=int):
                report.move(f"Pull request {REPOSITORY}#{number} is no longer open, merged or closed: `{known[number]}`.")
    except (Unreadable, Refused, ValueError, KeyError, TypeError) as error:
        report.unreadable(f"the pull requests: {clean(error)}")

    if update and not report.unreadables:
        path = keep("thetvdb", {"taken": today(), "swagger": SWAGGER, **now})
        report.fact(f"Snapshot written to {shown(path)}.")
    return report.finish(update, taken)


def info_version(text):
    """`info.version` of an OpenAPI description in YAML, read without a YAML
    library: the `version:` line directly under `info:`, not one in the
    description's text, which sits deeper."""
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if re.fullmatch(r"info:\s*", line)), None)
    if start is None:
        return None
    depth = None
    for line in lines[start + 1 :]:
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip(" "))
        if indent == 0:
            return None
        depth = depth or indent
        match = re.fullmatch(r"version:\s*(.+?)\s*", line.strip())
        if indent == depth and match:
            return match.group(1).strip("'\"")
    return None
