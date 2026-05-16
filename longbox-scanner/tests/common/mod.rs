//! Shared test helpers: pool setup, fixture library builder, DB seeding.

use std::fs;
use std::io::Write;
use std::path::Path;

use longbox_db::{
    issue_repo, library_root_repo, series_repo, NewIssue, NewLibraryRoot, NewSeries, Pool,
    SeriesRow,
};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

#[allow(dead_code)]
pub async fn fresh_pool() -> Pool {
    longbox_db::open(":memory:")
        .await
        .expect("open in-memory pool with migrations applied")
}

/// Write a CBZ archive containing one placeholder page and (optionally) a
/// ComicInfo.xml entry with the given body.
#[allow(dead_code)]
pub fn write_cbz(path: &Path, comic_info: Option<&str>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let file = fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("page-001.jpg", opts).unwrap();
    zip.write_all(b"\xFF\xD8\xFF\xE0").unwrap();
    if let Some(xml) = comic_info {
        zip.start_file("ComicInfo.xml", opts).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

/// Builds the canonical test library inside `root`:
/// - `Walking Dead (2003)/Walking Dead 001 (2003).cbz`  — ComicInfo with CV `<Web>`
/// - `Walking Dead (2003)/Walking Dead 002 (2003).cbz`  — no ComicInfo (filename-only)
/// - `Walking Dead (2003)/Walking Dead 003 (2003).cbz`  — ComicInfo without `<Web>`
/// - `Saga (2012)/Saga 001 (2012).cbz`                  — different series
/// - `Mystery/UnknownComic.cbz`                         — won't match
/// - `Walking Dead (2003)/corrupt.cbz`                  — invalid zip bytes
/// - `Walking Dead (2003)/.DS_Store`                    — should be skipped
/// - `Walking Dead (2003)/cover.cbr`                    — should be skipped
#[allow(dead_code)]
pub fn build_fixture_library(root: &Path) {
    let wd_dir = root.join("Walking Dead (2003)");

    write_cbz(
        &wd_dir.join("Walking Dead 001 (2003).cbz"),
        Some(
            r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>The Walking Dead</Series>
  <Number>1</Number>
  <Volume>2003</Volume>
  <Web>https://comicvine.gamespot.com/issue/4000-101001/</Web>
</ComicInfo>"#,
        ),
    );

    write_cbz(&wd_dir.join("Walking Dead 002 (2003).cbz"), None);

    write_cbz(
        &wd_dir.join("Walking Dead 003 (2003).cbz"),
        Some(
            r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>The Walking Dead</Series>
  <Number>3</Number>
  <Volume>2003</Volume>
</ComicInfo>"#,
        ),
    );

    write_cbz(
        &root
            .join("Saga (2012)")
            .join("Saga 001 (2012).cbz"),
        Some(
            r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Saga</Series>
  <Number>1</Number>
  <Volume>2012</Volume>
</ComicInfo>"#,
        ),
    );

    write_cbz(
        &root.join("Mystery").join("UnknownComic.cbz"),
        None,
    );

    // Corrupt CBZ: random bytes that aren't a valid ZIP.
    let corrupt = wd_dir.join("corrupt.cbz");
    fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    fs::write(&corrupt, b"this is not a valid zip archive at all").unwrap();

    // Hidden file: should be skipped.
    fs::write(wd_dir.join(".DS_Store"), b"junk").unwrap();

    // CBR: should be silently skipped (we don't extract RAR).
    fs::write(wd_dir.join("cover.cbr"), b"fake rar").unwrap();
}

/// Seed the watchlist: The Walking Dead (2003) with 3 issues (one carrying
/// the CV ID 101001 the fixture's Tier 1 URL points to). Saga is NOT seeded
/// by default so individual tests can decide whether to include it.
#[allow(dead_code)]
pub async fn seed_walking_dead(pool: &Pool) -> SeriesRow {
    let s = series_repo::insert(
        pool,
        NewSeries {
            cv_id: Some(2127),
            metron_id: None,
            title: "The Walking Dead".into(),
            sort_title: "walking dead".into(),
            start_year: Some(2003),
            publisher: Some("Image Comics".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    for (number, cv_id) in [("1", 101_001_i64), ("2", 101_002), ("3", 101_003)] {
        issue_repo::insert(
            pool,
            NewIssue {
                series_id: s.id,
                cv_issue_id: Some(cv_id),
                metron_issue_id: None,
                number: number.into(),
                title: None,
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }

    s
}

#[allow(dead_code)]
pub async fn seed_library_root(pool: &Pool, path: &str) -> i64 {
    library_root_repo::insert(
        pool,
        NewLibraryRoot {
            path: path.into(),
        },
    )
    .await
    .unwrap()
    .id
}
