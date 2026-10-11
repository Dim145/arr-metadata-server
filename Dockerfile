# syntax=docker/dockerfile:1.7
#
# Three stages: build the UI, build the server with the UI embedded in it, then
# ship the single resulting binary on a distroless base — no shell, no package
# manager, and a non-root user by default.

# ── 1. Web UI ────────────────────────────────────────────────────────────────
FROM node:24-trixie-slim AS ui

WORKDIR /ui

# Dependencies first, so a source-only change does not reinstall them.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund

COPY frontend/ ./
RUN npm run build


# ── 2. Server ────────────────────────────────────────────────────────────────
FROM rust:1.98-trixie AS server

WORKDIR /src

# `cargo auditable` writes the list of crates a binary was built from into the
# binary itself. That is how a scanner, or `cargo audit bin`, can say what is
# inside a container that has no package database for Rust, and how the SBOM
# the registry keeps beside the image lists them. Installed ahead of everything
# that changes, so that it is compiled once and the layer kept; the version is
# named because this runs in every build, and `--locked` keeps its own
# dependencies to the ones it was released with.
RUN cargo install --locked --version 0.7.7 cargo-auditable

# Compile the dependency tree against a stub so it caches independently of the
# application sources, which change far more often.
COPY Cargo.toml Cargo.lock build.rs ./
RUN mkdir -p src \
 && echo 'fn main() {}' > src/main.rs \
 && cargo auditable build --release --locked \
 && rm -rf src

COPY src/ ./src/
COPY migrations/ ./migrations/
# Served to the clients as the way to trust the authority; embedded at
# compile time, so the build needs it beside the sources.
COPY docker/trust-ca.sh ./docker/trust-ca.sh
COPY --from=ui /ui/dist/ ./frontend/dist/

# `touch` defeats the stale mtime left by the stub build.
RUN touch src/main.rs build.rs \
 && cargo auditable build --release --locked \
 && strip target/release/arr-metadata-server \
 && mkdir -p /out/data

# The dependency list has to have come through the strip above: without it the
# image would say nothing about what is inside, and nothing would complain.
RUN readelf -S target/release/arr-metadata-server | grep -q '\.dep-v0' \
 || { echo 'the dependency list (.dep-v0) is not in the binary' >&2; exit 1; }


# ── 3. Runtime ───────────────────────────────────────────────────────────────
FROM gcr.io/distroless/cc-debian13:nonroot AS runtime

COPY --from=server /src/target/release/arr-metadata-server /usr/local/bin/arr-metadata-server

# uid 65532 is distroless's `nonroot`, and /data is its from the start: a named
# volume takes that ownership over when it is made, and a bind-mounted directory
# must be given it — `chown 65532:65532 ./data` on the host.
COPY --from=server --chown=65532:65532 /out/data /data
WORKDIR /data
# By number, so that a host or an orchestrator that checks for a non-root
# user can tell without the image's passwd file.
USER 65532:65532

# Everything the server keeps, under the one directory a volume mounts:
# the relative defaults would land under /data/data otherwise.
ENV AMS_BIND_ADDRESS=0.0.0.0:8080 \
    AMS_DATABASE_URL=sqlite:///data/ams.db?mode=rwc \
    AMS_TLS_DIR=/data/tls \
    AMS_MEDIA_DIR=/data/media \
    AMS_LOG=info

EXPOSE 8080 443

# There is no shell here; the binary probes itself.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/usr/local/bin/arr-metadata-server", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/arr-metadata-server"]
