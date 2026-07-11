//! Pure-text ComicInfo.xml generator. Takes a structured
//! [`ComicInfoMetadata`] value and emits UTF-8 XML bytes suitable for
//! embedding directly into a CBZ archive as `ComicInfo.xml`.
//!
//! Phase B's input shape, not Phase A's parser projection: a
//! `start_year` (Volume) field, plus Publisher and a single canonical
//! Web URL — none of which the parser-side [`crate::ComicInfo`] carries
//! today. The two types are kept distinct because their use cases
//! diverge (parse partial input vs. write a canonical set).
//!
//! Note: LongBox deliberately does NOT emit the issue's own publish
//! date (`Year`/`Month`/`Day`). Per-issue dates leak into reader apps
//! and mis-sort/mis-group issues; only the series-level `Volume` is
//! written. The `issues.cover_date` DB column is unaffected and still
//! serves enrichment, pull maxage, and solicited-issue detection.
//!
//! Output shape: canonical ComicInfo v1, with the xmlns:xsi / xmlns:xsd
//! attributes most external tools expect. 2-space indent, one element
//! per line, no BOM. Empty / None fields are omitted entirely (no empty
//! `<Title/>` elements). Summary is CDATA-wrapped because the locked
//! Phase B policy is to pass CV's raw HTML through; downstream readers
//! (Komga, Kavita, etc.) render it.

use serde::{Deserialize, Serialize};

/// Catalog-derived ComicInfo write set. Build from `(SeriesRow,
/// IssueRow)` at the call site; the writer doesn't reach into the DB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComicInfoMetadata {
    /// Canonical CV series name.
    pub series: String,
    /// Issue number, raw string. `"1"`, `"½"`, `"Annual 1"`, `"3A"`,
    /// all valid; writer never reformats.
    pub number: String,
    /// `series.start_year` — the volume's launch year. Distinct from
    /// `cover_date.year` (the issue's release year), and locked here
    /// per the Phase B brief to prevent the regression we hit in the
    /// previous iteration.
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub title: Option<String>,
    /// Canonical ComicVine issue URL. Phase B uses the issue's CV slug
    /// to construct one; absent if the catalog row has no CV link.
    pub web: Option<String>,
    /// Raw HTML allowed; goes into a CDATA section in the output.
    pub summary: Option<String>,
}

impl ComicInfoMetadata {
    /// Render to ComicInfo v1 XML bytes. The output is well-formed
    /// UTF-8 and ready to embed in a CBZ as `ComicInfo.xml`.
    pub fn to_xml(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str(
            r#"<?xml version="1.0" encoding="UTF-8"?>
"#,
        );
        out.push_str(
            r#"<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
"#,
        );

        push_text(&mut out, "Series", &self.series);
        push_text(&mut out, "Number", &self.number);
        if let Some(y) = self.start_year {
            push_int(&mut out, "Volume", y);
        }
        if let Some(p) = self.publisher.as_deref() {
            push_text(&mut out, "Publisher", p);
        }
        if let Some(t) = self.title.as_deref() {
            push_text(&mut out, "Title", t);
        }
        if let Some(w) = self.web.as_deref() {
            push_text(&mut out, "Web", w);
        }
        if let Some(s) = self.summary.as_deref() {
            push_cdata(&mut out, "Summary", s);
        }

        out.push_str("</ComicInfo>\n");
        out
    }
}

fn push_text(out: &mut String, tag: &str, value: &str) {
    out.push_str("  <");
    out.push_str(tag);
    out.push('>');
    xml_escape_into(value, out);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

fn push_int(out: &mut String, tag: &str, value: i32) {
    out.push_str("  <");
    out.push_str(tag);
    out.push('>');
    out.push_str(&value.to_string());
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// CDATA-wrap `value` and append as `<tag><![CDATA[ … ]]></tag>`. If
/// `value` contains the literal sequence `]]>`, that would prematurely
/// terminate the CDATA section; split it across two CDATA sections via
/// the standard `]]]]><![CDATA[>` trick so the resulting concatenation
/// reads `]]>` to the parser without ever appearing literally inside
/// a single CDATA.
fn push_cdata(out: &mut String, tag: &str, value: &str) {
    out.push_str("  <");
    out.push_str(tag);
    out.push_str("><![CDATA[");
    let safe = value.replace("]]>", "]]]]><![CDATA[>");
    out.push_str(&safe);
    out.push_str("]]></");
    out.push_str(tag);
    out.push_str(">\n");
}

/// XML-escape `s` into `out`. **Order matters:** `&` must be replaced
/// first, otherwise `<` -> `&lt;` then `&` -> `&amp;` would double-
/// escape the just-introduced `&lt;` into `&amp;lt;`. A single-pass
/// character walk avoids the multi-pass replace ordering footgun
/// entirely.
fn xml_escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ComicInfo;

    fn fixture() -> ComicInfoMetadata {
        ComicInfoMetadata {
            series: "Saga".into(),
            number: "1".into(),
            start_year: Some(2012),
            publisher: Some("Image".into()),
            title: Some("The Will".into()),
            web: Some("https://comicvine.gamespot.com/saga-1/4000-364354/".into()),
            summary: Some("<p>A galactic war epic.</p>".into()),
        }
    }

    #[test]
    fn full_metadata_round_trips_through_parser() {
        let xml = fixture().to_xml();
        let parsed = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(parsed.series.as_deref(), Some("Saga"));
        assert_eq!(parsed.number.as_deref(), Some("1"));
        assert_eq!(parsed.title.as_deref(), Some("The Will"));
        // The parser's `year` reads from <Volume>, per the existing
        // Phase A field semantics.
        assert_eq!(parsed.year, Some(2012));
        assert_eq!(
            parsed.web_urls.first().map(String::as_str),
            Some("https://comicvine.gamespot.com/saga-1/4000-364354/")
        );
        assert!(parsed
            .summary
            .as_deref()
            .map(|s| s.contains("galactic war epic"))
            .unwrap_or(false));
    }

    #[test]
    fn omits_none_fields_entirely() {
        let m = ComicInfoMetadata {
            series: "Saga".into(),
            number: "1".into(),
            start_year: None,
            publisher: None,
            title: None,
            web: None,
            summary: None,
        };
        let xml = m.to_xml();
        assert!(!xml.contains("<Title"));
        assert!(!xml.contains("<Publisher"));
        assert!(!xml.contains("<Volume"));
        assert!(!xml.contains("<Year"));
        assert!(!xml.contains("<Month"));
        assert!(!xml.contains("<Day"));
        assert!(!xml.contains("<Web"));
        assert!(!xml.contains("<Summary"));
        assert!(xml.contains("<Series>Saga</Series>"));
        assert!(xml.contains("<Number>1</Number>"));
    }

    #[test]
    fn xml_special_chars_escaped_in_text_fields() {
        let m = ComicInfoMetadata {
            series: r#"A&B<C>D"E'F"#.into(),
            number: "1".into(),
            start_year: None,
            publisher: None,
            title: None,
            web: None,
            summary: None,
        };
        let xml = m.to_xml();
        assert!(xml.contains("<Series>A&amp;B&lt;C&gt;D&quot;E&apos;F</Series>"));
        // No double-escape: the ampersand of the original input must
        // not become `&amp;amp;`.
        assert!(!xml.contains("&amp;amp;"));
        assert!(!xml.contains("&amp;lt;"));
    }

    #[test]
    fn non_integer_issue_numbers_pass_through() {
        for n in ["½", "Annual 1", "3A", "-1", "1.MU"] {
            let m = ComicInfoMetadata {
                series: "X".into(),
                number: n.into(),
                start_year: None,
                publisher: None,
                title: None,
                web: None,
                summary: None,
            };
            let xml = m.to_xml();
            // Number element body matches input verbatim modulo XML
            // escaping (none of the test cases contain XML specials).
            let needle = format!("<Number>{n}</Number>");
            assert!(xml.contains(&needle), "missing {needle:?} in {xml}");
        }
    }

    #[test]
    fn summary_carries_html_via_cdata() {
        let m = ComicInfoMetadata {
            summary: Some("<p>Has <i>italics</i> &amp; entities.</p>".into()),
            ..fixture()
        };
        let xml = m.to_xml();
        // HTML survives unescaped via CDATA; readers render it.
        assert!(xml
            .contains("<Summary><![CDATA[<p>Has <i>italics</i> &amp; entities.</p>]]></Summary>"));
    }

    #[test]
    fn summary_with_cdata_close_marker_splits_safely() {
        // Defensive: a Summary string that literally contains `]]>`
        // would terminate the CDATA section prematurely. The writer
        // splits it across two CDATA sections via the standard
        // `]]]]><![CDATA[>` trick.
        let m = ComicInfoMetadata {
            summary: Some("Before ]]> after".into()),
            ..fixture()
        };
        let xml = m.to_xml();
        // The literal `]]>` must not appear inside the CDATA body
        // (only as the outer terminator).
        let summary_start = xml.find("<Summary>").unwrap();
        let summary_end = xml.find("</Summary>").unwrap();
        let inner = &xml[summary_start + "<Summary>".len()..summary_end];
        // The inner content begins with one CDATA opener and ends
        // with one closer, with the split-trick between them.
        assert!(inner.starts_with("<![CDATA["));
        assert!(inner.ends_with("]]>"));
        // Strip outer CDATA wrappers and verify the split-trick is
        // present (which is how `]]>` makes it through).
        assert!(inner.contains("]]]]><![CDATA[>"));
        // Round-trip: a real XML parser should reconstruct the literal.
        let parsed = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(parsed.summary.as_deref(), Some("Before ]]> after"));
    }

    #[test]
    fn xml_declaration_and_canonical_root_attrs() {
        let xml = fixture().to_xml();
        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(xml.contains(
            r#"<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">"#
        ));
        assert!(xml.ends_with("</ComicInfo>\n"));
    }

    #[test]
    fn never_emits_per_issue_publish_date() {
        // The issue's own publish date must NOT be embedded — it leaks
        // into readers and mis-sorts/mis-groups issues. Only the
        // series-level <Volume> is written.
        let xml = fixture().to_xml();
        assert!(!xml.contains("<Year"), "must not emit <Year>: {xml}");
        assert!(!xml.contains("<Month"), "must not emit <Month>: {xml}");
        assert!(!xml.contains("<Day"), "must not emit <Day>: {xml}");
        assert!(xml.contains("<Volume>2012</Volume>"), "must keep <Volume>");
    }

    /// Golden-file check against a hand-built canonical ComicInfo.xml.
    /// Catches accidental formatting drift (indent, element order,
    /// element-name spellings) in one assertion.
    #[test]
    fn golden_full_output() {
        let m = fixture();
        let actual = m.to_xml();
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Series>Saga</Series>
  <Number>1</Number>
  <Volume>2012</Volume>
  <Publisher>Image</Publisher>
  <Title>The Will</Title>
  <Web>https://comicvine.gamespot.com/saga-1/4000-364354/</Web>
  <Summary><![CDATA[<p>A galactic war epic.</p>]]></Summary>
</ComicInfo>
"#;
        assert_eq!(actual, expected);
    }
}
