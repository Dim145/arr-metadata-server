//! `backup` and `restore`, from the command line.

use std::{io::IsTerminal as _, path::PathBuf, sync::Arc};

use age::secrecy::SecretString;
use anyhow::{Context, Result, bail};

use crate::backup::{
    Ask, Backups, Reason,
    restore::{Request, Source, Step},
};

/// Make an archive.
pub async fn backup(
    out: Option<PathBuf>,
    include_media: bool,
    passphrase_stdin: bool,
) -> Result<()> {
    let config = super::configuration()?;
    let passphrase = if passphrase_stdin {
        Some(passphrase()?)
    } else {
        None
    };
    let db = super::open_database(&config).await?;
    let backups = Backups::new(&config, db.clone(), None)?;
    if include_media && !backups.media_on_disk() {
        eprintln!(
            "The media are not kept on disk ({}): the archive holds the database alone.",
            backups.media_store_kind()
        );
    }
    let made = backups
        .make(Ask {
            include_media,
            by: Some(BY.to_string()),
            out,
            passphrase,
            ..Ask::new(Reason::Command)
        })
        .await?;
    db.close().await;

    println!("{}", made.name);
    println!("  kept in   {}", made.at);
    println!("  size      {} bytes", made.size);
    println!("  sha256    {}", made.sha256);
    println!(
        "  holds     the {} database at migration {:04}{}{}",
        made.manifest.engine,
        made.manifest.migration,
        if made.manifest.media.included {
            format!(", {} media", made.manifest.media.files)
        } else {
            String::new()
        },
        if made.encrypted { ", encrypted" } else { "" }
    );
    if let Some(note) = &made.manifest.media.note {
        println!("  note      {note}");
    }
    Ok(())
}

/// Restore an archive, the database held alone.
pub async fn restore(archive: &str, replace: bool, passphrase_stdin: bool) -> Result<()> {
    let config = super::configuration()?;
    let passphrase = if passphrase_stdin {
        Some(passphrase()?)
    } else {
        None
    };
    let _alone = crate::backup::lock::hold_alone(
        &config.database,
        ", or restore from its Backups page, which shuts its doors while it does",
    )
    .await?;
    if let Some(url) = super::server_answers(&config).await {
        bail!(
            "a server answers at {url}: stop it first, or restore from its Backups page, which \
             shuts its doors while it does"
        );
    }

    let db = super::open_database(&config).await?;
    // Brought up to this binary first: what is restored into it is.
    db.migrate().await?;
    let store = crate::media::store::Store::open(&config.media)?.map(Arc::new);
    let backups = Backups::new(&config, db.clone(), store)?;

    let path = PathBuf::from(archive);
    let source = if path.is_file() {
        Source::File(path)
    } else if crate::backup::is_archive_name(archive) {
        Source::Kept(archive.to_string())
    } else {
        bail!("{archive} is neither a file nor the name of an archive the server keeps");
    };

    let progress = |step: Step| match step {
        Step::Safety => eprintln!("Making an archive of the database first…"),
        Step::Reading => eprintln!("Reading the archive…"),
        // Once: a line a table would bury the rest.
        Step::Copying { step: 0, steps, .. } => eprintln!("Copying the rows of {steps} tables…"),
        Step::Copying { .. } => {}
    };
    let restored = backups
        .restore(Request {
            source,
            replace,
            passphrase,
            by: Some(BY.to_string()),
            progress: Some(&progress),
        })
        .await?;
    db.close().await;

    println!("Restored {}", restored.archive);
    println!(
        "  made      {} by version {}, from {}",
        restored.manifest.created_at, restored.manifest.server, restored.manifest.engine
    );
    println!("  rows      {}", restored.rows);
    if restored.media_restored + restored.media_skipped > 0 {
        println!(
            "  media     {} put back, {} the store had or could not take",
            restored.media_restored, restored.media_skipped
        );
    }
    if let Some(check) = &restored.media {
        println!(
            "  store     {} media named but not held, {} held but named by nothing",
            check.missing, check.unreferenced
        );
    }
    if let Some(safety) = &restored.safety {
        println!("  undo      restore {safety}, the archive made of the database first");
    }
    Ok(())
}

/// Who the journal says made what the command line did.
const BY: &str = "command line";

/// A passphrase from standard input — asked for, unseen, at a terminal.
fn passphrase() -> Result<SecretString> {
    let text = if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Passphrase: ").context("could not read the passphrase")?
    } else {
        super::read_stdin_line("passphrase")?
    };
    if text.trim().is_empty() {
        bail!("an empty passphrase encrypts nothing");
    }
    crate::telemetry::redact_also(&text);
    Ok(SecretString::from(text))
}
