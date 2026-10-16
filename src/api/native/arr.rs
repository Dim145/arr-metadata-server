//! The operator's own Sonarr and Radarr, as an administrator connects them:
//! where each is and its key, whether a change here is told to them, what
//! was told lately — and a test that asks each who it is.
//!
//! Set here as a whole rather than one setting at a time, as the identity
//! provider is: an address is checked as an address before it is kept, and
//! a key goes one way — typed, kept, never shown again.

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{
        audit::{self, Event},
        extract::ClientIp,
    },
    arr::{
        App, Connection,
        client::{self, Status},
        history, push,
    },
    auth::Identity,
    db::repo::audit::Action,
    error::{AppError, AppResult},
    settings::Scope,
    state::AppState,
};

/// An administrator's.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(configuration, configure))
        .routes(routes!(test))
}

/// How many of the latest sends the page is given.
const LATEST: i64 = 50;

/// Why a key kept is neither tried nor kept at a new address: it was given
/// for the Sonarr or Radarr at the old one, and is sent to no other — not
/// by a test, not by the commands and the reads that would follow a save.
const KEY_FOR_ANOTHER: &str =
    "the API key kept was given for another address: type it again to use this one";

/// Whether the address a key was kept with is at the origin of `address`:
/// its scheme, host and port. A key kept with no address is kept with none.
fn kept_for(state: &AppState, app: App, address: &str) -> bool {
    state
        .settings
        .at(Scope::Server, "", app.url_setting())
        .is_some_and(|kept| crate::outbound::same_origin(&kept, address))
}

/// One of the two, as set.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArrConnection {
    pub app: App,
    /// The address set, or empty.
    pub url: String,
    /// Whether a key is kept. It is never sent back.
    pub key_set: bool,
    /// An address and a key both set: this server reaches it.
    pub ready: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArrConfiguration {
    pub sonarr: ArrConnection,
    pub radarr: ArrConnection,
    /// Whether a change here is told to them.
    pub push_refresh: bool,
    /// Works changed here and not told yet, on the instance answering.
    pub waiting: usize,
    /// The latest works told of, newest first.
    pub pushes: Vec<history::Push>,
}

fn connection(state: &AppState, app: App) -> ArrConnection {
    let url = state
        .settings
        .at(Scope::Server, "", app.url_setting())
        .unwrap_or_default();
    let key_set = state
        .settings
        .at(Scope::Server, "", app.key_setting())
        .is_some();
    ArrConnection {
        app,
        ready: Connection::configured(state, app).is_some(),
        url,
        key_set,
    }
}

async fn read(state: &AppState) -> AppResult<ArrConfiguration> {
    Ok(ArrConfiguration {
        sonarr: connection(state, App::Sonarr),
        radarr: connection(state, App::Radarr),
        push_refresh: state.flag(push::SETTING, false),
        waiting: push::waiting(),
        pushes: history::latest(&state.db, LATEST).await?,
    })
}

#[utoipa::path(
    get, path = "/admin/arr", tag = super::meta::TAG,
    responses(
        (status = 200, body = ArrConfiguration),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn configuration(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> AppResult<Json<ArrConfiguration>> {
    identity.require_admin()?;
    Ok(Json(read(&state).await?))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArrUpdate {
    /// Left out, the address stays; empty, it is removed — and the key kept
    /// with it.
    pub sonarr_url: Option<String>,
    /// Left out, the stored key stays; empty, it is removed. Given again
    /// whenever the address moves to another scheme, host or port: a key
    /// kept goes to no other.
    pub sonarr_key: Option<String>,
    pub radarr_url: Option<String>,
    pub radarr_key: Option<String>,
    pub push_refresh: Option<bool>,
}

/// Connect Sonarr and Radarr, and say whether a change is told to them. Each
/// field left out stays; all of it is checked before any of it is written.
///
/// A key is kept for the origin it was given with — the address's scheme,
/// host and port. An address moved to another origin is refused unless the
/// key comes with it (or is emptied); an address removed takes the key with
/// it.
#[utoipa::path(
    put, path = "/admin/arr", tag = super::meta::TAG,
    request_body = ArrUpdate,
    responses(
        (status = 200, body = ArrConfiguration),
        (status = 400, description = "An address or a key was refused, or an address moved to another origin without its key"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn configure(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ip: ClientIp,
    Json(update): Json<ArrUpdate>,
) -> AppResult<Json<ArrConfiguration>> {
    identity.require_admin()?;

    // What would be kept, key by key: `None` removes it.
    let mut changes: Vec<(&'static str, Option<String>)> = Vec::new();
    for (app, url, key) in [
        (App::Sonarr, &update.sonarr_url, &update.sonarr_key),
        (App::Radarr, &update.radarr_url, &update.radarr_key),
    ] {
        if let Some(url) = url {
            let url = url.trim();
            let kept = if url.is_empty() {
                None
            } else {
                let address = client::address(url)
                    .map_err(|why| AppError::BadRequest(format!("{}: {why}", app.name())))?;
                Some(address.as_str().trim_end_matches('/').to_string())
            };
            // A key kept goes with the origin it was given for. Moved to
            // another, the address needs the key given with it — or emptied;
            // removed, it takes the key along rather than leave it waiting
            // for whatever address comes next.
            let key_kept = state
                .settings
                .at(Scope::Server, "", app.key_setting())
                .is_some();
            if key_kept && key.is_none() {
                let had_one = state
                    .settings
                    .at(Scope::Server, "", app.url_setting())
                    .is_some();
                match &kept {
                    Some(address) if !kept_for(&state, app, address) => {
                        return Err(AppError::BadRequest(format!(
                            "{}: {KEY_FOR_ANOTHER}",
                            app.name()
                        )));
                    }
                    None if had_one => changes.push((app.key_setting(), None)),
                    _ => {}
                }
            }
            changes.push((app.url_setting(), kept));
        }
        if let Some(key) = key {
            let key = key.trim();
            changes.push((
                app.key_setting(),
                (!key.is_empty()).then(|| key.to_string()),
            ));
        }
    }
    if let Some(on) = update.push_refresh {
        changes.push((push::SETTING, Some(on.to_string())));
    }

    // Every value checked before a single one is written.
    for (key, value) in &changes {
        if let Some(value) = value {
            let def = crate::settings::registry::find(key)
                .ok_or_else(|| AppError::BadRequest(format!("no setting called {key}")))?;
            crate::settings::registry::validate(def, value)
                .map_err(|why| AppError::BadRequest(format!("{key} {why}")))?;
        }
    }

    // Only what moves is written, and recorded: a form saved again as it
    // was says nothing in the trail.
    let mut moved: Vec<(&'static str, bool)> = Vec::new();
    for (key, value) in &changes {
        let before = state.settings.at(Scope::Server, "", key);
        if before.as_deref() == value.as_deref() {
            continue;
        }
        match value {
            Some(value) => {
                state
                    .settings
                    .set(Scope::Server, "", key, value, Some(&identity.label()))
                    .await?;
            }
            None => {
                state.settings.clear(Scope::Server, "", key).await?;
            }
        }
        moved.push((key, value.is_some()));
    }

    if !moved.is_empty() {
        // The other instances read the settings again.
        state.coord.tell(crate::coord::Message::Settings);
    }

    for (key, now_set) in moved {
        let secret = crate::settings::registry::find(key).is_some_and(|def| def.kind.is_secret());
        // A key's value never reaches the journal: only that it moved.
        let value = state.settings.at(Scope::Server, "", key);
        let detail = match (secret, now_set) {
            (true, true) => "set".to_string(),
            (_, false) => "cleared".to_string(),
            (false, true) => value.unwrap_or_default(),
        };
        audit::record(
            &state,
            Event {
                identity: Some(&identity),
                ip: &ip,
                action: Action::SettingChanged,
                target: Some(&format!("server:{key}")),
                detail: Some(&detail),
            },
        )
        .await;
    }

    Ok(Json(read(&state).await?))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArrTest {
    pub app: App,
    /// The address to try; the stored one when left out.
    pub url: Option<String>,
    /// The key to try with; the stored one when left out — which is tried
    /// only at the origin it was kept for, the stored address's scheme, host
    /// and port.
    pub key: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArrTestOutcome {
    pub ok: bool,
    /// Who answered, and in which version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    /// What went wrong, in words that name neither the address nor the key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Ask Sonarr or Radarr who it is — `/api/v3/system/status` — with the
/// address and the key given, or the stored ones: to check a connection
/// before relying on it. The key kept is tried only at the origin it was
/// kept for: another address is tried with the key given with it.
#[utoipa::path(
    post, path = "/admin/arr/test", tag = super::meta::TAG,
    request_body = ArrTest,
    responses(
        (status = 200, body = ArrTestOutcome),
        (status = 400, description = "No address or no key to try, an address refused, or another address than the key kept was given for, without a key"),
        (status = 403, description = "The caller is not an administrator"),
    ),
)]
async fn test(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<ArrTest>,
) -> AppResult<Json<ArrTestOutcome>> {
    identity.require_admin()?;
    let app = request.app;

    let given = |value: Option<String>| {
        value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let url = given(request.url)
        .or_else(|| state.settings.at(Scope::Server, "", app.url_setting()))
        .ok_or_else(|| {
            AppError::BadRequest(format!("there is no address of {} to try", app.name()))
        })?;
    // Read as an address first: one that is none is said to be none, rather
    // than taken for another than the key's.
    let address = client::address(&url)
        .map_err(|why| AppError::BadRequest(format!("{}: {why}", app.name())))?;
    let key = match given(request.key) {
        Some(key) => key,
        None => {
            let kept = state
                .settings
                .at(Scope::Server, "", app.key_setting())
                .ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "there is no API key of {} to try with",
                        app.name()
                    ))
                })?;
            if !kept_for(&state, app, address.as_str()) {
                return Err(AppError::BadRequest(format!(
                    "{}: {KEY_FOR_ANOTHER}",
                    app.name()
                )));
            }
            kept
        }
    };
    let connection = Connection::new(app, address.as_str(), &key)
        .map_err(|why| AppError::BadRequest(format!("{}: {why}", app.name())))?;

    Ok(Json(match connection.status().await {
        Ok(status) => ArrTestOutcome {
            ok: true,
            status: Some(status),
            error: None,
        },
        Err(e) => ArrTestOutcome {
            ok: false,
            status: None,
            error: Some(crate::telemetry::redact(&crate::providers::clip(
                &format!("{e:#}"),
                400,
            ))),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_address_is_kept_plain_and_a_key_never_shown() {
        let state = crate::api::native::testing::server().await;
        let admin = Identity::Anonymous;

        let saved = configure(
            State(state.clone()),
            Extension(admin.clone()),
            ClientIp::NONE,
            Json(ArrUpdate {
                sonarr_url: Some(" http://sonarr:8989/ ".into()),
                sonarr_key: Some("0123456789abcdef".into()),
                radarr_url: None,
                radarr_key: None,
                push_refresh: Some(true),
            }),
        )
        .await
        .expect("saved")
        .0;
        assert_eq!(saved.sonarr.url, "http://sonarr:8989");
        assert!(saved.sonarr.key_set && saved.sonarr.ready);
        assert!(!saved.radarr.ready);
        assert!(saved.push_refresh);
        let shown = serde_json::to_string(&saved).unwrap();
        assert!(!shown.contains("0123456789abcdef"), "{shown}");

        // An address no Sonarr is at is refused, and nothing is written.
        let refused = configure(
            State(state.clone()),
            Extension(admin.clone()),
            ClientIp::NONE,
            Json(ArrUpdate {
                sonarr_url: Some("http://169.254.169.254/".into()),
                sonarr_key: Some(String::new()),
                radarr_url: None,
                radarr_key: None,
                push_refresh: Some(false),
            }),
        )
        .await;
        assert!(matches!(refused, Err(AppError::BadRequest(_))));
        let kept = read(&state).await.unwrap();
        assert_eq!(kept.sonarr.url, "http://sonarr:8989");
        assert!(kept.sonarr.key_set && kept.push_refresh);

        // Empty: removed.
        let cleared = configure(
            State(state.clone()),
            Extension(admin),
            ClientIp::NONE,
            Json(ArrUpdate {
                sonarr_url: Some(String::new()),
                sonarr_key: Some(String::new()),
                radarr_url: None,
                radarr_key: None,
                push_refresh: None,
            }),
        )
        .await
        .expect("cleared")
        .0;
        assert_eq!(cleared.sonarr.url, "");
        assert!(!cleared.sonarr.key_set && !cleared.sonarr.ready);
    }

    fn sonarr(url: Option<&str>, key: Option<&str>) -> ArrUpdate {
        ArrUpdate {
            sonarr_url: url.map(str::to_string),
            sonarr_key: key.map(str::to_string),
            radarr_url: None,
            radarr_key: None,
            push_refresh: None,
        }
    }

    #[tokio::test]
    async fn a_key_kept_goes_to_no_other_address() {
        use std::sync::Arc;

        use crate::arr::client::tests::{Seen, stand_in};

        let state = crate::api::native::testing::server().await;
        let admin = || Extension(Identity::Anonymous);
        let status = Arc::new(|_: &Seen| {
            (
                200,
                r#"{"appName":"Sonarr","version":"4.0.15.2941"}"#.to_string(),
            )
        });
        // Two Sonarrs, on two ports of this machine: two origins.
        let (here, seen_here) = stand_in(status.clone()).await;
        let (there, seen_there) = stand_in(status).await;

        let Json(_) = configure(
            State(state.clone()),
            admin(),
            ClientIp::NONE,
            Json(sonarr(Some(&here), Some("0123456789abcdef"))),
        )
        .await
        .expect("saved");

        // The key kept is tried where it was kept, under a URL base too.
        let Json(tried) = test(
            State(state.clone()),
            admin(),
            Json(ArrTest {
                app: App::Sonarr,
                url: Some(format!("{here}/sonarr")),
                key: None,
            }),
        )
        .await
        .expect("tried");
        assert!(tried.ok, "{:?}", tried.error);
        assert_eq!(seen_here.lock()[0].key.as_deref(), Some("0123456789abcdef"));

        // At another address, it is not sent: the test is refused, saying
        // why, before that Sonarr is asked anything.
        let refused = test(
            State(state.clone()),
            admin(),
            Json(ArrTest {
                app: App::Sonarr,
                url: Some(there.clone()),
                key: None,
            }),
        )
        .await;
        assert!(
            matches!(&refused, Err(AppError::BadRequest(why)) if why.contains("another address")),
            "{:?}",
            refused.err()
        );
        assert!(seen_there.lock().is_empty());
        // With a key of its own, it is tried.
        let Json(tried) = test(
            State(state.clone()),
            admin(),
            Json(ArrTest {
                app: App::Sonarr,
                url: Some(there.clone()),
                key: Some("fedcba9876543210".into()),
            }),
        )
        .await
        .expect("tried");
        assert!(tried.ok);
        assert_eq!(
            seen_there.lock()[0].key.as_deref(),
            Some("fedcba9876543210")
        );

        // Nor is it kept for another address: saved without its key, the
        // move is refused and nothing is written.
        let refused = configure(
            State(state.clone()),
            admin(),
            ClientIp::NONE,
            Json(sonarr(Some(&there), None)),
        )
        .await;
        assert!(matches!(refused, Err(AppError::BadRequest(_))));
        let kept = read(&state).await.unwrap();
        assert_eq!(kept.sonarr.url, here);
        assert!(kept.sonarr.key_set);

        // Another URL base at the same origin keeps it.
        let saved = configure(
            State(state.clone()),
            admin(),
            ClientIp::NONE,
            Json(sonarr(Some(&format!("{here}/sonarr")), None)),
        )
        .await
        .expect("saved")
        .0;
        assert!(saved.sonarr.key_set && saved.sonarr.ready);

        // With its key, the move is kept, and the key is that one.
        let saved = configure(
            State(state.clone()),
            admin(),
            ClientIp::NONE,
            Json(sonarr(Some(&there), Some("fedcba9876543210"))),
        )
        .await
        .expect("saved")
        .0;
        assert_eq!(saved.sonarr.url, there);
        assert_eq!(
            state
                .settings
                .at(Scope::Server, "", App::Sonarr.key_setting())
                .as_deref(),
            Some("fedcba9876543210")
        );

        // The address taken away takes its key along.
        let cleared = configure(
            State(state.clone()),
            admin(),
            ClientIp::NONE,
            Json(sonarr(Some(""), None)),
        )
        .await
        .expect("cleared")
        .0;
        assert_eq!(cleared.sonarr.url, "");
        assert!(!cleared.sonarr.key_set);
    }
}
