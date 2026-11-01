"""TMDB's API reference, against its snapshot: each page's date and the
OpenAPI definition it carries.

TMDB's reference (developer.themoviedb.org, a ReadMe site) gives each page
in Markdown too, at the page's address with `.md` added, and that version
holds what changes when the method does: a front matter whose `updatedAt`
is the page's own date, and the method's OpenAPI definition, whole, as
JSON. Both are kept for every page of the v3 and the v4 reference — some
180 pages, listed from the menu every page carries — the definition as a
fingerprint of its JSON, written again with its keys sorted, so that only
what it says counts, not how ReadMe lays it out. A page that is new or gone,
dated anew or with another definition, is a move.

What was tried and does not do: the dates the menu itself carries are one
and the same for all of v3 (2026-03-23 14:24:54 when this was written,
while the pages' own dates run from 2025-10-01 to 2025-12-12), the stamp of
something done to every page at once; the ETag changes with ReadMe's
deployments; llms.txt lists a quarter of the pages; and the reference
offers no OpenAPI download of its own.
"""

from __future__ import annotations

import hashlib
import json
import re
from concurrent.futures import ThreadPoolExecutor

from common import Refused, Report, Unreadable, clean, fetch, keep, listed, shown, snapshot, today

SITE = "https://developer.themoviedb.org"

# A page of each reference, for its menu, which lists all of that
# reference's pages; the second of each is read if the first is gone.
MENUS = {
    "reference": ["/reference/movie-details", "/reference/getting-started"],
    "v4/reference": ["/v4/reference/list-details", "/v4/reference/getting-started"],
}

FRONT_MATTER = re.compile(r"\A---\n(?P<front>.*?)\n---\n", re.S)
DEFINITION = re.compile(r"^# OpenAPI definition\s*\n+```json\n(?P<json>.*?)\n```", re.S | re.M)


def run(update):
    report = Report("tmdb", "TMDB's API reference")
    before = snapshot("tmdb") or {}
    taken = before.get("taken")
    if not before:
        report.move("there is no snapshot yet: take one.")

    paths = []
    for section, menus in MENUS.items():
        found = None
        for menu in menus:
            try:
                html = fetch(SITE + menu, headers={"Accept": "text/html"}, kept=True).decode("utf-8", "replace")
            except Refused:
                continue
            except Unreadable as error:
                report.unreadable(f"the menu of `{section}`: {clean(error)}")
                break
            found = sorted(set(re.findall(rf'href="/({re.escape(section)}/[a-z0-9-]+)"', html)))
            if found:
                break
        if not found:
            if not report.unreadables:
                report.unreadable(f"no page of `{section}` was found from {', '.join(menus)}.")
            continue
        paths += found
    if report.unreadables:
        return report.finish(update, taken)

    pages, failed = {}, []
    with ThreadPoolExecutor(max_workers=6) as pool:
        for path, result in zip(paths, pool.map(read_page, paths)):
            if isinstance(result, Exception):
                failed.append(f"`{path}` ({clean(result, 80)})")
            else:
                pages[path] = result
    v3 = sum(1 for path in pages if path.startswith("reference/"))
    report.fact(f"{v3} pages of the v3 reference and {len(pages) - v3} of the v4 one read, as Markdown.")
    if failed:
        report.unreadable(f"{len(failed)} pages could not be read: {listed(failed, 4)}.")
    if pages:
        newest = max(entry.split()[0][:10] for entry in pages.values())
        latest = sorted(path for path, entry in pages.items() if entry.startswith(newest))
        report.fact(f"Most recent page date: {newest}, on {listed((f'`{p}`' for p in latest), 4)}.")

    old = before.get("pages")
    if old is not None:
        for path in sorted(pages.keys() - old.keys()):
            report.move(f"`{path}` is a new page, dated {pages[path].split()[0][:10]}: a new method?")
        for path in sorted(old.keys() - pages.keys() - {p for p in paths if p not in pages}):
            report.move(f"`{path}` is gone from the menu.")
        for path in sorted(old.keys() & pages.keys()):
            if old[path] != pages[path]:
                report.move(f"`{path}` {what_changed(old[path], pages[path])}.")

    if update and not report.unreadables:
        path = keep("tmdb", {"taken": today(), "site": SITE, "pages": pages})
        report.fact(f"Snapshot written to {shown(path)}.")
    return report.finish(update, taken)


def read_page(path):
    """A page's date and the fingerprint of what it defines, as one string:
    `2025-10-01T20:29:44.000Z definition:3f…`. A page with no OpenAPI
    definition — the introductions — is held to its text instead."""
    try:
        text = fetch(f"{SITE}/{path}.md", headers={"Accept": "text/markdown"}, timeout=30, tries=3, kept=True)
    except (Unreadable, Refused) as error:
        return error
    text = text.decode("utf-8", "replace").replace("\r\n", "\n")
    front = FRONT_MATTER.match(text)
    if not front:
        return ValueError("no front matter")
    dated = re.search(r"^updatedAt:\s*(\S+)", front.group("front"), re.M)
    body = text[front.end() :]
    definition = DEFINITION.search(body)
    if definition:
        try:
            content = json.dumps(json.loads(definition.group("json")), sort_keys=True, separators=(",", ":"))
        except ValueError:
            content = definition.group("json")
        kind = "definition"
    else:
        content, kind = body.strip(), "text"
    digest = hashlib.sha256(content.encode()).hexdigest()[:16]
    return f"{dated.group(1) if dated else 'undated'} {kind}:{digest}"


def what_changed(old, new):
    (old_date, old_print), (new_date, new_print) = old.split(), new.split()
    said = []
    if old_date != new_date:
        said.append(f"is dated {new_date[:10]} (was {old_date[:10]})")
    if old_print != new_print:
        changed = "its OpenAPI definition" if new_print.startswith("definition:") else "its text"
        said.append(f"{changed} changed")
    return " and ".join(said)
