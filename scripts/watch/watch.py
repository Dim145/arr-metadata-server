#!/usr/bin/env python3
"""The upstream watch: what this server imitates, relays or downloads, read
against what it was when last looked at.

Those contracts move without notice — AniList deprecated `averageScore`
without a word, Skyhook grew a route by TMDB id, TheTVDB prepares a new
version of its API — and this server learns of it when a client breaks.
Each check reads one of them and sets it beside its snapshot, under
snapshots/:

  anilist   AniList's schema: the fields this server and its known clients
            read, the deprecated, the gone
  skyhook   the keys of four of Skyhook's documents, and the last change to
            Sonarr's client of it
  thetvdb   the version of TheTVDB's API description, the last commit of its
            repository, its open pull requests
  tmdb      the date and the OpenAPI definition of each page of TMDB's
            reference
  datasets  the age of the anime list this server downloads and of the
            database it is built from, and the keys of its entries

    python3 scripts/watch/watch.py                   every check
    python3 scripts/watch/watch.py anilist skyhook   these two
    python3 scripts/watch/watch.py --update tmdb     take TMDB's snapshot again

Exit status: 0 when nothing moved; 1 when something did, or a list is older
than this server can live with; 2 when an upstream could not be read. With
--update, what moved is accepted into the snapshot: look at it first.

Needs python3 and the network, nothing else. GitHub's API is read without a
token, sixty calls an hour; GITHUB_TOKEN or GH_TOKEN, when set, is sent to
api.github.com and nowhere else.
"""

import argparse
import sys

# Nothing written beside the scripts: they run from the checkout as they are.
sys.dont_write_bytecode = True

import anilist  # noqa: E402
import datasets  # noqa: E402
import skyhook  # noqa: E402
import thetvdb  # noqa: E402
import tmdb  # noqa: E402
from common import MOVED, OK, UNREADABLE  # noqa: E402

CHECKS = {
    "anilist": anilist.run,
    "skyhook": skyhook.run,
    "thetvdb": thetvdb.run,
    "tmdb": tmdb.run,
    "datasets": datasets.run,
}


def main():
    parser = argparse.ArgumentParser(
        description="Read what this server imitates, relays or downloads, against its snapshots."
    )
    parser.add_argument("--update", action="store_true", help="take the snapshots of the checks run again")
    parser.add_argument("checks", nargs="*", metavar="check", help=", ".join(CHECKS) + "; all when none")
    arguments = parser.parse_args()
    unknown = [name for name in arguments.checks if name not in CHECKS]
    if unknown:
        parser.error(f"no such check: {', '.join(unknown)}; the checks are {', '.join(CHECKS)}")

    statuses = []
    for name in arguments.checks or CHECKS:
        if statuses:
            print()
        statuses.append(CHECKS[name](arguments.update))
    if MOVED in statuses:
        return MOVED
    return UNREADABLE if UNREADABLE in statuses else OK


if __name__ == "__main__":
    sys.exit(main())
