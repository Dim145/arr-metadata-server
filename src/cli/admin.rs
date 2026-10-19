//! `user`, `keys` and `locks`: what an administrator does from the interface,
//! done from the command line through the same handlers, in this process.
//!
//! The command line is no account, and nobody signs it in: it calls the
//! server's own API as a key with the `admin` scope that exists nowhere but
//! here, named `command line`. Every rule the interface meets is met — a
//! username taken, an administrator the last of them, a scope unknown — and
//! every change is written in the journal and told to the webhooks as the
//! interface's are, as `client:command line`'s.

use std::io::{IsTerminal as _, Read as _, Write as _};

use anyhow::{Context, Result, bail};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::{KeysCommand, LocksCommand, Password, UserCommand};
use crate::{auth::Identity, db::repo, state::AppState};

/// The identity the command line calls the API as.
fn identity() -> Identity {
    Identity::Client(Box::new(repo::client::ApiClient {
        id: "command-line".into(),
        name: "command line".into(),
        key_prefix: String::new(),
        scopes: vec!["admin".into()],
        is_enabled: true,
        expires_at: None,
        created_at: crate::db::now(),
        last_used_at: None,
        last_used_ip: None,
        note: None,
        owner_id: None,
        owner_name: None,
        owner: None,
    }))
}

/// Ask the server's own API, in this process: the answer as `T`, or what
/// the API said was wrong.
async fn ask<T: DeserializeOwned>(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T> {
    let (router, _) = crate::api::native::router().split_for_parts();
    let router = router
        .with_state(state.clone())
        .layer(axum::Extension(identity()));
    let mut request = Request::builder().method(method).uri(path);
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body)?)
        }
        None => Body::empty(),
    };
    let response = router
        .oneshot(request.body(body)?)
        .await
        .context("the API did not answer")?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024).await?;
    if !status.is_success() {
        let said: Option<Value> = serde_json::from_slice(&bytes).ok();
        let message = said
            .as_ref()
            .and_then(|said| said.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("refused"));
        bail!("{message}");
    }
    if status == StatusCode::NO_CONTENT || bytes.is_empty() {
        return Ok(serde_json::from_value(Value::Null)?);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// The whole server, for a command that goes through its API.
async fn server() -> Result<AppState> {
    super::server(super::configuration()?).await
}

/// The id of the account `username`.
async fn account(state: &AppState, username: &str) -> Result<String> {
    Ok(repo::user::find_by_username(&state.db, username)
        .await?
        .with_context(|| format!("no account is called {username:?}"))?
        .user
        .id)
}

/// The password given: from standard input, asked for twice at a terminal,
/// or none, for the server to make one.
fn password(given: &Password) -> Result<Option<String>> {
    if given.generate_password {
        return Ok(None);
    }
    let text = if given.password_stdin {
        super::read_stdin_line("password")?
    } else if std::io::stdin().is_terminal() {
        let first = rpassword::prompt_password("Password: ").context("could not read it")?;
        let again = rpassword::prompt_password("Again: ").context("could not read it")?;
        if first != again {
            bail!("the two passwords differ");
        }
        first
    } else {
        bail!(
            "give the password on standard input with --password-stdin, or have one made with \
             --generate-password"
        );
    };
    crate::telemetry::redact_also(&text);
    Ok(Some(text))
}

pub async fn user(command: UserCommand) -> Result<()> {
    let state = server().await?;
    match command {
        UserCommand::List => {
            let mut offset = 0;
            loop {
                let page: Value = ask(
                    &state,
                    Method::GET,
                    &format!("/users?limit=500&offset={offset}"),
                    None,
                )
                .await?;
                let users = page["users"].as_array().cloned().unwrap_or_default();
                for user in &users {
                    println!(
                        "{:<24} {:<7} {:<9} {}",
                        user["username"].as_str().unwrap_or(""),
                        user["role"].as_str().unwrap_or(""),
                        user["status"].as_str().unwrap_or(""),
                        user["lastLoginAt"]
                            .as_str()
                            .map(|at| format!("last signed in {at}"))
                            .unwrap_or_else(|| "never signed in".to_string())
                    );
                }
                offset += users.len();
                if users.is_empty() || offset as i64 >= page["total"].as_i64().unwrap_or(0) {
                    break;
                }
            }
        }
        UserCommand::Create {
            username,
            role,
            display_name,
            email,
            password: given,
        } => {
            let password = password(&given)?;
            let made: Value = ask(
                &state,
                Method::POST,
                "/users",
                Some(json!({
                    "username": username,
                    "role": role.as_str(),
                    "displayName": display_name,
                    "email": email,
                    "password": password,
                })),
            )
            .await?;
            println!(
                "Opened {} as {}.",
                made["username"].as_str().unwrap_or(&username),
                role.as_str()
            );
            shown_once(made["password"].as_str());
        }
        UserCommand::ResetPassword {
            username,
            password: given,
        } => {
            let id = account(&state, &username).await?;
            let password = password(&given)?;
            let reset: Value = ask(
                &state,
                Method::POST,
                &format!("/users/{id}/password"),
                Some(json!({ "password": password })),
            )
            .await?;
            println!("{username} has a new password; their sessions are closed.");
            shown_once(reset["password"].as_str());
        }
        UserCommand::SetAdmin { username, revoke } => {
            let id = account(&state, &username).await?;
            let change = if revoke {
                json!({ "role": "member" })
            } else {
                json!({ "role": "admin", "status": "active" })
            };
            let _: Value =
                ask(&state, Method::PATCH, &format!("/users/{id}"), Some(change)).await?;
            if revoke {
                println!("{username} is a member now.");
            } else {
                println!("{username} is an active administrator.");
            }
        }
    }
    state.db.close().await;
    Ok(())
}

/// A password the server made: shown here, once, and nowhere else.
fn shown_once(password: Option<&str>) {
    if let Some(password) = password {
        println!("Its password, shown this once: {password}");
    }
}

pub async fn keys(command: KeysCommand) -> Result<()> {
    let state = server().await?;
    let KeysCommand::Create {
        name,
        scopes,
        expires,
        note,
    } = command;
    let made: Value = ask(
        &state,
        Method::POST,
        "/clients",
        Some(json!({
            "name": name,
            "scopes": scopes,
            "expiresAt": expires,
            "note": note,
        })),
    )
    .await?;
    eprintln!(
        "Issued {} [{}]. The key, shown this once — only a hash of it is kept:",
        made["name"].as_str().unwrap_or(&name),
        scopes.join(", ")
    );
    println!("{}", made["key"].as_str().unwrap_or(""));
    state.db.close().await;
    Ok(())
}

pub async fn locks(command: LocksCommand) -> Result<()> {
    let state = server().await?;
    match command {
        LocksCommand::Export { out } => {
            let document: Value = ask(&state, Method::GET, "/admin/locks", None).await?;
            let text = serde_json::to_string_pretty(&document)?;
            let count = document["locks"].as_array().map_or(0, Vec::len);
            match out {
                Some(path) => {
                    std::fs::write(&path, text + "\n")
                        .with_context(|| format!("could not write {}", path.display()))?;
                    eprintln!("{count} locks written to {}", path.display());
                }
                None => {
                    let mut stdout = std::io::stdout().lock();
                    writeln!(stdout, "{text}")?;
                }
            }
        }
        LocksCommand::Import { file } => {
            let text = if file == "-" {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text)?;
                text
            } else {
                std::fs::read_to_string(&file).with_context(|| format!("could not read {file}"))?
            };
            let document: Value =
                serde_json::from_str(&text).context("not a locks document: it is not JSON")?;
            let locks = document["locks"]
                .as_array()
                .context("not a locks document: it has no list of locks")?;
            let version = document["version"].clone();
            let (mut applied, mut unchanged, mut works) = (0, 0, 0);
            let (mut unmatched, mut refused) = (Vec::new(), Vec::new());
            // In parts, as the interface sends it: one request takes so many.
            for part in locks.chunks(crate::api::native::locks::MOST_LOCKS) {
                let done: Value = ask(
                    &state,
                    Method::POST,
                    "/admin/locks",
                    Some(json!({ "version": version, "locks": part })),
                )
                .await?;
                applied += done["applied"].as_u64().unwrap_or(0);
                unchanged += done["unchanged"].as_u64().unwrap_or(0);
                works += done["works"].as_u64().unwrap_or(0);
                unmatched.extend(done["unmatched"].as_array().cloned().unwrap_or_default());
                refused.extend(done["refused"].as_array().cloned().unwrap_or_default());
            }
            println!(
                "{applied} locks set and {unchanged} already so, on {works} works; {} not held, \
                 {} refused",
                unmatched.len(),
                refused.len()
            );
            for work in unmatched {
                println!("  not held: {}", work.as_str().unwrap_or(""));
            }
            for why in refused {
                println!("  refused: {}", why.as_str().unwrap_or(""));
            }
        }
    }
    state.db.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_command_line_opens_an_account_and_makes_it_an_administrator() {
        let state = crate::api::native::testing::server().await;

        let made: Value = ask(
            &state,
            Method::POST,
            "/users",
            Some(json!({ "username": "rescuer", "role": "member", "password": null })),
        )
        .await
        .unwrap();
        // None given: one made, and handed back once.
        assert!(made["password"].as_str().is_some_and(|p| p.len() >= 12));

        let id = account(&state, "rescuer").await.unwrap();
        let _: Value = ask(
            &state,
            Method::PATCH,
            &format!("/users/{id}"),
            Some(json!({ "role": "admin", "status": "active" })),
        )
        .await
        .unwrap();
        let found = repo::user::get(&state.db, &id).await.unwrap().unwrap();
        assert_eq!(found.role, repo::user::Role::Admin);

        // The journal says who did it.
        let trail = repo::audit::list(
            &state.db,
            &repo::audit::Query {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            trail
                .iter()
                .any(|entry| entry.actor.as_deref() == Some("client:command line")),
            "{trail:?}"
        );

        // A rule the interface meets, met: the username is taken.
        let refused = ask::<Value>(
            &state,
            Method::POST,
            "/users",
            Some(json!({ "username": "rescuer", "role": "member" })),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(refused.contains("taken"), "{refused}");
    }

    #[tokio::test]
    async fn a_key_is_issued_and_the_locks_travel_as_a_document() {
        let state = crate::api::native::testing::server().await;
        let made: Value = ask(
            &state,
            Method::POST,
            "/clients",
            Some(json!({ "name": "scraper", "scopes": ["metrics"] })),
        )
        .await
        .unwrap();
        assert!(made["key"].as_str().is_some_and(|k| k.starts_with("ams_")));

        let refused = ask::<Value>(
            &state,
            Method::POST,
            "/clients",
            Some(json!({ "name": "odd", "scopes": ["everything"] })),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(refused.contains("unknown scope"), "{refused}");

        let document: Value = ask(&state, Method::GET, "/admin/locks", None)
            .await
            .unwrap();
        assert_eq!(document["locks"], json!([]));
        let imported: Value = ask(&state, Method::POST, "/admin/locks", Some(document))
            .await
            .unwrap();
        assert_eq!(imported["applied"], 0);
    }
}
