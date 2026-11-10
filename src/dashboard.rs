//! The dashboard's static export, served at `/dashboard/` by an open-mode server.
//!
//! `mise run build` builds the export (`dashboard/dist/dashboard`) before the
//! binary, and the binary embeds it, so a downloaded `sigiro` serves its UI with
//! no extra files. `SIGIRO_DASHBOARD_DIR` points at a build on disk instead (for
//! dashboard development); when it is set, the disk copy wins. A plain
//! `cargo build` without the export embeds nothing and serves no UI.

use std::path::PathBuf;

use axum::Router;
use rust_embed::RustEmbed;

use crate::static_site::{Site, Source};

#[derive(RustEmbed)]
#[folder = "dashboard/dist/dashboard/"]
#[allow_missing = true]
struct Embedded;

/// Next.js puts content-hashed build output here; those files never change.
const IMMUTABLE_PREFIX: &str = "_next/static/";

/// The dashboard to serve, if any. A server that requires auth serves none:
/// the hosted dashboard signs in on sigiro.com, and a self-hosted server with
/// auth has no sign-in page. `dir` is the directory that holds `dashboard/`.
pub(crate) fn select(requires_auth: bool, dir: Option<String>, embedded: bool) -> Option<Source> {
    if requires_auth {
        return None;
    }
    match dir {
        Some(dir) if !dir.trim().is_empty() => {
            Some(Source::Disk(PathBuf::from(dir).join("dashboard")))
        }
        _ if embedded => Some(Source::Embedded(Embedded::get)),
        _ => None,
    }
}

/// As [`select`], reading `SIGIRO_DASHBOARD_DIR` and checking whether this
/// binary embeds a build.
pub(crate) fn from_env(requires_auth: bool) -> Option<Source> {
    let dir = crate::config::DASHBOARD_DIR.value().ok();
    select(requires_auth, dir, is_embedded())
}

/// Whether this binary was built with the dashboard export.
pub fn is_embedded() -> bool {
    Embedded::get("index.html").is_some()
}

fn site(source: Source) -> Site {
    Site {
        source,
        immutable_prefix: IMMUTABLE_PREFIX,
        not_found_page: None,
    }
}

/// Add `/dashboard/` routes serving `source`.
pub(crate) fn routes<S>(router: Router<S>, source: Source) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    crate::static_site::routes(router, "/dashboard", site(source))
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
    #[folder = "tests/fixtures/dashboard/"]
    struct Fixture;

    fn fixture() -> Arc<Site> {
        Arc::new(site(Source::Embedded(Fixture::get)))
    }

    async fn get_path(
        source: Arc<Site>,
        path: &str,
        headers: HeaderMap,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let response = serve(source, path.to_string(), headers).await;
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map(|b| b.to_vec())
            .unwrap_or_default();
        (status, headers, body)
    }

    #[test]
    fn auth_servers_never_serve_the_dashboard() {
        assert!(select(true, Some("/tmp/x".into()), true).is_none());
        assert!(select(true, None, true).is_none());
    }

    #[test]
    fn a_dashboard_dir_wins_over_the_embedded_build() {
        assert!(matches!(
            select(false, Some("/tmp/x".into()), true),
            Some(Source::Disk(dir)) if dir == Path::new("/tmp/x/dashboard")
        ));
        assert!(matches!(
            select(false, None, true),
            Some(Source::Embedded(_))
        ));
        assert!(matches!(
            select(false, Some(" ".into()), true),
            Some(Source::Embedded(_))
        ));
        assert!(select(false, None, false).is_none());
    }

    #[tokio::test]
    async fn the_index_is_html_and_revalidated() {
        let (status, headers, body) = get_path(fixture(), "", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
        assert!(headers.contains_key(header::ETAG));
        assert!(String::from_utf8_lossy(&body).contains("fixture dashboard"));
    }

    #[tokio::test]
    async fn pages_resolve_with_and_without_a_trailing_slash() {
        for path in ["profiles/", "profiles"] {
            let (status, _, body) = get_path(fixture(), path, HeaderMap::new()).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(
                String::from_utf8_lossy(&body).contains("fixture profiles"),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn hashed_assets_are_immutable() {
        let (status, headers, body) = get_path(
            fixture(),
            "_next/static/chunks/app-0a1b2c.js",
            HeaderMap::new(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            headers[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn a_matching_etag_is_not_modified() {
        let (_, headers, _) = get_path(fixture(), "", HeaderMap::new()).await;
        let mut request = HeaderMap::new();
        request.insert(header::IF_NONE_MATCH, headers[header::ETAG].clone());
        let (status, headers, body) = get_path(fixture(), "", request).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert!(body.is_empty());
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    }

    #[tokio::test]
    async fn missing_files_and_escapes_are_not_found() {
        for path in ["nope.js", "../Cargo.toml", "_next/../../Cargo.toml"] {
            let (status, _, _) = get_path(fixture(), path, HeaderMap::new()).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn the_disk_override_serves_the_same_layout() {
        let dir =
            std::env::temp_dir().join(format!("sigiro-dashboard-test-{}", std::process::id()));
        let pages = dir.join("dashboard/profiles");
        std::fs::create_dir_all(&pages).expect("create fixture dir");
        std::fs::write(dir.join("dashboard/index.html"), "<p>disk index</p>").expect("write index");
        std::fs::write(pages.join("index.html"), "<p>disk profiles</p>").expect("write page");
        let source = Arc::new(site(Source::Disk(dir.join("dashboard"))));
        let (status, headers, body) = get_path(source.clone(), "profiles", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(body, b"<p>disk profiles</p>");
        let (status, _, _) = get_path(source, "../secret", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(dir).expect("remove fixture dir");
    }
}
