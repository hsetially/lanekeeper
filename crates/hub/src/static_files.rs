//! The built web app (`web/dist`), served by the hub itself (T1, S12).
//!
//! Served with a small handler of our own, not a general static-file server, because three rules matter more than features:
//!
//! - **`index.html` gets the response's CSP nonce.** The build leaves the placeholder `__CSP_NONCE__` in every `nonce`
//!   attribute; each response replaces it with that response's own nonce, so the CSP can say `'nonce-…'` and never
//!   `'unsafe-inline'`. (Prompt 08's build must emit the placeholder; recorded in the plan, A14.)
//! - **Paths are untrusted (rule 10).** A path segment is letters, digits, `.`, `_` and `-` and nothing else, never `.` or
//!   `..`; a `%` is refused rather than decoded; the resolved file must be a regular file inside the root, and a symlink
//!   anywhere on the way is a 404.
//! - **Hashed assets are immutable, everything else is not.** Files under `assets/` have a content hash in their name (Vite's
//!   default) and get `immutable`; `index.html` and the rest get `no-store`, set by the headers layer.

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, Method, header};
use hub_platform::ApiError;
use std::sync::Arc;

use crate::headers::CspNonce;

/// The placeholder in `index.html` that becomes the nonce.
pub const NONCE_PLACEHOLDER: &str = "__CSP_NONCE__";

/// The largest file served. A bundle chunk is a few MiB; anything bigger is a mistake in the build, not content.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Why the web app could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StaticError {
    #[error("the web root does not exist or is not a directory")]
    NoRoot,
    #[error("the web root has no readable index.html")]
    NoIndex,
    #[error("index.html does not contain the nonce placeholder")]
    NoPlaceholder,
}

/// The web app, opened once at start-up.
#[derive(Debug)]
pub struct StaticFiles {
    root: PathBuf,
    index: String,
}

impl StaticFiles {
    /// Open `root`. Reads `index.html` once (a blocking read, before any listener is open) and checks it has the nonce
    /// placeholder, so a build that forgot it fails at start-up and not as a blank page.
    pub fn open(root: &Path) -> Result<Self, StaticError> {
        let root = std::fs::canonicalize(root).map_err(|_| StaticError::NoRoot)?;
        if !root.is_dir() {
            return Err(StaticError::NoRoot);
        }
        let index = std::fs::read_to_string(root.join("index.html")).map_err(|_| StaticError::NoIndex)?;
        if !index.contains(NONCE_PLACEHOLDER) {
            return Err(StaticError::NoPlaceholder);
        }
        Ok(Self { root, index })
    }

    fn index_response(&self, nonce: &str, head_only: bool) -> Response {
        let body = self.index.replace(NONCE_PLACEHOLDER, nonce);
        file_response("text/html; charset=utf-8", body.into_bytes(), false, head_only)
    }
}

/// A path split into safe segments, or `None` if any segment is not.
fn safe_segments(path: &str) -> Option<Vec<&str>> {
    let trimmed = path.strip_prefix('/')?;
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for segment in trimmed.split('/') {
        let ok = !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment.len() <= 128
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !ok {
            return None;
        }
        out.push(segment);
    }
    Some(out)
}

fn content_type(name: &str) -> &'static str {
    match name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn file_response(content_type: &'static str, bytes: Vec<u8>, immutable: bool, head_only: bool) -> Response {
    let length = bytes.len();
    let mut response = Response::new(if head_only {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    if immutable {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    response
}

/// The router's fallback: files, then the single-page app's `index.html` for client-side routes.
pub async fn serve(State(files): State<Arc<StaticFiles>>, request: Request) -> Response {
    let head_only = request.method() == Method::HEAD;
    if request.method() != Method::GET && !head_only {
        let mut response = ApiError::MethodNotAllowed.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    let path = request.uri().path();
    // API and webhook paths that no route matched are errors, never the app.
    if path.starts_with("/api/") || path.starts_with("/hooks/") || path == "/api" || path == "/hooks" {
        return ApiError::NotFound.into_response();
    }
    let Some(segments) = safe_segments(path) else {
        return ApiError::NotFound.into_response();
    };
    let nonce = request.extensions().get::<CspNonce>().map(|n| n.0.clone());
    let index = |nonce: Option<String>| -> Response {
        if let Some(nonce) = nonce {
            files.index_response(&nonce, head_only)
        } else {
            tracing::error!("index.html requested without a CSP nonce in the request");
            ApiError::Internal.into_response()
        }
    };
    if segments.is_empty() || segments == ["index.html"] {
        return index(nonce);
    }
    let last = segments.last().copied().unwrap_or_default();
    let candidate = segments.iter().fold(files.root.clone(), |p, s| p.join(s));
    match read_file(&files.root, &candidate).await {
        Some(bytes) => {
            let immutable = segments.first() == Some(&"assets");
            file_response(content_type(last), bytes, immutable, head_only)
        }
        // A client-side route (no extension in the last segment) is the app; a missing file with an extension is a 404.
        None if !last.contains('.') => index(nonce),
        None => ApiError::NotFound.into_response(),
    }
}

/// Read a regular file inside `root`, or `None`. Symlinks are refused at every component.
async fn read_file(root: &Path, candidate: &Path) -> Option<Vec<u8>> {
    // Every component under the root must be a plain name; this is what `safe_segments` already guarantees, and it is
    // checked again here because this function is the one that touches the file system.
    let relative = candidate.strip_prefix(root).ok()?;
    if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let mut walked = root.to_path_buf();
    for component in relative.components() {
        walked.push(component);
        let meta = tokio::fs::symlink_metadata(&walked).await.ok()?;
        if meta.file_type().is_symlink() {
            return None;
        }
    }
    let meta = tokio::fs::metadata(candidate).await.ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    tokio::fs::read(candidate).await.ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn segments_allow_plain_names_only() {
        assert_eq!(safe_segments("/"), Some(vec![]));
        assert_eq!(
            safe_segments("/assets/index-a1b2.js"),
            Some(vec!["assets", "index-a1b2.js"])
        );
        for bad in [
            "/../etc/passwd",
            "/assets/../x",
            "/a//b",
            "/a/./b",
            "/a%2e%2e/b",
            "/a\\b",
            "/a b",
            "/a\0b",
            "/\u{e9}",
            "relative",
        ] {
            assert_eq!(safe_segments(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn content_types_are_by_extension() {
        assert_eq!(content_type("a.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("A.CSS"), "text/css; charset=utf-8");
        assert_eq!(content_type("x.unknown"), "application/octet-stream");
        assert_eq!(content_type("noext"), "application/octet-stream");
    }

    #[test]
    fn a_build_without_the_placeholder_does_not_open() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        assert_eq!(
            StaticFiles::open(dir.path()).unwrap_err(),
            StaticError::NoPlaceholder
        );
    }

    #[test]
    fn a_missing_root_or_index_does_not_open() {
        assert_eq!(
            StaticFiles::open(Path::new("/definitely/not/here")).unwrap_err(),
            StaticError::NoRoot
        );
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(StaticFiles::open(dir.path()).unwrap_err(), StaticError::NoIndex);
    }
}
