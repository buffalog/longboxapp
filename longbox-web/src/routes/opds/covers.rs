//! OPDS cover proxy with a local write-behind cache.
//!
//! `GET /opds/v1/covers/{issue_id}`:
//!   1. Serve `{covers_dir}/{issue_id}.jpg` directly if it exists.
//!   2. Otherwise look up the issue's `cover_url`; 404 if the issue is
//!      unknown or has no cover URL.
//!   3. Fetch the image from the ComicVine CDN, return it to the client,
//!      and write it to the cache in the background (best-effort).
//!
//! Covers are small, so the image is buffered in full rather than
//! true-streamed — this keeps the tee-to-disk simple and lets a failed
//! cache write never affect the response. The cache directory lives inside
//! the longbox-db Docker volume (`config.opds_covers_dir`).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use longbox_db::issue_repo;

use crate::error::ApiError;
use crate::state::AppState;

/// Shared, connection-pooled HTTP client for CDN fetches. Built once.
/// Redirects are disabled: a cover URL must resolve directly, so an
/// attacker-influenced `cover_url` can't 302-pivot into an internal host.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("longbox-opds/1.0")
            .build()
            .expect("cover proxy HTTP client builds")
    })
}

/// SSRF guard for a stored `cover_url`. Always refuses anything that isn't
/// an `http(s)` URL. Unless `allow_private` is set, also refuses literal
/// loopback/private/link-local/unspecified IP hosts. Hostnames that
/// *resolve* to private space are not caught here — combined with the
/// no-redirect client that residual is narrow; full egress filtering
/// belongs at the network layer. `allow_private` exists for deployments
/// whose covers live on a private host (and for tests, whose mock CDN
/// binds loopback).
fn is_safe_cover_url(raw: &str, allow_private: bool) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if allow_private {
        return true;
    }
    // host_str keeps IPv6 brackets (`[::1]`); strip them so a literal IP
    // parses.
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_loopback() || ip.is_unspecified() {
            return false;
        }
        if let std::net::IpAddr::V4(v4) = ip {
            if v4.is_private() || v4.is_link_local() {
                return false;
            }
        }
    }
    true
}

/// `GET /opds/v1/covers/{issue_id}`.
pub async fn cover(
    State(state): State<AppState>,
    Path(issue_id): Path<i64>,
) -> Result<Response, ApiError> {
    let cache_path = cache_path(&state.config.opds_covers_dir, issue_id);

    // 1. Cache hit.
    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return Ok(jpeg_response(bytes));
    }

    // 2. Resolve the issue's cover URL; 404 if unknown or unset.
    let cover_url = issue_repo::find_by_id(&state.db, issue_id)
        .await?
        .and_then(|issue| issue.cover_url)
        .filter(|url| !url.trim().is_empty())
        .ok_or(ApiError::NotFound {
            resource: "cover",
            id: issue_id.to_string(),
        })?;

    // SSRF guard before any outbound request.
    if !is_safe_cover_url(&cover_url, state.config.opds_allow_private_cover_hosts) {
        tracing::warn!(issue_id, "OPDS cover: refused unsafe cover URL");
        return Err(ApiError::NotFound {
            resource: "cover",
            id: issue_id.to_string(),
        });
    }

    // 3. Fetch from the CDN.
    let bytes = fetch_cover(&cover_url).await?;

    // 4. Write-behind cache (non-blocking, best-effort).
    spawn_cache_write(cache_path, bytes.clone());

    Ok(jpeg_response(bytes))
}

/// Fetch the cover bytes from the CDN, mapping transport/status failures to
/// a 502.
async fn fetch_cover(url: &str) -> Result<Vec<u8>, ApiError> {
    let resp = http_client().get(url).send().await.map_err(|err| {
        ApiError::Upstream {
            service: "cover_cdn",
            status: 502,
            message: format!("cover fetch failed: {err}"),
        }
    })?;

    let status = resp.status();
    if !status.is_success() {
        return Err(ApiError::Upstream {
            service: "cover_cdn",
            status: status.as_u16(),
            message: format!("cover CDN returned {status}"),
        });
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|err| ApiError::Upstream {
            service: "cover_cdn",
            status: 502,
            message: format!("cover body read failed: {err}"),
        })
}

/// Cache file for an issue's cover: `{dir}/{issue_id}.jpg`.
fn cache_path(dir: &std::path::Path, issue_id: i64) -> PathBuf {
    dir.join(format!("{issue_id}.jpg"))
}

/// Spawn a detached task to write the cover to the cache. Creates the
/// directory if needed; any failure is logged, never surfaced — the client
/// already has its image.
fn spawn_cache_write(path: PathBuf, bytes: Vec<u8>) {
    tokio::spawn(async move {
        if let Some(parent) = path.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                tracing::warn!(error = %err, ?path, "OPDS cover cache: mkdir failed");
                return;
            }
        }
        if let Err(err) = tokio::fs::write(&path, &bytes).await {
            tracing::warn!(error = %err, ?path, "OPDS cover cache: write failed");
        }
    });
}

fn jpeg_response(bytes: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/jpeg")],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::is_safe_cover_url;

    #[test]
    fn accepts_normal_cdn_urls() {
        assert!(is_safe_cover_url("https://comicvine.gamespot.com/a/uploads/x.jpg", false));
        assert!(is_safe_cover_url("http://cdn.example.com/x.jpg", false));
        // Public IP literal is fine.
        assert!(is_safe_cover_url("https://8.8.8.8/x.jpg", false));
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(!is_safe_cover_url("file:///etc/hosts", false));
        assert!(!is_safe_cover_url("gopher://evil/x", false));
        assert!(!is_safe_cover_url("not a url", false));
        // Non-http schemes are refused even with allow_private.
        assert!(!is_safe_cover_url("file:///etc/hosts", true));
    }

    #[test]
    fn rejects_internal_ip_literals() {
        assert!(!is_safe_cover_url("http://127.0.0.1/x.jpg", false)); // loopback
        assert!(!is_safe_cover_url("http://169.254.169.254/latest/meta-data", false)); // link-local
        assert!(!is_safe_cover_url("http://10.0.0.5/x.jpg", false)); // private
        assert!(!is_safe_cover_url("http://192.168.1.1/x.jpg", false)); // private
        assert!(!is_safe_cover_url("http://[::1]/x.jpg", false)); // v6 loopback
        assert!(!is_safe_cover_url("http://0.0.0.0/x.jpg", false)); // unspecified
    }

    #[test]
    fn allow_private_permits_loopback() {
        assert!(is_safe_cover_url("http://127.0.0.1:8080/x.jpg", true));
        assert!(is_safe_cover_url("http://[::1]/x.jpg", true));
    }
}
