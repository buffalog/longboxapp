//! Newznab RSS/XML response parsing.
//!
//! Streaming parse via `quick-xml`. Defensive throughout: missing
//! `<newznab:attr>` fields become `None`, malformed entries are
//! skipped rather than panicking, and a Newznab `<error>` element
//! short-circuits to the right [`IndexerError`].

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use time::OffsetDateTime;

use crate::error::IndexerError;
use crate::types::Release;

/// Parse a Newznab search response body into releases.
///
/// `Err` on: XML that doesn't parse, or a Newznab `<error>` element
/// (classified into `BadCredentials` for the 100-107 account range,
/// `MalformedResponse` otherwise). `Ok(vec![])` for a well-formed
/// response with zero `<item>`s.
pub fn parse_response(xml: &str) -> Result<Vec<Release>, IndexerError> {
    let mut reader = Reader::from_str(xml);
    let mut releases = Vec::new();
    let mut builder: Option<ReleaseBuilder> = None;
    let mut current_field: Option<Field> = None;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| IndexerError::MalformedResponse(format!("xml parse error: {e}")))?;
        match event {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"error" => return Err(parse_error_element(&e)),
                    b"item" => builder = Some(ReleaseBuilder::default()),
                    b"title" => current_field = Some(Field::Title),
                    b"guid" => current_field = Some(Field::Guid),
                    b"pubDate" => current_field = Some(Field::PubDate),
                    b"enclosure" => {
                        if let Some(b) = builder.as_mut() {
                            if let Some(url) = attr_value(&e, b"url") {
                                b.nzb_url = Some(url);
                            }
                        }
                    }
                    b"attr" => {
                        if let Some(b) = builder.as_mut() {
                            apply_newznab_attr(b, &e);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(e) => {
                if let (Some(b), Some(field)) = (builder.as_mut(), current_field) {
                    let text = e.unescape().unwrap_or_default().trim().to_string();
                    b.set_field(field, text);
                }
            }
            Event::CData(e) => {
                // Newznab titles are often CDATA-wrapped.
                if let (Some(b), Some(field)) = (builder.as_mut(), current_field) {
                    let text = String::from_utf8_lossy(&e.into_inner()).trim().to_string();
                    b.set_field(field, text);
                }
            }
            Event::End(e) => {
                if e.local_name().as_ref() == b"item" {
                    if let Some(b) = builder.take() {
                        if let Some(r) = b.finish() {
                            releases.push(r);
                        }
                    }
                }
                current_field = None;
            }
            _ => {}
        }
    }
    Ok(releases)
}

#[derive(Clone, Copy)]
enum Field {
    Title,
    Guid,
    PubDate,
}

#[derive(Default)]
struct ReleaseBuilder {
    title: Option<String>,
    guid: Option<String>,
    nzb_url: Option<String>,
    pub_date: Option<String>,
    size_bytes: Option<i64>,
    grabs: Option<i64>,
    category: Option<String>,
}

impl ReleaseBuilder {
    fn set_field(&mut self, field: Field, text: String) {
        match field {
            Field::Title => self.title = Some(text),
            Field::Guid => self.guid = Some(text),
            Field::PubDate => self.pub_date = Some(text),
        }
    }

    /// Finalize into a `Release`, or `None` when the item lacks the
    /// essentials (a title to match on, an NZB URL to download).
    fn finish(self) -> Option<Release> {
        let title = self.title.filter(|s| !s.is_empty())?;
        let nzb_url = self.nzb_url.filter(|s| !s.is_empty())?;
        // guid is the downstream release_id; fall back to the URL when
        // an indexer omits it.
        let guid = self
            .guid
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| nzb_url.clone());
        let published = self.pub_date.as_deref().and_then(parse_pub_date);
        Some(Release {
            title,
            nzb_url,
            guid,
            published,
            size_bytes: self.size_bytes,
            grabs: self.grabs,
            category: self.category,
        })
    }
}

/// Read a single attribute's value off an element. `flatten()` drops
/// any attribute that fails to parse — defensive, never panics.
fn attr_value(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.unescape_value().ok())
        .map(|v| v.into_owned())
}

fn apply_newznab_attr(b: &mut ReleaseBuilder, e: &BytesStart) {
    let (Some(name), Some(value)) = (attr_value(e, b"name"), attr_value(e, b"value")) else {
        return;
    };
    match name.as_str() {
        "size" => b.size_bytes = value.parse().ok(),
        "grabs" => b.grabs = value.parse().ok(),
        "category" => b.category = Some(value),
        _ => {}
    }
}

/// Classify a Newznab `<error code="..." description="..."/>` element.
/// Codes 100-107 are the account range → `BadCredentials` (permanent);
/// everything else → `MalformedResponse` with the code preserved.
fn parse_error_element(e: &BytesStart) -> IndexerError {
    let code = attr_value(e, b"code")
        .and_then(|c| c.parse::<u32>().ok())
        .unwrap_or(0);
    let description = attr_value(e, b"description").unwrap_or_else(|| "(no description)".into());
    if (100..=107).contains(&code) {
        IndexerError::BadCredentials { code, description }
    } else {
        IndexerError::MalformedResponse(format!("newznab error {code}: {description}"))
    }
}

/// RSS `<pubDate>` is RFC-2822. Unparseable / absent → `None`.
fn parse_pub_date(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw.trim(), &time::format_description::well_known::Rfc2822).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
  <channel>
    <item>
      <title>Wolverine 005 (1982) (digital).cbz</title>
      <guid>abc-123</guid>
      <pubDate>Mon, 05 May 2025 14:30:00 +0000</pubDate>
      <enclosure url="https://idx.example.com/nzb/abc-123" length="9999" type="application/x-nzb"/>
      <newznab:attr name="size" value="48234567"/>
      <newznab:attr name="grabs" value="42"/>
      <newznab:attr name="category" value="7030"/>
    </item>
    <item>
      <title><![CDATA[Wolverine 005 (1982).cbr]]></title>
      <guid>def-456</guid>
      <enclosure url="https://idx.example.com/nzb/def-456"/>
      <newznab:attr name="grabs" value="3"/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_valid_multi_item_response() {
        let releases = parse_response(VALID).unwrap();
        assert_eq!(releases.len(), 2);

        let first = &releases[0];
        assert_eq!(first.title, "Wolverine 005 (1982) (digital).cbz");
        assert_eq!(first.guid, "abc-123");
        assert_eq!(first.nzb_url, "https://idx.example.com/nzb/abc-123");
        assert_eq!(first.size_bytes, Some(48_234_567));
        assert_eq!(first.grabs, Some(42));
        assert_eq!(first.category.as_deref(), Some("7030"));
        assert!(first.published.is_some());
    }

    #[test]
    fn cdata_title_is_decoded() {
        let releases = parse_response(VALID).unwrap();
        assert_eq!(releases[1].title, "Wolverine 005 (1982).cbr");
    }

    #[test]
    fn missing_newznab_attrs_become_none() {
        let releases = parse_response(VALID).unwrap();
        let second = &releases[1];
        // second item has grabs but no size / category / pubDate
        assert_eq!(second.grabs, Some(3));
        assert_eq!(second.size_bytes, None);
        assert_eq!(second.category, None);
        assert_eq!(second.published, None);
    }

    #[test]
    fn guid_falls_back_to_url_when_absent() {
        let xml = r#"<rss><channel><item>
          <title>No Guid.cbz</title>
          <enclosure url="https://x/nzb/1"/>
        </item></channel></rss>"#;
        let releases = parse_response(xml).unwrap();
        assert_eq!(releases[0].guid, "https://x/nzb/1");
    }

    #[test]
    fn items_missing_essentials_are_skipped() {
        // No enclosure URL → not catalogable, dropped.
        let xml = r#"<rss><channel>
          <item><title>No Url.cbz</title></item>
          <item><title>Has Url.cbz</title><enclosure url="https://x/1"/></item>
        </channel></rss>"#;
        let releases = parse_response(xml).unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].title, "Has Url.cbz");
    }

    #[test]
    fn zero_item_response_is_ok_empty() {
        let xml = r#"<rss><channel></channel></rss>"#;
        assert_eq!(parse_response(xml).unwrap().len(), 0);
    }

    #[test]
    fn bad_credentials_error_classified() {
        let xml = r#"<error code="100" description="Incorrect user credentials"/>"#;
        match parse_response(xml) {
            Err(IndexerError::BadCredentials { code, description }) => {
                assert_eq!(code, 100);
                assert_eq!(description, "Incorrect user credentials");
            }
            other => panic!("expected BadCredentials, got {other:?}"),
        }
    }

    #[test]
    fn non_credential_error_is_malformed_response() {
        let xml = r#"<error code="910" description="API Disabled"/>"#;
        match parse_response(xml) {
            Err(IndexerError::MalformedResponse(msg)) => {
                assert!(msg.contains("910"));
                assert!(msg.contains("API Disabled"));
            }
            other => panic!("expected MalformedResponse, got {other:?}"),
        }
    }

    #[test]
    fn broken_markup_is_malformed_response_error() {
        // A mismatched end tag — quick-xml rejects this outright.
        let xml = r#"<rss><channel></item></channel></rss>"#;
        assert!(matches!(
            parse_response(xml),
            Err(IndexerError::MalformedResponse(_))
        ));
    }

    #[test]
    fn truncated_response_degrades_gracefully() {
        // A response cut off mid-stream is syntactically fine up to
        // the cut — quick-xml streams to EOF without erroring. The
        // complete item is returned; the half-written one (no
        // enclosure) is dropped by finish(). Defensive, never panics.
        let xml = r#"<rss><channel>
          <item><title>Complete.cbz</title><enclosure url="https://x/1"/></item>
          <item><title>Truncated mid-wr"#;
        let releases = parse_response(xml).unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].title, "Complete.cbz");
    }
}
