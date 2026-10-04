#!/usr/bin/env python3
"""A stand-in for TheTVDB's v4 API and AniList's GraphQL, for the relays' run.

Serves TheTVDB under /v4 and AniList at the root, as the real services do,
and answers with documents whose every field says "Upstream": what the
relays rewrite shows against it. It records what it is asked — the path,
the credential, the body — so the run can say what reached it and what did
not, and takes a few orders:

    GET  /_fake/requests      what was asked since the last reset, as JSON
    POST /_fake/reset         forget it
    POST /_fake/expire-token  the next call with the operator's token is a 401
    POST /_fake/ratelimit     the next AniList query is a 429, with its headers

    e2e-relays-upstream.py <port> <operator's TheTVDB key>
"""

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

OPERATOR_KEY = sys.argv[2] if len(sys.argv) > 2 else "operator-tvdb-key"

state = {"requests": [], "expire_once": False, "ratelimit_once": False}
lock = threading.Lock()


def token_for(key):
    return f"fake-token-for-{key}"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def _body(self):
        length = int(self.headers.get("content-length") or 0)
        return self.rfile.read(length) if length else b""

    def _send(self, status, payload, headers=None):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(data)

    def _record(self, body):
        with lock:
            state["requests"].append(
                {
                    "method": self.command,
                    "path": self.path,
                    "authorization": self.headers.get("authorization"),
                    "x-api-key": self.headers.get("x-api-key"),
                    "cookie": self.headers.get("cookie"),
                    "user-agent": self.headers.get("user-agent"),
                    "x-ams-instance": self.headers.get("x-ams-instance"),
                    "body": body.decode("utf-8", "replace"),
                }
            )

    def do_GET(self):
        self._handle()

    def do_POST(self):
        self._handle()

    def _handle(self):
        body = self._body()
        path = urlparse(self.path).path
        if path.startswith("/_fake/"):
            return self._control(path)
        self._record(body)
        if path.startswith("/v4/"):
            return self._tvdb(path, body)
        return self._anilist(body)

    def _control(self, path):
        if path == "/_fake/requests":
            with lock:
                return self._send(200, state["requests"])
        if path == "/_fake/reset":
            with lock:
                state["requests"] = []
            return self._send(200, {})
        if path == "/_fake/expire-token":
            with lock:
                state["expire_once"] = True
            return self._send(200, {})
        if path == "/_fake/ratelimit":
            with lock:
                state["ratelimit_once"] = True
            return self._send(200, {})
        return self._send(404, {})

    # ── TheTVDB ────────────────────────────────────────────────────────────
    def _tvdb(self, path, body):
        if path == "/v4/login":
            try:
                asked = json.loads(body or b"{}")
            except ValueError:
                return self._send(400, {"status": "failure", "message": "not json"})
            key = asked.get("apikey")
            if not key:
                return self._send(401, {"status": "failure", "message": "InvalidAPIKey"})
            return self._send(200, {"status": "success", "data": {"token": token_for(key)}})

        auth = self.headers.get("authorization") or ""
        token = auth[7:] if auth.lower().startswith("bearer ") else ""
        with lock:
            expire = state["expire_once"] and token == token_for(OPERATOR_KEY)
            if expire:
                state["expire_once"] = False
        if expire or not token.startswith("fake-token-for-"):
            return self._send(401, {"status": "failure", "message": "Unauthorized", "data": None})

        parts = path[len("/v4/"):].strip("/").split("/")
        if parts[0] == "series" and len(parts) >= 2 and parts[1].isdigit():
            sid = int(parts[1])
            if len(parts) >= 4 and parts[2] == "translations":
                return self._send(
                    200,
                    {"status": "success", "data": {"name": "Upstream Name", "overview": "Upstream overview", "language": parts[3]}},
                )
            if len(parts) >= 4 and parts[2] == "episodes":
                episodes = [
                    {"id": sid * 10 + n, "seriesId": sid, "seasonNumber": 1, "number": n, "name": f"Upstream E{n}", "aired": f"2026-01-0{n}"}
                    for n in (1, 2)
                ]
                return self._send(
                    200,
                    {
                        "status": "success",
                        "data": {"series": {"id": sid, "name": "Upstream Name"}, "episodes": episodes},
                        "links": {"prev": None, "next": None, "self": self.path, "total_items": 2, "page_size": 500},
                    },
                )
            return self._send(
                200,
                {
                    "status": "success",
                    "data": {
                        "id": sid,
                        "name": "Upstream Name",
                        "overview": "Upstream overview",
                        "year": "2020",
                        "averageRuntime": 24,
                        "status": {"id": 1, "name": "Continuing", "recordType": "series", "keepUpdated": True},
                        "image": "https://artworks.thetvdb.com/banners/posters/upstream.jpg",
                        "remoteIds": [{"id": "1396", "type": 12, "sourceName": "TheMovieDB.com"}],
                    },
                },
            )
        if parts[0] == "episodes" and len(parts) >= 2 and parts[1].isdigit():
            eid = int(parts[1])
            return self._send(
                200,
                {"status": "success", "data": {"id": eid, "seriesId": eid // 10, "seasonNumber": 1, "number": eid % 10, "name": f"Upstream E{eid % 10}"}},
            )
        if parts[0] == "user":
            return self._send(200, {"status": "success", "data": {"id": 1, "name": "operator"}})
        if parts[0] == "search":
            return self._send(200, {"status": "success", "data": [{"objectID": "series-1", "name": "Upstream Hit"}]})
        return self._send(200, {"status": "success", "data": {}})

    # ── AniList ────────────────────────────────────────────────────────────
    def _anilist(self, body):
        with lock:
            limited = state["ratelimit_once"]
            state["ratelimit_once"] = False
        if limited:
            return self._send(
                429,
                {"errors": [{"message": "Too Many Requests.", "status": 429}]},
                {"retry-after": "3", "x-ratelimit-limit": "90", "x-ratelimit-remaining": "0"},
            )
        query = ""
        if body:
            try:
                query = json.loads(body).get("query", "")
            except (ValueError, AttributeError):
                query = ""
        else:
            query = parse_qs(urlparse(self.path).query).get("query", [""])[0]
        if query.strip().lower().startswith("mutation"):
            return self._send(200, {"data": {"SaveMediaListEntry": {"id": 1, "status": "CURRENT"}}})
        if "MediaListCollection" in query:
            # Somebody's list, as Yamtrack's import asks for it: the entries
            # name MyAnimeList's id and never AniList's.
            return self._send(
                200,
                {
                    "data": {
                        "MediaListCollection": {
                            "lists": [
                                {
                                    "name": "Completed",
                                    "entries": [
                                        {
                                            "status": "COMPLETED",
                                            "media": {
                                                "title": {"userPreferred": "Upstream Romaji"},
                                                "coverImage": {"large": "https://s4.anilist.co/upstream-l.jpg"},
                                                "idMal": 16498,
                                                "episodes": 25,
                                            },
                                        }
                                    ],
                                }
                            ]
                        }
                    }
                },
            )
        return self._send(
            200,
            {
                "data": {
                    "Media": {
                        "id": 16498,
                        "type": "ANIME",
                        "title": {"romaji": "Upstream Romaji", "english": "Upstream English", "userPreferred": "Upstream Romaji", "native": "進撃の巨人"},
                        "description": "Upstream description",
                        "coverImage": {"extraLarge": "https://s4.anilist.co/upstream.jpg", "large": "https://s4.anilist.co/upstream-l.jpg"},
                        "genres": ["Action"],
                    }
                }
            },
        )


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
