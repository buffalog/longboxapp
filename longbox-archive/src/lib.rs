//! Archive reading for comic files — CBZ (ZIP) and CBR (RAR).
//!
//! Two operations, each dispatched on the file extension:
//! - [`read_comic_info`] — a targeted read of `ComicInfo.xml`. Used by
//!   the scanner's match cascade and by Phase B's match step.
//! - [`read_entries`] — every file entry, decompressed into memory.
//!   Used by Phase B's repack: a matched CBR is re-emitted as a CBZ,
//!   since `ComicInfo.xml` regeneration needs a writable archive format
//!   and no RAR writer exists.
//!
//! CBZ goes through the `zip` crate; CBR through `unrar-ng` (libunrar).
//! `longbox-scanner` and `longbox-postprocess` both depend on this crate
//! so archive handling lives in exactly one place — before this crate
//! the ZIP-reading logic was duplicated across the two.

pub mod error;
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
}

/// Classify by extension, case-insensitive. `.cb7` and everything else
/// return `None`; callers pre-filter on extension, so `None` here is an
/// error path, not a routine outcome.
fn classify(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "cbz" => Some(Format::Zip),
        "cbr" => Some(Format::Rar),
        _ => None,
    }
}

/// True when `path` is a CBR (RAR). Phase B uses this to choose between
/// its CBZ raw-copy repack and the CBR decompress-and-re-emit repack.
pub fn is_rar(path: &Path) -> bool {
    classify(path) == Some(Format::Rar)
}

/// Read `ComicInfo.xml` (case-insensitive full-name match) from a CBZ or
/// CBR. `Ok(None)` means the archive carries no ComicInfo — the common
/// case for untagged files, not an error.
pub fn read_comic_info(path: &Path) -> Result<Option<String>, ArchiveError> {
    match classify(path).ok_or_else(|| ArchiveError::UnknownFormat(path.to_path_buf()))? {
        Format::Zip => zip_reader::read_comic_info(path),
        Format::Rar => rar_reader::read_comic_info(path),
    }
}

/// Read every file entry of a CBZ or CBR, decompressed into memory.
/// Directory entries are omitted; entry order follows the archive.
pub fn read_entries(path: &Path) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    match classify(path).ok_or_else(|| ArchiveError::UnknownFormat(path.to_path_buf()))? {
        Format::Zip => zip_reader::read_entries(path),
        Format::Rar => rar_reader::read_entries(path),
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
        assert_eq!(classify(Path::new("a.cb7")), None);
        assert_eq!(classify(Path::new("a.txt")), None);
        assert_eq!(classify(Path::new("no-extension")), None);
    }

    #[test]
    fn is_rar_only_for_cbr() {
        assert!(is_rar(Path::new("Saga 1.cbr")));
        assert!(!is_rar(Path::new("Saga 1.cbz")));
        assert!(!is_rar(Path::new("Saga 1.cb7")));
    }

    #[test]
    fn unknown_format_is_an_error() {
        let err = read_comic_info(Path::new("x.cb7")).unwrap_err();
        assert!(matches!(err, ArchiveError::UnknownFormat(_)));
    }
}
