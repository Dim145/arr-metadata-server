//! The web UI, compiled into the binary.
//!
//! One binary with no companion directory is the whole point: the container
//! image is a single static file. Vite's fingerprinted bundles are served
//! with immutable caching; everything else, the page first of all, is asked
//! about on every load.
//!
//! The page is the same for every route but one thing: a link to a work or
//! to a list carries, in its head, what a link preview shows of it — a title,
//! a line, a poster — for the readers that never run the app, and only while
//! a reader with no credential would be let in at all.

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::any,
};
use rust_embed::RustEmbed;

use crate::{
    config::SurfacePolicy,
    db::repo::{self, list::CuratedList},
    domain::{CoverType, MediaKind},
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
    "/api/", "/v1/", "/3/", "/4/", "/media/", "/health", "/ready",
];

/// Where the page keeps the lines a preview replaces.
const PREVIEW_START: &str = "<!-- preview -->";
const PREVIEW_END: &str = "<!-- /preview -->";

pub fn router() -> Router<AppState> {
    Router::new().fallback(any(serve))
}

async fn serve(State(state): State<AppState>, request: Request) -> Response {
    let path = request.uri().path();

    if API_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
        return not_found();
    }

    let trimmed = path.trim_start_matches('/');

    if let Some(response) = asset(trimmed) {
        return response;
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
    let cache = if path.starts_with("assets/") {
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

/// Whether a URI addresses the UI rather than the API.
#[allow(dead_code)]
pub fn is_ui_path(uri: &Uri) -> bool {
    !API_PREFIXES
        .iter()
        .any(|prefix| uri.path().starts_with(prefix))
}

// ─── link previews ───────────────────────────────────────────────────────────

/// What a link preview shows of a page: a title, a line, a picture.
struct Preview {
    title: String,
    description: Option<String>,
    image: Option<String>,
    /// Open Graph's word for it: `video.tv_show`, `video.movie`, `website`.
    kind: &'static str,
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
    let image = item
        .images
        .iter()
        .filter(|i| i.cover_type == CoverType::Poster && i.season_number.is_none())
        .min_by_key(|i| i.sort_order)
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
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"));
    Some(Preview {
        title,
        description: item.overview.as_deref().and_then(line),
        image,
        kind: match item.kind {
            MediaKind::Series => "video.tv_show",
            MediaKind::Movie => "video.movie",
        },
    })
}

async fn list_preview(state: &AppState, key: &str) -> Option<Preview> {
    let list: CuratedList = match repo::list::get(&state.db, key).await.ok().flatten() {
        Some(list) => list,
        None => repo::list::by_slug(&state.db, key).await.ok().flatten()?,
    };
    if !list.is_public {
        return None;
    }
    Some(Preview {
        title: list.name.clone(),
        description: list.description.as_deref().and_then(line),
        image: None,
        kind: "website",
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
            assert!(!is_ui_path(&path.parse::<Uri>().unwrap()), "{path}");
        }
    }

    #[test]
    fn everything_else_is_a_ui_path() {
        for path in ["/", "/catalogue", "/catalogue/abc", "/assets/index-abc.js"] {
            assert!(is_ui_path(&path.parse::<Uri>().unwrap()), "{path}");
        }
    }

    #[test]
    fn a_path_that_merely_starts_like_an_api_path_is_still_ui() {
        // "/v1abc" is not under "/v1/", so it belongs to the client router.
        assert!(is_ui_path(&"/v1abc".parse::<Uri>().unwrap()));
        assert!(is_ui_path(&"/apifoo".parse::<Uri>().unwrap()));
    }

    #[test]
    fn a_preview_takes_the_place_of_the_sites_own_lines() {
        let preview = Preview {
            title: "Tom & \"Jerry\" </title> (1940)".into(),
            description: Some("A cat, a mouse.".into()),
            image: Some("https://image.example/p.jpg".into()),
            kind: "video.movie",
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
