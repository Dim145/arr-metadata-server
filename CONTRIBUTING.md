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

A release build optimises the whole program at once (`lto = "fat"`), and its
last step wants some 4 GB: more than a Docker VM shared with other containers
may have. On your machine, ask for thin LTO, a little larger and as fast in
practice; CI and the published image keep the whole-program one:

```bash
CARGO_PROFILE_RELEASE_LTO=thin cargo build --release --features avif-asm
docker build --build-arg CARGO_PROFILE_RELEASE_LTO=thin -t arr-metadata-server:local .
```

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

## The interface's design system

**Tokens.** The colours, the steps of type and the corners are named in
[`frontend/src/styles.css`](frontend/src/styles.css) (`@theme`), each with what
it is for. A page asks for what a colour says rather than the colour:
`text-status-error` for something that failed, `status-ok` for done or on,
`status-warn` for what wants a look — vermillion stays the way through the
building, brass a person's hand, slate a machine's answer. The lint refuses a
bracketed value where a token says it (`text-[0.75rem]` is `text-caption`,
`rounded-[6px]` is `rounded-md`); `npx eslint --fix` writes the token in.

**States.** Every component in `frontend/src/components/ui/` says what it looks
like at rest, under the pointer, reached by the keyboard, pressed, disabled,
waiting, failed and with nothing to show — or whose state it is instead. A new
component, or a new state, says so too, and gets its row in the styleguide.

**The styleguide** draws every component in each of those states, in both themes
and both languages, side by side: `/__styleguide` on the Vite dev server
(`npm run dev`), or in a build made with `npx vite build --mode styleguide`.
What ships never has it.

**The visual tests** (`frontend/e2e/visual.spec.ts`) hold every gallery of the
styleguide, and a few pages as a visitor and an administrator first meet them,
to the pictures in `frontend/e2e/__screenshots__/`. Those pictures are drawn in
one place only, the official Playwright image of the version
`frontend/package-lock.json` pins: type is drawn differently by every system's
fonts, so on anything but Linux the spec stands aside. They want the styleguide
build, served by the debug binary on a new database (the pages are drawn on an
empty catalogue, within ten minutes of the start), with its administrator named
`admin` — CI's `visual` job, in [`.github/workflows/ci.yml`](.github/workflows/ci.yml),
starts it so. Then, from `frontend/`:

```bash
docker run --rm --ipc=host --network host -v "$PWD/..:/work" -w /work/frontend \
  -e AMS_E2E_URL=http://127.0.0.1:8503 -e AMS_E2E_USER=admin -e AMS_E2E_PASSWORD \
  mcr.microsoft.com/playwright:v1.64.1-noble \
  ./node_modules/.bin/playwright test --project visual
```

On a Mac, Docker's host network is not the Mac's: drop `--network host` and reach
the server at `http://host.docker.internal:8503`. CI runs on x86-64; a Mac with
Apple silicon runs the arm64 image, whose pictures can differ from CI's by a
pixel's shade at an edge, which the comparison lets through.

A change that means to change a picture draws it again: run **Draw the pictures
again** (`visual-update.yml`) from your branch under *Actions*, which opens a pull
request against it with the pictures that changed — or add
`--update-snapshots=changed` to the command above. Look at each picture before it
goes in: from then on it is what the interface is held to.

## Commits and pull requests

One logical change to a commit: a summary in the imperative, in sentence case and
with no prefix (`git log` has the tone), and a body that says why when the diff
cannot. The pull request template is a short checklist of the above; fill it in
rather than delete it.

CI checks a pull request in full, and a push to main too, but for code it has
checked already: a pull request merged by rebase onto the main it was tested
against lands the very tree its run passed with, and a release moves nothing
past a checked commit but the version and the changelog. Its first job, *What
to check*, says which it found; either is checked again in full when it changes
the dependencies, or when main's caches have expired, so that those caches —
which every pull request starts from — follow the dependencies and are there. A pull request that is behind main is therefore worth
rebasing before it is merged: its merge is then not checked a second time.

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
   Push it once CI has passed on the commit before it — CI's clippy is the latest
   stable, which may know a lint yours does not. CI then sees nothing but the
   version and the changelog moved, and checks nothing again; wait for CodeQL
   and Audit.
4. **The tag**, annotated: `git tag -a vX -m "arr-metadata-server X"`, then
   `git push origin vX`. The tag runs *Publish the image*
   (`.github/workflows/docker.yml`): amd64 and arm64, signed, tagged `X`, `X.Y`
   and `latest`. Each platform starts from the dependency layers the last
   release left in the registry (`buildcache-amd64`, `buildcache-arm64`), which
   a change of version alone does not invalidate.
5. **The release page**, once the image is published and not before — a
   running server's update check reads the releases, and would announce a
   version with no image to pull: `gh release create vX --title X`, with the
   changelog's section for `X` as the notes.

Before a minor, two checks CI does not run, each needing Docker and the
internet: `scripts/e2e-stack.sh`, the whole stack with the real software — this
server, Sonarr 4, the latest Sonarr v5 build, Radarr and a Plex Media Server —
and `scripts/e2e-skyhook-v2.sh`, which compares what this server answers Sonarr
v5 with the real Skyhook's second edition, series by series.

**Deprecating.** Something of the API or of the configuration goes in two
minors, never in one, as
[`docs/api.md`](docs/api.md#what-stays-and-how-something-goes) promises. In
the first it is marked in the document — `#[schema(deprecated)]` on a field,
Rust's `#[deprecated]` on a route's handler — with a doc comment that says
what to read instead, so the interface's types leave it out
(`--exclude-deprecated`) and the interface stops reading it first; its answers
carry a `Deprecation` header; the page under `docs/` that describes it says it
is deprecated; and the changelog says under *Deprecated* what goes, what
replaces it, and from which version it is gone. A setting or a variable that
is renamed is read under both names, the old one with a warning in the log. In
a later minor it goes: from the code, from `frontend/openapi.json` and the
generated types (`UPDATE_OPENAPI=1 cargo test openapi`, then `npm run types`),
from the documentation, from whatever under `frontend/src` or `frontend/e2e`
still read it, with a line under *Removed*. What Sonarr, Radarr and Plex read
is never deprecated here: it goes when they stop reading it.
