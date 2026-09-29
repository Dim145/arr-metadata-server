//! HTTP surfaces.
//!
//! Three compatibility surfaces plus this server's own API. They are separate
//! routers because they have separate authentication policies — see
//! [`crate::config::Surface`] — and they are documented together, because an
//! operator wants one page describing everything the server answers.

pub mod audit;
pub mod extract;
pub mod native;
pub mod radarr;
pub mod sonarr;
pub mod sonarr_services;
pub mod tmdb;

use axum::{Router, middleware::from_fn_with_state};
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
here in full.

**Locks.** Any field a person edits is recorded separately from provider data \
and is never overwritten by a refresh. That is what the `/items/{id}/overrides` \
routes manage, and what `lockedFields` reports.

**Authentication differs per surface.** Sonarr and Radarr have their metadata \
URLs compiled in and cannot attach a credential, so those routes are guarded by \
address rather than by key. The routes documented without a security \
requirement are those; they are not open.",
        version = env!("CARGO_PKG_VERSION"),
        license(name = "MIT"),
    ),
    // The TMDB relay is registered with an axum wildcard, which `routes!` cannot
    // collect; name it here so it still reaches the spec.
    paths(tmdb::proxy, tmdb::proxy_v4),
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

    let mut api = ApiDoc::openapi();
    for part in [native_api, public_api, arr_api, tmdb_api] {
        api.merge(part);
    }

    let router = Router::new()
        .merge(
            native
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
        .merge(tmdb_surface(state, tmdb));

    (router, api)
}

/// The surfaces the clients with compiled-in addresses call, for their own
/// door: the same handlers under the same guards, and nothing else.
pub fn build_clients(state: AppState) -> Router<AppState> {
    let (arr, _) = arr_routers().split_for_parts();
    let (tmdb, _) = tmdb::router().split_for_parts();
    Router::new()
        .merge(arr_surface(state.clone(), arr))
        .merge(tmdb_surface(state, tmdb))
}

/// Whatever else Sonarr asks of services.sonarr.tv, relayed under the guards
/// of Sonarr's surface: what the clients' door hands a path it has no route
/// for, when it was asked of that name.
pub fn build_sonarr_services_relay(state: AppState) -> Router {
    arr_surface(
        state.clone(),
        Router::new().fallback(sonarr_services::fallback),
    )
    .with_state(state)
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

fn tmdb_surface(state: AppState, tmdb: Router<AppState>) -> Router<AppState> {
    tmdb.layer(from_fn_with_state(state.clone(), guards::guard_tmdb))
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

    let mut api = ApiDoc::openapi();
    for part in [native_api, public_api, arr_api, tmdb_api] {
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
            "/api/v1/lists/{key}/sonarr.json",
            "/v1/tvdb/shows/{language}/{tvdb_id}",
            "/v1/movie/{tmdb_id}",
            "/v1/list/imdb/{id}",
            "/3/{path}",
            "/4/{path}",
        ] {
            assert!(paths.contains_key(expected), "{expected} is not documented");
        }
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

        for path in ["/v1/tvdb/shows/{language}/{tvdb_id}", "/v1/movie/{tmdb_id}"] {
            assert_eq!(
                json["paths"][path]["get"]["security"]
                    .as_array()
                    .map(Vec::len),
                Some(0),
                "{path} should carry no security requirement"
            );
        }
    }
}
