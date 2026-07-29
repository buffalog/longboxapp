//! CBZ (ZIP) archive reading via the `zip` crate.

use std::io::Read;
use std::path::Path;

use crate::error::ArchiveError;
use crate::ArchiveEntry;

/// The metadata entry name, matched case-insensitively — some files
/// ship `comicinfo.xml`.
const COMIC_INFO: &str = "ComicInfo.xml";

/// Targeted read: find `ComicInfo.xml` and return just its contents.
/// `Ok(None)` if the archive has no such entry.
pub(crate) fn read_comic_info(path: &Path) -> Result<Option<String>, ArchiveError> {
    let file = std::fs::File::open(path).map_err(|source| io_err(path, source))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| zip_err(path, e))?;

    let mut found: Option<usize> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| zip_err(path, e))?;
        if entry.name().eq_ignore_ascii_case(COMIC_INFO) {
            found = Some(i);
            break;
        }
    }
    let Some(idx) = found else {
        return Ok(None);
    };

    let mut entry = archive.by_index(idx).map_err(|e| zip_err(path, e))?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|source| io_err(path, source))?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(e) => Err(ArchiveError::Zip {
            path: path.to_path_buf(),
            message: format!("ComicInfo.xml is not valid UTF-8: {e}"),
        }),
    }
}

/// Read every file entry decompressed into memory. Directory entries
/// are skipped.
pub(crate) fn read_entries(path: &Path) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let file = std::fs::File::open(path).map_err(|source| io_err(path, source))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| zip_err(path, e))?;

    let mut entries = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| zip_err(path, e))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut data)
            .map_err(|source| io_err(path, source))?;
        entries.push(ArchiveEntry { name, data });
    }
    Ok(entries)
}

/// ZIP stores whatever separator the writer used, and archives written on
/// Windows tooling really do carry `\`. The RAR reader has always normalized;
/// this one did not, so a backslash CBZ produced entry names that no consumer
/// could split into directory and basename — which reads to the archive-label
/// parser as one long filename, gluing the folder onto the series text and
/// manufacturing a "filed under the wrong series" claim out of a separator.
fn normalize_separators(name: &str) -> String {
    name.replace('\\', "/")
}

/// List the names of every file entry (directories skipped), in archive
/// order, with `/` separators. Only the central directory is read — no entry
/// data is decompressed.
pub(crate) fn list_entry_names(path: &Path) -> Result<Vec<String>, ArchiveError> {
    let file = std::fs::File::open(path).map_err(|source| io_err(path, source))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| zip_err(path, e))?;

    let mut names = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| zip_err(path, e))?;
        if entry.is_dir() {
            continue;
        }
        names.push(normalize_separators(entry.name()));
    }
    Ok(names)
}

/// Extract one entry's bytes by name. `Ok(None)` when no such entry exists.
///
/// Matches on the SEPARATOR-NORMALIZED name, not the raw stored one, so a
/// name handed back by [`list_entry_names`] always round-trips. Normalizing
/// only the listing side would have broken page serving for backslash
/// archives: the reader lists names and then asks for one of them back.
pub(crate) fn extract_entry(path: &Path, name: &str) -> Result<Option<Vec<u8>>, ArchiveError> {
    let file = std::fs::File::open(path).map_err(|source| io_err(path, source))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| zip_err(path, e))?;

    let wanted = normalize_separators(name);
    let mut found = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| zip_err(path, e))?;
        if !entry.is_dir() && normalize_separators(entry.name()) == wanted {
            found = Some(i);
            break;
        }
    }
    let Some(idx) = found else {
        return Ok(None);
    };

    let mut entry = archive.by_index(idx).map_err(|e| zip_err(path, e))?;
    let mut data = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut data)
        .map_err(|source| io_err(path, source))?;
    Ok(Some(data))
}

fn io_err(path: &Path, source: std::io::Error) -> ArchiveError {
    ArchiveError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn zip_err(path: &Path, e: zip::result::ZipError) -> ArchiveError {
    ArchiveError::Zip {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    /// Write a CBZ with one placeholder page and (optionally) a
    /// ComicInfo.xml entry.
    fn write_cbz(dir: &Path, name: &str, comic_info: Option<&str>) -> std::path::PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("page-001.jpg", opts).unwrap();
        zip.write_all(b"\xFF\xD8\xFF").unwrap();
        if let Some(xml) = comic_info {
            zip.start_file("ComicInfo.xml", opts).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn extracts_comic_info_when_present() {
        let tmp = TempDir::new().unwrap();
        let xml = r#"<?xml version="1.0"?><ComicInfo><Series>Saga</Series></ComicInfo>"#;
        let cbz = write_cbz(tmp.path(), "Saga 1.cbz", Some(xml));
        assert_eq!(read_comic_info(&cbz).unwrap().as_deref(), Some(xml));
    }

    #[test]
    fn returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let cbz = write_cbz(tmp.path(), "Untagged.cbz", None);
        assert!(read_comic_info(&cbz).unwrap().is_none());
    }

    #[test]
    fn finds_lowercase_comicinfo() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Lower.cbz");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("comicinfo.xml", opts).unwrap();
        zip.write_all(b"<ComicInfo/>").unwrap();
        zip.finish().unwrap();
        assert_eq!(
            read_comic_info(&path).unwrap().as_deref(),
            Some("<ComicInfo/>")
        );
    }

    #[test]
    fn corrupt_zip_is_a_zip_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("garbage.cbz");
        std::fs::write(&path, b"this is not a zip file at all").unwrap();
        let err = read_comic_info(&path).unwrap_err();
        assert!(matches!(err, ArchiveError::Zip { .. }));
    }

    #[test]
    fn read_entries_returns_every_file() {
        let tmp = TempDir::new().unwrap();
        let xml = "<ComicInfo/>";
        let cbz = write_cbz(tmp.path(), "Saga 1.cbz", Some(xml));
        let entries = read_entries(&cbz).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["page-001.jpg", "ComicInfo.xml"]);
        assert_eq!(entries[0].data, b"\xFF\xD8\xFF");
        assert_eq!(entries[1].data, xml.as_bytes());
    }

    /// Write a CBZ with the given entries (name, bytes), in order.
    fn write_cbz_entries(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (entry_name, bytes) in entries {
            zip.start_file(*entry_name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn list_entry_names_lists_files_in_order() {
        let tmp = TempDir::new().unwrap();
        let cbz = write_cbz_entries(
            tmp.path(),
            "Multi.cbz",
            &[
                ("001.jpg", b"a"),
                ("002.jpg", b"b"),
                ("ComicInfo.xml", b"<x/>"),
            ],
        );
        assert_eq!(
            list_entry_names(&cbz).unwrap(),
            vec!["001.jpg", "002.jpg", "ComicInfo.xml"]
        );
    }

    #[test]
    fn extract_entry_returns_bytes_or_none() {
        let tmp = TempDir::new().unwrap();
        let cbz = write_cbz_entries(tmp.path(), "Multi.cbz", &[("001.jpg", b"\xFF\xD8\xFF")]);
        assert_eq!(
            extract_entry(&cbz, "001.jpg").unwrap().as_deref(),
            Some(&b"\xFF\xD8\xFF"[..])
        );
        assert!(extract_entry(&cbz, "missing.jpg").unwrap().is_none());
    }
}
