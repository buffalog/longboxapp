//! Pure-function skip-pattern matcher. Decides whether a path in the
//! watch folder is a candidate for Phase B processing or should be
//! ignored. No I/O — operates on the [`std::path::Path`] alone.
//!
//! The 2-second mtime-stability check from the brief is deliberately
//! **not** included here. Step 5 is detection-only (we never open the
//! file), so racing a mid-write file is harmless. Step 6 will apply
//! the stability check at the point it actually opens the .cbz for
//! ComicInfo write.

use std::path::Path;

/// Why a path was filtered out. Carried for log diagnostics — useful
/// when the user is wondering "why didn't Phase B pick up that file?"
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Filename starts with `.` (.DS_Store, .AppleDouble, etc.).
    Dotfile,
    /// Filename ends with a downloader's in-progress suffix
    /// (`.partial`, `.crdownload`, `.!ut`) — case-insensitive.
    InProgress,
    /// Not a comic-file extension. `.cbz` (ZIP), `.cbr` (RAR) and
    /// `.pdf` are candidates; `.cb7` and everything else land here.
    /// Kept in step with `longbox_scanner::walker` — a format Phase B
    /// files into the library that the scanner then refuses to index
    /// (or the reverse) is worse than not supporting it at all.
    NotComicArchive,
    /// `path.file_name()` returned None (path ends in `..` or `/`,
    /// or is otherwise not a normal file path). Defensive; should
    /// not happen for paths emitted by walkdir or notify.
    NoFilename,
}

/// `None` means: this is a candidate. Phase B should enqueue it.
/// `Some(reason)` means: skip; log the reason for diagnostics.
pub fn should_skip(path: &Path) -> Option<SkipReason> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Some(SkipReason::NoFilename);
    };

    if name.starts_with('.') {
        return Some(SkipReason::Dotfile);
    }

    // In-progress suffixes: case-insensitive ends_with. Some downloaders
    // produce `Foo.cbz.partial`; others produce `Foo.partial`. Both
    // match because we test the suffix on the full filename.
    let lower = name.to_ascii_lowercase();
    for suffix in [".partial", ".crdownload", ".!ut"] {
        if lower.ends_with(suffix) {
            return Some(SkipReason::InProgress);
        }
    }

    if ![".cbz", ".cbr", ".pdf"].iter().any(|e| lower.ends_with(e)) {
        return Some(SkipReason::NotComicArchive);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn cbz_cbr_and_pdf_are_candidates() {
        // Every comic extension, mixed case, absolute paths.
        for n in [
            "Saga 001.cbz",
            "/abs/path/Saga 001.cbz",
            "Saga 001.CBZ",
            "Saga 001.Cbz",
            "Saga 001.cbr",
            "Saga 001.CBR",
            "Saga 001.CbR",
            "Saga 001.pdf",
            "/abs/path/Saga 001.pdf",
            "Saga 001.PDF",
            "Saga 001.Pdf",
        ] {
            assert_eq!(
                should_skip(Path::new(n)),
                None,
                "expected candidate for {n:?}"
            );
        }
    }

    #[test]
    fn non_comic_extensions_skipped() {
        // `.rar` is NOT a comic archive — only `.cbr` is.
        for n in ["foo.cb7", "foo.zip", "foo.rar", "foo.txt", "foo"] {
            assert_eq!(
                should_skip(Path::new(n)),
                Some(SkipReason::NotComicArchive),
                "expected NotComicArchive for {n:?}"
            );
        }
    }

    #[test]
    fn dotfiles_skipped_before_extension_check() {
        for n in [
            ".DS_Store",
            ".AppleDouble",
            ".fuse_hidden0001",
            ".thing.cbz",
        ] {
            assert_eq!(
                should_skip(Path::new(n)),
                Some(SkipReason::Dotfile),
                "expected Dotfile for {n:?}"
            );
        }
    }

    #[test]
    fn in_progress_suffix_catches_both_naming_conventions() {
        // SAB-style: `Foo.partial` (suffix replaces extension).
        // Browser-style: `Foo.cbz.partial` (suffix appended).
        for n in [
            "Saga 001.partial",
            "Saga 001.cbz.partial",
            "Saga 001.crdownload",
            "Saga 001.cbz.crdownload",
            "Saga 001.!ut",
            "Saga 001.cbz.!ut",
            "Saga 001.pdf.partial",
        ] {
            assert_eq!(
                should_skip(Path::new(n)),
                Some(SkipReason::InProgress),
                "expected InProgress for {n:?}"
            );
        }
    }

    #[test]
    fn in_progress_suffix_case_insensitive() {
        for n in [
            "Saga 001.PARTIAL",
            "Saga 001.CrDownload",
            "Saga 001.cbz.PARTIAL",
        ] {
            assert_eq!(
                should_skip(Path::new(n)),
                Some(SkipReason::InProgress),
                "expected InProgress for {n:?}"
            );
        }
    }

    #[test]
    fn dotfile_check_wins_over_other_filters() {
        // A dotfile that happens to look like a partial download is
        // still skipped as Dotfile — order matters for the diagnostic.
        assert_eq!(
            should_skip(Path::new(".Saga 001.cbz.partial")),
            Some(SkipReason::Dotfile)
        );
    }
}
