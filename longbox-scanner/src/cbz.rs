use std::io::Read;
use std::path::Path;

use crate::error::ScanError;

/// Open a CBZ archive and return the contents of `ComicInfo.xml` as a UTF-8
/// string, or `None` if the archive doesn't carry one (the common case for
/// untagged files). Case-insensitive entry name lookup — some files have
/// `comicinfo.xml`.
///
/// Errors:
/// - `Io` if the file isn't readable, isn't a valid ZIP, or the
///   `ComicInfo.xml` entry exists but isn't valid UTF-8.
///
/// Does NOT extract images or any other archive contents.
pub fn extract_comic_info(cbz_path: &Path) -> Result<Option<String>, ScanError> {
    let file = std::fs::File::open(cbz_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_to_io)?;

    let mut found_idx: Option<usize> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(zip_to_io)?;
        if entry.name().eq_ignore_ascii_case("ComicInfo.xml") {
            found_idx = Some(i);
            break;
        }
    }

    let Some(idx) = found_idx else {
        return Ok(None);
    };

    let mut entry = archive.by_index(idx).map_err(zip_to_io)?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("ComicInfo.xml is not valid UTF-8: {e}"),
        )
    })?;
    Ok(Some(text))
}

fn zip_to_io(e: zip::result::ZipError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("zip: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn write_cbz(dir: &Path, name: &str, comic_info: Option<&str>) -> std::path::PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // Always include a placeholder page so the archive isn't empty.
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
        let result = extract_comic_info(&cbz).unwrap();
        assert_eq!(result.as_deref(), Some(xml));
    }

    #[test]
    fn returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let cbz = write_cbz(tmp.path(), "Untagged.cbz", None);
        let result = extract_comic_info(&cbz).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn finds_lowercase_comicinfo() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Lower.cbz");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("comicinfo.xml", opts).unwrap();
        zip.write_all(b"<ComicInfo/>").unwrap();
        zip.finish().unwrap();
        let result = extract_comic_info(&path).unwrap();
        assert_eq!(result.as_deref(), Some("<ComicInfo/>"));
    }

    #[test]
    fn corrupt_zip_returns_io_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("garbage.cbz");
        std::fs::write(&path, b"this is not a zip file at all").unwrap();
        let err = extract_comic_info(&path).unwrap_err();
        assert!(matches!(err, ScanError::Io(_)));
    }
}
