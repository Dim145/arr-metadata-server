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

The series and the film entered by hand (HAND_SERIES, HAND_FILM) are made
through the native API by the first phase that needs one, and found by the
others in the catalogue: no phase counts on another having run. A phase that
locks a title lifts the lock again — the push phase leaves its own, as the
real works' are.

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
ORDERED_SERIES = 412806  # added on Sonarr 5 in an order made here, `ams-…`
FROZEN_SERIES = 305288  # an order made here frozen as its aired numbering, then thawed
ORDER_LABEL = "E2E order"  # the order made here, on ORDERED_SERIES
FROZEN_LABEL = "E2E frozen"  # the one frozen, on FROZEN_SERIES
OWN_TITLE = "E2E record of the order's own"  # an episode the order carries, the work does not hold
LOCKED = 81189  # the series, and film 603, a title is locked on
FRENCH = {"id": 2, "name": "French"}  # Sonarr's Language.French
ENGLISH = {"id": 1, "name": "English"}

# Works entered by hand that nothing elsewhere names — a series with no
# TheTVDB, TMDB or Fankai id, a film with no TMDB id — are handed a number of
# the server's own, from 1 000 000 000 to 2 147 483 647, made of the work's id
# in the catalogue. Sonarr keeps it as the series' TheTVDB id, Radarr as the
# film's TMDB id. The titles are ones no real work bears, so that a search by
# text finds these and not what a provider lists under a name like them;
# scripts/e2e-stack.sh names the videos Plex scans after the same ones.
HAND_SERIES = "Quenzabar Tides"
HAND_SERIES_YEAR = 2024
HAND_EPISODES = ((1, "The Tide Goes Out", "2024-01-10"), (2, "The Tide Comes Back", "2024-01-17"))
# A spelling a release group might use. Sonarr's scene mapping takes it from
# the catalogue; the title a series is served under is one Sonarr knows
# already, so a series with no other title is mapped under none.
HAND_ALTERNATIVE = "Tides of Quenzabar"
HAND_FILM = "Brindlemoss Without Source"
HAND_FILM_YEAR = 2019
HAND_FILM_STUDIO = "Hand Films"
# What the catalogue files them under: a lock on a title leaves the slug as it
# was made, where the title itself shows the lock.
HAND_SLUGS = {"series": "quenzabar-tides", "movie": "brindlemoss-without-source"}
OWN_FIRST, OWN_LAST = 1_000_000_000, 2_147_483_647
# What the seed says of a work its library holds under such a number once the
# catalogue is new: nothing here is made of it, and no source could be asked.
SEED_NO_ID = "held there under no id to look it up by"

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

    def by_hand(self, kind):
        """The series or the film entered by hand for these checks, if the
        catalogue holds it: found by its slug, which a lock on its title leaves
        as it was made."""
        for item in self.items(kind):
            if item.get("isManual") and item.get("slug", "").startswith(HAND_SLUGS[kind]):
                return item
        return None

    def lock_title(self, item_id, title):
        self.call("PUT", f"/items/{item_id}/overrides", {"field": "title", "value": title})

    def orders(self, item_id):
        return self.call("GET", f"/items/{item_id}/orders")

    def order_made(self, item_id, label, change):
        """An order made here from the aired one, its places written as
        `change` makes them of the aired ones; one of that name a run before
        made is forgotten first, so the order is the aired one as it is now."""
        for order in self.orders(item_id)["orders"]:
            if order["source"] == "local" and order["label"] == label:
                self.call("DELETE", f"/items/{item_id}/orders/{urllib.parse.quote(order['key'])}")
        made = self.call("POST", f"/items/{item_id}/orders", {"label": label, "from": "official"})
        places = [
            {
                "id": place["id"],
                "seasonNumber": place["seasonNumber"],
                "episodeNumber": place["episodeNumber"],
                "absoluteNumber": place.get("absoluteNumber"),
                "episodeId": place.get("episodeId"),
                "tvdbId": place.get("tvdbId"),
                "tmdbId": place.get("tmdbId"),
                "record": place.get("record"),
            }
            for place in made["episodes"]
        ]
        written = self.call(
            "PUT",
            f"/items/{item_id}/orders/{urllib.parse.quote(made['key'])}",
            {"label": label, "places": change(places)},
        )
        return written["key"]

    def order_forgotten(self, item_id, label):
        for order in self.orders(item_id)["orders"]:
            if order["source"] == "local" and order["label"] == label:
                self.call("DELETE", f"/items/{item_id}/orders/{urllib.parse.quote(order['key'])}")


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


# ── Works entered by hand ──────────────────────────────────────────────────


def own_number(catalogue_id):
    """The number the server hands a work entered by hand: its catalogue id's
    FNV-1a hash on 64 bits, taken into the range (`own` in src/service/ids.rs).
    It is worked out here as it is there because a client keeps the number for
    good: were it made another way one day, every such work a Sonarr or a
    Radarr holds would be under a number nobody answers for."""
    hashed = 0xCBF29CE484222325
    for byte in catalogue_id.encode():
        hashed = ((hashed ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return OWN_FIRST + hashed % (OWN_LAST - OWN_FIRST + 1)


def entered_by_hand(native, kind):
    """The series or the film entered by hand, made as the administration's own
    editor makes it — *New entry*, and for a series an alternative title, a
    season and its episodes — unless the catalogue holds it already: a phase
    before, or a run before, made it, and each phase must find the one work
    rather than make another of the same title, which would be handed another
    number."""
    held = native.by_hand(kind)
    if held is not None:
        return held
    if kind == "movie":
        return native.call("POST", "/items", {
            "kind": "movie", "title": HAND_FILM, "year": HAND_FILM_YEAR, "status": "released",
            "overview": "A film entered by hand, with no source behind it.", "runtime": 93,
            "inCinemas": f"{HAND_FILM_YEAR}-05-01", "studio": HAND_FILM_STUDIO, "genres": ["Drama"],
        })
    made = native.call("POST", "/items", {
        "kind": "series", "title": HAND_SERIES, "year": HAND_SERIES_YEAR, "status": "continuing",
        "overview": "A series entered by hand, with no source behind it.", "network": "Hand TV",
        "firstAired": HAND_EPISODES[0][2], "genres": ["Animation"],
    })
    native.call("POST", f"/items/{made['id']}/alternative-titles", {"title": HAND_ALTERNATIVE, "titleType": "working", "language": "en"})
    native.call("POST", f"/items/{made['id']}/seasons", {"seasonNumber": 1})
    for number, title, aired in HAND_EPISODES:
        native.call("POST", f"/items/{made['id']}/episodes", {
            "seasonNumber": 1, "episodeNumber": number, "title": title, "airDate": aired,
            "overview": f"{title}, typed in.", "runtime": 24,
        })
    return made


def lift_title(native, item_id):
    """The lock on a work's title taken off again: the work is left as it was
    typed, for the next phase and the next run to find."""
    native.call("DELETE", f"/items/{item_id}/overrides/item/title")


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


def swapped_first_two(places):
    """The first two places of the first regular season, what they hold
    swapped and their numbers where they were: the aired order, but for two
    episodes — enough for a numbering to be told from the aired one."""
    regular = sorted((p for p in places if p["seasonNumber"] > 0), key=lambda p: (p["seasonNumber"], p["episodeNumber"]))
    if len(regular) < 2 or regular[0]["seasonNumber"] != regular[1]["seasonNumber"]:
        raise Failure(f"no first season of two episodes to swap: {[(p['seasonNumber'], p['episodeNumber']) for p in regular[:4]]}")
    first, second = regular[0], regular[1]
    for field in ("id", "episodeId", "tvdbId", "tmdbId", "record"):
        first[field], second[field] = second.get(field), first.get(field)
    return places


def with_own_record(places):
    """The places swapped as above, and a special of the order's own the work
    does not hold, at S00E99."""
    places = swapped_first_two(places)
    places = [p for p in places if (p["seasonNumber"], p["episodeNumber"]) != (0, 99)]
    places.append({"seasonNumber": 0, "episodeNumber": 99, "record": {"title": OWN_TITLE, "aired": "2023-01-01", "runtime": 24}})
    return places


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


def contract_problems(show, five, optional=()):
    """What of a served series Sonarr cannot read: a missing field it needs, a
    field of another kind, in the series, its episodes and what they hold.
    `optional` names the fields this series may lack."""
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
        if name not in optional and show.get(name) in (None, ""):
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
        if ORDERED_SERIES in stack.series:
            # An order made here, offered under a key TheTVDB never gave:
            # whether Sonarr v5 takes a season type it was not built with.
            with R.check(f"{label}: an order made here on tvdb:{ORDERED_SERIES} is offered among its season types"):
                item = native.item("series", "tvdb", ORDERED_SERIES)
                expect(item is not None, f"tvdb:{ORDERED_SERIES} is not in the catalogue")
                key = native.order_made(item["id"], ORDER_LABEL, with_own_record)
                served = served_show(stack, arr, ORDERED_SERIES)
                offered = [t.get("type") for t in served.get("seasonTypes") or []]
                expect(key in offered, f"this server offers {offered}, not {key!r}")
                found = arr.get(f"{arr.api}/series/lookup?term=tvdb:{ORDERED_SERIES}")
                held = [s for s in found if s.get("tvdbId") == ORDERED_SERIES]
                types = [t.get("type") for s in held for t in s.get("seasonTypes") or []]
                expect(
                    key in types,
                    f"Sonarr's lookup offers {types} for tvdb:{ORDERED_SERIES}; it answered "
                    f"{json.dumps(held[:1])[:800]}",
                )
                options[ORDERED_SERIES] = dict(season_type=key)
                R.note(f"{label}: offered {key!r} among {types}")
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

    # ── A series entered by hand: no id anywhere, a number of the server's own ─
    phase_sonarr_by_hand(R, stack, arr)

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
    if ORDERED_SERIES in ids:
        tvdb = ORDERED_SERIES
        with R.check(f"{label}: added in the order made here, tvdb:{tvdb} is numbered by it, its own record among its episodes"):
            held = arr.get(f"{arr.api}/series/{ids[tvdb]}")
            key = held.get("seasonType")
            expect(
                isinstance(key, str) and key.startswith("ams-"),
                f"the series' season type is {key!r}; Sonarr holds {json.dumps(held)[:800]}",
            )
            served = served_show(stack, arr, tvdb, "eng", key)
            aired = served_show(stack, arr, tvdb, "eng", "official")
            episodes = arr.get(f"{arr.api}/episode?seriesId={ids[tvdb]}")
            here = {key_of(e): e.get("title") for e in episodes}
            there = {key_of(e): e.get("title") for e in served["episodes"]}
            expect(
                set(here) == set(there),
                f"Sonarr's numbering is not the order's: Sonarr lacks {sorted(set(there) - set(here))[:6]}, "
                f"has extra {sorted(set(here) - set(there))[:6]}",
            )
            expect(here.get((0, 99)) == OWN_TITLE, f"S00E99 in Sonarr is {here.get((0, 99))!r}, not {OWN_TITLE!r}")
            aired_titles = {key_of(e): e.get("title") for e in aired["episodes"]}
            first = min(k for k in aired_titles if k[0] > 0)
            second = min(k for k in aired_titles if k[0] == first[0] and k > first)
            expect(
                (here.get(first), here.get(second)) == (aired_titles[second], aired_titles[first]),
                f"{first} and {second} are {here.get(first)!r} and {here.get(second)!r} in Sonarr; aired, "
                f"{aired_titles[first]!r} and {aired_titles[second]!r}",
            )
            R.note(f"{label}: {len(episodes)} episodes in {key!r}, {len(aired['episodes'])} aired")
    if FROZEN_SERIES in ids:
        phase_frozen(R, stack, arr, ids)


def phase_frozen(R, stack, arr, ids):
    """An order made here, frozen as the aired numbering: what Sonarr asking
    for the aired order is served from then on, which it follows at its next
    refresh; thawed, it goes back. Thawed whatever came of it, so that the
    series is left as the stack found it."""
    label, tvdb, native = arr.label, FROZEN_SERIES, stack.native
    item = native.item("series", "tvdb", tvdb)
    if item is None:
        with R.check(f"{label}: an order frozen on tvdb:{tvdb} numbers it in Sonarr"):
            raise Failure(f"tvdb:{tvdb} is not in the catalogue")
        return
    aired = served_show(stack, arr, tvdb)
    aired_titles = {key_of(e): e.get("title") for e in aired["episodes"]}
    regular = sorted(k for k in aired_titles if k[0] > 0)
    first, second = regular[0], regular[1]
    try:
        with R.check(f"{label}: an order frozen as the aired numbering of tvdb:{tvdb} numbers it in Sonarr after a refresh"):
            key = native.order_made(item["id"], FROZEN_LABEL, swapped_first_two)
            native.call("POST", f"/items/{item['id']}/orders/{urllib.parse.quote(key)}/freeze")
            frozen = served_show(stack, arr, tvdb)
            served = {key_of(e): e.get("title") for e in frozen["episodes"]}
            expect(
                (served.get(first), served.get(second)) == (aired_titles[second], aired_titles[first]),
                f"the aired order is served as {served.get(first)!r}, {served.get(second)!r} at {first}, {second}: "
                f"not the frozen numbering",
            )
            refresh_series(arr, ids[tvdb])
            episodes = {key_of(e): e.get("title") for e in arr.get(f"{arr.api}/episode?seriesId={ids[tvdb]}")}
            expect(
                (episodes.get(first), episodes.get(second)) == (aired_titles[second], aired_titles[first]),
                f"after its refresh Sonarr holds {episodes.get(first)!r}, {episodes.get(second)!r} at {first}, "
                f"{second}; the frozen numbering is {served.get(first)!r}, {served.get(second)!r}",
            )
    finally:
        with R.check(f"{label}: thawed, tvdb:{tvdb} is numbered as it aired again in Sonarr"):
            native.order_forgotten(item["id"], FROZEN_LABEL)
            back = served_show(stack, arr, tvdb)
            served = {key_of(e): e.get("title") for e in back["episodes"]}
            expect(served.get(first) == aired_titles[first], f"the aired order is served as {served.get(first)!r} at {first}")
            refresh_series(arr, ids[tvdb])
            episodes = {key_of(e): e.get("title") for e in arr.get(f"{arr.api}/episode?seriesId={ids[tvdb]}")}
            expect(
                episodes.get(first) == aired_titles[first],
                f"after its refresh Sonarr holds {episodes.get(first)!r} at {first}, aired {aired_titles[first]!r}",
            )


def phase_sonarr_by_hand(R, stack, arr):
    """A series entered by hand, which nothing elsewhere names: Sonarr finds it
    by its title and by `tvdb:` and the number it was handed, adds it with the
    season and the episodes it was given, and takes a title locked here at its
    next refresh — under the same number throughout. A check whose work is
    missing is skipped: the first one says why."""
    label, v5, native = arr.label, arr.version.startswith("5."), stack.native
    item = number = sonarr_id = None

    with R.check(f"{label}: a series entered by hand is in the catalogue, with its season and episodes, and no id elsewhere"):
        item = entered_by_hand(native, "series")
        number = own_number(item["id"])
        named = {k: v for k, v in (item.get("externalIds") or {}).items() if v}
        expect(not named, f"it is named by {named}: not a series nothing elsewhere names")
        R.note(f"{label}: catalogue id {item['id']}, which makes the number {number}")
    with R.check(f"{label}: a search by its title lists the series entered by hand, under a number of the server's own"):
        if item is None:
            raise Skipped("no series entered by hand")
        found = arr.get(f"{arr.api}/series/lookup?term={urllib.parse.quote(HAND_SERIES.lower())}")
        listed = [s for s in found if s.get("title") == HAND_SERIES]
        expect(listed, f"not among {[(s['title'], s['tvdbId']) for s in found[:6]]}")
        expect(len(listed) == 1, f"listed {len(listed)} times, under {[s['tvdbId'] for s in listed]}")
        expect(OWN_FIRST <= listed[0]["tvdbId"] <= OWN_LAST, f"listed under {listed[0]['tvdbId']}, which is not in the server's own range")
        expect(listed[0]["tvdbId"] == number, f"listed under {listed[0]['tvdbId']}; its catalogue id makes {number}")
    with R.check(f"{label}: a search by tvdb: and that number finds the series entered by hand, and it alone"):
        if number is None:
            raise Skipped("no series entered by hand")
        found = arr.get(f"{arr.api}/series/lookup?term=tvdb:{number}")
        expect(
            len(found) == 1 and found[0]["tvdbId"] == number and found[0]["title"] == HAND_SERIES,
            f"answered {[(s['title'], s['tvdbId']) for s in found[:4]]}",
        )
        # A number of the range no work was handed names nothing, and the
        # server says so at once: no source has it to be asked.
        unhanded = OWN_FIRST if number != OWN_FIRST else OWN_FIRST + 1
        began = time.time()
        found = arr.get(f"{arr.api}/series/lookup?term=tvdb:{unhanded}")
        expect(not found, f"tvdb:{unhanded}, which no work was handed, found {[(s['title'], s['tvdbId']) for s in found[:4]]}")
        expect(time.time() - began < 15, f"tvdb:{unhanded} took {time.time() - began:.0f}s to find nothing: was a source asked for it?")
    with R.check(f"{label}: the server serves the series entered by hand under that number on both editions, with its episodes"):
        if number is None:
            raise Skipped("no series entered by hand")
        for edition, path in (("v1", f"/v1/tvdb/shows/en/{number}"), ("v2", f"/v2/tvdb/shows/eng/{number}/official")):
            shown = stack.door.get("skyhook.sonarr.tv", path)
            expect(shown["tvdbId"] == number and shown["title"] == HAND_SERIES, f"{edition}: {shown['title']!r} as {shown['tvdbId']}")
            expect(len(shown["episodes"]) == len(HAND_EPISODES), f"{edition}: {len(shown['episodes'])} episodes")
            expect(all(e.get("tvdbShowId", number) == number for e in shown["episodes"]), f"{edition}: an episode is of another series")
    with R.check(f"{label}: the series entered by hand is added"):
        if number is None:
            raise Skipped("no series entered by hand")
        # A catalogue the seed phase replaced handed the series another number,
        # and Sonarr, never told, still holds it under the old one, at the very
        # path this add needs: that one goes, as a run that began afresh has none.
        for stale in arr.get(f"{arr.api}/series"):
            if OWN_FIRST <= stale["tvdbId"] <= OWN_LAST and stale["tvdbId"] != number:
                arr.call("DELETE", f"{arr.api}/series/{stale['id']}?deleteFiles=false")
        sonarr_id = add_series(stack, arr, number)
        settle_commands(arr, SONARR_REFRESHES)
    with R.check(f"{label}: the series entered by hand has the season and the episodes it was given, as the server serves them"):
        if sonarr_id is None:
            raise Skipped("the series entered by hand is not added")
        held = arr.get(f"{arr.api}/series/{sonarr_id}")
        episodes = arr.get(f"{arr.api}/episode?seriesId={sonarr_id}")
        served = served_show(stack, arr, number)
        expect(held["tvdbId"] == number, f"Sonarr holds it under {held['tvdbId']}, not {number}")
        expect(held["title"] == served["title"] == HAND_SERIES, f"title {held['title']!r}, served {served['title']!r}")
        seasons_here = sorted(s["seasonNumber"] for s in held["seasons"])
        seasons_served = sorted(s["seasonNumber"] for s in served["seasons"])
        expect(seasons_here == seasons_served == [1], f"seasons: Sonarr {seasons_here}, served {seasons_served}")
        typed = {(1, n): (title, aired) for n, title, aired in HAND_EPISODES}
        here = {key_of(e): (e["title"], e.get("airDate")) for e in episodes}
        there = {key_of(e): (e["title"], e.get("airDate")) for e in served["episodes"]}
        expect(here == typed, f"Sonarr holds {sorted(here.items())}; it was given {sorted(typed.items())}")
        expect(here == there, f"Sonarr holds {sorted(here.items())}; served {sorted(there.items())}")
        R.note(f"{label}: {held['title']!r} {held.get('year')}, {len(episodes)} episodes, status {held.get('status')}, path {held.get('path')}")
    with R.check(f"{label}: the series entered by hand is served with every field Sonarr {arr.version[:1]} reads, as it reads it"):
        if number is None:
            raise Skipped("no series entered by hand")
        # No source says which language a series entered by hand was made in,
        # and nobody typed one: Sonarr 5 takes its own default for it.
        problems = contract_problems(served_show(stack, arr, number), v5, optional=("originalLanguage",))
        expect(not problems, "; ".join(problems))
    if v5:
        with R.check(f"{label}: Sonarr 5 is sent metadataSource local for the series entered by hand, in a search and when it asks for it"):
            if number is None:
                raise Skipped("no series entered by hand")
            searched = stack.door.get("skyhook.sonarr.tv", f"/v2/tvdb/search?term={urllib.parse.quote(HAND_SERIES.lower())}&language=eng")
            hit = [s for s in searched if s.get("tvdbId") == number]
            expect(hit and hit[0].get("metadataSource") == "local", f"the search says {[(s.get('tvdbId'), s.get('metadataSource')) for s in searched[:4]]}")
            source = served_show(stack, arr, number).get("metadataSource")
            expect(source == "local", f"the series is served with metadataSource {source!r}")
            # Sonarr may not show it back: what it holds is then the server's answer above.
            held = arr.get(f"{arr.api}/series/{sonarr_id}") if sonarr_id else {}
            if "metadataSource" in held:
                expect(held["metadataSource"] == "local", f"Sonarr holds metadataSource {held['metadataSource']!r}")
            else:
                R.note(f"{label}: Sonarr's series resource shows no metadataSource; the server's own answer says local")
    with R.check(f"{label}: a title locked on the series entered by hand appears in Sonarr after its refresh, and goes with the lock"):
        if sonarr_id is None:
            raise Skipped("the series entered by hand is not added")
        text = f"Locked by the e2e stack ({label}, by hand)"
        native.lock_title(item["id"], text)
        try:
            refresh_series(arr, sonarr_id)
            held = arr.get(f"{arr.api}/series/{sonarr_id}")
            expect(held["title"] == text, f"Sonarr's title is {held['title']!r}, not {text!r}")
            expect(held["tvdbId"] == number, f"Sonarr holds it under {held['tvdbId']} after the refresh, not {number}")
        finally:
            lift_title(native, item["id"])
        refresh_series(arr, sonarr_id)
        held = arr.get(f"{arr.api}/series/{sonarr_id}")
        expect(held["title"] == HAND_SERIES, f"Sonarr's title is {held['title']!r} after the lock was lifted")
        episodes = arr.get(f"{arr.api}/episode?seriesId={sonarr_id}")
        expect(len(episodes) == len(HAND_EPISODES), f"{len(episodes)} episodes after the refreshes")


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
    with R.check(f"{label}: the scene mapping names the series entered by hand by its own number, and a release named so finds it"):
        item = stack.native.by_hand("series")
        if item is None:
            raise Skipped("the catalogue holds no series entered by hand")
        if not hasattr(stack, "scene_began"):
            raise Skipped("Update Scene Mapping did not complete above")
        number = own_number(item["id"])
        added = scene_additions(stack, {number})
        expect(added, f"this catalogue adds no mapping under {number}")
        if series_by_tvdb(arr, number) is None:
            raise Skipped("Sonarr does not hold the series entered by hand: its own phase adds it")
        for mapping in added:
            release = re.sub(r"[\W_]+", ".", mapping["title"]).strip(".") + ".S01E01.1080p.WEB.x264-GRP"
            parsed = arr.get(f"/api/v3/parse?title={urllib.parse.quote(release)}")
            found = (parsed.get("series") or {}).get("tvdbId")
            expect(found == number, f"{release!r} is taken for tvdb:{found}, not {number}")
        R.note(f"{label}: mapped as {[m['title'] for m in added]}")
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

    # ── A film entered by hand: no TMDB id, a number of the server's own ───
    phase_radarr_by_hand(R, stack, arr)

    # Last: it reads the whole of Radarr's log.
    with R.check(f"{label}: Radarr refused no certificate"):
        log = subprocess.run(["docker", "logs", "ams-e2e-radarr"], capture_output=True, text=True)
        lines = [l for l in (log.stdout + log.stderr).splitlines() if "RemoteCertificate" in l]
        expect(not lines, f"{len(lines)} refused certificates in Radarr's log: {lines[0][:160] if lines else ''}")


def phase_radarr_by_hand(R, stack, arr):
    """A film entered by hand, which has no TMDB id: Radarr finds it by its
    title and by `tmdb:` and the number it was handed, adds it, holds it as the
    server serves it, and takes a title locked here at its next refresh — under
    the same number throughout. A check whose work is missing is skipped: the
    first one says why."""
    label, native = arr.label, stack.native
    item = number = movie_id = None

    with R.check(f"{label}: a film entered by hand is in the catalogue, with no id elsewhere"):
        item = entered_by_hand(native, "movie")
        number = own_number(item["id"])
        named = {k: v for k, v in (item.get("externalIds") or {}).items() if v}
        expect(not named, f"it is named by {named}: not a film nothing elsewhere names")
        R.note(f"{label}: catalogue id {item['id']}, which makes the number {number}")
    with R.check(f"{label}: a search by its title lists the film entered by hand, under a number of the server's own"):
        if item is None:
            raise Skipped("no film entered by hand")
        found = arr.get(f"/api/v3/movie/lookup?term={urllib.parse.quote(HAND_FILM.lower())}")
        listed = [m for m in found if m.get("title") == HAND_FILM]
        expect(listed, f"not among {[(m['title'], m['tmdbId']) for m in found[:6]]}")
        expect(len(listed) == 1, f"listed {len(listed)} times, under {[m['tmdbId'] for m in listed]}")
        expect(OWN_FIRST <= listed[0]["tmdbId"] <= OWN_LAST, f"listed under {listed[0]['tmdbId']}, which is not in the server's own range")
        expect(listed[0]["tmdbId"] == number, f"listed under {listed[0]['tmdbId']}; its catalogue id makes {number}")
    with R.check(f"{label}: a search by tmdb: and that number finds the film entered by hand, and it alone"):
        if number is None:
            raise Skipped("no film entered by hand")
        found = arr.get(f"/api/v3/movie/lookup?term=tmdb:{number}")
        expect(
            len(found) == 1 and found[0]["tmdbId"] == number and found[0]["title"] == HAND_FILM,
            f"answered {[(m['title'], m['tmdbId']) for m in found[:4]]}",
        )
        # A number of the range no work was handed names nothing, and the
        # server says so at once: no source has it to be asked.
        unhanded = OWN_FIRST if number != OWN_FIRST else OWN_FIRST + 1
        began = time.time()
        found = arr.get(f"/api/v3/movie/lookup?term=tmdb:{unhanded}")
        expect(not found, f"tmdb:{unhanded}, which no work was handed, found {[(m['title'], m['tmdbId']) for m in found[:4]]}")
        expect(time.time() - began < 15, f"tmdb:{unhanded} took {time.time() - began:.0f}s to find nothing: was a source asked for it?")
    with R.check(f"{label}: the film entered by hand is added"):
        if number is None:
            raise Skipped("no film entered by hand")
        # One a catalogue the seed phase replaced handed another number is still
        # held under the old one, at the very path this add needs: it goes.
        for stale in arr.get("/api/v3/movie"):
            if OWN_FIRST <= stale["tmdbId"] <= OWN_LAST and stale["tmdbId"] != number:
                arr.call("DELETE", f"/api/v3/movie/{stale['id']}?deleteFiles=false")
        existing = [m for m in arr.get("/api/v3/movie") if m.get("tmdbId") == number]
        if existing:
            movie_id = existing[0]["id"]
        else:
            body = [m for m in arr.get(f"/api/v3/movie/lookup?term=tmdb:{number}") if m.get("tmdbId") == number][0]
            body.update(
                qualityProfileId=1, rootFolderPath=RADARR_ROOT, monitored=False, minimumAvailability="released",
                addOptions={"monitor": "none", "searchForMovie": False},
            )
            try:
                movie_id = arr.call("POST", "/api/v3/movie", body)["id"]
            except HttpError as e:
                raise Failure(f"Radarr refused the add of tmdb:{number}: {e}") from None
        settle_commands(arr, RADARR_REFRESHES)
    with R.check(f"{label}: the film entered by hand is refreshed, and is what the server serves"):
        if movie_id is None:
            raise Skipped("the film entered by hand is not added")
        state = arr.command("RefreshMovie", movieIds=[movie_id])
        expect(state.get("status") == "completed", f"RefreshMovie ended {state.get('status')}: {state.get('message')}")
        movie = arr.get(f"/api/v3/movie/{movie_id}")
        served = stack.door.get("api.radarr.video", f"/v1/movie/{number}")
        expect(movie["tmdbId"] == number == served["TmdbId"], f"Radarr holds it under {movie['tmdbId']}, served {served['TmdbId']}, made {number}")
        expect(movie["title"] == served["Title"] == HAND_FILM, f"title {movie['title']!r}, served {served['Title']!r}")
        expect(movie["year"] == served["Year"] == HAND_FILM_YEAR, f"year {movie['year']}, served {served['Year']}")
        expect(movie["runtime"] == served["Runtime"] == 93, f"runtime {movie['runtime']}, served {served['Runtime']}")
        expect((movie.get("studio") or "") == (served.get("Studio") or "") == HAND_FILM_STUDIO, f"studio {movie.get('studio')!r}, served {served.get('Studio')!r}")
        expect(movie.get("overview") == served.get("Overview"), "the overview is not the served one")
    with R.check(f"{label}: a title locked on the film entered by hand appears in Radarr after its refresh, and goes with the lock"):
        if movie_id is None:
            raise Skipped("the film entered by hand is not added")
        text = f"Locked by the e2e stack ({label}, by hand)"
        # An instant, UTC, a moment before the edit: what was changed since.
        since = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(time.time() - 5))
        native.lock_title(item["id"], text)
        try:
            changed = stack.door.get("api.radarr.video", f"/v1/movie/changed?since={since}")
            expect(number in changed, f"the films changed since {since} are {len(changed)}, and not {number}")
            state = arr.command("RefreshMovie", movieIds=[movie_id])
            expect(state.get("status") == "completed", f"RefreshMovie ended {state.get('status')}: {state.get('message')}")
            movie = arr.get(f"/api/v3/movie/{movie_id}")
            expect(movie["title"] == text, f"Radarr's title is {movie['title']!r}, not {text!r}")
            expect(movie["tmdbId"] == number, f"Radarr holds it under {movie['tmdbId']} after the refresh, not {number}")
        finally:
            lift_title(native, item["id"])
        state = arr.command("RefreshMovie", movieIds=[movie_id])
        expect(state.get("status") == "completed", f"RefreshMovie ended {state.get('status')}")
        movie = arr.get(f"/api/v3/movie/{movie_id}")
        expect(movie["title"] == HAND_FILM, f"Radarr's title is {movie['title']!r} after the lock was lifted")


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


def held_as(arr, kind, number):
    """The series or the film a client holds under that id: its TheTVDB id for
    a series, its TMDB id for a film."""
    if kind == "series":
        return series_by_tvdb(arr, number)
    return next((m for m in arr.get("/api/v3/movie") if m.get("tmdbId") == number), None)


def push_after_lock(R, native, arr, name, ids_field, kind, work):
    """A lock put on a work sends the client its refresh — RefreshSeries or
    RefreshMovie, for that work — and the server records the push. `work` is
    the id the client holds the work under, TheTVDB's or TMDB's; of the series
    or the film entered by hand, which nothing elsewhere names, it is None: the
    client holds it under the number the server made of its catalogue id. The
    lock is left on, as is the real work's: lifting it would send a second
    command a moment later, to whichever client is connected by then."""
    label, is_sonarr, by_hand = arr.label, kind == "series", work is None
    held_by = f"the {kind} entered by hand" if by_hand else f"{kind} {work}"
    skipped = False
    with R.check(f"push: a lock put on the {kind} entered by hand sends {name} to {label}" if by_hand else f"push: a lock put in the catalogue sends {name} to {label}"):
        if by_hand:
            item = native.by_hand(kind)
            work = own_number(item["id"]) if item else None
        else:
            item = native.item(kind, "tvdb" if is_sonarr else "tmdb", work)
        find = held_as(arr, kind, work) if work else None
        if find is None:
            skipped = True
            raise Skipped(f"{label} does not hold {held_by}: run its own phase first")
        expect(item is not None, f"{held_by} is not in the catalogue")
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
        held = arr.get(path)
        expect(held["title"] == text, f"{label} holds the title {held['title']!r}, not the lock {text!r}")
        expect((held["tvdbId"] if is_sonarr else held["tmdbId"]) == work, f"{label} holds it under another id than {work}")
    with R.check(f"push: the server's own record of it, for {label}, the {kind} entered by hand" if by_hand else f"push: the server's own record of it, for {label}"):
        if skipped:
            raise Skipped(f"no lock was put on {held_by}")
        app = "sonarr" if is_sonarr else "radarr"
        sent = [p for p in native.call("GET", "/admin/arr").get("pushes", []) if p.get("app") == app and p.get("outcome") == "sent"]
        expect(sent, f"the page lists no {app} work sent")
        expect("lock" in sent[0].get("reasons", []), f"the latest send was for {sent[0].get('reasons')}, not for a lock")
        R.note(f"push: {label}: {sent[0].get('title')!r} sent as {sent[0].get('detail')!r}")


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
        # A work of the catalogue, then the one entered by hand.
        push_after_lock(R, native, arr, name, ids_field, kind, work)
        push_after_lock(R, native, arr, name, ids_field, kind, None)


def phase_seed(R, stack):
    """`arr.seed` into a catalogue of its own: the stack's libraries are what
    a deployment's Sonarr and Radarr hold before they are pointed here."""
    native = stack.native
    with R.check("seed: the catalogue begins empty of what the libraries hold"):
        held = [i for i in native.items("series") if i["externalIds"].get("tvdb") in stack.series]
        held += [i for i in native.items("movie") if i["externalIds"].get("tmdb") in stack.films]
        # The works entered by hand in the catalogue this one replaced are gone with it.
        held += [i for i in (native.by_hand("series"), native.by_hand("movie")) if i is not None]
        expect(not held, f"the catalogue already holds {[i['title'] for i in held]}: the server was not restarted on a fresh database")
    sources = [(stack.sonarr5 or stack.sonarr4), stack.radarr]
    with R.check("seed: Sonarr and Radarr are connected"):
        connect(native, sources[0], stack.radarr, push=False)
    expected_series = [t for t in stack.series if series_by_tvdb(sources[0], t)] if sources[0] else []
    expected_films = [m for m in stack.films if stack.radarr and any(x.get("tmdbId") == m for x in stack.radarr.get("/api/v3/movie"))]
    # What the libraries hold under a number of the server's own: the works
    # entered by hand in the catalogue the restart replaced. Nothing here can
    # answer for those numbers any more, and no source ever could.
    library = [("series", s) for s in (sources[0].get(f"{sources[0].api}/series") if sources[0] else [])]
    library += [("movie", m) for m in (stack.radarr.get("/api/v3/movie") if stack.radarr else [])]
    handed = [
        f"{w['title']} ({w['year']})" if w.get("year") else w["title"]
        for kind, w in library
        if OWN_FIRST <= (w.get("tvdbId" if kind == "series" else "tmdbId") or 0) <= OWN_LAST
    ]
    job = run = None
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
    with R.check(f"seed: the {len(handed)} works entered by hand the libraries hold are not imported: reported held under no id to look them up by"):
        if not handed:
            raise Skipped("the libraries hold no work entered by hand: run the sonarr and radarr phases first")
        if run is None:
            raise Skipped("the run above did not end")
        listed = native.call("GET", f"/jobs/{job}")
        noted = [e for e in listed["entries"] if e.get("note") == SEED_NO_ID]
        expect(sorted(e.get("title") for e in noted) == sorted(handed), f"reported held under no id: {sorted(e.get('title') for e in noted)}; held by hand: {sorted(handed)}")
        expect(not any(e.get("outcome") == "imported" or e.get("mediaId") for e in noted), f"one was made here: {noted}")
        # Nothing came of them but a note: the others are the run's own count, and the libraries' whole.
        expect(
            re.search(rf"\b{len(handed)} not found, 0 failed, of {len(library)}\b", run.get("detail") or ""),
            f"the run says {run.get('detail')!r}; {len(handed)} works by hand, {len(library)} in the libraries",
        )
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
        # What cannot be held here is reported as often as it is asked.
        if handed:
            expect(re.search(rf"\b{len(handed)} not found, 0 failed\b", detail), f"the second run says: {detail}")


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


def plex_put(base, path, params, timeout=120):
    query = urllib.parse.urlencode(params, quote_via=urllib.parse.quote)
    return plex_xml(http("PUT", f"{base}{path}?{query}", timeout=timeout, raw=True))


def plex_restarted(base):
    """Plex restarted, and answering again. Its agents keep the address a
    provider had when Plex started: one changed since is theirs only after."""
    subprocess.run(["docker", "restart", "ams-e2e-plex"], check=True, capture_output=True)
    for _ in range(60):
        try:
            if plex_get(base, "/identity").get("startState") in (None, "started"):
                return
        except TRANSIENT + (HttpError,):
            pass
        time.sleep(3)
    raise Failure("Plex did not come back after its restart")


def plex_refreshed(base, rating_key, tag, attr, expected, tries=4):
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
        if known and known[0].get("uri") != provider:
            # Left at another address by an earlier run: the one with a key.
            node = plex_put(base, f"/media/providers/metadata/{known[0].get('id')}", {"uri": provider}).find("MetadataAgentProvider")
        else:
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
        return plex_refreshed(base, rating_key, tag, attr, expected, tries)

    def picture(path):
        data = http("GET", base + path, raw=True, timeout=60)
        expect(len(data) > 500 and (data[:3] == b"\xff\xd8\xff" or data[:4] == b"\x89PNG" or data[:4] == b"RIFF"), f"{path} is not a picture ({len(data)} bytes)")

    with R.check("plex: the film is matched, and has the title, summary, year, genres, classification and poster of the catalogue"):
        videos = eventually(
            lambda: plex_get(base, f"/library/sections/{films_key}/all").findall("Video"),
            # Plex fills the record in steps: the genres land after the text and
            # the poster, so a look taken too early finds none.
            lambda found: len(found) == 1
            and all(found[0].get(a) for a in ("summary", "thumb", "contentRating"))
            and found[0].findall("Genre"),
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
    # ── A series and a film entered by hand: matched under the catalogue's own id ──
    # Libraries of their own, on the same agent — this provider alone, with no
    # Plex provider beside it to guess a match by title — and folders of their
    # own: the checks above count what theirs hold. The works are made in the
    # catalogue before the libraries are, so that the first scan finds them.
    hand = {}
    with R.check("plex: a library of series and one of films entered by hand are made on the agent that is this provider alone"):
        series, film = entered_by_hand(native, "series"), entered_by_hand(native, "movie")
        sections = {s.get("title"): s for s in plex_get(base, "/library/sections").findall("Directory")}
        wanted = 0
        for title, kind, scanner, location in (
            ("Series by hand AMS", "2", "Plex TV Series", "/media/By hand/TV Shows"),
            ("Films by hand AMS", "1", "Plex Movie", "/media/By hand/Movies"),
        ):
            if title not in sections:
                wanted += 1
                plex_post(base, "/library/sections", {
                    "name": title, "type": kind, "scanner": scanner, "agent": PLEX_IDENTIFIER,
                    "metadataAgentProviderGroupId": group_id, "language": "en-US", "location": location,
                })
        sections = {s.get("title"): s for s in plex_wait_scan(base, len(sections) + wanted)}
        for title in ("Series by hand AMS", "Films by hand AMS"):
            expect(title in sections, f"no library {title}")
            expect(sections[title].get("agent") == PLEX_IDENTIFIER, f"{title} uses the agent {sections[title].get('agent')!r}")
            expect(sections[title].get("metadataAgentProviderGroupId") == group_id, f"{title} is not on the group")
        hand = {"series": series, "movie": film, "series_key": sections["Series by hand AMS"].get("key"), "film_key": sections["Films by hand AMS"].get("key")}
    with R.check("plex: the series entered by hand is matched as show-id-…, with the title, summary, season and episodes the catalogue holds"):
        if not hand:
            raise Skipped("no library for the works entered by hand")
        series = hand["series"]
        shows = eventually(
            lambda: plex_get(base, f"/library/sections/{hand['series_key']}/all").findall("Directory"),
            lambda found: len(found) == 1 and found[0].get("guid", "").startswith(PLEX_IDENTIFIER) and found[0].get("summary"),
        )
        expect(len(shows) == 1, f"the library holds {len(shows)} series: {[(s.get('title'), s.get('guid')) for s in shows]}")
        rating = f"show-id-{series['id']}"
        ours = provider_json(f"/library/metadata/{rating}")
        show = refreshed(shows[0].get("ratingKey"), "Directory", "title", ours["title"])
        expect(show.get("guid") == f"{PLEX_IDENTIFIER}://show/{rating}", f"guid {show.get('guid')!r}")
        expect(show.get("title") == ours["title"] == HAND_SERIES, f"title {show.get('title')!r}, served {ours['title']!r}")
        expect(show.get("year") == str(ours["year"]) == str(HAND_SERIES_YEAR), f"year {show.get('year')!r}")
        expect(show.get("summary") == ours["summary"] and ours["summary"], "the summary is not the catalogue's")
        seasons = plex_get(base, f"/library/metadata/{show.get('ratingKey')}/children").findall("Directory")
        expect(len(seasons) == 1 and seasons[0].get("index") == "1", f"Plex lists {len(seasons)} seasons: {[s.get('title') for s in seasons]}")
        season_ours = provider_json(f"/library/metadata/season-id-{series['id']}-s1")
        expect(seasons[0].get("title") == season_ours["title"], f"season title {seasons[0].get('title')!r}, served {season_ours['title']!r}")
        episodes = eventually(
            lambda: plex_get(base, f"/library/metadata/{seasons[0].get('ratingKey')}/children").findall("Video"),
            lambda found: len(found) == len(HAND_EPISODES) and all(e.get("summary") for e in found),
        )
        expect(len(episodes) == len(HAND_EPISODES), f"Plex lists {len(episodes)} episodes")
        typed = {str(n): title for n, title, _ in HAND_EPISODES}
        for episode in episodes:
            number = episode.get("index")
            theirs = provider_json(f"/library/metadata/episode-id-{series['id']}-s1e{number}")
            expect(episode.get("title") == theirs["title"] == typed.get(number), f"episode {number}: title {episode.get('title')!r}, served {theirs['title']!r}, typed {typed.get(number)!r}")
            expect(episode.get("summary") == theirs.get("summary") and episode.get("summary"), f"episode {number}: the summary is not the catalogue's")
        # Nothing elsewhere names it, so no id is listed for Plex to map it by: one made
        # up for it would be taken for another work's.
        named = [g.get("id") for g in show.findall("Guid")]
        expect(not named, f"Plex lists the ids {named} for a series nothing elsewhere names")
        R.note(f"plex: {show.get('title')}: {[e.get('title') for e in episodes]}")
    with R.check("plex: the film entered by hand is matched as movie-id-…, with the title, year and summary the catalogue holds"):
        if not hand:
            raise Skipped("no library for the works entered by hand")
        film = hand["movie"]
        videos = eventually(
            lambda: plex_get(base, f"/library/sections/{hand['film_key']}/all").findall("Video"),
            lambda found: len(found) == 1 and found[0].get("guid", "").startswith(PLEX_IDENTIFIER) and found[0].get("summary"),
        )
        expect(len(videos) == 1, f"the library holds {len(videos)} films: {[(v.get('title'), v.get('guid')) for v in videos]}")
        rating = f"movie-id-{film['id']}"
        ours = provider_json(f"/library/metadata/{rating}")
        node = refreshed(videos[0].get("ratingKey"), "Video", "title", ours["title"])
        expect(node.get("guid") == f"{PLEX_IDENTIFIER}://movie/{rating}", f"guid {node.get('guid')!r}")
        expect(node.get("title") == ours["title"] == HAND_FILM, f"title {node.get('title')!r}, served {ours['title']!r}")
        expect(node.get("year") == str(ours["year"]) == str(HAND_FILM_YEAR), f"year {node.get('year')!r}")
        expect(node.get("summary") == ours["summary"] and ours["summary"], "the summary is not the catalogue's")
        named = [g.get("id") for g in node.findall("Guid")]
        expect(not named, f"Plex lists the ids {named} for a film nothing elsewhere names")
        R.note(f"plex: {node.get('title')} ({node.get('year')})")
    with R.check("plex: this server counted the calls Plex made, and refused none"):
        apis = {a["api"]: a for a in native.call("GET", "/admin/access")["apis"]}
        served = apis["plex"]["served"] - served_before
        # A library kept from an earlier run is not scanned again from nothing.
        expect(served >= 1, f"{served} calls counted since the provider was added")
        expect(apis["plex"]["refused"] == 0, f"{apis['plex']['refused']} calls refused")
        R.note(f"plex: {served} calls to /plex answered while Plex scanned")


def phase_plexkey(R, stack):
    """The provider asking for a key — the server restarted with
    AMS_PLEX_AUTH=apikey — and Plex given the address that carries one, on
    the libraries the plex phase made."""
    base = env("E2E_PLEX_URL").rstrip("/")
    provider = env("E2E_PLEX_PROVIDER")
    native = stack.native
    with R.check("plexkey: /plex answers nobody without a key, now that the provider asks for one"):
        try:
            http("GET", f"{native.url}/plex")
        except HttpError as e:
            expect(e.status == 401, f"/plex answers {e.status} without a key")
        else:
            raise Failure("/plex answered without a key")
    made = native.call("POST", "/clients", {"name": "Plex (e2e stack)", "scopes": ["read"]})
    key = made["key"]
    try:
        node_id = None
        with R.check("plexkey: Plex is given the address with a key in it, and finds the provider answering there"):
            known = [p for p in plex_get(base, "/media/providers/metadata").findall("MetadataAgentProvider") if p.get("identifier") == PLEX_IDENTIFIER]
            expect(known, "no provider left by the plex phase")
            node_id = known[0].get("id")
            node = plex_put(base, f"/media/providers/metadata/{node_id}", {"uri": f"{provider}/{key}"}).find("MetadataAgentProvider")
            expect(node is not None and node.get("online") == "1", "Plex says the provider is offline at the address with the key")
        with R.check("plexkey: once Plex is restarted, a title locked here reaches it through that address"):
            if node_id is None:
                raise Skipped("Plex took no address with a key")
            # Plex's agents still call the address the provider had when Plex
            # started — /plex, refused now — until Plex is restarted.
            plex_restarted(base)
            sections = {s.get("title"): s for s in plex_get(base, "/library/sections").findall("Directory")}
            expect("Series AMS" in sections, "no library Series AMS: the plex phase runs first")
            series = plex_get(base, f"/library/sections/{sections['Series AMS'].get('key')}/all").findall("Directory")
            expect(series, "the series library is empty")
            item = native.item("series", "tvdb", 81189)
            expect(item is not None, "tvdb:81189 is not in the catalogue")
            text = "Locked by the e2e stack (plexkey)"
            native.lock_title(item["id"], text)
            try:
                node = plex_refreshed(base, series[0].get("ratingKey"), "Directory", "title", text)
                expect(node.get("title") == text, f"Plex's title is {node.get('title')!r}, not the lock {text!r}")
            finally:
                native.call("DELETE", f"/items/{item['id']}/overrides/item/title")
            plex_refreshed(base, series[0].get("ratingKey"), "Directory", "title", "Breaking Bad")
        with R.check("plexkey: an address with a key this server never issued is not answered"):
            if node_id is None:
                raise Skipped("Plex took no address with a key")
            try:
                node = plex_put(base, f"/media/providers/metadata/{node_id}", {"uri": f"{provider}/ams_neverissued"}).find("MetadataAgentProvider")
                expect(node is None or node.get("online") != "1", "Plex finds the provider answering with a key never issued")
            except HttpError as e:
                expect(e.status == 400, f"Plex answered {e.status}")
            finally:
                plex_put(base, f"/media/providers/metadata/{node_id}", {"uri": f"{provider}/{key}"})
        with R.check("plexkey: the server's log holds the key nowhere"):
            with open(env("E2E_SERVER_LOG"), encoding="utf-8", errors="replace") as log:
                expect(key not in log.read(), "the key is written in the server's log")
    finally:
        native.call("DELETE", f"/clients/{made['id']}")


# ── Main ───────────────────────────────────────────────────────────────────


PHASES = ("sonarr4", "sonarr5", "radarr", "push", "seed", "plex", "plexkey")


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
            elif phase == "plexkey" and os.environ.get("E2E_PLEX_URL"):
                phase_plexkey(report, stack)
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
