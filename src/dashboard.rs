//! The dashboard's static export, served at `/dashboard/` by an open-mode server.
//!
//! `mise run build` builds the export (`dashboard/dist/dashboard`) before the
//! binary, and the binary embeds it, so a downloaded `sigiro` serves its UI with
//! no extra files. `SIGIRO_DASHBOARD_DIR` points at a build on disk instead (for
//! dashboard development); when it is set, the disk copy wins. A plain
//! `cargo build` without the export embeds nothing and serves no UI.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use rust_embed::{EmbeddedFile, RustEmbed};

#[derive(RustEmbed)]
#[folder = "dashboard/dist/dashboard/"]
#[allow_missing = true]
struct Embedded;

/// Next.js puts content-hashed build output here; those files never change.
const IMMUTABLE_PREFIX: &str = "_next/static/";

/// Where the dashboard's files come from.
#[derive(Clone)]
pub(crate) enum Source {
    /// A static export on disk: the directory that holds `dashboard/`.
    Disk(PathBuf),
    /// Files compiled into the binary.
    Embedded(fn(&str) -> Option<EmbeddedFile>),
}

impl Source {
    /// The dashboard to serve, if any. A server that requires auth serves none:
    /// the hosted dashboard signs in on sigiro.com, and a self-hosted server
    /// with auth has no sign-in page.
    pub(crate) fn select(requires_auth: bool, dir: Option<String>, embedded: bool) -> Option<Self> {
        if requires_auth {
            return None;
        }
        match dir {
            Some(dir) if !dir.trim().is_empty() => Some(Self::Disk(PathBuf::from(dir))),
            _ if embedded => Some(Self::Embedded(Embedded::get)),
            _ => None,
        }
    }

    /// As [`Source::select`], reading `SIGIRO_DASHBOARD_DIR` and checking
    /// whether this binary embeds a build.
    pub(crate) fn from_env(requires_auth: bool) -> Option<Self> {
        let dir = crate::config::DASHBOARD_DIR.value().ok();
        Self::select(requires_auth, dir, is_embedded())
    }

    fn describe(&self) -> String {
        match self {
            Self::Disk(dir) => dir.display().to_string(),
            Self::Embedded(_) => "embedded".to_string(),
        }
    }
}

/// Whether this binary was built with the dashboard export.
pub fn is_embedded() -> bool {
    Embedded::get("index.html").is_some()
}

/// Add `/dashboard/` routes serving `source`.
pub(crate) fn routes<S>(router: Router<S>, source: Source) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    tracing::info!(source = %source.describe(), "serving the dashboard at /dashboard/");
    let source = Arc::new(source);
    router
        .route(
            "/dashboard",
            get(|| async { Redirect::permanent("/dashboard/") }),
        )
        .route("/dashboard/", get(index).with_state(source.clone()))
        .route("/dashboard/{*path}", get(file).with_state(source))
}

async fn index(State(source): State<Arc<Source>>, headers: HeaderMap) -> Response {
    serve(source, String::new(), headers).await
}

async fn file(
    State(source): State<Arc<Source>>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    serve(source, path, headers).await
}

/// The file a request path names, relative to the export root. Paths that are
/// empty or end in `/` name that directory's `index.html` (the export uses
/// trailing slashes). `None` for `..`, absolute, or other non-plain components.
fn relative_file(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if path.is_empty() || path.ends_with('/') {
        parts.push("index.html");
    }
    Some(parts.join("/"))
}

struct Found {
    name: String,
    bytes: Cow<'static, [u8]>,
    etag: Option<String>,
}

async fn load(source: &Source, relative: &str) -> Option<Found> {
    let index = format!("{relative}/index.html");
    match source {
        Source::Disk(root) => {
            let root = root.join("dashboard");
            for name in [relative, index.as_str()] {
                let file = root.join(name);
                if file.is_file()
                    && let Ok(bytes) = tokio::fs::read(&file).await
                {
                    return Some(Found {
                        name: name.to_string(),
                        bytes: Cow::Owned(bytes),
                        etag: None,
                    });
                }
            }
            None
        }
        Source::Embedded(get_file) => [relative, index.as_str()].into_iter().find_map(|name| {
            get_file(name).map(|file| Found {
                name: name.to_string(),
                etag: Some(format!("\"{}\"", hex::encode(file.metadata.sha256_hash()))),
                bytes: file.data,
            })
        }),
    }
}

fn content_type(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

/// Serve one file of the dashboard export.
async fn serve(source: Arc<Source>, path: String, headers: HeaderMap) -> Response {
    let Some(relative) = relative_file(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(found) = load(&source, &relative).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let cache = if found.name.starts_with(IMMUTABLE_PREFIX) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = match &found.etag {
        Some(etag)
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*")) =>
        {
            StatusCode::NOT_MODIFIED.into_response()
        }
        _ => (
            [(header::CONTENT_TYPE, content_type(&found.name))],
            axum::body::Body::from(found.bytes),
        )
            .into_response(),
    };
    let response_headers = response.headers_mut();
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Some(etag) = found.etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        response_headers.insert(header::ETAG, etag);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny export with the same layout as the real one.
    #[derive(RustEmbed)]
    #[folder = "tests/fixtures/dashboard/"]
    struct Fixture;

    fn fixture() -> Arc<Source> {
        Arc::new(Source::Embedded(Fixture::get))
    }

    async fn get_path(
        source: Arc<Source>,
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
        assert!(Source::select(true, Some("/tmp/x".into()), true).is_none());
        assert!(Source::select(true, None, true).is_none());
    }

    #[test]
    fn a_dashboard_dir_wins_over_the_embedded_build() {
        assert!(matches!(
            Source::select(false, Some("/tmp/x".into()), true),
            Some(Source::Disk(dir)) if dir == Path::new("/tmp/x")
        ));
        assert!(matches!(
            Source::select(false, None, true),
            Some(Source::Embedded(_))
        ));
        assert!(matches!(
            Source::select(false, Some(" ".into()), true),
            Some(Source::Embedded(_))
        ));
        assert!(Source::select(false, None, false).is_none());
    }

    #[test]
    fn request_paths_map_to_files() {
        assert_eq!(relative_file("").as_deref(), Some("index.html"));
        assert_eq!(
            relative_file("profiles/").as_deref(),
            Some("profiles/index.html")
        );
        assert_eq!(relative_file("./a/b.js").as_deref(), Some("a/b.js"));
        assert_eq!(relative_file("../secret"), None);
        assert_eq!(relative_file("a/../../secret"), None);
        assert_eq!(relative_file("/etc/passwd"), None);
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
        let source = Arc::new(Source::Disk(dir.clone()));
        let (status, headers, body) = get_path(source.clone(), "profiles", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(body, b"<p>disk profiles</p>");
        let (status, _, _) = get_path(source, "../secret", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(dir).expect("remove fixture dir");
    }
}
