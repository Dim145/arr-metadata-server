//! HTTP surfaces.
//!
//! Three compatibility surfaces, the metadata provider Plex asks, and this
//! server's own API. They are separate routers because they have separate
//! authentication policies — see
//! [`crate::config::Surface`] — and they are documented together, because an
//! operator wants one page describing everything the server answers.

pub mod anilist;
pub mod audit;
#[cfg(test)]
mod by_hand_tests;
pub mod cursor;
pub mod extract;
pub mod merged;
pub mod native;
pub mod plex;
pub mod radarr;
pub mod relay;
pub mod sonarr;
pub mod sonarr_services;
pub mod thetvdb;
pub mod tmdb;

use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    middleware::from_fn_with_state,
    response::{IntoResponse, Response},
};
use utoipa::{
    Modify, OpenApi,
    openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme},
};
use utoipa_axum::router::OpenApiRouter;

use crate::{auth::middleware as guards, state::AppState};

/// Spec metadata. Paths and schemas are collected from the handlers themselves,
/// so this cannot drift from what the server actually serves.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "arr-metadata-server",
        description = "\
A self-hosted metadata server for the *arr stack.

It answers three client protocols from one store — Sonarr's Skyhook API, \
Radarr's metadata API, and the TMDB v3 API — alongside its own API, documented \
here in full, and it is a metadata provider Plex Media Server can be given, \
under `/plex`. TheTVDB's v4 API and AniList's are relayed to the real \
services, with the fields locked here written into their answers.

**Locks.** Any field a person edits is recorded separately from provider data \
and is never overwritten by a refresh. That is what the `/items/{id}/overrides` \
routes manage, and what `lockedFields` reports.

**Authentication differs per surface.** Sonarr and Radarr have their metadata \
URLs compiled in and cannot attach a credential, and Plex sends none to a \
provider, so those routes are guarded by address rather than by key. The \
routes documented without a security requirement are those; they are not open.",
        version = env!("CARGO_PKG_VERSION"),
        license(name = "MIT"),
    ),
    // The relays are registered with an axum wildcard, which `routes!` cannot
    // collect; name them here so they still reach the spec.
    paths(tmdb::proxy, tmdb::proxy_v4, thetvdb::relay, anilist::relay),
    modifiers(&SecurityAddon),
    security(("apiKey" = []), ("apiKeyQuery" = []), ("session" = [])),
    tags(
        (name = "Catalogue", description = "Browse, create and refresh works"),
        (name = "Locks", description = "Manual edits, and the locks they create"),
        (name = "Clients", description = "API keys"),
        (name = "Session", description = "Administrator sign-in"),
        (name = "Audit", description = "Who changed what"),
        (name = "Server", description = "Configuration, statistics, cache"),
        (name = "Sonarr compatibility", description = "Skyhook-shaped responses"),
        (name = "Radarr compatibility", description = "api.radarr.video-shaped responses"),
        (name = "TMDB compatibility", description = "TMDB v3, relayed and patched"),
        (name = "TheTVDB compatibility", description = "TheTVDB v4, relayed and patched, in api4.thetvdb.com's place"),
        (name = "AniList compatibility", description = "AniList's GraphQL, relayed and patched, in graphql.anilist.co's place"),
        (name = "Plex provider", description = "A custom metadata provider for Plex Media Server, given to Plex by its address"),
    ),
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let Some(components) = openapi.components.as_mut() else {
            return;
        };

        components.add_security_scheme(
            "apiKey",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-Api-Key",
                "A key issued under Clients.",
            ))),
        );

        // What a TMDB client already sends: point one here and set its "TMDB
        // API key" to a key issued by this server.
        components.add_security_scheme(
            "apiKeyQuery",
            SecurityScheme::ApiKey(ApiKey::Query(ApiKeyValue::with_description(
                "api_key",
                "The same key, for clients that can only put it in the query string.",
            ))),
        );

        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("The same key, as a bearer token."))
                    .build(),
            ),
        );

        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "ams_session",
                "Set by /api/v1/auth/login. What the web UI uses.",
            ))),
        );
    }
}

/// Build every surface, with its own guard, plus the combined spec.
pub fn build(state: AppState) -> (Router<AppState>, utoipa::openapi::OpenApi) {
    // Each surface is split separately so its guard wraps only its own routes,
    // then the specs are merged back into one document.
    let (native, native_api) = OpenApiRouter::new()
        .nest("/api/v1", native::router())
        .split_for_parts();

    let (public, public_api) = OpenApiRouter::new()
        .nest("/api/v1", native::public_router())
        .split_for_parts();

    let (arr, arr_api) = arr_routers().split_for_parts();

    let (tmdb, tmdb_api) = tmdb::router().split_for_parts();

    let (thetvdb, thetvdb_api) = thetvdb::router().split_for_parts();

    let (plex, plex_api) = plex::router().split_for_parts();

    let mut api = ApiDoc::openapi();
    for part in [
        native_api,
        public_api,
        arr_api,
        tmdb_api,
        thetvdb_api,
        plex_api,
    ] {
        api.merge(part);
    }

    let router = Router::new()
        .merge(
            native
                // Inside the guard: only a caller let in learns a work was merged.
                .layer(from_fn_with_state(state.clone(), merged::refuse_writes))
                .layer(from_fn_with_state(state.clone(), guards::guard_native))
                .layer(from_fn_with_state(
                    state.clone(),
                    crate::auth::ratelimit::limit,
                )),
        )
        // Sign-in is rate limited but not guarded: putting it behind the guard
        // would mean needing a credential in order to obtain one.
        .merge(public.layer(from_fn_with_state(
            state.clone(),
            crate::auth::ratelimit::limit,
        )))
        .merge(arr_surface(state.clone(), arr))
        .merge(tmdb_surface(state.clone(), tmdb))
        .merge(thetvdb_surface(state.clone(), thetvdb))
        // On this door only: Plex is given an address, and has no name
        // compiled in for the clients' door to answer under.
        .merge(plex_surface(state, plex));

    (router, api)
}

/// The surfaces the clients with compiled-in addresses call, for their own
/// door: the same handlers under the same guards, and nothing else.
pub fn build_clients(state: AppState) -> Router<AppState> {
    let (arr, _) = arr_routers().split_for_parts();
    let (tmdb, _) = tmdb::router().split_for_parts();
    let (thetvdb, _) = thetvdb::router().split_for_parts();
    Router::new()
        .merge(arr_surface(state.clone(), arr))
        .merge(tmdb_surface(state.clone(), tmdb))
        .merge(thetvdb_surface(state, thetvdb))
}

/// What the clients' door answers for a path none of its routes has, by the
/// name the request was addressed to: whatever else Sonarr asks of
/// services.sonarr.tv, relayed under the guards of Sonarr's surface; a query
/// to graphql.anilist.co, relayed under the AniList relay's own. Nothing,
/// for any other name.
pub struct HostRelays {
    sonarr_services: Router,
    anilist: Router,
}

impl HostRelays {
    pub fn new(state: AppState) -> Self {
        let (anilist, _) = anilist::router().split_for_parts();
        Self {
            sonarr_services: arr_surface(
                state.clone(),
                Router::new().fallback(sonarr_services::fallback),
            )
            .with_state(state.clone()),
            anilist: anilist_surface(state.clone(), anilist).with_state(state),
        }
    }

    pub async fn answer(&self, mut request: Request) -> Response {
        use tower::ServiceExt as _;
        let relay = if sonarr_services::is_services_host(request.headers(), request.uri()) {
            request.extensions_mut().insert(sonarr_services::Relayed);
            &self.sonarr_services
        } else if anilist::is_anilist_host(request.headers(), request.uri()) {
            &self.anilist
        } else {
            return StatusCode::NOT_FOUND.into_response();
        };
        match relay.clone().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        }
    }
}

/// Sonarr's and Radarr's routes: Skyhook, services.sonarr.tv's scene-mapping
/// list, and api.radarr.video.
fn arr_routers() -> OpenApiRouter<AppState> {
    sonarr::router()
        .merge(sonarr_services::router())
        .merge(radarr::router())
}

fn arr_surface(state: AppState, arr: Router<AppState>) -> Router<AppState> {
    arr.layer(from_fn_with_state(state.clone(), guards::guard_arr))
        // Outermost on this surface: a loop has to be caught before any
        // work is done, and before the allowlist rejects our own address.
        .layer(from_fn_with_state(state.clone(), guards::reject_self_calls))
        // These two surfaces went without one for a while, on the
        // reasoning that they answer Sonarr and Radarr rather than the
        // open web. But the allowlist is what decides that, and a host
        // that is not on it can still spend this server's time being
        // told so — and the relay spends the operator's TMDB quota.
        .layer(from_fn_with_state(state, crate::auth::ratelimit::limit))
}

/// The TMDB relay's surface. Its name is one this server calls as a
/// provider too, and the relay asks it in turn, so a request that came round
/// a loop is caught first — before the allowlist could take our own address
/// for a client's and ask again.
fn tmdb_surface(state: AppState, tmdb: Router<AppState>) -> Router<AppState> {
    tmdb.layer(from_fn_with_state(state.clone(), guards::guard_tmdb))
        .layer(from_fn_with_state(state.clone(), guards::reject_self_calls))
        .layer(from_fn_with_state(state, crate::auth::ratelimit::limit))
}

/// The TheTVDB relay's surface. Its name is one this server calls as a
/// provider too, so a request that came round a loop is caught first.
fn thetvdb_surface(state: AppState, thetvdb: Router<AppState>) -> Router<AppState> {
    thetvdb
        .layer(from_fn_with_state(state.clone(), guards::guard_tvdb))
        .layer(from_fn_with_state(state.clone(), guards::reject_self_calls))
        .layer(from_fn_with_state(state, crate::auth::ratelimit::limit))
}

/// The AniList relay's surface, reached by its name alone.
fn anilist_surface(state: AppState, anilist: Router<AppState>) -> Router<AppState> {
    anilist
        .layer(from_fn_with_state(state.clone(), guards::guard_anilist))
        .layer(from_fn_with_state(state.clone(), guards::reject_self_calls))
        .layer(from_fn_with_state(state, crate::auth::ratelimit::limit))
}

/// The provider Plex asks: guarded by address, since Plex presents no
/// credential to a provider yet — or, asked for a key, by the one its
/// address carries (`/plex/<key>`) — and limited like the others: a host not
/// on the list can still spend this server's time being told so.
fn plex_surface(state: AppState, plex: Router<AppState>) -> Router<AppState> {
    plex.merge(plex::keyed_router())
        .layer(from_fn_with_state(state.clone(), guards::guard_plex))
        .layer(from_fn_with_state(state, crate::auth::ratelimit::limit))
}

/// Assemble the spec alone, without the state a running server needs.
///
/// [`build`] cannot be called from a test — it wants an `AppState`, and that
/// wants a database. This walks the same routers for their documentation, which
/// is where a recursive schema shows up.
#[cfg(test)]
fn openapi_only() -> utoipa::openapi::OpenApi {
    let (_, native_api) = OpenApiRouter::<AppState>::new()
        .nest("/api/v1", native::router())
        .split_for_parts();

    let (_, public_api) = OpenApiRouter::<AppState>::new()
        .nest("/api/v1", native::public_router())
        .split_for_parts();

    let (_, arr_api) = arr_routers().split_for_parts();
    let (_, tmdb_api) = tmdb::router().split_for_parts();
    let (_, thetvdb_api) = thetvdb::router().split_for_parts();
    let (_, plex_api) = plex::router().split_for_parts();

    let mut api = ApiDoc::openapi();
    for part in [
        native_api,
        public_api,
        arr_api,
        tmdb_api,
        thetvdb_api,
        plex_api,
    ] {
        api.merge(part);
    }
    api
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collecting the spec walks every schema a handler references. A cycle
    /// between two of them — a movie belongs to a collection, a collection holds
    /// movies — recurses until the stack runs out, and it does so at startup,
    /// not at compile time. This test is the thing that catches that.
    #[test]
    fn the_whole_spec_assembles_without_recursing_forever() {
        let api = openapi_only();
        let json = serde_json::to_value(&api).expect("the spec must serialize");

        assert_eq!(json["openapi"].as_str().unwrap_or_default(), "3.1.0");
        assert!(
            json["components"]["schemas"].as_object().unwrap().len() > 30,
            "schemas should have been collected from the handlers"
        );
    }

    #[test]
    fn every_surface_appears_in_the_spec() {
        let api = openapi_only();
        let json = serde_json::to_value(&api).unwrap();
        let paths = json["paths"].as_object().expect("paths");

        for expected in [
            "/api/v1/items",
            "/api/v1/items/{id}/overrides",
            "/api/v1/clients",
            "/api/v1/auth/login",
            "/api/v1/audit",
            "/api/v1/settings",
            "/api/v1/calendar.ics",
            "/api/v1/items/{id}/calendar.ics",
            "/api/v1/feed/added.atom",
            "/api/v1/feed/airing.atom",
            "/api/v1/lists",
            "/api/v1/items/{id}/watch",
            "/api/v1/items/{id}/similar",
            "/api/v1/items/{id}/orders",
            "/api/v1/collections/{tmdbId}",
            "/api/v1/figures",
            "/api/v1/admin/health",
            "/api/v1/admin/metrics",
            "/api/v1/admin/locks",
            "/api/v1/jobs/{id}",
            "/api/v1/lists/{key}/sonarr.json",
            "/api/v1/lists/{key}/ids.txt",
            "/api/v1/lists/{key}/feed.atom",
            "/api/v1/export/kometa.yml",
            "/api/v1/export/nfo",
            "/v1/tvdb/shows/{language}/{tvdb_id}",
            "/v2/tvdb/shows/{language}/{tvdb_id}/{season_type}",
            "/v2/tvdb/search",
            "/v1/movie/{tmdb_id}",
            "/v1/list/imdb/{id}",
            "/3/{path}",
            "/4/{path}",
            "/v4/login",
            "/v4/{path}",
            "/plex",
            "/plex/library/metadata/matches",
            "/plex/library/metadata/{rating_key}",
            "/plex/library/metadata/{rating_key}/children",
            "/plex/library/metadata/{rating_key}/grandchildren",
            "/plex/library/metadata/{rating_key}/images",
            "/",
        ] {
            assert!(paths.contains_key(expected), "{expected} is not documented");
        }
    }

    /// Every operation has an id, and no two share one: the interface's
    /// generated types list the operations by id, and two of one name are a
    /// type that does not compile — which is why those types were once a
    /// `.d.ts` the compiler passed over.
    #[test]
    fn every_operation_has_an_id_of_its_own() {
        // A path item may hold its parameters beside its operations.
        const METHODS: [&str; 8] = [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ];

        let json = serde_json::to_value(openapi_only()).unwrap();
        let mut seen = std::collections::BTreeMap::new();
        let mut shared = Vec::new();

        for (path, item) in json["paths"].as_object().expect("paths") {
            for (method, operation) in item.as_object().expect("a path item") {
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                let id = operation["operationId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{method} {path} has no operationId"));
                let at = format!("{} {path}", method.to_uppercase());
                if let Some(first) = seen.insert(id, at.clone()) {
                    shared.push(format!("{id}: {first} and {at}"));
                }
            }
        }

        assert!(seen.len() > 150, "the operations were not found");
        assert!(shared.is_empty(), "operationIds given twice: {shared:#?}");
    }

    /// And every id is `<module>_<handler>` — the handler's file and its
    /// function, which no two handlers share — so a handler added without one
    /// does not take its function's bare name, `list` or `detail`, as every
    /// one once did.
    #[test]
    fn every_operation_is_named_for_its_module_and_handler() {
        let declared = Declared::read();
        let misnamed: Vec<_> = declared
            .handlers
            .iter()
            .filter(|(expected, given)| given.as_ref() != Some(expected))
            .collect();

        assert!(declared.handlers.len() > 150, "the handlers were not found");
        assert!(
            misnamed.is_empty(),
            "#[utoipa::path] without operation_id = \"<module>_<handler>\" \
             (expected, given): {misnamed:#?}"
        );
    }

    /// utoipa keys schemas on the leaf type name, so two modules each declaring
    /// a `ListResponse` silently collapse into one and half the API ends up
    /// documented with the wrong body. Every schema name must be distinct, and
    /// the audit response must be the audit response.
    #[test]
    fn schema_names_do_not_collide() {
        let json = serde_json::to_value(openapi_only()).unwrap();
        let schemas = json["components"]["schemas"].as_object().expect("schemas");

        let audit = schemas
            .get("AuditListResponse")
            .expect("the audit response must have its own schema");
        let props = audit["properties"].as_object().expect("properties");

        assert!(
            props.contains_key("entries"),
            "AuditListResponse lost its fields"
        );
        assert!(props.contains_key("actions"));

        let items = schemas["ItemListResponse"]["properties"]
            .as_object()
            .expect("properties");
        assert!(items.contains_key("items"));

        // Two sign-ins, each its own: the interface's, and a TheTVDB client's.
        assert!(
            schemas["LoginRequest"]["properties"]
                .as_object()
                .is_some_and(|props| props.contains_key("username")),
            "the interface's sign-in lost its fields"
        );
        assert!(
            schemas["TvdbLoginRequest"]["properties"]
                .as_object()
                .is_some_and(|props| props.contains_key("apikey")),
            "the TheTVDB sign-in lost its fields"
        );

        // Two translations, each its own: Radarr's in PascalCase, Skyhook's
        // second edition in camelCase.
        assert!(
            schemas["TranslationResource"]["properties"]
                .as_object()
                .is_some_and(|props| props.contains_key("Language")),
            "Radarr's translation lost its fields"
        );
        assert!(
            schemas["SkyhookTranslationResource"]["properties"]
                .as_object()
                .is_some_and(|props| props.contains_key("language")),
            "Skyhook's translation lost its fields"
        );
    }

    /// The test above checks the pairs that once collided; this one, every
    /// type. utoipa files a schema under its type's name — or the one
    /// `#[schema(as = …)]` gives — and of two types filed under one name it
    /// keeps one without a word, so the fields typed with the other point at
    /// the wrong shape: before this was checked, ten names stood for two
    /// types or more, and twenty-five fields for the wrong shape. The
    /// assembled spec cannot show that, having kept one of each, so this
    /// reads the source as utoipa's derive does.
    #[test]
    fn no_two_types_share_a_schema_name() {
        let declared = Declared::read();

        assert!(
            declared.types.len() > 200,
            "the types deriving ToSchema were not found"
        );
        let shared: Vec<_> = declared
            .types
            .iter()
            .filter(|(_, types)| types.len() > 1)
            .collect();
        assert!(
            shared.is_empty(),
            "schema names given to more than one type — name one apart with \
             #[schema(as = …)]: {shared:#?}"
        );
    }

    /// A field the native API always sends — an `Option` written out as
    /// `null` when there is none, a list with a `#[serde(default)]` — is
    /// documented as required, and nullable where it can be `null`. utoipa
    /// reads either as optional, and the interface's types take optional to
    /// mean absent, so a `null` would reach a page told it cannot.
    /// `#[schema(required)]` says what is sent; `skip_serializing_if` would
    /// change it, for every client. The compatibility surfaces' resources are
    /// documented as their clients read them, and are not checked here.
    #[test]
    fn what_the_native_api_always_sends_is_documented_as_required() {
        let json = serde_json::to_value(openapi_only()).unwrap();
        let schemas = &json["components"]["schemas"];

        // Every schema an answer of the native API reaches, at any depth.
        let mut answered = std::collections::BTreeSet::new();
        for (path, item) in json["paths"].as_object().unwrap() {
            if path.starts_with("/api/v1/") {
                for operation in item.as_object().unwrap().values() {
                    collect_refs(&operation["responses"], &mut answered);
                }
            }
        }
        let mut unread: Vec<String> = answered.iter().cloned().collect();
        while let Some(name) = unread.pop() {
            let mut refs = std::collections::BTreeSet::new();
            collect_refs(&schemas[name.as_str()], &mut refs);
            unread.extend(refs.into_iter().filter(|r| answered.insert(r.clone())));
        }

        let declared = Declared::read();
        let mut loose = Vec::new();
        for name in &answered {
            let Some(sent) = declared.always_sent.get(name) else {
                continue;
            };
            let schema = &schemas[name.as_str()];
            let required = schema["required"].as_array().cloned().unwrap_or_default();
            for field in sent {
                if schema["properties"].get(field).is_some()
                    && !required.iter().any(|r| r.as_str() == Some(field.as_str()))
                {
                    loose.push(format!("{name}.{field}"));
                }
            }
        }

        assert!(
            answered.len() > 100,
            "the native API's answers were not found"
        );
        assert!(
            loose.is_empty(),
            "always sent, yet documented as optional — mark them \
             #[schema(required)]: {loose:#?}"
        );
    }

    /// The spec as the source declares it, read with syn — the types deriving
    /// `ToSchema`, the handlers with a `#[utoipa::path]` — for what the
    /// assembled spec no longer shows.
    #[derive(Default)]
    struct Declared {
        /// The file being read, from `src/`.
        file: String,
        /// Every schema name, and the types given it.
        types: std::collections::BTreeMap<String, Vec<String>>,
        /// Of every struct the server writes out, the fields always written,
        /// by the names serde writes them under.
        always_sent: std::collections::BTreeMap<String, Vec<String>>,
        /// Of every handler with a `#[utoipa::path]`, the operation id the
        /// scheme gives it, and the one it is given.
        handlers: Vec<(String, Option<String>)>,
    }

    impl Declared {
        fn read() -> Self {
            let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
            let mut declared = Self::default();
            for path in rust_files(&src) {
                let text = std::fs::read_to_string(&path).unwrap();
                let file = syn::parse_file(&text)
                    .unwrap_or_else(|error| panic!("{} does not parse: {error}", path.display()));
                declared.file = path.strip_prefix(&src).unwrap().display().to_string();
                syn::visit::Visit::visit_file(&mut declared, &file);
            }
            declared
        }

        /// The schema name of a type deriving `ToSchema`, noted.
        fn note(&mut self, attrs: &[syn::Attribute], ident: &syn::Ident) -> Option<String> {
            if !derives(attrs, "ToSchema") {
                return None;
            }
            let name = attrs
                .iter()
                .find_map(schema_as)
                .unwrap_or_else(|| ident.to_string());
            self.types
                .entry(name.clone())
                .or_default()
                .push(format!("{ident} in {}", self.file));
            Some(name)
        }
    }

    impl<'ast> syn::visit::Visit<'ast> for Declared {
        fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
            use syn::ext::IdentExt;

            if let Some(name) = self.note(&item.attrs, &item.ident)
                && derives(&item.attrs, "Serialize")
                && let syn::Fields::Named(fields) = &item.fields
            {
                let rename_all = serde_of(&item.attrs).rename_all;
                let written = fields.named.iter().filter_map(|field| {
                    let serde = serde_of(&field.attrs);
                    if serde.left_out || serde.flatten {
                        return None;
                    }
                    let ident = field.ident.as_ref()?.unraw().to_string();
                    Some(
                        serde
                            .rename
                            .unwrap_or_else(|| renamed(&ident, rename_all.as_deref())),
                    )
                });
                self.always_sent.entry(name).or_default().extend(written);
            }
            syn::visit::visit_item_struct(self, item);
        }

        fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
            self.note(&item.attrs, &item.ident);
            syn::visit::visit_item_enum(self, item);
        }

        fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
            let documented = item.attrs.iter().find(|attr| {
                let path: Vec<_> = attr
                    .path()
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                path == ["utoipa", "path"]
            });
            if let Some(attr) = documented {
                let module = std::path::Path::new(&self.file)
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let given = match argument(attr, "operation_id") {
                    Some(syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(id),
                        ..
                    })) => Some(id.value()),
                    _ => None,
                };
                self.handlers
                    .push((format!("{module}_{}", item.sig.ident), given));
            }
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn derives(attrs: &[syn::Attribute], name: &str) -> bool {
        use syn::punctuated::Punctuated;

        attrs
            .iter()
            .filter(|attr| attr.path().is_ident("derive"))
            .any(|attr| {
                attr.parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
                    .is_ok_and(|derived| {
                        derived
                            .iter()
                            .any(|path| path.segments.last().is_some_and(|s| s.ident == name))
                    })
            })
    }

    /// The name a `#[schema(as = …)]` gives, as utoipa writes it: the path's
    /// segments joined by dots.
    fn schema_as(attr: &syn::Attribute) -> Option<String> {
        if !attr.path().is_ident("schema") {
            return None;
        }
        match argument(attr, "as")? {
            syn::Expr::Path(path) => {
                let segments: Vec<_> = path
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                Some(segments.join("."))
            }
            _ => None,
        }
    }

    /// What `key = …` says among an attribute's arguments.
    fn argument(attr: &syn::Attribute, key: &str) -> Option<syn::Expr> {
        attr.parse_args_with(|input: syn::parse::ParseStream| {
            let mut value = None;
            while !input.is_empty() {
                // A token at a time: the key — `as` is a keyword, and still
                // an identifier here — or whatever else the attribute says,
                // `title = "…"`, `example = json!(…)`.
                let named = input.step(|cursor| {
                    if let Some((ident, rest)) = cursor.ident() {
                        return Ok((ident == key, rest));
                    }
                    match cursor.token_tree() {
                        Some((_, rest)) => Ok((false, rest)),
                        None => Err(cursor.error("nothing left")),
                    }
                })?;
                if named && input.peek(syn::Token![=]) {
                    input.parse::<syn::Token![=]>()?;
                    value = Some(input.parse::<syn::Expr>()?);
                }
            }
            Ok(value)
        })
        .ok()
        .flatten()
    }

    /// What a type's or a field's `#[serde]` says of how it is written.
    #[derive(Default)]
    struct SerdeSays {
        rename: Option<String>,
        rename_all: Option<String>,
        /// Always or sometimes: `skip`, `skip_serializing`, `skip_serializing_if`.
        left_out: bool,
        flatten: bool,
    }

    fn serde_of(attrs: &[syn::Attribute]) -> SerdeSays {
        let mut says = SerdeSays::default();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            attr.parse_nested_meta(|meta| {
                let key = meta
                    .path
                    .get_ident()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                match key.as_str() {
                    "rename" => says.rename = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                    "rename_all" => {
                        says.rename_all = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                    }
                    "skip" | "skip_serializing" => says.left_out = true,
                    "skip_serializing_if" => {
                        meta.value()?.parse::<syn::LitStr>()?;
                        says.left_out = true;
                    }
                    "flatten" => says.flatten = true,
                    // Anything else — `default`, `alias = "…"`,
                    // `serialize_with = "…"` — neither renames a field nor
                    // leaves it out.
                    _ if meta.input.peek(syn::Token![=]) => {
                        meta.value()?.parse::<syn::LitStr>()?;
                    }
                    _ => {}
                }
                Ok(())
            })
            .unwrap_or_else(|error| panic!("a #[serde] this test cannot read: {error}"));
        }
        says
    }

    /// A field's name as serde writes it under its type's `rename_all`.
    fn renamed(field: &str, rule: Option<&str>) -> String {
        let capitalized: String = field
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                    .unwrap_or_default()
            })
            .collect();
        match rule {
            None | Some("snake_case") => field.to_string(),
            Some("lowercase") => field.to_lowercase(),
            Some("PascalCase") => capitalized,
            Some("camelCase") => {
                let mut chars = capitalized.chars();
                chars
                    .next()
                    .map(|first| first.to_lowercase().chain(chars).collect())
                    .unwrap_or_default()
            }
            Some(other) => panic!("rename_all = {other:?} is not one this test reads"),
        }
    }

    fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(rust_files(&path));
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
        files
    }

    /// A schema nothing points at is either dead weight or a sign that a `$ref`
    /// went to a collided name instead.
    #[test]
    fn every_schema_is_reachable_from_some_operation() {
        let json = serde_json::to_value(openapi_only()).unwrap();
        let defined: std::collections::BTreeSet<String> = json["components"]["schemas"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();

        let mut referenced = std::collections::BTreeSet::new();
        collect_refs(&json, &mut referenced);

        let dangling: Vec<_> = referenced.difference(&defined).collect();
        assert!(
            dangling.is_empty(),
            "references with no schema: {dangling:?}"
        );

        let orphaned: Vec<_> = defined.difference(&referenced).collect();
        assert!(
            orphaned.is_empty(),
            "schemas nothing references: {orphaned:?}"
        );
    }

    fn collect_refs(node: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match node {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(r)) = map.get("$ref")
                    && let Some(name) = r.strip_prefix("#/components/schemas/")
                {
                    out.insert(name.to_string());
                }
                for value in map.values() {
                    collect_refs(value, out);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    collect_refs(value, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn the_ways_a_client_may_authenticate_are_all_described() {
        let json = serde_json::to_value(openapi_only()).unwrap();
        let schemes = json["components"]["securitySchemes"]
            .as_object()
            .expect("securitySchemes");

        for name in ["apiKey", "apiKeyQuery", "bearer", "session"] {
            assert!(schemes.contains_key(name), "{name} is not described");
        }
    }

    #[test]
    fn sign_in_is_documented_as_needing_no_credential() {
        // It is the one route that cannot require one.
        let json = serde_json::to_value(openapi_only()).unwrap();
        let login = &json["paths"]["/api/v1/auth/login"]["post"];

        assert_eq!(
            login["security"].as_array().map(Vec::len),
            Some(0),
            "sign-in must not advertise a security requirement"
        );
    }

    #[test]
    fn the_arr_surfaces_are_documented_as_address_guarded() {
        // They cannot present a credential, so advertising one would mislead.
        let json = serde_json::to_value(openapi_only()).unwrap();

        for path in [
            "/v1/tvdb/shows/{language}/{tvdb_id}",
            "/v1/movie/{tmdb_id}",
            "/plex",
            "/plex/library/metadata/{rating_key}",
        ] {
            assert_eq!(
                json["paths"][path]["get"]["security"]
                    .as_array()
                    .map(Vec::len),
                Some(0),
                "{path} should carry no security requirement"
            );
        }
        assert_eq!(
            json["paths"]["/plex/library/metadata/matches"]["post"]["security"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "Plex's matches should carry no security requirement"
        );
    }

    /// The interface's types are generated from a copy of the spec kept in
    /// `frontend/openapi.json` (`npm run types` there writes
    /// `src/lib/api.gen.ts`), so a field added here reaches the pages' types
    /// without anybody copying it by hand — as long as the copy is current.
    /// `UPDATE_OPENAPI=1` rewrites it; otherwise a stale copy fails here.
    ///
    /// The version is left out of the copy: a release would make it stale
    /// without a single shape having changed.
    #[test]
    fn the_interfaces_openapi_copy_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend/openapi.json");
        let mut json = serde_json::to_value(openapi_only()).unwrap();
        json["info"]["version"] = "see Cargo.toml".into();
        let fresh = serde_json::to_string_pretty(&json).unwrap() + "\n";

        if std::env::var_os("UPDATE_OPENAPI").is_some_and(|value| value == "1") {
            std::fs::write(&path, &fresh).expect("frontend/openapi.json must be writable");
            return;
        }

        let kept = std::fs::read_to_string(&path).unwrap_or_default();
        // Not `assert_eq!`: both sides are the whole spec, and the diff that
        // matters is the one git shows once the copy is refreshed.
        assert!(
            kept == fresh,
            "frontend/openapi.json is not the spec this server builds. Refresh it with \
             `UPDATE_OPENAPI=1 cargo test openapi`, then `npm run types` in frontend/, \
             and commit both files."
        );
    }
}
