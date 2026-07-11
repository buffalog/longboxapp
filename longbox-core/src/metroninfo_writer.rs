//! Pure-text MetronInfo.xml generator. Sibling to
//! [`crate::comicinfo_writer`]; takes a structured
//! [`MetronInfoMetadata`] value and emits UTF-8 XML bytes suitable for
//! embedding directly into a CBZ archive as `MetronInfo.xml`.
//!
//! MetronInfo is a younger, richer schema than ComicInfo and is
//! intentionally source-tagged: the whole point of the format is the
//! `<IDS>` block letting downstream tools (Perdoo, ComicRack CE,
//! Comicbox, Codex, Metron-Tagger) know which database each id comes
//! from. LongBox emits both ComicInfo.xml and MetronInfo.xml on import
//! for maximum compatibility — readers that prefer one format can pick
//! it without losing the other.
//!
//! The two writers are deliberately not factored into a shared "XML
//! helper" module yet. They share four short helper functions
//! (`push_text`, `push_int`, `push_cdata`, `xml_escape_into`); copying
//! them keeps each writer self-contained and readable. If a third
//! writer ever appears, refactor then — not before.
//!
//! Field-population reality: at Phase B import time LongBox has
//! `(SeriesRow, IssueRow)` in hand and nothing else. That populates
//! `<IDS>` (CV + Metron source-tagged), `<Publisher><Name>`,
//! `<Series>` (name, sort name, start year), `<Number>`, `<Summary>`
//! (HTML in CDATA), `<URLs>` (CV canonical), and `<LastModified>`.
//!
//! Note: LongBox deliberately does NOT emit the issue's own
//! `<CoverDate>`. Per-issue publish dates leak into reader apps and
//! mis-sort/mis-group issues; only the series-level `<StartYear>` is
//! written. `issues.cover_date` in the DB is unaffected.
//! The schema's optional richer fields
//! (`Stories`, `Genres`, `Tags`, `Arcs`, `Characters`, `Teams`,
//! `Universes`, `Locations`, `Reprints`, `GTIN`, `Credits`,
//! `Prices`, `StoreDate`, `Format`, `PageCount`, etc.) are not in
//! the LongBox catalog and are omitted rather than invented.

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Catalog-derived MetronInfo write set. Build from `(SeriesRow,
/// IssueRow)` at the call site; the writer doesn't reach into the DB.
///
/// `cv_issue_id` drives both the `<IDS><ID source="Comic Vine"
/// primary="true">` entry AND the `<URLs><URL primary="true">` entry.
/// `metron_issue_id` drives the sibling `<IDS><ID source="Metron">`
/// without the `primary` attribute — LongBox is CV-rooted, so CV wins
/// the primary slot whenever it's present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetronInfoMetadata {
    /// CV issue id. When present, emitted as the primary `<IDS>` entry
    /// and as the canonical `<URLs>` entry.
    pub cv_issue_id: Option<i64>,
    /// Metron issue id (stored as a string in the catalog). When
    /// present, emitted as a sibling `<IDS>` entry without the
    /// `primary` flag.
    pub metron_issue_id: Option<String>,
    /// Publisher name. No Imprint — LongBox doesn't track imprints.
    pub publisher: Option<String>,
    /// CV series id, used for the `<Series id="...">` attribute.
    pub cv_series_id: Option<i64>,
    /// Series title, emitted as `<Series><Name>`.
    pub series: String,
    /// Series sort title. Emitted as `<SortName>` only when it differs
    /// from `series` — repeating an identical value is just noise.
    pub series_sort: Option<String>,
    /// Series start year, emitted as `<StartYear>`.
    pub start_year: Option<i32>,
    /// Issue number, raw string. Pass-through; never reformatted.
    pub number: String,
    /// Raw HTML allowed; goes into a CDATA section in the output.
    pub summary: Option<String>,
    /// `<LastModified>` timestamp. Caller supplies — tests use a fixed
    /// value, production hands `OffsetDateTime::now_utc()`. Output is
    /// RFC 3339, which `xs:dateTime` accepts.
    pub last_modified: OffsetDateTime,
}

impl MetronInfoMetadata {
    /// Render to MetronInfo v1.1 XML bytes. Output is well-formed
    /// UTF-8 and ready to embed in a CBZ as `MetronInfo.xml`.
    pub fn to_xml(&self) -> String {
        let mut out = String::with_capacity(640);
        out.push_str(
            r#"<?xml version="1.0" encoding="UTF-8"?>
"#,
        );
        out.push_str(
            r#"<MetronInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="MetronInfo.xsd">
"#,
        );

        push_ids(&mut out, self.cv_issue_id, self.metron_issue_id.as_deref());
        push_publisher(&mut out, self.publisher.as_deref());
        push_series(
            &mut out,
            &self.series,
            self.series_sort.as_deref(),
            self.cv_series_id,
            self.start_year,
        );
        push_text(&mut out, "Number", &self.number, 2);
        if let Some(s) = self.summary.as_deref() {
            push_cdata(&mut out, "Summary", s, 2);
        }
        push_urls(&mut out, self.cv_issue_id);
        push_last_modified(&mut out, self.last_modified);

        out.push_str("</MetronInfo>\n");
        out
    }
}

fn push_ids(out: &mut String, cv_issue_id: Option<i64>, metron_issue_id: Option<&str>) {
    if cv_issue_id.is_none() && metron_issue_id.is_none() {
        return;
    }
    out.push_str("  <IDS>\n");
    if let Some(cv) = cv_issue_id {
        // CV wins the primary slot whenever present — the catalog is
        // CV-rooted, so this matches the source of truth.
        out.push_str(r#"    <ID source="Comic Vine" primary="true">"#);
        out.push_str(&cv.to_string());
        out.push_str("</ID>\n");
    }
    if let Some(m) = metron_issue_id {
        out.push_str(r#"    <ID source="Metron">"#);
        xml_escape_into(m, out);
        out.push_str("</ID>\n");
    }
    out.push_str("  </IDS>\n");
}

fn push_publisher(out: &mut String, name: Option<&str>) {
    let Some(name) = name else {
        return;
    };
    out.push_str("  <Publisher>\n");
    push_text(out, "Name", name, 4);
    out.push_str("  </Publisher>\n");
}

fn push_series(
    out: &mut String,
    name: &str,
    sort_name: Option<&str>,
    cv_series_id: Option<i64>,
    start_year: Option<i32>,
) {
    out.push_str("  <Series");
    if let Some(id) = cv_series_id {
        out.push_str(r#" id=""#);
        out.push_str(&id.to_string());
        out.push('"');
    }
    out.push_str(">\n");
    push_text(out, "Name", name, 4);
    // Skip <SortName> when it duplicates <Name> — no information added.
    if let Some(sort) = sort_name {
        if sort != name {
            push_text(out, "SortName", sort, 4);
        }
    }
    if let Some(y) = start_year {
        push_int(out, "StartYear", y, 4);
    }
    out.push_str("  </Series>\n");
}

fn push_urls(out: &mut String, cv_issue_id: Option<i64>) {
    let Some(cv) = cv_issue_id else {
        return;
    };
    out.push_str("  <URLs>\n");
    out.push_str(r#"    <URL primary="true">https://comicvine.gamespot.com/issue/4000-"#);
    out.push_str(&cv.to_string());
    out.push_str("/</URL>\n");
    out.push_str("  </URLs>\n");
}

fn push_last_modified(out: &mut String, ts: OffsetDateTime) {
    // Rfc3339 format failure on a constructed OffsetDateTime is an
    // internal invariant violation — format::Rfc3339 accepts every
    // valid OffsetDateTime. expect() with context surfaces the bug
    // rather than hiding it behind an Err return that callers would
    // unwrap anyway.
    let formatted = ts
        .format(&Rfc3339)
        .expect("OffsetDateTime always formats as RFC 3339");
    out.push_str("  <LastModified>");
    out.push_str(&formatted);
    out.push_str("</LastModified>\n");
}

// -------- shared formatting primitives (copied from comicinfo_writer
// per the kickoff: don't extract until a third writer needs them) --------

fn push_text(out: &mut String, tag: &str, value: &str, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push('<');
    out.push_str(tag);
    out.push('>');
    xml_escape_into(value, out);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

fn push_int(out: &mut String, tag: &str, value: i32, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push('<');
    out.push_str(tag);
    out.push('>');
    out.push_str(&value.to_string());
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// CDATA-wrap `value`. If `value` contains a literal `]]>`, split it
/// across two CDATA sections via the standard `]]]]><![CDATA[>` trick
/// so the resulting concatenation reads `]]>` to a parser without
/// ever appearing literally inside a single CDATA section.
fn push_cdata(out: &mut String, tag: &str, value: &str, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push('<');
    out.push_str(tag);
    out.push_str("><![CDATA[");
    let safe = value.replace("]]>", "]]]]><![CDATA[>");
    out.push_str(&safe);
    out.push_str("]]></");
    out.push_str(tag);
    out.push_str(">\n");
}

/// XML-escape `s` into `out`. Single-pass — order-safe by construction.
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
    use time::macros::datetime;

    fn fixture() -> MetronInfoMetadata {
        MetronInfoMetadata {
            cv_issue_id: Some(364354),
            metron_issue_id: Some("99999".into()),
            publisher: Some("Image".into()),
            cv_series_id: Some(42215),
            series: "Saga".into(),
            series_sort: Some("Saga".into()),
            start_year: Some(2012),
            number: "1".into(),
            summary: Some("<p>A galactic war epic.</p>".into()),
            last_modified: datetime!(2026-06-01 18:06:08 UTC),
        }
    }

    #[test]
    fn golden_full_output() {
        let m = fixture();
        let actual = m.to_xml();
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetronInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="MetronInfo.xsd">
  <IDS>
    <ID source="Comic Vine" primary="true">364354</ID>
    <ID source="Metron">99999</ID>
  </IDS>
  <Publisher>
    <Name>Image</Name>
  </Publisher>
  <Series id="42215">
    <Name>Saga</Name>
    <StartYear>2012</StartYear>
  </Series>
  <Number>1</Number>
  <Summary><![CDATA[<p>A galactic war epic.</p>]]></Summary>
  <URLs>
    <URL primary="true">https://comicvine.gamespot.com/issue/4000-364354/</URL>
  </URLs>
  <LastModified>2026-06-01T18:06:08Z</LastModified>
</MetronInfo>
"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn sort_name_omitted_when_equal_to_series_name() {
        let m = fixture();
        let xml = m.to_xml();
        // Fixture has series == series_sort == "Saga"; <SortName> must
        // not appear at all.
        assert!(!xml.contains("<SortName"));
    }

    #[test]
    fn sort_name_emitted_when_distinct() {
        let m = MetronInfoMetadata {
            series: "The Walking Dead Deluxe".into(),
            series_sort: Some("Walking Dead Deluxe".into()),
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(
            xml.contains("<SortName>Walking Dead Deluxe</SortName>"),
            "missing SortName in {xml}"
        );
    }

    #[test]
    fn ids_block_with_only_cv_marks_it_primary() {
        let m = MetronInfoMetadata {
            metron_issue_id: None,
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(xml.contains(r#"<ID source="Comic Vine" primary="true">364354</ID>"#));
        assert!(!xml.contains("source=\"Metron\""));
    }

    #[test]
    fn ids_block_with_only_metron_has_no_primary_flag() {
        // Per the spec/kickoff: CV is rooted in our catalog, so when
        // only Metron is available it does NOT inherit the primary
        // slot — the absence of CV means there's no primary at all.
        let m = MetronInfoMetadata {
            cv_issue_id: None,
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(xml.contains(r#"<ID source="Metron">99999</ID>"#));
        assert!(!xml.contains("source=\"Comic Vine\""));
        assert!(
            !xml.contains("primary=\"true\""),
            "no primary flag should land on the Metron-only path"
        );
    }

    #[test]
    fn ids_block_omitted_when_neither_id_present() {
        let m = MetronInfoMetadata {
            cv_issue_id: None,
            metron_issue_id: None,
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(!xml.contains("<IDS>"));
        assert!(!xml.contains("</IDS>"));
    }

    #[test]
    fn urls_block_omitted_when_no_cv_id() {
        let m = MetronInfoMetadata {
            cv_issue_id: None,
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(!xml.contains("<URLs>"));
        assert!(!xml.contains("comicvine.gamespot.com"));
    }

    #[test]
    fn publisher_omitted_when_absent() {
        let m = MetronInfoMetadata {
            publisher: None,
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(!xml.contains("<Publisher>"));
    }

    #[test]
    fn series_id_attribute_omitted_when_no_cv_series_id() {
        let m = MetronInfoMetadata {
            cv_series_id: None,
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(xml.contains("<Series>"));
        assert!(!xml.contains(r#"<Series id="#));
    }

    #[test]
    fn minimal_fixture_omits_optional_blocks() {
        let m = MetronInfoMetadata {
            cv_issue_id: None,
            metron_issue_id: None,
            publisher: None,
            cv_series_id: None,
            series: "Unknown Series".into(),
            series_sort: None,
            start_year: None,
            number: "1".into(),
            summary: None,
            last_modified: datetime!(2026-06-01 18:06:08 UTC),
        };
        let xml = m.to_xml();
        assert!(!xml.contains("<IDS>"));
        assert!(!xml.contains("<Publisher>"));
        assert!(!xml.contains("<StartYear>"));
        assert!(!xml.contains("<SortName>"));
        assert!(!xml.contains("<CoverDate>"));
        assert!(!xml.contains("<Summary>"));
        assert!(!xml.contains("<URLs>"));
        // Required and always-present things still land.
        assert!(xml.contains("<Series>"));
        assert!(xml.contains("<Name>Unknown Series</Name>"));
        assert!(xml.contains("<Number>1</Number>"));
        assert!(xml.contains("<LastModified>2026-06-01T18:06:08Z</LastModified>"));
    }

    #[test]
    fn xml_special_chars_escaped_in_text_fields() {
        let m = MetronInfoMetadata {
            series: r#"A&B<C>D"E'F"#.into(),
            series_sort: None,
            publisher: Some("Funny & Co. <Inc.>".into()),
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(xml.contains("<Name>A&amp;B&lt;C&gt;D&quot;E&apos;F</Name>"));
        assert!(xml.contains("<Name>Funny &amp; Co. &lt;Inc.&gt;</Name>"));
        // No double-escape: the ampersand of the input must not become
        // `&amp;amp;`.
        assert!(!xml.contains("&amp;amp;"));
        assert!(!xml.contains("&amp;lt;"));
    }

    #[test]
    fn metron_id_xml_escaped_even_though_typically_numeric() {
        // Metron ids today are numeric strings, but the type is
        // String — defensive: any future literal `<` or `&` in the
        // identifier (e.g., a slug-style id from a different source)
        // must not break the XML.
        let m = MetronInfoMetadata {
            metron_issue_id: Some("a<b&c".into()),
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(xml.contains(r#"<ID source="Metron">a&lt;b&amp;c</ID>"#));
    }

    #[test]
    fn summary_carries_html_via_cdata() {
        let m = MetronInfoMetadata {
            summary: Some("<p>Has <i>italics</i> &amp; entities.</p>".into()),
            ..fixture()
        };
        let xml = m.to_xml();
        assert!(xml
            .contains("<Summary><![CDATA[<p>Has <i>italics</i> &amp; entities.</p>]]></Summary>"));
    }

    #[test]
    fn summary_with_cdata_close_marker_splits_safely() {
        let m = MetronInfoMetadata {
            summary: Some("Before ]]> after".into()),
            ..fixture()
        };
        let xml = m.to_xml();
        let s_start = xml.find("<Summary>").unwrap();
        let s_end = xml.find("</Summary>").unwrap();
        let inner = &xml[s_start + "<Summary>".len()..s_end];
        assert!(inner.starts_with("<![CDATA["));
        assert!(inner.ends_with("]]>"));
        assert!(inner.contains("]]]]><![CDATA[>"));
    }

    #[test]
    fn never_emits_per_issue_cover_date() {
        // The issue's own publish date must NOT be embedded — it leaks
        // into readers and mis-sorts/mis-groups issues. Only the
        // series-level <StartYear> is written, even for a fully-populated
        // fixture.
        let xml = fixture().to_xml();
        assert!(
            !xml.contains("<CoverDate"),
            "must not emit <CoverDate>: {xml}"
        );
        assert!(
            xml.contains("<StartYear>2012</StartYear>"),
            "must keep <StartYear>"
        );
    }

    #[test]
    fn non_integer_issue_numbers_pass_through() {
        for n in ["½", "Annual 1", "3A", "-1", "1.MU"] {
            let m = MetronInfoMetadata {
                number: n.into(),
                ..fixture()
            };
            let xml = m.to_xml();
            let needle = format!("<Number>{n}</Number>");
            assert!(xml.contains(&needle), "missing {needle:?} in {xml}");
        }
    }

    #[test]
    fn xml_declaration_and_canonical_root_attrs() {
        let xml = fixture().to_xml();
        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(xml.contains(
            r#"<MetronInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="MetronInfo.xsd">"#
        ));
        assert!(xml.ends_with("</MetronInfo>\n"));
    }

    #[test]
    fn last_modified_uses_rfc3339_utc() {
        let m = fixture();
        let xml = m.to_xml();
        // RFC 3339 UTC produces `Z`-suffixed output for OffsetDateTime
        // in UTC offset. The fixture uses datetime!(... UTC) so the
        // suffix must be `Z`, not `+00:00`.
        assert!(xml.contains("<LastModified>2026-06-01T18:06:08Z</LastModified>"));
    }
}
