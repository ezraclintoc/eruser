//! Static files, embedded in the binary.
//!
//! Go served these from an `embed.FS` too, but pointed the templates at
//! CDNs anyway. Here the embedded copies are the only copies.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "static/"]
pub struct StaticFiles;

/// How long a browser may cache an asset.
///
/// The filenames are not content-hashed, so this stays short enough that a
/// rebuilt stylesheet shows up without a hard refresh.
const CACHE_CONTROL: &str = "public, max-age=3600";

/// Serve one embedded file.
pub async fn serve(Path(path): Path<String>) -> Response {
    // rust-embed resolves paths against the embedded set, so `..` cannot
    // escape it, but rejecting the attempt outright is clearer than relying
    // on that.
    if path.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(file) = StaticFiles::get(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime = mime_guess::from_path(&path).first_or_octet_stream();

    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, CACHE_CONTROL),
        ],
        file.data,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheet_and_htmx_are_embedded() {
        assert!(StaticFiles::get("css/app.css").is_some());
        assert!(StaticFiles::get("js/htmx.min.js").is_some());
    }

    #[test]
    fn the_webfonts_are_embedded() {
        let fonts: Vec<_> = StaticFiles::iter()
            .filter(|path| path.ends_with(".woff2"))
            .collect();
        assert!(
            fonts.len() >= 2,
            "expected the vendored webfonts, found {fonts:?}"
        );
    }

    /// The whole point of vendoring: no page load should reach a third party.
    #[test]
    fn the_stylesheet_does_not_reference_a_remote_host() {
        let css = StaticFiles::get("css/app.css").unwrap();
        let text = std::str::from_utf8(&css.data).unwrap();

        assert!(
            !text.contains("fonts.gstatic.com"),
            "the fonts are not local"
        );
        assert!(
            !text.contains("//cdn."),
            "something is still loaded from a CDN"
        );
        assert!(
            text.contains("/static/fonts/"),
            "font paths should be local"
        );
    }

    /// A missing utility class renders as no styling at all, so check a few
    /// the templates rely on made it into the generated build.
    #[test]
    fn the_generated_stylesheet_covers_classes_the_templates_use() {
        let css = StaticFiles::get("css/app.css").unwrap();
        let text = std::str::from_utf8(&css.data).unwrap();

        for class in [".text-accent", ".bg-accent", ".flex", ".rounded-full"] {
            assert!(
                text.contains(class),
                "{class} is missing from the stylesheet"
            );
        }
    }

    #[tokio::test]
    async fn a_known_file_is_served_with_its_content_type() {
        let response = serve(Path("css/app.css".to_string())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/css"
        );
    }

    #[tokio::test]
    async fn an_unknown_file_is_not_found() {
        let response = serve(Path("css/nothing.css".to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_traversal_attempt_is_refused() {
        let response = serve(Path("../../etc/passwd".to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
