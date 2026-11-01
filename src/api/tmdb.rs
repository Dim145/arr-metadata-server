//! TMDB compatibility: `/3/*`.
//!
//! This grew out of an earlier TMDB relay, with one addition that is the whole
//! point of this server: responses are **patched with local edits** before
//! they are returned.
//!
//! A request is relayed upstream with this server's own TMDB credentials
//! substituted for whatever the client sent — or, where an administrator
//! chose so (`tmdb.relayCredential = client`), with the client's own, as it
//! came, and a key issued here refused. If the path addresses a title a
//! person locked fields on — the title's own document, one of its seasons or
//! episodes, or a page of search, trending or discover results it is among —
//! those fields are written into the upstream document on the way back, a
//! text as it is locked for the language the client asked: in the
//! catalogue's own language, the text locked in it or in every one; in
//! another, only one locked in that language itself, TMDB's own text standing
//! otherwise ([`relay::lay_locks`]). What the sources gave and a person hid
//! here — a credit, a picture, an alternative title — is taken out of the
//! lists TMDB answers with. The client sees TMDB's full response, with the
//! operator's corrections applied.
//!
//! A series' seasons and their episodes asked with its own document
//! (`append_to_response=season/1,season/1/images`) take the locks as the
//! same documents asked on their own do. Of TMDB's v4 API only the public
//! lists are relayed; anything else of it is refused in TMDB's own shape,
//! naming this server. And where an administrator chose this server for
//! TMDB's pictures (`tmdb.images = server`), TMDB's configuration sends its
//! clients to this server's gate for them ([`super::tmdb_images`]).
//!
//! Where an administrator chose so (`tmdb.relayMode = local-first`), a work
//! this server holds is answered by this server itself, in TMDB's shape and
//! from the catalogue, with no key of TMDB's and without asking TMDB: its
//! document and what hangs off it, its seasons and episodes where the
//! catalogue numbers them as TMDB does, `find`, and the catalogue's works
//! first on a search's first page ([`here`]). A series' TheTVDB id is the
//! number Sonarr keeps it under, there and wherever TMDB has none for a
//! series held here, so that a client handing it to Sonarr hands one that
//! Skyhook's dialect answers.

use std::collections::HashMap;

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Map, Value, json};
use utoipa_axum::router::OpenApiRouter;

use crate::{
    api::{
        relay::{self, RelayCredential, climbs},
        tmdb_images,
    },
    config::Surface,
    db::repo,
    domain::{CoverType, CreditType, Episode, ExternalSource, MediaItem, MediaKind},
    error::{AppError, AppResult},
    service::{self, ids, language},
    state::AppState,
    wire::{
        self,
        tmdb::{Withheld, crew_kind},
    },
};

/// Query parameters this server controls and the client does not.
///
/// Every spelling of a key the guard accepts has to be here too. A client may
/// authenticate with `?apikey=`, and for a while only `?api_key=` was stripped —
/// so its credential for *this* server was relayed to TMDB, and landed in
/// TMDB's access logs on every request.
const OVERRIDDEN_PARAMS: &[&str] = &["api_key", "apikey", "include_adult"];

/// The parameters this server controls when the client's own key travels:
/// its word on adult titles, and `apikey`, which is a key for this server and
/// never TMDB's. `api_key` is then the client's own TMDB key, handed on as it
/// came — one issued here was refused before anything was asked.
const OURS_BESIDE_THEIR_KEY: &[&str] = &["apikey", "include_adult"];

/// What a relayed request keeps of the caller's headers: what describes it,
/// and nothing else.
///
/// Not the caller's credential for *this* server — its `Authorization`, its
/// `X-Api-Key`, and its session cookie above all: the interface is
/// same-origin with `/3/*` and the cookie is `Path=/`, so an `<img
/// src="/3/...">` or a bookmark carries a live admin session, which TMDB's
/// logs would have. Nor a proxy's `X-Forwarded-For`, `Forwarded` or
/// `Referer`, which describe the caller's network. Nor a `Range`, an
/// `If-Range` or an `If-Match`: TMDB's edge answers a range with a fragment,
/// and a fragment kept would be every caller's answer for hours.
const ASKED_WITH: &[&str] = &["user-agent", "accept", "accept-language"];

/// The caller's validators, handed on only for an answer that is neither
/// kept nor patched: TMDB would answer "unchanged" for a body it never saw
/// the final shape of, and an answer kept has to be the whole document.
/// This server's own layer answers them on the body it serves.
const VALIDATORS: &[&str] = &["if-none-match", "if-modified-since"];

/// The caller's headers that travel and may change TMDB's answer: what a
/// kept document is told apart by, besides its path and its query.
const VARIES_BY: &[&str] = &["accept", "accept-language"];

/// The paths that mint or use a TMDB session — a request token, a guest
/// session, an account's own pages — whose answers are one caller's.
const SESSION_PATHS: &[&str] = &["authentication", "guest_session", "account"];

/// The parameters a TMDB session travels in.
const SESSION_PARAMS: &[&str] = &["session_id", "guest_session_id"];

/// The most an answer may weigh, as the other relays read theirs: TMDB's
/// largest documents — a series with its images and credits appended — run
/// to a few megabytes.
const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;

/// The language TMDB answers in when none is asked.
const TMDB_DEFAULT_LANGUAGE: &str = "en-US";

/// How many results of one page are looked up here, in the order the page
/// lists them. TMDB's pages hold twenty.
const MOST_RESULTS: usize = 100;

/// The tag every route here is filed under in the documentation.
pub const TAG: &str = "TMDB compatibility";

pub fn router() -> OpenApiRouter<AppState> {
    // Registered by hand rather than through `routes!`: this needs axum's
    // wildcard (`{*path}`) to match every TMDB path, and OpenAPI has no
    // wildcard syntax to express that. The documented path is attached to the
    // spec separately, in `ApiDoc`.
    OpenApiRouter::new()
        .route("/3/{*path}", any(proxy))
        .route("/4/{*path}", any(proxy_v4))
}

fn loop_refused() -> AppError {
    // A warning, not an error: anybody past the guard can send the header.
    tracing::warn!(
        "api.themoviedb.org was asked by an instance of this server: it resolves back here; set \
         AMS_TMDB_UPSTREAM to TMDB's real address"
    );
    AppError::LoopDetected
}

/// Whose key TMDB is asked with.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asking {
    /// The operator's own: a v3 key in the query, a v4 token in a header.
    Operator(String),
    /// The client's own, as it came: its v3 key in the query, its v4 token
    /// in `Authorization`.
    Client,
}

/// Relay any TMDB v3 request, with this server's locks written in on the way
/// back.
///
/// The request goes upstream with this server's own credentials substituted
/// for whatever the client sent — or, when `tmdb.relayCredential` is
/// `client`, with the client's own TMDB key or token as it came, a key issued
/// here being refused with a 401 that says why. If the path addresses a
/// title a person locked fields on, those fields are written into the
/// upstream document before it is returned, so the client gets TMDB's full
/// response with the operator's corrections in it. A text — a title, an
/// overview — is written as it is locked for the language the client asked
/// (`language=`, TMDB's `en-US` when none is): locked in that language, or in
/// every one. One locked in another language alone leaves TMDB's own text in
/// the one asked.
///
/// Every TMDB v3 path is accepted; see TMDB's own documentation for their
/// shapes. Patched: `/3/movie/{id}`, `/3/tv/{id}` — with the seasons and
/// the episodes asked with it (`append_to_response=season/1,…`) —
/// `/3/tv/{id}/season/{n}`, `/3/tv/{id}/season/{n}/episode/{e}`, and the
/// results of `/3/search/{movie,tv,multi}`,
/// `/3/trending/{movie,tv,all}/{window}` and `/3/discover/{movie,tv}`.
/// `/3/configuration` gives this server's gate as the address of TMDB's
/// pictures where an administrator chose so (`tmdb.images = server`).
#[utoipa::path(
    get, path = "/3/{path}", tag = TAG,
    operation_id = "tmdb_proxy",
    // Registered against axum as `/3/{*path}`; see `router` above.
    params(("path" = String, Path, description = "A TMDB v3 path, e.g. `tv/1396` or `search/movie`")),
    responses(
        (status = 200, description = "TMDB's response, with the fields locked here written in where any are"),
        (status = 401, description = "No valid credential was presented; or, with each client's own key relayed, a key issued here was presented"),
        (status = 503, description = "This server has no TMDB API key configured to stand in with"),
    ),
)]
pub async fn proxy(State(state): State<AppState>, request: Request) -> AppResult<Response> {
    if relay::looped(request.headers()) {
        return Err(loop_refused());
    }
    if !state.config.tmdb.passthrough {
        return Err(AppError::ProviderNotConfigured);
    }

    // TMDB's v4 API, for the lists Radarr's import lists read from it when
    // TMDB is redirected here too. Only the public lists: an account's own
    // lists, ratings and watchlist are read with that account's token, which
    // this relay must not carry. Forwarding a caller's would relay a
    // credential; substituting the operator's would serve the operator's
    // account to every caller. Refused before any key is looked for, so the
    // answer is the same whatever is configured — in TMDB's own shape, so
    // that the client's log says who refused it and why.
    let v4 = request.uri().path().starts_with("/4/");
    if v4 && !v4_allowed(request.uri().path()) {
        return Ok(v4_refused(state.config.server.public_url.as_deref()));
    }

    // A read carries nothing in its body TMDB looks at: whatever a caller
    // put there stays here, so what is kept is the answer to the path and
    // the query alone.
    let (parts, _) = request.into_parts();

    // A work this server holds, answered by this server itself in TMDB's
    // shape where an administrator chose so (`tmdb.relayMode =
    // local-first`): neither TMDB nor a key of TMDB's is needed for it, so
    // it is answered before any key is looked for. Anything else — a work
    // held nowhere here, a path not answered here — goes on to TMDB.
    if !v4
        && matches!(parts.method, Method::GET | Method::HEAD)
        && relay::local_first(&state, relay::Service::Tmdb)
        && let Some(asked) = here::recognise(parts.uri.path())
        && let Some(answer) = here::answer(&state, &parts, asked).await?
    {
        return Ok(answer);
    }
    relayed(state, parts).await
}

/// TMDB's answer to a request, with this server's credentials or the
/// client's own as an administrator chose, kept, shared, standing in when
/// TMDB fails, and patched with the locks: the relay itself, which
/// [`proxy`] hands every request it does not answer itself.
async fn relayed(state: AppState, parts: Parts) -> AppResult<Response> {
    let v4 = parts.uri.path().starts_with("/4/");

    // Whose key TMDB is asked with: this server's, in place of whatever the
    // client sent — or, where an administrator chose so, the client's own,
    // as it came. A key issued here is then refused before TMDB is asked: it
    // is no key of TMDB's, and nothing of this server's leaves it.
    let asking = match relay::credential(&state, Surface::Tmdb) {
        RelayCredential::Server => {
            let Some(api_key) = state.config.tmdb.api_key.clone() else {
                return Err(AppError::ProviderNotConfigured);
            };
            // A v4 path is answered only with a v4 read token; a v3 key is
            // refused there, and the relay says so rather than passing the
            // refusal on.
            if v4 && !api_key.starts_with("eyJ") {
                return Err(AppError::ProviderNotConfigured);
            }
            Asking::Operator(api_key)
        }
        RelayCredential::Client => {
            if relay::carries_key_issued_here(&parts.headers, &parts.uri) {
                return Ok(relay::key_issued_here_refused(
                    "TMDB",
                    "tmdb.relayCredential",
                ));
            }
            Asking::Client
        }
    };

    // Reading only. The clients this relay exists for — Jellyseerr, Overseerr,
    // Plex — never write, and a relay that forwards writes is one that lets any
    // key issued here rate a film, or empty a list, as the operator.
    if !matches!(parts.method, Method::GET | Method::HEAD) {
        return Err(AppError::Forbidden);
    }

    // `..` in any spelling. The path is interpolated after `/3`, and a URL
    // parser resolves dot segments before the request goes out: `/3/%2e%2e/4/x`
    // leaves TMDB's v3 API for its v4 one, carrying the operator's credentials.
    if climbs(parts.uri.path()) {
        return Err(AppError::BadRequest("that is not a TMDB path".into()));
    }

    let target = upstream_url(
        &state.config.tmdb.upstream,
        &parts.uri,
        &asking,
        state.config.tmdb.include_adult,
    );

    // The parser that builds the outgoing request has the last word on where
    // it goes — a backslash is a slash to it, and there may be spellings
    // `climbs` has not met — so the path it reads must be the path checked.
    if !url::Url::parse(&target).is_ok_and(|url| url.path().ends_with(parts.uri.path())) {
        return Err(AppError::BadRequest("that is not a TMDB path".into()));
    }

    let patchable = patch_target(parts.uri.path());
    // A feed of changes, which the works changed here are laid beside.
    let feed = changes::of(parts.uri.path());
    // A series' ids, where TMDB may have no TheTVDB id for one held here.
    let ids_of = tvdb_slot(parts.uri.path());

    // What TMDB answered last time, while it is still good: a read that is
    // the same for every caller — never one that mints or uses a TMDB
    // session — filed under the path, the query and the headers that may
    // change the answer, without the credential, under the generation the
    // settings are on; and patched with the locks on each serve, so an edit
    // shows at once whatever the cache holds. Only what the operator's key
    // was answered: what a client's own key was answered is that client's,
    // and a caller whose key TMDB would refuse must not be served what
    // another's was told. Callers asking the same at once share one
    // exchange with TMDB, and what was kept stands in when TMDB fails — a
    // reference list from the database when the memory holds nothing —
    // but for a feed of changes, whose failure is handed on
    // ([`relay::answer`]).
    let filed_as = (matches!(asking, Asking::Operator(_)) && may_keep(&parts.method, &parts.uri))
        .then(|| relay_key(&state, &parts.uri, &parts.headers));
    // The caller's validators travel only with a request answered for it
    // alone and as TMDB wrote it: TMDB's "unchanged" is one caller's
    // answer, about bytes this server does not hand on as they came.
    let validators =
        filed_as.is_none() && patchable.is_none() && feed.is_none() && ids_of.is_none();
    let ask = relay::Ask {
        service: relay::Service::Tmdb,
        key: filed_as.clone(),
        path: parts.uri.path().to_string(),
        reference: filed_as
            .is_some()
            .then(|| reference_key(&state, &parts.uri, &parts.headers)),
    };
    let fetched = relay::answer(&state, ask, {
        let (state, parts, asking) = (state.clone(), parts.clone(), asking.clone());
        let kept = filed_as.is_some();
        async move {
            let (status, mut headers, bytes) =
                send(&state, &parts, &target, &asking, validators).await?;
            if kept {
                // Asked without the caller's validators, the answer's own
                // could never be answered "unchanged" by TMDB: this server's
                // layer tags the body it serves instead — kept or not,
                // shared or not, the same for all.
                headers.remove(header::ETAG);
                headers.remove(header::LAST_MODIFIED);
            }
            Ok(relay::Fetched::new(status, headers, bytes))
        }
    })
    .await?;
    let status = fetched.status;
    let mut headers = fetched.headers;
    let bytes = fetched.body;

    // Only TMDB's own documents about the works it lists, its configuration
    // and its feeds of changes are worth inspecting; pictures, refusals and
    // errors pass through untouched. What stood in for TMDB is written as
    // what it answers is: the locks are this server's, and current.
    if status == StatusCode::OK
        && (patchable.is_some() || feed.is_some() || ids_of.is_some())
        && relay::is_json(&headers)
        && let Ok(mut document) = serde_json::from_slice::<Value>(&bytes)
    {
        let mut changed = false;
        if let Some(target) = patchable {
            let asked = asked_language(&parts.uri);
            changed |= patch(&state, &mut document, target, &asked).await?;
        }
        if let Some(kind) = feed {
            changed |= with_changes_here(&state, &mut document, kind, &parts.uri).await?;
        }
        if let Some((tmdb_id, slot)) = ids_of {
            changed |= with_tvdb_id(&state, &mut document, tmdb_id, slot).await?;
        }
        if changed {
            // TMDB's validators described TMDB's bytes; the patched document
            // is tagged by this server's own layer, and kept as long as it
            // says.
            relay::without_validators(&mut headers);
            return Ok((status, headers, Json(document)).into_response());
        }
    }

    Ok((status, headers, Body::from(bytes)).into_response())
}

/// Where a series' TheTVDB id is in a document of TMDB's: the ids asked on
/// their own (`/3/tv/{id}/external_ids`), or appended to the series'
/// document (`/3/tv/{id}`, under `external_ids`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TvdbSlot {
    Alone,
    Appended,
}

/// The series and the place of its TheTVDB id, for a path of TMDB's that
/// may carry one.
fn tvdb_slot(path: &str) -> Option<(i64, TvdbSlot)> {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        ["3", "tv", id] => Some((id.parse().ok()?, TvdbSlot::Appended)),
        ["3", "tv", id, "external_ids"] => Some((id.parse().ok()?, TvdbSlot::Alone)),
        _ => None,
    }
}

/// A series' TheTVDB id where TMDB has none for one held here: the number
/// Sonarr keeps the series under ([`ids::for_client`]) — TheTVDB's id the
/// catalogue knows, or one this server made — so that a client that hands
/// the series on to Sonarr by it, as Seerr does, hands one Skyhook's
/// dialect answers. A TheTVDB id TMDB gives stays. Whether it was written.
async fn with_tvdb_id(
    state: &AppState,
    document: &mut Value,
    tmdb_id: i64,
    slot: TvdbSlot,
) -> AppResult<bool> {
    let ids = match slot {
        TvdbSlot::Alone => Some(&mut *document),
        TvdbSlot::Appended => document.get_mut("external_ids"),
    };
    let Some(Value::Object(ids)) = ids else {
        return Ok(false);
    };
    if !ids.get("tvdb_id").is_some_and(Value::is_null) {
        return Ok(false);
    }
    let source = ExternalSource::tmdb_for(MediaKind::Series);
    let Some(found) =
        repo::item::find_id_by_external(&state.db, source, &tmdb_id.to_string()).await?
    else {
        return Ok(false);
    };
    let Some(item) = service::load(state, &found).await? else {
        return Ok(false);
    };
    let Some(handed) = ids::for_client(
        MediaKind::Series,
        &item.id,
        item.is_manual,
        &item.external_ids,
    ) else {
        return Ok(false);
    };
    ids.insert("tvdb_id".into(), json!(handed));
    Ok(true)
}

/// A page of TMDB's changes of a kind of work, the works changed here laid
/// after TMDB's own ([`changes::with_here`]): those with a TMDB id of that
/// kind whose journal took a change in the window asked
/// ([`changes::window`]) — a lock put on or lifted, something added or
/// taken out by hand, a refresh that changed a value — on the first page
/// alone, which every client reads. An adult title only where this server
/// shows them. Whether any was added.
async fn with_changes_here(
    state: &AppState,
    document: &mut Value,
    kind: MediaKind,
    uri: &Uri,
) -> AppResult<bool> {
    if !changes::first_page(uri) {
        return Ok(false);
    }
    let Some((from, to)) = changes::window(uri, chrono::Utc::now()) else {
        return Ok(false);
    };
    let adult = state.adult_visible();
    let here: Vec<(i64, bool)> =
        relay::changed_here(state, ExternalSource::tmdb_for(kind), from, to)
            .await?
            .into_iter()
            .filter(|work| adult || !work.adult)
            .filter_map(|work| Some((work.external_id.parse().ok()?, work.adult)))
            .collect();
    Ok(changes::with_here(document, &here))
}

/// Ask TMDB, with the operator's credentials or the client's own, and read
/// what it answered — through TMDB's own door, the one this server's own
/// fetches go through ([`crate::providers::tmdb::TmdbClient::gate`]): as
/// many requests at once between them, and none while TMDB asked to be left
/// alone. A relay never waits that out: a door closed, or full past a
/// client's patience, is a `429` that says when to come back
/// ([`relay::unsent`]), where nothing kept stands in.
async fn send(
    state: &AppState,
    parts: &Parts,
    target: &str,
    asking: &Asking,
    validators: bool,
) -> AppResult<(StatusCode, HeaderMap, Bytes)> {
    let mut upstream = state
        .tmdb
        .request(parts.method.clone(), target)
        .headers(forwarded_headers(&parts.headers, validators))
        // This name is one this server answers on: a resolver sending it
        // back here would otherwise have it call itself.
        .header(crate::providers::radarr::LOOP_HEADER, &state.instance);

    match asking {
        // A v4 token authenticates by header; the query parameter is
        // ignored then.
        Asking::Operator(key) if key.starts_with("eyJ") => {
            upstream = upstream.bearer_auth(key);
        }
        Asking::Operator(_) => {}
        // The client's own v4 token, as it came; its v3 key is in the
        // address already. One issued here was refused before.
        Asking::Client => {
            if let Some(value) = parts.headers.get(header::AUTHORIZATION) {
                upstream = upstream.header(header::AUTHORIZATION, value.clone());
            }
        }
    }

    // Counted as every exchange with TMDB is. The error says the kind of
    // failure and what lay under it, never the address it was sent to,
    // which carries a key in its query — the operator's, or the client's
    // own. The place at the door is held until the body is read.
    let (response, _place) = state
        .tmdb
        .gate()
        .send_now(upstream)
        .await
        .map_err(|e| relay::unsent(relay::Service::Tmdb, e))?;

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::LOOP_DETECTED {
        tracing::warn!(
            "api.themoviedb.org resolves to this server; set AMS_TMDB_UPSTREAM to TMDB's real \
             address"
        );
    }
    let headers = answer_headers(response.headers());

    // A HEAD's answer has no body, whatever its length says of the GET's.
    // Read as a stream, bounded: the error of a body read whole would name
    // the address asked, a key in its query.
    let body = if parts.method == Method::HEAD {
        Vec::new()
    } else {
        crate::providers::read_body(response, ANSWER_LIMIT)
            .await
            .map_err(|e| {
                AppError::UpstreamUnavailable(anyhow::anyhow!(
                    "TMDB's answer could not be read: {}",
                    crate::telemetry::redact(&format!("{e:#}"))
                ))
            })?
    };
    Ok((status, headers, Bytes::from(body)))
}

/// Whether TMDB's answer to a request may be kept, and served to anybody
/// else who asks the same: a read that neither mints nor uses a TMDB
/// session. A request token or a guest session made for one caller, an
/// account's pages, anything asked with a session's id, is that caller's.
fn may_keep(method: &Method, uri: &Uri) -> bool {
    *method == Method::GET && !in_a_session(uri)
}

/// Whether a request mints or uses a TMDB session: one of the paths that
/// do — judged on the decoded segment, `%61ccount` being `account` to
/// TMDB — or a session's id in its query, by any spelling of its name.
fn in_a_session(uri: &Uri) -> bool {
    let mut segments = uri.path().split('/').filter(|segment| !segment.is_empty());
    // The version, `3` or `4`; the next segment names what is asked.
    let _version = segments.next();
    let session_path = segments
        .next()
        .is_some_and(|segment| match urlencoding::decode(segment) {
            Ok(segment) => SESSION_PATHS
                .iter()
                .any(|path| segment.eq_ignore_ascii_case(path)),
            // Not a path of TMDB's, and kept for nobody.
            Err(_) => true,
        });
    let session_param = uri.query().is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes()).any(|(name, _)| {
            SESSION_PARAMS
                .iter()
                .any(|param| name.eq_ignore_ascii_case(param))
        })
    });
    session_path || session_param
}

/// What a relayed document is filed under: what tells one answer of TMDB's
/// from another ([`filed_under`]) — and the upstream, the word on adult
/// titles this server adds, and the generation, so a settings change files
/// what follows elsewhere. Kept a while; and a whole document, the only
/// answer of TMDB's that is ([`relay::answer`]).
fn relay_key(state: &AppState, uri: &Uri, headers: &HeaderMap) -> String {
    format!(
        "tmdb:{}:{}",
        state.caches.generation(),
        asked_of_tmdb(state, uri, headers)
    )
}

/// What a reference list — TMDB's configuration, its genres, its
/// certifications — is kept under in the database too ([`relay::Ask`]):
/// what [`relay_key`] files it under, less the generation, which starts
/// again with the process; a list kept before a restart is found after it.
fn reference_key(state: &AppState, uri: &Uri, headers: &HeaderMap) -> String {
    format!("tmdb:{}", asked_of_tmdb(state, uri, headers))
}

/// Where TMDB is asked, the word on adult titles this server adds, and
/// what tells one answer of TMDB's from another ([`filed_under`]).
fn asked_of_tmdb(state: &AppState, uri: &Uri, headers: &HeaderMap) -> String {
    format!(
        "{}:{}:{}",
        state.config.tmdb.upstream,
        state.config.tmdb.include_adult,
        filed_under(uri, headers)
    )
}

/// The path, the query asked — less the parameters this server sets, in a
/// fixed order ([`relay::sorted_query`]) — and the caller's headers that
/// travel and may change TMDB's answer, each value escaped so that no value
/// can pass for another header's.
fn filed_under(uri: &Uri, headers: &HeaderMap) -> String {
    let varies = VARIES_BY
        .iter()
        .map(|name| {
            let values: Vec<String> = headers
                .get_all(*name)
                .iter()
                .map(|value| url::form_urlencoded::byte_serialize(value.as_bytes()).collect())
                .collect();
            format!("{name}={}", values.join(","))
        })
        .collect::<Vec<_>>()
        .join("&");
    format!(
        "{}?{}#{varies}",
        uri.path(),
        relay::sorted_query(uri, OVERRIDDEN_PARAMS)
    )
}

/// Where TMDB is asked: its address, the path, the query asked less the
/// parameters this server sets — told by their name as TMDB reads it,
/// decoded and in any case, so that `include%5Fadult` is one of them as it
/// is for the key the answer is kept under — and this server's own: the
/// operator's v3 key, when it is the operator's key TMDB is asked with and
/// the query is where it goes, and its word on adult titles. Asked with the
/// client's own key, the client's `api_key` stays as it came.
fn upstream_url(base: &str, uri: &Uri, asking: &Asking, include_adult: bool) -> String {
    let ours = match asking {
        Asking::Operator(_) => OVERRIDDEN_PARAMS,
        Asking::Client => OURS_BESIDE_THEIR_KEY,
    };
    let mut pairs: Vec<String> = uri
        .query()
        .map(|query| {
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                // The client's own TMDB key, where the operator's stands in —
                // or the key it used to authenticate *here* — must never
                // reach upstream.
                .filter(|pair| {
                    let name = url::form_urlencoded::parse(pair.as_bytes())
                        .next()
                        .map(|(name, _)| name.into_owned())
                        .unwrap_or_default();
                    !ours.iter().any(|ours| name.eq_ignore_ascii_case(ours))
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // A v4 token travels in a header, not in the address.
    if let Asking::Operator(key) = asking
        && !key.starts_with("eyJ")
    {
        pairs.push(format!("api_key={key}"));
    }
    pairs.push(format!("include_adult={include_adult}"));

    format!("{base}{}?{}", uri.path(), pairs.join("&"))
}

/// The v4 route, registered by hand as `/4/{*path}` like the v3 one: the same
/// relay, with the checks above that hold it to the public lists.
#[utoipa::path(
    get, path = "/4/{path}", tag = TAG,
    operation_id = "tmdb_proxy_v4",
    params(("path" = String, Path, description = "A TMDB v4 list path: `list/{id}`")),
    responses(
        (status = 200, description = "TMDB's own response"),
        (status = 401, description = "With each client's own token relayed, a key issued here was presented"),
        (status = 403, description = "Not a read; or not a public list — an account's lists, ratings and watchlist are not relayed — answered in TMDB's own error shape (`success: false`, `status_code: 3`) with a message naming this server"),
        (status = 503, description = "No TMDB v4 read token is configured to stand in with"),
    ),
)]
pub async fn proxy_v4(state: State<AppState>, request: Request) -> AppResult<Response> {
    proxy(state, request).await
}

/// The v4 paths relayed: the public lists, and nothing of an account.
fn v4_allowed(path: &str) -> bool {
    path.starts_with("/4/list/") && path.len() > "/4/list/".len()
}

/// TMDB's code for a request its key may not make: "You do not have
/// permissions to access the service."
const NOT_PERMITTED: i64 = 3;

/// What a path of TMDB's v4 API other than a public list is answered: a
/// `403` in TMDB's own shape — `success`, `status_code`, `status_message` —
/// that a client logs as it logs any refusal of TMDB's, naming this server,
/// its version and its public address when it has one, and saying what is
/// not relayed. Nothing is made up in an account's place: an empty
/// watchlist would read to Radarr as one emptied, and a library cleaned
/// against that list would lose its films.
fn v4_refused(public_url: Option<&str>) -> Response {
    let server = match public_url {
        Some(url) => format!("arr-metadata-server {} at {url}", env!("CARGO_PKG_VERSION")),
        None => format!("arr-metadata-server {}", env!("CARGO_PKG_VERSION")),
    };
    let message = format!(
        "{server} answers for api.themoviedb.org and does not relay TMDB accounts or their \
         lists: of TMDB's v4 API, only public lists (/4/list/{{id}}) are relayed"
    );
    let mut response = (
        StatusCode::FORBIDDEN,
        Json(json!({
            "success": false,
            "status_code": NOT_PERMITTED,
            "status_message": message,
        })),
    )
        .into_response();
    response.extensions_mut().insert(relay::RefusedHere);
    response
}

/// The caller's headers as TMDB is asked with them: those that describe the
/// request ([`ASKED_WITH`]), and the caller's validators where TMDB may
/// answer them ([`VALIDATORS`]).
fn forwarded_headers(headers: &HeaderMap, validators: bool) -> HeaderMap {
    let mut asked = relay::asked_with(headers, ASKED_WITH);
    if validators {
        asked.extend(relay::asked_with(headers, VALIDATORS));
    }
    asked
}

/// The headers TMDB's answer keeps: those every relay keeps
/// ([`relay::kept`]: not the hops, a cookie, or TMDB's word on cross-origin
/// reads, which is this server's to give), less an address of TMDB's — a
/// redirect the client was not followed past, a `Content-Location` — which
/// would name the request this server made, a key in its query.
fn answer_headers(headers: &HeaderMap) -> HeaderMap {
    let mut kept = relay::kept(headers);
    kept.remove(header::LOCATION);
    kept.remove(header::CONTENT_LOCATION);
    kept
}

/// The language a client asked TMDB in: the `language` asked, the last when
/// there are several, as TMDB reads them, and TMDB's own `en-US` when none
/// is.
fn asked_language(uri: &Uri) -> String {
    let asked = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .filter(|(name, _)| name == "language")
            .last()
            .map(|(_, value)| value.into_owned())
    });
    asked
        .as_deref()
        .map(str::trim)
        .filter(|asked| !asked.is_empty())
        .unwrap_or(TMDB_DEFAULT_LANGUAGE)
        .to_string()
}

/// Whether a client asked TMDB in the language the catalogue's own text is
/// held in — the one a homepage and the genres' names were locked in. A
/// language is one whatever its region: `fr`, `fr-FR` and `fr-CA` are
/// French.
fn asks_in(asked: &str, catalogue: &str) -> bool {
    let asked = language::normalize(asked);
    !asked.is_empty() && asked == language::normalize(catalogue)
}

/// A document of TMDB's this server may hold edits for.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `/3/tv/{id}`, `/3/movie/{id}`: a title's own document.
    Title { kind: MediaKind, tmdb_id: i64 },
    /// `/3/tv/{id}/season/{n}`: one of a series' seasons, with its episodes.
    Season { tmdb_id: i64, season: i32 },
    /// `/3/tv/{id}/season/{n}/episode/{e}`: one episode, which says its own
    /// numbers and id.
    Episode { tmdb_id: i64 },
    /// `/3/search/{movie,tv,multi}`, `/3/trending/{movie,tv,all}/{window}`,
    /// `/3/discover/{movie,tv}`: a page of results, each of the kind the
    /// path names, or of the kind it says it is (`media_type`).
    Results { kind: Option<MediaKind> },
    /// What hangs off a title, listing what its sources gave it: its people
    /// (`/credits`, a series' `/aggregate_credits`), its pictures
    /// (`/images`, a season's too) or its other titles
    /// (`/alternative_titles`). What a person hid here is taken out.
    Part {
        kind: MediaKind,
        tmdb_id: i64,
        part: Part,
    },
    /// `/3/configuration`: where TMDB's pictures are fetched from, which an
    /// administrator may have made this server ([`tmdb_images::base`]).
    Configuration,
}

/// Which of a title's lists a document of it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    People,
    Pictures,
    Titles,
}

/// Recognise the documents patched, exactly: a title's, a season's, an
/// episode's, a page of results, and the lists a title's sources gave it —
/// its people, its pictures, its other titles, which a person may have hid
/// some of — and TMDB's configuration, which says where its pictures are.
/// Any other sub-resource is left alone: its document carries nothing a
/// lock addresses.
fn patch_target(path: &str) -> Option<Target> {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let id = |text: &str| text.parse::<i64>().ok();
    let number = |text: &str| text.parse::<i32>().ok().filter(|n| *n >= 0);

    match segments.as_slice() {
        ["3", "configuration"] => Some(Target::Configuration),
        ["3", "tv", series] => Some(Target::Title {
            kind: MediaKind::Series,
            tmdb_id: id(series)?,
        }),
        ["3", "movie", movie] => Some(Target::Title {
            kind: MediaKind::Movie,
            tmdb_id: id(movie)?,
        }),
        ["3", "tv", series, "season", season] => Some(Target::Season {
            tmdb_id: id(series)?,
            season: number(season)?,
        }),
        ["3", "tv", series, "season", season, "episode", episode] => {
            number(season)?;
            number(episode)?;
            Some(Target::Episode {
                tmdb_id: id(series)?,
            })
        }
        ["3", "search", "movie"] | ["3", "discover", "movie"] | ["3", "trending", "movie", _] => {
            Some(Target::Results {
                kind: Some(MediaKind::Movie),
            })
        }
        ["3", "search", "tv"] | ["3", "discover", "tv"] | ["3", "trending", "tv", _] => {
            Some(Target::Results {
                kind: Some(MediaKind::Series),
            })
        }
        ["3", "search", "multi"] | ["3", "trending", "all", _] => {
            Some(Target::Results { kind: None })
        }
        ["3", kind @ ("movie" | "tv"), title, part] => {
            let kind = match *kind {
                "movie" => MediaKind::Movie,
                _ => MediaKind::Series,
            };
            let part = match (*part, kind) {
                ("credits", _) | ("aggregate_credits", MediaKind::Series) => Part::People,
                ("images", _) => Part::Pictures,
                ("alternative_titles", _) => Part::Titles,
                _ => return None,
            };
            Some(Target::Part {
                kind,
                tmdb_id: id(title)?,
                part,
            })
        }
        // A season's posters: a person may have hid one of them too.
        ["3", "tv", series, "season", season, "images"] => {
            number(season)?;
            Some(Target::Part {
                kind: MediaKind::Series,
                tmdb_id: id(series)?,
                part: Part::Pictures,
            })
        }
        _ => None,
    }
}

/// Write this server's locks into TMDB's document. Whether anything changed.
///
/// A work is written in as it is locked for the language the document is in
/// (`asked`), as a relay writes one ([`relay::lay_locks`]): in the
/// catalogue's own language, the texts locked in it or in every language; in
/// another, those locked in that language alone. What a person hid of what
/// the sources gave is taken out of the lists the document carries
/// ([`take_hidden`]). A series' document takes its seasons' and their
/// episodes' locks too, where they were asked with it ([`patch_appended`]).
async fn patch(
    state: &AppState,
    document: &mut Value,
    target: Target,
    asked: &str,
) -> AppResult<bool> {
    let unlocalize = |address: &str| state.media.unlocalize(address);
    let in_its_language = asks_in(asked, &state.language(None, None));
    match target {
        Target::Title { kind, tmdb_id } => {
            let source = ExternalSource::tmdb_for(kind);
            let Some(mut item) = relay::locked_work(state, source, &tmdb_id.to_string()).await?
            else {
                return Ok(false);
            };
            relay::lay_locks(state, &mut item, asked);
            let Value::Object(map) = document else {
                return Ok(false);
            };
            let written = write_locked(map, &item, kind, in_its_language, &unlocalize);
            let taken = take_hidden(map, &item, &unlocalize);
            let appended =
                kind == MediaKind::Series && patch_appended(map, tmdb_id, &item, &unlocalize);
            Ok(written | taken | appended)
        }
        Target::Season { tmdb_id, season } => {
            let source = ExternalSource::tmdb_for(MediaKind::Series);
            let Some(mut item) = relay::locked_work(state, source, &tmdb_id.to_string()).await?
            else {
                return Ok(false);
            };
            relay::lay_locks(state, &mut item, asked);
            let Value::Object(map) = document else {
                return Ok(false);
            };
            let withheld = Withheld::of(&item, &unlocalize);
            Ok(patch_season(map, &item, season, &withheld, &unlocalize))
        }
        Target::Episode { tmdb_id } => {
            let source = ExternalSource::tmdb_for(MediaKind::Series);
            let Some(mut item) = relay::locked_work(state, source, &tmdb_id.to_string()).await?
            else {
                return Ok(false);
            };
            relay::lay_locks(state, &mut item, asked);
            let Value::Object(map) = document else {
                return Ok(false);
            };
            Ok(write_episode(map, &item, &unlocalize))
        }
        Target::Results { kind } => {
            patch_results(state, document, kind, asked, in_its_language).await
        }
        Target::Part {
            kind,
            tmdb_id,
            part,
        } => {
            let source = ExternalSource::tmdb_for(kind);
            let Some(item) = relay::locked_work(state, source, &tmdb_id.to_string()).await? else {
                return Ok(false);
            };
            let Value::Object(map) = document else {
                return Ok(false);
            };
            let withheld = Withheld::of(&item, &unlocalize);
            Ok(match part {
                Part::People => take_people(map, &withheld),
                Part::Pictures => take_pictures(map, &withheld),
                Part::Titles => take_titles(map, &withheld),
            })
        }
        Target::Configuration => {
            Ok(tmdb_images::base(state).is_some_and(|base| pictures_from(document, &base)))
        }
    }
}

/// A season's document — asked on its own, or carried by its series' — with
/// the season's locks and its episodes' written in, and the posters a
/// person hid taken out of those asked with it.
fn patch_season(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    number: i32,
    withheld: &Withheld,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let written = write_season(map, item, number, unlocalize);
    let taken = match map.get_mut("images") {
        Some(Value::Object(images)) => take_pictures(images, withheld),
        _ => false,
    };
    written | taken
}

/// What a series' own document carries of its seasons, asked with it
/// (`append_to_response=season/1,season/1/images,season/1/episode/2`):
/// each filed under its path below the series, and patched as the document
/// at that path is when it is asked on its own ([`patch_target`]) — a season
/// with its episodes, a season's posters, an episode. What else hangs off
/// them — a season's or an episode's people, an episode's stills, ids,
/// videos, where to watch — carries nothing a lock addresses, there as on
/// its own. The work comes laid for the language the document is in.
fn patch_appended(
    map: &mut Map<String, Value>,
    tmdb_id: i64,
    item: &MediaItem,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    if !map.keys().any(|key| key.starts_with("season/")) {
        return false;
    }
    let withheld = Withheld::of(item, unlocalize);
    let mut changed = false;
    for (key, appended) in map.iter_mut() {
        let Some(below) = key.strip_prefix("season/") else {
            continue;
        };
        let Value::Object(document) = appended else {
            continue;
        };
        changed |= match patch_target(&format!("/3/tv/{tmdb_id}/season/{below}")) {
            Some(Target::Season { season, .. }) => {
                patch_season(document, item, season, &withheld, unlocalize)
            }
            Some(Target::Episode { .. }) => write_episode(document, item, unlocalize),
            Some(Target::Part {
                part: Part::Pictures,
                ..
            }) => take_pictures(document, &withheld),
            _ => false,
        };
    }
    changed
}

/// TMDB's configuration, its pictures sent to this server's gate: the
/// address a client puts a size and a picture's path after, as
/// `images.base_url` and `images.secure_base_url` both — a client reads one
/// or the other, and this server is reached by its public address whatever
/// its scheme. The sizes stay TMDB's, which the gate answers. Whether it
/// was written.
fn pictures_from(document: &mut Value, base: &str) -> bool {
    let Some(Value::Object(images)) = document.get_mut("images") else {
        return false;
    };
    let mut written = false;
    for key in ["base_url", "secure_base_url"] {
        written |= put(images, key, Some(Value::String(base.to_string())));
    }
    written
}

/// The locks of every work of a page of results that this catalogue holds
/// and a person locked something on, each written in as its own document
/// takes them, in the fields a result carries: its title, its overview, its
/// date, its poster and its background. Looked up together — which of them
/// the catalogue holds, one query a kind; which of those carry a lock, one
/// more — and only those read whole.
async fn patch_results(
    state: &AppState,
    document: &mut Value,
    kind: Option<MediaKind>,
    asked: &str,
    in_its_language: bool,
) -> AppResult<bool> {
    let entries: Vec<(usize, MediaKind, i64)> = match document.get("results") {
        Some(Value::Array(results)) => results
            .iter()
            .enumerate()
            .take(MOST_RESULTS)
            .filter_map(|(at, result)| {
                let kind = kind.or_else(|| result_kind(result))?;
                let id = result.get("id")?.as_i64()?;
                Some((at, kind, id))
            })
            .collect(),
        _ => return Ok(false),
    };

    let mut held: HashMap<(MediaKind, i64), String> = HashMap::new();
    for kind in [MediaKind::Movie, MediaKind::Series] {
        let ids: Vec<String> = entries
            .iter()
            .filter(|(_, of, _)| *of == kind)
            .map(|(_, _, id)| id.to_string())
            .collect();
        if ids.is_empty() {
            continue;
        }
        let found =
            repo::item::held_external_ids(&state.db, ExternalSource::tmdb_for(kind), &ids).await?;
        for (value, media_id) in found {
            if let Ok(id) = value.parse() {
                held.insert((kind, id), media_id);
            }
        }
    }
    if held.is_empty() {
        return Ok(false);
    }

    // Which of them a person locked something on, of the work itself: the
    // others are TMDB's as they came, and not read at all.
    let media_ids: Vec<String> = held.values().cloned().collect();
    let overrides = repo::override_field::list_for_many(&state.db, &media_ids).await?;
    let mut works: HashMap<String, MediaItem> = HashMap::new();
    for media_id in media_ids {
        let locked = overrides
            .get(&media_id)
            .is_some_and(|found| found.iter().any(|o| o.scope == "item"));
        if !locked || works.contains_key(&media_id) {
            continue;
        }
        if let Some(mut item) = service::load(state, &media_id).await?
            && !item.locked_fields.is_empty()
        {
            state.media.for_clients(&mut item);
            relay::lay_locks(state, &mut item, asked);
            works.insert(media_id, item);
        }
    }
    if works.is_empty() {
        return Ok(false);
    }

    let Some(Value::Array(results)) = document.get_mut("results") else {
        return Ok(false);
    };
    let unlocalize = |address: &str| state.media.unlocalize(address);
    let mut changed = false;
    for (at, kind, id) in entries {
        let Some(item) = held
            .get(&(kind, id))
            .and_then(|media_id| works.get(media_id))
        else {
            continue;
        };
        if let Some(Value::Object(result)) = results.get_mut(at) {
            changed |= write_locked(result, item, kind, in_its_language, &unlocalize);
        }
    }
    Ok(changed)
}

/// What a result of a page of several kinds says it is: a film, a series,
/// or neither — a person.
fn result_kind(result: &Value) -> Option<MediaKind> {
    match result.get("media_type")?.as_str()? {
        "movie" => Some(MediaKind::Movie),
        "tv" => Some(MediaKind::Series),
        _ => None,
    }
}

/// The work's locks, in TMDB's names: only the fields a person locked, only
/// into those TMDB's document carries — a title's own, or a result of a
/// list, which carries fewer. The work comes laid for the language the
/// document is in, so a text is written as it is locked for that language
/// ([`relay::lay_locks`]). The homepage and the genres' names only for a
/// client that asked in the catalogue's own language (`in_its_language`),
/// the one they were locked in: TMDB names the genres in the language asked,
/// and a German reader is answered TMDB's German homepage.
///
/// `unlocalize` gives the address a provider gave a picture this server
/// keeps a copy of: a TMDB client builds an image's address from TMDB's
/// path, so only one of TMDB's own can be named.
fn write_locked(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    kind: MediaKind,
    in_its_language: bool,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let mut changed = false;
    let (name, original_name) = match kind {
        MediaKind::Series => ("name", "original_name"),
        MediaKind::Movie => ("title", "original_title"),
    };
    let text = |value: &Option<String>| value.clone().map(Value::String);

    if relay::is_locked(item, "title") {
        changed |= put(map, name, Some(Value::String(item.title.clone())));
    }
    if relay::is_locked(item, "overview") {
        changed |= put(map, "overview", text(&item.overview));
    }
    if in_its_language {
        if relay::is_locked(item, "homepage") {
            changed |= put(map, "homepage", text(&item.homepage));
        }
        if relay::is_locked(item, "genres")
            && let Some(Value::Array(theirs)) = map.get("genres")
        {
            let genres = tmdb_genres(theirs, &item.genres).map(Value::Array);
            changed |= put(map, "genres", genres);
        }
    }

    if relay::is_locked(item, "originalTitle") {
        changed |= put(map, original_name, text(&item.original_title));
    }

    match kind {
        MediaKind::Series => {
            if relay::is_locked(item, "firstAired") {
                changed |= put(map, "first_air_date", text(&item.first_aired));
            }
            if relay::is_locked(item, "lastAired") {
                changed |= put(map, "last_air_date", text(&item.last_aired));
            }
            if relay::is_locked(item, "status") {
                let status = item.status.as_deref().and_then(tmdb_series_status);
                changed |= put(map, "status", status.map(|s| Value::String(s.to_string())));
            }
            if relay::is_locked(item, "runtime") {
                changed |= put(map, "episode_run_time", item.runtime.map(|r| json!([r])));
            }
        }
        MediaKind::Movie => {
            if relay::is_locked(item, "inCinemas") {
                changed |= put(map, "release_date", text(&item.in_cinemas));
            }
            if relay::is_locked(item, "runtime") {
                changed |= put(map, "runtime", item.runtime.map(|r| json!(r)));
            }
        }
    }

    // The poster and the background a person chose, when TMDB has them: a
    // kept copy is named by the address it came from.
    for (key, field, cover) in [
        ("poster_path", "primaryPoster", CoverType::Poster),
        ("backdrop_path", "primaryFanart", CoverType::Fanart),
    ] {
        if relay::is_locked(item, field) {
            let path = relay::chosen_image(item, cover)
                .and_then(|address| tmdb_image_path(&unlocalize(&address)));
            changed |= put(map, key, path.map(Value::String));
        }
    }

    changed
}

/// A season's locks, and its episodes', into TMDB's document of it. The
/// season is the catalogue's of the same number, as the merge pairs them —
/// unless the catalogue knows it under another TMDB id, when it is another
/// season and only its episodes are looked at, each on its own. The work
/// comes laid for the language the document is in: a name and an overview
/// are written as they are locked for it.
fn write_season(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    number: i32,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let mut changed = false;
    let theirs = map.get("id").and_then(Value::as_i64);
    let season = item
        .seasons
        .iter()
        .find(|s| s.season_number == number && s.tmdb_id.is_none_or(|id| Some(id) == theirs));
    if let Some(season) = season {
        let locked = |field: &str| relay::season_locked(item, number, field);
        if locked("title") {
            changed |= put(map, "name", season.title.clone().map(Value::String));
        }
        if locked("overview") {
            changed |= put(map, "overview", season.overview.clone().map(Value::String));
        }
        if locked("airDate") {
            changed |= put(map, "air_date", season.air_date.clone().map(Value::String));
        }
        if locked("primaryPoster") {
            let path = relay::chosen_season_poster(season)
                .and_then(|address| tmdb_image_path(&unlocalize(&address)));
            changed |= put(map, "poster_path", path.map(Value::String));
        }
    }

    if let Some(Value::Array(episodes)) = map.get_mut("episodes") {
        for episode in episodes.iter_mut() {
            if let Value::Object(episode) = episode {
                changed |= write_episode(episode, item, unlocalize);
            }
        }
    }
    changed
}

/// One episode's locks into TMDB's record of it, as the catalogue's episode
/// it is ([`catalogue_episode`]): its title and overview as they are locked
/// for the language the document is in, its date, runtime and still in
/// every language.
fn write_episode(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let (Some(season), Some(number)) = (
        map.get("season_number").and_then(Value::as_i64),
        map.get("episode_number").and_then(Value::as_i64),
    ) else {
        return false;
    };
    let theirs = map.get("id").and_then(Value::as_i64);
    let Some(episode) = catalogue_episode(item, season, number, theirs) else {
        return false;
    };
    let locked = |field: &str| relay::episode_locked(item, episode, field);

    let mut changed = false;
    if locked("title") {
        changed |= put(map, "name", Some(Value::String(episode.title.clone())));
    }
    if locked("overview") {
        changed |= put(map, "overview", episode.overview.clone().map(Value::String));
    }
    if locked("airDate") {
        changed |= put(map, "air_date", episode.air_date.clone().map(Value::String));
    }
    if locked("runtime") {
        changed |= put(map, "runtime", episode.runtime.map(|r| json!(r)));
    }
    if locked("image") {
        let path = episode
            .image
            .as_deref()
            .and_then(|address| tmdb_image_path(&unlocalize(address)));
        changed |= put(map, "still_path", path.map(Value::String));
    }
    changed
}

/// The catalogue's episode a TMDB episode is: the one the merge gave its
/// TMDB id, wherever the catalogue numbers it — TMDB numbers its specials its
/// own way, and cuts some series into seasons of its own, its 2×01 being
/// TheTVDB's 1×13. Failing that, an episode outside the specials by its
/// numbers — unless the catalogue knows that one under another TMDB id, when
/// it is another episode.
fn catalogue_episode(
    item: &MediaItem,
    season: i64,
    number: i64,
    theirs: Option<i64>,
) -> Option<&Episode> {
    if let Some(theirs) = theirs
        && let Some(found) = item.episodes.iter().find(|e| e.tmdb_id == Some(theirs))
    {
        return Some(found);
    }
    if season == 0 {
        return None;
    }
    item.episodes.iter().find(|e| {
        i64::from(e.season_number) == season
            && i64::from(e.episode_number) == number
            && e.tmdb_id.is_none_or(|id| Some(id) == theirs)
    })
}

/// What a person hid of a work, taken out of TMDB's document of it: of its
/// people — a series' creators, its `credits` and `aggregate_credits` when
/// they were asked with it — of its pictures and of its other titles. The
/// people of a season or an episode — its guest stars, its crew — are not
/// the work's credits, which are all a person can hide, and stay as TMDB
/// gives them. Whether anything was taken out.
fn take_hidden(
    map: &mut Map<String, Value>,
    item: &MediaItem,
    unlocalize: &dyn Fn(&str) -> String,
) -> bool {
    let withheld = Withheld::of(item, unlocalize);
    let mut taken = take_people(map, &withheld);
    for key in ["credits", "aggregate_credits"] {
        if let Some(Value::Object(people)) = map.get_mut(key) {
            taken |= take_people(people, &withheld);
        }
    }
    if let Some(Value::Object(pictures)) = map.get_mut("images") {
        taken |= take_pictures(pictures, &withheld);
    }
    if let Some(Value::Object(titles)) = map.get_mut("alternative_titles") {
        taken |= take_titles(titles, &withheld);
    }
    taken
}

/// TMDB's lists of a work's people, less those hidden here: its cast by the
/// person's id among the actors hidden, its crew by the person's id and the
/// kind of credit their job makes ([`crew_kind`]), a series' creators among
/// its writers — a creator is one, as the catalogue files them.
fn take_people(map: &mut Map<String, Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    for (key, kind) in [
        ("cast", CreditType::Actor),
        ("created_by", CreditType::Writer),
    ] {
        if let Some(Value::Array(people)) = map.get_mut(key) {
            let before = people.len();
            people.retain(|person| {
                !person
                    .get("id")
                    .and_then(Value::as_i64)
                    .is_some_and(|id| withheld.hides(kind, id))
            });
            taken |= people.len() != before;
        }
    }
    if let Some(Value::Array(crew)) = map.get_mut("crew") {
        taken |= take_crew(crew, withheld);
    }
    taken
}

/// TMDB's crew, less the jobs hidden here: an entry a job, or in a series'
/// aggregate a person with every job they had, of which the hidden ones are
/// taken out — and the person with them when none is left.
fn take_crew(crew: &mut Vec<Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    crew.retain_mut(|member| {
        let Value::Object(member) = member else {
            return true;
        };
        let Some(id) = member.get("id").and_then(Value::as_i64) else {
            return true;
        };
        let department = member
            .get("department")
            .and_then(Value::as_str)
            .map(str::to_string);
        let hidden = |job: &Value| {
            job.as_str()
                .and_then(|job| crew_kind(job, department.as_deref()))
                .is_some_and(|kind| withheld.hides(kind, id))
        };
        if let Some(Value::Array(jobs)) = member.get_mut("jobs") {
            let before = jobs.len();
            jobs.retain(|job| !job.get("job").is_some_and(hidden));
            if jobs.len() == before {
                return true;
            }
            taken = true;
            return !jobs.is_empty();
        }
        if member.get("job").is_some_and(hidden) {
            taken = true;
            return false;
        }
        true
    });
    taken
}

/// TMDB's lists of a work's or a season's pictures, less those hidden here,
/// told by the path TMDB files each under.
fn take_pictures(map: &mut Map<String, Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    for key in ["posters", "backdrops", "logos"] {
        if let Some(Value::Array(pictures)) = map.get_mut(key) {
            let before = pictures.len();
            pictures.retain(|picture| {
                !picture
                    .get("file_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| withheld.pictures.contains(path))
            });
            taken |= pictures.len() != before;
        }
    }
    taken
}

/// TMDB's list of a work's other titles — a film's `titles`, a series'
/// `results` — less those hidden here.
fn take_titles(map: &mut Map<String, Value>, withheld: &Withheld) -> bool {
    let mut taken = false;
    for key in ["titles", "results"] {
        if let Some(Value::Array(titles)) = map.get_mut(key) {
            let before = titles.len();
            titles.retain(|title| !withheld.hides_title(title));
            taken |= titles.len() != before;
        }
    }
    taken
}

/// Write `value` over a field TMDB's document carries; a field it does not
/// carry — an error's, a document of another shape — is not added, and no
/// value leaves the field as TMDB gave it. Whether it was written.
fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) -> bool {
    match (map.get_mut(key), value) {
        (Some(slot), Some(value)) => {
            *slot = value;
            true
        }
        _ => false,
    }
}

/// TMDB's own genre objects for the genres locked here, in the order they
/// were locked: each found among those TMDB's document carries by its name,
/// so that its id is TMDB's. None when one of them is not there — an id
/// cannot be made up, and a genre without one breaks the clients that link
/// or key by it — and TMDB's genres then stay as they came.
fn tmdb_genres(theirs: &[Value], locked: &[String]) -> Option<Vec<Value>> {
    let mut genres: Vec<Value> = Vec::with_capacity(locked.len());
    for name in locked {
        let wanted = name.trim().to_lowercase();
        let genre = theirs.iter().find(|genre| {
            genre.get("id").is_some_and(Value::is_number)
                && genre
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|theirs| theirs.trim().to_lowercase() == wanted)
        })?;
        if !genres.contains(genre) {
            genres.push(genre.clone());
        }
    }
    Some(genres)
}

/// The path TMDB files an image under, from its address: `/abc.jpg` from
/// `https://image.tmdb.org/t/p/original/abc.jpg`, whatever the size asked.
fn tmdb_image_path(address: &str) -> Option<String> {
    wire::tmdb::tmdb_path(address)
}

/// Canonical status back to TMDB's vocabulary; anything else is left as
/// TMDB said it.
fn tmdb_series_status(status: &str) -> Option<&'static str> {
    match status {
        "ended" => Some("Ended"),
        "upcoming" => Some("Planned"),
        "continuing" => Some("Returning Series"),
        _ => None,
    }
}

/// What this server answers itself, in TMDB's shape, where an administrator
/// chose so (`tmdb.relayMode = local-first`): a work it holds — found by
/// its id in TMDB's dialect ([`ids::for_tmdb`]), switched on, and for
/// adults only where this server shows them — its own document with what is
/// appended to it, what hangs off it asked on its own, its seasons and
/// episodes where the catalogue numbers them as TMDB does, `find` by an id
/// of another site's; and the first page of a search, the works the
/// catalogue finds by the term before TMDB's. The documents are written by
/// [`crate::wire::tmdb`].
///
/// Neither TMDB nor a key of TMDB's is needed for any of it. What another
/// resource of TMDB's holds — where to watch a work, its reviews, the works
/// like it — is asked of TMDB through the relay where TMDB knows the work,
/// and added; where TMDB cannot be asked, or does not know the work, it is
/// the empty list TMDB writes. Anything else goes on to TMDB, as it would
/// without the setting: a work held nowhere here, a season TMDB numbers
/// otherwise than the catalogue, a path this server does not write.
mod here {
    use std::collections::{HashMap, HashSet};

    use axum::{
        http::{HeaderValue, StatusCode, Uri, header, request::Parts},
        response::{IntoResponse, Response},
    };
    use serde_json::{Map, Value, json};

    use super::{ANSWER_LIMIT, asked_language, relayed};
    use crate::{
        api::{relay, tmdb_images},
        db::repo,
        domain::{EpisodeOrder, ExternalSource, MediaItem, MediaKind},
        error::AppResult,
        media::Media,
        providers::names,
        service::{self, ids, language, orders},
        state::AppState,
        wire::tmdb::{self as shape, EpisodeText, Filters, Language, Numbering, Part, Work},
    };

    /// A path of TMDB's this server may answer itself.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Asked {
        /// `/3/movie/{id}`, `/3/tv/{id}`: a work's own document — or what
        /// hangs off it, `part`: `credits`, `watch/providers`.
        Title {
            kind: MediaKind,
            id: i64,
            part: Option<String>,
        },
        /// `/3/tv/{id}/season/{n}`, or what hangs off it.
        Season {
            id: i64,
            season: i32,
            part: Option<String>,
        },
        /// `/3/tv/{id}/season/{n}/episode/{e}`, or what hangs off it.
        Episode {
            id: i64,
            season: i32,
            episode: i32,
            part: Option<String>,
        },
        /// `/3/find/{id}`: a work by an id of another site's.
        Find { external_id: String },
        /// `/3/search/{movie,tv,multi}`.
        Search { kind: Option<MediaKind> },
    }

    /// The paths this server may answer itself, exactly; any other is
    /// TMDB's.
    pub fn recognise(path: &str) -> Option<Asked> {
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        let id = |text: &str| text.parse::<i64>().ok().filter(|id| *id > 0);
        let number = |text: &str| text.parse::<i32>().ok().filter(|n| *n >= 0);
        let part = |rest: &[&str]| -> Option<Option<String>> {
            if rest.is_empty() {
                return Some(None);
            }
            rest.iter()
                .all(|segment| !segment.is_empty())
                .then(|| Some(rest.join("/")))
        };
        match segments.as_slice() {
            ["3", "search", "movie"] => Some(Asked::Search {
                kind: Some(MediaKind::Movie),
            }),
            ["3", "search", "tv"] => Some(Asked::Search {
                kind: Some(MediaKind::Series),
            }),
            ["3", "search", "multi"] => Some(Asked::Search { kind: None }),
            ["3", "find", external_id] => Some(Asked::Find {
                external_id: urlencoding::decode(external_id).ok()?.trim().to_string(),
            }),
            [
                "3",
                "tv",
                series,
                "season",
                season,
                "episode",
                episode,
                rest @ ..,
            ] => Some(Asked::Episode {
                id: id(series)?,
                season: number(season)?,
                episode: number(episode)?,
                part: part(rest)?,
            }),
            ["3", "tv", series, "season", season, rest @ ..] => Some(Asked::Season {
                id: id(series)?,
                season: number(season)?,
                part: part(rest)?,
            }),
            ["3", "movie", movie, rest @ ..] => Some(Asked::Title {
                kind: MediaKind::Movie,
                id: id(movie)?,
                part: part(rest)?,
            }),
            ["3", "tv", series, rest @ ..] => Some(Asked::Title {
                kind: MediaKind::Series,
                id: id(series)?,
                part: part(rest)?,
            }),
            _ => None,
        }
    }

    /// The answer this server gives itself, or none for TMDB to answer.
    pub async fn answer(
        state: &AppState,
        parts: &Parts,
        asked: Asked,
    ) -> AppResult<Option<Response>> {
        let written = match asked {
            Asked::Title { kind, id, part } => title(state, parts, kind, id, part).await?,
            Asked::Season { id, season, part } => {
                season_or_episode(state, parts, id, season, None, part).await?
            }
            Asked::Episode {
                id,
                season,
                episode,
                part,
            } => season_or_episode(state, parts, id, season, Some(episode), part).await?,
            Asked::Find { external_id } => find(state, parts, &external_id).await?,
            Asked::Search { kind } => return search(state, parts, kind).await,
        };
        Ok(written.map(|(status, document)| respond(parts, status, &document)))
    }

    /// A document written here: TMDB's type of answer, with no age — it is
    /// this server's own — and validators this server's layer puts on it.
    /// Counted apart, and said in the log.
    fn respond(parts: &Parts, status: StatusCode, document: &Value) -> Response {
        crate::metrics::relay_local("tmdb");
        tracing::debug!(path = %parts.uri.path(), %status, "answered here in TMDB's shape");
        let body = serde_json::to_vec(document).unwrap_or_default();
        let mut response = (status, body).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=utf-8"),
        );
        response
    }

    // ─── what a document is written from ─────────────────────────────────

    /// A work held here under its id in TMDB's dialect, as a document of
    /// TMDB's shape may be written of it: switched on, for adults only where
    /// this server shows them, and known to TMDB's clients by that very id.
    async fn held(state: &AppState, kind: MediaKind, id: i64) -> AppResult<Option<MediaItem>> {
        let found = if ids::is_own(id) {
            service::own_holders(state, kind).await?.remove(&id)
        } else if kind == MediaKind::Series
            && let Some(fankai) = ids::from_fankai(id)
        {
            repo::item::find_id_by_external(&state.db, ExternalSource::Fankai, &fankai.to_string())
                .await?
        } else {
            let source = ExternalSource::tmdb_for(kind);
            repo::item::find_id_by_external(&state.db, source, &id.to_string()).await?
        };
        let Some(found) = found else {
            return Ok(None);
        };
        Ok(service::load(state, &found)
            .await?
            .filter(|item| item.kind == kind && answered(state, item) == Some(id)))
    }

    /// The id a work held here is answered under in TMDB's dialect, where
    /// it is answered at all: switched on, and for adults only where this
    /// server shows them.
    fn answered(state: &AppState, item: &MediaItem) -> Option<i64> {
        if !item.is_enabled || (item.is_adult && !state.adult_visible()) {
            return None;
        }
        ids::for_tmdb(item.kind, &item.id, item.is_manual, &item.external_ids)
    }

    /// What a document of a work is written from beside the work: TMDB's
    /// own document of it, whose numbering its seasons are, the language
    /// asked, and what is held in that language of its episodes.
    struct Context {
        snapshot: Option<Value>,
        /// The series' orders, as kept, and which numbers its seasons as
        /// TMDB does.
        orders: Vec<EpisodeOrder>,
        numbering: Choice,
        language: Language,
        filters: Filters,
        texts: HashMap<(i32, i32), EpisodeText>,
        gate: bool,
    }

    /// Whose numbering a series' seasons are written in.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Choice {
        /// The catalogue's own: TMDB's too, or the only one there is.
        Own,
        /// The order of that key, which keeps TMDB's numbering.
        Order(String),
        /// TMDB's, which nothing here keeps.
        Theirs,
    }

    impl Context {
        /// What a document of a work is written from — reading TMDB's
        /// numbering of a series first where none is kept and its seasons
        /// are asked for (`seasons`), as Skyhook's TMDB edition reads it.
        async fn of(
            state: &AppState,
            parts: &Parts,
            item: &MediaItem,
            seasons: bool,
        ) -> AppResult<Self> {
            let snapshot = snapshot_of(state, item).await?;
            let (orders, numbering) = numbering_of(state, item, snapshot.as_ref(), seasons).await?;
            let mut context = Self::with(state, parts, item, snapshot, numbering, true).await?;
            context.orders = orders;
            Ok(context)
        }

        /// Whether a series' seasons are written here: in a numbering that
        /// is TMDB's.
        fn numbers_seasons(&self) -> bool {
            self.numbering != Choice::Theirs
        }

        fn numbering(&self) -> Numbering<'_> {
            match &self.numbering {
                Choice::Own => Numbering::Own,
                Choice::Order(key) => self
                    .orders
                    .iter()
                    .find(|order| order.key == *key)
                    .map_or(Numbering::Theirs, Numbering::Order),
                Choice::Theirs => Numbering::Theirs,
            }
        }

        /// What a work is listed from on a page of results, which carries
        /// none of its seasons: TMDB's document of it, and the language.
        async fn listed(state: &AppState, parts: &Parts, item: &MediaItem) -> AppResult<Self> {
            let snapshot = snapshot_of(state, item).await?;
            Self::with(state, parts, item, snapshot, Choice::Theirs, false).await
        }

        /// The rest, `episodes` saying whether the episodes' text is read.
        async fn with(
            state: &AppState,
            parts: &Parts,
            item: &MediaItem,
            snapshot: Option<Value>,
            numbering: Choice,
            episodes: bool,
        ) -> AppResult<Self> {
            let language = Language::new(&asked_language(&parts.uri), &language::stored(state));
            let named = asked(&parts.uri, "language").is_some();
            let filters = Filters::from_query(parts.uri.query(), named.then_some(&language));
            let texts = if episodes && item.kind == MediaKind::Series && !language.is_stored() {
                repo::translation::for_episodes(&state.db, &item.id, &language.asked)
                    .await?
                    .into_iter()
                    .map(|(at, text)| {
                        let text = EpisodeText {
                            title: text.title,
                            overview: text.overview,
                        };
                        (at, text)
                    })
                    .collect()
            } else {
                HashMap::new()
            };
            Ok(Self {
                snapshot,
                orders: Vec::new(),
                numbering,
                language,
                filters,
                texts,
                gate: tmdb_images::base(state).is_some(),
            })
        }

        fn work<'a>(&'a self, item: &'a MediaItem, id: i64, pictures: &'a Kept<'a>) -> Work<'a> {
            let tvdb_id = (item.kind == MediaKind::Series)
                .then(|| {
                    ids::for_client(
                        MediaKind::Series,
                        &item.id,
                        item.is_manual,
                        &item.external_ids,
                    )
                })
                .flatten();
            Work::new(item, id, &self.language, pictures)
                .with_snapshot(self.snapshot.as_ref())
                .with_tvdb_id(tvdb_id)
                .numbered(self.numbering())
                .with_episode_texts(self.texts.clone())
        }
    }

    /// TMDB's own document of a work, as it was last fetched: in the
    /// catalogue's language, with what this server asks TMDB for with it.
    async fn snapshot_of(state: &AppState, item: &MediaItem) -> AppResult<Option<Value>> {
        if item.external_ids.tmdb.is_none() {
            return Ok(None);
        }
        let texts = repo::snapshot::texts(&state.db, &item.id, &[names::TMDB]).await?;
        Ok(texts
            .into_iter()
            .next()
            .and_then(|(_, text)| serde_json::from_str(&text).ok()))
    }

    /// Whose numbering a series' seasons are written in, and its orders: its
    /// own where no other source numbers it — entered by hand, named by no
    /// TMDB id; TMDB's own numbering where the catalogue keeps it
    /// (`tmdb-aired`), read first from TMDB where none is kept and `read`
    /// asks so ([`orders::for_clients`]); else, for a series no TheTVDB
    /// numbers, the order frozen as its numbering, or its own where TMDB's
    /// document numbers its seasons alike; TMDB's otherwise.
    async fn numbering_of(
        state: &AppState,
        item: &MediaItem,
        snapshot: Option<&Value>,
        read: bool,
    ) -> AppResult<(Vec<EpisodeOrder>, Choice)> {
        if item.kind != MediaKind::Series || item.is_manual || item.external_ids.tmdb.is_none() {
            return Ok((Vec::new(), Choice::Own));
        }
        let kept = if read {
            orders::for_clients(state, item, Some(orders::TMDB_AIRED)).await?
        } else {
            repo::order::list(&state.db, &item.id).await?
        };
        let tmdbs = orders::in_use(item, &kept, orders::TMDB_AIRED).map(|order| order.key.clone());
        if let Some(key) = tmdbs {
            return Ok((kept, Choice::Order(key)));
        }
        let by_tvdb = orders::aired(&kept).is_none_or(|aired| aired.source == names::TVDB);
        let frozen = orders::frozen(item, &kept).map(|order| order.key.clone());
        let choice = match frozen {
            _ if by_tvdb => Choice::Theirs,
            Some(key) => Choice::Order(key),
            None if shape::numbered_alike(item, snapshot) => Choice::Own,
            None => Choice::Theirs,
        };
        Ok((kept, choice))
    }

    /// The catalogue's pictures as TMDB's clients are given them: TMDB's by
    /// TMDB's path, and — where TMDB's clients are sent to this server for
    /// pictures — a copy kept here of any other by the name the gate
    /// serves it under.
    struct Kept<'a> {
        media: &'a Media,
        gate: bool,
    }

    impl shape::Pictures for Kept<'_> {
        fn origin(&self, address: &str) -> String {
            self.media.unlocalize(address)
        }

        fn kept(&self, address: &str) -> Option<String> {
            if !self.gate {
                return None;
            }
            tmdb_images::kept_name(Media::key_in(address)?)
        }

        fn size(&self, address: &str) -> Option<(u32, u32)> {
            let keyed = self.media.keyed(Media::key_in(address)?)?;
            Some((keyed.look.width?, keyed.look.height?))
        }
    }

    // ─── a work, its seasons, its episodes ───────────────────────────────

    /// The names a request appends to the document asked, each once, as
    /// TMDB reads them.
    fn appends(uri: &Uri) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        asked(uri, "append_to_response")
            .map(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .filter(|name| seen.insert(name.to_string()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The last value of a parameter of the query, as TMDB reads one.
    fn asked(uri: &Uri, name: &str) -> Option<String> {
        url::form_urlencoded::parse(uri.query()?.as_bytes())
            .filter(|(key, _)| key == name)
            .last()
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.trim().is_empty())
    }

    /// A work's own document, or what hangs off it.
    async fn title(
        state: &AppState,
        parts: &Parts,
        kind: MediaKind,
        id: i64,
        part: Option<String>,
    ) -> AppResult<Option<(StatusCode, Value)>> {
        let Some(item) = held(state, kind, id).await? else {
            return Ok(None);
        };
        let appended = appends(&parts.uri);
        let nested = appended.iter().any(|name| name.starts_with("season/"));
        let context = Context::of(state, parts, &item, nested).await?;
        let pictures = Kept {
            media: &state.media,
            gate: context.gate,
        };
        let work = context.work(&item, id, &pictures);
        let tmdb_id = item.external_ids.tmdb;
        let path =
            |below: &str| format!("/3/{}/{}/{below}", kind_path(kind), tmdb_id.unwrap_or(id));

        if let Some(name) = part {
            return Ok(match shape::title_part(&work, &name, &context.filters) {
                Part::Doc(document) => Some((StatusCode::OK, document)),
                // Another resource's: TMDB's to answer where it knows the
                // work, else empty here.
                Part::Elsewhere { empty } if tmdb_id.is_none() => {
                    empty.map(|empty| (StatusCode::OK, empty))
                }
                Part::Elsewhere { .. } | Part::Unknown => None,
            });
        }

        // More than TMDB answers: TMDB's to refuse. A series' seasons in
        // TMDB's numbering nothing here keeps, or listed with nothing to list
        // them from: TMDB's to give.
        if appended.len() > shape::MOST_APPENDED
            || (kind == MediaKind::Series
                && !context.numbers_seasons()
                && (nested || context.snapshot.is_none()))
        {
            return Ok(None);
        }

        let mut document = match kind {
            MediaKind::Movie => shape::movie(&work),
            MediaKind::Series => shape::tv(&work),
        };
        let mut added = Map::new();
        for name in appended {
            if let Some(below) = name.strip_prefix("season/") {
                if let Some(value) = nested_part(state, parts, &work, &context, below, &path).await
                {
                    added.insert(name.clone(), value);
                }
                continue;
            }
            let value = match shape::title_part(&work, &name, &context.filters) {
                Part::Doc(document) => Some(document),
                Part::Elsewhere { empty } => match tmdb_id {
                    Some(_) => graft(state, parts, &path(&name)).await.or(empty),
                    None => empty,
                },
                Part::Unknown => None,
            };
            if let Some(value) = value {
                added.insert(name, shape::appended(value));
            }
        }
        if let Value::Object(map) = &mut document {
            map.extend(added);
        }
        Ok(Some((StatusCode::OK, document)))
    }

    /// What a series' document carries of one of its seasons, asked with it
    /// (`season/1`, `season/1/images`, `season/1/episode/2`): as the same
    /// path below the series is answered on its own, less its id. Nothing
    /// for a season or an episode the catalogue does not hold.
    async fn nested_part(
        state: &AppState,
        parts: &Parts,
        work: &Work<'_>,
        context: &Context,
        below: &str,
        path: &(dyn Fn(&str) -> String + Sync),
    ) -> Option<Value> {
        let segments: Vec<&str> = below.split('/').collect();
        let number = |text: &str| text.parse::<i32>().ok().filter(|n| *n >= 0);
        let filters = &context.filters;
        let value = match segments.as_slice() {
            [season] => shape::season(work, number(season)?)?,
            [season, "episode", episode] => {
                shape::episode(work, number(season)?, number(episode)?)?
            }
            [season, "episode", episode, rest @ ..] => {
                let (season, episode) = (number(season)?, number(episode)?);
                if !work.has_episode(season, episode) {
                    return None;
                }
                match shape::episode_part(work, season, episode, &rest.join("/"), filters) {
                    Part::Doc(document) => document,
                    _ => return None,
                }
            }
            [season, rest @ ..] => {
                let season = number(season)?;
                if !work.has_season(season) {
                    return None;
                }
                match shape::season_part(work, season, &rest.join("/"), filters) {
                    Part::Doc(document) => document,
                    Part::Elsewhere { empty } => {
                        let theirs = if work.tmdb_id().is_some() {
                            graft(state, parts, &path(&format!("season/{below}"))).await
                        } else {
                            None
                        };
                        theirs.or(empty)?
                    }
                    Part::Unknown => return None,
                }
            }
            _ => return None,
        };
        Some(shape::appended(value))
    }

    /// A season's document or an episode's, or what hangs off either, where
    /// the catalogue numbers the series as TMDB does.
    async fn season_or_episode(
        state: &AppState,
        parts: &Parts,
        id: i64,
        season: i32,
        episode: Option<i32>,
        part: Option<String>,
    ) -> AppResult<Option<(StatusCode, Value)>> {
        let Some(item) = held(state, MediaKind::Series, id).await? else {
            return Ok(None);
        };
        let context = Context::of(state, parts, &item, true).await?;
        if !context.numbers_seasons() {
            return Ok(None);
        }
        let pictures = Kept {
            media: &state.media,
            gate: context.gate,
        };
        let work = context.work(&item, id, &pictures);
        let held_here = match episode {
            Some(episode) => work.has_episode(season, episode),
            None => work.has_season(season),
        };
        // A season or an episode the catalogue does not hold: TMDB's to
        // answer where it knows the series, else none there is.
        if !held_here {
            return Ok(item
                .external_ids
                .tmdb
                .is_none()
                .then(|| (StatusCode::NOT_FOUND, shape::not_found())));
        }
        let below = match episode {
            Some(episode) => format!("season/{season}/episode/{episode}"),
            None => format!("season/{season}"),
        };
        let path = |rest: &str| {
            format!(
                "/3/tv/{}/{below}/{rest}",
                item.external_ids.tmdb.unwrap_or(id)
            )
        };
        let part_of = |name: &str| match episode {
            Some(episode) => shape::episode_part(&work, season, episode, name, &context.filters),
            None => shape::season_part(&work, season, name, &context.filters),
        };

        if let Some(name) = part {
            return Ok(match part_of(&name) {
                Part::Doc(document) => Some((StatusCode::OK, document)),
                Part::Elsewhere { empty } if item.external_ids.tmdb.is_none() => {
                    empty.map(|empty| (StatusCode::OK, empty))
                }
                Part::Elsewhere { .. } | Part::Unknown => None,
            });
        }

        let appended = appends(&parts.uri);
        if appended.len() > shape::MOST_APPENDED {
            return Ok(None);
        }
        let document = match episode {
            Some(episode) => shape::episode(&work, season, episode),
            None => shape::season(&work, season),
        };
        let Some(mut document) = document else {
            return Ok(None);
        };
        let mut added = Map::new();
        for name in appended {
            let value = match part_of(&name) {
                Part::Doc(document) => Some(document),
                Part::Elsewhere { empty } => match item.external_ids.tmdb {
                    Some(_) => graft(state, parts, &path(&name)).await.or(empty),
                    None => empty,
                },
                Part::Unknown => None,
            };
            if let Some(value) = value {
                added.insert(name, shape::appended(value));
            }
        }
        if let Value::Object(map) = &mut document {
            map.extend(added);
        }
        Ok(Some((StatusCode::OK, document)))
    }

    /// Another resource's document, asked of TMDB through the relay — its
    /// key, what it kept, its locks — with the request's own query less
    /// what it appended: none where TMDB did not answer it.
    async fn graft(state: &AppState, parts: &Parts, path: &str) -> Option<Value> {
        let query = relay::query_without(&parts.uri, &["append_to_response"]);
        let mut asked = parts.clone();
        asked.uri = match query {
            Some(query) => format!("{path}?{query}"),
            None => path.to_string(),
        }
        .parse()
        .ok()?;
        let answer = relayed(state.clone(), asked).await.ok()?;
        if answer.status() != StatusCode::OK {
            return None;
        }
        let bytes = axum::body::to_bytes(answer.into_body(), ANSWER_LIMIT as usize)
            .await
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn kind_path(kind: MediaKind) -> &'static str {
        match kind {
            MediaKind::Movie => "movie",
            MediaKind::Series => "tv",
        }
    }

    // ─── find ────────────────────────────────────────────────────────────

    /// `/3/find/{id}?external_source=…`: the work held here under an id of
    /// IMDb's, of TheTVDB's — or the number Sonarr keeps a series under —
    /// of Wikidata's or of TVRage's, where it has an id in TMDB's dialect.
    /// None for TMDB to answer otherwise.
    async fn find(
        state: &AppState,
        parts: &Parts,
        external_id: &str,
    ) -> AppResult<Option<(StatusCode, Value)>> {
        let Some(source) = asked(&parts.uri, "external_source") else {
            return Ok(None);
        };
        let found = match source.as_str() {
            "imdb_id" => match crate::domain::ids::normalize_imdb_id(external_id) {
                Some(imdb) => held_by(state, ExternalSource::Imdb, &imdb).await?,
                None => None,
            },
            "tvdb_id" => match external_id.parse::<i64>() {
                Ok(id) => series_by_client_id(state, id).await?,
                Err(_) => None,
            },
            "wikidata_id" => held_by(state, ExternalSource::Wikidata, external_id).await?,
            "tvrage_id" => held_by(state, ExternalSource::TvRage, external_id).await?,
            _ => None,
        };
        let Some(item) = found else {
            return Ok(None);
        };
        let Some(id) = answered(state, &item) else {
            return Ok(None);
        };
        let context = Context::listed(state, parts, &item).await?;
        let pictures = Kept {
            media: &state.media,
            gate: context.gate,
        };
        let result = shape::result(&context.work(&item, id, &pictures), true);
        let document = match item.kind {
            MediaKind::Movie => shape::found(vec![result], Vec::new()),
            MediaKind::Series => shape::found(Vec::new(), vec![result]),
        };
        Ok(Some((StatusCode::OK, document)))
    }

    /// A work held here by an id of another site's.
    async fn held_by(
        state: &AppState,
        source: ExternalSource,
        value: &str,
    ) -> AppResult<Option<MediaItem>> {
        match repo::item::find_id_by_external(&state.db, source, value).await? {
            Some(found) => Ok(service::load(state, &found).await?),
            None => Ok(None),
        }
    }

    /// A series held here by the number Sonarr keeps it under: TheTVDB's
    /// id, or one this server made of TMDB's id, of Fankai's, or of its own
    /// ([`ids::for_client`]). Only what is held: nothing is fetched.
    async fn series_by_client_id(state: &AppState, id: i64) -> AppResult<Option<MediaItem>> {
        let found = if ids::is_own(id) {
            service::own_holders(state, MediaKind::Series)
                .await?
                .remove(&id)
        } else if let Some(fankai) = ids::from_fankai(id) {
            repo::item::find_id_by_external(&state.db, ExternalSource::Fankai, &fankai.to_string())
                .await?
        } else if let Some(tmdb) = ids::from_synthetic(id) {
            repo::item::find_id_by_external(&state.db, ExternalSource::TmdbTv, &tmdb.to_string())
                .await?
        } else {
            repo::item::find_id_by_external(&state.db, ExternalSource::TvdbSeries, &id.to_string())
                .await?
        };
        match found {
            Some(found) => Ok(service::load(state, &found)
                .await?
                .filter(|item| item.kind == MediaKind::Series)),
            None => Ok(None),
        }
    }

    // ─── search ──────────────────────────────────────────────────────────

    /// The first page of a search, the works the catalogue finds by the term
    /// first — entered by hand or corrected, in TMDB's shape — then TMDB's,
    /// less those it lists already; its count is both together. TMDB failing
    /// with nothing kept to stand in, the catalogue's alone. Any other page,
    /// or a term the catalogue finds nothing by, is TMDB's.
    async fn search(
        state: &AppState,
        parts: &Parts,
        kind: Option<MediaKind>,
    ) -> AppResult<Option<Response>> {
        let Some(term) = asked(&parts.uri, "query") else {
            return Ok(None);
        };
        if asked(&parts.uri, "page").is_some_and(|page| page.trim() != "1") {
            return Ok(None);
        }
        let year = match kind {
            Some(MediaKind::Movie) => {
                asked(&parts.uri, "primary_release_year").or_else(|| asked(&parts.uri, "year"))
            }
            Some(MediaKind::Series) => {
                asked(&parts.uri, "first_air_date_year").or_else(|| asked(&parts.uri, "year"))
            }
            None => None,
        }
        .and_then(|year| year.trim().parse::<i32>().ok());

        let ours = catalogue(state, parts, kind, term.trim(), year).await?;
        if ours.is_empty() {
            return Ok(None);
        }
        let listed: HashSet<(String, i64)> = ours
            .iter()
            .map(|(kind, result)| {
                (
                    kind_path(*kind).to_string(),
                    result["id"].as_i64().unwrap_or(0),
                )
            })
            .collect();
        let of_ours: Vec<Value> = ours.into_iter().map(|(_, result)| result).collect();

        let theirs = match relayed(state.clone(), parts.clone()).await {
            Ok(answer) if answer.status() == StatusCode::OK => {
                axum::body::to_bytes(answer.into_body(), ANSWER_LIMIT as usize)
                    .await
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            }
            _ => None,
        };
        let page = merged(of_ours, theirs, kind, &listed);
        Ok(Some(respond(parts, StatusCode::OK, &page)))
    }

    /// The works the catalogue finds by a term, of a kind or of either, best
    /// first, each as TMDB's search lists one, with its kind: those with an
    /// id in TMDB's dialect, switched on, for adults where this server shows
    /// them.
    async fn catalogue(
        state: &AppState,
        parts: &Parts,
        kind: Option<MediaKind>,
        term: &str,
        year: Option<i32>,
    ) -> AppResult<Vec<(MediaKind, Value)>> {
        let query = repo::item::Query {
            term: Some(term.to_string()),
            kind,
            year,
            sort: repo::item::Sort::Relevance,
            // By its letters alone, as the catalogue's half of Sonarr's
            // search: TMDB's half, which follows, forgives a misspelling.
            tolerance: Some(Vec::new()),
            limit: state.search_limit() as i64,
            include_adult: state.adult_visible(),
            ..Default::default()
        };
        let mut results = Vec::new();
        for found in repo::item::search(&state.db, &query).await? {
            let Some(item) = service::load(state, &found.id).await? else {
                continue;
            };
            let Some(id) = answered(state, &item) else {
                continue;
            };
            let context = Context::listed(state, parts, &item).await?;
            let pictures = Kept {
                media: &state.media,
                gate: context.gate,
            };
            let work = context.work(&item, id, &pictures);
            results.push((item.kind, shape::result(&work, kind.is_none())));
        }
        Ok(results)
    }

    /// The catalogue's works laid before TMDB's page, less those of TMDB's
    /// it lists already — by the kind and the id — counted with them; or
    /// alone, as a page of their own, when TMDB gave none.
    fn merged(
        ours: Vec<Value>,
        theirs: Option<Value>,
        kind: Option<MediaKind>,
        listed: &HashSet<(String, i64)>,
    ) -> Value {
        let count = ours.len();
        let Some(mut page) = theirs.filter(|page| page.get("results").is_some_and(Value::is_array))
        else {
            return json!({
                "page": 1,
                "results": ours,
                "total_pages": 1,
                "total_results": count,
            });
        };
        let their_results = match page.get_mut("results") {
            Some(Value::Array(results)) => std::mem::take(results),
            _ => Vec::new(),
        };
        let before = their_results.len();
        let kept: Vec<Value> = their_results
            .into_iter()
            .filter(|result| {
                let of = match kind {
                    Some(kind) => Some(kind_path(kind)),
                    None => result.get("media_type").and_then(Value::as_str),
                };
                let id = result.get("id").and_then(Value::as_i64);
                match (of, id) {
                    (Some(of), Some(id)) => !listed.contains(&(of.to_string(), id)),
                    _ => true,
                }
            })
            .collect();
        let dropped = before - kept.len();
        let total = page
            .get("total_results")
            .and_then(Value::as_u64)
            .unwrap_or(before as u64);
        page["results"] = Value::Array(ours.into_iter().chain(kept).collect());
        page["total_results"] = json!((total + count as u64).saturating_sub(dropped as u64));
        if page.get("total_pages").and_then(Value::as_u64).unwrap_or(0) == 0 {
            page["total_pages"] = json!(1);
        }
        page["page"] = json!(1);
        page
    }
}

/// The works changed here, told beside TMDB's own changes:
/// `/3/movie/changes` and `/3/tv/changes` list on their first page, after
/// TMDB's ids, those of the works a lock was put on or lifted on here, a
/// work added or taken out by hand, or a refresh changed, in the window
/// asked ([`with_changes_here`]). A client that keeps itself in step by
/// changes, as Yamtrack does, then fetches again a work corrected here
/// without waiting for its own full refresh. When TMDB fails, the feed
/// hands its failure on — neither what was kept nor this server's changes
/// alone: a client moving a cursor along asks the same window again,
/// rather than miss what TMDB changed meanwhile.
///
/// What a page becomes, and the window it asks about.
mod changes {
    use std::collections::HashSet;

    use axum::http::Uri;
    use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
    use serde_json::{Value, json};

    use crate::domain::MediaKind;

    /// The longest window TMDB lists changes over: a longer one is cut to
    /// it.
    pub const AT_MOST: TimeDelta = TimeDelta::days(14);

    /// The window TMDB lists changes over when no start is asked: the last
    /// day.
    pub const BY_DEFAULT: TimeDelta = TimeDelta::hours(24);

    /// The kind of work a path of TMDB's lists the changes of.
    pub fn of(path: &str) -> Option<MediaKind> {
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        match segments.as_slice() {
            ["3", "movie", "changes"] => Some(MediaKind::Movie),
            ["3", "tv", "changes"] => Some(MediaKind::Series),
            _ => None,
        }
    }

    /// The last value of a parameter of the query, as TMDB reads one: none
    /// for one not asked, or asked empty.
    fn asked<'a>(uri: &'a Uri, name: &str) -> Option<std::borrow::Cow<'a, str>> {
        url::form_urlencoded::parse(uri.query()?.as_bytes())
            .filter(|(key, _)| key == name)
            .last()
            .map(|(_, value)| value)
            .filter(|value| !value.trim().is_empty())
    }

    /// The window a request for changes asks about, as TMDB reads it, in
    /// UTC: from the start of `start_date` to the end of `end_date`, or to
    /// `now`; the last day before its end when no start is asked; never
    /// longer than fourteen days, its start brought forward to fit. None
    /// when a date asked is not one, or the window ends before it starts:
    /// TMDB answers that as it sees fit, and nothing is added to it.
    pub fn window(uri: &Uri, now: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let day = |name: &str| -> Result<Option<NaiveDate>, ()> {
            match asked(uri, name) {
                None => Ok(None),
                Some(text) => NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                    .map(Some)
                    .map_err(|_| ()),
            }
        };
        let (start, end) = (day("start_date").ok()?, day("end_date").ok()?);
        let until = match end {
            Some(day) => day.succ_opt()?.and_hms_opt(0, 0, 0)?.and_utc(),
            None => now,
        };
        let from = match start {
            Some(day) => day.and_hms_opt(0, 0, 0)?.and_utc(),
            None => until - BY_DEFAULT,
        };
        (from < until).then(|| (from.max(until - AT_MOST), until))
    }

    /// Whether the page asked is the first, the one the works changed here
    /// are added to: no page asked, or `page=1`.
    pub fn first_page(uri: &Uri) -> bool {
        asked(uri, "page").is_none_or(|page| page.trim() == "1")
    }

    /// A page of TMDB's changes with the works changed here added after
    /// TMDB's own: each by its TMDB id and whether it is for adults, as
    /// TMDB lists one, and none twice — not one TMDB lists already. Its
    /// count grows by as many, and a page that listed nothing is one page.
    /// Whether any was added.
    pub fn with_here(document: &mut Value, here: &[(i64, bool)]) -> bool {
        let Some(Value::Array(results)) = document.get_mut("results") else {
            return false;
        };
        let mut listed: HashSet<i64> = results
            .iter()
            .filter_map(|result| result.get("id").and_then(Value::as_i64))
            .collect();
        let before = results.len();
        for (id, adult) in here {
            if listed.insert(*id) {
                results.push(json!({ "id": id, "adult": adult }));
            }
        }
        let added = results.len() - before;
        if added == 0 {
            return false;
        }
        if let Some(total) = document.get("total_results").and_then(Value::as_u64) {
            document["total_results"] = json!(total + added as u64);
        }
        if document.get("total_pages").and_then(Value::as_u64) == Some(0) {
            document["total_pages"] = json!(1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_path_that_climbs_out_of_v3_is_not_a_tmdb_path() {
        use super::climbs;
        // The outgoing URL is built by interpolation and parsed by reqwest,
        // which resolves dot segments — so this one would have left the v3 API
        // for the v4 one, carrying the operator's credentials with it.
        assert!(climbs("/3/%2e%2e/4/account"));
        // A backslash is a slash to that parser: this one would have left the
        // public lists for an account's, carrying the operator's token.
        assert!(climbs("/4/list/..\\account/1/lists"));
        assert!(climbs("/4/list/8136%2F..%2F..%2Faccount"));
        assert!(climbs("/3/movie/238%5C..%5C4"));
        // The parser's own reading of it — why the outgoing URL is checked
        // against the path too, whatever `climbs` misses.
        let url = url::Url::parse("https://api.themoviedb.org/4/list/..\\account/1/lists").unwrap();
        assert_eq!(url.path(), "/4/account/1/lists");
        assert!(!url.path().ends_with("/4/list/..\\account/1/lists"));
        let url = url::Url::parse("https://api.themoviedb.org/3/movie/238?api_key=x").unwrap();
        assert!(url.path().ends_with("/3/movie/238"));
    }

    #[test]
    fn of_tmdbs_v4_only_the_public_lists_are_relayed() {
        assert!(v4_allowed("/4/list/8136"));
        assert!(v4_allowed("/4/list/8136/item_status"));
        for refused in [
            "/4/list/",
            "/4/list",
            "/4/account/123/movie/watchlist",
            "/4/auth/request_token",
            "/4/",
        ] {
            assert!(!v4_allowed(refused), "{refused}");
        }
        assert!(climbs("/3/../4/account"));
        assert!(climbs("/3/tv/%2E%2E/list"));
        assert!(climbs("/3/./tv/1396"));

        assert!(!climbs("/3/tv/1396"));
        assert!(!climbs("/3/search/movie"));
        assert!(!climbs("/3/movie/550/credits"));
        // A dot inside a segment is just a character.
        assert!(!climbs("/3/configuration/countries.json"));
    }

    use std::collections::HashSet;

    use axum::http::HeaderValue;
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::{
        api::relay::testing::{laid_for, read_with},
        domain::{Image, Season},
    };

    fn uri(text: &str) -> Uri {
        text.parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn the_relay_strips_every_credential_the_caller_presented_to_us() {
        // A key accepted here must never be forwarded upstream, in any of the
        // spellings the guard accepts — nor the session cookie, which reaches
        // this route because the interface is same-origin with it.
        for name in crate::auth::middleware::API_KEY_PARAMS {
            assert!(
                OVERRIDDEN_PARAMS.contains(name),
                "{name} authenticates here but is relayed to TMDB"
            );
        }

        let asked = headers(&[
            ("user-agent", "Jellyseerr"),
            ("accept", "application/json"),
            ("accept-language", "fr-FR"),
            ("cookie", "ams_session=secret"),
            ("authorization", "Bearer ams_key"),
            ("x-api-key", "ams_key"),
            ("x-forwarded-for", "192.168.1.20"),
            ("forwarded", "for=192.168.1.20"),
            ("referer", "https://ams.lan/work/x"),
            ("origin", "https://ams.lan"),
            (crate::providers::radarr::LOOP_HEADER, "another-instance"),
            ("host", "api.themoviedb.org"),
            ("connection", "keep-alive"),
            ("accept-encoding", "gzip"),
            ("range", "bytes=0-0"),
            ("if-range", "\"tag\""),
            ("if-match", "\"tag\""),
            ("if-none-match", "\"tag\""),
            ("if-modified-since", "Wed, 21 Oct 2015 07:28:00 GMT"),
        ]);

        for validators in [false, true] {
            let forwarded = forwarded_headers(&asked, validators);
            assert_eq!(forwarded.get(header::USER_AGENT).unwrap(), "Jellyseerr");
            assert_eq!(forwarded.get(header::ACCEPT).unwrap(), "application/json");
            assert_eq!(forwarded.get(header::ACCEPT_LANGUAGE).unwrap(), "fr-FR");
            for never in [
                "cookie",
                "authorization",
                "x-api-key",
                "x-forwarded-for",
                "forwarded",
                "referer",
                "origin",
                crate::providers::radarr::LOOP_HEADER,
                "host",
                "connection",
                "accept-encoding",
                "range",
                "if-range",
                "if-match",
            ] {
                assert!(forwarded.get(never).is_none(), "{never} was forwarded");
            }
            // The validators, only where TMDB may answer them.
            assert_eq!(forwarded.get(header::IF_NONE_MATCH).is_some(), validators);
            assert_eq!(
                forwarded.get(header::IF_MODIFIED_SINCE).is_some(),
                validators
            );
        }
    }

    #[test]
    fn an_answer_names_no_address_of_tmdbs_and_takes_no_cookie() {
        let answer = headers(&[
            ("content-type", "application/json;charset=utf-8"),
            (
                "location",
                "https://api.themoviedb.org/3/movie/1?api_key=0123456789abcdef",
            ),
            ("content-location", "/3/movie/1?api_key=0123456789abcdef"),
            ("set-cookie", "a=b"),
            ("access-control-allow-origin", "*"),
            ("etag", "W/\"x\""),
        ]);
        let kept = answer_headers(&answer);
        assert!(kept.get(header::CONTENT_TYPE).is_some());
        assert!(kept.get(header::ETAG).is_some());
        for gone in [
            "location",
            "content-location",
            "set-cookie",
            "access-control-allow-origin",
        ] {
            assert!(kept.get(gone).is_none(), "{gone} was kept");
        }
    }

    #[test]
    fn only_a_read_nobodys_session_is_in_is_kept() {
        for kept in [
            "/3/movie/550",
            "/3/movie/550?language=fr-FR&append_to_response=credits",
            "/3/search/movie?query=account",
            "/3/configuration",
            "/3/tv/1396/season/1",
            "/4/list/8136",
        ] {
            assert!(may_keep(&Method::GET, &uri(kept)), "{kept}");
        }
        for personal in [
            // Minted for one caller.
            "/3/authentication/token/new",
            "/3/authentication/guest_session/new",
            "/3/authentication",
            // Read with one caller's session.
            "/3/guest_session/abc/rated/movies",
            "/3/account",
            "/3/account/1/favorite/movies?session_id=s",
            "/3/movie/550/account_states?session_id=s",
            "/3/movie/550?guest_session_id=g",
            "/3/movie/550?Session_ID=s",
            "/3/movie/550?session%5Fid=s",
            // Spelt so that a plain comparison misses it.
            "/3/%61ccount/1/lists",
            "/3/Account/1",
            "/3//account/1",
        ] {
            assert!(!may_keep(&Method::GET, &uri(personal)), "{personal}");
        }
        // A HEAD is never kept, nor answered from what is.
        assert!(!may_keep(&Method::HEAD, &uri("/3/movie/550")));
    }

    #[test]
    fn a_kept_document_is_told_apart_by_everything_that_changes_it() {
        let none = HeaderMap::new();

        // One request, however its query is spelt, and whatever key it
        // carried for this server.
        assert_eq!(
            filed_under(&uri("/3/movie/550?language=fr&page=1"), &none),
            filed_under(
                &uri("/3/movie/550?page=1&api_key=ams_x&language=fr&include_adult=true"),
                &none
            )
        );
        assert_eq!(
            filed_under(&uri("/3/movie/550?language=fr"), &none),
            filed_under(&uri("/3/movie/550?language=fr&include%5Fadult=true"), &none)
        );

        // Another language, another order of a parameter given twice,
        // another path: another answer.
        let french = filed_under(&uri("/3/movie/550?language=fr"), &none);
        assert_ne!(french, filed_under(&uri("/3/movie/550?language=de"), &none));
        assert_ne!(
            filed_under(&uri("/3/movie/550?language=de&language=fr"), &none),
            filed_under(&uri("/3/movie/550?language=fr&language=de"), &none)
        );
        assert_ne!(french, filed_under(&uri("/3/tv/550?language=fr"), &none));

        // The headers that travel and may change the answer.
        let path = uri("/3/movie/550");
        let plain = filed_under(&path, &none);
        assert_ne!(
            plain,
            filed_under(&path, &headers(&[("accept-language", "de-DE")]))
        );
        assert_ne!(
            plain,
            filed_under(&path, &headers(&[("accept", "application/xml")]))
        );
        // What does not travel, or does not change TMDB's answer, files
        // nothing apart.
        assert_eq!(
            plain,
            filed_under(
                &path,
                &headers(&[
                    ("range", "bytes=0-0"),
                    ("user-agent", "x"),
                    ("cookie", "a=b")
                ])
            )
        );
        // A value cannot pass for another header's.
        assert_ne!(
            filed_under(
                &path,
                &headers(&[("accept", "x&accept-language=y"), ("accept-language", "z")])
            ),
            filed_under(
                &path,
                &headers(&[("accept", "x"), ("accept-language", "y&accept-language=z")])
            )
        );
    }

    #[test]
    fn upstream_is_asked_without_the_parameters_this_server_sets() {
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri(
                "/3/movie/550?language=fr&include%5Fadult=true&API_KEY=theirs&apikey=ams_x&adult&page=2",
            ),
            &Asking::Operator("0123456789abcdef".into()),
            false,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/3/movie/550?language=fr&adult&page=2\
             &api_key=0123456789abcdef&include_adult=false"
        );

        // A v4 token travels in a header, not in the address.
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri("/4/list/8136?page=1"),
            &Asking::Operator("eyJtoken".into()),
            true,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/4/list/8136?page=1&include_adult=true"
        );
    }

    #[test]
    fn the_clients_own_key_travels_as_it_came_and_nothing_of_ours_does() {
        // Their `api_key` is TMDB's own parameter, and theirs: handed on.
        // `apikey` is this server's, and the word on adult titles too.
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri("/3/movie/550?api_key=their-tmdb-key&language=fr&apikey=x&include_adult=true"),
            &Asking::Client,
            false,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/3/movie/550?api_key=their-tmdb-key&language=fr\
             &include_adult=false"
        );
        // No key at all: none is added, and TMDB says so itself.
        let asked = upstream_url(
            "https://api.themoviedb.org",
            &uri("/3/movie/550"),
            &Asking::Client,
            false,
        );
        assert_eq!(
            asked,
            "https://api.themoviedb.org/3/movie/550?include_adult=false"
        );
    }

    #[test]
    fn the_language_asked_is_tmdbs_reading_of_the_query() {
        assert_eq!(asked_language(&uri("/3/movie/1?language=de-DE")), "de-DE");
        assert_eq!(
            asked_language(&uri("/3/movie/1?language=en&language=fr")),
            "fr"
        );
        // TMDB answers `en-US` when no language is asked.
        assert_eq!(asked_language(&uri("/3/movie/1")), "en-US");
        assert_eq!(asked_language(&uri("/3/movie/1?language=")), "en-US");
    }

    #[test]
    fn a_homepage_and_the_genres_are_locked_in_the_language_of_the_catalogue() {
        let asks = |text: &str, catalogue: &str| asks_in(&asked_language(&uri(text)), catalogue);
        assert!(asks("/3/movie/1?language=fr-FR", "fr-FR"));
        assert!(asks("/3/movie/1?language=fr", "fr-FR"));
        assert!(asks("/3/movie/1?language=fr-CA", "fr-FR"));
        assert!(asks("/3/movie/1?language=en&language=fr", "fr-FR"));
        assert!(asks("/3/movie/1", "en-US"));
        assert!(asks("/3/movie/1?language=", "en-US"));
        assert!(!asks("/3/movie/1", "fr-FR"));

        assert!(!asks("/3/movie/1?language=de-DE", "fr-FR"));
        assert!(!asks("/3/movie/1?language=fr&language=de", "fr-FR"));
        assert!(!asks("/3/movie/1?language=../../x", "fr-FR"));
    }

    #[test]
    fn the_documents_patched_are_recognised() {
        assert_eq!(
            patch_target("/3/tv/1396"),
            Some(Target::Title {
                kind: MediaKind::Series,
                tmdb_id: 1396
            })
        );
        assert_eq!(
            patch_target("/3/movie/329865"),
            Some(Target::Title {
                kind: MediaKind::Movie,
                tmdb_id: 329865
            })
        );
        assert_eq!(
            patch_target("/3/tv/1396/"),
            Some(Target::Title {
                kind: MediaKind::Series,
                tmdb_id: 1396
            })
        );
        assert_eq!(
            patch_target("/3/tv/1396/season/1"),
            Some(Target::Season {
                tmdb_id: 1396,
                season: 1
            })
        );
        assert_eq!(
            patch_target("/3/tv/1396/season/0/episode/3"),
            Some(Target::Episode { tmdb_id: 1396 })
        );
        assert_eq!(
            patch_target("/3/configuration"),
            Some(Target::Configuration)
        );
        for (path, kind) in [
            ("/3/search/movie", Some(MediaKind::Movie)),
            ("/3/search/tv", Some(MediaKind::Series)),
            ("/3/search/multi", None),
            ("/3/trending/movie/week", Some(MediaKind::Movie)),
            ("/3/trending/tv/day", Some(MediaKind::Series)),
            ("/3/trending/all/day", None),
            ("/3/discover/movie", Some(MediaKind::Movie)),
            ("/3/discover/tv", Some(MediaKind::Series)),
        ] {
            assert_eq!(patch_target(path), Some(Target::Results { kind }), "{path}");
        }
    }

    #[test]
    fn other_documents_are_left_alone() {
        // A season's people are not the work's credits; a film has no
        // aggregate; a list TMDB keeps besides is none a person hides from.
        assert_eq!(patch_target("/3/tv/1396/season/1/credits"), None);
        assert_eq!(patch_target("/3/tv/1396/season/1/aggregate_credits"), None);
        assert_eq!(patch_target("/3/tv/1396/season/1/episode/1/credits"), None);
        assert_eq!(patch_target("/3/movie/329865/aggregate_credits"), None);
        assert_eq!(patch_target("/3/movie/329865/keywords"), None);
        assert_eq!(patch_target("/3/movie/329865/credits/extra"), None);
        assert_eq!(patch_target("/3/tv/1396/season/x"), None);
        assert_eq!(patch_target("/3/tv/1396/season/1/episode/-1"), None);
        assert_eq!(patch_target("/3/search/person"), None);
        assert_eq!(patch_target("/3/search/collection"), None);
        assert_eq!(patch_target("/3/trending/person/day"), None);
        assert_eq!(patch_target("/3/trending/all"), None);
        // The configuration's lists: no picture's address in them.
        assert_eq!(patch_target("/3/configuration/countries"), None);
        assert_eq!(patch_target("/3/configuration/languages"), None);
        assert_eq!(patch_target("/3/movie/changes"), None);
        assert_eq!(patch_target("/3/tv/changes"), None);
        assert_eq!(patch_target("/3/tv/not-a-number"), None);
        assert_eq!(patch_target("/3/tv"), None);
        assert_eq!(patch_target("/other/tv/1"), None);
        assert_eq!(patch_target("/4/list/8136"), None);
    }

    fn as_is(address: &str) -> String {
        address.to_string()
    }

    /// *Fight Club* as its sources gave it: two actors, a director, a
    /// writer, two posters and three other titles.
    pub(super) fn fight_club() -> MediaItem {
        let credit = |kind: CreditType, id: i64, name: &str| crate::domain::Credit {
            id: crate::db::new_id(),
            credit_type: kind,
            person_name: name.into(),
            character_name: None,
            image: None,
            tmdb_person_id: Some(id),
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
            department: None,
            job: None,
        };
        let poster = |path: &str| Image {
            id: crate::db::new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: format!("https://image.tmdb.org/t/p/original{path}"),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        };
        let title = |text: &str, country: &str| crate::domain::AlternativeTitle {
            id: crate::db::new_id(),
            title: text.into(),
            title_type: None,
            language: Some(country.into()),
            is_manual: false,
        };
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Fight Club".into();
        item.slug = "fight-club-1999".into();
        item.external_ids.tmdb = Some(550);
        item.credits = vec![
            credit(CreditType::Actor, 819, "Edward Norton"),
            credit(CreditType::Actor, 287, "Brad Pitt"),
            credit(CreditType::Director, 7467, "David Fincher"),
            credit(CreditType::Writer, 7468, "Jim Uhls"),
        ];
        item.images = vec![poster("/kept.jpg"), poster("/hidden.jpg")];
        item.alternative_titles = vec![
            title("El club de la pelea", "mex"),
            title("Fight Club – Hidden", "usa"),
        ];
        item
    }

    /// What a person hid of it: an actor, the director, a poster, a title.
    pub(super) fn hidden_of_fight_club() -> Vec<(&'static str, &'static str, &'static str, Value)> {
        vec![
            ("credit:actor:tmdb:819", "hidden", "", json!(true)),
            ("credit:director:tmdb:7467", "hidden", "", json!(true)),
            (
                "image:https://image.tmdb.org/t/p/original/hidden.jpg",
                "hidden",
                "",
                json!(true),
            ),
            (
                "alternativeTitle:usa:Fight Club – Hidden",
                "hidden",
                "",
                json!(true),
            ),
        ]
    }

    /// The ids of a list of TMDB's people, in order.
    fn ids(list: &Value) -> Vec<i64> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_i64().unwrap())
            .collect()
    }

    #[test]
    fn the_lists_a_titles_sources_gave_are_documents_patched_too() {
        let part = |kind, tmdb_id, part| {
            Some(Target::Part {
                kind,
                tmdb_id,
                part,
            })
        };
        assert_eq!(
            patch_target("/3/movie/550/credits"),
            part(MediaKind::Movie, 550, Part::People)
        );
        assert_eq!(
            patch_target("/3/tv/1396/aggregate_credits"),
            part(MediaKind::Series, 1396, Part::People)
        );
        assert_eq!(
            patch_target("/3/movie/550/images"),
            part(MediaKind::Movie, 550, Part::Pictures)
        );
        assert_eq!(
            patch_target("/3/tv/1396/season/2/images"),
            part(MediaKind::Series, 1396, Part::Pictures)
        );
        assert_eq!(
            patch_target("/3/tv/1396/alternative_titles"),
            part(MediaKind::Series, 1396, Part::Titles)
        );
        assert_eq!(patch_target("/3/movie/x/credits"), None);
    }

    #[test]
    fn what_a_person_hid_is_taken_out_of_tmdbs_lists() {
        let item = read_with(fight_club(), &hidden_of_fight_club());
        assert_eq!(item.hidden.credits.len(), 2);

        let Value::Object(mut document) = json!({
            "id": 550,
            "title": "Fight Club",
            "credits": {
                "cast": [
                    { "id": 819, "name": "Edward Norton", "character": "Narrator" },
                    { "id": 287, "name": "Brad Pitt", "character": "Tyler Durden" }
                ],
                "crew": [
                    { "id": 7467, "name": "David Fincher", "job": "Director", "department": "Directing" },
                    // The same person in another job is another credit.
                    { "id": 7467, "name": "David Fincher", "job": "Producer", "department": "Production" },
                    { "id": 7468, "name": "Jim Uhls", "job": "Screenplay", "department": "Writing" }
                ]
            },
            "images": {
                "posters": [ { "file_path": "/kept.jpg" }, { "file_path": "/hidden.jpg" } ],
                "backdrops": [ { "file_path": "/backdrop.jpg" } ],
                "logos": []
            },
            "alternative_titles": { "titles": [
                { "iso_3166_1": "MX", "title": "El club de la pelea" },
                { "iso_3166_1": "US", "title": " fight club – hidden " },
                // The same text, filed for another country: another title.
                { "iso_3166_1": "GB", "title": "Fight Club – Hidden" }
            ] }
        }) else {
            unreachable!()
        };
        assert!(take_hidden(&mut document, &item, &as_is));

        assert_eq!(ids(&document["credits"]["cast"]), [287]);
        let crew: Vec<(i64, &str)> = document["credits"]["crew"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["id"].as_i64().unwrap(), c["job"].as_str().unwrap()))
            .collect();
        assert_eq!(crew, [(7467, "Producer"), (7468, "Screenplay")]);
        assert_eq!(
            document["images"]["posters"],
            json!([{ "file_path": "/kept.jpg" }])
        );
        assert_eq!(document["images"]["backdrops"].as_array().unwrap().len(), 1);
        let countries: Vec<&str> = document["alternative_titles"]["titles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["iso_3166_1"].as_str().unwrap())
            .collect();
        assert_eq!(countries, ["MX", "GB"]);
        assert_eq!(document["title"], json!("Fight Club"));

        // Taken out once: nothing more to take.
        assert!(!take_hidden(&mut document, &item, &as_is));
    }

    #[test]
    fn a_crew_credit_hidden_takes_every_crew_job_of_the_person_and_no_other() {
        let withheld = Withheld {
            credits: vec![(CreditType::Crew, 7)],
            pictures: HashSet::new(),
            titles: Vec::new(),
        };
        let mut crew = vec![
            json!({ "id": 7, "department": "Sound", "job": "Original Music Composer" }),
            json!({ "id": 7, "department": "Camera", "job": "Director of Photography" }),
            json!({ "id": 7, "department": "Directing", "job": "Director" }),
            json!({ "id": 8, "department": "Editing", "job": "Editor" }),
        ];
        assert!(take_crew(&mut crew, &withheld));
        let left: Vec<(i64, &str)> = crew
            .iter()
            .map(|c| (c["id"].as_i64().unwrap(), c["job"].as_str().unwrap()))
            .collect();
        assert_eq!(left, [(7, "Director"), (8, "Editor")]);
    }

    #[test]
    fn a_series_aggregate_and_its_creators_lose_what_was_hidden_alone() {
        let withheld = Withheld {
            credits: vec![
                (CreditType::Actor, 17419),
                (CreditType::Director, 66633),
                (CreditType::Writer, 66633),
            ],
            pictures: HashSet::new(),
            titles: Vec::new(),
        };
        let Value::Object(mut aggregate) = json!({
            "cast": [
                { "id": 17419, "roles": [ { "character": "Walter White" } ] },
                { "id": 84497, "roles": [ { "character": "Jesse Pinkman" } ] }
            ],
            "crew": [
                // Of a person's jobs, the hidden ones alone go.
                { "id": 66633, "department": "Directing",
                  "jobs": [ { "job": "Director" }, { "job": "Second Unit Director" } ] },
                // Every job a writer's, whatever it is called: the person goes.
                { "id": 66633, "department": "Writing", "jobs": [ { "job": "Writer" } ] },
                { "id": 66633, "department": "Production", "jobs": [ { "job": "Executive Producer" } ] },
                { "id": 29924, "department": "Directing", "jobs": [ { "job": "Director" } ] }
            ]
        }) else {
            unreachable!()
        };
        assert!(take_people(&mut aggregate, &withheld));
        assert_eq!(ids(&aggregate["cast"]), [84497]);
        assert_eq!(ids(&aggregate["crew"]), [66633, 66633, 29924]);
        assert_eq!(
            aggregate["crew"][0]["jobs"],
            json!([{ "job": "Second Unit Director" }])
        );
        assert_eq!(aggregate["crew"][1]["department"], json!("Production"));

        // A series' creator is one of its writers.
        let Value::Object(mut series) = json!({
            "created_by": [ { "id": 66633, "name": "Vince Gilligan" } ],
            "name": "Breaking Bad"
        }) else {
            unreachable!()
        };
        assert!(take_people(&mut series, &withheld));
        assert_eq!(series["created_by"], json!([]));
    }

    fn series_document() -> Map<String, Value> {
        let Value::Object(map) = json!({
            "id": 1396,
            "name": "Breaking Bad",
            "original_name": "Breaking Bad",
            "overview": "A chemistry teacher…",
            "homepage": "https://www.sonypictures.com/tv/breakingbad",
            "first_air_date": "2008-01-20",
            "last_air_date": "2013-09-29",
            "status": "Ended",
            "episode_run_time": [45, 47],
            "genres": [
                { "id": 18, "name": "Drame" },
                { "id": 80, "name": "Crime" },
                { "id": 9648, "name": "Mystère" }
            ],
            "poster_path": "/ggFHVNu6YYI5L9pCfOacjizRGt.jpg",
            "backdrop_path": "/tsRy63Mu5cu8etL1X7ZLyf7UP1M.jpg"
        }) else {
            unreachable!()
        };
        map
    }

    fn locked_series(fields: &[&str]) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.original_title = Some("Mon titre original".into());
        item.overview = Some("Mon résumé".into());
        item.homepage = Some("https://example.fr".into());
        item.first_aired = Some("2008-01-21".into());
        item.last_aired = Some("2013-09-30".into());
        item.status = Some("continuing".into());
        item.runtime = Some(50);
        item.genres = vec!["Crime".into(), "drame".into()];
        item.locked_fields = fields.iter().map(|f| format!("item/{f}")).collect();
        item
    }

    #[test]
    fn only_the_fields_locked_are_written() {
        let mut document = series_document();
        let item = locked_series(&["title"]);

        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));

        assert_eq!(document["name"], json!("Mon titre"));
        // Not locked: as TMDB gave it, whatever the catalogue holds.
        let upstream = series_document();
        for field in [
            "overview",
            "homepage",
            "original_name",
            "first_air_date",
            "last_air_date",
            "status",
            "episode_run_time",
            "genres",
            "poster_path",
        ] {
            assert_eq!(document[field], upstream[field], "{field}");
        }
    }

    #[test]
    fn every_lock_is_written_in_tmdbs_names() {
        let mut document = series_document();
        let item = locked_series(&[
            "title",
            "originalTitle",
            "overview",
            "homepage",
            "firstAired",
            "lastAired",
            "status",
            "runtime",
            "genres",
        ]);

        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));

        assert_eq!(document["name"], json!("Mon titre"));
        assert_eq!(document["original_name"], json!("Mon titre original"));
        assert_eq!(document["overview"], json!("Mon résumé"));
        assert_eq!(document["homepage"], json!("https://example.fr"));
        assert_eq!(document["first_air_date"], json!("2008-01-21"));
        assert_eq!(document["last_air_date"], json!("2013-09-30"));
        assert_eq!(document["status"], json!("Returning Series"));
        assert_eq!(document["episode_run_time"], json!([50]));
        // TMDB's own genres, its ids kept, in the order locked.
        assert_eq!(
            document["genres"],
            json!([{ "id": 80, "name": "Crime" }, { "id": 18, "name": "Drame" }])
        );
        assert!(!document.contains_key("title"));
    }

    /// A series locked in a catalogue kept in French: its title in every
    /// language, its overview in French alone and in German alone, its
    /// homepage and its first date.
    fn locked_in_languages() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.overview = Some("A chemistry teacher…".into());
        read_with(
            item,
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "overview", "fra", json!("Mon résumé")),
                ("item", "overview", "deu", json!("Mein Überblick")),
                ("item", "homepage", "", json!("https://example.fr")),
                ("item", "firstAired", "", json!("2008-01-21")),
            ],
        )
    }

    #[test]
    fn a_text_is_written_as_it_is_locked_for_the_language_asked() {
        // In the catalogue's own language, every lock.
        let item = locked_in_languages();
        let mut document = series_document();
        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(document["name"], json!("Mon titre"));
        assert_eq!(document["overview"], json!("Mon résumé"));
        assert_eq!(document["homepage"], json!("https://example.fr"));

        // In German: the overview locked in German. The title locked in
        // every language is the catalogue's own, written in its language:
        // TMDB's German one stands, as its German homepage does.
        let german = laid_for(locked_in_languages(), "deu");
        let mut document = series_document();
        assert!(write_locked(
            &mut document,
            &german,
            MediaKind::Series,
            false,
            &as_is
        ));
        assert_eq!(document["name"], series_document()["name"]);
        assert_eq!(document["overview"], json!("Mein Überblick"));
        assert_eq!(document["homepage"], series_document()["homepage"]);
        // A date is one in every language.
        assert_eq!(document["first_air_date"], json!("2008-01-21"));

        // In Spanish: nothing was locked in it, and its texts are TMDB's
        // Spanish ones; the date locked in every language is written all
        // the same.
        let spanish = laid_for(locked_in_languages(), "spa");
        let mut document = series_document();
        assert!(write_locked(
            &mut document,
            &spanish,
            MediaKind::Series,
            false,
            &as_is
        ));
        assert_eq!(document["name"], series_document()["name"]);
        assert_eq!(document["overview"], series_document()["overview"]);
        assert_eq!(document["first_air_date"], json!("2008-01-21"));
    }

    #[test]
    fn a_genre_tmdb_has_no_id_for_leaves_tmdbs_genres_alone() {
        let mut document = series_document();
        let mut item = locked_series(&["genres"]);
        item.genres = vec!["Drame".into(), "Inventé ici".into()];

        assert!(!write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(document["genres"], series_document()["genres"]);

        // Never a genre without its id, whatever is locked.
        assert_eq!(
            tmdb_genres(
                &[json!({ "id": null, "name": "Drame" })],
                &["Drame".to_string()]
            ),
            None
        );
        assert_eq!(
            tmdb_genres(&[json!({ "id": 18, "name": "Drame" })], &[]),
            Some(vec![])
        );
    }

    #[test]
    fn a_movie_takes_its_locks_under_its_own_names() {
        let Value::Object(mut document) = json!({
            "id": 329865, "title": "Arrival", "original_title": "Arrival",
            "overview": "…", "release_date": "2016-11-10", "runtime": 116,
            "status": "Released"
        }) else {
            unreachable!()
        };
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Premier Contact".into();
        item.in_cinemas = Some("2016-12-07".into());
        item.runtime = Some(118);
        item.status = Some("released".into());
        item.locked_fields = vec![
            "item/title".into(),
            "item/inCinemas".into(),
            "item/runtime".into(),
            "item/status".into(),
        ];

        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Movie,
            true,
            &as_is
        ));

        assert_eq!(document["title"], json!("Premier Contact"));
        assert_eq!(document["release_date"], json!("2016-12-07"));
        assert_eq!(document["runtime"], json!(118));
        // A film's statuses are another set, and left alone.
        assert_eq!(document["status"], json!("Released"));
        assert!(!document.contains_key("name"));
    }

    #[test]
    fn an_answer_that_is_not_a_title_takes_nothing() {
        let Value::Object(mut error) = json!({
            "success": false, "status_code": 34,
            "status_message": "The resource you requested could not be found."
        }) else {
            unreachable!()
        };
        let item = locked_series(&["title", "overview", "firstAired", "runtime"]);
        assert!(!write_locked(
            &mut error,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(error.len(), 3);
    }

    #[test]
    fn a_result_takes_the_locks_it_carries_a_field_for() {
        // A result of a search: fewer fields than the title's own document.
        let Value::Object(mut result) = json!({
            "id": 1396, "name": "Breaking Bad", "original_name": "Breaking Bad",
            "overview": "…", "first_air_date": "2008-01-20",
            "poster_path": "/upstream.jpg", "genre_ids": [18], "popularity": 9.5
        }) else {
            unreachable!()
        };
        let item = locked_series(&["title", "overview", "firstAired", "homepage", "status"]);
        assert!(write_locked(
            &mut result,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(result["name"], json!("Mon titre"));
        assert_eq!(result["overview"], json!("Mon résumé"));
        assert_eq!(result["first_air_date"], json!("2008-01-21"));
        // What a result does not carry is not added to it.
        assert!(!result.contains_key("homepage"));
        assert!(!result.contains_key("status"));
        assert_eq!(result["genre_ids"], json!([18]));
    }

    #[test]
    fn a_result_says_its_kind_on_a_page_of_several() {
        assert_eq!(
            result_kind(&json!({ "media_type": "movie" })),
            Some(MediaKind::Movie)
        );
        assert_eq!(
            result_kind(&json!({ "media_type": "tv" })),
            Some(MediaKind::Series)
        );
        assert_eq!(result_kind(&json!({ "media_type": "person" })), None);
        assert_eq!(result_kind(&json!({ "id": 1 })), None);
    }

    #[test]
    fn a_chosen_picture_is_written_only_when_locked() {
        let mut item = locked_series(&[]);
        item.images.push(Image {
            id: "img1".into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: "https://ams.example/media/abc".into(),
            language: None,
            sort_order: 0,
            source: None,
            is_manual: true,
        });
        item.primary_images.poster = Some("img1".into());
        let origin = |_: &str| "https://image.tmdb.org/t/p/original/chosen.jpg".to_string();

        // Chosen by the sources' order, not by a person: TMDB's stays.
        let mut document = series_document();
        assert!(!write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &origin
        ));
        assert_eq!(document["poster_path"], series_document()["poster_path"]);

        // Chosen by a person: written, in any language.
        item.locked_fields = vec!["item/primaryPoster".into()];
        assert!(write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            false,
            &origin
        ));
        assert_eq!(document["poster_path"], json!("/chosen.jpg"));
        assert_eq!(
            document["backdrop_path"],
            series_document()["backdrop_path"]
        );

        // A picture TMDB has no path for is not one to name.
        let mut document = series_document();
        assert!(!write_locked(
            &mut document,
            &item,
            MediaKind::Series,
            true,
            &as_is
        ));
        assert_eq!(document["poster_path"], series_document()["poster_path"]);
    }

    fn episode(season: i32, number: i32, tmdb_id: Option<i64>) -> Episode {
        Episode {
            id: format!("ep-{season}x{number}"),
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: format!("Épisode verrouillé {season}x{number}"),
            overview: Some(format!("Résumé verrouillé {season}x{number}")),
            air_date: Some("2008-02-01".into()),
            air_date_utc: None,
            runtime: Some(48),
            finale_type: None,
            image: Some("https://image.tmdb.org/t/p/original/still.jpg".into()),
            tvdb_id: None,
            tmdb_id,
            rating: None,
            is_manual: false,
        }
    }

    /// A series with a first season and a special, its season's and its
    /// episodes' fields to lock.
    fn series_with_a_season() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Mon titre".into();
        item.seasons = vec![Season {
            id: "s1".into(),
            season_number: 1,
            title: Some("Première saison".into()),
            overview: Some("Le début.".into()),
            air_date: Some("2008-01-21".into()),
            tmdb_id: Some(3572),
            tvdb_id: None,
            is_manual: false,
            images: vec![Image {
                id: "season-poster".into(),
                season_number: Some(1),
                cover_type: CoverType::Poster,
                url: "https://image.tmdb.org/t/p/w500/season.jpg".into(),
                language: None,
                sort_order: 0,
                source: None,
                is_manual: true,
            }],
            primary_images: crate::domain::PrimaryImages {
                poster: Some("season-poster".into()),
                fanart: None,
            },
            translations: Vec::new(),
        }];
        item.episodes = vec![
            episode(1, 1, Some(62085)),
            episode(1, 2, None),
            episode(0, 1, Some(1_000_001)),
        ];
        item
    }

    /// [`series_with_a_season`], with its season's every field and its
    /// episodes' locked as they are held.
    fn locked_season() -> MediaItem {
        let mut item = series_with_a_season();
        item.locked_fields = [
            "season:1/title",
            "season:1/overview",
            "season:1/airDate",
            "season:1/primaryPoster",
            "episode:1x1/title",
            "episode:1x1/overview",
            "episode:1x1/airDate",
            "episode:1x1/runtime",
            "episode:1x1/image",
            "episode:1x2/title",
            "episode:0x1/title",
        ]
        .iter()
        .map(|key| key.to_string())
        .collect();
        item
    }

    fn season_document() -> Map<String, Value> {
        let Value::Object(map) = json!({
            "_id": "52542282760ee313280017f9",
            "id": 3572,
            "name": "Saison 1",
            "overview": "TMDB's overview",
            "air_date": "2008-01-20",
            "poster_path": "/upstream-season.jpg",
            "season_number": 1,
            "episodes": [
                { "id": 62085, "season_number": 1, "episode_number": 1, "name": "Pilot",
                  "overview": "TMDB's", "air_date": "2008-01-20", "runtime": 59,
                  "still_path": "/upstream-still.jpg" },
                { "id": 62086, "season_number": 1, "episode_number": 2, "name": "Cat's in the Bag...",
                  "overview": "TMDB's", "air_date": "2008-01-27", "runtime": 49,
                  "still_path": "/upstream-still-2.jpg" },
                { "id": 62087, "season_number": 1, "episode_number": 3, "name": "...And the Bag's in the River",
                  "overview": "TMDB's", "air_date": "2008-02-10", "runtime": 48,
                  "still_path": null }
            ]
        }) else {
            unreachable!()
        };
        map
    }

    #[test]
    fn a_season_takes_its_locks_and_its_episodes_theirs() {
        let item = locked_season();
        let mut document = season_document();

        assert!(write_season(&mut document, &item, 1, &as_is));

        assert_eq!(document["name"], json!("Première saison"));
        assert_eq!(document["overview"], json!("Le début."));
        assert_eq!(document["air_date"], json!("2008-01-21"));
        assert_eq!(document["poster_path"], json!("/season.jpg"));

        let episodes = document["episodes"].as_array().unwrap();
        assert_eq!(episodes[0]["name"], json!("Épisode verrouillé 1x1"));
        assert_eq!(episodes[0]["overview"], json!("Résumé verrouillé 1x1"));
        assert_eq!(episodes[0]["air_date"], json!("2008-02-01"));
        assert_eq!(episodes[0]["runtime"], json!(48));
        assert_eq!(episodes[0]["still_path"], json!("/still.jpg"));
        // Its title alone is locked: the rest is TMDB's.
        assert_eq!(episodes[1]["name"], json!("Épisode verrouillé 1x2"));
        assert_eq!(episodes[1]["overview"], json!("TMDB's"));
        assert_eq!(episodes[1]["runtime"], json!(49));
        // Not in the catalogue: as it came.
        assert_eq!(episodes[2]["name"], json!("...And the Bag's in the River"));
    }

    #[test]
    fn a_season_in_another_language_takes_what_was_locked_in_it() {
        // An episode's title locked in German alone, a date in every
        // language, the season's name in every language.
        let item = read_with(
            series_with_a_season(),
            &[
                ("season:1", "title", "", json!("Première saison")),
                ("episode:1x1", "title", "deu", json!("Der Anfang")),
                ("episode:1x1", "airDate", "", json!("2008-02-03")),
            ],
        );

        // A French reader: the German title is not French.
        let mut document = season_document();
        assert!(write_season(&mut document, &item, 1, &as_is));
        assert_eq!(document["name"], json!("Première saison"));
        assert_eq!(document["episodes"][0]["name"], json!("Pilot"));
        assert_eq!(document["episodes"][0]["air_date"], json!("2008-02-03"));

        // A German reader: the title locked in German, the date locked in
        // every language — and TMDB's German name of the season, the one
        // locked in every language being the catalogue's own text.
        let german = laid_for(item, "deu");
        let mut document = season_document();
        assert!(write_season(&mut document, &german, 1, &as_is));
        assert_eq!(document["name"], season_document()["name"]);
        assert_eq!(document["episodes"][0]["name"], json!("Der Anfang"));
        assert_eq!(document["episodes"][0]["air_date"], json!("2008-02-03"));
        // Nothing locked on it: TMDB's German stays.
        assert_eq!(
            document["episodes"][1]["name"],
            json!("Cat's in the Bag...")
        );
    }

    #[test]
    fn a_season_tmdb_numbers_otherwise_is_left_to_its_episodes() {
        // The catalogue's first season is another of TMDB's: its locks are
        // not this one's, though its number is.
        let item = locked_season();
        let mut document = season_document();
        document.insert("id".into(), json!(9999));
        assert!(write_season(&mut document, &item, 1, &as_is));
        assert_eq!(document["name"], json!("Saison 1"));
        assert_eq!(document["poster_path"], json!("/upstream-season.jpg"));
        // The episodes are judged each on its own.
        assert_eq!(
            document["episodes"][0]["name"],
            json!("Épisode verrouillé 1x1")
        );
    }

    #[test]
    fn an_episode_is_the_catalogues_by_its_tmdb_id_then_by_its_numbers() {
        let item = locked_season();
        assert_eq!(
            catalogue_episode(&item, 1, 1, Some(62085)).map(|e| e.id.as_str()),
            Some("ep-1x1")
        );
        // TMDB's 2×01 that the merge gave the catalogue's 1×01: wherever TMDB
        // numbers it, the episode its id was given to.
        assert_eq!(
            catalogue_episode(&item, 2, 1, Some(62085)).map(|e| e.id.as_str()),
            Some("ep-1x1")
        );
        // Known under another TMDB id: another episode.
        assert!(catalogue_episode(&item, 1, 1, Some(1)).is_none());
        // Held with no TMDB id: its numbers say it.
        assert_eq!(
            catalogue_episode(&item, 1, 2, Some(62086)).map(|e| e.id.as_str()),
            Some("ep-1x2")
        );
        // A special by its TMDB id alone, whatever TMDB numbers it.
        assert_eq!(
            catalogue_episode(&item, 0, 7, Some(1_000_001)).map(|e| e.id.as_str()),
            Some("ep-0x1")
        );
        assert!(catalogue_episode(&item, 0, 1, Some(5)).is_none());
        assert!(catalogue_episode(&item, 0, 1, None).is_none());

        // One episode's own document.
        let Value::Object(mut special) = json!({
            "id": 1_000_001, "season_number": 0, "episode_number": 7,
            "name": "TMDB's special", "overview": "", "air_date": null, "runtime": null,
            "still_path": null
        }) else {
            unreachable!()
        };
        assert!(write_episode(&mut special, &item, &as_is));
        assert_eq!(special["name"], json!("Épisode verrouillé 0x1"));
    }

    #[test]
    fn statuses_round_trip_back_to_tmdbs_vocabulary() {
        assert_eq!(tmdb_series_status("ended"), Some("Ended"));
        assert_eq!(tmdb_series_status("continuing"), Some("Returning Series"));
        assert_eq!(tmdb_series_status("upcoming"), Some("Planned"));
        assert_eq!(tmdb_series_status("released"), None);
    }

    #[test]
    fn a_chosen_image_is_named_by_the_path_tmdb_files_it_under() {
        assert_eq!(
            tmdb_image_path("https://image.tmdb.org/t/p/original/abc.jpg").as_deref(),
            Some("/abc.jpg")
        );
        assert_eq!(
            tmdb_image_path("https://image.tmdb.org/t/p/w500/x_Y-z.png").as_deref(),
            Some("/x_Y-z.png")
        );
        assert_eq!(
            tmdb_image_path("https://assets.fanart.tv/fanart/tv/1/poster.jpg"),
            None
        );
        assert_eq!(tmdb_image_path("upload:0123"), None);
        assert_eq!(
            tmdb_image_path("https://image.tmdb.org/t/p/original/"),
            None
        );
    }

    async fn refusal_of(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn the_rest_of_v4_is_refused_in_tmdbs_shape_naming_this_server() {
        let version = env!("CARGO_PKG_VERSION");

        let refused = v4_refused(None);
        assert!(refused.extensions().get::<relay::RefusedHere>().is_some());
        let (status, body) = refusal_of(refused).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["success"], json!(false));
        assert_eq!(body["status_code"], json!(3));
        let message = body["status_message"].as_str().unwrap();
        assert!(
            message.starts_with(&format!("arr-metadata-server {version} ")),
            "{message}"
        );
        assert!(message.contains("TMDB accounts"), "{message}");
        assert!(message.contains("/4/list/{id}"), "{message}");
        // TMDB's shape and nothing else: no field a client would take for
        // an account's data.
        assert_eq!(body.as_object().unwrap().len(), 3);

        let (_, body) = refusal_of(v4_refused(Some("https://ams.example"))).await;
        let message = body["status_message"].as_str().unwrap();
        assert!(
            message.starts_with(&format!(
                "arr-metadata-server {version} at https://ams.example "
            )),
            "{message}"
        );
    }

    /// [`locked_season`], with a poster of its first season a person hid.
    fn locked_season_with_a_poster_hidden() -> MediaItem {
        let mut item = locked_season();
        item.hidden.images.push(Image {
            id: "hidden-season-poster".into(),
            season_number: Some(1),
            cover_type: CoverType::Poster,
            url: "https://image.tmdb.org/t/p/original/hidden-season.jpg".into(),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        });
        item
    }

    #[test]
    fn the_seasons_asked_with_a_series_take_their_locks_as_on_their_own() {
        let item = locked_season_with_a_poster_hidden();
        let credits = json!({ "id": 3572, "cast": [ { "id": 17419, "name": "Bryan Cranston" } ] });
        let stills = json!({ "id": 62085, "stills": [ { "file_path": "/hidden-season.jpg" } ] });
        let Value::Object(mut series) = json!({
            "id": 1396,
            "name": "Breaking Bad",
            "season/1": Value::Object(season_document()),
            "season/1/images": { "id": 3572, "posters": [
                { "file_path": "/kept-season.jpg" }, { "file_path": "/hidden-season.jpg" } ] },
            "season/1/episode/1": { "id": 62085, "season_number": 1, "episode_number": 1,
                "name": "Pilot", "overview": "TMDB's", "air_date": "2008-01-20", "runtime": 59,
                "still_path": "/upstream-still.jpg" },
            "season/1/credits": credits,
            "season/1/episode/1/images": stills,
            "season/9": { "id": 9999, "season_number": 9, "name": "Saison 9", "episodes": [] },
            "credits": { "cast": [] }
        }) else {
            unreachable!()
        };

        assert!(patch_appended(&mut series, 1396, &item, &as_is));

        // A season, as `/3/tv/1396/season/1` takes its locks, its episodes
        // theirs.
        let season = &series["season/1"];
        assert_eq!(season["name"], json!("Première saison"));
        assert_eq!(season["overview"], json!("Le début."));
        assert_eq!(season["poster_path"], json!("/season.jpg"));
        assert_eq!(
            season["episodes"][0]["name"],
            json!("Épisode verrouillé 1x1")
        );
        assert_eq!(
            season["episodes"][1]["name"],
            json!("Épisode verrouillé 1x2")
        );
        assert_eq!(
            season["episodes"][2]["name"],
            json!("...And the Bag's in the River")
        );
        // Its posters, less the one hidden.
        assert_eq!(
            series["season/1/images"]["posters"],
            json!([{ "file_path": "/kept-season.jpg" }])
        );
        // An episode, as `…/season/1/episode/1` takes its locks.
        let episode = &series["season/1/episode/1"];
        assert_eq!(episode["name"], json!("Épisode verrouillé 1x1"));
        assert_eq!(episode["runtime"], json!(48));
        assert_eq!(episode["still_path"], json!("/still.jpg"));
        // What carries nothing a lock addresses stays as TMDB gave it: a
        // season's people, an episode's stills, a season not held here.
        assert_eq!(series["season/1/credits"], credits);
        assert_eq!(series["season/1/episode/1/images"], stills);
        assert_eq!(series["season/9"]["name"], json!("Saison 9"));
        // The series' own fields are not this one's to write.
        assert_eq!(series["name"], json!("Breaking Bad"));

        // Nothing asked with it: nothing to do.
        let mut bare = series_document();
        assert!(!patch_appended(&mut bare, 1396, &item, &as_is));
        assert_eq!(bare, series_document());
    }

    #[test]
    fn the_configuration_sends_the_clients_to_the_gate_for_pictures() {
        let tmdbs = json!({
            "images": {
                "base_url": "http://image.tmdb.org/t/p/",
                "secure_base_url": "https://image.tmdb.org/t/p/",
                "poster_sizes": ["w92", "w500", "original"],
                "backdrop_sizes": ["w300", "w1280", "original"]
            },
            "change_keys": ["adult", "air_date"]
        });
        let mut document = tmdbs.clone();
        assert!(pictures_from(
            &mut document,
            "https://ams.example/tmdb-images/"
        ));
        assert_eq!(
            document["images"]["secure_base_url"],
            json!("https://ams.example/tmdb-images/")
        );
        assert_eq!(
            document["images"]["base_url"],
            json!("https://ams.example/tmdb-images/")
        );
        // The sizes stay TMDB's, which the gate answers.
        assert_eq!(
            document["images"]["poster_sizes"],
            tmdbs["images"]["poster_sizes"]
        );
        assert_eq!(document["change_keys"], tmdbs["change_keys"]);

        // An answer of another shape is not given a field it lacks.
        let mut refusal = json!({ "success": false, "status_code": 7 });
        assert!(!pictures_from(
            &mut refusal,
            "https://ams.example/tmdb-images/"
        ));
        assert_eq!(refusal, json!({ "success": false, "status_code": 7 }));
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    #[test]
    fn the_window_of_changes_asked_is_tmdbs_reading_of_the_dates() {
        let now = at("2026-11-01T10:00:00Z");
        let window = |text: &str| changes::window(&uri(text), now);

        // Nothing asked: the last day.
        assert_eq!(
            window("/3/movie/changes"),
            Some((at("2026-10-31T10:00:00Z"), now))
        );
        // From the start of a day, to now or to the end of another.
        assert_eq!(
            window("/3/tv/changes?start_date=2026-10-29"),
            Some((at("2026-10-29T00:00:00Z"), now))
        );
        assert_eq!(
            window("/3/tv/changes?start_date=2026-10-20&end_date=2026-10-25&page=2"),
            Some((at("2026-10-20T00:00:00Z"), at("2026-10-26T00:00:00Z")))
        );
        // The last day before an end asked alone.
        assert_eq!(
            window("/3/tv/changes?end_date=2026-10-25"),
            Some((at("2026-10-25T00:00:00Z"), at("2026-10-26T00:00:00Z")))
        );
        // Fourteen days at the most: the start is brought forward.
        assert_eq!(
            window("/3/movie/changes?start_date=2026-09-01"),
            Some((at("2026-10-18T10:00:00Z"), now))
        );
        // An empty date is none asked; one that is not a date, or a window
        // that ends before it starts, adds nothing.
        assert_eq!(
            window("/3/movie/changes?start_date=&end_date="),
            window("/3/movie/changes")
        );
        assert_eq!(window("/3/movie/changes?start_date=yesterday"), None);
        assert_eq!(window("/3/movie/changes?end_date=2026-13-01"), None);
        assert_eq!(
            window("/3/movie/changes?start_date=2026-10-25&end_date=2026-10-20"),
            None
        );
        assert_eq!(window("/3/movie/changes?start_date=2026-11-02"), None);
    }

    #[test]
    fn the_works_changed_here_follow_tmdbs_on_its_first_page_once_each() {
        assert_eq!(changes::of("/3/movie/changes"), Some(MediaKind::Movie));
        assert_eq!(changes::of("/3/tv/changes"), Some(MediaKind::Series));
        assert_eq!(changes::of("/3/person/changes"), None);
        assert_eq!(changes::of("/3/movie/550/changes"), None);

        assert!(changes::first_page(&uri("/3/movie/changes")));
        assert!(changes::first_page(&uri("/3/movie/changes?page=1")));
        assert!(changes::first_page(&uri("/3/movie/changes?page=")));
        assert!(!changes::first_page(&uri("/3/movie/changes?page=2")));
        assert!(!changes::first_page(&uri("/3/movie/changes?page=1&page=3")));

        let mut page = json!({
            "results": [ { "id": 550, "adult": false }, { "id": 551, "adult": false } ],
            "page": 1, "total_pages": 3, "total_results": 250
        });
        // One TMDB lists already, one listed twice here, an adult one.
        assert!(changes::with_here(
            &mut page,
            &[
                (550, false),
                (990_001, false),
                (990_001, false),
                (990_002, true)
            ]
        ));
        assert_eq!(
            page["results"],
            json!([
                { "id": 550, "adult": false }, { "id": 551, "adult": false },
                { "id": 990_001, "adult": false }, { "id": 990_002, "adult": true }
            ])
        );
        assert_eq!(page["total_results"], json!(252));
        assert_eq!(page["total_pages"], json!(3));
        // Nothing new: the page as it was.
        let before = page.clone();
        assert!(!changes::with_here(&mut page, &[(551, false)]));
        assert_eq!(page, before);

        // TMDB listing nothing: those changed here, on one page.
        let mut empty = json!({ "results": [], "page": 1, "total_pages": 0, "total_results": 0 });
        assert!(changes::with_here(&mut empty, &[(990_003, false)]));
        assert_eq!(
            empty,
            json!({ "results": [ { "id": 990_003, "adult": false } ],
                    "page": 1, "total_pages": 1, "total_results": 1 })
        );
        // An answer of another shape takes nothing.
        let mut refusal = json!({ "success": false, "status_code": 7 });
        assert!(!changes::with_here(&mut refusal, &[(990_005, false)]));
    }
}

/// The relay as a client meets it, against a stand-in for TMDB that keeps
/// what it was asked.
#[cfg(test)]
mod relayed {
    use axum::{
        body::Body,
        extract::{Request, State},
        http::{HeaderMap, StatusCode, Uri, header},
        response::{IntoResponse, Response},
    };
    use serde_json::{Value, json};

    use super::proxy;
    use crate::{
        api::relay::{
            self,
            testing::{self, Asked, OPERATOR_TMDB_KEY, StandIn},
        },
        cache,
        config::{Surface, SurfacePolicy},
        domain::{Episode, MediaItem, MediaKind, Season},
        state::AppState,
    };

    /// TMDB as the stand-in answers it: every text its own, and a request
    /// that carries no key refused as TMDB refuses one.
    fn tmdb(asked: &Asked) -> (StatusCode, Value) {
        let keyed = asked
            .query
            .split('&')
            .any(|pair| pair.starts_with("api_key="))
            || asked.authorization.is_some();
        if !keyed {
            return (
                StatusCode::UNAUTHORIZED,
                json!({ "success": false, "status_code": 7, "status_message": "Invalid API key" }),
            );
        }
        let results = json!({ "page": 1, "total_pages": 1, "total_results": 2, "results": [
            { "id": 1396, "media_type": "tv", "name": "Breaking Bad", "overview": "TMDB's" },
            { "id": 17419, "media_type": "person", "name": "Bryan Cranston" }
        ]});
        let episode = json!({ "id": 62085, "season_number": 1, "episode_number": 1,
                              "name": "Pilot", "overview": "TMDB's" });
        match asked.path.as_str() {
            "/3/tv/1396" => (
                StatusCode::OK,
                json!({ "id": 1396, "name": "Breaking Bad", "overview": "TMDB's",
                        "first_air_date": "2008-01-20" }),
            ),
            "/3/tv/1396/season/1" => (
                StatusCode::OK,
                json!({ "id": 3572, "season_number": 1, "name": "Season 1", "overview": "",
                        "episodes": [episode] }),
            ),
            "/3/tv/1396/season/1/episode/1" => (StatusCode::OK, episode),
            "/3/search/tv" | "/3/trending/all/day" | "/3/discover/tv" => (StatusCode::OK, results),
            _ => (
                StatusCode::NOT_FOUND,
                json!({ "success": false, "status_code": 34 }),
            ),
        }
    }

    /// The server, with a series TMDB knows as 1396 locked here: its title
    /// in every language, its overview in German alone, its first season's
    /// name and its first episode's title in every language.
    async fn server(stand_in: &StandIn) -> AppState {
        let state = testing::server(stand_in).await;
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Breaking Bad".into();
        item.slug = "breaking-bad".into();
        item.external_ids.tmdb = Some(1396);
        item.seasons = vec![Season {
            id: "s1".into(),
            season_number: 1,
            title: Some("Season 1".into()),
            overview: None,
            air_date: None,
            tmdb_id: Some(3572),
            tvdb_id: None,
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
            translations: Vec::new(),
        }];
        item.episodes = vec![Episode {
            id: "e1".into(),
            season_number: 1,
            episode_number: 1,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: "Pilot".into(),
            overview: None,
            air_date: None,
            air_date_utc: None,
            runtime: None,
            finale_type: None,
            image: None,
            tvdb_id: None,
            tmdb_id: Some(62085),
            rating: None,
            is_manual: false,
        }];
        testing::store(
            &state,
            &item,
            &[
                ("item", "title", "", json!("Mon titre")),
                ("item", "overview", "deu", json!("Mein Überblick")),
                ("season:1", "title", "", json!("Première saison")),
                ("episode:1x1", "title", "", json!("Le pilote")),
            ],
        )
        .await;
        state
    }

    async fn get(state: &AppState, target: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
        let mut request = Request::builder().uri(target);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let answer = proxy(State(state.clone()), request.body(Body::empty()).unwrap())
            .await
            .expect("an answer");
        let status = answer.status();
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn the_operators_key_stands_in_and_every_document_takes_its_locks() {
        let stand_in = StandIn::start(tmdb).await;
        let state = server(&stand_in).await;
        assert_eq!(relay::policy(&state, Surface::Tmdb), SurfacePolicy::ApiKey);

        let (status, title) = get(&state, "/3/tv/1396?api_key=ams_issued&language=fr", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(title["name"], json!("Mon titre"));
        let asked = stand_in.asked_at("/3/tv/1396");
        assert!(
            asked[0]
                .query
                .contains(&format!("api_key={OPERATOR_TMDB_KEY}"))
        );
        assert!(!asked[0].query.contains("ams_issued"));

        // Kept, and patched again on each serve.
        let (_, again) = get(&state, "/3/tv/1396?language=fr&api_key=ams_issued", &[]).await;
        assert_eq!(again["name"], json!("Mon titre"));
        assert_eq!(stand_in.asked_at("/3/tv/1396").len(), 1);

        let (_, season) = get(&state, "/3/tv/1396/season/1?language=fr-FR", &[]).await;
        assert_eq!(season["name"], json!("Première saison"));
        assert_eq!(season["episodes"][0]["name"], json!("Le pilote"));
        let (_, episode) = get(&state, "/3/tv/1396/season/1/episode/1?language=fr", &[]).await;
        assert_eq!(episode["name"], json!("Le pilote"));
        for list in [
            "/3/search/tv?query=breaking&language=fr",
            "/3/trending/all/day?language=fr",
            "/3/discover/tv?language=fr",
        ] {
            let (status, page) = get(&state, list, &[]).await;
            assert_eq!(status, StatusCode::OK, "{list}");
            assert_eq!(page["results"][0]["name"], json!("Mon titre"), "{list}");
            assert_eq!(
                page["results"][1]["name"],
                json!("Bryan Cranston"),
                "{list}"
            );
        }

        // Asked in no language, TMDB answers in its own English: a text
        // locked in every language is the catalogue's French, and TMDB's
        // English stands.
        let (_, episode) = get(&state, "/3/tv/1396/season/1/episode/1", &[]).await;
        assert_eq!(episode["name"], json!("Pilot"));
        let (_, page) = get(&state, "/3/discover/tv", &[]).await;
        assert_eq!(page["results"][0]["name"], json!("Breaking Bad"));
    }

    #[tokio::test]
    async fn the_language_asked_takes_what_was_locked_for_it() {
        let stand_in = StandIn::start(tmdb).await;
        let state = server(&stand_in).await;

        // In German: the overview locked in German alone. The title locked
        // in every language is the catalogue's own text, written in its
        // language, French: TMDB's German one stands, as it did before a
        // lock had a language.
        let (_, german) = get(&state, "/3/tv/1396?language=de-DE", &[]).await;
        assert_eq!(german["name"], json!("Breaking Bad"));
        assert_eq!(german["overview"], json!("Mein Überblick"));
        let (_, page) = get(&state, "/3/search/tv?query=x&language=de", &[]).await;
        assert_eq!(page["results"][0]["name"], json!("Breaking Bad"));
        assert_eq!(page["results"][0]["overview"], json!("Mein Überblick"));

        // Nothing locked in Spanish: TMDB's own text, all of it.
        let (_, spanish) = get(&state, "/3/tv/1396?language=es-ES", &[]).await;
        assert_eq!(spanish["name"], json!("Breaking Bad"));
        assert_eq!(spanish["overview"], json!("TMDB's"));

        // In French, the catalogue's own: the title locked in every
        // language; the overview, locked in German alone, is TMDB's.
        let (_, french) = get(&state, "/3/tv/1396?language=fr-FR", &[]).await;
        assert_eq!(french["name"], json!("Mon titre"));
        assert_eq!(french["overview"], json!("TMDB's"));
    }

    /// TMDB's lists of *Fight Club*, as the stand-in answers them: each on
    /// its own, and appended to the film's document.
    fn fight_clubs_lists(asked: &Asked) -> (StatusCode, Value) {
        let credits = json!({ "id": 550,
            "cast": [ { "id": 819, "name": "Edward Norton" }, { "id": 287, "name": "Brad Pitt" } ],
            "crew": [ { "id": 7467, "name": "David Fincher", "job": "Director", "department": "Directing" } ] });
        let images = json!({ "id": 550,
            "posters": [ { "file_path": "/kept.jpg" }, { "file_path": "/hidden.jpg" } ],
            "backdrops": [], "logos": [] });
        let titles = json!({ "id": 550, "titles": [
            { "iso_3166_1": "MX", "title": "El club de la pelea" },
            { "iso_3166_1": "US", "title": "Fight Club – Hidden" } ] });
        match asked.path.as_str() {
            "/3/movie/550/credits" => (StatusCode::OK, credits),
            "/3/movie/550/images" => (StatusCode::OK, images),
            "/3/movie/550/alternative_titles" => (StatusCode::OK, titles),
            "/3/movie/550" => (
                StatusCode::OK,
                json!({ "id": 550, "title": "Fight Club",
                        "credits": credits, "images": images, "alternative_titles": titles }),
            ),
            _ => (StatusCode::NOT_FOUND, json!({ "status_code": 34 })),
        }
    }

    #[tokio::test]
    async fn what_a_person_hid_is_in_no_list_relayed() {
        let stand_in = StandIn::start(fight_clubs_lists).await;
        let state = testing::server(&stand_in).await;
        testing::store(
            &state,
            &super::tests::fight_club(),
            &super::tests::hidden_of_fight_club(),
        )
        .await;
        let ids = |list: &Value| -> Vec<i64> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["id"].as_i64().unwrap())
                .collect()
        };
        let paths = |list: &Value| -> Vec<String> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["file_path"].as_str().unwrap().to_string())
                .collect()
        };

        let (status, credits) = get(&state, "/3/movie/550/credits", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids(&credits["cast"]), [287]);
        assert_eq!(credits["crew"], json!([]));
        let (_, images) = get(&state, "/3/movie/550/images", &[]).await;
        assert_eq!(paths(&images["posters"]), ["/kept.jpg"]);
        let (_, titles) = get(&state, "/3/movie/550/alternative_titles", &[]).await;
        assert_eq!(titles["titles"].as_array().unwrap().len(), 1);
        assert_eq!(titles["titles"][0]["iso_3166_1"], json!("MX"));

        // Appended to the film's own document, in any language: the same.
        let (_, film) = get(
            &state,
            "/3/movie/550?append_to_response=credits,images,alternative_titles&language=de",
            &[],
        )
        .await;
        assert_eq!(ids(&film["credits"]["cast"]), [287]);
        assert_eq!(paths(&film["images"]["posters"]), ["/kept.jpg"]);
        assert_eq!(
            film["alternative_titles"]["titles"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn each_clients_own_key_travels_and_one_issued_here_is_refused() {
        let stand_in = StandIn::start(tmdb).await;
        let state = server(&stand_in).await;
        testing::choose(&state, "tmdb.relayCredential", "client").await;
        // A key is no use here any more: the address list guards the relay.
        assert_eq!(
            relay::policy(&state, Surface::Tmdb),
            SurfacePolicy::Allowlist
        );

        // A key issued here, wherever it is: refused, saying why, and TMDB
        // asked nothing.
        for (target, headers) in [
            ("/3/tv/1396?api_key=ams_issued", vec![]),
            ("/3/tv/1396", vec![("authorization", "Bearer ams_issued")]),
            ("/3/tv/1396", vec![("x-api-key", "ams_issued")]),
            // Its scheme in capitals, another scheme, or behind a token of
            // their own: found all the same, and kept here.
            ("/3/tv/1396", vec![("authorization", "BEARER ams_issued")]),
            ("/3/tv/1396", vec![("authorization", "Token ams_issued")]),
            (
                "/3/tv/1396",
                vec![("authorization", "Bearer their-token ams_issued")],
            ),
        ] {
            let (status, refusal) = get(&state, target, &headers).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{target}");
            assert_eq!(refusal["error"], json!("own_key_required"));
        }
        assert!(stand_in.asked_at("/3/tv/1396").is_empty());

        // Their own key travels as it came, the operator's never; the locks
        // are written in all the same.
        let (status, title) =
            get(&state, "/3/tv/1396?api_key=their-tmdb-key&language=fr", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(title["name"], json!("Mon titre"));
        let asked = stand_in.asked_at("/3/tv/1396");
        assert!(asked[0].query.contains("api_key=their-tmdb-key"));
        assert!(!asked[0].query.contains(OPERATOR_TMDB_KEY));

        // Their v4 token, in its header; and what it was answered is kept
        // for nobody.
        let (status, _) = get(
            &state,
            "/3/tv/1396?language=fr",
            &[("authorization", "Bearer their-token")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let asked = stand_in.asked_at("/3/tv/1396");
        assert_eq!(asked.len(), 2);
        assert_eq!(
            asked[1].authorization.as_deref(),
            Some("Bearer their-token")
        );

        // No key at all: TMDB's own refusal, handed back.
        let (status, refusal) = get(&state, "/3/tv/1396", &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(refusal["status_code"], json!(7));
    }

    /// The TMDB id of the series the appended documents are about: one no
    /// other test stores.
    const APPENDED: i64 = 990_901;

    /// TMDB as the stand-in answers a series asked with its first season,
    /// that season's posters, its first episode and the season's people.
    fn with_its_seasons(asked: &Asked) -> (StatusCode, Value) {
        let episode = json!({ "id": 990_921, "season_number": 1, "episode_number": 1,
                              "name": "Pilot D9", "overview": "TMDB's", "runtime": 58 });
        match asked.path.as_str() {
            "/3/tv/990901" => (
                StatusCode::OK,
                json!({
                    "id": APPENDED, "name": "Appended Series D9", "overview": "TMDB's",
                    "season/1": { "id": 990_911, "season_number": 1, "name": "Season 1",
                                  "overview": "TMDB's", "episodes": [episode] },
                    "season/1/images": { "id": 990_911, "posters": [
                        { "file_path": "/kept-d9.jpg" }, { "file_path": "/hidden-d9.jpg" } ] },
                    "season/1/episode/1": episode,
                    "season/1/credits": { "id": 990_911,
                        "cast": [ { "id": 17419, "name": "Bryan Cranston" } ] }
                }),
            ),
            _ => (StatusCode::NOT_FOUND, json!({ "status_code": 34 })),
        }
    }

    #[tokio::test]
    async fn the_seasons_asked_with_a_series_take_their_locks() {
        let stand_in = StandIn::start(with_its_seasons).await;
        let state = testing::server(&stand_in).await;
        let poster = |file: &str| crate::domain::Image {
            id: crate::db::new_id(),
            season_number: Some(1),
            cover_type: crate::domain::CoverType::Poster,
            url: format!("https://image.tmdb.org/t/p/original/{file}"),
            language: None,
            sort_order: 0,
            source: Some("tmdb".into()),
            is_manual: false,
        };
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Série aux saisons ajoutées D9".into();
        item.slug = "serie-aux-saisons-ajoutees-d9".into();
        item.external_ids.tmdb = Some(APPENDED);
        item.seasons = vec![Season {
            id: crate::db::new_id(),
            season_number: 1,
            title: Some("Season 1".into()),
            overview: None,
            air_date: None,
            tmdb_id: Some(990_911),
            tvdb_id: None,
            is_manual: false,
            images: vec![poster("kept-d9.jpg"), poster("hidden-d9.jpg")],
            primary_images: Default::default(),
            translations: Vec::new(),
        }];
        item.episodes = vec![Episode {
            id: crate::db::new_id(),
            season_number: 1,
            episode_number: 1,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: "Pilot D9".into(),
            overview: None,
            air_date: None,
            air_date_utc: None,
            runtime: Some(58),
            finale_type: None,
            image: None,
            tvdb_id: None,
            tmdb_id: Some(990_921),
            rating: None,
            is_manual: false,
        }];
        testing::store(
            &state,
            &item,
            &[
                ("season:1", "title", "", json!("Première saison D9")),
                ("episode:1x1", "title", "", json!("Le pilote D9")),
                ("episode:1x1", "runtime", "", json!(61)),
                (
                    "image:https://image.tmdb.org/t/p/original/hidden-d9.jpg",
                    "hidden",
                    "",
                    json!(true),
                ),
            ],
        )
        .await;

        let asked = "/3/tv/990901?append_to_response=season/1,season/1/images,\
                     season/1/episode/1,season/1/credits&language=fr-FR";
        let (status, series) = get(&state, asked, &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(series["season/1"]["name"], json!("Première saison D9"));
        assert_eq!(
            series["season/1"]["episodes"][0]["name"],
            json!("Le pilote D9")
        );
        assert_eq!(series["season/1"]["episodes"][0]["runtime"], json!(61));
        assert_eq!(series["season/1/episode/1"]["name"], json!("Le pilote D9"));
        assert_eq!(
            series["season/1/images"]["posters"],
            json!([{ "file_path": "/kept-d9.jpg" }])
        );
        // A season's people are not the work's credits.
        assert_eq!(
            series["season/1/credits"]["cast"][0]["name"],
            json!("Bryan Cranston")
        );
        // Nothing of the series itself was locked: TMDB's.
        assert_eq!(series["name"], json!("Appended Series D9"));

        // In another language, a text locked in every language is the
        // catalogue's own, and TMDB's text stands; a runtime and a hidden
        // poster hold in every language.
        let (_, german) = get(
            &state,
            "/3/tv/990901?append_to_response=season/1,season/1/images,season/1/episode/1\
             &language=de-DE",
            &[],
        )
        .await;
        assert_eq!(german["season/1"]["name"], json!("Season 1"));
        assert_eq!(german["season/1/episode/1"]["name"], json!("Pilot D9"));
        assert_eq!(german["season/1/episode/1"]["runtime"], json!(61));
        assert_eq!(
            german["season/1/images"]["posters"],
            json!([{ "file_path": "/kept-d9.jpg" }])
        );
    }

    #[tokio::test]
    async fn the_rest_of_v4_is_refused_in_tmdbs_own_shape_and_tmdb_is_not_asked() {
        let stand_in = StandIn::start(tmdb).await;
        let state = server(&stand_in).await;

        for refused in [
            "/4/account/4bc889XXXXXXXXXX/movie/watchlist",
            "/4/account/4bc889XXXXXXXXXX/lists",
            "/4/auth/request_token",
            "/4/list",
        ] {
            let (status, refusal) = get(&state, refused, &[]).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
            assert_eq!(refusal["success"], json!(false), "{refused}");
            assert_eq!(refusal["status_code"], json!(3), "{refused}");
            let message = refusal["status_message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&format!(
                    "arr-metadata-server {}",
                    env!("CARGO_PKG_VERSION")
                )),
                "{message}"
            );
            assert!(stand_in.asked_at(refused).is_empty(), "{refused}");
        }

        // A public list is no refusal of this kind: the operator's v3 key
        // cannot ask for it, and the relay says so as it did.
        let list = Request::builder()
            .uri("/4/list/8136")
            .body(Body::empty())
            .unwrap();
        assert!(matches!(
            proxy(State(state.clone()), list).await,
            Err(crate::error::AppError::ProviderNotConfigured)
        ));
    }

    /// TMDB's configuration, as the stand-in answers it.
    fn configuration(asked: &Asked) -> (StatusCode, Value) {
        match asked.path.as_str() {
            "/3/configuration" => (
                StatusCode::OK,
                json!({
                    "images": {
                        "base_url": "http://image.tmdb.org/t/p/",
                        "secure_base_url": "https://image.tmdb.org/t/p/",
                        "poster_sizes": ["w92", "w500", "original"]
                    },
                    "change_keys": ["adult"]
                }),
            ),
            "/3/configuration/countries" => (
                StatusCode::OK,
                json!([{ "iso_3166_1": "FR", "english_name": "France" }]),
            ),
            _ => (StatusCode::NOT_FOUND, json!({ "status_code": 34 })),
        }
    }

    #[tokio::test]
    async fn the_configuration_gives_this_servers_gate_when_it_was_chosen() {
        use crate::api::tmdb_images::testing::{choose, server_keeping_media};

        let stand_in = StandIn::start(configuration).await;
        let (state, dir) = server_keeping_media(&stand_in, Some("https://ams.example")).await;

        // TMDB's own, until this server is chosen.
        let (_, document) = get(&state, "/3/configuration", &[]).await;
        assert_eq!(
            document["images"]["secure_base_url"],
            json!("https://image.tmdb.org/t/p/")
        );

        choose(&state, "server").await;
        let (status, document) = get(&state, "/3/configuration", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            document["images"]["secure_base_url"],
            json!("https://ams.example/tmdb-images/")
        );
        assert_eq!(
            document["images"]["base_url"],
            json!("https://ams.example/tmdb-images/")
        );
        assert_eq!(
            document["images"]["poster_sizes"],
            json!(["w92", "w500", "original"])
        );
        // Its lists carry no picture's address.
        let (_, countries) = get(&state, "/3/configuration/countries", &[]).await;
        assert_eq!(countries[0]["iso_3166_1"], json!("FR"));

        // Chosen back: TMDB's own again, from what is kept, at once.
        choose(&state, "tmdb").await;
        let (_, document) = get(&state, "/3/configuration", &[]).await;
        assert_eq!(
            document["images"]["secure_base_url"],
            json!("https://image.tmdb.org/t/p/")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn without_a_public_address_the_configuration_stays_tmdbs() {
        use crate::api::tmdb_images::testing::{choose, server_keeping_media};

        let stand_in = StandIn::start(configuration).await;
        let (state, dir) = server_keeping_media(&stand_in, None).await;
        choose(&state, "server").await;
        let (_, document) = get(&state, "/3/configuration", &[]).await;
        assert_eq!(
            document["images"]["secure_base_url"],
            json!("https://image.tmdb.org/t/p/")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── When TMDB fails, and what is shared ────────────────────────────────

    /// TMDB as the stand-in answers it while it has trouble: an error of its
    /// own for one film, "too many requests" for two others — which shuts
    /// TMDB's door five seconds, saying no longer — and a list of genres and
    /// a page of films' changes it answers whole. Anything else: down.
    fn failing(asked: &Asked) -> (StatusCode, Value) {
        match asked.path.as_str() {
            "/3/movie/990941" => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "success": false, "status_code": 11 }),
            ),
            "/3/movie/990951" | "/3/movie/990961" => (
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "success": false, "status_code": 25 }),
            ),
            "/3/genre/movie/list" => (
                StatusCode::OK,
                json!({ "genres": [ { "id": 28, "name": "Action D3" } ] }),
            ),
            "/3/movie/changes" => (
                StatusCode::OK,
                json!({ "results": [ { "id": 550, "adult": false } ],
                        "page": 1, "total_pages": 1, "total_results": 1 }),
            ),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({ "success": false, "status_code": 9 }),
            ),
        }
    }

    /// The relay's answer, an error of this server's as it is told.
    async fn answered(state: &AppState, target: &str) -> Response {
        let request = Request::builder().uri(target).body(Body::empty()).unwrap();
        match proxy(State(state.clone()), request).await {
            Ok(answer) => answer,
            Err(refusal) => refusal.into_response(),
        }
    }

    async fn body_of(answer: Response) -> Value {
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    }

    /// What the relay kept of TMDB's answer to `target`, as it is once its
    /// time is up and it is still young enough to stand in: fetched a minute
    /// and its kind's time ago.
    async fn kept_a_while_ago(state: &AppState, target: &str, document: Value) {
        let uri: Uri = target.parse().unwrap();
        state
            .caches
            .relay
            .insert_for(
                super::relay_key(state, &uri, &HeaderMap::new()),
                cache::Relayed {
                    status: 200,
                    content_type: "application/json;charset=utf-8".into(),
                    expires_at: cache::now_secs() - 60,
                    body: serde_json::to_vec(&document).unwrap().into(),
                },
                cache::RELAY_KEEP,
            )
            .await;
    }

    #[tokio::test]
    async fn when_tmdb_fails_what_was_kept_stands_in_with_its_age_and_the_locks() {
        let stand_in = StandIn::start(failing).await;
        let state = testing::server(&stand_in).await;
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = "Série tombée en panne D2".into();
        item.slug = "serie-tombee-en-panne-d2".into();
        item.external_ids.tmdb = Some(990_931);
        testing::store(
            &state,
            &item,
            &[("item", "title", "", json!("Titre verrouillé D2"))],
        )
        .await;
        let asked = "/3/tv/990931?language=fr-FR";
        kept_a_while_ago(
            &state,
            asked,
            json!({ "id": 990_931, "name": "Kept D2", "overview": "Kept overview" }),
        )
        .await;

        // TMDB down: what was kept answers, saying how old it is, with the
        // locks written in as they are into any answer.
        let answer = answered(&state, asked).await;
        assert_eq!(answer.status(), StatusCode::OK);
        let age: u64 = answer.headers()[header::AGE]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(age >= cache::relay_ttl("/3/tv/990931").as_secs(), "{age}");
        let document = body_of(answer).await;
        assert_eq!(document["name"], json!("Titre verrouillé D2"));
        assert_eq!(document["overview"], json!("Kept overview"));
        assert_eq!(stand_in.asked_at("/3/tv/990931").len(), 1);

        // Turned off: TMDB's failure, as it came.
        testing::choose(&state, relay::STALE_IF_ERROR, "false").await;
        let answer = answered(&state, asked).await;
        assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(answer.headers().get(header::AGE).is_none());
    }

    #[tokio::test]
    async fn callers_asking_the_same_at_once_share_one_exchange() {
        let stand_in = StandIn::start(failing).await;
        let state = testing::server(&stand_in).await;
        let asked = "/3/movie/990941?language=fr-FR";

        // Eight at once, while TMDB answers an error, which nothing keeps:
        // one exchange for all of them.
        let answers = futures::future::join_all((0..8).map(|_| answered(&state, asked))).await;
        for answer in &answers {
            assert_eq!(answer.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }
        assert_eq!(stand_in.asked_at("/3/movie/990941").len(), 1);

        // Once it has ended, the next caller asks anew.
        let answer = answered(&state, asked).await;
        assert_eq!(answer.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(stand_in.asked_at("/3/movie/990941").len(), 2);
    }

    #[tokio::test]
    async fn a_door_tmdb_shut_is_a_429_that_says_when_or_what_was_kept() {
        let stand_in = StandIn::start(failing).await;
        let state = testing::server(&stand_in).await;

        // TMDB asks to be left alone: its refusal handed on, and its door
        // shut for every exchange with it, this server's own fetches' too.
        let answer = answered(&state, "/3/movie/990951").await;
        assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(state.tmdb.gate().quiet_for().is_some());

        // Nothing kept: a 429 that says when to come back; TMDB is not
        // asked, nor waited for.
        let answer = answered(&state, "/3/movie/990952").await;
        assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);
        let wait: u64 = answer.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&wait), "{wait}");
        assert!(stand_in.asked_at("/3/movie/990952").is_empty());

        // Something kept: it answers, with its age.
        kept_a_while_ago(
            &state,
            "/3/movie/990953",
            json!({ "id": 990_953, "title": "Kept D4" }),
        )
        .await;
        let answer = answered(&state, "/3/movie/990953").await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert!(answer.headers().contains_key(header::AGE));
        assert_eq!(body_of(answer).await["title"], json!("Kept D4"));
        assert!(stand_in.asked_at("/3/movie/990953").is_empty());
    }

    #[tokio::test]
    async fn a_reference_list_answers_from_the_database_when_the_memory_holds_nothing() {
        use crate::db::repo::relay_document;

        let stand_in = StandIn::start(failing).await;
        let state = testing::server(&stand_in).await;
        let asked = "/3/genre/movie/list?language=fr-FR";

        let (status, genres) = get(&state, asked, &[]).await;
        assert_eq!(status, StatusCode::OK);
        // Kept in the database too, under a key no generation is part of.
        let filed_as = super::reference_key(&state, &asked.parse().unwrap(), &HeaderMap::new());
        assert!(
            relay_document::read(&state.db, "tmdb", &filed_as, &relay_document::cutoff())
                .await
                .unwrap()
                .is_some()
        );

        // The memory's keys move on, as they do at every start; and TMDB
        // shuts its door.
        state.caches.bump_generation().await;
        let answer = answered(&state, "/3/movie/990961").await;
        assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);

        let answer = answered(&state, asked).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert!(answer.headers().contains_key(header::AGE));
        assert_eq!(body_of(answer).await, genres);
        assert_eq!(stand_in.asked_at("/3/genre/movie/list").len(), 1);
    }

    /// Note a change of a work in its journal, as a lock put on its title
    /// writes one.
    async fn changed(state: &AppState, item: &MediaItem) {
        use crate::db::repo::change::{self, Cause, NewChange};

        testing::store(state, item, &[]).await;
        change::record(
            &state.db,
            &item.id,
            None,
            &[NewChange {
                cause: Cause::Lock,
                scope: "item".into(),
                field: "title".into(),
                language: String::new(),
                before: None,
                after: Some(json!(item.title)),
                provider: None,
                was_locked: false,
            }],
        )
        .await
        .expect("the change is noted");
    }

    fn work(kind: MediaKind, tmdb_id: i64, title: &str) -> MediaItem {
        let mut item = MediaItem::empty(kind);
        item.title = title.into();
        item.slug = crate::domain::make_slug(title, None);
        item.external_ids.tmdb = Some(tmdb_id);
        item
    }

    fn sorted_ids(results: &Value) -> Vec<i64> {
        let mut ids: Vec<i64> = results
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_i64().unwrap())
            .collect();
        ids.sort_unstable();
        ids
    }

    #[tokio::test]
    async fn changes_lay_the_works_changed_here_beside_tmdbs_and_fail_as_tmdb_does() {
        let stand_in = StandIn::start(failing).await;
        let state = testing::server(&stand_in).await;
        changed(
            &state,
            &work(MediaKind::Movie, 990_971, "Film changé ici D11"),
        )
        .await;

        let (status, page) = get(&state, "/3/movie/changes", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["results"][0], json!({ "id": 550, "adult": false }));
        assert_eq!(page["results"][1], json!({ "id": 990_971, "adult": false }));
        assert_eq!(page["total_results"], json!(2));

        // A work changed since shows at once, though TMDB's page is kept.
        changed(
            &state,
            &work(MediaKind::Movie, 990_972, "Second film changé ici D11"),
        )
        .await;
        let (_, page) = get(&state, "/3/movie/changes", &[]).await;
        assert_eq!(sorted_ids(&page["results"]), [550, 990_971, 990_972]);
        assert_eq!(stand_in.asked_at("/3/movie/changes").len(), 1);
        // Another page is TMDB's alone.
        let (_, next) = get(&state, "/3/movie/changes?page=2", &[]).await;
        assert_eq!(sorted_ids(&next["results"]), [550]);

        // TMDB failing: its failure, handed on — neither what was kept of
        // the feed nor the works changed here alone.
        changed(
            &state,
            &work(MediaKind::Series, 990_973, "Série changée ici D11"),
        )
        .await;
        kept_a_while_ago(
            &state,
            "/3/tv/changes",
            json!({ "results": [ { "id": 1, "adult": false } ],
                    "page": 1, "total_pages": 1, "total_results": 1 }),
        )
        .await;
        let answer = answered(&state, "/3/tv/changes").await;
        assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(answer.headers().get(header::AGE).is_none());
        assert!(body_of(answer).await.get("results").is_none());
    }
}

#[cfg(test)]
mod local {
    use axum::{
        body::Body,
        extract::{Request, State},
        http::{StatusCode, header},
        response::Response,
    };
    use serde_json::{Value, json};

    use super::proxy;
    use crate::{
        api::relay::testing::{self, Asked, StandIn},
        db::repo,
        domain::{Credit, CreditType, Episode, MediaItem, MediaKind, Season},
        service::ids,
        state::AppState,
    };

    /// TMDB as the stand-in answers it: a film it knows by 603, where to
    /// watch the one held here, a page of search, and a series' ids
    /// without TheTVDB's.
    fn tmdb(asked: &Asked) -> (StatusCode, Value) {
        match asked.path.as_str() {
            "/3/movie/603" => (StatusCode::OK, json!({ "id": 603, "title": "The Matrix" })),
            "/3/movie/990550" => (StatusCode::OK, json!({ "id": 990_550, "title": "TMDB's" })),
            "/3/movie/990550/watch/providers" => (
                StatusCode::OK,
                json!({ "id": 990_550, "results": { "FR": { "link": "https://example.test/fr",
                        "flatrate": [ { "provider_id": 8, "provider_name": "Netflix" } ] } } }),
            ),
            "/3/search/movie" => (
                StatusCode::OK,
                json!({ "page": 1, "total_pages": 1, "total_results": 2, "results": [
                    { "id": 990_550, "title": "TMDB's copy of the one held here" },
                    { "id": 990_999, "title": "One held nowhere here" }
                ]}),
            ),
            "/3/search/tv" => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "success": false, "status_code": 11 }),
            ),
            "/3/tv/990396/external_ids" => (
                StatusCode::OK,
                json!({ "id": 990_396, "imdb_id": null, "tvdb_id": null, "tvrage_id": null }),
            ),
            "/3/tv/990396/season/1" => (
                StatusCode::OK,
                json!({ "id": 1, "season_number": 1, "name": "TMDB's season", "episodes": [] }),
            ),
            _ => (
                StatusCode::NOT_FOUND,
                json!({ "success": false, "status_code": 34 }),
            ),
        }
    }

    fn actor(name: &str, person: i64) -> Credit {
        Credit {
            id: crate::db::new_id(),
            credit_type: CreditType::Actor,
            person_name: name.into(),
            character_name: Some("The narrator".into()),
            image: None,
            tmdb_person_id: Some(person),
            credit_tmdb_id: Some(format!("credit-{person}")),
            sort_order: 0,
            is_manual: false,
            department: None,
            job: None,
        }
    }

    fn episode(season: i32, number: i32) -> Episode {
        Episode {
            id: crate::db::new_id(),
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: format!("Écrit à la main, {number}"),
            overview: None,
            air_date: Some(format!("2024-0{number}-10")),
            air_date_utc: None,
            runtime: Some(24),
            finale_type: None,
            image: None,
            tvdb_id: None,
            tmdb_id: None,
            rating: None,
            // As a source's: the catalogue writes the children typed by
            // hand through the editor's own routes.
            is_manual: false,
        }
    }

    /// A film TMDB knows as 990550, held here with a title locked, as the
    /// catalogue's own search lists it.
    fn held_film() -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "Pellicule tenue ici E3".into();
        item.slug = "pellicule-tenue-ici-e3".into();
        item.overview = Some("Le synopsis du catalogue.".into());
        item.external_ids.tmdb = Some(990_550);
        item.external_ids.imdb = Some("tt9905500".into());
        item.genres = vec!["Drame".into()];
        item.credits = vec![actor("Edward Norton", 819)];
        item
    }

    /// The whole server, its relay answering the works it holds itself,
    /// with the film held, and a series entered by hand: its number.
    async fn server(stand_in: &StandIn) -> (AppState, i64) {
        let state = testing::server(stand_in).await;
        testing::choose(&state, "tmdb.relayMode", "local-first").await;
        let film = held_film();
        testing::store(
            &state,
            &film,
            &[("item", "title", "", json!("Pellicule verrouillée E3"))],
        )
        .await;
        crate::service::listing::after_write(&state, &film.id).await;

        let mut series = MediaItem::empty(MediaKind::Series);
        series.title = "Feuilleton tapé à la main E3".into();
        series.slug = "feuilleton-tape-a-la-main-e3".into();
        series.is_manual = true;
        series.seasons = vec![Season {
            id: crate::db::new_id(),
            season_number: 1,
            title: None,
            overview: None,
            air_date: None,
            tmdb_id: None,
            tvdb_id: None,
            is_manual: false,
            images: Vec::new(),
            primary_images: Default::default(),
            translations: Vec::new(),
        }];
        series.episodes = vec![episode(1, 1), episode(1, 2)];
        testing::store(&state, &series, &[]).await;
        crate::service::listing::after_write(&state, &series.id).await;
        (state, ids::own(&series.id))
    }

    async fn answered(state: &AppState, target: &str) -> Response {
        let request = Request::builder().uri(target).body(Body::empty()).unwrap();
        proxy(State(state.clone()), request)
            .await
            .expect("an answer")
    }

    async fn get(state: &AppState, target: &str) -> (StatusCode, Value) {
        let answer = answered(state, target).await;
        let status = answer.status();
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[test]
    fn the_paths_answered_here_are_recognised_exactly() {
        use super::here::{Asked, recognise};

        let part = |text: &str| Some(text.to_string());
        assert_eq!(
            recognise("/3/movie/550"),
            Some(Asked::Title {
                kind: MediaKind::Movie,
                id: 550,
                part: None
            })
        );
        assert_eq!(
            recognise("/3/tv/1396/watch/providers"),
            Some(Asked::Title {
                kind: MediaKind::Series,
                id: 1396,
                part: part("watch/providers")
            })
        );
        assert_eq!(
            recognise("/3/tv/1396/season/0/episode/2/images"),
            Some(Asked::Episode {
                id: 1396,
                season: 0,
                episode: 2,
                part: part("images")
            })
        );
        assert_eq!(
            recognise("/3/tv/1396/season/1/"),
            Some(Asked::Season {
                id: 1396,
                season: 1,
                part: None
            })
        );
        assert_eq!(
            recognise("/3/find/tt0137523"),
            Some(Asked::Find {
                external_id: "tt0137523".into()
            })
        );
        assert_eq!(
            recognise("/3/search/multi"),
            Some(Asked::Search { kind: None })
        );
        // TMDB's own lists, a person, a collection, an id that is none.
        for path in [
            "/3/movie/popular",
            "/3/tv/changes",
            "/3/movie/-5",
            "/3/movie/0",
            "/3/person/819",
            "/3/collection/10",
            "/3/tv/1396/season/one",
            "/3/tv/1396//credits",
            "/3/search/person",
            "/3/configuration",
        ] {
            assert_eq!(recognise(path), None, "{path}");
        }
    }

    #[tokio::test]
    async fn a_film_held_here_is_answered_here_and_tmdb_is_asked_only_for_what_it_holds() {
        let stand_in = StandIn::start(tmdb).await;
        let (state, _) = server(&stand_in).await;
        // A key issued here, which the relay would refuse to pass on to TMDB
        // under each client's own key: this server needs no key of TMDB's.
        testing::choose(&state, "tmdb.relayCredential", "client").await;

        let answer = answered(
            &state,
            "/3/movie/990550?language=fr-FR&append_to_response=credits,watch/providers,reviews&api_key=ams_issued",
        )
        .await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(
            answer.headers()[header::CONTENT_TYPE],
            "application/json;charset=utf-8"
        );
        assert!(answer.headers().get(header::AGE).is_none());
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        let film: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(film["id"], json!(990_550));
        assert_eq!(film["title"], json!("Pellicule verrouillée E3"));
        assert_eq!(film["overview"], json!("Le synopsis du catalogue."));
        assert_eq!(film["imdb_id"], json!("tt9905500"));
        assert_eq!(film["genres"], json!([{ "id": 18, "name": "Drame" }]));
        assert_eq!(film["credits"]["cast"][0]["id"], json!(819));
        assert!(film["credits"].get("id").is_none());
        // Where to watch is TMDB's: refused under the client's own key a
        // key issued here is, and so empty, as TMDB writes none.
        assert_eq!(film["watch/providers"], json!({ "results": {} }));
        assert_eq!(film["reviews"]["results"], json!([]));
        assert!(stand_in.asked_at("/3/movie/990550").is_empty());

        // Under the operator's key, TMDB is asked for it, and it is added.
        testing::choose(&state, "tmdb.relayCredential", "server").await;
        let (_, film) = get(
            &state,
            "/3/movie/990550?language=fr-FR&append_to_response=watch/providers",
        )
        .await;
        assert_eq!(
            film["watch/providers"]["results"]["FR"]["flatrate"][0]["provider_name"],
            json!("Netflix")
        );
        let asked = stand_in.asked_at("/3/movie/990550/watch/providers");
        assert_eq!(asked.len(), 1);
        assert!(!asked[0].query.contains("append_to_response"), "{asked:?}");
        assert!(stand_in.asked_at("/3/movie/990550").is_empty());

        // What hangs off it, asked on its own, carries its id.
        let (status, credits) = get(&state, "/3/movie/990550/credits").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(credits["id"], json!(990_550));

        // In relay mode, TMDB's own document, with the lock.
        testing::choose(&state, "tmdb.relayMode", "relay").await;
        let (_, film) = get(&state, "/3/movie/990550?language=fr-FR").await;
        assert_eq!(film["title"], json!("Pellicule verrouillée E3"));
        assert_eq!(stand_in.asked_at("/3/movie/990550").len(), 1);
    }

    #[tokio::test]
    async fn what_is_not_held_here_or_not_shown_goes_on_to_tmdb() {
        let stand_in = StandIn::start(tmdb).await;
        let (state, _) = server(&stand_in).await;
        let (status, film) = get(&state, "/3/movie/603").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(film["title"], json!("The Matrix"));
        assert_eq!(stand_in.asked_at("/3/movie/603").len(), 1);

        // Switched off, or for adults where this server hides them: TMDB's.
        let id = repo::item::find_id_by_external(
            &state.db,
            crate::domain::ExternalSource::TmdbMovie,
            "990550",
        )
        .await
        .unwrap()
        .unwrap();
        let mut film = crate::service::load(&state, &id).await.unwrap().unwrap();
        film.is_adult = true;
        testing::store(&state, &film, &[]).await;
        state.caches.touched(&film.id).await;
        let (_, answer) = get(&state, "/3/movie/990550").await;
        assert_eq!(answer["title"], json!("TMDB's"));
        assert_eq!(stand_in.asked_at("/3/movie/990550").len(), 1);
    }

    #[tokio::test]
    async fn a_series_entered_by_hand_is_answered_with_its_seasons_and_episodes() {
        let stand_in = StandIn::start(tmdb).await;
        let (state, handed) = server(&stand_in).await;

        let (status, show) = get(
            &state,
            &format!(
                "/3/tv/{handed}?language=fr-FR&append_to_response=external_ids,season/1,season/1/episode/2,watch/providers"
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{show}");
        assert_eq!(show["id"], json!(handed));
        assert_eq!(show["name"], json!("Feuilleton tapé à la main E3"));
        assert_eq!(show["external_ids"]["tvdb_id"], json!(handed));
        assert_eq!(show["number_of_episodes"], json!(2));
        assert_eq!(show["seasons"][0]["episode_count"], json!(2));
        assert_eq!(
            show["season/1"]["episodes"][1]["name"],
            json!("Écrit à la main, 2")
        );
        assert!(show["season/1"].get("id").is_none());
        assert_eq!(show["season/1/episode/2"]["episode_number"], json!(2));
        // TMDB does not know it: nothing of TMDB's to add, and TMDB not asked.
        assert_eq!(show["watch/providers"], json!({ "results": {} }));

        let (status, season) = get(&state, &format!("/3/tv/{handed}/season/1")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(season["episodes"].as_array().map(Vec::len), Some(2));
        let (status, episode) = get(
            &state,
            &format!(
                "/3/tv/{handed}/season/1/episode/1?language=fr-FR&append_to_response=external_ids"
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(episode["name"], json!("Écrit à la main, 1"));
        // English — TMDB's language when none is asked — which nothing is
        // held in: as TMDB names an episode then.
        let (_, episode) = get(&state, &format!("/3/tv/{handed}/season/1/episode/1")).await;
        assert_eq!(episode["name"], json!("Episode 1"));
        assert_eq!(episode["overview"], json!(""));
        // A season it has not: TMDB's "not found", from here.
        let (status, missing) = get(&state, &format!("/3/tv/{handed}/season/2")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(missing["status_code"], json!(34));
        assert!(stand_in.asked_at(&format!("/3/tv/{handed}")).is_empty());
    }

    #[tokio::test]
    async fn the_seasons_of_a_series_numbered_otherwise_than_tmdb_are_tmdbs() {
        let stand_in = StandIn::start(tmdb).await;
        let (state, _) = server(&stand_in).await;
        // Held, TheTVDB numbering it: TMDB's seasons are TMDB's to answer.
        let mut series = MediaItem::empty(MediaKind::Series);
        series.title = "Série numérotée par TheTVDB E3".into();
        series.slug = "serie-numerotee-par-thetvdb-e3".into();
        series.external_ids.tmdb = Some(990_396);
        series.external_ids.tvdb = Some(990_123);
        series.episodes = vec![episode(1, 1)];
        testing::store(&state, &series, &[]).await;
        let (_, season) = get(&state, "/3/tv/990396/season/1").await;
        assert_eq!(season["name"], json!("TMDB's season"));
        assert_eq!(stand_in.asked_at("/3/tv/990396/season/1").len(), 1);
    }

    #[tokio::test]
    async fn find_answers_with_a_work_held_here_by_another_sites_id() {
        let stand_in = StandIn::start(tmdb).await;
        let (state, handed) = server(&stand_in).await;
        let (status, found) = get(&state, "/3/find/tt9905500?external_source=imdb_id").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(found["movie_results"][0]["id"], json!(990_550));
        assert_eq!(found["movie_results"][0]["media_type"], json!("movie"));
        assert_eq!(found["tv_results"], json!([]));
        assert!(stand_in.asked_at("/3/find/tt9905500").is_empty());
        // By the number Sonarr keeps a series entered by hand under.
        let (_, found) = get(&state, &format!("/3/find/{handed}?external_source=tvdb_id")).await;
        assert_eq!(found["tv_results"][0]["id"], json!(handed));
        // An id nothing here is held under: TMDB's.
        let (status, _) = get(&state, "/3/find/tt0000001?external_source=imdb_id").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(stand_in.asked_at("/3/find/tt0000001").len(), 1);
    }

    #[tokio::test]
    async fn a_search_lists_the_works_held_here_first_and_tmdbs_after() {
        let stand_in = StandIn::start(tmdb).await;
        let (state, handed) = server(&stand_in).await;
        let (status, page) = get(&state, "/3/search/movie?query=pellicule&language=fr-FR").await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<i64> = page["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_i64().unwrap())
            .collect();
        // Held here first, as the catalogue writes it; TMDB's copy of it
        // not twice; TMDB's other one after.
        assert_eq!(ids, [990_550, 990_999]);
        assert_eq!(
            page["results"][0]["title"],
            json!("Pellicule verrouillée E3")
        );
        assert_eq!(page["total_results"], json!(2));
        // Another page is TMDB's own.
        let (_, next) = get(&state, "/3/search/movie?query=pellicule&page=2").await;
        assert_eq!(next["results"].as_array().map(Vec::len), Some(2));

        // TMDB failing, nothing kept: the catalogue's alone.
        let (status, page) = get(&state, "/3/search/tv?query=feuilleton").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["results"][0]["id"], json!(handed));
        assert_eq!(page["total_results"], json!(1));
        // A term the catalogue finds nothing by: TMDB's answer, its failure.
        let (status, _) = get(&state, "/3/search/tv?query=introuvable").await;
        assert_ne!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_picture_uploaded_here_is_named_only_where_the_gate_serves_it() {
        use crate::{
            db::repo::asset::Thumb,
            domain::{CoverType, Image},
        };

        let stand_in = StandIn::start(tmdb).await;
        let (state, dir) = crate::api::tmdb_images::testing::server_keeping_media(
            &stand_in,
            Some("https://ams.example"),
        )
        .await;
        testing::choose(&state, "tmdb.relayMode", "local-first").await;
        let bytes = vec![0xff, 0xd8, 0xff, 0xe0, 17, 0x4a, 0x46, 0x49, 0x46];
        let (key, _) = crate::media::file::key_for(&bytes, "jpg");
        state
            .media
            .remember("upload:e3-poster", &key, Thumb::None, "image/jpeg");
        let picture = |id: &str, url: String, manual: bool| Image {
            id: id.into(),
            season_number: None,
            cover_type: CoverType::Poster,
            url,
            language: None,
            sort_order: 0,
            source: (!manual).then(|| "tmdb".to_string()),
            is_manual: manual,
        };
        let mut film = held_film();
        film.images = vec![picture(
            "tmdb-poster",
            "https://image.tmdb.org/t/p/original/tmdbsPosterE3.jpg".into(),
            false,
        )];
        testing::store(&state, &film, &[]).await;
        // An upload, added by hand: the catalogue files it by the name the
        // store keeps it under, as the editor's route does, and the person
        // chose it.
        let upload = picture("uploaded", "upload:e3-poster".into(), true);
        repo::child::add_image(&state.db, &film.id, &upload)
            .await
            .expect("the upload is added");
        crate::db::repo::override_field::set_in(
            &state.db,
            &film.id,
            "item",
            "primaryPoster",
            "",
            Some(&json!(upload.url)),
            None,
        )
        .await
        .expect("the poster is chosen");
        state.caches.touched(&film.id).await;

        // TMDB's address for pictures: the upload has none, TMDB's poster is
        // the work's.
        let (_, document) = get(&state, "/3/movie/990550?language=fr-FR").await;
        assert_eq!(document["poster_path"], json!("/tmdbsPosterE3.jpg"));
        // This server's gate: the upload, by its key, which the gate serves.
        testing::choose(&state, crate::api::tmdb_images::SETTING, "server").await;
        let (_, document) = get(
            &state,
            "/3/movie/990550?language=fr-FR&append_to_response=images&include_image_language=fr,null",
        )
        .await;
        assert_eq!(document["poster_path"], json!(format!("/{key}")));
        let posters = &document["images"]["posters"];
        assert_eq!(
            posters[0]["file_path"],
            json!(format!("/{key}")),
            "{posters}"
        );
        assert_eq!(
            posters[1]["file_path"],
            json!("/tmdbsPosterE3.jpg"),
            "{posters}"
        );
        // As TMDB lists them: in the language asked alone, where it asked
        // for no other — and these carry no text.
        let (_, document) = get(
            &state,
            "/3/movie/990550?language=fr-FR&append_to_response=images",
        )
        .await;
        assert_eq!(document["images"]["posters"], json!([]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_series_thetvdb_numbers_is_answered_in_tmdbs_numbering_kept_here() {
        use crate::domain::orders::PlacedEpisode;

        let stand_in = StandIn::start(tmdb).await;
        let (state, _) = server(&stand_in).await;
        // TheTVDB's two seasons, which TMDB numbers as one.
        let mut series = MediaItem::empty(MediaKind::Series);
        series.title = "Série en deux saisons chez TheTVDB E2".into();
        series.slug = "serie-en-deux-saisons-chez-thetvdb-e2".into();
        series.external_ids.tmdb = Some(990_396);
        series.external_ids.tvdb = Some(990_777);
        series.episodes = [(1, 1, 1001), (1, 2, 1002), (2, 1, 1003)]
            .into_iter()
            .map(|(season, number, tvdb)| {
                let mut held = episode(season, number);
                held.tvdb_id = Some(tvdb);
                held.tmdb_id = Some(tvdb + 4000);
                held
            })
            .collect();
        testing::store(&state, &series, &[]).await;
        let places: Vec<PlacedEpisode> = [(1, 1001), (2, 1002), (3, 1003)]
            .into_iter()
            .map(|(number, tvdb)| {
                let mut place = PlacedEpisode::of_tvdb(tvdb, 1, number);
                place.tmdb_id = Some(tvdb + 4000);
                place
            })
            .collect();
        repo::order::write_tmdb_aired(&state.db, &series.id, &places)
            .await
            .expect("TMDB's numbering is kept");

        let (status, season) = get(&state, "/3/tv/990396/season/1?language=fr-FR").await;
        assert_eq!(status, StatusCode::OK, "{season}");
        let numbered: Vec<(i64, i64, String)> = season["episodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["episode_number"].as_i64().unwrap(),
                    e["id"].as_i64().unwrap(),
                    e["name"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            numbered,
            [
                (1, 5001, "Écrit à la main, 1".into()),
                (2, 5002, "Écrit à la main, 2".into()),
                (3, 5003, "Écrit à la main, 1".into()),
            ]
        );
        let (status, third) = get(&state, "/3/tv/990396/season/1/episode/3?language=fr-FR").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (third["season_number"].clone(), third["id"].clone()),
            (json!(1), json!(5003))
        );
        let (_, show) = get(
            &state,
            "/3/tv/990396?language=fr-FR&append_to_response=season/1",
        )
        .await;
        assert_eq!(show["number_of_episodes"], json!(3));
        assert_eq!(show["number_of_seasons"], json!(1));
        assert_eq!(
            show["season/1"]["episodes"].as_array().map(Vec::len),
            Some(3)
        );
        assert!(stand_in.asked_at("/3/tv/990396/season/1").is_empty());
        assert!(stand_in.asked_at("/3/tv/990396").is_empty());
        // A season TMDB's numbering has not is TMDB's to answer.
        let _ = get(&state, "/3/tv/990396/season/2").await;
        assert_eq!(stand_in.asked_at("/3/tv/990396/season/2").len(), 1);
    }

    #[tokio::test]
    async fn a_series_held_here_is_given_the_tvdb_id_tmdb_has_none_for() {
        let stand_in = StandIn::start(tmdb).await;
        let (state, _) = server(&stand_in).await;
        testing::choose(&state, "tmdb.relayMode", "relay").await;
        let mut series = MediaItem::empty(MediaKind::Series);
        series.title = "Série de TMDB seul E5".into();
        series.slug = "serie-de-tmdb-seul-e5".into();
        series.external_ids.tmdb = Some(990_396);
        testing::store(&state, &series, &[]).await;
        let (status, ids) = get(&state, "/3/tv/990396/external_ids").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids["tvdb_id"], json!(ids::to_synthetic(990_396)));
    }
}
