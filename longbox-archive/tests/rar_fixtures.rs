//! RAR-path integration tests, against vendored `.cbr` fixtures
//! covering both container formats — RAR4 and RAR5. The CBZ paths are
//! unit-tested inline in `zip_reader.rs`; this file is the RAR
//! (libunrar / `unrar-ng`) coverage.
//!
//! See `tests/fixtures/REGENERATE.md` for how the fixtures were made.

use std::path::{Path, PathBuf};

use longbox_archive::{read_comic_info, read_entries};

/// The exact `ComicInfo.xml` body baked into both fixtures.
const EXPECTED_COMIC_INFO: &str =
    r#"<?xml version="1.0"?><ComicInfo><Series>Saga</Series><Number>1</Number></ComicInfo>"#;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn reads_comic_info_from_rar4() {
    let xml = read_comic_info(&fixture("sample-rar4.cbr")).unwrap();
    assert_eq!(xml.as_deref(), Some(EXPECTED_COMIC_INFO));
}

#[test]
fn reads_comic_info_from_rar5() {
    let xml = read_comic_info(&fixture("sample-rar5.cbr")).unwrap();
    assert_eq!(xml.as_deref(), Some(EXPECTED_COMIC_INFO));
}

#[test]
fn reads_all_entries_from_rar4() {
    let entries = read_entries(&fixture("sample-rar4.cbr")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["page-001.jpg", "ComicInfo.xml"]);
}

#[test]
fn reads_all_entries_from_rar5() {
    let entries = read_entries(&fixture("sample-rar5.cbr")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["page-001.jpg", "ComicInfo.xml"]);
    // Entries come back decompressed — the ComicInfo bytes round-trip.
    assert_eq!(entries[1].data, EXPECTED_COMIC_INFO.as_bytes());
}
