//! The web UI, compiled into the binary.
//!
//! One binary with no companion directory is the whole point: the container
//! image is a single static file. Vite's fingerprinted bundles are served
//! with immutable caching; everything else, the page first of all, is asked
//! about on every load.
//!
//! The page is the same for every route but one thing: a link to a work, to a
//! list, or to a genre's, a network's, a year's or a decade's page carries,
//! in its head, what a link preview shows of it — a title, a line, a poster —
//! for the readers that never run the app, and only while a reader with no
//! credential would be let in at all. A work's page carries what a search
//! engine reads of it too, as schema.org describes a film or a series — only
//! where the site is public, the one case a search engine is meant to read it.

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use rust_embed::RustEmbed;

use crate::{
    config::SurfacePolicy,
    db::repo::{self, editorial::Scope, list::CuratedList},
    domain::{CoverType, CreditType, MediaItem, MediaKind},
    service,
    state::AppState,
};

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

/// Paths that belong to the API. A request under one of these must 404 rather
/// than fall through to the UI's index page, or a mistyped endpoint would
/// answer `200 text/html` and confuse every client.
const API_PREFIXES: &[&str] = &[
    "/api/", "/v1/", "/v2/", "/3/", "/4/", "/v4/", "/plex/", "/media/", "/health", "/ready",
];

/// The same roots, as the trailing-slash normalisation leaves `/api/` and
/// its like: no prefix above matches them any more, and they used to be
/// answered the interface's page.
const API_ROOTS: &[&str] = &["/api", "/v1", "/v2", "/3", "/4", "/v4", "/plex", "/media"];

/// Where Vite puts what it fingerprints. A file there that is not in the
/// build is a bundle a page from before a deploy still names: a 404 the
/// browser reports, never the page itself served as a script.
const ASSETS: &str = "assets/";

/// Where the page keeps the lines a preview replaces.
const PREVIEW_START: &str = "<!-- preview -->";
const PREVIEW_END: &str = "<!-- /preview -->";

pub fn router() -> Router<AppState> {
    Router::new().fallback(any(serve))
}

async fn serve(State(state): State<AppState>, request: Request) -> Response {
    let path = request.uri().path();

    if is_api_path(path) {
        // In the API's own shape, as its other refusals are.
        return crate::error::AppError::NotFound.into_response();
    }

    let trimmed = path.trim_start_matches('/');

    if let Some(response) = asset(trimmed) {
        return response;
    }
    if trimmed.starts_with(ASSETS) {
        return not_found();
    }

    // Anything else is a client-side route: hand back the app and let the
    // router in the browser resolve it — with the preview of what the route
    // shows, for whoever reads the page without running it.
    let Some(file) = Assets::get("index.html") else {
        return not_found();
    };
    let mut html = String::from_utf8_lossy(&file.data).into_owned();
    if let Some(preview) = preview(&state, path).await {
        html = preview.apply(&html, state.config.server.public_url.as_deref(), path);
    } else if let Some(public) = state.config.server.public_url.as_deref() {
        html = absolute_site_image(&html, public);
    }
    page(html)
}

fn asset(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;

    let mime = mime_guess::from_path(path).first_or_octet_stream();

    // Only what Vite fingerprints may be kept for a year: a manifest or an
    // icon at a fixed name would otherwise be the old one until then.
    let cache = if path.starts_with(ASSETS) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    let mut response = Response::new(Body::from(file.data.into_owned()));

    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref()).ok()?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));

    Some(response)
}

/// The entry page: never cached, since it names the fingerprinted bundles.
fn page(html: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        html,
    )
        .into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// Whether a path belongs to the API: under one of its prefixes, or one of
/// its roots on its own.
fn is_api_path(path: &str) -> bool {
    API_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) || API_ROOTS.contains(&path)
}

// ─── link previews ───────────────────────────────────────────────────────────

/// What a link preview shows of a page: a title, a line, a picture.
struct Preview {
    title: String,
    description: Option<String>,
    image: Option<String>,
    /// Open Graph's word for it: `video.tv_show`, `video.movie`, `website`.
    kind: &'static str,
    /// What a search engine reads of the page, as JSON-LD ready to sit in a
    /// script element (see [`structured`]): a work's, on a public site.
    structured: Option<String>,
}

impl Preview {
    /// The page with this preview in its head in place of the site's own
    /// lines — those it has something to say for; the site's description and
    /// picture stay where the preview has none.
    fn apply(&self, html: &str, public_url: Option<&str>, path: &str) -> String {
        let title = escape(&self.title);
        let mut head = format!(
            "<meta property=\"og:site_name\" content=\"Cinémathèque\" />\n    <meta property=\"og:type\" content=\"{}\" />\n    <meta property=\"og:title\" content=\"{title}\" />\n",
            self.kind
        );
        if let Some(public) = public_url {
            head.push_str(&format!(
                "    <meta property=\"og:url\" content=\"{}{}\" />\n",
                escape(public),
                escape(path)
            ));
        }
        match &self.description {
            Some(description) => {
                let description = escape(description);
                head.push_str(&format!(
                    "    <meta property=\"og:description\" content=\"{description}\" />\n    <meta name=\"description\" content=\"{description}\" />\n"
                ));
            }
            None => head.push_str(
                "    <meta property=\"og:description\" content=\"A private film and television catalogue, served to the whole stack.\" />\n    <meta name=\"description\" content=\"A private film and television catalogue, served to the whole stack.\" />\n",
            ),
        }
        let image = match &self.image {
            Some(image) => escape(image),
            None => match public_url {
                Some(public) => format!("{}/icon-512.png", escape(public)),
                None => "/icon-512.png".to_string(),
            },
        };
        head.push_str(&format!(
            "    <meta property=\"og:image\" content=\"{image}\" />\n    <meta name=\"twitter:card\" content=\"summary\" />\n    "
        ));
        // A data block, not a script: the page's policy, which runs only this
        // origin's scripts, has nothing to say to it, and nothing runs it.
        if let Some(data) = &self.structured {
            head.push_str(&format!(
                "<script type=\"application/ld+json\">{data}</script>\n    "
            ));
        }

        let replaced = match (html.find(PREVIEW_START), html.find(PREVIEW_END)) {
            (Some(start), Some(end)) if start < end => {
                format!(
                    "{}{head}{}",
                    &html[..start],
                    &html[end + PREVIEW_END.len()..]
                )
            }
            // A page without the markers keeps its own lines; the title is
            // still the work's.
            _ => html.to_string(),
        };
        replaced.replacen(
            "<title>Cinémathèque</title>",
            &format!("<title>{title} · Cinémathèque</title>"),
            1,
        )
    }
}

/// The site's own picture, as an address an unfurler can fetch.
fn absolute_site_image(html: &str, public_url: &str) -> String {
    html.replacen(
        "content=\"/icon-512.png\"",
        &format!("content=\"{}/icon-512.png\"", escape(public_url)),
        1,
    )
}

/// Whether a reader with no credential is let into the catalogue at all —
/// which is what a preview fetcher is. The native surface's own policy
/// decides, as it does for the pages themselves.
fn open_to_readers(state: &AppState) -> bool {
    match state.config.security.native_policy {
        SurfacePolicy::Open => true,
        SurfacePolicy::ApiKey => state.public_site(),
        SurfacePolicy::Allowlist => false,
    }
}

/// The preview a path deserves: a work's, a list's, or none. The shape of
/// the id or slug is checked before anything is looked up, so a stranger
/// walking random addresses is answered from the page alone.
async fn preview(state: &AppState, path: &str) -> Option<Preview> {
    if !open_to_readers(state) {
        return None;
    }
    let mut parts = path.trim_start_matches('/').split('/');
    match (parts.next()?, parts.next()?) {
        ("work", id) if uuid::Uuid::parse_str(id).is_ok() => work_preview(state, id).await,
        ("lists", key) if is_key(key) && parts.next().is_none() => list_preview(state, key).await,
        (page @ ("genre" | "network" | "year" | "decade"), key)
            if !key.is_empty() && key.len() <= 128 && parts.next().is_none() =>
        {
            let key = urlencoding::decode(key).ok()?;
            page_preview(state, page, &key).await
        }
        _ => None,
    }
}

/// A list's slug or id: what `make_slug` produces, or a UUID.
fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

async fn work_preview(state: &AppState, id: &str) -> Option<Preview> {
    let item = service::load(state, id).await.ok().flatten()?;
    // As the work's own page decides it for a reader with no credential.
    if !item.is_enabled || (item.is_adult && !state.adult_for(None, None, Some(true))) {
        return None;
    }
    let title = match item.year {
        Some(year) => format!("{} ({year})", item.title),
        None => item.title.clone(),
    };
    let image = poster_of(state, &item);
    let description = item.overview.as_deref().and_then(line);
    // For a search engine, and only where the site is public: one let in by
    // the surface's own policy is a home network's, not the web's.
    let structured = state.public_site().then(|| {
        let published = match item.kind {
            MediaKind::Series => item.first_aired.as_deref(),
            MediaKind::Movie => item
                .in_cinemas
                .as_deref()
                .or(item.digital_release.as_deref())
                .or(item.physical_release.as_deref()),
        };
        let mut directors: Vec<&str> = Vec::new();
        for credit in &item.credits {
            if credit.credit_type == CreditType::Director
                && !directors.contains(&credit.person_name.as_str())
            {
                directors.push(&credit.person_name);
            }
        }
        structured(&Described {
            kind: item.kind,
            name: &item.title,
            url: state
                .config
                .server
                .public_url
                .as_deref()
                .map(|public| format!("{}/work/{}", public.trim_end_matches('/'), item.id)),
            image: image.as_deref(),
            description: description.as_deref(),
            published: published.and_then(day),
            genres: &item.genres,
            directors,
            rating: item
                .headline_rating()
                .and_then(|r| Some((r.value?, r.votes.filter(|&votes| votes > 0)?))),
        })
    });
    Some(Preview {
        title,
        description,
        image,
        kind: match item.kind {
            MediaKind::Series => "video.tv_show",
            MediaKind::Movie => "video.movie",
        },
        structured,
    })
}

/// The poster every client is given of a work — the chosen one, else one
/// added by hand, else the best the sources offered — as an address a
/// crawler can follow.
fn poster_of(state: &AppState, item: &MediaItem) -> Option<String> {
    crate::domain::lead_images(
        item.images.iter().filter(|i| i.season_number.is_none()),
        &item.primary_images,
    )
    .into_iter()
    .find(|i| i.cover_type == CoverType::Poster)
    .and_then(|i| {
        // A copy kept, addressed by a path: under the public URL for a
        // crawler, which cannot follow the path, or the provider's
        // address when there is none.
        if i.url.starts_with(crate::media::ROUTE) {
            state.media.for_elsewhere(&i.url)
        } else {
            Some(i.url.clone())
        }
    })
    .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
}

/// The preview of a genre's, a network's, a year's or a decade's page: its
/// name in the catalogue's own language, its programmer's note — or else how
/// many works it holds — and the poster of the one rated highest.
async fn page_preview(state: &AppState, page: &str, key: &str) -> Option<Preview> {
    // A preview fetcher says no language: the one the catalogue is kept in.
    let language = repo::genre::language_of(&state.language(None, None));
    let adult = state.adult_for(None, None, None);
    let (scope, slug, title, filters) = match page {
        "genre" => {
            let genre = repo::genre::find(&state.db, key).await.ok().flatten()?;
            let title = genre.name(Some(&language));
            let filters = repo::item::Query {
                genres: vec![genre.slug.clone()],
                ..Default::default()
            };
            (Scope::Genre, genre.slug, title, filters)
        }
        "network" => {
            let slug = slug::slugify(key);
            let name = crate::api::native::browse::network_named(state, &slug, adult)
                .await
                .ok()
                .flatten()?;
            let filters = repo::item::Query {
                network: Some(name.clone()),
                ..Default::default()
            };
            (Scope::Network, slug, name, filters)
        }
        "year" => {
            let year = crate::api::native::browse::year_of(key)?;
            let filters = repo::item::Query {
                year_from: Some(year),
                year_to: Some(year),
                ..Default::default()
            };
            (Scope::Year, year.to_string(), year.to_string(), filters)
        }
        _ => {
            let year = crate::api::native::browse::year_of(key)?;
            let first = year - year.rem_euclid(10);
            let filters = repo::item::Query {
                year_from: Some(first),
                year_to: Some(first + 9),
                ..Default::default()
            };
            let title = if language == "fr" {
                format!("Les années {first}")
            } else {
                format!("The {first}s")
            };
            (Scope::Decade, first.to_string(), title, filters)
        }
    };
    facet_preview(state, scope, &slug, title, filters, &language, adult).await
}

/// What [`page_preview`] reads once it knows what the page is of.
async fn facet_preview(
    state: &AppState,
    scope: Scope,
    slug: &str,
    title: String,
    filters: repo::item::Query,
    language: &str,
    adult: bool,
) -> Option<Preview> {
    let filters = repo::item::Query {
        include_adult: adult,
        sort: repo::item::Sort::Rating,
        limit: 1,
        offset: 0,
        ..filters
    };
    let (found, count) = repo::item::search_counted(&state.db, &filters).await.ok()?;
    let image = match found.first() {
        Some(best) => service::load(state, &best.id)
            .await
            .ok()
            .flatten()
            .and_then(|work| poster_of(state, &work)),
        None => None,
    };
    let note = repo::editorial::read(&state.db, scope, slug, language)
        .await
        .ok()
        .flatten();
    let description = note
        .map(|note| repo::editorial::plain(&note.body))
        .and_then(|text| line(&text))
        .or_else(|| Some(counted(count, language)));
    Some(Preview {
        title,
        description,
        image,
        kind: "website",
        structured: None,
    })
}

/// How many works a page holds, said in the catalogue's language.
fn counted(count: i64, language: &str) -> String {
    match (language, count) {
        ("fr", 0) => "Aucune œuvre au catalogue pour l’instant.".to_string(),
        ("fr", 1) => "Une œuvre au catalogue.".to_string(),
        ("fr", n) => format!("{n} œuvres au catalogue."),
        (_, 0) => "Nothing in the catalogue yet.".to_string(),
        (_, 1) => "One work in the catalogue.".to_string(),
        (_, n) => format!("{n} works in the catalogue."),
    }
}

/// A work, as much of it as a search engine is told.
struct Described<'a> {
    kind: MediaKind,
    name: &'a str,
    url: Option<String>,
    image: Option<&'a str>,
    description: Option<&'a str>,
    /// The day it came out: `YYYY-MM-DD`.
    published: Option<&'a str>,
    genres: &'a [String],
    directors: Vec<&'a str>,
    /// The mark the plate leads with, out of ten, and how many votes made
    /// it: a mark nobody voted for is no rating to a search engine.
    rating: Option<(f64, i64)>,
}

/// A work as schema.org describes one: a `Movie` — the type Google documents
/// for its film results — or a `TVSeries`, with what the page itself shows
/// of it, and nothing it lacks. Written for a script element: no `<`, `>` or
/// `&` is left in it as itself, so no synopsis can close the element early.
fn structured(work: &Described) -> String {
    let mut data = serde_json::Map::new();
    data.insert("@context".into(), "https://schema.org".into());
    data.insert(
        "@type".into(),
        match work.kind {
            MediaKind::Movie => "Movie",
            MediaKind::Series => "TVSeries",
        }
        .into(),
    );
    data.insert("name".into(), work.name.into());
    if let Some(url) = &work.url {
        data.insert("url".into(), url.as_str().into());
    }
    if let Some(image) = work.image {
        data.insert("image".into(), image.into());
    }
    if let Some(description) = work.description {
        data.insert("description".into(), description.into());
    }
    if let Some(published) = work.published {
        data.insert("datePublished".into(), published.into());
    }
    if !work.genres.is_empty() {
        data.insert("genre".into(), work.genres.into());
    }
    if !work.directors.is_empty() {
        data.insert(
            "director".into(),
            work.directors
                .iter()
                .map(|name| serde_json::json!({ "@type": "Person", "name": name }))
                .collect(),
        );
    }
    if let Some((value, votes)) = work.rating {
        data.insert(
            "aggregateRating".into(),
            serde_json::json!({
                "@type": "AggregateRating",
                "ratingValue": (value * 10.0).round() / 10.0,
                "bestRating": 10,
                // One unless a source gave less, which TMDB's scale allows.
                "worstRating": if value < 1.0 { 0 } else { 1 },
                "ratingCount": votes,
            }),
        );
    }
    serde_json::Value::Object(data)
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// The day a date begins with, `YYYY-MM-DD`, where it has the shape of one.
fn day(value: &str) -> Option<&str> {
    let day = value.get(..10)?;
    let shaped = day.bytes().enumerate().all(|(at, b)| match at {
        4 | 7 => b == b'-',
        _ => b.is_ascii_digit(),
    });
    shaped.then_some(day)
}

async fn list_preview(state: &AppState, key: &str) -> Option<Preview> {
    let list: CuratedList = match repo::list::get(&state.db, key).await.ok().flatten() {
        Some(list) => list,
        None => repo::list::by_slug(&state.db, key).await.ok().flatten()?,
    };
    if !list.is_public {
        return None;
    }
    // The selection's cover, as an address a crawler can follow: the copy
    // kept, under the public URL, or the address it was given by.
    let image = list
        .cover
        .as_deref()
        .and_then(|cover| state.media.for_elsewhere(&state.media.localized(cover)))
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"));
    Some(Preview {
        title: list.name.clone(),
        description: list.description.as_deref().and_then(line),
        image,
        kind: "website",
        structured: None,
    })
}

/// One line of a text: its whitespace run together, and cut at a word before
/// three hundred characters. Nothing, for a text that says nothing.
fn line(text: &str) -> Option<String> {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        return None;
    }
    if joined.chars().count() <= 300 {
        return Some(joined);
    }
    let cut: String = joined.chars().take(297).collect();
    let cut = cut
        .rsplit_once(' ')
        .map(|(head, _)| head.to_string())
        .unwrap_or(cut);
    Some(format!("{cut}…"))
}

/// Text inside an attribute: the five characters HTML reserves.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<html>\n  <head>\n    <title>Cinémathèque</title>\n    <!-- preview -->\n    <meta property=\"og:site_name\" content=\"Cinémathèque\" />\n    <meta property=\"og:title\" content=\"Cinémathèque\" />\n    <meta name=\"description\" content=\"A catalogue.\" />\n    <meta property=\"og:image\" content=\"/icon-512.png\" />\n    <meta name=\"twitter:card\" content=\"summary\" />\n    <!-- /preview -->\n    <script type=\"module\" src=\"/assets/index-abc.js\"></script>\n  </head>\n  <body></body>\n</html>";

    #[test]
    fn api_paths_are_not_ui_paths() {
        for path in [
            "/api/v1/items",
            "/v1/tvdb/search/en",
            "/3/tv/1",
            "/4/list/8136",
            "/health",
            "/ready",
        ] {
            assert!(is_api_path(path), "{path}");
        }
    }

    #[test]
    fn everything_else_is_a_ui_path() {
        for path in ["/", "/catalogue", "/catalogue/abc", "/assets/index-abc.js"] {
            assert!(!is_api_path(path), "{path}");
        }
    }

    #[test]
    fn a_path_that_merely_starts_like_an_api_path_is_still_ui() {
        // "/v1abc" is not under "/v1/", so it belongs to the client router.
        assert!(!is_api_path("/v1abc"));
        assert!(!is_api_path("/apifoo"));
    }

    #[test]
    fn skyhook_s_second_edition_is_an_api_path() {
        // Sonarr v5 reads JSON here, and a route it mistypes must be told
        // so, not handed the interface's page.
        assert!(is_api_path("/v2/tvdb/shows/eng/81189/official"));
        assert!(is_api_path("/v2/tvdb/search"));
        assert!(is_api_path("/v2"));
    }

    #[test]
    fn plex_s_provider_is_an_api_path() {
        // Plex reads its manifest at the root and JSON under it: a route it
        // mistypes is told so, not handed the interface's page.
        assert!(is_api_path("/plex"));
        assert!(is_api_path("/plex/library/metadata/movie-tmdb-603"));
        assert!(!is_api_path("/plexus"));
    }

    #[test]
    fn an_api_root_trimmed_of_its_slash_is_still_the_apis() {
        // `/api/` reaches here as `/api`, the trailing slash normalised away.
        for path in ["/api", "/v1", "/3", "/4", "/v4", "/media"] {
            assert!(is_api_path(path), "{path}");
        }
    }

    #[test]
    fn a_bundle_no_longer_built_is_not_the_page() {
        // Whatever the build holds, a name it cannot hold is missing.
        let missing = asset("assets/index-0000000000-gone.js");
        assert!(missing.is_none());
        assert!("assets/index-0000000000-gone.js".starts_with(ASSETS));
    }

    #[test]
    fn a_preview_takes_the_place_of_the_sites_own_lines() {
        let preview = Preview {
            title: "Tom & \"Jerry\" </title> (1940)".into(),
            description: Some("A cat, a mouse.".into()),
            image: Some("https://image.example/p.jpg".into()),
            kind: "video.movie",
            structured: None,
        };
        let page = preview.apply(PAGE, Some("https://films.example"), "/work/abc");
        assert!(page.contains(
            "<title>Tom &amp; &quot;Jerry&quot; &lt;/title&gt; (1940) · Cinémathèque</title>"
        ));
        assert!(page.contains("<meta property=\"og:title\" content=\"Tom &amp; &quot;Jerry&quot; &lt;/title&gt; (1940)\" />"));
        assert!(page.contains("<meta property=\"og:type\" content=\"video.movie\" />"));
        assert!(
            page.contains(
                "<meta property=\"og:url\" content=\"https://films.example/work/abc\" />"
            )
        );
        assert!(
            page.contains("<meta property=\"og:image\" content=\"https://image.example/p.jpg\" />")
        );
        assert!(page.contains("<meta name=\"description\" content=\"A cat, a mouse.\" />"));
        // Each of a kind once, the site's own gone, the rest of the head kept.
        assert_eq!(page.matches("og:title").count(), 1);
        assert_eq!(page.matches("og:site_name").count(), 1);
        assert_eq!(page.matches("name=\"description\"").count(), 1);
        assert_eq!(page.matches("twitter:card").count(), 1);
        assert!(!page.contains("A catalogue."));
        assert!(page.contains("<script type=\"module\" src=\"/assets/index-abc.js\"></script>"));
        assert!(page.contains("</head>"));
        assert!(!page.contains("<!-- preview -->"));

        // A page with nothing of its own to say keeps the site's line and
        // picture, made absolute where the address is known.
        let bare = Preview {
            title: "Untitled".into(),
            description: None,
            image: None,
            kind: "website",
            structured: None,
        };
        let page = bare.apply(PAGE, Some("https://films.example"), "/lists/untitled");
        assert!(page.contains("content=\"https://films.example/icon-512.png\""));
        assert!(page.contains("<meta name=\"description\" content=\"A private film and television catalogue, served to the whole stack.\" />"));
        let page = bare.apply(PAGE, None, "/lists/untitled");
        assert!(page.contains("content=\"/icon-512.png\""));
        assert!(!page.contains("og:url"));

        // Without the markers the page is left as it is, but for the title.
        let plain = bare.apply(
            "<title>Cinémathèque</title><meta property=\"og:title\" content=\"x\" />",
            None,
            "/",
        );
        assert!(plain.contains("<title>Untitled · Cinémathèque</title>"));
        assert!(plain.contains("content=\"x\""));
    }

    #[test]
    fn a_film_is_described_as_schema_org_has_one_for_a_search_engine() {
        let genres = vec!["Drama".to_string(), "Crime".to_string()];
        let data = structured(&Described {
            kind: MediaKind::Movie,
            name: "Heat",
            url: Some("https://films.example/work/abc".into()),
            image: Some("https://image.example/heat.jpg"),
            description: Some("A thief, a detective."),
            published: Some("1995-12-15"),
            genres: &genres,
            directors: vec!["Michael Mann"],
            rating: Some((8.27, 700_000)),
        });
        let value: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(value["@context"], "https://schema.org");
        assert_eq!(value["@type"], "Movie");
        assert_eq!(value["name"], "Heat");
        assert_eq!(value["url"], "https://films.example/work/abc");
        assert_eq!(value["image"], "https://image.example/heat.jpg");
        assert_eq!(value["description"], "A thief, a detective.");
        assert_eq!(value["datePublished"], "1995-12-15");
        assert_eq!(value["genre"], serde_json::json!(["Drama", "Crime"]));
        assert_eq!(
            value["director"],
            serde_json::json!([{ "@type": "Person", "name": "Michael Mann" }])
        );
        assert_eq!(value["aggregateRating"]["@type"], "AggregateRating");
        assert_eq!(value["aggregateRating"]["ratingValue"], 8.3);
        assert_eq!(value["aggregateRating"]["bestRating"], 10);
        assert_eq!(value["aggregateRating"]["ratingCount"], 700_000);
    }

    #[test]
    fn a_series_says_only_what_it_has_and_never_closes_its_element() {
        let data = structured(&Described {
            kind: MediaKind::Series,
            name: "Tom & Jerry",
            url: None,
            image: None,
            description: Some("</script><script>alert(1)</script>"),
            published: None,
            genres: &[],
            directors: Vec::new(),
            rating: None,
        });
        // Nothing in it is markup: `<`, `>` and `&` are escapes, which a
        // JSON reader turns back into what they were.
        assert!(!data.contains('<') && !data.contains('>') && !data.contains('&'));
        let value: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(value["@type"], "TVSeries");
        assert_eq!(value["name"], "Tom & Jerry");
        assert_eq!(value["description"], "</script><script>alert(1)</script>");
        // What it lacks, it does not claim.
        for absent in [
            "url",
            "image",
            "datePublished",
            "genre",
            "director",
            "aggregateRating",
        ] {
            assert!(value.get(absent).is_none(), "{absent}");
        }
    }

    #[test]
    fn the_description_sits_in_the_head_with_the_preview_once_there_is_one() {
        let described = Preview {
            title: "Heat (1995)".into(),
            description: None,
            image: None,
            kind: "video.movie",
            structured: Some("{\"@type\":\"Movie\"}".into()),
        };
        let page = described.apply(PAGE, None, "/work/abc");
        assert!(
            page.contains("<script type=\"application/ld+json\">{\"@type\":\"Movie\"}</script>")
        );
        assert_eq!(page.matches("application/ld+json").count(), 1);
        // Before the page's own scripts, inside the head.
        assert!(page.find("application/ld+json") < page.find("/assets/index-abc.js"));

        let plain = Preview {
            structured: None,
            ..described
        };
        assert!(
            !plain
                .apply(PAGE, None, "/work/abc")
                .contains("application/ld+json")
        );
    }

    #[test]
    fn a_day_is_the_date_a_value_begins_with() {
        assert_eq!(day("1995-12-15"), Some("1995-12-15"));
        assert_eq!(day("2016-07-15T00:00:00Z"), Some("2016-07-15"));
        assert_eq!(day("1995"), None);
        assert_eq!(day("15/12/1995"), None);
        assert_eq!(day("1995-12-1é"), None);
    }

    #[test]
    fn a_line_is_one_line_and_not_too_long() {
        assert_eq!(
            line("  two\n\nlines  here ").as_deref(),
            Some("two lines here")
        );
        assert!(line("   \n ").is_none());
        let long = line(&"word ".repeat(100)).unwrap();
        assert!(long.chars().count() <= 300, "{}", long.chars().count());
        assert!(long.ends_with("word…"));
        let one_word = line(&"x".repeat(400)).unwrap();
        assert!(one_word.chars().count() <= 300);
    }

    #[tokio::test]
    async fn a_genre_s_page_is_previewed_by_its_note_or_by_how_many_works_it_holds() {
        let state = crate::api::native::testing::server().await;
        for (title, genres) in [("One", vec!["Drama"]), ("Two", vec!["Drame", "Crime"])] {
            let mut work = MediaItem::empty(MediaKind::Movie);
            work.title = title.into();
            work.slug = crate::domain::make_slug(title, None);
            work.genres = genres.into_iter().map(String::from).collect();
            repo::item::upsert(
                &state.db,
                repo::item::ItemWrite {
                    item: &work,
                    replace_children: true,
                },
            )
            .await
            .unwrap();
        }
        crate::service::listing::relist_stale(&state.db, false, 100)
            .await
            .unwrap();

        // Asked for by its French name: the genre's page all the same.
        let preview = page_preview(&state, "genre", "Drame")
            .await
            .expect("previewed");
        assert_eq!(preview.title, "Drama");
        assert_eq!(preview.kind, "website");
        assert_eq!(
            preview.description.as_deref(),
            Some("2 works in the catalogue.")
        );

        repo::editorial::put(
            &state.db,
            Scope::Genre,
            "drama",
            "en",
            "The *quiet* ones, [all of them](/genre/drama).",
            None,
        )
        .await
        .unwrap();
        let preview = page_preview(&state, "genre", "drama")
            .await
            .expect("previewed");
        assert_eq!(
            preview.description.as_deref(),
            Some("The quiet ones, all of them.")
        );

        let decade = page_preview(&state, "decade", "1994")
            .await
            .expect("previewed");
        assert_eq!(decade.title, "The 1990s");
        assert!(page_preview(&state, "year", "nineteen").await.is_none());
        assert!(page_preview(&state, "network", "nobody").await.is_none());
        assert!(page_preview(&state, "genre", "isekai").await.is_none());
    }

    #[test]
    fn a_count_is_said_in_the_catalogue_s_language() {
        assert_eq!(counted(0, "en"), "Nothing in the catalogue yet.");
        assert_eq!(counted(1, "en"), "One work in the catalogue.");
        assert_eq!(counted(12, "fr"), "12 œuvres au catalogue.");
    }

    #[test]
    fn only_a_key_shaped_like_one_is_looked_up() {
        assert!(is_key("soirees-d-automne"));
        assert!(is_key("01a0d073-6ce8-760c-a2cb-113d4cf78503"));
        assert!(!is_key(""));
        assert!(!is_key("Soirées"));
        assert!(!is_key("a/b"));
        assert!(!is_key(&"a".repeat(129)));
    }
}
