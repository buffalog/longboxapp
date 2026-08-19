//! Archive reading for comic files — CBZ (ZIP), CBR (RAR) and PDF.
//!
//! Two operations, each dispatched on the file extension:
//! - [`read_comic_info`] — a targeted read of `ComicInfo.xml`. Used by
//!   the scanner's match cascade and by Phase B's match step.
//! - [`read_entries`] — every file entry, decompressed into memory.
//!   Used by Phase B's repack: a matched CBR is re-emitted as a CBZ,
//!   since `ComicInfo.xml` regeneration needs a writable archive format
//!   and no RAR writer exists.
//!
//! CBZ goes through the `zip` crate; CBR through `unrar-ng` (libunrar);
//! PDF through poppler-utils (`pdfinfo`/`pdftoppm`).
//! `longbox-scanner` and `longbox-postprocess` both depend on this crate
//! so archive handling lives in exactly one place — before this crate
//! the ZIP-reading logic was duplicated across the two.
//!
//! PDF is the odd one out in two ways, both of which callers must respect:
//! its "entries" are *rendered* on demand rather than decompressed, so its
//! entry names are synthetic (`page_001.jpg`) and carry no information
//! about the file's provenance; and it is permanently read-only — LongBox
//! never writes into a PDF, so the repack path must never receive one.

pub mod error;
mod pdf_reader;
mod rar_reader;
mod zip_reader;

use std::path::Path;

pub use error::ArchiveError;

/// One archive entry: its name (path within the archive) and its
/// decompressed bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub name: String,
    pub data: Vec<u8>,
}

/// The comic-archive container formats LongBox reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// `.cbz` — a ZIP archive.
    Zip,
    /// `.cbr` — a RAR archive.
    Rar,
    /// `.pdf` — not an archive at all; pages are rendered, not extracted.
    Pdf,
}

/// Classify by extension, case-insensitive. `.cb7` and everything else
/// return `None`; callers pre-filter on extension, so `None` here is an
/// error path, not a routine outcome.
fn classify(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "cbz" => Some(Format::Zip),
        "cbr" => Some(Format::Rar),
        "pdf" => Some(Format::Pdf),
        _ => None,
    }
}

/// True when `path` is a PDF. Callers that write — the repack path, the
/// ComicInfo embed — use this to refuse: a PDF is read-only forever.
pub fn is_pdf(path: &Path) -> bool {
    classify(path) == Some(Format::Pdf)
}

/// True when `path` is a CBR (RAR). Phase B uses this to choose between
/// its CBZ raw-copy repack and the CBR decompress-and-re-emit repack.
pub fn is_rar(path: &Path) -> bool {
    classify(path) == Some(Format::Rar)
}

/// Read `ComicInfo.xml` (case-insensitive full-name match) from a CBZ or
/// CBR. `Ok(None)` means the archive carries no ComicInfo — the common
/// case for untagged files, not an error. Always `Ok(None)` for a PDF,
/// which has no embedded-metadata equivalent.
pub fn read_comic_info(path: &Path) -> Result<Option<String>, ArchiveError> {
    match classify(path).ok_or_else(|| ArchiveError::UnknownFormat(path.to_path_buf()))? {
        Format::Zip => zip_reader::read_comic_info(path),
        Format::Rar => rar_reader::read_comic_info(path),
        Format::Pdf => pdf_reader::read_comic_info(path),
    }
}

/// Read every file entry of a CBZ or CBR, decompressed into memory.
/// Directory entries are omitted; entry order follows the archive. For a
/// PDF this renders every page, which is expensive — the repack path that
/// calls it never runs for a PDF.
pub fn read_entries(path: &Path) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    match classify(path).ok_or_else(|| ArchiveError::UnknownFormat(path.to_path_buf()))? {
        Format::Zip => zip_reader::read_entries(path),
        Format::Rar => rar_reader::read_entries(path),
        Format::Pdf => pdf_reader::read_entries(path),
    }
}

/// List the names of every file entry (directories omitted), in archive
/// order — no entry data is decompressed. The reader uses this to enumerate
/// pages cheaply, then pulls individual pages with [`extract_entry`]. Names
/// use `/` separators (RAR `\` separators are normalized).
///
/// For a PDF the names are SYNTHESIZED (`page_001.jpg`, one per page) — the
/// file contains no such entries. Consumers that read entry names as
/// evidence about the file's origin, rather than as page handles, must
/// exclude PDFs themselves: `longbox_core::archive_label` only ever sees
/// names, so it cannot tell a synthesized one from a real one. Today the
/// only such consumer is `longbox_web::content_hash::read_archive_label`,
/// and it does NOT yet exclude them.
pub fn list_entry_names(path: &Path) -> Result<Vec<String>, ArchiveError> {
    match classify(path).ok_or_else(|| ArchiveError::UnknownFormat(path.to_path_buf()))? {
        Format::Zip => zip_reader::list_entry_names(path),
        Format::Rar => rar_reader::list_entry_names(path),
        Format::Pdf => pdf_reader::list_entry_names(path),
    }
}

/// Extract a single file entry by its exact name (as returned by
/// [`list_entry_names`]), decompressing only that entry. `Ok(None)` if no
/// entry matches — the reader treats that as a 404 rather than an error.
/// For a PDF this renders the named page rather than decompressing it.
pub fn extract_entry(path: &Path, name: &str) -> Result<Option<Vec<u8>>, ArchiveError> {
    match classify(path).ok_or_else(|| ArchiveError::UnknownFormat(path.to_path_buf()))? {
        Format::Zip => zip_reader::extract_entry(path, name),
        Format::Rar => rar_reader::extract_entry(path, name),
        Format::Pdf => pdf_reader::extract_entry(path, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_is_case_insensitive() {
        assert_eq!(classify(Path::new("a.cbz")), Some(Format::Zip));
        assert_eq!(classify(Path::new("a.CBZ")), Some(Format::Zip));
        assert_eq!(classify(Path::new("a.cbr")), Some(Format::Rar));
        assert_eq!(classify(Path::new("a.CbR")), Some(Format::Rar));
        assert_eq!(classify(Path::new("a.pdf")), Some(Format::Pdf));
        assert_eq!(classify(Path::new("a.PDF")), Some(Format::Pdf));
        assert_eq!(classify(Path::new("a.cb7")), None);
        assert_eq!(classify(Path::new("a.txt")), None);
        assert_eq!(classify(Path::new("no-extension")), None);
    }

    #[test]
    fn is_rar_only_for_cbr() {
        assert!(is_rar(Path::new("Saga 1.cbr")));
        assert!(!is_rar(Path::new("Saga 1.cbz")));
        assert!(!is_rar(Path::new("Saga 1.cb7")));
        assert!(!is_rar(Path::new("Saga 1.pdf")));
    }

    #[test]
    fn is_pdf_only_for_pdf() {
        assert!(is_pdf(Path::new("Legacy 1.pdf")));
        assert!(is_pdf(Path::new("Legacy 1.PDF")));
        assert!(!is_pdf(Path::new("Legacy 1.cbz")));
        assert!(!is_pdf(Path::new("Legacy 1.cbr")));
    }

    /// A PDF carries no ComicInfo equivalent, and this is the untagged
    /// path every untagged CBZ already takes — not an error.
    #[test]
    fn pdf_comic_info_is_none_not_an_error() {
        assert!(read_comic_info(Path::new("anything.pdf"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn unknown_format_is_an_error() {
        let err = read_comic_info(Path::new("x.cb7")).unwrap_err();
        assert!(matches!(err, ArchiveError::UnknownFormat(_)));
    }
}
