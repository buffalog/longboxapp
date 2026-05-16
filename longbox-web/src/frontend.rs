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
    Some(resp)
}

fn reserved_404() -> Response {
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}
