//! PDF reading via poppler-utils (`pdfinfo`, `pdftoppm`).
//!
//! Unlike CBZ/CBR, a PDF has no pre-existing image entries — pages are
//! *rendered*, not decompressed. So the entry names this module reports
//! are synthetic (`page_001.jpg`, …), one per page, and [`extract_entry`]
//! renders the named page on demand.
//!
//! Shelling out to poppler rather than binding a Rust PDF crate: poppler
//! packages identically on both arches of the multi-arch image, and a
//! Rust binding would wrap the same C++ anyway, with more cross-compile
//! risk for no gain.

use std::path::Path;
use std::process::Command;

use crate::error::ArchiveError;
use crate::ArchiveEntry;

/// Render resolution, dots per inch. 150 is the usual scan-quality floor
/// for comic pages and keeps a rendered page in the low hundreds of KB.
const RENDER_DPI: &str = "150";

/// A PDF never carries a ComicInfo.xml equivalent, so this is always
/// `Ok(None)` — the same path already taken by every untagged CBZ.
/// Deliberately does not open the file: there is nothing to look for.
pub(crate) fn read_comic_info(_path: &Path) -> Result<Option<String>, ArchiveError> {
    Ok(None)
}

/// The synthetic name for a 1-indexed page, at a given pad width.
fn page_name(page: usize, width: usize) -> String {
    format!("page_{page:0width$}.jpg")
}

/// Synthetic page names, one per page, in page order.
pub(crate) fn list_entry_names(path: &Path) -> Result<Vec<String>, ArchiveError> {
    let count = page_count(path)?;
    let width = name_width(count);
    Ok((1..=count).map(|n| page_name(n, width)).collect())
}

/// Render every page. Used only by the repack path, which never runs for
/// a PDF — kept so the format implements the full reader interface.
///
/// Renders from the same range that names the pages, rather than parsing
/// the page back out of a name it just formatted. The round trip was not
/// wrong, but nothing could observe it going wrong: a name/render mismatch
/// still yields a valid JPEG per entry.
pub(crate) fn read_entries(path: &Path) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let count = page_count(path)?;
    let width = name_width(count);
    (1..=count)
        .map(|n| {
            render_page(path, n).map(|data| ArchiveEntry {
                name: page_name(n, width),
                data,
            })
        })
        .collect()
}

/// Render the named page to JPEG. `Ok(None)` when the name is not one this
/// module would have synthesized, or names a page past the end.
///
/// ponytail: renders on every call, no cache. A page render is ~100-300ms
/// and already runs on `spawn_blocking`. If that ever measurably hurts,
/// cache to a temp dir keyed by (path, mtime, page, dpi).
pub(crate) fn extract_entry(path: &Path, name: &str) -> Result<Option<Vec<u8>>, ArchiveError> {
    let Some(page) = page_number(name) else {
        return Ok(None);
    };
    if page < 1 || page > page_count(path)? {
        return Ok(None);
    }
    render_page(path, page).map(Some)
}

/// Page count via `pdfinfo`, read off its `Pages:` line.
fn page_count(path: &Path) -> Result<usize, ArchiveError> {
    let out = run(Command::new("pdfinfo").arg(path), path)?;
    let text = String::from_utf8_lossy(&out);
    text.lines()
        .find_map(|line| line.strip_prefix("Pages:"))
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| pdf_err(path, "pdfinfo reported no page count"))
}

/// Render one 1-indexed page to JPEG bytes. With no output prefix,
/// `pdftoppm` writes the image to stdout.
fn render_page(path: &Path, page: usize) -> Result<Vec<u8>, ArchiveError> {
    let n = page.to_string();
    let out = run(
        Command::new("pdftoppm")
            .args(["-jpeg", "-r", RENDER_DPI, "-f", &n, "-l", &n])
            .arg(path),
        path,
    )?;
    if out.is_empty() {
        return Err(pdf_err(
            path,
            format!("pdftoppm rendered page {page} empty"),
        ));
    }
    Ok(out)
}

/// Run a poppler command, returning stdout. A non-zero exit or a missing
/// binary both surface as [`ArchiveError::Pdf`] naming the tool, so an
/// install gap reads as an install gap rather than a corrupt file. The
/// tool name is read off the command so the two cannot disagree.
fn run(cmd: &mut Command, path: &Path) -> Result<Vec<u8>, ArchiveError> {
    let tool = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd
        .output()
        .map_err(|e| pdf_err(path, format!("could not run {tool} (poppler-utils): {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(pdf_err(
            path,
            format!("{tool} failed ({}): {}", out.status, stderr.trim()),
        ));
    }
    Ok(out.stdout)
}

/// Zero-pad width for synthesized names: enough digits for the page count,
/// but never fewer than 3 — `page_001.jpg` is the comic-page convention and
/// real books effectively never exceed 999 pages.
fn name_width(count: usize) -> usize {
    count.to_string().len().max(3)
}

/// The 1-indexed page a synthesized name refers to, or `None` if the name
/// is not one [`list_entry_names`] would have produced. Padding is not
/// re-checked: `page_7.jpg` and `page_007.jpg` both mean page 7.
///
/// Digits only — `usize::from_str` would otherwise accept a leading `+`,
/// admitting `page_+7.jpg`, a name this module can never emit.
fn page_number(name: &str) -> Option<usize> {
    let digits = name.strip_prefix("page_")?.strip_suffix(".jpg")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn pdf_err(path: &Path, message: impl Into<String>) -> ArchiveError {
    ArchiveError::Pdf {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Write a minimal valid PDF with `pages` pages. Hand-rolled rather
    /// than pulled from a crate: the byte layout is a dozen lines and a
    /// real fixture keeps poppler in the loop for every assertion.
    ///
    /// Each page gets a DIFFERENT height on purpose. With identical pages
    /// every render is byte-identical, so a page-selection bug (wrong
    /// `-f`/`-l`, off-by-one) would still produce a valid JPEG and every
    /// assertion would pass. Distinct heights make the rendered bytes
    /// distinguishable, which is what lets a test detect that at all.
    fn write_pdf(dir: &Path, name: &str, pages: usize) -> std::path::PathBuf {
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + i)).collect();
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                pages
            ),
        ];
        objs.extend((0..pages).map(|i| {
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 {}] >>",
                150 + i * 120
            )
        }));

        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objs.len());
        for (i, obj) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );

        let path = dir.join(name);
        std::fs::write(&path, out).unwrap();
        path
    }

    #[test]
    fn comic_info_is_always_none() {
        let tmp = TempDir::new().unwrap();
        let pdf = write_pdf(tmp.path(), "Legacy 1.pdf", 2);
        assert!(read_comic_info(&pdf).unwrap().is_none());
    }

    #[test]
    fn entry_names_are_one_per_page_zero_padded() {
        let tmp = TempDir::new().unwrap();
        let pdf = write_pdf(tmp.path(), "Legacy 1.pdf", 3);
        assert_eq!(
            list_entry_names(&pdf).unwrap(),
            vec!["page_001.jpg", "page_002.jpg", "page_003.jpg"]
        );
    }

    #[test]
    fn entry_names_widen_past_three_digits() {
        assert_eq!(name_width(3), 3);
        assert_eq!(name_width(999), 3);
        assert_eq!(name_width(1000), 4);
    }

    #[test]
    fn extract_entry_renders_the_named_page_as_jpeg() {
        let tmp = TempDir::new().unwrap();
        let pdf = write_pdf(tmp.path(), "Legacy 1.pdf", 3);
        let bytes = extract_entry(&pdf, "page_002.jpg").unwrap().expect("page");
        assert_eq!(&bytes[..2], b"\xFF\xD8", "not a JPEG: {:?}", &bytes[..4]);
    }

    /// Each name must render ITS page, not just some page. The fixture's
    /// pages differ in height, so three identical renders would mean page
    /// selection is being ignored — the failure a JPEG-magic-bytes
    /// assertion alone cannot see.
    #[test]
    fn each_name_renders_its_own_page() {
        let tmp = TempDir::new().unwrap();
        let pdf = write_pdf(tmp.path(), "Legacy 1.pdf", 3);
        let render = |n: &str| extract_entry(&pdf, n).unwrap().expect("page");
        let (p1, p2, p3) = (
            render("page_001.jpg"),
            render("page_002.jpg"),
            render("page_003.jpg"),
        );
        assert_ne!(p1, p2);
        assert_ne!(p2, p3);
        assert_ne!(p1, p3);
    }

    #[test]
    fn extract_entry_is_none_for_a_page_past_the_end() {
        let tmp = TempDir::new().unwrap();
        let pdf = write_pdf(tmp.path(), "Legacy 1.pdf", 3);
        assert!(extract_entry(&pdf, "page_009.jpg").unwrap().is_none());
        assert!(extract_entry(&pdf, "page_000.jpg").unwrap().is_none());
    }

    #[test]
    fn extract_entry_is_none_for_a_name_we_never_synthesize() {
        let tmp = TempDir::new().unwrap();
        let pdf = write_pdf(tmp.path(), "Legacy 1.pdf", 3);
        assert!(extract_entry(&pdf, "ComicInfo.xml").unwrap().is_none());
        assert!(extract_entry(&pdf, "001.jpg").unwrap().is_none());
        // `usize::from_str` accepts a leading `+`; this module never emits
        // one, so the name must be rejected rather than aliased to page 2.
        assert!(extract_entry(&pdf, "page_+2.jpg").unwrap().is_none());
        assert!(extract_entry(&pdf, "page_.jpg").unwrap().is_none());
    }

    #[test]
    fn read_entries_renders_every_page() {
        let tmp = TempDir::new().unwrap();
        let pdf = write_pdf(tmp.path(), "Legacy 1.pdf", 2);
        let entries = read_entries(&pdf).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["page_001.jpg", "page_002.jpg"]);
        assert!(entries.iter().all(|e| e.data.starts_with(b"\xFF\xD8")));
        // Names and JPEG magic bytes both survive rendering the same page
        // under every name; only comparing the bytes catches that.
        assert_ne!(
            entries[0].data, entries[1].data,
            "every entry rendered the same page"
        );
    }

    /// Asserts the failure is caught by poppler's EXIT STATUS, not merely
    /// by the absence of a `Pages:` line downstream. Both produce an
    /// `ArchiveError::Pdf`, so matching on the variant alone cannot tell
    /// them apart — and a `pdftoppm` that exits non-zero after emitting
    /// partial bytes would then be served as a valid page.
    #[test]
    fn a_corrupt_pdf_fails_on_the_tool_exit_status() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("garbage.pdf");
        std::fs::write(&path, b"this is not a pdf file at all").unwrap();
        let err = list_entry_names(&path).unwrap_err();
        let ArchiveError::Pdf { message, .. } = &err else {
            panic!("got {err:?}");
        };
        assert!(
            message.contains("pdfinfo failed"),
            "expected an exit-status failure, got: {message}"
        );
    }
}
