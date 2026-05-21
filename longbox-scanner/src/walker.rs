use std::path::{Path, PathBuf};

use time::{OffsetDateTime, PrimitiveDateTime};
use walkdir::WalkDir;

use crate::error::ScanError;

/// One discovered comic file from a library walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub path_absolute: PathBuf,
    /// Path relative to the library root with forward slashes. Stable across
    /// macOS/Linux/Docker bind mounts.
    pub path_relative: String,
    pub size_bytes: u64,
    pub mtime: PrimitiveDateTime,
}

/// Walk `root` and yield every `.cbz` and `.cbr` file. Hidden entries
/// (names starting with `.`), `Thumbs.db`, `.cb7` archives, and
/// non-regular files (sockets, fifos, broken symlinks) are silently
/// skipped. Symlink loops are detected by walkdir and surface as
/// `ScanError::Walk`.
pub fn walk_library(root: &Path) -> impl Iterator<Item = Result<DiscoveredFile, ScanError>> + '_ {
    let root_owned = root.to_path_buf();
    WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(should_descend)
        .filter_map(move |entry| dispatch(entry, &root_owned))
}

fn should_descend(entry: &walkdir::DirEntry) -> bool {
    // The root itself (`depth() == 0`) always descends, even if its name
    // starts with `.` — common for temp dirs (`.tmpXXXX`) and some real
    // library mounts.
    if entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    // Hide-dot rule applies to both files and directories; skipping a hidden
    // dir prunes the whole subtree (`.AppleDouble/`, `.git/`, etc.).
    if name.starts_with('.') {
        return false;
    }
    if name.eq_ignore_ascii_case("Thumbs.db") {
        return false;
    }
    true
}

fn dispatch(
    entry: Result<walkdir::DirEntry, walkdir::Error>,
    root: &Path,
) -> Option<Result<DiscoveredFile, ScanError>> {
    let entry = match entry {
        Ok(e) => e,
        Err(e) => return Some(Err(ScanError::Walk(e.to_string()))),
    };
    if !entry.file_type().is_file() {
        return None;
    }
    let name = entry.file_name().to_string_lossy();
    if !is_comic_archive(&name) {
        // CB7 / non-comic files: silently skip.
        return None;
    }
    Some(make_discovered(entry, root))
}

/// True for the comic-archive extensions LongBox indexes — `.cbz` (ZIP)
/// and `.cbr` (RAR), case-insensitive. `.cb7` is excluded: there is no
/// 7-Zip reader.
fn is_comic_archive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".cbz") || lower.ends_with(".cbr")
}

fn make_discovered(entry: walkdir::DirEntry, root: &Path) -> Result<DiscoveredFile, ScanError> {
    let path_absolute = entry.path().to_path_buf();
    let relative = path_absolute
        .strip_prefix(root)
        .map_err(|e| ScanError::InvalidPath {
            path: path_absolute.clone(),
            reason: format!("computed outside library root: {e}"),
        })?;
    let path_relative = relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string();

    let metadata = entry
        .metadata()
        .map_err(|e| ScanError::Walk(e.to_string()))?;
    let size_bytes = metadata.len();
    let modified = metadata.modified()?;
    let off = OffsetDateTime::from(modified);
    // Normalize to UTC and drop the offset for SQLite TEXT storage.
    let off_utc = off.to_offset(time::UtcOffset::UTC);
    let mtime = PrimitiveDateTime::new(off_utc.date(), off_utc.time());

    Ok(DiscoveredFile {
        path_absolute,
        path_relative,
        size_bytes,
        mtime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn skips_hidden_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".DS_Store"), b"junk").unwrap();
        fs::write(root.join("Saga 1.cbz"), b"fake").unwrap();
        let names: Vec<String> = walk_library(root)
            .filter_map(|r| r.ok().map(|d| d.path_relative))
            .collect();
        assert_eq!(names, vec!["Saga 1.cbz"]);
    }

    #[test]
    fn skips_thumbs_db() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("Thumbs.db"), b"junk").unwrap();
        fs::write(root.join("Saga 1.cbz"), b"fake").unwrap();
        let names: Vec<String> = walk_library(root)
            .filter_map(|r| r.ok().map(|d| d.path_relative))
            .collect();
        assert_eq!(names, vec!["Saga 1.cbz"]);
    }

    #[test]
    fn walks_cbz_and_cbr_skips_cb7_and_others() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("Saga 1.cbz"), b"fake").unwrap();
        fs::write(root.join("Saga 1.cbr"), b"fake").unwrap();
        fs::write(root.join("Saga 1.cb7"), b"fake").unwrap();
        fs::write(root.join("README.txt"), b"fake").unwrap();
        let mut names: Vec<String> = walk_library(root)
            .filter_map(|r| r.ok().map(|d| d.path_relative))
            .collect();
        names.sort();
        assert_eq!(names, vec!["Saga 1.cbr", "Saga 1.cbz"]);
    }

    #[test]
    fn matches_extensions_case_insensitively() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("upper.CBZ"), b"fake").unwrap();
        fs::write(root.join("upper.CbR"), b"fake").unwrap();
        let mut names: Vec<String> = walk_library(root)
            .filter_map(|r| r.ok().map(|d| d.path_relative))
            .collect();
        names.sort();
        assert_eq!(names, vec!["upper.CBZ", "upper.CbR"]);
    }

    #[test]
    fn path_relative_uses_forward_slashes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let subdir = root.join("Walking Dead (2003)");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("Walking Dead 001.cbz"), b"fake").unwrap();
        let names: Vec<String> = walk_library(root)
            .filter_map(|r| r.ok().map(|d| d.path_relative))
            .collect();
        assert_eq!(names, vec!["Walking Dead (2003)/Walking Dead 001.cbz"]);
    }

    #[test]
    fn hidden_directory_prunes_subtree() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let hidden = root.join(".AppleDouble");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("Saga 1.cbz"), b"fake").unwrap();
        fs::write(root.join("Saga 2.cbz"), b"fake").unwrap();
        let names: Vec<String> = walk_library(root)
            .filter_map(|r| r.ok().map(|d| d.path_relative))
            .collect();
        assert_eq!(names, vec!["Saga 2.cbz"]);
    }
}
