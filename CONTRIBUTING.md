# Contributing

Thanks for wanting to help. This is a Rust server with a React web interface in
`frontend/`, and a change is easiest to review when it is small, does one thing,
and says why.

For a bigger change, or one that alters what the API or the configuration
means, open an issue or a discussion first: it is cheaper to agree on the shape
before the code than after. A question about running the server belongs in the
discussions, not in the issues. A security problem belongs in neither: see
[`SECURITY.md`](SECURITY.md).

By taking part you agree to the [code of conduct](CODE_OF_CONDUCT.md).

## Before you send a patch

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test

cd frontend
npm ci
npx tsc -b
npx tsc -p e2e --noEmit
npx vite build
```

CI runs the same, with `--locked`, and then applies the migrations to both
database engines, builds the container image and starts it. If you touched
storage, run `scripts/smoke.sh` yourself against SQLite and PostgreSQL, as the
README's *Development* section shows.

The tests under `frontend/e2e/` run Playwright against the **debug binary**, not
the Vite dev server: that binary is what ships, serving the interface and the
API from one origin. Build both, start the binary on port 8479 against a
database named `data/e2e.db`, and run the suite with the credentials of the
administrator the server was started with:

```bash
cd frontend
AMS_E2E_USER=admin AMS_E2E_PASSWORD='choose-one' npm run test:e2e
```

Without those two variables the administration tests skip themselves, and a run
that says "skipped" has not tested them. The suite reads a catalogue instead of
seeding one, so `data/e2e.db` has to hold a few films and series, one of them
with an episode airing this week: import them first (the administration's
**Import**, with `AMS_TMDB_API_KEY` set), and keep the file between runs. The exact
environment the server needs, and what the specs named `*.serial.spec.ts` do to
it, is in [`frontend/e2e/README.md`](frontend/e2e/README.md).

## What a good change looks like

**Migrations.** The schema is kept separately for each engine: a migration is one
file in `migrations/sqlite/` and one in `migrations/postgres/`, with the same
number and name, doing the same thing in each engine's own dialect. Never edit a
migration that has been committed: databases that already applied it would no
longer match it, and startup fails on them. Add a new one.

**Comments.** Full sentences, and they say why: the constraint, the incident or
the surprise the code cannot say for itself — not what the next line does. No
`TODO`: what is left undone is an issue. No commented-out code.

**The changelog.** [`CHANGELOG.md`](CHANGELOG.md) follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). A change that someone
running the server would notice gets a line under `## [Unreleased]`, in *Added*,
*Changed*, *Deprecated*, *Removed*, *Fixed* or *Security*, written for that
person and not for whoever wrote the code.

**Language.** English in the code, the comments, the documentation, the commit
messages and the issues. The interface itself is translated: its strings go in
`frontend/src/lib/lang/en.ts`, the source of truth, and in `fr.ts`, which ships
complete; [`frontend/src/lib/lang/README.md`](frontend/src/lib/lang/README.md)
also says how to add a language.

**Rust version.** The minimum is the `rust-version` in `Cargo.toml`, and CI
builds with exactly that: do not lean on anything newer.

**Dependencies.** A change to them has to pass `cargo deny check`
(`cargo install --locked cargo-deny`). It refuses a licence that
[`deny.toml`](deny.toml) does not list, a crate from anywhere but crates.io and
a wildcard version, and it reports what is in the tree twice. `cargo audit`
reads `Cargo.lock` against the RustSec advisories, with
[`.cargo/audit.toml`](.cargo/audit.toml) for the ones that are set aside, and
why. Say in the pull request why a new dependency is worth carrying.

**Secrets.** None, in the code, the tests or the fixtures.
`gitleaks detect --source . --redact` reads the history with
[`.gitleaks.toml`](.gitleaks.toml).

## Commits and pull requests

One logical change to a commit: a summary in the imperative, in sentence case and
with no prefix (`git log` has the tone), and a body that says why when the diff
cannot. The pull request template is a short checklist of the above; fill it in
rather than delete it.

## Releasing

A release is a commit, a tag and a release page, in that order, each waiting
on the one before.

1. **The version**, in three places: `version` in `Cargo.toml` (`Cargo.lock`
   follows on the next `cargo build`); `frontend/package.json` and its
   lockfile, which `npm version X --no-git-tag-version` run in `frontend/`
   writes together; and the placeholder of the version field in
   `.github/ISSUE_TEMPLATE/bug_report.yml`.
2. **The changelog**: the `## [Unreleased]` section becomes `## [X] — YYYY-MM-DD`,
   an empty `## [Unreleased]` goes above it, and the compare links at the foot
   gain the new version's line, `[Unreleased]` now comparing from `vX`.
3. **The commit**, "Release X", with a prose summary of what the release holds.
   Push it and wait for CI, CodeQL and Audit to pass on that commit: CI's clippy
   is the latest stable, which may know a lint yours does not.
4. **The tag**, annotated: `git tag -a vX -m "arr-metadata-server X"`, then
   `git push origin vX`. The tag runs *Publish the image*
   (`.github/workflows/docker.yml`): amd64 and arm64, signed, tagged `X`, `X.Y`
   and `latest`.
5. **The release page**, once the image is published and not before — a
   running server's update check reads the releases, and would announce a
   version with no image to pull: `gh release create vX --title X`, with the
   changelog's section for `X` as the notes.

Before a minor, two checks CI does not run, each needing Docker and the
internet: `scripts/e2e-stack.sh`, the whole stack with the real software — this
server, Sonarr 4, the latest Sonarr v5 build, Radarr and a Plex Media Server —
and `scripts/e2e-skyhook-v2.sh`, which compares what this server answers Sonarr
v5 with the real Skyhook's second edition, series by series.
