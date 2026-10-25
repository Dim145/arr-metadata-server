mod api;
mod arr;
mod auth;
mod backup;
mod cache;
mod cli;
mod config;
mod coord;
mod db;
mod domain;
mod error;
mod export;
mod jobs;
mod mcp;
mod media;
mod merge;
mod metrics;
mod outbound;
mod providers;
mod search;
mod service;
mod settings;
mod state;
mod telemetry;
mod tls;
mod ui;
mod web;
mod wire;

use clap::Parser as _;

/// The allocator: a server of many small, short-lived allocations across
/// many threads, which this one serves faster and with less fragmentation
/// than the system's.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// What `--help` says after the commands: how each is used, with the forms
/// the distroless image needs. The commands themselves are in `cli`.
const USAGE: &str = "\
With no command, the server runs.

  transfer takes two connection URLs and copies every row of the first into
  the second, which must be empty: its migrations are applied first.

    arr-metadata-server transfer \\
        'sqlite://data/ams.db' \\
        'postgres://ams:secret@localhost/ams'

  backup writes an archive of the database where the server keeps them
  (AMS_BACKUP_DIR, or the bucket among several instances); restore puts one
  back, and is refused while a server runs on the database. A password or a
  passphrase is read from standard input or asked for, never an argument.

    arr-metadata-server backup --include-media
    arr-metadata-server restore ams-20261019-020000-scheduled.tar.zst --replace
    printf '%s\\n' \"$PASSWORD\" | arr-metadata-server user reset-password alice --password-stdin

  The image has no shell, and the binary is its entrypoint: run a command in
  the container that serves, or in one of its own on the same volume and
  environment while the server is stopped.

    docker exec -it arr-metadata-server /usr/local/bin/arr-metadata-server user list
    docker compose stop metadata
    docker compose run --rm metadata restore ams-20261019-020000-scheduled.tar.zst --replace
";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // nosemgrep: rust.lang.security.args.args — the binary's own subcommands, parsed against a fixed grammar
    cli::run(cli::Cli::parse()).await
}
