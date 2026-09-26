//! The web client (single-page app built from `web/` into `web/dist`),
//! embedded at compile time by `build.rs` and served at `/`.
//!
//! * Files are served with their content type; hashed build outputs under
//!   `/assets/` are cached for a year (`immutable`), everything else —
//!   `index.html` first — is revalidated on every use (`no-cache` + ETag).
//! * SPA fallback: a `GET` of any other path outside `/api`, `/hooks` and
//!   `/healthz` that isn't a file gets `index.html`, so client-side routes
//!   (`/services/web/deploys`) survive reloads and deep links. Missing
//!   `/assets/` files stay 404s (a stale chunk must not be answered with HTML).
//! * `FERRY_UI_DIR=<dir>` serves a built client from disk instead (for
//!   development: rebuild the client without recompiling the server).

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};

use crate::error::ApiError;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
}

/// An embedded file: `(path, ETag, bytes)`.
pub type EmbeddedFile = (&'static str, &'static str, &'static [u8]);

/// True when the server was compiled without a built web client (it then
/// serves a page explaining how to build it).
pub fn is_placeholder() -> bool {
    embedded::PLACEHOLDER
}

/// The embedded file at `path` (relative, e.g. `index.html`), if any.
pub fn embedded_file(path: &str) -> Option<&'static [u8]> {
    lookup(embedded::FILES, path).map(|(_, _, bytes)| *bytes)
}

/// Paths of every embedded file.
pub fn embedded_paths() -> impl Iterator<Item = &'static str> {
    embedded::FILES.iter().map(|(p, _, _)| *p)
}

fn lookup(files: &'static [EmbeddedFile], path: &str) -> Option<&'static EmbeddedFile> {
    files.binary_search_by(|(p, _, _)| (*p).cmp(path)).ok().map(|i| &files[i])
}

/// Where the client's files come from.
#[derive(Debug, Clone)]
pub enum Ui {
    /// Compiled into the binary.
    Embedded(&'static [EmbeddedFile]),
    /// A directory on disk (`FERRY_UI_DIR`).
    Dir(PathBuf),
}

impl Ui {
    /// `FERRY_UI_DIR` when set, else the embedded client.
    pub fn from_env() -> Self {
        match std::env::var_os("FERRY_UI_DIR").filter(|v| !v.is_empty()) {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                tracing::info!(dir = %dir.display(), "serving the web client from FERRY_UI_DIR");
                Ui::Dir(dir)
            }
            None => {
                if is_placeholder() {
                    tracing::warn!(
                        "this server was built without the web client: `/` shows how to build it (cd web && npm ci && npm run build)"
                    );
                }
                Ui::Embedded(embedded::FILES)
            }
        }
    }

    /// Handle a request that no API / hook route matched.
    pub async fn serve(&self, method: &Method, uri: &Uri, headers: &HeaderMap) -> Response {
        let path = uri.path();
        if is_reserved(path) {
            return ApiError::not_found(format!("{path} not found")).into_response();
        }
        if method != Method::GET && method != Method::HEAD {
            return ApiError::method_not_allowed().into_response();
        }
        let Some(rel) = relative_path(path) else {
            return ApiError::not_found(format!("{path} not found")).into_response();
        };
        let resp = match self.file(&rel).await {
            Some(file) => file.into_response(&rel, headers),
            None if rel.starts_with("assets/") => ApiError::not_found(format!("{path} not found")).into_response(),
            None => match self.file("index.html").await {
                Some(index) => index.into_response("index.html", headers),
                None => ApiError::not_found("the web client has no index.html").into_response(),
            },
        };
        if method == Method::HEAD { without_body(resp) } else { resp }
    }

    async fn file(&self, rel: &str) -> Option<File> {
        match self {
            Ui::Embedded(files) => embedded_entry(files, rel),
            Ui::Dir(dir) => {
                let path = dir.join(rel);
                if !matches!(tokio::fs::metadata(&path).await, Ok(meta) if meta.is_file()) {
                    // A directory without its own index.html gets the embedded one.
                    return if rel == "index.html" { embedded_entry(embedded::FILES, rel) } else { None };
                }
                match tokio::fs::read(&path).await {
                    Ok(bytes) => Some(File { len: bytes.len(), bytes: Body::from(bytes), etag: None }),
                    Err(e) => {
                        tracing::warn!(path = %path.display(), "reading a web client file failed: {e}");
                        None
                    }
                }
            }
        }
    }
}

fn embedded_entry(files: &'static [EmbeddedFile], rel: &str) -> Option<File> {
    lookup(files, rel).map(|(_, etag, bytes)| File { bytes: Body::from(*bytes), len: bytes.len(), etag: Some(etag) })
}

/// Paths that belong to the API, webhooks or liveness: unknown ones there
/// stay JSON 404s instead of becoming the SPA.
fn is_reserved(path: &str) -> bool {
    ["/api", "/hooks", "/healthz"]
        .iter()
        .any(|p| path == *p || path.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
}

/// The file a request path names, relative to the client's root: decoded,
/// `/` → `index.html`, and never escaping the root (`None`).
fn relative_path(path: &str) -> Option<String> {
    let decoded = percent_decode(path)?;
    let trimmed = decoded.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some("index.html".to_string());
    }
    if trimmed.contains('\\') || trimmed.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for c in Path::new(trimmed).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_string()),
            Component::CurDir => {}
            // `..`, absolute or prefixed components.
            _ => return None,
        }
    }
    if parts.is_empty() {
        return Some("index.html".to_string());
    }
    Some(parts.join("/"))
}

/// Decode `%XX` escapes (`+` stays a plus: this is a path, not a form).
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

struct File {
    bytes: Body,
    len: usize,
    etag: Option<&'static str>,
}

impl File {
    fn into_response(self, rel: &str, req_headers: &HeaderMap) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(rel)));
        let cache = if rel.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        if rel.ends_with(".html") {
            headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
            headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        }
        if let Some(etag) = self.etag {
            headers.insert(header::ETAG, HeaderValue::from_static(etag));
            let matches = req_headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"));
            if matches {
                return (StatusCode::NOT_MODIFIED, headers).into_response();
            }
        }
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(self.len));
        (headers, self.bytes).into_response()
    }
}

/// HEAD: the GET response's headers without its body.
fn without_body(resp: Response) -> Response {
    let (parts, _) = resp.into_parts();
    Response::from_parts(parts, Body::empty())
}

/// The `Content-Type` of a file, by extension.
pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    static FILES: &[EmbeddedFile] = &[
        ("assets/app-1a2b3c.js", "\"js\"", b"console.log(1)"),
        ("favicon.svg", "\"svg\"", b"<svg/>"),
        ("index.html", "\"idx\"", b"<!doctype html><title>t</title>"),
    ];

    async fn get(ui: &Ui, method: Method, uri: &str, headers: HeaderMap) -> (StatusCode, HeaderMap, Vec<u8>) {
        let resp = ui.serve(&method, &uri.parse().unwrap(), &headers).await;
        let (parts, body) = resp.into_parts();
        (parts.status, parts.headers, body.collect().await.unwrap().to_bytes().to_vec())
    }

    #[test]
    fn the_embedded_table_has_an_index_and_is_sorted() {
        assert!(embedded_file("index.html").is_some());
        assert!(embedded::FILES.windows(2).all(|w| w[0].0 < w[1].0));
        let index = String::from_utf8_lossy(embedded_file("index.html").unwrap()).to_ascii_lowercase();
        assert!(index.contains("<html"), "{index}");
        assert_eq!(is_placeholder(), index.contains(r#"name="ferry-ui" content="placeholder""#));
    }

    #[test]
    fn paths_stay_inside_the_root() {
        assert_eq!(relative_path("/").as_deref(), Some("index.html"));
        assert_eq!(relative_path("/assets/a.js").as_deref(), Some("assets/a.js"));
        assert_eq!(relative_path("/./x/./y.css").as_deref(), Some("x/y.css"));
        assert_eq!(relative_path("/a%20b.txt").as_deref(), Some("a b.txt"));
        assert_eq!(relative_path("/a+b").as_deref(), Some("a+b"));
        for bad in ["/../etc/passwd", "/assets/../../x", "/%2e%2e/x", "/a\\b", "/a%00", "/%zz", "/%c3"] {
            assert_eq!(relative_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("assets/x.JS"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("assets/x.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("assets/f.woff2"), "font/woff2");
        assert_eq!(content_type("logo.svg"), "image/svg+xml");
        assert_eq!(content_type("noext"), "application/octet-stream");
    }

    #[tokio::test]
    async fn files_caching_and_spa_fallback() {
        let ui = Ui::Embedded(FILES);
        let (status, h, body) = get(&ui, Method::GET, "/", HeaderMap::new()).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"<!doctype html><title>t</title>"[..]));
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(h[header::CACHE_CONTROL], "no-cache");
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(h[header::ETAG], "\"idx\"");

        let (status, h, body) = get(&ui, Method::GET, "/assets/app-1a2b3c.js", HeaderMap::new()).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"console.log(1)"[..]));
        assert_eq!(h[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(h[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

        let (_, h, _) = get(&ui, Method::GET, "/favicon.svg", HeaderMap::new()).await;
        assert_eq!(
            (h[header::CONTENT_TYPE].to_str().unwrap(), h[header::CACHE_CONTROL].to_str().unwrap()),
            ("image/svg+xml", "no-cache")
        );

        // client-side routes get index.html
        for route in ["/services", "/services/web/deploys/dep-123", "/index.html", "/login?next=%2Fservices"] {
            let (status, h, body) = get(&ui, Method::GET, route, HeaderMap::new()).await;
            assert_eq!(status, StatusCode::OK, "{route}");
            assert_eq!(h[header::CACHE_CONTROL], "no-cache", "{route}");
            assert!(body.starts_with(b"<!doctype html>"), "{route}");
        }
        // ...but a missing hashed asset is a 404, and reserved prefixes stay JSON 404s
        for path in ["/assets/app-old.js", "/api/v1/nope", "/api", "/hooks/nope", "/healthz", "/../x"] {
            let (status, h, _) = get(&ui, Method::GET, path, HeaderMap::new()).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(h[header::CONTENT_TYPE], "application/json", "{path}");
        }
        // other methods
        let (status, _, _) = get(&ui, Method::POST, "/services", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        // HEAD: headers only
        let (status, h, body) = get(&ui, Method::HEAD, "/assets/app-1a2b3c.js", HeaderMap::new()).await;
        assert_eq!((status, body.len()), (StatusCode::OK, 0));
        assert_eq!(h[header::CONTENT_LENGTH], "14");
        // revalidation
        let mut cond = HeaderMap::new();
        cond.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"other\", \"idx\""));
        let (status, h, body) = get(&ui, Method::GET, "/services", cond).await;
        assert_eq!((status, body.len()), (StatusCode::NOT_MODIFIED, 0));
        assert_eq!(h[header::ETAG], "\"idx\"");
    }

    #[tokio::test]
    async fn serves_a_directory_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("index.html"), "<!doctype html>disk").unwrap();
        std::fs::write(dir.path().join("assets/main-9f.css"), "body{}").unwrap();
        let ui = Ui::Dir(dir.path().to_path_buf());
        let (status, h, body) = get(&ui, Method::GET, "/assets/main-9f.css", HeaderMap::new()).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"body{}"[..]));
        assert_eq!(h[header::CONTENT_TYPE], "text/css; charset=utf-8");
        assert_eq!(h[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        let (status, _, body) = get(&ui, Method::GET, "/datastores/app-db", HeaderMap::new()).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"<!doctype html>disk"[..]));
        // directories aren't files; traversal is refused
        let (status, _, body) = get(&ui, Method::GET, "/assets", HeaderMap::new()).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"<!doctype html>disk"[..]));
        let (status, _, _) = get(&ui, Method::GET, "/assets/%2e%2e/index.html", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // a directory without index.html falls back to the embedded one
        let empty = tempfile::tempdir().unwrap();
        let ui = Ui::Dir(empty.path().to_path_buf());
        let (status, _, body) = get(&ui, Method::GET, "/", HeaderMap::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, embedded_file("index.html").unwrap());
    }
}
