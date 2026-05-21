//! CBR (RAR) archive reading via `unrar-ng` (libunrar bindings).
//!
//! `unrar-ng` uses a move-based cursor API: each `read_header` /
//! `read` / `skip` consumes the archive handle and returns the next
//! state. The loop below threads that handle through.

use std::path::Path;

use unrar_ng::Archive;

use crate::error::ArchiveError;
use crate::ArchiveEntry;

/// The metadata entry name, matched case-insensitively.
const COMIC_INFO: &str = "ComicInfo.xml";

/// Targeted read: walk headers, extract only `ComicInfo.xml`, skip the
/// rest. `Ok(None)` if the archive has no such entry.
pub(crate) fn read_comic_info(path: &Path) -> Result<Option<String>, ArchiveError> {
    let mut archive = Archive::new(path)
        .open_for_processing()
        .map_err(|e| rar_err(path, e))?;

    while let Some(header) = archive.read_header().map_err(|e| rar_err(path, e))? {
        // Inspect the entry, then release the borrow before the cursor
        // move (`read`/`skip` consume `header`).
        let (is_file, name) = {
            let entry = header.entry();
            // RAR may store `\` separators; normalize to `/` so a
            // CBR re-emitted as a CBZ keeps standard ZIP entry names.
            (
                entry.is_file(),
                entry.filename.to_string_lossy().replace('\\', "/"),
            )
        };
        if is_file && name.eq_ignore_ascii_case(COMIC_INFO) {
            let (data, _rest) = header.read().map_err(|e| rar_err(path, e))?;
            return match String::from_utf8(data) {
                Ok(text) => Ok(Some(text)),
                Err(e) => Err(ArchiveError::Rar {
                    path: path.to_path_buf(),
                    message: format!("ComicInfo.xml is not valid UTF-8: {e}"),
                }),
            };
        }
        archive = header.skip().map_err(|e| rar_err(path, e))?;
    }
    Ok(None)
}

/// Read every file entry decompressed into memory. Directory entries
/// are skipped.
pub(crate) fn read_entries(path: &Path) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let mut archive = Archive::new(path)
        .open_for_processing()
        .map_err(|e| rar_err(path, e))?;

    let mut entries = Vec::new();
    while let Some(header) = archive.read_header().map_err(|e| rar_err(path, e))? {
        let (is_file, name) = {
            let entry = header.entry();
            // RAR may store `\` separators; normalize to `/` so a
            // CBR re-emitted as a CBZ keeps standard ZIP entry names.
            (
                entry.is_file(),
                entry.filename.to_string_lossy().replace('\\', "/"),
            )
        };
        archive = if is_file {
            let (data, rest) = header.read().map_err(|e| rar_err(path, e))?;
            entries.push(ArchiveEntry { name, data });
            rest
        } else {
            header.skip().map_err(|e| rar_err(path, e))?
        };
    }
    Ok(entries)
}

fn rar_err<E: std::fmt::Display>(path: &Path, e: E) -> ArchiveError {
    ArchiveError::Rar {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}
