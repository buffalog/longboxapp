//! Pure ComicInfo.xml parser. Takes XML bytes (the scanner reads the .cbz
//! archive and hands the bytes in), returns a [`ComicInfo`] struct.
//!
//! `<Web>` element handling: a ComicInfo file may have a single `<Web>` with
//! one URL, a single `<Web>` with multiple whitespace-separated URLs (the
//! ComicTagger 1.6+ / MetronTagger convention), or multiple `<Web>` elements.
//! The parser flattens all of them into a single `web_urls: Vec<String>`
//! where each entry is exactly one URL string — callers iterate without
//! having to split.
//!
//! Issue-ID extraction helpers ([`extract_cv_issue_id_from_url`] and
//! [`extract_metron_issue_id_from_url`]) operate on a single URL string. They
//! live here (not in the matcher) because the scanner orchestrates Tier 1
//! using direct DB lookups against each URL.
//!
//! ComicVine URL filter: the entity-type code in the slug must be `4000`
//! (issue). Volume URLs (`4050-`), character URLs (`4005-`), publisher URLs
//! (`4010-`), etc. are silently ignored.

use std::sync::OnceLock;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ComicInfo {
    pub title: Option<String>,
    pub series: Option<String>,
    pub number: Option<String>,
    /// Publication year carried by ComicInfo.xml's `<Volume>` element. We use
    /// the semantic name `year` rather than the XML element name `Volume`
    /// because ComicInfo overloads "Volume" to mean two different things in
    /// different communities; year is unambiguous.
    pub year: Option<i32>,
    pub summary: Option<String>,
    /// Every URL pulled from every `<Web>` element, in document order, with
    /// any whitespace-separated multi-URL strings already split into
    /// individual entries.
    pub web_urls: Vec<String>,
}

impl ComicInfo {
    pub fn parse(xml: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(xml)
            .map_err(|e| CoreError::ComicInfoParse(format!("not valid UTF-8: {e}")))?;
        parse_str(text)
    }

    /// Convenience: iterate [`web_urls`](Self::web_urls) and return the first
    /// ComicVine issue ID via [`extract_cv_issue_id_from_url`].
    pub fn cv_issue_id(&self) -> Option<i64> {
        self.web_urls
            .iter()
            .find_map(|url| extract_cv_issue_id_from_url(url))
    }

    /// Convenience: iterate [`web_urls`](Self::web_urls) and return the first
    /// Metron issue slug via [`extract_metron_issue_id_from_url`].
    pub fn metron_issue_slug(&self) -> Option<String> {
        self.web_urls
            .iter()
            .find_map(|url| extract_metron_issue_id_from_url(url))
    }
}

/// Extract a ComicVine issue ID from a single URL string. Returns `Some(id)`
/// only when the entity-type prefix is `4000` (CV's code for "issue"). Volume
/// (`4050`), character (`4005`), publisher (`4010`) and other entity types
/// return `None`.
pub fn extract_cv_issue_id_from_url(url: &str) -> Option<i64> {
    let caps = cv_regex().captures(url)?;
    let entity_type = caps.get(1)?.as_str();
    if entity_type != "4000" {
        return None;
    }
    caps.get(2)?.as_str().parse::<i64>().ok()
}

/// Extract a Metron issue slug from a single URL string.
pub fn extract_metron_issue_id_from_url(url: &str) -> Option<String> {
    let caps = metron_regex().captures(url)?;
    caps.get(1).map(|m| m.as_str().to_owned())
}

fn cv_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"comicvine\.gamespot\.com/(?:issue|issue-detail)/?[^/]*?(\d+)-(\d+)")
            .expect("static CV regex must compile")
    })
}

fn metron_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"metron\.cloud/issue/([\w-]+)").expect("static Metron regex must compile")
    })
}

#[derive(Copy, Clone)]
enum Field {
    Title,
    Series,
    Number,
    Volume,
    Summary,
    Web,
}

fn field_for(name: &[u8]) -> Option<Field> {
    Some(match name {
        b"Title" => Field::Title,
        b"Series" => Field::Series,
        b"Number" => Field::Number,
        b"Volume" => Field::Volume,
        b"Summary" => Field::Summary,
        b"Web" => Field::Web,
        _ => return None,
    })
}

fn parse_str(xml: &str) -> Result<ComicInfo> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

    let mut info = ComicInfo::default();
    let mut current: Option<Field> = None;
    let mut buf = String::new();

    loop {
        match reader
            .read_event()
            .map_err(|e| CoreError::ComicInfoParse(e.to_string()))?
        {
            Event::Start(e) => {
                current = field_for(e.name().as_ref());
                buf.clear();
            }
            Event::Text(t) if current.is_some() => {
                let s = t
                    .unescape()
                    .map_err(|e| CoreError::ComicInfoParse(e.to_string()))?;
                buf.push_str(&s);
            }
            Event::CData(c) if current.is_some() => {
                let s = std::str::from_utf8(&c)
                    .map_err(|e| CoreError::ComicInfoParse(e.to_string()))?;
                buf.push_str(s);
            }
            Event::End(_) => {
                if let Some(field) = current.take() {
                    let text = buf.trim().to_owned();
                    if !text.is_empty() {
                        match field {
                            Field::Title => info.title = Some(text),
                            Field::Series => info.series = Some(text),
                            Field::Number => info.number = Some(text),
                            Field::Volume => info.year = text.parse().ok(),
                            Field::Summary => info.summary = Some(text),
                            Field::Web => {
                                // Split whitespace-separated multi-URL entries
                                // so each Vec element is exactly one URL.
                                for token in text.split_whitespace() {
                                    info.web_urls.push(token.to_owned());
                                }
                            }
                        }
                    }
                    buf.clear();
                }
            }
            Event::Empty(_) => {}
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>One Small Step</Title>
  <Series>The Walking Dead</Series>
  <Number>1</Number>
  <Volume>2003</Volume>
  <Summary>The end of the world.</Summary>
  <Web>https://comicvine.gamespot.com/issue/4000-12345/</Web>
</ComicInfo>"#;

    #[test]
    fn parses_happy_path() {
        let ci = ComicInfo::parse(HAPPY_PATH.as_bytes()).unwrap();
        assert_eq!(ci.title.as_deref(), Some("One Small Step"));
        assert_eq!(ci.series.as_deref(), Some("The Walking Dead"));
        assert_eq!(ci.number.as_deref(), Some("1"));
        assert_eq!(ci.year, Some(2003));
        assert_eq!(ci.summary.as_deref(), Some("The end of the world."));
        assert_eq!(ci.web_urls.len(), 1);
        assert_eq!(ci.cv_issue_id(), Some(12345));
        assert!(ci.metron_issue_slug().is_none());
    }

    #[test]
    fn missing_web_yields_no_urls() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Saga</Series>
  <Number>1</Number>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert!(ci.web_urls.is_empty());
        assert!(ci.cv_issue_id().is_none());
        assert!(ci.metron_issue_slug().is_none());
    }

    #[test]
    fn cv_only_url() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://comicvine.gamespot.com/issue/4000-99/</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.cv_issue_id(), Some(99));
        assert!(ci.metron_issue_slug().is_none());
    }

    #[test]
    fn cv_issue_detail_form() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://comicvine.gamespot.com/issue-detail/4000-77</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.cv_issue_id(), Some(77));
    }

    #[test]
    fn metron_only_url() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://metron.cloud/issue/walking-dead-1-2003</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert!(ci.cv_issue_id().is_none());
        assert_eq!(
            ci.metron_issue_slug().as_deref(),
            Some("walking-dead-1-2003")
        );
    }

    #[test]
    fn cv_wins_when_both_present_in_separate_elements() {
        // Both URLs are flattened into web_urls in document order. The
        // matcher decides priority; ComicInfo just exposes both.
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://metron.cloud/issue/walking-dead-1-2003</Web>
  <Web>https://comicvine.gamespot.com/issue/4000-12345/</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.web_urls.len(), 2);
        assert_eq!(ci.cv_issue_id(), Some(12345));
        assert_eq!(
            ci.metron_issue_slug().as_deref(),
            Some("walking-dead-1-2003")
        );
    }

    #[test]
    fn multiple_urls_in_single_web_element_get_split() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://comicvine.gamespot.com/issue/4000-12345/ https://metron.cloud/issue/saga-1-2012</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.web_urls.len(), 2, "whitespace-separated tokens become separate entries");
        assert_eq!(ci.cv_issue_id(), Some(12345));
        assert_eq!(ci.metron_issue_slug().as_deref(), Some("saga-1-2012"));
    }

    #[test]
    fn rejects_non_4000_cv_entity_types() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://comicvine.gamespot.com/issue/4050-111/</Web>
  <Web>https://comicvine.gamespot.com/issue/4005-222/</Web>
  <Web>https://comicvine.gamespot.com/issue/4010-333/</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.web_urls.len(), 3);
        assert!(ci.cv_issue_id().is_none());
    }

    #[test]
    fn finds_4000_among_other_entity_types() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://comicvine.gamespot.com/issue/4050-111/ https://comicvine.gamespot.com/issue/4000-99/</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.cv_issue_id(), Some(99));
    }

    #[test]
    fn ignores_non_url_text_in_web() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>see also https://comicvine.gamespot.com/issue/4000-42/ archived</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.cv_issue_id(), Some(42));
    }

    #[test]
    fn handles_http_protocol_too() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>http://comicvine.gamespot.com/issue/4000-7/</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.cv_issue_id(), Some(7));
    }

    #[test]
    fn malformed_xml_errors() {
        let xml = r#"<ComicInfo><Series>Saga</UnclosedTag>"#;
        let err = ComicInfo::parse(xml.as_bytes()).err();
        assert!(err.is_some(), "expected parse error");
    }

    #[test]
    fn non_utf8_errors() {
        let bytes = [0xff, 0xfe, 0xfd];
        let err = ComicInfo::parse(&bytes).err();
        assert!(err.is_some(), "expected UTF-8 error");
    }

    #[test]
    fn xml_entities_unescaped() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Cable &amp; X-Force</Series>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.series.as_deref(), Some("Cable & X-Force"));
    }

    #[test]
    fn invalid_year_silently_dropped() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Saga</Series>
  <Volume>Unknown</Volume>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert!(ci.year.is_none());
    }

    #[test]
    fn empty_elements_ignored() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Saga</Series>
  <Number></Number>
  <Web></Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.series.as_deref(), Some("Saga"));
        assert!(ci.number.is_none());
        assert!(ci.web_urls.is_empty());
    }

    #[test]
    fn cdata_summary_handled() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Saga</Series>
  <Summary><![CDATA[<p>HTML summary</p>]]></Summary>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.summary.as_deref(), Some("<p>HTML summary</p>"));
    }

    #[test]
    fn extract_cv_helper_rejects_non_4000() {
        assert_eq!(
            extract_cv_issue_id_from_url("https://comicvine.gamespot.com/issue/4000-42/"),
            Some(42)
        );
        assert_eq!(
            extract_cv_issue_id_from_url("https://comicvine.gamespot.com/issue/4050-99/"),
            None
        );
        assert_eq!(extract_cv_issue_id_from_url("not a url"), None);
    }

    #[test]
    fn extract_metron_helper() {
        assert_eq!(
            extract_metron_issue_id_from_url("https://metron.cloud/issue/saga-1-2012"),
            Some("saga-1-2012".to_owned())
        );
        assert_eq!(extract_metron_issue_id_from_url("not a url"), None);
    }
}
