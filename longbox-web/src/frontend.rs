//! Frontend asset serving via `rust-embed`. The build embeds the contents of
//! `longbox-web/frontend-dist/` at compile time. Step 6 replaces that
//! directory with the SvelteKit build output; everything here is unchanged.
//!
//! SPA fallback: any GET that isn't `/api/*`, isn't a known reserved path
//! (`/.well-known/...`), and doesn't match an embedded asset returns
//! `index.html` so client-side routing works for `/series/42`, `/scan`, etc.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend-dist/"]
struct Frontend;

pub async fn fallback_handler(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');

    // True 404 for reserved paths — never fall back to index.
    if path.starts_with(".well-known/") {
        return reserved_404();
    }

    // Try the literal file path first.
    if let Some(resp) = serve_embedded(path) {
        return resp;
    }

    // Bare root → index.html.
    if path.is_empty() {
        if let Some(resp) = serve_embedded("index.html") {
            return resp;
        }
    }

    // SPA fallback: client-side routes like `/series/42` → index.html.
    if let Some(resp) = serve_embedded("index.html") {
        return resp;
    }

    // No index.html in the bundle — give the 404 page if we have one, else
    // a plain 404.
    if let Some(resp) = serve_embedded("404.html") {
        let (parts, body) = resp.into_parts();
        return (StatusCode::NOT_FOUND, parts.headers, body).into_response();
    }
    reserved_404()
}

fn serve_embedded(path: &str) -> Option<Response> {
    let file = Frontend::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut resp = Response::new(Body::from(file.data.into_owned()));
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        resp.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control_for(path)),
    );
    Some(resp)
}

/// Cache policy for an embedded asset by path. SvelteKit's bundle assets under
/// `_app/immutable/` are content-hashed, so a given URL's bytes never change —
/// cache them forever. Everything else — above all `index.html`, which is also
/// what the SPA fallback returns for every client route — must revalidate: the
/// shell embeds the current build's hashed chunk filenames, and a browser that
/// serves a stale shell after a redeploy requests chunk hashes that no longer
/// exist (404), crashing the app on the first route whose chunk changed.
fn cache_control_for(path: &str) -> &'static str {
    if path.starts_with("_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

fn reserved_404() -> Response {
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

#[cfg(test)]
mod tests {
    use super::cache_control_for;

    #[test]
    fn immutable_bundle_assets_cache_forever() {
        assert_eq!(
            cache_control_for("_app/immutable/entry/app.B95htGq7.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_for("_app/immutable/chunks/Wdp1YZRo.js"),
            "public, max-age=31536000, immutable"
        );
    }

    #[test]
    fn shell_and_other_assets_revalidate() {
        // The shell and the SPA fallback must never be cached stale.
        assert_eq!(cache_control_for("index.html"), "no-cache");
        assert_eq!(cache_control_for("404.html"), "no-cache");
        assert_eq!(cache_control_for("favicon.ico"), "no-cache");
        assert_eq!(cache_control_for(""), "no-cache");
    }
}
