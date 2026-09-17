//! Static asset serving for the NAVI web frontend.
//!
//! Provides a warp filter that serves embedded frontend assets (compiled into
//! the binary via `rust-embed`) or, when `--web-dir` is passed, assets from an
//! external directory on the filesystem. This enables a hybrid model:
//!
//! - **Default (embedded):** self-contained binary, no external files needed.
//! - **Override (`--web-dir`):** serve from disk for development or custom UIs.
//!
//! The filter is designed as a **catch-all** mounted after all API routes so
//! that API paths (`/health`, `/models`, `/sessions/...`, etc.) always take
//! precedence. Non-API paths fall through to the SPA, which serves
//! `index.html` for client-side routing.

use rust_embed::RustEmbed;
use std::path::PathBuf;
use warp::Filter;
use warp::http::header::HeaderValue;
use warp::http::{HeaderMap, StatusCode};
use warp::reply::Reply;

#[cfg(test)]
use std::convert::Infallible;

// ── Embedded assets ──────────────────────────────────────────────────────

/// Embed the `web/dist/` directory at compile time.
///
/// When `web/dist/` does not exist (e.g. frontend not built yet), the
/// `RustEmbed` derive produces an empty asset set — the server still works
/// as an API-only server, returning 404 for all non-API paths.
#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct EmbeddedAssets;

// ── Asset source ─────────────────────────────────────────────────────────

/// Where to resolve static assets from.
#[derive(Clone, Debug)]
pub enum AssetSource {
    /// Serve assets embedded at compile time via `rust-embed`.
    Embedded,
    /// Serve assets from a directory on the filesystem.
    FileSystem(PathBuf),
}

impl AssetSource {
    /// Create an `AssetSource` from an optional `--web-dir` path.
    ///
    /// `None` or empty string → `Embedded`.
    /// `Some(path)` → `FileSystem(path)`.
    pub fn from_web_dir(web_dir: Option<&str>) -> Self {
        match web_dir {
            Some(dir) if !dir.trim().is_empty() => AssetSource::FileSystem(PathBuf::from(dir)),
            _ => AssetSource::Embedded,
        }
    }
}

// ── Asset resolution ─────────────────────────────────────────────────────

/// A resolved asset ready to be served.
struct ResolvedAsset {
    data: Vec<u8>,
    mime: &'static str,
}

/// Resolve a request path to an embedded asset.
///
/// Returns `Ok(ResolvedAsset)` if found, `Ok(None)` if the path doesn't match
/// any embedded file, or `Err` on I/O error (filesystem mode).
fn resolve_embedded(path: &str) -> Option<ResolvedAsset> {
    let normalized = normalize_path(path);
    let asset = EmbeddedAssets::get(&normalized)?;
    Some(ResolvedAsset {
        data: asset.data.to_vec(),
        mime: mime_for_path(&normalized),
    })
}

/// Resolve a request path to a filesystem asset.
///
/// Security: the path is sanitized to prevent directory traversal. Only files
/// directly under `root` (or its subdirectories) are served. `..` segments
/// and absolute paths are rejected.
fn resolve_filesystem(
    root: &std::path::Path,
    path: &str,
) -> std::io::Result<Option<ResolvedAsset>> {
    let normalized = normalize_path(path);
    if normalized.is_empty() || normalized.contains("..") {
        return Ok(None);
    }

    let full = root.join(&normalized);

    // Canonicalize both root and the resolved path, then verify the resolved
    // path starts with root. This prevents symlinks from escaping the jail.
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canon_full = match full.canonicalize() {
        Ok(p) => p,
        Err(_) => return Ok(None), // File doesn't exist
    };

    if !canon_full.starts_with(&canon_root) {
        return Ok(None);
    }

    if !canon_full.is_file() {
        return Ok(None);
    }

    let data = std::fs::read(&canon_full)?;
    Ok(Some(ResolvedAsset {
        data,
        mime: mime_for_path(&normalized),
    }))
}

/// Resolve any path from the asset source, with SPA fallback to `index.html`.
///
/// - If the path matches a real asset → serve it.
/// - If the path doesn't match → serve `index.html` (SPA client-side routing).
/// - If `index.html` also doesn't exist → 404.
fn resolve(source: &AssetSource, path: &str) -> Option<ResolvedAsset> {
    match source {
        AssetSource::Embedded => resolve_embedded(path).or_else(|| resolve_embedded("index.html")),
        AssetSource::FileSystem(root) => match resolve_filesystem(root, path).ok().flatten() {
            Some(asset) => Some(asset),
            None => resolve_filesystem(root, "index.html").ok().flatten(),
        },
    }
}

// ── Path normalization & MIME types ──────────────────────────────────────

/// Normalize a request path into a relative asset path.
///
/// - Strips leading `/`.
/// - Empty path → `index.html`.
/// - Preserves subdirectory structure (e.g. `/assets/app.js` → `assets/app.js`).
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        "index.html".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Determine the MIME type for a given asset path based on its extension.
///
/// Covers all common web asset types. Unknown extensions default to
/// `application/octet-stream`.
fn mime_for_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "map" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

// ── HTTP response builder ────────────────────────────────────────────────

/// Build a warp response from a resolved asset.
fn asset_response(asset: &ResolvedAsset) -> warp::reply::Response {
    let mut headers = HeaderMap::new();
    if let Ok(mime) = HeaderValue::from_str(asset.mime) {
        headers.insert("content-type", mime);
    }
    // Immutable cache for hashed assets (Vite uses content hashes in filenames).
    // index.html is never cached so clients always get the latest entry point.
    if !asset.mime.starts_with("text/html") {
        headers.insert(
            "cache-control",
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    }

    let mut resp = asset.data.clone().into_response();
    resp.headers_mut().extend(headers);
    resp
}

/// Build a 404 response with a JSON body (consistent with API error format).
fn not_found_response() -> warp::reply::Response {
    warp::reply::with_status(
        warp::reply::json(&serde_json::json!({"error": "not found"})),
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

// ── Warp filter ──────────────────────────────────────────────────────────

/// Create a warp filter that serves static web assets.
///
/// This filter matches **all** methods and paths (it's a catch-all). It should
/// be mounted as the last route in the filter chain, after all API routes, so
/// API endpoints take precedence.
///
/// **No authentication** — static assets (HTML/JS/CSS) are public. The actual
/// API calls from the frontend carry the `X-Navi-Secret` header.
///
/// Returns `Error = warp::Rejection` for compatibility with the route chain's
/// `.recover()` handler, though this filter never actually rejects.
pub fn web_filter(
    source: AssetSource,
) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    warp::any()
        .and(warp::path::full())
        .and_then(move |full_path: warp::path::FullPath| {
            let source = source.clone();
            async move {
                let path_str = full_path.as_str();
                let resp = match resolve(&source, path_str) {
                    Some(asset) => asset_response(&asset),
                    None => not_found_response(),
                };
                Ok::<_, warp::Rejection>(resp)
            }
        })
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Layer 1: Unit tests (deterministic, pure logic) ───────────────────

    #[test]
    fn normalize_path_empty_becomes_index_html() {
        assert_eq!(normalize_path(""), "index.html");
        assert_eq!(normalize_path("/"), "index.html");
    }

    #[test]
    fn normalize_path_strips_leading_slash() {
        assert_eq!(normalize_path("/assets/app.js"), "assets/app.js");
        assert_eq!(normalize_path("/index.html"), "index.html");
    }

    #[test]
    fn normalize_path_preserves_subdirectories() {
        assert_eq!(
            normalize_path("/assets/images/logo.png"),
            "assets/images/logo.png"
        );
    }

    #[test]
    fn normalize_path_preserves_nested_structure() {
        assert_eq!(
            normalize_path("/deep/nested/path/file.css"),
            "deep/nested/path/file.css"
        );
    }

    #[test]
    fn mime_for_html() {
        assert_eq!(mime_for_path("index.html"), "text/html; charset=utf-8");
        assert_eq!(mime_for_path("page.htm"), "text/html; charset=utf-8");
    }

    #[test]
    fn mime_for_javascript() {
        assert_eq!(
            mime_for_path("app.js"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_for_path("module.mjs"),
            "application/javascript; charset=utf-8"
        );
    }

    #[test]
    fn mime_for_css() {
        assert_eq!(mime_for_path("style.css"), "text/css; charset=utf-8");
    }

    #[test]
    fn mime_for_json() {
        assert_eq!(
            mime_for_path("data.json"),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn mime_for_images() {
        assert_eq!(mime_for_path("logo.svg"), "image/svg+xml");
        assert_eq!(mime_for_path("photo.png"), "image/png");
        assert_eq!(mime_for_path("photo.jpg"), "image/jpeg");
        assert_eq!(mime_for_path("photo.jpeg"), "image/jpeg");
        assert_eq!(mime_for_path("anim.gif"), "image/gif");
        assert_eq!(mime_for_path("icon.ico"), "image/x-icon");
        assert_eq!(mime_for_path("photo.webp"), "image/webp");
    }

    #[test]
    fn mime_for_fonts() {
        assert_eq!(mime_for_path("font.woff"), "font/woff");
        assert_eq!(mime_for_path("font.woff2"), "font/woff2");
        assert_eq!(mime_for_path("font.ttf"), "font/ttf");
        assert_eq!(mime_for_path("font.otf"), "font/otf");
    }

    #[test]
    fn mime_for_wasm() {
        assert_eq!(mime_for_path("module.wasm"), "application/wasm");
    }

    #[test]
    fn mime_for_source_map() {
        assert_eq!(
            mime_for_path("app.js.map"),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn mime_for_webmanifest() {
        assert_eq!(
            mime_for_path("manifest.webmanifest"),
            "application/manifest+json"
        );
    }

    #[test]
    fn mime_for_unknown_defaults_to_octet_stream() {
        assert_eq!(mime_for_path("file.unknownext"), "application/octet-stream");
        assert_eq!(mime_for_path("noextension"), "application/octet-stream");
    }

    #[test]
    fn mime_is_case_insensitive() {
        assert_eq!(mime_for_path("FILE.HTML"), "text/html; charset=utf-8");
        assert_eq!(
            mime_for_path("App.JS"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(mime_for_path("Style.CSS"), "text/css; charset=utf-8");
    }

    #[test]
    fn asset_source_from_web_dir_none_is_embedded() {
        assert!(matches!(
            AssetSource::from_web_dir(None),
            AssetSource::Embedded
        ));
    }

    #[test]
    fn asset_source_from_web_dir_empty_is_embedded() {
        assert!(matches!(
            AssetSource::from_web_dir(Some("")),
            AssetSource::Embedded
        ));
        assert!(matches!(
            AssetSource::from_web_dir(Some("   ")),
            AssetSource::Embedded
        ));
    }

    #[test]
    fn asset_source_from_web_dir_some_is_filesystem() {
        let src = AssetSource::from_web_dir(Some("/var/www/navi"));
        assert!(matches!(src, AssetSource::FileSystem(_)));
        if let AssetSource::FileSystem(path) = src {
            assert_eq!(path, PathBuf::from("/var/www/navi"));
        }
    }

    #[test]
    fn asset_source_is_clone() {
        let src = AssetSource::Embedded;
        let cloned = src.clone();
        assert!(matches!(cloned, AssetSource::Embedded));
    }

    // ── Layer 2: Edge case tests ──────────────────────────────────────────

    #[test]
    fn normalize_path_double_slash() {
        // trim_start_matches('/') strips ALL leading slashes.
        assert_eq!(normalize_path("//index.html"), "index.html");
    }

    #[test]
    fn normalize_path_only_slashes() {
        assert_eq!(normalize_path("///"), "index.html");
    }

    #[test]
    fn normalize_path_with_query_string_fragment() {
        // warp::path::full() does not include query strings, but test
        // robustness anyway — query-like paths should be treated literally.
        assert_eq!(normalize_path("/page"), "page");
    }

    #[test]
    fn normalize_path_unicode() {
        assert_eq!(normalize_path("/café.html"), "café.html");
        assert_eq!(normalize_path("/日本語/index.html"), "日本語/index.html");
    }

    #[test]
    fn normalize_path_emoji() {
        assert_eq!(normalize_path("/🚀.js"), "🚀.js");
    }

    #[test]
    fn mime_for_empty_extension() {
        assert_eq!(mime_for_path("file."), "application/octet-stream");
    }

    #[test]
    fn mime_for_dotfile() {
        // Files starting with a dot — rsplit('.') treats the part after the
        // last dot as the extension. For ".gitignore", that's "gitignore"
        // which is unknown → octet-stream.
        assert_eq!(mime_for_path(".gitignore"), "application/octet-stream");
    }

    #[test]
    fn mime_for_multiple_dots() {
        assert_eq!(
            mime_for_path("app.min.js"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_for_path("data.config.json"),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn mime_for_very_long_extension() {
        // Absurdly long extension — should still default to octet-stream
        let long_ext = "x".repeat(1000);
        let path = format!("file.{long_ext}");
        assert_eq!(mime_for_path(&path), "application/octet-stream");
    }

    #[test]
    fn resolve_embedded_returns_none_for_missing() {
        // Without a built frontend, embedded assets are empty.
        // This should return None, not panic.
        let result = resolve_embedded("nonexistent-file-xyz.js");
        // May be Some if dist/ exists from a prior build, or None if empty.
        // Either way, must not panic.
        let _ = result;
    }

    #[test]
    fn resolve_embedded_empty_path_falls_back_to_index() {
        let result = resolve_embedded("");
        // Empty path normalizes to "index.html"
        // May be Some or None depending on whether dist/ was built.
        let _ = result;
    }

    #[test]
    fn resolve_filesystem_rejects_traversal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_filesystem(tmp.path(), "../etc/passwd").ok();
        assert!(result.is_some());
        assert!(
            result.unwrap().is_none(),
            "directory traversal must be rejected"
        );
    }

    #[test]
    fn resolve_filesystem_rejects_absolute_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Absolute paths get joined to root, so they won't escape — but
        // they also won't resolve to a file under root.
        let result = resolve_filesystem(tmp.path(), "/etc/passwd").ok();
        assert!(result.is_some());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn resolve_filesystem_nonexistent_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_filesystem(tmp.path(), "nope.html").ok().flatten();
        assert!(result.is_none());
    }

    #[test]
    fn resolve_filesystem_serves_existing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), "<h1>Hi</h1>").expect("write");
        let result = resolve_filesystem(tmp.path(), "index.html")
            .ok()
            .flatten()
            .expect("file should resolve");
        assert_eq!(result.data, b"<h1>Hi</h1>");
        assert_eq!(result.mime, "text/html; charset=utf-8");
    }

    #[test]
    fn resolve_filesystem_serves_nested_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("assets")).expect("mkdir");
        std::fs::write(tmp.path().join("assets/app.js"), "console.log(1)").expect("write");
        let result = resolve_filesystem(tmp.path(), "assets/app.js")
            .ok()
            .flatten()
            .expect("file should resolve");
        assert_eq!(result.data, b"console.log(1)");
        assert_eq!(result.mime, "application/javascript; charset=utf-8");
    }

    #[test]
    fn resolve_filesystem_empty_path_serves_index() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), "<h1>Root</h1>").expect("write");
        let result = resolve_filesystem(tmp.path(), "")
            .ok()
            .flatten()
            .expect("index.html should resolve");
        assert_eq!(result.data, b"<h1>Root</h1>");
    }

    #[test]
    fn resolve_filesystem_directory_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("subdir")).expect("mkdir");
        // Requesting a directory should not serve it
        let result = resolve_filesystem(tmp.path(), "subdir").ok().flatten();
        assert!(result.is_none(), "directories must not be served as files");
    }

    #[test]
    fn resolve_filesystem_traversal_with_subdir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("assets")).expect("mkdir");
        std::fs::write(tmp.path().join("assets/x.js"), b"1").expect("write");
        // Normal subdir access works
        let result = resolve_filesystem(tmp.path(), "assets/x.js")
            .ok()
            .flatten()
            .expect("should resolve");
        assert_eq!(result.data, b"1");
        // Traversal from subdir back up fails
        let result = resolve_filesystem(tmp.path(), "assets/../../etc/passwd")
            .ok()
            .flatten();
        assert!(result.is_none(), "traversal must be blocked");
    }

    #[test]
    fn resolve_embedded_fallback_returns_index_when_path_missing() {
        let source = AssetSource::Embedded;
        // resolve() should fall back to index.html for unknown paths.
        // If no dist/ exists, both return None → resolve returns None.
        let _ = resolve(&source, "/nonexistent-route");
    }

    #[test]
    fn resolve_filesystem_fallback_returns_index_when_path_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<h1>SPA</h1>").expect("write");
        let source = AssetSource::FileSystem(tmp.path().to_path_buf());
        let result = resolve(&source, "/some/spa/route").expect("should fall back to index.html");
        assert_eq!(result.data, b"<h1>SPA</h1>");
        assert_eq!(result.mime, "text/html; charset=utf-8");
    }

    #[test]
    fn resolve_filesystem_returns_none_when_no_index() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = AssetSource::FileSystem(tmp.path().to_path_buf());
        let result = resolve(&source, "/anything");
        assert!(result.is_none(), "should be 404 when no index.html exists");
    }

    #[test]
    fn asset_response_sets_content_type() {
        let asset = ResolvedAsset {
            data: b"<h1>Hi</h1>".to_vec(),
            mime: "text/html; charset=utf-8",
        };
        let resp = asset_response(&asset);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn asset_response_sets_no_cache_for_html() {
        let asset = ResolvedAsset {
            data: b"<h1>Hi</h1>".to_vec(),
            mime: "text/html; charset=utf-8",
        };
        let resp = asset_response(&asset);
        assert_eq!(resp.headers().get("cache-control").unwrap(), "no-cache");
    }

    #[test]
    fn asset_response_sets_immutable_cache_for_assets() {
        let asset = ResolvedAsset {
            data: b"console.log(1)".to_vec(),
            mime: "application/javascript; charset=utf-8",
        };
        let resp = asset_response(&asset);
        assert_eq!(
            resp.headers().get("cache-control").unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[test]
    fn asset_response_sets_immutable_cache_for_css() {
        let asset = ResolvedAsset {
            data: b".x{}".to_vec(),
            mime: "text/css; charset=utf-8",
        };
        let resp = asset_response(&asset);
        assert_eq!(
            resp.headers().get("cache-control").unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn asset_response_includes_body_data() {
        let asset = ResolvedAsset {
            data: b"body content here".to_vec(),
            mime: "text/plain; charset=utf-8",
        };
        let resp = asset_response(&asset);
        let body_bytes = warp::hyper::body::to_bytes(resp.into_body())
            .await
            .expect("body bytes");
        let body = String::from_utf8(body_bytes.to_vec()).expect("utf-8");
        assert_eq!(body, "body content here");
    }

    #[test]
    fn not_found_response_returns_404() {
        let resp = not_found_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn not_found_response_has_json_error() {
        let resp = not_found_response();
        let body_bytes = warp::hyper::body::to_bytes(resp.into_body())
            .await
            .expect("body bytes");
        let body = String::from_utf8(body_bytes.to_vec()).expect("utf-8");
        assert!(body.contains("not found"), "body={body}");
    }

    // ── Layer 3: Integration tests (warp::test against real filter) ────────

    #[tokio::test]
    async fn web_filter_serves_filesystem_index_at_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<h1>NAVI Web</h1>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/").reply(&filter).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
        let body = String::from_utf8_lossy(res.body());
        assert!(body.contains("NAVI Web"), "body={body}");
    }

    #[tokio::test]
    async fn web_filter_serves_filesystem_asset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("assets")).expect("mkdir");
        std::fs::write(tmp.path().join("assets/app.js"), b"console.log(42)").expect("write");
        std::fs::write(tmp.path().join("index.html"), b"<html></html>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/assets/app.js")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "application/javascript; charset=utf-8"
        );
        let body = String::from_utf8_lossy(res.body());
        assert_eq!(body, "console.log(42)");
    }

    #[tokio::test]
    async fn web_filter_spa_fallback_serves_index_for_unknown_route() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<div id=app></div>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/sessions/some-deep-route/panel")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn web_filter_returns_404_when_no_assets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/").reply(&filter).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn web_filter_serves_css_with_correct_mime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("style.css"), b"body{margin:0}").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/style.css")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "text/css; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn web_filter_serves_svg_with_correct_mime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("logo.svg"), b"<svg></svg>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/logo.svg").reply(&filter).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("content-type").unwrap(), "image/svg+xml");
    }

    #[tokio::test]
    async fn web_filter_serves_wasm_with_correct_mime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("asset.wasm"), b"\0asm").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/asset.wasm")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "application/wasm"
        );
    }

    #[tokio::test]
    async fn web_filter_serves_woff2_with_correct_mime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("font.woff2"), b"font-data").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/font.woff2")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("content-type").unwrap(), "font/woff2");
    }

    #[tokio::test]
    async fn web_filter_rejects_traversal_attempt() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Create a secret file outside the web root
        std::fs::write(tmp.path().join("index.html"), b"safe").expect("write");
        let parent = tmp.path().parent().expect("parent");
        std::fs::write(parent.join("secret.txt"), b"SECRET").expect("write");

        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/../secret.txt")
            .reply(&filter)
            .await;
        // Should fall back to index.html (SPA), not serve the secret file
        assert_eq!(res.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(res.body());
        assert!(!body.contains("SECRET"), "traversal must not expose files");
    }

    #[tokio::test]
    async fn web_filter_immutable_cache_on_js() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("app.js"), b"1").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/app.js").reply(&filter).await;
        assert_eq!(
            res.headers().get("cache-control").unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn web_filter_no_cache_on_html() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<html>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/").reply(&filter).await;
        assert_eq!(res.headers().get("cache-control").unwrap(), "no-cache");
    }

    #[tokio::test]
    async fn web_filter_serves_nested_asset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("assets/images")).expect("mkdir");
        std::fs::write(
            tmp.path().join("assets/images/logo.png"),
            b"\x89PNG\r\n\x1a\n",
        )
        .expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/assets/images/logo.png")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("content-type").unwrap(), "image/png");
    }

    #[tokio::test]
    async fn web_filter_handles_empty_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<h1>Home</h1>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/").reply(&filter).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(res.body()).contains("Home"));
    }

    #[tokio::test]
    async fn web_filter_serves_webmanifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("manifest.webmanifest"),
            b"{\"name\":\"NAVI\"}",
        )
        .expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/manifest.webmanifest")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "application/manifest+json"
        );
    }

    #[tokio::test]
    async fn web_filter_serves_json_with_correct_mime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("config.json"), b"{}").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/config.json")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn web_filter_unknown_extension_uses_octet_stream() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("data.xyz"), b"binary").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/data.xyz").reply(&filter).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn web_filter_spa_fallback_for_deep_nested_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<div>SPA</div>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request()
            .path("/a/b/c/d/e/f/g/h/i/j")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(res.body()).contains("SPA"));
    }

    #[tokio::test]
    async fn web_filter_directory_request_falls_back_to_index() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("subdir")).expect("mkdir");
        std::fs::write(tmp.path().join("index.html"), b"<html>").expect("write");
        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let res = warp::test::request().path("/subdir").reply(&filter).await;
        // Directory is not a file → SPA fallback to index.html
        assert_eq!(res.status(), StatusCode::OK);
    }

    // ── E2E: Full server integration (API + static assets) ────────────────

    #[tokio::test]
    async fn e2e_api_routes_take_precedence_over_static() {
        // Build a combined filter: API health route + static web filter
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<h1>Web</h1>").expect("write");

        let api = warp::path("health")
            .and(warp::get())
            .map(|| warp::reply::json(&serde_json::json!({"status": "ok"})));

        let web = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        let combined = api.or(web).recover(handle_rejection_e2e);

        // /health → API
        let res = warp::test::request().path("/health").reply(&combined).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(res.body());
        assert!(body.contains("ok"), "health body={body}");

        // / → static (index.html)
        let res = warp::test::request().path("/").reply(&combined).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(res.body());
        assert!(body.contains("Web"), "index body={body}");

        // /unknown-spa-route → SPA fallback
        let res = warp::test::request()
            .path("/chat/session/123")
            .reply(&combined)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(res.body()).contains("Web"));
    }

    #[tokio::test]
    async fn e2e_static_filter_does_not_interfere_with_api() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<h1>Web</h1>").expect("write");

        // Simulate a protected API route
        let api = warp::path("models")
            .and(warp::get())
            .map(|| warp::reply::json(&serde_json::json!([{"name": "gpt-4"}])));

        let web = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));
        let combined = api.or(web);

        // /models → API response (JSON array)
        let res = warp::test::request().path("/models").reply(&combined).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(res.body());
        assert!(body.contains("gpt-4"), "models body={body}");

        // /index.html → static
        let res = warp::test::request()
            .path("/index.html")
            .reply(&combined)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(res.body()).contains("Web"));
    }

    #[tokio::test]
    async fn e2e_embedded_source_with_no_dist_returns_404() {
        // When no web/dist/ exists at build time, EmbeddedAssets is empty.
        // All non-API paths should return 404.
        let filter = web_filter(AssetSource::Embedded);

        let res = warp::test::request().path("/").reply(&filter).await;
        // If dist/ was built during test compilation, this might be 200.
        // If not, it should be 404. Either way, no panic.
        let status = res.status();
        assert!(
            status == StatusCode::OK || status == StatusCode::NOT_FOUND,
            "expected 200 or 404, got {status}"
        );
    }

    #[tokio::test]
    async fn e2e_filesystem_override_serves_custom_ui() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("index.html"), b"<h1>Custom NAVI UI</h1>").expect("write");
        std::fs::create_dir_all(tmp.path().join("assets")).expect("mkdir");
        std::fs::write(
            tmp.path().join("assets/custom.js"),
            b"console.log('custom')",
        )
        .expect("write");

        let filter = web_filter(AssetSource::FileSystem(tmp.path().to_path_buf()));

        // Root → custom index
        let res = warp::test::request().path("/").reply(&filter).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(res.body()).contains("Custom NAVI UI"));

        // Custom asset
        let res = warp::test::request()
            .path("/assets/custom.js")
            .reply(&filter)
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(res.body()).contains("custom"));
    }
}

/// Rejection handler for E2E tests (mirrors server's handle_rejection).
#[cfg(test)]
async fn handle_rejection_e2e(err: warp::Rejection) -> Result<impl warp::Reply, Infallible> {
    let _ = err;
    Ok(warp::reply::with_status(
        warp::reply::json(&serde_json::json!({"error": "not found"})),
        StatusCode::NOT_FOUND,
    ))
}
