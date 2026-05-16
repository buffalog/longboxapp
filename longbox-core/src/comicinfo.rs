//! Pure ComicInfo.xml parser. Takes XML bytes (the scanner reads the .cbz
//! archive and hands the bytes in), returns a [`ComicInfo`] struct. URL
//! extraction for ComicVine and Metron lives here.
//!
//! `<Web>` element handling: a ComicInfo file may have a single `<Web>` with
//! one URL, a single `<Web>` with multiple whitespace-separated URLs (the
//! ComicTagger 1.6+ / MetronTagger convention), or multiple `<Web>` elements.
//! We capture every element's text and split each on whitespace at extraction
//! time. ComicVine wins over Metron when both are present.
//!
//! ComicVine URL filter: the entity-type code in the slug must be `4000`
//! (issue). Volume URLs (`4050-`), character URLs (`4005-`), publisher URLs
//! (`4010-`), etc. are silently ignored even when present in `<Web>`.

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
    pub volume: Option<i32>,
    pub summary: Option<String>,
    /// Raw text content of every `<Web>` element in document order. URLs
    /// inside each entry may be whitespace-separated; extraction handles that.
    pub web: Vec<String>,
}

impl ComicInfo {
    pub fn parse(xml: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(xml)
            .map_err(|e| CoreError::ComicInfoParse(format!("not valid UTF-8: {e}")))?;
        parse_str(text)
    }

    /// Iterate every `<Web>` element's URLs (splitting on whitespace) and
    /// return the first ComicVine issue ID whose entity-type code is `4000`.
    pub fn cv_issue_id(&self) -> Option<i64> {
        let re = cv_regex();
        for raw in &self.web {
            for token in raw.split_whitespace() {
                if let Some(caps) = re.captures(token) {
                    let entity_type = caps.get(1)?.as_str();
                    let id_str = caps.get(2)?.as_str();
                    if entity_type == "4000" {
                        return id_str.parse::<i64>().ok();
                    }
                }
            }
        }
        None
    }

    /// Iterate every `<Web>` element's URLs (splitting on whitespace) and
    /// return the first Metron issue slug. Returns the captured slug verbatim
    /// (e.g. `walking-dead-1-2003`).
    pub fn metron_issue_slug(&self) -> Option<String> {
        let re = metron_regex();
        for raw in &self.web {
            for token in raw.split_whitespace() {
                if let Some(caps) = re.captures(token) {
                    if let Some(m) = caps.get(1) {
                        return Some(m.as_str().to_owned());
                    }
                }
            }
        }
        None
    }
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
    RE.get_or_init(|| Regex::new(r"metron\.cloud/issue/([\w-]+)").expect("static Metron regex must compile"))
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
    // Accumulate text + CDATA inside a single element so split text events
    // (rare but legal) don't get dropped.
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
                            Field::Volume => info.volume = text.parse().ok(),
                            Field::Summary => info.summary = Some(text),
                            Field::Web => info.web.push(text),
                        }
                    }
                    buf.clear();
                }
            }
            Event::Empty(_) => {
                // Self-closing element — ignore.
            }
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
        assert_eq!(ci.volume, Some(2003));
        assert_eq!(ci.summary.as_deref(), Some("The end of the world."));
        assert_eq!(ci.web.len(), 1);
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
        assert!(ci.web.is_empty());
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
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://metron.cloud/issue/walking-dead-1-2003</Web>
  <Web>https://comicvine.gamespot.com/issue/4000-12345/</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.web.len(), 2);
        // Both extractable, but matcher logic (Tier 1) prefers CV.
        assert_eq!(ci.cv_issue_id(), Some(12345));
        assert_eq!(
            ci.metron_issue_slug().as_deref(),
            Some("walking-dead-1-2003")
        );
    }

    #[test]
    fn multiple_urls_in_single_web_element() {
        // ComicTagger 1.6+ / MetronTagger convention.
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://comicvine.gamespot.com/issue/4000-12345/ https://metron.cloud/issue/saga-1-2012</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.web.len(), 1);
        assert_eq!(ci.cv_issue_id(), Some(12345));
        assert_eq!(ci.metron_issue_slug().as_deref(), Some("saga-1-2012"));
    }

    #[test]
    fn rejects_non_4000_cv_entity_types() {
        // 4050 is volume, 4005 is character, 4010 is publisher — all should be
        // ignored.
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Web>https://comicvine.gamespot.com/issue/4050-111/</Web>
  <Web>https://comicvine.gamespot.com/issue/4005-222/</Web>
  <Web>https://comicvine.gamespot.com/issue/4010-333/</Web>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert_eq!(ci.web.len(), 3);
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
        // Sometimes taggers stuff arbitrary text alongside URLs.
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
        // Invalid UTF-8 byte sequence
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
    fn invalid_volume_silently_dropped() {
        // Volume should be numeric. "Unknown" can't parse to i32.
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Saga</Series>
  <Volume>Unknown</Volume>
</ComicInfo>"#;
        let ci = ComicInfo::parse(xml.as_bytes()).unwrap();
        assert!(ci.volume.is_none());
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
        assert!(ci.web.is_empty());
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
}
