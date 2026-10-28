//! Strip secrets from text that gets logged, stored or shown.
//!
//! Downloader and indexer URLs carry credentials: SABnzbd's `apikey=`,
//! the indexer's NZB link with its own key and `r=` token. An error that
//! echoes a request URL (reqwest's Display does, SAB's own error text can)
//! ended up in `pull_attempts.error_message` and on Needs Attention.
//! `without_url()` at each reqwest call is the first line; this is the
//! second, for every error string before it's stored or displayed.

use std::sync::OnceLock;

use regex::Regex;

/// `text` with every URL, `apikey=`/`api_key=` value and 32-64 char hex
/// token replaced by a placeholder.
pub fn secrets(text: &str) -> String {
    static RULES: OnceLock<[(Regex, &str); 3]> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            // Any scheme://… up to whitespace or a closing quote/bracket.
            // Takes percent-encoded query values (an NZB link inside
            // SAB's `name=`) with it.
            (
                Regex::new(r#"(?i)\b[a-z][a-z0-9+.\-]*://[^\s"'<>()\]]+"#).unwrap(),
                "<url>",
            ),
            (
                Regex::new(r#"(?i)\b(api_?key)\s*[=:]\s*[^\s&"',;)]+"#).unwrap(),
                "${1}=<redacted>",
            ),
            (Regex::new(r"\b[0-9a-fA-F]{32,64}\b").unwrap(), "<redacted>"),
        ]
    });
    rules.iter().fold(text.to_owned(), |t, (re, with)| {
        re.replace_all(&t, *with).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_urls_keys_and_hex_tokens() {
        // The live I, Tyrant #2 row's shape, with made-up values.
        let live = "submit failed: http request failed: SABnzbd request failed: error sending \
                    request for url (http://host.docker.internal:8081/api?mode=addurl&name=https%3A%2F%2Findexer.example%2Fgetnzb%2F0123456789abcdef0123456789abcdef.nzb%26r%3Dfedcba9876543210fedcba9876543210&apikey=00112233445566778899aabbccddeeff&output=json)";
        let out = secrets(live);
        assert_eq!(
            out,
            "submit failed: http request failed: SABnzbd request failed: error sending \
             request for url (<url>)"
        );

        for (input, want) in [
            ("bad apikey=abc123 here", "bad apikey=<redacted> here"),
            ("API_KEY: s3cret, then", "API_KEY=<redacted>, then"),
            (
                "token 0123456789ABCDEF0123456789ABCDEF ok",
                "token <redacted> ok",
            ),
            ("nzb at ftp://x.example/a.nzb.", "nzb at <url>"),
        ] {
            assert_eq!(secrets(input), want, "{input}");
        }

        // Ordinary error text survives untouched.
        for keep in [
            "downloaded archive is unreadable: File CRC error",
            "indexer returned 2 results, best similarity 0.40 below threshold 0.75",
            "SABnzbd returned HTTP 503 Service Unavailable",
            "nzo id SABnzbd_nzo_abc123 short hex deadbeef",
        ] {
            assert_eq!(secrets(keep), keep);
        }
    }
}
