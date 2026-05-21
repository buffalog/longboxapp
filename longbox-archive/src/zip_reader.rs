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
}
