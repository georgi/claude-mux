//! The embedded web app (architecture 11.2), served at `/`.
//!
//! `npm run build` in `web/` writes `web/dist/`; with the `web` feature (on by
//! default) `rust-embed` puts it into the binary (debug builds read it from
//! disk). Without a build, or without the feature, `/` serves a placeholder
//! page, so `cargo test` needs no Node.
//!
//! Every page gets a strict CSP. Vite names assets under `assets/` by content
//! hash, so they are cached for a year; `index.html` and the other files are
//! revalidated on every load. Any other path that is not an API route gets
//! `index.html`, so the app's own routes (such as `/pair?code=`) load.

use std::borrow::Cow;

use axum::Router;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

/// Scripts and connections only from this origin. Styles allow inline
/// because xterm.js writes `<style>` elements; nothing else does.
pub const CSP: &str = "default-src 'none'; script-src 'self'; \
    style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; \
    connect-src 'self'; manifest-src 'self'; worker-src 'self'; \
    base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// For content-hashed files under `assets/`.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// For `index.html` and other unhashed files.
pub const REVALIDATE: &str = "no-cache";

const PLACEHOLDER: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>cmux</title></head><body>\
<h1>cmux</h1><p>This cmuxd was built without the web app. \
Run <code>npm ci &amp;&amp; npm run build</code> in <code>web/</code>, \
then rebuild cmuxd.</p></body></html>\n";

#[cfg(feature = "web")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Dist;

/// A file of the build and its MIME type.
#[cfg(feature = "web")]
fn file(path: &str) -> Option<(Cow<'static, [u8]>, String)> {
    let f = Dist::get(path)?;
    Some((f.data, f.metadata.mimetype().to_string()))
}

#[cfg(not(feature = "web"))]
fn file(_path: &str) -> Option<(Cow<'static, [u8]>, String)> {
    None
}

/// The routes: a fallback, so it never shadows an API route.
pub fn routes() -> Router {
    Router::new().fallback(serve)
}

async fn serve(method: Method, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // Unknown API paths stay errors, and a missing asset must not turn into
    // a page the browser then refuses to run as a script.
    if path == "v1" || path.starts_with("v1/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let hashed = path.starts_with("assets/");
    let (body, mime, cache) = match file(path).filter(|_| !path.is_empty()) {
        Some((body, mime)) => (body, mime, if hashed { IMMUTABLE } else { REVALIDATE }),
        None if hashed => return StatusCode::NOT_FOUND.into_response(),
        None => {
            let body = file("index.html").map_or(Cow::Borrowed(PLACEHOLDER.as_bytes()), |f| f.0);
            (body, "text/html; charset=utf-8".into(), REVALIDATE)
        }
    };
    let mut resp = (StatusCode::OK, body).into_response();
    let h = resp.headers_mut();
    let mime = HeaderValue::from_str(&mime)
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_TYPE, mime);
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    resp
}
