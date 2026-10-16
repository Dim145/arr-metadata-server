#!/usr/bin/env python3
"""The checks of scripts/e2e-stack.sh: Sonarr 4, Sonarr 5, Radarr and Plex, the
real software, driven through their own APIs against this server.

Not run by hand: the shell script starts the server and the containers, hands
this file their addresses and keys in the environment (E2E_*), and calls it
once per phase.

    e2e-stack.py PHASE [PHASE...]

Phases: sonarr4 sonarr5 radarr push seed plex. Each stands alone: what it needs
it finds in the clients' own libraries. Every check has a name; a failed one
says what it saw, and the run goes on to the next. The exit status is the
number of failed checks, capped at 100; skipped checks (with the reason) are
not failures.

Python 3.8 or later, the standard library and a `curl` on the path (the
clients' door is reached by the names the clients call it by, which curl
resolves with --resolve and Python's ssl would need a hosts file for).
"""

import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
# Plex's answers only, and none with a document type: see plex_xml.
import xml.etree.ElementTree as ET  # nosemgrep: python.lang.security.use-defused-xml.use-defused-xml

# ── What the stack is made of ──────────────────────────────────────────────

# Series, by TheTVDB id, with a term that finds each by its text. A long
# series with a DVD order and a French title; a western series with a DVD
# order that differs from the aired one (the specials move); a series still
# airing, whose seasons are named; an anime.
SERIES = {
    81189: ("Breaking Bad", "breaking bad"),
    305288: ("Stranger Things", "stranger things"),
    412806: ("More than a Married Couple, but Not Lovers", "more than a married couple"),
    71663: ("The Simpsons", "the simpsons"),
}
# Films, by TMDB id: one in a collection, one not.
FILMS = {603: ("The Matrix", "matrix"), 157336: ("Interstellar", "interstellar")}

DVD_SERIES = 81189  # added in its DVD order on Sonarr 5
FRENCH_SERIES = 71663  # added with French as its metadata language on Sonarr 5
SEASON_TITLES = 305288  # TheTVDB names its seasons
LOCKED = 81189  # the series, and film 603, a title is locked on
FRENCH = {"id": 2, "name": "French"}  # Sonarr's Language.French
ENGLISH = {"id": 1, "name": "English"}

PLEX_IDENTIFIER = "tv.plex.agents.custom.arrmetadataserver"

SONARR_ROOT = "/config/tv"
RADARR_ROOT = "/config/movies"

NO_PROXY = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def env(name, default=None):
    value = os.environ.get(name, default)
    if value is None or value == "":
        if default is None:
            sys.exit(f"{name} is not set: this file is driven by scripts/e2e-stack.sh")
    return value


# ── Checks ─────────────────────────────────────────────────────────────────


class Failure(Exception):
    """A check that did not hold: the message says what was seen."""


class Skipped(Exception):
    """A check that could not be made, and why. Not a failure."""


class Report:
    def __init__(self):
        self.failures = []
        self.skipped = []
        self.notes = []
        self.passed = 0
        self.pending = None  # the notes of the check being made

    def check(self, name):
        return Check(self, name)

    def note(self, text):
        self.notes.append(text)
        if self.pending is not None:
            self.pending.append(text)
        else:
            print(f"    note: {text}")
            sys.stdout.flush()


class Check:
    def __init__(self, report, name):
        self.report, self.name = report, name

    def __enter__(self):
        sys.stdout.write(f"  {self.name:<100} ")
        sys.stdout.flush()
        self.started = time.time()
        self.report.pending = []
        return self

    def __exit__(self, kind, error, _trace):
        took = time.time() - self.started
        if kind is None:
            print(f"ok ({took:.1f}s)")
            self.report.passed += 1
        elif issubclass(kind, Skipped):
            print(f"skipped: {error}")
            self.report.skipped.append(f"{self.name}: {error}")
        elif issubclass(kind, KeyboardInterrupt):
            return False
        else:
            if issubclass(kind, (Failure, AssertionError)):
                message = str(error)
            else:
                message = f"{kind.__name__}: {error}"
            print(f"FAILED: {message}")
            self.report.failures.append(f"{self.name}: {message}")
        for note in self.report.pending or []:
            print(f"    note: {note}")
        self.report.pending = None
        sys.stdout.flush()
        return True


def expect(condition, message):
    if not condition:
        raise Failure(message)


# ── HTTP ───────────────────────────────────────────────────────────────────


class HttpError(Exception):
    def __init__(self, status, body, url):
        super().__init__(f"HTTP {status} from {url}: {body[:300]}")
        self.status, self.body, self.url = status, body, url


def http(method, url, body=None, headers=None, timeout=180, want_headers=False, raw=False):
    """One request, no proxy. JSON bodies are sent and read as such; `raw`
    returns the bytes. An answer of 400 or more is an HttpError."""
    data = None
    headers = dict(headers or {})
    if body is not None:
        data = json.dumps(body).encode()
        headers.setdefault("Content-Type", "application/json")
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with NO_PROXY.open(request, timeout=timeout) as response:
            payload, out_headers = response.read(), response.headers
    except urllib.error.HTTPError as e:
        raise HttpError(e.code, e.read().decode("utf-8", "replace"), url) from None
    if raw:
        parsed = payload
    else:
        text = payload.decode("utf-8", "replace")
        content_type = out_headers.get("Content-Type", "")
        parsed = text
        if text.strip() and ("json" in content_type or text.lstrip()[:1] in "[{"):
            try:
                parsed = json.loads(text)
            except ValueError:
                parsed = text
    return (parsed, out_headers) if want_headers else parsed


TRANSIENT = (urllib.error.URLError, ConnectionError, TimeoutError, OSError)


class Arr:
    """Sonarr or Radarr, by its own API and its key. Both restart themselves
    once or twice after a first start, so a call that finds nobody listening
    — or a server error — is made again."""

    def __init__(self, label, url, key, version):
        self.label, self.url, self.key, self.version = label, url.rstrip("/"), key, version
        # Sonarr 5 has a v5 API and still carries the v3 one; the others v3.
        self.api = "/api/v5" if version.startswith("5.") else "/api/v3"

    def call(self, method, path, body=None, tries=40, wait=2.0):
        last = None
        for _ in range(tries):
            try:
                return http(method, self.url + path, body, {"X-Api-Key": self.key})
            except HttpError as e:
                if e.status < 500:
                    raise
                last = e
            except TRANSIENT as e:
                last = e
            time.sleep(wait)
        raise Failure(f"{self.label} did not answer {method} {path}: {last}")

    def get(self, path, **kw):
        return self.call("GET", path, **kw)

    def command(self, name, wait=240, **fields):
        """A command, waited for. Returns its final state."""
        started = self.call("POST", "/api/v3/command", {"name": name, **fields})
        for _ in range(wait):
            state = self.call("GET", f"/api/v3/command/{started['id']}")
            if state.get("status") in ("completed", "failed", "aborted", "cancelled", "orphaned"):
                return state
            time.sleep(1)
        raise Failure(f"{self.label}: {name} did not end in {wait}s")


class Native:
    """This server's own API, signed in as its administrator."""

    def __init__(self, url, user, password):
        self.url, self.user, self.password, self.cookie = url.rstrip("/"), user, password, None

    def login(self):
        request = urllib.request.Request(
            f"{self.url}/api/v1/auth/login",
            data=json.dumps({"username": self.user, "password": self.password}).encode(),
            method="POST",
            headers={"Content-Type": "application/json"},
        )
        try:
            with NO_PROXY.open(request, timeout=30) as response:
                cookie = response.headers.get("Set-Cookie", "")
        except urllib.error.HTTPError as e:
            raise Failure(
                f"the server refused the administrator's sign-in (HTTP {e.code}): a catalogue kept from "
                "a run with another password? the account is made with the catalogue"
            ) from None
        expect(cookie.startswith("ams_session="), "the server gave no session on sign-in")
        self.cookie = cookie.split(";")[0]

    def call(self, method, path, body=None, timeout=300):
        if self.cookie is None:
            self.login()
        return http(method, f"{self.url}/api/v1{path}", body, {"Cookie": self.cookie}, timeout=timeout)

    def items(self, kind):
        return self.call("GET", f"/items?kind={kind}&limit=200&includeDisabled=true")["items"]

    def item(self, kind, key, value):
        for item in self.items(kind):
            if item["externalIds"].get(key) == value:
                return item
        return None

    def lock_title(self, item_id, title):
        self.call("PUT", f"/items/{item_id}/overrides", {"field": "title", "value": title})


class Door:
    """The server's clients' door, by the names the clients call it by."""

    def __init__(self, ca, port):
        self.ca, self.port = ca, port

    def get(self, host, path, timeout=300):
        command = [
            "curl", "-sS", "-m", str(timeout), "--cacert", self.ca,
            "--resolve", f"{host}:{self.port}:127.0.0.1",
            "-H", "User-Agent: e2e-stack", "-w", "\n%{http_code}",
            f"https://{host}:{self.port}{path}",
        ]
        done = subprocess.run(command, capture_output=True, text=True)
        if done.returncode != 0:
            raise Failure(f"the clients' door did not answer {host}{path}: {done.stderr.strip()}")
        body, _, code = done.stdout.rpartition("\n")
        if code != "200":
            raise Failure(f"{host}{path}: HTTP {code} {body[:200]}")
        return json.loads(body)


# ── What the stack hands us ────────────────────────────────────────────────


class Stack:
    def __init__(self):
        self.native = Native(env("E2E_AMS_URL"), env("E2E_AMS_USER"), env("E2E_AMS_PASSWORD"))
        self.door = Door(env("E2E_DOOR_CA"), int(env("E2E_DOOR_PORT", "443")))
        self.sonarr4 = self.sonarr5 = self.radarr = None
        self.sonarr4 = self.arr("SONARR4", "sonarr-v4")
        self.sonarr5 = self.arr("SONARR5", "sonarr-v5")
        self.radarr = self.arr("RADARR", "radarr")
        listed = os.environ.get("E2E_SERIES", "").split()
        self.series = {int(i): SERIES[int(i)] for i in listed if int(i) in SERIES} or dict(SERIES)
        self.films = dict(FILMS)

    @staticmethod
    def arr(prefix, label):
        url = os.environ.get(f"E2E_{prefix}_URL")
        if not url:
            return None
        key = env(f"E2E_{prefix}_KEY")
        return Arr(label, url, key, os.environ.get(f"E2E_{prefix}_VERSION", "0"))


# ── The catalogue ──────────────────────────────────────────────────────────


def settle(native, kind, key, wanted, label):
    """Every work a client asked for is held whole. A provider that did not
    answer once — a DNS lookup that failed, a rate limit — leaves a work
    'refreshed in part', and what a client was handed then is not what the
    server serves now: refreshed until it is whole, so that the comparison
    below is between two reads of the same thing."""
    last = ""
    for _ in range(6):
        held = {i["externalIds"].get(key): i for i in native.items(kind)}
        missing = sorted(set(wanted) - set(held))
        partial = [i for w, i in held.items() if w in wanted and i.get("refreshError")]
        if not missing and not partial:
            return
        last = f"missing {missing}; partly refreshed {[(i['title'], i['refreshError']) for i in partial]}"
        for item in partial:
            try:
                native.call("POST", f"/items/{item['id']}/refresh")
            except HttpError as e:
                last += f"; refresh refused: {e}"
        time.sleep(4)
    raise Failure(f"{label}: the catalogue is not whole after six refreshes: {last}")


# ── Sonarr 4 and Sonarr 5 ──────────────────────────────────────────────────


def iso_language(sonarr_language):
    return {1: "eng", 2: "fra"}[sonarr_language["id"]]


def series_by_tvdb(arr, tvdb):
    found = [s for s in arr.get(f"{arr.api}/series") if s.get("tvdbId") == tvdb]
    return found[0] if found else None


def settle_commands(arr, names, wait=900):
    """Adding a series makes Sonarr refresh it, in a command of its own, and a
    film makes Radarr. A second refresh asked for while that one runs races
    it — Sonarr 4 listed each episode of The Simpsons twice once — so the
    add's own is waited for: none of these commands queued or started, three
    polls in a row."""
    deadline, quiet = time.time() + wait, 0
    while time.time() < deadline:
        running = [c for c in arr.get("/api/v3/command") if c.get("name") in names and c.get("status") in ("queued", "started")]
        quiet = 0 if running else quiet + 1
        if quiet >= 3:
            return
        time.sleep(2)
    raise Failure(f"{arr.label}: {' and '.join(names)} did not end in {wait}s")


SONARR_REFRESHES = ("RefreshSeries", "RescanSeries")
RADARR_REFRESHES = ("RefreshMovie", "RescanMovie")


def refresh_series(arr, series_id):
    state = arr.command("RefreshSeries", seriesId=series_id)
    expect(state.get("status") == "completed", f"RefreshSeries ended {state.get('status')}: {state.get('message')}")


def served_show(stack, arr, tvdb, language="eng", season_type="official"):
    """The series as this server hands it to this Sonarr."""
    if arr.version.startswith("5."):
        return stack.door.get("skyhook.sonarr.tv", f"/v2/tvdb/shows/{language}/{tvdb}/{season_type}")
    return stack.door.get("skyhook.sonarr.tv", f"/v1/tvdb/shows/en/{tvdb}")


def key_of(episode):
    return (episode["seasonNumber"], episode["episodeNumber"])


# What Sonarr deserializes a series into: the resources under
# NzbDrone.Core/MetadataSource/SkyHook/Resource/ of Sonarr 5.0.0.2635 (the
# ones of 4.0.20 are the same without translations, seasonTypes, originalCountry
# and a season's title). A field that is missing is a default; one that is of
# another kind than the property's throws, and fails the series' refresh.
_DAY = re.compile(r"^\d{4}-\d{2}-\d{2}$")
_MOMENT = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$")
_INT, _STR, _NUM, _LIST, _OBJ = "int", "string", "number", "list", "object"
_SHOW = {
    "tvdbId": _INT, "title": _STR, "overview": _STR, "slug": _STR, "firstAired": "day", "lastAired": "day",
    "tvRageId": _INT, "tvMazeId": _INT, "tmdbId": _INT, "malIds": _LIST, "aniListIds": _LIST, "status": _STR,
    "runtime": _INT, "timeOfDay": _OBJ, "network": _STR, "imdbId": _STR, "originalLanguage": _STR,
    "originalCountry": _STR, "actors": _LIST, "genres": _LIST, "contentRating": _STR, "rating": _OBJ,
    "images": _LIST, "seasons": _LIST, "episodes": _LIST, "translations": _LIST, "seasonTypes": _LIST,
}
_EPISODE = {
    "tvdbId": _INT, "seasonNumber": _INT, "episodeNumber": _INT, "absoluteEpisodeNumber": _INT,
    "airedAfterSeasonNumber": _INT, "airedBeforeSeasonNumber": _INT, "airedBeforeEpisodeNumber": _INT,
    "title": _STR, "airDate": "day", "airDateUtc": "moment", "runtime": _INT, "finaleType": _STR,
    "rating": _OBJ, "overview": _STR, "image": _STR,
}
_NESTED = {
    "seasons": {"seasonNumber": _INT, "title": _STR, "images": _LIST},
    "translations": {"language": _STR, "title": _STR, "overview": _STR},
    "seasonTypes": {"name": _STR, "type": _STR, "seasonNumbers": _LIST, "episodeCount": _INT},
    "images": {"coverType": _STR, "url": _STR},
    "actors": {"name": _STR, "character": _STR, "image": _STR},
    "rating": {"count": _INT, "value": _NUM},
    "timeOfDay": {"hours": _INT, "minutes": _INT},
}
_SHOW_NEEDED = ("tvdbId", "title", "status", "seasons", "episodes", "images", "genres")
_SHOW_NEEDED_5 = ("translations", "seasonTypes", "originalLanguage")


def _kind_fits(value, kind):
    """Sonarr reads a number from a number or from a string that is one: the
    real service writes a rating's value as a string ("9.5") and Sonarr reads
    it as the decimal it is."""
    if value is None:
        return True
    if kind == _INT:
        if isinstance(value, str):
            return bool(re.match(r"^-?\d+$", value))
        return isinstance(value, int) and not isinstance(value, bool)
    if kind == _NUM:
        if isinstance(value, str):
            return bool(re.match(r"^-?\d+(\.\d+)?$", value))
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if kind == _STR:
        return isinstance(value, str)
    if kind == _LIST:
        return isinstance(value, list)
    if kind == _OBJ:
        return isinstance(value, dict)
    if kind == "day":
        return isinstance(value, str) and bool(_DAY.match(value))
    if kind == "moment":
        return isinstance(value, str) and bool(_MOMENT.match(value))
    return True


def contract_problems(show, five):
    """What of a served series Sonarr cannot read: a missing field it needs, a
    field of another kind, in the series, its episodes and what they hold."""
    problems = []

    def check(node, fields, where):
        for name, kind in fields.items():
            if name in node and not _kind_fits(node[name], kind):
                problems.append(f"{where}.{name} is {type(node[name]).__name__} {str(node[name])[:40]!r}, a {kind} is read")
        for name, spec in _NESTED.items():
            value = node.get(name)
            if isinstance(value, dict):
                check(value, spec, f"{where}.{name}")
            elif isinstance(value, list):
                for i, entry in enumerate(value[:50]):
                    if isinstance(entry, dict):
                        check(entry, spec, f"{where}.{name}[{i}]")
                    elif name in ("seasons", "translations", "seasonTypes", "images", "actors"):
                        problems.append(f"{where}.{name}[{i}] is not an object")

    for name in _SHOW_NEEDED + (_SHOW_NEEDED_5 if five else ()):
        if show.get(name) in (None, ""):
            problems.append(f"the series has no {name}")
    check(show, _SHOW, "show")
    for i, episode in enumerate(show.get("episodes") or []):
        check(episode, _EPISODE, f"episodes[{i}]")
        for name in ("seasonNumber", "episodeNumber", "title"):
            if episode.get(name) in (None, ""):
                problems.append(f"episodes[{i}] has no {name}")
        if len(problems) > 12:
            break
    seen = set()
    for season in show.get("seasons") or []:
        if season.get("seasonNumber") in seen:
            problems.append(f"season {season.get('seasonNumber')} is listed twice")
        seen.add(season.get("seasonNumber"))
    if five:
        original = show.get("originalLanguage")
        titles = {t.get("language"): t.get("title") for t in show.get("translations") or []}
        if original and original not in titles:
            problems.append(f"no translation in the original language {original!r}, where Sonarr reads the original title")
    return problems[:12]


def add_series(stack, arr, tvdb, language=ENGLISH, season_type="official"):
    """Look a series up and add it, unmonitored, with nothing searched for."""
    found = arr.get(f"{arr.api}/series/lookup?term=tvdb:{tvdb}")
    lookup = [s for s in found if s.get("tvdbId") == tvdb]
    expect(lookup, f"tvdb:{tvdb} was not found")
    body = lookup[0]
    body.update(
        qualityProfileId=1, rootFolderPath=SONARR_ROOT, monitored=False, seasonFolder=True,
        addOptions={"monitor": "none", "searchForMissingEpisodes": False, "searchForCutoffUnmetEpisodes": False},
    )
    if arr.version.startswith("5."):
        body.update(language=language, seasonType=season_type)
    existing = series_by_tvdb(arr, tvdb)
    if existing:
        return existing["id"]
    try:
        added = arr.call("POST", f"{arr.api}/series", body)
    except HttpError as e:
        raise Failure(f"Sonarr refused the add of tvdb:{tvdb}: {e}") from None
    return added["id"]


def scene_additions(stack, wanted):
    """The mappings this server adds to Sonarr's own list for the series given:
    what a release named that way must be taken for."""
    listed = stack.door.get("services.sonarr.tv", "/v1/scenemapping")
    return [m for m in listed if m.get("comment") == "arr-metadata-server" and m.get("tvdbId") in wanted]


def phase_sonarr(R, stack, arr):
    label, v5 = arr.label, arr.version.startswith("5.")
    native = stack.native
    wanted = list(stack.series)

    with R.check(f"{label}: it answers, and is the version the stack was asked for"):
        status = arr.get("/api/v3/system/status")
        expect(status["version"].startswith(arr.version[:2]), f"{status['version']} is not a {arr.version[:2]}x")
        R.note(f"{label} {status['version']} ({status.get('packageVersion', '')}), authentication {status.get('authentication')}")

    with R.check(f"{label}: a root folder"):
        folders = arr.get(f"{arr.api}/rootfolder")
        if not any(f["path"] == SONARR_ROOT for f in folders):
            arr.call("POST", f"{arr.api}/rootfolder", {"path": SONARR_ROOT})

    # ── The search ─────────────────────────────────────────────────────────
    for tvdb, (title, term) in stack.series.items():
        with R.check(f"{label}: a search by text finds {title} (tvdb:{tvdb})"):
            found = arr.get(f"{arr.api}/series/lookup?term={urllib.parse.quote(term)}")
            expect(any(s.get("tvdbId") == tvdb for s in found), f"not among {[(s['title'], s['tvdbId']) for s in found[:6]]}")
        with R.check(f"{label}: a search by tvdb:{tvdb}"):
            found = arr.get(f"{arr.api}/series/lookup?term=tvdb:{tvdb}")
            expect(len(found) == 1 and found[0]["tvdbId"] == tvdb, f"answered {[(s['title'], s['tvdbId']) for s in found[:4]]}")
    first = next(iter(stack.series))
    for kind, value in (("tmdb", None), ("imdb", None)):
        with R.check(f"{label}: a search by {kind}: finds the series"):
            item = native.item("series", "tvdb", first)
            expect(item is not None, f"tvdb:{first} is not in the catalogue after its lookups")
            value = item["externalIds"].get(kind)
            if not value:
                raise Skipped(f"the catalogue holds no {kind} id for tvdb:{first}")
            found = arr.get(f"{arr.api}/series/lookup?term={kind}:{value}")
            expect(any(s.get("tvdbId") == first for s in found), f"{kind}:{value} found {[(s['title'], s['tvdbId']) for s in found[:4]]}")

    anime = 412806
    for kind, field in (("mal", "malIds"), ("anilist", "aniListIds")):
        with R.check(f"{label}: a search by {kind}: finds the anime"):
            if anime not in stack.series:
                raise Skipped("the anime is not among the series of this run")
            held = served_show(stack, arr, anime).get(field) or []
            if not held:
                raise Skipped(f"the server serves no {kind} id for tvdb:{anime}")
            found = arr.get(f"{arr.api}/series/lookup?term={kind}:{held[0]}")
            expect(any(s.get("tvdbId") == anime for s in found), f"{kind}:{held[0]} found {[(s['title'], s['tvdbId']) for s in found[:4]]}")

    with R.check(f"{label}: the catalogue is whole before the add"):
        settle(native, "series", "tvdb", wanted, label)

    # ── The add: seasons and episodes as served ────────────────────────────
    options = {}
    if v5:
        options[FRENCH_SERIES] = dict(language=FRENCH)
        options[DVD_SERIES] = dict(season_type="dvd")
    ids = {}
    for tvdb, (title, _) in stack.series.items():
        with R.check(f"{label}: {title} (tvdb:{tvdb}) is added" + (f" {options[tvdb]}" if tvdb in options else "")):
            ids[tvdb] = add_series(stack, arr, tvdb, **options.get(tvdb, {}))
            settle_commands(arr, SONARR_REFRESHES)
    for tvdb, (title, _) in stack.series.items():
        if tvdb not in ids:
            continue
        with R.check(f"{label}: {title}: as many seasons and episodes as the server serves"):
            held = arr.get(f"{arr.api}/series/{ids[tvdb]}")
            episodes = arr.get(f"{arr.api}/episode?seriesId={ids[tvdb]}")
            asked = dict(options.get(tvdb, {}))
            language = iso_language(asked.get("language", ENGLISH))
            served = served_show(stack, arr, tvdb, language, asked.get("season_type", "official"))
            served_keys = [key_of(e) for e in served["episodes"]]
            expect(len(set(served_keys)) == len(served_keys), f"the server serves {len(served_keys) - len(set(served_keys))} episodes twice")
            seasons_here = sorted(s["seasonNumber"] for s in held["seasons"])
            seasons_served = sorted(s["seasonNumber"] for s in served["seasons"])
            expect(seasons_here == seasons_served, f"seasons: Sonarr {seasons_here}, served {seasons_served}")
            expect(
                len(episodes) == len(served_keys),
                f"episodes: Sonarr {len(episodes)}, served {len(served_keys)}; "
                f"Sonarr lacks {sorted(set(served_keys) - {key_of(e) for e in episodes})[:5]}, "
                f"has extra {sorted({key_of(e) for e in episodes} - set(served_keys))[:5]}",
            )
            wrong = [
                (key_of(e), e["title"], t["title"])
                for e in episodes
                for t in [next(x for x in served["episodes"] if key_of(x) == key_of(e))]
                if (e["title"] or "") != (t["title"] or "")
            ]
            expect(not wrong, f"{len(wrong)} episode titles differ from the served ones, first {wrong[:3]}")

    # ── Every field Sonarr reads, of the kind it reads it as ───────────────
    for tvdb, (title, _) in stack.series.items():
        with R.check(f"{label}: {title} is served with every field Sonarr {arr.version[:1]} reads, as it reads it"):
            asked = dict(options.get(tvdb, {}))
            served = served_show(stack, arr, tvdb, iso_language(asked.get("language", ENGLISH)), asked.get("season_type", "official"))
            problems = contract_problems(served, v5)
            expect(not problems, "; ".join(problems))

    # ── What only Sonarr 5 reads ───────────────────────────────────────────
    if v5:
        phase_sonarr5_reads(R, stack, arr, ids)

    # ── A lock made here appears there after a refresh ─────────────────────
    if LOCKED in ids:
        with R.check(f"{label}: a title locked in the catalogue appears in Sonarr after its refresh"):
            item = native.item("series", "tvdb", LOCKED)
            expect(item is not None, f"tvdb:{LOCKED} is not in the catalogue")
            text = f"Locked by the e2e stack ({label})"
            native.lock_title(item["id"], text)
            refresh_series(arr, ids[LOCKED])
            held = arr.get(f"{arr.api}/series/{ids[LOCKED]}")
            expect(held["title"] == text, f"Sonarr's title is {held['title']!r}, not {text!r}")

    # ── Scene mappings: Sonarr's own list, this catalogue's titles added ───
    phase_scene(R, stack, arr, ids)


def phase_sonarr5_reads(R, stack, arr, ids):
    label = arr.label
    if FRENCH_SERIES in ids:
        tvdb = FRENCH_SERIES
        with R.check(f"{label}: French as the metadata language: the series and its episodes are in French"):
            held = arr.get(f"{arr.api}/series/{ids[tvdb]}")
            french = served_show(stack, arr, tvdb, "fra")
            english = served_show(stack, arr, tvdb, "eng")
            expect(held["language"]["id"] == FRENCH["id"], f"the series' language is {held['language']}")
            expect(held["title"] == french["title"], f"title {held['title']!r}, served in French {french['title']!r}")
            expect(held["title"] != english["title"], f"the French title {held['title']!r} is the English one")
            expect(
                held.get("overview") == french.get("overview") and held.get("overview") != english.get("overview"),
                "the overview is not the French one",
            )
            episodes = arr.get(f"{arr.api}/episode?seriesId={ids[tvdb]}")
            english_titles = {key_of(e): e["title"] for e in english["episodes"]}
            translated = sum(1 for e in episodes if e["title"] != english_titles.get(key_of(e)))
            expect(
                translated >= 0.2 * len(episodes),
                f"only {translated} of {len(episodes)} episode titles differ from the English ones",
            )
            R.note(f"{label}: {held['title']!r}, {translated} of {len(episodes)} episode titles in French")
        with R.check(f"{label}: a search in French gives the series its French title, by id and by its French name"):
            french = served_show(stack, arr, tvdb, "fra")
            by_id = arr.get(f"{arr.api}/series/lookup?term=tvdb:{tvdb}&language={FRENCH['id']}")
            expect(by_id and by_id[0]["title"] == french["title"], f"by id: {[s['title'] for s in by_id[:3]]}, served {french['title']!r}")
            name = urllib.parse.quote(french["title"].lower())
            by_name = [s for s in arr.get(f"{arr.api}/series/lookup?term={name}&language={FRENCH['id']}") if s.get("tvdbId") == tvdb]
            expect(by_name and by_name[0]["title"] == french["title"], f"by name {french['title']!r}: {[s['title'] for s in by_name[:3]]}")
        with R.check(f"{label}: the series is also named in the other languages it is held in"):
            held = arr.get(f"{arr.api}/series/{ids[FRENCH_SERIES]}")
            languages = {t["language"]["id"] for t in held.get("translations", [])}
            expect(len(languages) >= 3, f"Sonarr kept {len(languages)} translations")
            expect(ENGLISH["id"] in languages and FRENCH["id"] in languages, f"translations in {sorted(languages)}")
    if DVD_SERIES in ids:
        tvdb = DVD_SERIES
        with R.check(f"{label}: the DVD order chosen at the add numbers the episodes"):
            held = arr.get(f"{arr.api}/series/{ids[tvdb]}")
            expect(held.get("seasonType") == "dvd", f"the series' season type is {held.get('seasonType')!r}")
            dvd = served_show(stack, arr, tvdb, "eng", "dvd")
            aired = served_show(stack, arr, tvdb, "eng", "official")
            episodes = arr.get(f"{arr.api}/episode?seriesId={ids[tvdb]}")
            expect(
                {key_of(e) for e in episodes} == {key_of(e) for e in dvd["episodes"]},
                f"Sonarr's numbering is not the DVD one: {len(episodes)} episodes, served {len(dvd['episodes'])}",
            )
            expect(
                {key_of(e) for e in dvd["episodes"]} != {key_of(e) for e in aired["episodes"]},
                "the DVD order is the aired one: the choice proves nothing",
            )
            types = [t["type"] for t in held.get("seasonTypes", [])]
            expect("official" in types and "dvd" in types, f"the orders Sonarr was offered are {types}")
            R.note(f"{label}: {len(dvd['episodes'])} episodes in the DVD order, {len(aired['episodes'])} aired")
        with R.check(f"{label}: the DVD order is the real service's, to the count of episodes"):
            try:
                real = http("GET", f"https://skyhook.sonarr.tv/v2/tvdb/shows/eng/{tvdb}/dvd", timeout=60)
            except (HttpError,) + TRANSIENT as e:
                raise Skipped(f"the real service did not answer: {e}")
            ours = served_show(stack, arr, tvdb, "eng", "dvd")
            expect(
                len(real["episodes"]) == len(ours["episodes"]),
                f"the real service has {len(real['episodes'])} episodes in the DVD order, this server {len(ours['episodes'])}",
            )
    if SEASON_TITLES in ids:
        tvdb = SEASON_TITLES
        with R.check(f"{label}: the seasons' own titles reach Sonarr"):
            held = arr.get(f"{arr.api}/series/{ids[tvdb]}")
            served = served_show(stack, arr, tvdb)
            titles_served = {s["seasonNumber"]: s.get("title") for s in served["seasons"] if s.get("title")}
            expect(len(titles_served) >= 3, f"the server names only {len(titles_served)} seasons")
            titles_here = {s["seasonNumber"]: s.get("title") for s in held["seasons"] if s.get("title")}
            expect(titles_here == titles_served, f"Sonarr's season titles {titles_here}, served {titles_served}")


def phase_scene(R, stack, arr, ids):
    """Sonarr takes a release for a series by the titles of its scene-mapping
    list; this server adds the catalogue's. A Sonarr 5 skips a mapping that
    has no `mappingId`."""
    label = arr.label
    with R.check(f"{label}: Update Scene Mapping takes this catalogue's titles, and a release named by one finds its series"):
        added = scene_additions(stack, set(ids))
        if not added:
            raise Skipped("this catalogue adds no mapping for the series of the stack")
        began = int(time.time()) - 1
        state = arr.command("UpdateSceneMapping")
        expect(state.get("status") == "completed", f"Update Scene Mapping ended {state.get('status')}")
        stack.scene_began = began
        misses = []
        for mapping in added[:6]:
            release = re.sub(r"[\W_]+", ".", mapping["title"]).strip(".") + ".S01E01.1080p.WEB.x264-GRP"
            parsed = arr.get(f"/api/v3/parse?title={urllib.parse.quote(release)}")
            found = (parsed.get("series") or {}).get("tvdbId")
            if found != mapping["tvdbId"]:
                misses.append((mapping["title"], mapping["tvdbId"], found))
        expect(not misses, f"{len(misses)} of {min(len(added), 6)} releases are not taken for their series: {misses[:3]}")
        R.note(f"{label}: {len(added)} mappings added by this catalogue, tried {min(len(added), 6)}")
    if arr.version.startswith("5."):
        with R.check(f"{label}: no scene mapping is skipped for want of a mappingId"):
            # Only what Sonarr logged since the command: the log of a stack kept
            # from an earlier run holds that run's.
            since = str(getattr(stack, "scene_began", 0))
            log = subprocess.run(["docker", "logs", "--since", since, "ams-e2e-sonarr5"], capture_output=True, text=True)
            lines = [l for l in (log.stdout + log.stderr).splitlines() if "missing MappingId" in l]
            expect(not lines, f"{len(lines)} mappings skipped, first: {lines[0][:200] if lines else ''}")


# ── Radarr ─────────────────────────────────────────────────────────────────


def phase_radarr(R, stack, arr):
    label, native = arr.label, stack.native
    with R.check(f"{label}: it answers"):
        status = arr.get("/api/v3/system/status")
        R.note(f"{label} {status['version']}")
    with R.check(f"{label}: a root folder"):
        if not any(f["path"] == RADARR_ROOT for f in arr.get("/api/v3/rootfolder")):
            arr.call("POST", "/api/v3/rootfolder", {"path": RADARR_ROOT})
    for tmdb, (title, term) in stack.films.items():
        with R.check(f"{label}: a search by text finds {title} (tmdb:{tmdb})"):
            found = arr.get(f"/api/v3/movie/lookup?term={urllib.parse.quote(term)}")
            expect(any(m.get("tmdbId") == tmdb for m in found), f"not among {[(m['title'], m['tmdbId']) for m in found[:6]]}")
        with R.check(f"{label}: a search by tmdb:{tmdb}"):
            found = arr.get(f"/api/v3/movie/lookup?term=tmdb:{tmdb}")
            expect(found and found[0]["tmdbId"] == tmdb, f"answered {[(m['title'], m['tmdbId']) for m in found[:4]]}")
    with R.check(f"{label}: the catalogue is whole before the add"):
        settle(native, "movie", "tmdb", list(stack.films), label)

    held = {}
    for tmdb, (title, _) in stack.films.items():
        with R.check(f"{label}: {title} (tmdb:{tmdb}) is added"):
            existing = [m for m in arr.get("/api/v3/movie") if m.get("tmdbId") == tmdb]
            if existing:
                held[tmdb] = existing[0]["id"]
                continue
            body = [m for m in arr.get(f"/api/v3/movie/lookup?term=tmdb:{tmdb}") if m.get("tmdbId") == tmdb][0]
            body.update(
                qualityProfileId=1, rootFolderPath=RADARR_ROOT, monitored=False, minimumAvailability="released",
                addOptions={"monitor": "none", "searchForMovie": False},
            )
            try:
                held[tmdb] = arr.call("POST", "/api/v3/movie", body)["id"]
            except HttpError as e:
                raise Failure(f"Radarr refused the add of tmdb:{tmdb}: {e}") from None
    for tmdb, (title, _) in stack.films.items():
        if tmdb not in held:
            continue
        with R.check(f"{label}: {title} is refreshed, and is what the server serves"):
            settle_commands(arr, RADARR_REFRESHES)
            state = arr.command("RefreshMovie", movieIds=[held[tmdb]])
            expect(state.get("status") == "completed", f"RefreshMovie ended {state.get('status')}: {state.get('message')}")
            movie = arr.get(f"/api/v3/movie/{held[tmdb]}")
            served = stack.door.get("api.radarr.video", f"/v1/movie/{tmdb}")
            expect(movie["title"] == served["Title"], f"title {movie['title']!r}, served {served['Title']!r}")
            expect(movie["year"] == served["Year"], f"year {movie['year']}, served {served['Year']}")
            expect(movie["runtime"] == served["Runtime"], f"runtime {movie['runtime']}, served {served['Runtime']}")
            expect((movie.get("studio") or "") == (served.get("Studio") or ""), f"studio {movie.get('studio')!r}, served {served.get('Studio')!r}")
            expect(movie.get("imdbId") == served.get("ImdbId"), f"imdb id {movie.get('imdbId')}, served {served.get('ImdbId')}")
        with R.check(f"{label}: {title}: its classification is the server's, for Radarr's country"):
            movie = arr.get(f"/api/v3/movie/{held[tmdb]}")
            served = stack.door.get("api.radarr.video", f"/v1/movie/{tmdb}")
            wanted = next((c["Certification"] for c in served["Certifications"] if c["Country"] == "US"), None)
            expect(wanted, "the server serves no classification for the United States")
            expect(movie.get("certification") == wanted, f"certification {movie.get('certification')!r}, served {wanted!r}")
    if 603 in held:
        with R.check(f"{label}: The Matrix is in its collection, which Radarr lists"):
            movie = arr.get(f"/api/v3/movie/{held[603]}")
            served = stack.door.get("api.radarr.video", "/v1/movie/603")
            collection = movie.get("collection") or {}
            expect(collection.get("tmdbId") == served["Collection"]["TmdbId"], f"collection {collection}")
            expect(collection.get("title") == served["Collection"]["Name"], f"collection title {collection.get('title')!r}")
            listed = [c for c in arr.get("/api/v3/collection") if c.get("tmdbId") == collection["tmdbId"]]
            expect(listed, "the collections Radarr lists lack it")
            expect(len(listed[0].get("movies", [])) >= 2, f"the collection lists {len(listed[0].get('movies', []))} films")
    with R.check(f"{label}: Interstellar belongs to no collection"):
        if 157336 not in held:
            raise Skipped("not added")
        movie = arr.get(f"/api/v3/movie/{held[157336]}")
        served = stack.door.get("api.radarr.video", "/v1/movie/157336")
        expect(bool(movie.get("collection")) == bool((served.get("Collection") or {}).get("TmdbId")), f"collection {movie.get('collection')}, served {served.get('Collection')}")
    with R.check(f"{label}: recommendations, drawn from the films' own, are listed"):
        served = stack.door.get("api.radarr.video", "/v1/movie/603")
        expect(served["Recommendations"], "the server serves no recommendations for The Matrix")
        recommended = arr.get("/api/v3/importlist/movie?includeRecommendations=true", tries=3)
        listed = [m for m in recommended if m.get("isRecommendation")]
        expect(listed, f"Radarr lists {len(recommended)} films, none a recommendation")
        known = {r["TmdbId"] for r in served["Recommendations"]}
        expect(any(m["tmdbId"] in known for m in listed), "none of the films Radarr recommends is one the server recommends")
        R.note(f"{label}: {len(listed)} recommendations")
    with R.check(f"{label}: a title locked in the catalogue appears in Radarr after its refresh"):
        if 603 not in held:
            raise Skipped("The Matrix is not added")
        item = native.item("movie", "tmdb", 603)
        expect(item is not None, "tmdb:603 is not in the catalogue")
        text = "Locked by the e2e stack (radarr)"
        native.lock_title(item["id"], text)
        state = arr.command("RefreshMovie", movieIds=[held[603]])
        expect(state.get("status") == "completed", f"RefreshMovie ended {state.get('status')}")
        movie = arr.get(f"/api/v3/movie/{held[603]}")
        expect(movie["title"] == text, f"Radarr's title is {movie['title']!r}, not {text!r}")
    with R.check(f"{label}: Radarr refused no certificate"):
        log = subprocess.run(["docker", "logs", "ams-e2e-radarr"], capture_output=True, text=True)
        lines = [l for l in (log.stdout + log.stderr).splitlines() if "RemoteCertificate" in l]
        expect(not lines, f"{len(lines)} refused certificates in Radarr's log: {lines[0][:160] if lines else ''}")


# ── The other way: the server asks Sonarr and Radarr ───────────────────────


def connect(native, sonarr, radarr, push):
    body = {"pushRefresh": push}
    for prefix, arr in (("sonarr", sonarr), ("radarr", radarr)):
        if arr is not None:
            body[f"{prefix}Url"] = arr.url
            body[f"{prefix}Key"] = arr.key
    return native.call("PUT", "/admin/arr", body)


def newest_command(arr, name, since_ids):
    for command in arr.get("/api/v3/command"):
        if command.get("name") == name and command["id"] not in since_ids:
            return command
    return None


def phase_push(R, stack):
    native = stack.native
    pairs = []
    for arr in (stack.sonarr4, stack.sonarr5):
        if arr is not None:
            pairs.append((arr, "RefreshSeries", "seriesIds", "series", LOCKED))
    if stack.radarr is not None:
        pairs.append((stack.radarr, "RefreshMovie", "movieIds", "movie", 603))
    for arr, name, ids_field, kind, work in pairs:
        label = arr.label
        is_sonarr = kind == "series"
        with R.check(f"push: {label} is connected, and the server tests it"):
            sonarr = arr if is_sonarr else None
            radarr = arr if not is_sonarr else None
            connect(native, sonarr, radarr, push=True)
            tested = native.call("POST", "/admin/arr/test", {"app": "sonarr" if is_sonarr else "radarr"})
            expect(tested.get("ok"), f"the test answered {tested}")
        with R.check(f"push: a lock put in the catalogue sends {name} to {label}"):
            find = series_by_tvdb(arr, work) if is_sonarr else next((m for m in arr.get("/api/v3/movie") if m.get("tmdbId") == work), None)
            if find is None:
                raise Skipped(f"{label} does not hold {kind} {work}: run its own phase first")
            item = native.item("series" if is_sonarr else "movie", "tvdb" if is_sonarr else "tmdb", work)
            expect(item is not None, f"{kind} {work} is not in the catalogue")
            before = {c["id"] for c in arr.get("/api/v3/command")}
            text = f"Pushed to {label} at {int(time.time())}"
            native.lock_title(item["id"], text)
            found = None
            # Ten seconds after the last change, or a minute after the first.
            for _ in range(100):
                command = newest_command(arr, name, before)
                if command and find["id"] in (command.get("body") or {}).get(ids_field, []):
                    found = command
                    break
                time.sleep(2)
            expect(found, f"{label} was sent no {name} for its {kind} {find['id']} in 200 s")
            for _ in range(120):
                state = arr.get(f"/api/v3/command/{found['id']}")
                if state.get("status") in ("completed", "failed", "aborted"):
                    break
                time.sleep(1)
            expect(state.get("status") == "completed", f"the command ended {state.get('status')}")
            path = f"{arr.api}/series/{find['id']}" if is_sonarr else f"/api/v3/movie/{find['id']}"
            title = arr.get(path)["title"]
            expect(title == text, f"{label} holds the title {title!r}, not the lock {text!r}")
        with R.check(f"push: the server's own record of it, for {label}"):
            app = "sonarr" if is_sonarr else "radarr"
            sent = [p for p in native.call("GET", "/admin/arr").get("pushes", []) if p.get("app") == app and p.get("outcome") == "sent"]
            expect(sent, f"the page lists no {app} work sent")
            expect("lock" in sent[0].get("reasons", []), f"the latest send was for {sent[0].get('reasons')}, not for a lock")
            R.note(f"push: {label}: {sent[0].get('title')!r} sent as {sent[0].get('detail')!r}")


def phase_seed(R, stack):
    """`arr.seed` into a catalogue of its own: the stack's libraries are what
    a deployment's Sonarr and Radarr hold before they are pointed here."""
    native = stack.native
    with R.check("seed: the catalogue begins empty of what the libraries hold"):
        held = [i for i in native.items("series") if i["externalIds"].get("tvdb") in stack.series]
        held += [i for i in native.items("movie") if i["externalIds"].get("tmdb") in stack.films]
        expect(not held, f"the catalogue already holds {[i['title'] for i in held]}: the server was not restarted on a fresh database")
    sources = [(stack.sonarr5 or stack.sonarr4), stack.radarr]
    with R.check("seed: Sonarr and Radarr are connected"):
        connect(native, sources[0], stack.radarr, push=False)
    expected_series = [t for t in stack.series if series_by_tvdb(sources[0], t)] if sources[0] else []
    expected_films = [m for m in stack.films if stack.radarr and any(x.get("tmdbId") == m for x in stack.radarr.get("/api/v3/movie"))]
    with R.check(f"seed: Seed from Sonarr and Radarr brings {len(expected_series)} series and {len(expected_films)} films"):
        expect(expected_series or expected_films, "the libraries hold none of the stack's works: run the sonarr and radarr phases first")
        started = native.call("POST", "/tasks/arr.seed/run")
        job = started["jobId"]
        expect(job, "the task started no run")
        run = None
        for _ in range(900):
            run = native.call("GET", f"/jobs/{job}")["job"]
            if run["status"] != "running":
                break
            time.sleep(2)
        expect(run["status"] == "succeeded", f"the run {run['status']}: {run.get('error')}")
        R.note(f"seed: {run.get('detail')}")
        missing = [t for t in expected_series if native.item("series", "tvdb", t) is None]
        missing += [m for m in expected_films if native.item("movie", "tmdb", m) is None]
        expect(not missing, f"not in the catalogue after the run: {missing}")
    with R.check("seed: a second run finds everything held, and fetches nothing"):
        run = None
        job = native.call("POST", "/tasks/arr.seed/run")["jobId"]
        for _ in range(900):
            run = native.call("GET", f"/jobs/{job}")["job"]
            if run["status"] != "running":
                break
            time.sleep(2)
        expect(run["status"] == "succeeded", f"the run {run['status']}: {run.get('error')}")
        detail = run.get("detail") or ""
        expect(re.search(r"\b0 imported\b", detail), f"the second run says: {detail}")


# ── Plex ───────────────────────────────────────────────────────────────────


def plex_xml(out):
    """An answer of Plex's, read as XML when it is XML. A document type — the
    way in for entities, which no answer of Plex's carries — is refused
    rather than read."""
    if out.strip()[:1] != b"<":
        return out
    if b"<!DOCTYPE" in out or b"<!ENTITY" in out:
        raise RuntimeError("Plex answered with a document type, which is not read")
    return ET.fromstring(out)


def plex_get(base, path, headers=None, timeout=60):
    """Plex answers XML unless asked otherwise."""
    return plex_xml(http("GET", base + path, headers=headers, timeout=timeout, raw=True))


def plex_post(base, path, params, timeout=120):
    query = urllib.parse.urlencode(params, quote_via=urllib.parse.quote)
    return plex_xml(http("POST", f"{base}{path}?{query}", timeout=timeout, raw=True))


def eventually(fetch, done, wait=90, every=3):
    """Plex fills an item in stages, a moment after the scan says it is over:
    read again until what is looked at is all there, or the time is up, and
    hand back the last read either way."""
    deadline = time.time() + wait
    while True:
        value = fetch()
        if done(value) or time.time() > deadline:
            return value
        time.sleep(every)


def plex_wait_scan(base, expect_sections, wait=240):
    last = None
    for _ in range(wait // 2):
        root = plex_get(base, "/library/sections")
        sections = root.findall("Directory")
        if len(sections) >= expect_sections and not any(s.get("refreshing") == "1" for s in sections):
            return sections
        last = [(s.get("title"), s.get("refreshing")) for s in sections]
        time.sleep(2)
    raise Failure(f"Plex was still scanning after {wait}s: {last}")


def phase_plex(R, stack):
    base = env("E2E_PLEX_URL").rstrip("/")
    provider = env("E2E_PLEX_PROVIDER")
    native = stack.native
    with R.check("plex: the server answers /identity, in a version that takes custom providers (1.43 or later)"):
        root = None
        for _ in range(60):
            try:
                root = plex_get(base, "/identity")
                if root.get("startState") in (None, "started"):
                    break
            except TRANSIENT + (HttpError,):
                pass
            time.sleep(3)
        expect(root is not None, "Plex did not answer")
        version = tuple(int(p) for p in re.findall(r"\d+", root.get("version", "0"))[:2])
        expect(version >= (1, 43), f"Plex {root.get('version')} is older than 1.43")
        R.note(f"plex {root.get('version')}, claimed={root.get('claimed')}")
        claimed = root.get("claimed") == "1"
    refused = []
    with R.check("plex: its API takes the stack's calls without any token"):
        try:
            http("GET", base + "/library/sections", timeout=30)
        except HttpError as e:
            if e.status in (401, 403):
                refused.append(e.status)
                raise Skipped(
                    "Plex refuses a call without a token"
                    + (" — this server is claimed" if claimed else "")
                    + ". Allow the Docker network without authorization: Settings > Network > List of IP addresses and "
                    "networks that are allowed without auth (the gateway's address, e.g. 172.18.0.0/16), or start the "
                    "stack with PLEX_ALLOWED_NETWORKS=172.16.0.0/12,192.168.0.0/16 on a new volume; then run PHASES=plex again. "
                    "No Plex token is ever used here."
                )
            raise
    if refused:
        # Nothing else can be asked of it without a token, and none is ever used.
        return
    served_before = next(a["served"] for a in native.call("GET", "/admin/access")["apis"] if a["api"] == "plex")

    with R.check("plex: the provider is added, and Plex finds it answering, for films and series"):
        known = [p for p in plex_get(base, "/media/providers/metadata").findall("MetadataAgentProvider") if p.get("identifier") == PLEX_IDENTIFIER]
        node = known[0] if known else plex_post(base, "/media/providers/metadata", {"uri": provider}).find("MetadataAgentProvider")
        expect(node is not None, "Plex took no provider from the address")
        expect(node.get("identifier") == PLEX_IDENTIFIER, f"the provider's identifier is {node.get('identifier')!r}")
        expect(node.get("online") == "1", "Plex says the provider is offline")
        types = {t.get("type") for t in node.findall("MetadataType")}
        expect({"1", "2", "3", "4"} <= types, f"the provider serves the types {sorted(types)}")
        provider_id = node.get("id")
    with R.check("plex: an agent is made of it, and a library of films and one of series use it"):
        groups = [g for g in plex_get(base, "/media/providers/metadata/group").findall("MetadataAgentProviderGroup") if g.get("title") == "ams-e2e"]
        group = groups[0] if groups else plex_post(base, "/media/providers/metadata/group", {"title": "ams-e2e", "primaryIdentifier": PLEX_IDENTIFIER}).find("MetadataAgentProviderGroup")
        expect(group is not None, "Plex made no agent")
        group_id = group.get("id")
        sections = {s.get("title"): s for s in plex_get(base, "/library/sections").findall("Directory")}
        for title, kind, scanner, location in (
            ("Films AMS", "1", "Plex Movie", "/media/Movies"),
            ("Series AMS", "2", "Plex TV Series", "/media/TV Shows"),
        ):
            if title not in sections:
                plex_post(base, "/library/sections", {
                    "name": title, "type": kind, "scanner": scanner, "agent": PLEX_IDENTIFIER,
                    "metadataAgentProviderGroupId": group_id, "language": "en-US", "location": location,
                })
        sections = plex_wait_scan(base, 2)
        by_title = {s.get("title"): s for s in sections}
        for title in ("Films AMS", "Series AMS"):
            expect(title in by_title, f"no library {title}")
            expect(by_title[title].get("agent") == PLEX_IDENTIFIER, f"{title} uses the agent {by_title[title].get('agent')!r}")
            expect(by_title[title].get("metadataAgentProviderGroupId") == group_id, f"{title} is not on the group")
        films_key, series_key = by_title["Films AMS"].get("key"), by_title["Series AMS"].get("key")

    def provider_json(path, language=None):
        headers = {"Accept": "application/json"}
        if language:
            headers["X-Plex-Language"] = language
        return http("GET", f"{stack.native.url}/plex{path}", headers=headers)["MediaContainer"]["Metadata"][0]

    def refreshed(rating_key, tag, attr, expected, tries=4):
        """What Plex holds is what the agent said when it was last asked. A
        catalogue changed since — a title locked by an earlier phase — shows in
        Plex after the item's refresh, which is asked for until it does."""
        for _ in range(tries):
            node = plex_get(base, f"/library/metadata/{rating_key}").find(tag)
            if node is not None and node.get(attr) == expected:
                return node
            http("PUT", f"{base}/library/metadata/{rating_key}/refresh", raw=True, timeout=60)
            time.sleep(10)
        return plex_get(base, f"/library/metadata/{rating_key}").find(tag)

    def picture(path):
        data = http("GET", base + path, raw=True, timeout=60)
        expect(len(data) > 500 and (data[:3] == b"\xff\xd8\xff" or data[:4] == b"\x89PNG" or data[:4] == b"RIFF"), f"{path} is not a picture ({len(data)} bytes)")

    with R.check("plex: the film is matched, and has the title, summary, year, genres, classification and poster of the catalogue"):
        videos = eventually(
            lambda: plex_get(base, f"/library/sections/{films_key}/all").findall("Video"),
            lambda found: len(found) == 1 and all(found[0].get(a) for a in ("summary", "thumb", "contentRating")),
        )
        expect(len(videos) == 1, f"the films library holds {len(videos)} films")
        ours = provider_json("/library/metadata/movie-tmdb-603")
        film = refreshed(videos[0].get("ratingKey"), "Video", "title", ours["title"])
        expect(film.get("guid") == f"{PLEX_IDENTIFIER}://movie/movie-tmdb-603", f"guid {film.get('guid')!r}")
        expect(film.get("title") == ours["title"], f"title {film.get('title')!r}, served {ours['title']!r}")
        expect(film.get("year") == str(ours["year"]) == "1999", f"year {film.get('year')!r}")
        expect(film.get("summary") == ours["summary"] and len(film.get("summary", "")) > 40, "the summary is not the catalogue's")
        expect(film.get("contentRating") == ours.get("contentRating"), f"classification {film.get('contentRating')!r}, served {ours.get('contentRating')!r}")
        expect({g.get("tag") for g in film.findall("Genre")} >= {"Action"}, f"genres {[g.get('tag') for g in film.findall('Genre')]}")
        expect(film.get("thumb"), "no poster")
        picture(film.get("thumb"))
    with R.check("plex: the series is matched, with its seasons and its episodes, summaries and pictures"):
        shows = eventually(
            lambda: plex_get(base, f"/library/sections/{series_key}/all").findall("Directory"),
            lambda found: len(found) == 1 and all(found[0].get(a) for a in ("summary", "thumb", "art")),
        )
        expect(len(shows) == 1, f"the series library holds {len(shows)} series")
        ours = provider_json("/library/metadata/show-tvdb-81189")
        show = refreshed(shows[0].get("ratingKey"), "Directory", "title", ours["title"])
        expect(show.get("guid") == f"{PLEX_IDENTIFIER}://show/show-tvdb-81189", f"guid {show.get('guid')!r}")
        expect(show.get("title") == ours["title"], f"title {show.get('title')!r}, served {ours['title']!r}")
        expect(show.get("year") == str(ours["year"]) == "2008", f"year {show.get('year')!r}")
        expect(show.get("summary") == ours["summary"] and len(show.get("summary", "")) > 40, "the summary is not the catalogue's")
        expect(show.get("thumb") and show.get("art"), "no poster or background")
        picture(show.get("thumb"))
        seasons = plex_get(base, f"/library/metadata/{show.get('ratingKey')}/children").findall("Directory")
        expect(len(seasons) == 1 and seasons[0].get("index") == "1", f"Plex lists {len(seasons)} seasons: {[s.get('title') for s in seasons]}")
        season = seasons[0]
        season_ours = provider_json("/library/metadata/season-tvdb-81189-s1")
        expect(season.get("title") == season_ours["title"], f"season title {season.get('title')!r}, served {season_ours['title']!r}")
        expect(season.get("summary") == season_ours.get("summary"), "the season's summary is not the catalogue's")
        episodes = plex_get(base, f"/library/metadata/{season.get('ratingKey')}/children").findall("Video")
        expect(len(episodes) == 2, f"Plex lists {len(episodes)} episodes")
        for episode in episodes:
            number = episode.get("index")
            theirs = provider_json(f"/library/metadata/episode-tvdb-81189-s1e{number}")
            expect(episode.get("title") == theirs["title"], f"episode {number}: title {episode.get('title')!r}, served {theirs['title']!r}")
            expect(episode.get("summary") == theirs.get("summary") and episode.get("summary"), f"episode {number}: the summary is not the catalogue's")
            expect(episode.get("thumb"), f"episode {number}: no picture")
        picture(episodes[0].get("thumb"))
        R.note(f"plex: {show.get('title')}: {[e.get('title') for e in episodes]}")
    with R.check("plex: a title locked in the catalogue appears in Plex after the item's refresh, and goes when the lock does"):
        item = native.item("series", "tvdb", 81189)
        expect(item is not None, "tvdb:81189 is not in the catalogue")
        key = plex_get(base, f"/library/sections/{series_key}/all").findall("Directory")[0].get("ratingKey")
        text = "Locked by the e2e stack (plex)"
        native.lock_title(item["id"], text)
        try:
            node = refreshed(key, "Directory", "title", text)
            expect(node.get("title") == text, f"Plex's title is {node.get('title')!r}, not the lock {text!r}")
        finally:
            native.call("DELETE", f"/items/{item['id']}/overrides/item/title")
        node = refreshed(key, "Directory", "title", "Breaking Bad")
        expect(node.get("title") == "Breaking Bad", f"Plex's title is {node.get('title')!r} after the lock was lifted")
    with R.check("plex: a series library in French is given the French text, as the catalogue serves it"):
        english, french = provider_json("/library/metadata/show-tvdb-81189"), provider_json("/library/metadata/show-tvdb-81189", "fr")
        if english["summary"] == french["summary"]:
            raise Skipped("the catalogue holds no French summary for the series")
        sections = {s.get("title"): s for s in plex_get(base, "/library/sections").findall("Directory")}
        if "Séries AMS" not in sections:
            plex_post(base, "/library/sections", {
                "name": "Séries AMS", "type": "2", "scanner": "Plex TV Series", "agent": PLEX_IDENTIFIER,
                "metadataAgentProviderGroupId": group_id, "language": "fr-FR", "location": "/media/TV Shows",
            })
        sections = {s.get("title"): s for s in plex_wait_scan(base, 3)}
        key = sections["Séries AMS"].get("key")
        shows = eventually(
            lambda: plex_get(base, f"/library/sections/{key}/all").findall("Directory"),
            lambda found: len(found) == 1 and found[0].get("summary") == french["summary"],
        )
        expect(shows, "the French library holds nothing")
        expect(shows[0].get("summary") == french["summary"], "the French library's summary is not the French one the catalogue serves")
    with R.check("plex: this server counted the calls Plex made, and refused none"):
        apis = {a["api"]: a for a in native.call("GET", "/admin/access")["apis"]}
        served = apis["plex"]["served"] - served_before
        # A library kept from an earlier run is not scanned again from nothing.
        expect(served >= 1, f"{served} calls counted since the provider was added")
        expect(apis["plex"]["refused"] == 0, f"{apis['plex']['refused']} calls refused")
        R.note(f"plex: {served} calls to /plex answered while Plex scanned")


# ── Main ───────────────────────────────────────────────────────────────────


PHASES = ("sonarr4", "sonarr5", "radarr", "push", "seed", "plex")


def main(argv):
    wanted = argv[1:] or list(PHASES)
    unknown = [p for p in wanted if p not in PHASES]
    if unknown:
        sys.exit(f"unknown phase {unknown}; phases: {' '.join(PHASES)}")
    stack = Stack()
    report = Report()
    for phase in wanted:
        print(f"== {phase}")
        sys.stdout.flush()
        try:
            if phase == "sonarr4" and stack.sonarr4:
                phase_sonarr(report, stack, stack.sonarr4)
            elif phase == "sonarr5" and stack.sonarr5:
                phase_sonarr(report, stack, stack.sonarr5)
            elif phase == "radarr" and stack.radarr:
                phase_radarr(report, stack, stack.radarr)
            elif phase == "push":
                phase_push(report, stack)
            elif phase == "seed":
                phase_seed(report, stack)
            elif phase == "plex" and os.environ.get("E2E_PLEX_URL"):
                phase_plex(report, stack)
            else:
                print("  not part of this stack")
        except Exception as error:  # a phase that cannot go on is a failure, named
            print(f"  FAILED: the {phase} phase stopped: {type(error).__name__}: {error}")
            report.failures.append(f"{phase}: the phase stopped: {type(error).__name__}: {error}")
    print(f"== {report.passed} passed, {len(report.failures)} failed, {len(report.skipped)} skipped")
    for failure in report.failures:
        print(f"   FAILED  {failure}")
    for skip in report.skipped:
        print(f"   skipped {skip}")
    return min(len(report.failures), 100)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
