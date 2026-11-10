//! Serving a static site export under a path prefix: the dashboard at
//! `/dashboard/` (src/dashboard.rs) and the docs at `/docs/` (src/docs.rs).
//!
//! Both are prerendered builds with trailing-slash directories, content-hashed
//! assets under one folder, and an optional copy on disk that replaces the
//! embedded one during development. This module is that shared shape; each
//! caller decides whether its site is served at all.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use rust_embed::EmbeddedFile;

/// Where a site's files come from.
#[derive(Clone)]
pub(crate) enum Source {
    /// A static export on disk: the directory whose `index.html` is the site root.
    Disk(PathBuf),
    /// Files compiled into the binary.
    Embedded(fn(&str) -> Option<EmbeddedFile>),
}

impl Source {
    fn describe(&self) -> String {
        match self {
            Self::Disk(dir) => dir.display().to_string(),
            Self::Embedded(_) => "embedded".to_string(),
        }
    }
}

/// One static site and how to serve it.
#[derive(Clone)]
pub(crate) struct Site {
    pub(crate) source: Source,
    /// Content-hashed build output (`_next/static/`, `_astro/`); never changes,
    /// so it is cached for a year.
    pub(crate) immutable_prefix: &'static str,
    /// A page to answer unknown paths with, as a 404. `None` sends an empty 404.
    pub(crate) not_found_page: Option<&'static str>,
}

/// Add `{mount}`, `{mount}/` and `{mount}/{*path}` routes serving `site`.
/// `mount` is a path like `/docs`, without a trailing slash.
pub(crate) fn routes<S>(router: Router<S>, mount: &'static str, site: Site) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    tracing::info!(source = %site.source.describe(), "serving {mount}/");
    let site = Arc::new(site);
    let slash = format!("{mount}/");
    let redirect_to = slash.clone();
    router
        .route(
            mount,
            get(move || {
                let to = redirect_to.clone();
                async move { Redirect::permanent(&to) }
            }),
        )
        .route(&slash, get(index).with_state(site.clone()))
        .route(&format!("{mount}/{{*path}}"), get(file).with_state(site))
}

async fn index(State(site): State<Arc<Site>>, headers: HeaderMap) -> Response {
    serve(site, String::new(), headers).await
}

async fn file(
    State(site): State<Arc<Site>>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    serve(site, path, headers).await
}

/// The file a request path names, relative to the export root. Paths that are
/// empty or end in `/` name that directory's `index.html` (the exports use
/// trailing slashes). `None` for `..`, absolute, or other non-plain components.
pub(crate) fn relative_file(path: &str) -> Option<String> {
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
        // The docs publish a raw-Markdown twin of every page (`/docs/x.md`).
        // Without a charset a browser decodes the Japanese, Korean and Chinese
        // pages as Windows-1252.
        Some("md" | "mdx") => "text/markdown; charset=utf-8",
        Some("xml") => "application/xml",
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

/// Serve one file of the export.
pub(crate) async fn serve(site: Arc<Site>, path: String, headers: HeaderMap) -> Response {
    let Some(relative) = relative_file(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (status, found) = match load(&site.source, &relative).await {
        Some(found) => (StatusCode::OK, found),
        None => match site.not_found_page {
            Some(page) => match load(&site.source, page).await {
                Some(found) => (StatusCode::NOT_FOUND, found),
                None => return StatusCode::NOT_FOUND.into_response(),
            },
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let cache = if status == StatusCode::OK && found.name.starts_with(site.immutable_prefix) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let etag = found.etag.filter(|_| status == StatusCode::OK);
    let mut response = match &etag {
        Some(etag)
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*")) =>
        {
            StatusCode::NOT_MODIFIED.into_response()
        }
        _ => (
            status,
            [(header::CONTENT_TYPE, content_type(&found.name))],
            axum::body::Body::from(found.bytes),
        )
            .into_response(),
    };
    let response_headers = response.headers_mut();
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Some(etag) = etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        response_headers.insert(header::ETAG, etag);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn markdown_twins_declare_utf8() {
        assert_eq!(
            content_type("ja/reference.md"),
            "text/markdown; charset=utf-8"
        );
        assert_eq!(content_type("llms.txt"), "text/plain; charset=utf-8");
        assert_eq!(content_type("blume-search.json"), "application/json");
    }
}
