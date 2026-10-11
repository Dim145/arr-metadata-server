# Security policy

## Supported versions

Only the latest minor release is supported, and a fix is released as a new patch
of that line. While the major version is 0 a minor release may change what the
API or the configuration means, and the [changelog](CHANGELOG.md) says so: to
get a fix, move to the latest minor, not to a patched copy of an older one.

## Reporting a vulnerability

Report it privately, through GitHub's private vulnerability reporting: on the
repository's **Security** tab, choose **Report a vulnerability**
([direct link](https://github.com/Dim145/arr-metadata-server/security/advisories/new)).
Please do not open a public issue or a discussion about it, and do not publish
the details before a fix is out.

Say what is needed to reproduce it: the version, the database engine (SQLite or
PostgreSQL), whether it runs as a single instance or several, the configuration
that matters (without secrets), and the steps. A proof of concept helps. An
attack on somebody else's server does not.

What to expect, as an intention and not a promise of service: an
acknowledgement within 7 days; then an assessment — confirmed, not reproduced,
or out of scope — and news as the fix progresses; then a release, published
together with a GitHub security advisory that credits you unless you would
rather it did not.

## Scope

In scope is the server and its image:

- the server itself: the web interface, the native API, the Sonarr, Radarr and
  TMDB-compatible surfaces, the clients' TLS door, sign-in, sessions, API keys,
  the media store;
- the container image published at `ghcr.io/dim145/arr-metadata-server`, and
  the workflows in this repository that build and publish it.

Out of scope:

- the third-party services the server talks to or stands in for — TMDB,
  TheTVDB, AniList, the IMDb datasets, Sonarr's and Radarr's own metadata
  services, identity providers. Report those to their owners;
- how somebody deploys it: the reverse proxy in front, the firewall, exposing
  it with authentication switched off on purpose, a weak administrator
  password;
- denial of service by sheer volume. The rate limits exist and are
  configurable.

A flaw in a dependency is in scope when it can be exploited through this
server. When it cannot, report it to the dependency, and tell us as well.

## Checking what you run

The image is signed and carries its provenance and an SBOM, and the binary
lists the crates it was built from. "Verifying the image" in the
[README](README.md#verifying-the-image) has the commands.
