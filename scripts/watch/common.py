"""What the checks of the upstream watch share.

Reading an upstream, with a few more tries for the failures that pass;
GitHub's API, which three of the checks read; the snapshots kept beside the
scripts; and the report — on the terminal, and in the run's summary when
GitHub Actions runs the check.

The standard library only, as the other scripts of the repository: python3
and the network are all a check needs, here or in CI.
"""

from __future__ import annotations

import base64
import datetime
import gzip
import http.client
import json
import os
import re
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
SNAPSHOTS = HERE / "snapshots"

# Who asks, for the services' logs: a script, once a week, and where to find
# whoever runs it.
USER_AGENT = "arr-metadata-server-upstream-watch (+https://github.com/Dim145/arr-metadata-server)"

# A check's exit status: nothing moved; something moved, or a list aged past
# what this server can live with; an upstream could not be read.
OK, MOVED, UNREADABLE = 0, 1, 2

# The statuses that say "later" rather than "no".
LATER = {429, 500, 502, 503, 504}

# In GitHub Actions, findings also become annotations on the run.
IN_ACTIONS = os.environ.get("GITHUB_ACTIONS") == "true"


class Unreadable(Exception):
    """An upstream that did not answer, or not in time, however often asked."""


class Refused(Exception):
    """An upstream that answered a status that is not "later": 404, 401, 403…"""

    def __init__(self, url, status, body, headers):
        super().__init__(f"{url}: {status}")
        self.url, self.status, self.body, self.headers = url, status, body, headers


def fetch(url, *, data=None, headers=None, method=None, timeout=60, tries=4, limit=None, kept=False):
    """The body of `url`, its first `limit` bytes when given.

    A network failure, a timeout and a "later" status are tried again, `tries`
    times in all, each wait twice the one before — or what the service's
    `Retry-After` asks, a minute at most. Any other status is `Refused`;
    failing every try is `Unreadable`. The body is asked for compressed,
    unless only its start is wanted.

    `kept` sends the request on a connection kept open for the next one to
    the same host, rather than one urllib closes after it. TMDB's reference
    needs it: to a client that asks it to close, it sends some pages with
    their body cut off — empty, every second time or so.
    """
    asked = {"User-Agent": USER_AGENT, **(headers or {})}
    if not limit:
        asked.setdefault("Accept-Encoding", "gzip")
    send = _kept if kept else _closed
    wait = 5.0
    for attempt in range(1, tries + 1):
        try:
            status, answered, body = send(url, data, asked, method, timeout, limit)
        except (OSError, EOFError, http.client.HTTPException) as error:
            detail = getattr(error, "reason", None) or error
            reason = clean(str(detail) or type(error).__name__, 120)
            pause = wait
        else:
            if status < 300:
                return body
            if status not in LATER:
                raise Refused(url, status, body[:4096], answered)
            reason = f"{status} {clean(body[:4096].decode('utf-8', 'replace'), 120)}".strip()
            pause = retry_after(answered) or wait
        if attempt == tries:
            raise Unreadable(f"{url}: {reason} ({tries} tries)")
        time.sleep(min(pause, 60))
        wait *= 2
    raise AssertionError("every try returns or raises")


def _closed(url, data, headers, method, timeout, limit):
    """One request on a connection of its own, as urllib makes them."""
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        # nosemgrep: python.lang.security.audit.dynamic-urllib-use-detected.dynamic-urllib-use-detected — the URLs are the checks' own https constants
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read(limit) if limit else response.read()
            return response.status, response.headers, _decoded(body, response.headers, limit)
    except urllib.error.HTTPError as error:
        body = error.read()
        try:
            body = _decoded(body, error.headers, limit)
        except (OSError, EOFError):
            pass
        return error.code, error.headers, body


_connections = threading.local()


def _kept(url, data, headers, method, timeout, limit):
    """One request on this thread's connection to the host, opened the first
    time and kept while the host keeps it — through the proxy the
    environment names, as urllib would have gone."""
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "https":
        return _closed(url, data, headers, method, timeout, limit)
    pool = _connections.__dict__.setdefault("pool", {})
    connection = pool.get(parts.netloc)
    if connection is None:
        connection = pool[parts.netloc] = _connect(parts.hostname, parts.port or 443, timeout)
    path = (parts.path or "/") + (f"?{parts.query}" if parts.query else "")
    try:
        connection.request(method or ("POST" if data else "GET"), path, body=data, headers=headers)
        response = connection.getresponse()
        body = response.read(limit) if limit else response.read()
    except BaseException:
        connection.close()
        pool.pop(parts.netloc, None)
        raise
    if response.will_close or limit:
        connection.close()
        pool.pop(parts.netloc, None)
    return response.status, response.headers, _decoded(body, response.headers, limit)


def _connect(host, port, timeout):
    proxy = urllib.request.getproxies().get("https")
    if not proxy or urllib.request.proxy_bypass(host):
        # nosemgrep: python.lang.security.audit.httpsconnection-detected.httpsconnection-detected — http.client verifies the certificate and the host name
        return http.client.HTTPSConnection(host, port, timeout=timeout)
    via = urllib.parse.urlsplit(proxy if "://" in proxy else f"http://{proxy}")
    # nosemgrep: python.lang.security.audit.httpsconnection-detected.httpsconnection-detected — http.client verifies the certificate and the host name
    connection = http.client.HTTPSConnection(via.hostname, via.port or 80, timeout=timeout)
    tunnel = {}
    if via.username:
        user = f"{urllib.parse.unquote(via.username)}:{urllib.parse.unquote(via.password or '')}"
        tunnel["Proxy-Authorization"] = "Basic " + base64.b64encode(user.encode()).decode()
    connection.set_tunnel(host, port, headers=tunnel)
    return connection


def _decoded(body, headers, limit):
    if not limit and (headers.get("Content-Encoding") or "").lower() == "gzip":
        return gzip.decompress(body)
    return body


def retry_after(headers):
    """The seconds a `Retry-After` header asks for, when it gives a number."""
    value = (headers or {}).get("Retry-After", "")
    return float(value) if value.strip().isdigit() else None


def github(path):
    """A document of GitHub's API.

    Signed with the token the environment holds, if any — `GITHUB_TOKEN` in
    CI, `GH_TOKEN` or `GITHUB_TOKEN` here — and sent to api.github.com and
    nowhere else. Without one, GitHub allows sixty calls an hour from an
    address, and a run of every check makes five.
    """
    headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    try:
        return json.loads(fetch("https://api.github.com" + path, headers=headers))
    except Refused as error:
        if error.status in (403, 429) and error.headers.get("X-RateLimit-Remaining") == "0":
            hint = "" if token else "; set GITHUB_TOKEN or GH_TOKEN for more"
            raise Unreadable(f"GitHub's API: its rate limit is reached{hint}") from None
        raise


def snapshot(name):
    """The snapshot of a check, as last taken; None when there is none yet."""
    try:
        return json.loads((SNAPSHOTS / f"{name}.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None


def keep(name, data):
    """Write a check's snapshot, where the next run reads it."""
    SNAPSHOTS.mkdir(exist_ok=True)
    path = SNAPSHOTS / f"{name}.json"
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return path


def shown(path):
    """A path as the repository names it."""
    try:
        return str(Path(path).resolve().relative_to(ROOT))
    except ValueError:
        return str(path)


def now():
    return datetime.datetime.now(datetime.timezone.utc)


def today():
    return now().date().isoformat()


def moment(text):
    """A time as GitHub and most APIs write it, `2026-10-27T17:04:42Z`."""
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))


def clean(text, limit=160):
    """Upstream text, made safe to print and to put in Markdown.

    One line — a line of a run's log that starts with `::` is an order to the
    runner, and upstream text never starts one of ours — with no control
    characters and no backticks, since it goes in code spans, and not too
    long.
    """
    text = re.sub(r"[\x00-\x1f\x7f\s]+", " ", str(text)).strip().replace("`", "'")
    return text if len(text) <= limit else text[: limit - 1] + "…"


def listed(names, most=6):
    """A few names, and how many more."""
    names = list(names)
    shown = ", ".join(names[:most])
    return shown + (f" and {len(names) - most} more" if len(names) > most else "")


class Report:
    """What one check found: printed as it goes, summed up at the end.

    A fact is what was read; a note, something known that still holds and is
    worth saying each week; a move, a difference from the snapshot, which
    taking the snapshot again accepts; stale, a list older than this server
    can live with, which no snapshot excuses; unreadable, an upstream that
    did not answer.
    """

    def __init__(self, name, title):
        self.name, self.title = name, title
        self.facts, self.notes, self.moves, self.stales, self.unreadables = [], [], [], [], []
        self.details = []
        print(f"watch {name}: {title}")

    def detail(self, heading, rows):
        """A table for the run's summary alone, folded: too long for the
        terminal every week, there when wanted. `rows` are lists of cells, the
        first the header."""
        self.details.append((heading, rows))

    def fact(self, text):
        self.facts.append(text)
        print(f"  {text}")

    def note(self, text):
        self.notes.append(text)
        print(f"  known: {text}")

    def move(self, text):
        self.moves.append(text)
        print(f"  MOVED: {text}")
        annotate("error", self.title, text)

    def stale(self, text):
        self.stales.append(text)
        print(f"  STALE: {text}")
        annotate("error", self.title, text)

    def unreadable(self, text):
        self.unreadables.append(text)
        print(f"  UNREADABLE: {text}")
        annotate("error", self.title, text)

    def finish(self, update, taken=None):
        """Say how it went, here and in the run's summary, and return the exit status."""
        since = f"the snapshot of {taken}" if taken else "the snapshot"
        if self.unreadables:
            verdict, status = "could not be read", UNREADABLE
        elif self.stales:
            verdict, status = "too old", MOVED
        elif self.moves and not update:
            verdict, status = f"moved since {since}", MOVED
        elif update:
            verdict, status = "snapshot taken", OK
        else:
            verdict, status = f"nothing moved since {since}", OK
        print(f"watch {self.name}: {verdict}")

        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            lines = [f"### {self.title}: {verdict}", ""]
            lines += [f"- {fact}" for fact in self.facts]
            sections = (
                ("Could not be read", self.unreadables),
                ("Too old", self.stales),
                (
                    f"Moved since {since} — look at each, then take the snapshot again with "
                    f"`python3 scripts/watch/watch.py --update {self.name}`",
                    self.moves,
                ),
                ("Known, and still so", self.notes),
            )
            for heading, items in sections:
                if items:
                    lines += ["", f"**{heading}**", ""] + [f"- {item}" for item in items]
            for heading, rows in self.details:
                table = ["| " + " | ".join(cell.replace("|", "\\|") for cell in row) + " |" for row in rows]
                table.insert(1, "|" + "---|" * len(rows[0]))
                lines += ["", f"<details><summary>{heading}</summary>", ""] + table + ["", "</details>"]
            with open(summary, "a", encoding="utf-8") as out:
                out.write("\n".join(lines) + "\n\n")
        return status


def annotate(level, title, text):
    """An annotation on the run, in GitHub Actions: the finding where the run is listed."""
    if not IN_ACTIONS:
        return

    def escape(value):
        return value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")

    title = escape(f"Upstream watch - {title}").replace(":", "%3A").replace(",", "%2C")
    print(f"::{level} title={title}::{escape(text)}")
    sys.stdout.flush()
