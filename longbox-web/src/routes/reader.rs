//! Built-in comic reader HTTP surface.
//!
//! - `GET /api/issues/:id` — issue record (the reader reads `series_id` from
//!   it for exit navigation).
//! - `GET /api/issues/:id/pages/count` — number of page images in the archive.
//! - `GET /api/issues/:id/pages/:page` — one page image (1-indexed), streamed
//!   with the right content-type.
//! - `GET|PUT /api/issues/:id/reading-progress` — restore/save reader position.
//!
//! Pages are the image entries of the issue's CBZ/CBR, filtered by extension
//! and natural-sorted so "1, 2, 10" order correctly. The archive resolves to
//! the same present file the OPDS download serves (`find_downloadable_file`),
//! guarded by the shared path-containment check. Archive reads are synchronous
//! (zip / libunrar), so they run on `spawn_blocking` to keep the async runtime
//! responsive.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use longbox_archive::ArchiveError;
use longbox_db::{issue_repo, opds_repo, reading_progress_repo, IssueRow};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::pathsafe::is_contained;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/issues/:id", get(issue_detail))
        .route("/issues/:id/pages/count", get(pages_count))
        .route("/issues/:id/pages/:page", get(page_image))
        .route(
            "/issues/:id/reading-progress",
            get(get_progress).put(put_progress),
        )
}

/// Image extensions that count as reader pages (lowercase, no dot).
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif"];

#[derive(Serialize)]
struct PageCount {
    count: usize,
}

#[derive(Serialize, Deserialize)]
struct ReadingProgress {
    last_page: i64,
}

#[derive(Serialize)]
struct OkResponse {
    ok: bool,
}

/// `GET /api/issues/:id` — the raw issue record, incl. `series_id`.
async fn issue_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<i64>,
) -> Result<Json<IssueRow>, ApiError> {
    let issue = issue_repo::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| issue_not_found(id))?;
    Ok(Json(issue))
}

/// `GET /api/issues/:id/pages/count`.
async fn pages_count(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<i64>,
) -> Result<Json<PageCount>, ApiError> {
    let path = resolve_archive(&state, id).await?;
    let pages = run_archive(id, move || list_pages(&path)).await?;
    Ok(Json(PageCount { count: pages.len() }))
}

/// `GET /api/issues/:id/pages/:page` — 1-indexed page image.
async fn page_image(
    State(state): State<AppState>,
    AxumPath((id, page)): AxumPath<(i64, usize)>,
) -> Result<Response, ApiError> {
    if page < 1 {
        return Err(page_not_found(id, page));
    }
    let path = resolve_archive(&state, id).await?;
    let index = page - 1;
    let Some((bytes, mime)) = run_archive(id, move || extract_page(&path, index)).await? else {
        return Err(page_not_found(id, page));
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, mime)
        .body(Body::from(bytes))
        .map(IntoResponse::into_response)
        .map_err(|err| ApiError::Internal {
            message: "failed to build page response".to_owned(),
            source: err.into(),
        })
}

/// `GET /api/issues/:id/reading-progress` — never 404s; a missing row is
/// reported as page 1.
async fn get_progress(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<i64>,
) -> Result<Json<ReadingProgress>, ApiError> {
    let last_page = reading_progress_repo::get_last_page(&state.db, id).await?;
    Ok(Json(ReadingProgress { last_page }))
}

/// `PUT /api/issues/:id/reading-progress` — upsert the reader position.
async fn put_progress(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<i64>,
    Json(body): Json<ReadingProgress>,
) -> Result<Json<OkResponse>, ApiError> {
    reading_progress_repo::set_last_page(&state.db, id, body.last_page).await?;
    Ok(Json(OkResponse { ok: true }))
}

/// Resolve an issue to its on-disk archive path — the present file the OPDS
/// download serves — guarded against path traversal. `NotFound` when the
/// issue has no present file or the stored relative path isn't contained.
async fn resolve_archive(state: &AppState, issue_id: i64) -> Result<PathBuf, ApiError> {
    let file = opds_repo::find_downloadable_file(&state.db, issue_id)
        .await?
        .ok_or_else(|| issue_not_found(issue_id))?;
    if !is_contained(&file.path_relative) {
        tracing::warn!(
            issue_id,
            path = %file.path_relative,
            "reader: refused non-contained path"
        );
        return Err(issue_not_found(issue_id));
    }
    Ok(Path::new(&file.root_path).join(&file.path_relative))
}

/// Run a synchronous archive operation on the blocking pool, folding both the
/// join failure (panic) and the archive error into an `ApiError::Internal`
/// (500 with a descriptive message, per spec).
async fn run_archive<T, F>(issue_id: i64, f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ArchiveError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|err| ApiError::Internal {
            message: format!("archive task failed for issue {issue_id}"),
            source: err.into(),
        })?
        .map_err(|err| ApiError::Internal {
            message: format!("failed to read archive for issue {issue_id}: {err}"),
            source: err.into(),
        })
}

/// The naturally-sorted image page names of an archive. Filters to image
/// extensions, then orders by the leading numeric run of each name's stem
/// (so "2" precedes "10"), with the full name as a stable tiebreak.
fn list_pages(path: &Path) -> Result<Vec<String>, ArchiveError> {
    let mut names: Vec<String> = longbox_archive::list_entry_names(path)?
        .into_iter()
        .filter(|n| is_image(n))
        .collect();
    names.sort_by(|a, b| page_sort_key(a).cmp(&page_sort_key(b)));
    Ok(names)
}

/// Extract the page image at `index` (0-based) from the naturally-sorted page
/// list, with its MIME type. `Ok(None)` when the index is out of range (or the
/// listed entry has since vanished) — the endpoint maps that to 404.
fn extract_page(
    path: &Path,
    index: usize,
) -> Result<Option<(Vec<u8>, &'static str)>, ArchiveError> {
    let pages = list_pages(path)?;
    let Some(name) = pages.get(index) else {
        return Ok(None);
    };
    let mime = mime_for(name).unwrap_or("application/octet-stream");
    Ok(longbox_archive::extract_entry(path, name)?.map(|bytes| (bytes, mime)))
}

/// Lowercase extension of an archive entry name (which may contain `/`), or
/// `None` if it has none.
fn ext_lower(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

fn is_image(name: &str) -> bool {
    ext_lower(name).is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()))
}

fn mime_for(name: &str) -> Option<&'static str> {
    match ext_lower(name)?.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// Natural-sort key: (leading digit run of the basename stem, full name).
/// Zero-padded names ("001, 002, 010") order by the full-name tiebreak;
/// bare-numbered names ("1, 2, 10") order by the numeric prefix.
///
/// ponytail: numeric-prefix-of-stem only — a name like "page10" vs "page2"
/// (unpadded digits after a text prefix) would tie at 0 and fall to
/// lexicographic order. No real comic archive names pages that way; swap in
/// the `alphanumeric-sort` crate if one ever does.
fn page_sort_key(name: &str) -> (u64, &str) {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    (digits.parse().unwrap_or(0), name)
}

fn issue_not_found(id: i64) -> ApiError {
    ApiError::NotFound {
        resource: "issue",
        id: id.to_string(),
    }
}

fn page_not_found(id: i64, page: usize) -> ApiError {
    ApiError::NotFound {
        resource: "page",
        id: format!("{id}/{page}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_to_image_extensions() {
        assert!(is_image("001.jpg"));
        assert!(is_image("cover.JPEG"));
        assert!(is_image("p/002.PNG"));
        assert!(is_image("x.webp"));
        assert!(is_image("y.gif"));
        assert!(!is_image("ComicInfo.xml"));
        assert!(!is_image("notes.txt"));
        assert!(!is_image("noext"));
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(mime_for("a.jpg"), Some("image/jpeg"));
        assert_eq!(mime_for("a.jpeg"), Some("image/jpeg"));
        assert_eq!(mime_for("a.PNG"), Some("image/png"));
        assert_eq!(mime_for("a.webp"), Some("image/webp"));
        assert_eq!(mime_for("a.gif"), Some("image/gif"));
        assert_eq!(mime_for("a.xml"), None);
    }

    #[test]
    fn natural_sort_orders_bare_and_padded_numbers() {
        // Bare numbers: numeric prefix beats lexicographic ("10" would sort
        // before "2" lexically).
        let mut bare = vec!["10.jpg".to_string(), "2.jpg".into(), "1.jpg".into()];
        bare.sort_by(|a, b| page_sort_key(a).cmp(&page_sort_key(b)));
        assert_eq!(bare, vec!["1.jpg", "2.jpg", "10.jpg"]);

        // Zero-padded prefixed names tie at numeric 0, ordered by full name.
        let mut padded = vec![
            "page-010.jpg".to_string(),
            "page-002.jpg".into(),
            "page-001.jpg".into(),
        ];
        padded.sort_by(|a, b| page_sort_key(a).cmp(&page_sort_key(b)));
        assert_eq!(padded, vec!["page-001.jpg", "page-002.jpg", "page-010.jpg"]);
    }
}
