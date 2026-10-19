//! The command line: the server, and what an operator does to it from a
//! shell — or from `docker exec`, the image having no shell of its own.
//!
//! One binary, `serve` when no command is named, so that the image's
//! entrypoint and every compose file go on starting the server. Each command
//! reads the configuration the server reads — the same variables, a `.env`
//! beside it — and opens the same database. What changes accounts, keys or
//! locks goes through the server's own API, in this process, as an
//! administrator the journal calls `client:command line`: the same checks,
//! the same journal, the same webhooks as the interface. Passwords and
//! passphrases are read from standard input or asked for at the terminal,
//! never taken as an argument, which a process list and a shell's history
//! would keep.

mod admin;
mod archive;
mod check;
mod migrate;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{config, db, state::AppState};

#[derive(Debug, Parser)]
#[command(
    name = "arr-metadata-server",
    version,
    about = "A metadata server for the *arr stack",
    after_long_help = crate::USAGE
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server: what the binary does when no command is named.
    Serve,
    /// Probe a running server over loopback; exit non-zero if it is unwell.
    Healthcheck,
    /// Copy a database to another engine, every row of it.
    #[command(
        long_about = "Copy a database to another engine, every row of it.\n\nThe target is \
                      migrated first and must be empty, unless --force adds to what it holds."
    )]
    Transfer {
        /// The database to copy, as a connection URL.
        from: String,
        /// The database to copy it into, as a connection URL.
        to: String,
        /// Add to a target that already holds rows.
        #[arg(long)]
        force: bool,
    },
    /// Make an archive of the database — and of the media kept on disk, when
    /// asked — where the server keeps them.
    Backup {
        /// Write it there instead, a file or a directory to name it in; it is
        /// then not listed on the Backups page.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// Put the media kept on disk in it.
        #[arg(long)]
        include_media: bool,
        /// Encrypt it with a passphrase read from standard input, rather
        /// than AMS_BACKUP_PASSPHRASE.
        #[arg(long)]
        passphrase_stdin: bool,
    },
    /// Restore an archive into the database. Refused while a server runs on
    /// it: restore from the server's Backups page then.
    Restore {
        /// The archive: a file, or the name of one the server keeps.
        archive: String,
        /// Restore over a database that holds rows, once an archive of it is
        /// made.
        #[arg(long)]
        replace: bool,
        /// Read the archive's passphrase from standard input, rather than
        /// AMS_BACKUP_PASSPHRASE.
        #[arg(long)]
        passphrase_stdin: bool,
    },
    /// The accounts.
    #[command(subcommand)]
    User(UserCommand),
    /// The configuration, as the server reads it.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// The database's migrations.
    #[command(subcommand)]
    Migrate(MigrateCommand),
    /// The server's own API keys.
    #[command(subcommand)]
    Keys(KeysCommand),
    /// The locks — every manual edit — as a document.
    #[command(subcommand)]
    Locks(LocksCommand),
    /// The version, and the last migration and archive layout it knows.
    Version,
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Every account, with its role and status.
    List,
    /// Open an account.
    Create {
        /// What it signs in as.
        username: String,
        /// What it may do.
        #[arg(long, value_enum, default_value_t = Role::Member)]
        role: Role,
        /// What it is called in the interface.
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[command(flatten)]
        password: Password,
    },
    /// Give an account a new password; its sessions are closed.
    ResetPassword {
        /// The account, by what it signs in as.
        username: String,
        #[command(flatten)]
        password: Password,
    },
    /// Make an account an active administrator — the way back in for one
    /// who lost every other — or, with --revoke, a member again.
    SetAdmin {
        /// The account, by what it signs in as.
        username: String,
        /// Make it a member again.
        #[arg(long)]
        revoke: bool,
    },
}

/// Where a password comes from: standard input, the terminal, or the server.
#[derive(Debug, Args)]
pub struct Password {
    /// Read the password from standard input.
    #[arg(long, conflicts_with = "generate_password")]
    pub password_stdin: bool,
    /// Have one made, and shown once.
    #[arg(long)]
    pub generate_password: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Role {
    Member,
    Editor,
    Admin,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Editor => "editor",
            Self::Admin => "admin",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the configuration as the server reads it, its secrets masked,
    /// and probe the database, the bucket, the archives' place and the
    /// public address.
    Check,
}

#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    /// The migrations the database has had, and those this binary would add.
    Status,
    /// Apply this binary's migrations. Refused while a server runs on the
    /// database: a server applies them itself when it starts.
    Run,
}

#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// Issue a key of the server's own, shown once.
    Create {
        /// What it is called on Keys & network: what calls with it.
        name: String,
        /// What it may do: read, write, admin or metrics. Given once a scope.
        #[arg(long = "scope", value_name = "SCOPE", required = true)]
        scopes: Vec<String>,
        /// When it stops working: a date, or an RFC 3339 instant.
        #[arg(long)]
        expires: Option<String>,
        /// A word on what it is for.
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum LocksCommand {
    /// Write every lock to a file, or to standard output.
    Export {
        /// The file to write: standard output when left out.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Set the locks of a document on the works held here.
    Import {
        /// The document: a file, or `-` for standard input.
        file: String,
    },
}

/// Do what the command line says.
pub async fn run(cli: Cli) -> Result<()> {
    let command = cli.command.unwrap_or(Command::Serve);

    // A local .env is a convenience for development; real deployments pass
    // the environment directly and this call is a no-op.
    let _ = dotenvy::dotenv();

    if let Command::Serve = command {
        return serve().await;
    }

    quiet_log();
    // Every command may speak TLS — to PostgreSQL, to a bucket, to the public
    // address — and reqwest and sqlx each link rustls: one provider has to
    // be chosen, or every handshake fails.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    match command {
        Command::Serve => unreachable!("served above"),
        Command::Healthcheck => healthcheck().await,
        Command::Transfer { from, to, force } => transfer(&from, &to, force).await,
        Command::Version => {
            version();
            Ok(())
        }
        Command::Backup {
            out,
            include_media,
            passphrase_stdin,
        } => archive::backup(out, include_media, passphrase_stdin).await,
        Command::Restore {
            archive,
            replace,
            passphrase_stdin,
        } => archive::restore(&archive, replace, passphrase_stdin).await,
        Command::Config(ConfigCommand::Check) => check::run().await,
        Command::Migrate(MigrateCommand::Status) => migrate::status().await,
        Command::Migrate(MigrateCommand::Run) => migrate::run().await,
        Command::User(command) => admin::user(command).await,
        Command::Keys(command) => admin::keys(command).await,
        Command::Locks(command) => admin::locks(command).await,
    }
}

/// Run the server.
async fn serve() -> Result<()> {
    crate::telemetry::init();

    // reqwest and axum-server both link rustls; without an explicit choice the
    // process has two candidate providers and every handshake fails at runtime.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install the rustls crypto provider"))?;

    let config = configuration()?;

    // Held for as long as the server runs, before it migrates anything: a
    // restore or a migration run from the command line is refused meanwhile,
    // and the server does not start while one runs.
    let _held = crate::backup::lock::hold_shared(&config.database).await?;

    let state = AppState::bootstrap(config).await?;

    crate::web::serve(state).await
}

/// The configuration the server reads, its secrets kept out of whatever is
/// logged or printed from here on.
fn configuration() -> Result<config::Config> {
    let config = config::Config::from_env().context("invalid configuration")?;
    for secret in config.secrets() {
        crate::telemetry::redact_also(&secret);
    }
    Ok(config)
}

/// A command's log: errors alone, on standard error, so that what a command
/// prints — a document, a key — is all there is on standard output, and
/// what the server says of its configuration as it builds itself is not
/// said again at every command. `AMS_LOG` says otherwise when it is set.
fn quiet_log() {
    use std::io::IsTerminal as _;
    let filter = tracing_subscriber::EnvFilter::try_from_env("AMS_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init();
}

/// The database, opened as the server opens it, without migrating it.
async fn open_database(config: &config::Config) -> Result<db::Db> {
    db::Db::connect(&config.database)
        .await
        .context("cannot open the database")
}

/// The whole server, as the server builds it, for a command that goes
/// through its API — refused when the database's migrations are not this
/// binary's, which building it would otherwise apply under a server still
/// running an older one.
async fn server(config: config::Config) -> Result<AppState> {
    let db = open_database(&config).await?;
    migrate::ensure_current(&db).await?;
    db.close().await;
    AppState::bootstrap(config).await
}

/// Copy one database into another.
///
/// Deliberately a separate command rather than something the server does at
/// startup: moving data is a decision, and doing it implicitly on a config
/// change would be the kind of surprise nobody wants from a database.
async fn transfer(from: &str, to: &str, force: bool) -> Result<()> {
    let open = |url: &str| config::Database {
        url: url.to_string(),
        max_connections: 4,
        acquire_timeout: std::time::Duration::from_secs(30),
    };

    let source = db::Db::connect(&open(from))
        .await
        .context("cannot open the source")?;
    let target = db::Db::connect(&open(to))
        .await
        .context("cannot open the target")?;

    // The source is read only; migrating it would be an unasked-for change.
    target
        .migrate()
        .await
        .context("cannot migrate the target")?;

    let report = db::transfer::run(&source, &target, force).await?;

    for (table, rows) in &report.copied {
        if *rows > 0 {
            println!("  {rows:>8}  {table}");
        }
    }
    println!("\n{} rows copied", report.total());

    source.close().await;
    target.close().await;

    Ok(())
}

/// Probe the running server and exit non-zero if it is unwell.
///
/// The address and the scheme are the server's own, read from the same
/// configuration it was started with — every name `AMS_BIND_ADDRESS` goes
/// by, an empty variable read as unset — and probed over loopback when it
/// listens on every interface, at the address it is bound to otherwise.
async fn healthcheck() -> Result<()> {
    let config = config::Config::from_env().context("invalid configuration")?;
    let url = health_url(config.server.bind, config.server.tls.is_some());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        // The certificate is issued for a provider hostname, not for loopback.
        .danger_accept_invalid_certs(url.starts_with("https://"))
        .build()?;

    let status = client.get(&url).send().await?.status();

    if status.is_success() {
        Ok(())
    } else {
        anyhow::bail!("health check returned {status}")
    }
}

/// Whether a server answers where this configuration would have it listen:
/// the second look a command that must have the database alone takes, for
/// a server it shares this machine's loopback with.
async fn server_answers(config: &config::Config) -> Option<String> {
    let url = health_url(config.server.bind, config.server.tls.is_some());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .danger_accept_invalid_certs(url.starts_with("https://"))
        .build()
        .ok()?;
    let answered = client.get(&url).send().await.ok()?;
    answered.status().is_success().then_some(url)
}

/// Where the health check asks: loopback for a server on every interface —
/// IPv4's, which a dual-stack `[::]` answers too — the bound address for one
/// bound to a single interface.
fn health_url(bind: std::net::SocketAddr, tls: bool) -> String {
    let scheme = if tls { "https" } else { "http" };
    let host = match bind.ip() {
        ip if ip.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        ip => ip.to_string(),
    };
    format!("{scheme}://{host}:{}/health", bind.port())
}

/// What this binary is, and what it reads and writes.
fn version() {
    println!("arr-metadata-server {}", env!("CARGO_PKG_VERSION"));
    println!(
        "migrations   up to {} (sqlite and postgres)",
        db::latest_migration()
    );
    println!("archives     layout {}", crate::backup::manifest::FORMAT);
}

/// A line of standard input, without its end: a password or a passphrase
/// piped in.
fn read_stdin_line(what: &str) -> Result<String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .with_context(|| format!("could not read the {what} from standard input"))?;
    let line = line.trim_end_matches(['\r', '\n']).to_string();
    if line.is_empty() {
        anyhow::bail!("standard input held no {what}");
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
    }

    fn parsed(args: &[&str]) -> Option<Command> {
        Cli::try_parse_from(std::iter::once("arr-metadata-server").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"))
            .command
    }

    #[test]
    fn no_command_runs_the_server() {
        assert!(parsed(&[]).is_none());
        assert!(matches!(parsed(&["serve"]), Some(Command::Serve)));
    }

    #[test]
    fn the_health_check_and_the_transfer_are_called_as_they_always_were() {
        assert!(matches!(
            parsed(&["healthcheck"]),
            Some(Command::Healthcheck)
        ));
        let Some(Command::Transfer { from, to, force }) = parsed(&[
            "transfer",
            "sqlite://data/ams.db",
            "postgres://ams:secret@localhost/ams",
        ]) else {
            panic!("a transfer");
        };
        assert_eq!(
            (from.as_str(), to.as_str(), force),
            (
                "sqlite://data/ams.db",
                "postgres://ams:secret@localhost/ams",
                false
            )
        );
        // `--force` wherever it is put, as before.
        for args in [
            ["transfer", "--force", "sqlite://a.db", "sqlite://b.db"],
            ["transfer", "sqlite://a.db", "sqlite://b.db", "--force"],
        ] {
            assert!(matches!(
                parsed(&args),
                Some(Command::Transfer { force: true, .. })
            ));
        }
    }

    #[test]
    fn a_password_is_never_an_argument() {
        let refused = Cli::try_parse_from([
            "arr-metadata-server",
            "user",
            "create",
            "alice",
            "--password",
            "hunter2hunter2",
        ]);
        assert!(refused.is_err());
        let Some(Command::User(UserCommand::Create {
            username,
            role,
            password,
            ..
        })) = parsed(&[
            "user",
            "create",
            "alice",
            "--role",
            "admin",
            "--password-stdin",
        ])
        else {
            panic!("an account");
        };
        assert_eq!(
            (username.as_str(), role, password.password_stdin),
            ("alice", Role::Admin, true)
        );
        // Asked for on standard input and made by the server at once: no.
        assert!(
            Cli::try_parse_from([
                "arr-metadata-server",
                "user",
                "reset-password",
                "alice",
                "--password-stdin",
                "--generate-password",
            ])
            .is_err()
        );
    }

    #[test]
    fn the_maintenance_commands_read_as_documented() {
        assert!(matches!(
            parsed(&[
                "backup",
                "--out",
                "/backups",
                "--include-media",
                "--passphrase-stdin"
            ]),
            Some(Command::Backup {
                out: Some(_),
                include_media: true,
                passphrase_stdin: true
            })
        ));
        assert!(matches!(
            parsed(&["restore", "ams-20261019-020000-manual.tar.zst", "--replace"]),
            Some(Command::Restore { replace: true, .. })
        ));
        assert!(matches!(
            parsed(&["keys", "create", "scraper", "--scope", "metrics"]),
            Some(Command::Keys(KeysCommand::Create { .. }))
        ));
        // A key with nothing it may do is no key.
        assert!(Cli::try_parse_from(["arr-metadata-server", "keys", "create", "scraper"]).is_err());
        assert!(matches!(
            parsed(&["locks", "import", "-"]),
            Some(Command::Locks(LocksCommand::Import { .. }))
        ));
        for args in [
            ["migrate", "status"],
            ["migrate", "run"],
            ["config", "check"],
        ] {
            assert!(parsed(&args).is_some(), "{args:?}");
        }
        assert!(matches!(parsed(&["version"]), Some(Command::Version)));
    }

    #[test]
    fn the_health_check_asks_where_the_server_listens() {
        assert_eq!(
            health_url("0.0.0.0:8080".parse().unwrap(), false),
            "http://127.0.0.1:8080/health"
        );
        assert_eq!(
            health_url("[::]:9000".parse().unwrap(), true),
            "https://127.0.0.1:9000/health"
        );
        assert_eq!(
            health_url("192.168.1.5:8080".parse().unwrap(), false),
            "http://192.168.1.5:8080/health"
        );
        assert_eq!(
            health_url("[::1]:8080".parse().unwrap(), false),
            "http://[::1]:8080/health"
        );
    }
}
