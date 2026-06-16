//! OpenSearch 1.1 descriptor document for OPDS search discovery.
//!
//! OPDS clients fetch this descriptor (linked from feeds via
//! `rel="search"`) to learn the search URL template, then substitute the
//! user's query for `{searchTerms}`.

use crate::feed::esc;

/// Content-type of the OpenSearch descriptor document.
pub const OPENSEARCH_DESCRIPTION_TYPE: &str = "application/opensearchdescription+xml";

/// MIME advertised for the search URL template — an OPDS catalog feed.
const SEARCH_RESULT_TYPE: &str = "application/atom+xml;profile=opds-catalog";

/// Build the OpenSearch descriptor advertising the OPDS search endpoint.
/// `base_url` is the public base (no trailing slash); the `{searchTerms}`
/// placeholder is left literal for the client to fill.
pub fn descriptor(base_url: &str) -> String {
    let base = esc(base_url);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\">\n  \
         <ShortName>LongBox</ShortName>\n  \
         <Description>Search your comic library</Description>\n  \
         <InputEncoding>UTF-8</InputEncoding>\n  \
         <Url type=\"{SEARCH_RESULT_TYPE}\" template=\"{base}/opds/v1/search?q={{searchTerms}}\"/>\n\
         </OpenSearchDescription>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    #[test]
    fn descriptor_is_well_formed_with_template_placeholder() {
        let xml = descriptor("http://host:3000");

        let mut reader = Reader::from_str(&xml);
        let mut depth = 0i32;
        loop {
            match reader.read_event() {
                Ok(Event::Start(_)) => depth += 1,
                Ok(Event::End(_)) => depth -= 1,
                Ok(Event::Eof) => break,
                Err(e) => panic!("malformed OpenSearch XML: {e}"),
                _ => {}
            }
        }
        assert_eq!(depth, 0);
        assert!(xml.contains("<ShortName>LongBox</ShortName>"));
        assert!(xml.contains("template=\"http://host:3000/opds/v1/search?q={searchTerms}\""));
    }

    #[test]
    fn base_url_is_escaped() {
        // A stray & in the base must not break the XML.
        let xml = descriptor("http://h/?a=1&b=2");
        assert!(xml.contains("a=1&amp;b=2"));
    }
}
