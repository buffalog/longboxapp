//! OPDS file streaming: `GET /opds/v1/issues/{id}/download`.
//!
//! Resolves a present file backing the issue (the same selection the
//! acquisition feed uses to show a download link), then streams it from
//! disk with the archive content-type, a download filename, and the real
//! on-disk length. The file is streamed (not buffered) since comic
//! archives can be hundreds of MB. No filesystem path is ever exposed.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use longbox_db::opds_repo;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use tokio_util::io::ReaderStream;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /opds/v1/issues/{id}/download`.
pub async fn download(
    State(state): State<AppState>,
    Path(issue_id): Path<i64>,
) -> Result<Response, ApiError> {
    let not_found = || ApiError::NotFound {
        resource: "download",
        id: issue_id.to_string(),
    };

    let file = opds_repo::find_downloadable_file(&state.db, issue_id)
        .await?
        .ok_or_else(not_found)?;

    // Defense in depth: the scanner writes root-relative paths, but a
    // malformed/tampered `path_relative` must never stream a file outside
    // the library root. An absolute path or a `..` component would let
    // `Path::join` escape — reject anything that isn't a plain contained
    // relative path before touching the filesystem.
    if !is_contained(&file.path_relative) {
        tracing::warn!(
            issue_id,
            path = %file.path_relative,
            "OPDS download: refused non-contained path"
        );
        return Err(not_found());
    }

    let full_path = std::path::Path::new(&file.root_path).join(&file.path_relative);

    // is_present can lag the filesystem; a row pointing at a vanished file
    // is a 404, not a 500.
    let handle = tokio::fs::File::open(&full_path)
        .await
        .map_err(|_| not_found())?;
    // Use the real on-disk length. If metadata is unreadable, omit
    // Content-Length entirely (chunked) rather than advertise a possibly
    // stale DB size that would truncate or hang strict clients.
    let length = handle.metadata().await.ok().map(|m| m.len());

    let basename = file
        .path_relative
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("comic.cbz");
    let mime = longbox_opds::archive_mime(basename);

    let body = Body::from_stream(ReaderStream::new(handle));

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_DISPOSITION, content_disposition(basename));
    if let Some(len) = length {
        builder = builder.header(CONTENT_LENGTH, len);
    }
    builder
        .body(body)
        .map(IntoResponse::into_response)
        .map_err(|err| ApiError::Internal {
            message: "failed to build download response".to_owned(),
            source: err.into(),
        })
}

/// True only when `path_relative` stays within the library root: a
/// non-empty relative path with no root/prefix component and no `..`
/// escape. Only `Normal` and `CurDir` components are allowed.
fn is_contained(path_relative: &str) -> bool {
    use std::path::Component;
    !path_relative.is_empty()
        && std::path::Path::new(path_relative)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// RFC 6266 `Content-Disposition` with an ASCII-sanitized `filename` and a
/// percent-encoded `filename*` so non-ASCII names round-trip and embedded
/// quotes/control chars can't break the header.
fn content_disposition(filename: &str) -> String {
    let ascii_fallback: String = filename
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() || !c.is_ascii() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let encoded = utf8_percent_encode(filename, NON_ALPHANUMERIC);
    format!("attachment; filename=\"{ascii_fallback}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_allows_plain_relative_paths() {
        assert!(is_contained("Batman/Batman 001.cbz"));
        assert!(is_contained("file.cbz"));
        assert!(is_contained("./a/b.cbz"));
    }

    #[test]
    fn containment_rejects_escapes() {
        assert!(!is_contained("")); // empty
        assert!(!is_contained("/etc/hosts")); // absolute
        assert!(!is_contained("../../../etc/hosts")); // parent traversal
        assert!(!is_contained("a/../../b.cbz")); // mid-path traversal
    }

    #[test]
    fn content_disposition_neutralizes_quotes_and_keeps_basename() {
        let cd = content_disposition(r#"we"ird.cbz"#);
        assert!(cd.contains(r#"filename="we_ird.cbz""#));
        assert!(cd.contains("filename*=UTF-8''"));
    }
}
