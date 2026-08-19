//! Error type for archive reading. Both `longbox-scanner` and
//! `longbox-postprocess` fold this into their own error enums via
//! `#[from]`, so a CBZ or CBR read failure propagates with `?`.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The file could not be opened or read at the filesystem level.
    #[error("archive I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The path's extension is none of `.cbz`, `.cbr`, `.pdf`. Callers
    /// that pre-filter by extension should never see this.
    #[error("unrecognized archive type (expected .cbz, .cbr or .pdf): {0}")]
    UnknownFormat(PathBuf),

    /// The file claims to be a CBZ but isn't a valid ZIP, or a ZIP
    /// operation failed mid-read.
    #[error("zip error for {path}: {message}")]
    Zip { path: PathBuf, message: String },

    /// The file claims to be a CBR but isn't a valid RAR, or a RAR
    /// operation failed mid-read.
    #[error("rar error for {path}: {message}")]
    Rar { path: PathBuf, message: String },

    /// A poppler-utils call failed: the file isn't a readable PDF, a page
    /// would not render, or `pdfinfo`/`pdftoppm` is not installed. The
    /// message names the tool so a missing dependency is distinguishable
    /// from a bad file.
    #[error("pdf error for {path}: {message}")]
    Pdf { path: PathBuf, message: String },
}
