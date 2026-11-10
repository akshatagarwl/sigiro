//! The docs, served at `/docs/`: the same pages as sigiro.com/docs, in every
//! locale, with their search index, so a downloaded `sigiro` carries its manual
//! and reads it offline.
//!
//! `mise run build` builds the export (`www/dist-binary`, see
//! `scripts/build-docs-export.sh`) before the binary, and the binary embeds it.
//! `SIGIRO_DOCS_DIR` points at an export on disk instead; when it is set, the
//! disk copy wins. A plain `cargo build` without the export embeds nothing and
//! serves no docs.
//!
//! Unlike the dashboard, the docs are served whatever the auth mode: they are
//! public, hold no tenant data, and need no sign-in.

use std::path::PathBuf;

use axum::Router;
use rust_embed::RustEmbed;

use crate::static_site::{Site, Source};

#[derive(RustEmbed)]
#[folder = "www/dist-binary/"]
#[allow_missing = true]
struct Embedded;

/// Astro puts content-hashed build output here; those files never change.
const IMMUTABLE_PREFIX: &str = "_astro/";

/// The docs to serve, if any. `dir` is the export root (the directory holding
/// `index.html`).
pub(crate) fn select(dir: Option<String>, embedded: bool) -> Option<Source> {
    match dir {
        Some(dir) if !dir.trim().is_empty() => Some(Source::Disk(PathBuf::from(dir))),
        _ if embedded => Some(Source::Embedded(Embedded::get)),
        _ => None,
    }
}

/// As [`select`], reading `SIGIRO_DOCS_DIR` and checking whether this binary
/// embeds an export.
pub(crate) fn from_env() -> Option<Source> {
    let dir = crate::config::DOCS_DIR.value().ok();
    select(dir, is_embedded())
}

/// Whether this binary was built with the docs export.
pub fn is_embedded() -> bool {
    Embedded::get("index.html").is_some()
}

fn site(source: Source) -> Site {
    Site {
        source,
        immutable_prefix: IMMUTABLE_PREFIX,
        // Blume's own "page not found" page, in the docs shell, with search.
        not_found_page: Some("404.html"),
    }
}

/// Add `/docs/` routes serving `source`.
pub(crate) fn routes<S>(router: Router<S>, source: Source) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    crate::static_site::routes(router, "/docs", site(source))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use axum::http::{HeaderMap, StatusCode, header};

    use super::*;
    use crate::static_site::serve;

    /// A tiny export with the same layout as the real one.
    #[derive(RustEmbed)]
    #[folder = "tests/fixtures/docs/"]
    struct Fixture;

    async fn get_path(path: &str) -> (StatusCode, HeaderMap, String) {
        let site = Arc::new(site(Source::Embedded(Fixture::get)));
        let response = serve(site, path.to_string(), HeaderMap::new()).await;
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        (status, headers, body)
    }

    #[test]
    fn a_docs_dir_wins_over_the_embedded_export() {
        assert!(matches!(
            select(Some("/tmp/d".into()), true),
            Some(Source::Disk(dir)) if dir == Path::new("/tmp/d")
        ));
        assert!(matches!(select(None, true), Some(Source::Embedded(_))));
        assert!(matches!(
            select(Some(" ".into()), true),
            Some(Source::Embedded(_))
        ));
        assert!(select(None, false).is_none());
    }

    #[tokio::test]
    async fn pages_and_locales_resolve_without_a_trailing_slash() {
        for (path, text) in [
            ("", "fixture docs"),
            ("how-to/executor", "fixture executor"),
            ("ja", "fixture ja"),
        ] {
            let (status, headers, body) = get_path(path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
            assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
            assert!(body.contains(text), "{path}: {body}");
        }
    }

    #[tokio::test]
    async fn astro_assets_are_immutable() {
        let (status, headers, _) = get_path("_astro/page.abc123.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn unknown_pages_get_the_docs_404_page() {
        let (status, headers, body) = get_path("no/such/page").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
        assert!(!headers.contains_key(header::ETAG));
        assert!(body.contains("fixture not found"));
    }

    #[tokio::test]
    async fn escapes_are_a_bare_404() {
        let (status, _, body) = get_path("../Cargo.toml").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.is_empty());
    }
}
