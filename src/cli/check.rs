//! `config check`: the configuration as the server reads it, its secrets
//! masked, and whether what it names answers — the database, the media's
//! store, where the archives are kept, the public address.
//!
//! What the server would only find out at its start, or on the first day a
//! bucket is written to, said before: the configuration a start refuses is
//! said as the start would say it, and a probe that fails says why.

use anyhow::{Result, bail};

use crate::{
    backup::{self, Place},
    config::MediaStorage,
    db,
};

/// One line of the probes: what, and how it went.
fn said(ok: Option<bool>, what: &str, how: &str) {
    let mark = match ok {
        Some(true) => "ok",
        Some(false) => "FAILED",
        None => "warning",
    };
    println!("  {what:<10} {mark:<8} {how}");
}

pub async fn run() -> Result<()> {
    let config = match super::configuration() {
        Ok(config) => config,
        Err(e) => {
            println!("The configuration is not one the server starts with:\n\n  {e:#}");
            bail!("the configuration was refused");
        }
    };

    println!("The configuration, as the server reads it — secrets masked:\n");
    // A line at a time: the masking escapes a line's end, as it would one
    // that came from outside into a log line.
    for line in format!("{config:#?}").lines() {
        println!("{}", crate::telemetry::redact(line));
    }

    println!("\nWhat it names:\n");
    let mut failed = false;

    // The database: reached, and how its migrations stand.
    let database = match db::Db::connect(&config.database).await {
        Ok(found) => match found.health().await {
            Ok(()) => {
                let last = found.last_migration().await.unwrap_or(0);
                said(
                    Some(true),
                    "database",
                    &format!(
                        "{}, at migration {last:04} of this binary's {:04}",
                        backup::engine_name(found.dialect()),
                        db::latest_migration()
                    ),
                );
                Some(found)
            }
            Err(e) => {
                failed = true;
                said(Some(false), "database", &format!("{e:#}"));
                None
            }
        },
        Err(e) => {
            failed = true;
            said(Some(false), "database", &format!("{e:#}"));
            None
        }
    };

    // The media's store, as it is opened at the start.
    match config.media.storage {
        MediaStorage::Off => said(
            Some(true),
            "media",
            "not kept: every picture is its provider's",
        ),
        storage => match crate::media::store::Store::open(&config.media) {
            Ok(Some(store)) => match store.exists("0000-a-probe-of-config-check.jpg").await {
                Ok(_) => said(
                    Some(true),
                    "media",
                    &match storage {
                        MediaStorage::S3 => format!(
                            "the bucket {} answers",
                            config
                                .media
                                .s3
                                .as_ref()
                                .map(|s3| s3.bucket.as_str())
                                .unwrap_or("")
                        ),
                        _ => format!("kept in {}", config.media.dir.display()),
                    },
                ),
                Err(e) => {
                    failed = true;
                    said(Some(false), "media", &format!("{e:#}"));
                }
            },
            Ok(None) => said(Some(true), "media", "not kept"),
            Err(e) => {
                failed = true;
                said(Some(false), "media", &format!("{e:#}"));
            }
        },
    }

    // Where the archives are kept, written to and taken away again.
    if let Some(found) = &database {
        match backup::Backups::new(&config, found.clone(), None) {
            Ok(backups) => match probe_place(backups.place()).await {
                Ok(()) => said(
                    Some(true),
                    "backups",
                    &format!(
                        "kept in {} ({}), {}",
                        backups.place().label(),
                        backups.place().kind(),
                        if backups.encrypts() {
                            format!("encrypted with {}", backup::PASSPHRASE_VARIABLE)
                        } else {
                            format!("not encrypted: {} is unset", backup::PASSPHRASE_VARIABLE)
                        }
                    ),
                ),
                Err(e) => {
                    failed = true;
                    said(
                        Some(false),
                        "backups",
                        &format!(
                            "{e:#} — set {} to a directory the server can write",
                            backup::DIR_VARIABLE
                        ),
                    );
                }
            },
            Err(e) => {
                failed = true;
                said(Some(false), "backups", &format!("{e:#}"));
            }
        }
    }

    // The public address: asked from here, which a proxy's network may not
    // allow — said, and not counted a failure.
    match &config.server.public_url {
        Some(url) => {
            let health = format!("{url}/health");
            let asked = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()?
                .get(&health)
                .send()
                .await;
            match asked {
                Ok(answer) if answer.status().is_success() => {
                    said(Some(true), "public", &format!("{url} answers"));
                }
                Ok(answer) => said(
                    None,
                    "public",
                    &format!("{health} answered {}", answer.status()),
                ),
                Err(e) => said(
                    None,
                    "public",
                    &format!(
                        "{url} could not be reached from here ({}): a proxy or the network may \
                         route it only from outside",
                        crate::providers::describe_request_error(&e)
                    ),
                ),
            }
        }
        None => said(
            None,
            "public",
            "AMS_PUBLIC_URL is unset: the media are addressed by their providers' addresses, and \
             a sign-in through an identity provider cannot come back",
        ),
    }

    if let Some(found) = database {
        found.close().await;
    }
    if failed {
        bail!("something the configuration names did not answer");
    }
    Ok(())
}

/// Write a file where the archives are kept, read the listing, take it away.
async fn probe_place(place: &Place) -> Result<()> {
    if let Some(dir) = place.directory() {
        tokio::fs::create_dir_all(dir).await?;
        let probe = dir.join(".ams-config-check");
        tokio::fs::write(&probe, b"probe").await?;
        tokio::fs::remove_file(&probe).await?;
    }
    place.list().await?;
    Ok(())
}
