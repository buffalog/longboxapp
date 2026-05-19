//! Result selection — pick the single best release for an issue.
//!
//! Sort key (brief): cbz preferred over cbr over unknown, then higher
//! `grabs` count, then more recent `pubDate`. Both cbz and cbr are
//! acceptable — format is only a tie-ordering preference.

use std::cmp::Ordering;

use crate::types::{ArchiveFormat, Release};

/// Infer archive format from a release title. Newznab has no
/// structured format field; sniff the `.cbz` / `.cbr` substring.
pub fn archive_format(title: &str) -> ArchiveFormat {
    let lower = title.to_lowercase();
    if lower.contains(".cbz") {
        ArchiveFormat::Cbz
    } else if lower.contains(".cbr") {
        ArchiveFormat::Cbr
    } else {
        ArchiveFormat::Unknown
    }
}

fn format_rank(title: &str) -> u8 {
    match archive_format(title) {
        ArchiveFormat::Cbz => 0,
        ArchiveFormat::Cbr => 1,
        ArchiveFormat::Unknown => 2,
    }
}

/// Pick the best release from a non-prioritized pool, or `None` when
/// the pool is empty.
pub fn select_best(mut releases: Vec<Release>) -> Option<Release> {
    releases.sort_by(cmp_releases);
    releases.into_iter().next()
}

/// Ordering where "less" = "better" (sorts the winner to the front).
fn cmp_releases(a: &Release, b: &Release) -> Ordering {
    format_rank(&a.title)
        .cmp(&format_rank(&b.title))
        // higher grabs first
        .then_with(|| b.grabs.unwrap_or(0).cmp(&a.grabs.unwrap_or(0)))
        // more recent first — None (unknown date) sorts last
        .then_with(|| b.published.cmp(&a.published))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn release(title: &str, grabs: Option<i64>) -> Release {
        Release {
            title: title.into(),
            nzb_url: format!("https://x/{title}"),
            guid: title.into(),
            published: None,
            size_bytes: None,
            grabs,
            category: None,
        }
    }

    #[test]
    fn detects_archive_format_from_title() {
        assert_eq!(archive_format("Wolverine 005.cbz"), ArchiveFormat::Cbz);
        assert_eq!(archive_format("Wolverine 005.CBR"), ArchiveFormat::Cbr);
        assert_eq!(archive_format("Wolverine 005"), ArchiveFormat::Unknown);
    }

    #[test]
    fn prefers_cbz_over_cbr_even_with_fewer_grabs() {
        let pool = vec![
            release("Wolverine 005.cbr", Some(100)),
            release("Wolverine 005.cbz", Some(5)),
        ];
        assert_eq!(select_best(pool).unwrap().title, "Wolverine 005.cbz");
    }

    #[test]
    fn within_same_format_prefers_higher_grabs() {
        let pool = vec![
            release("a.cbz", Some(5)),
            release("b.cbz", Some(80)),
            release("c.cbz", Some(40)),
        ];
        assert_eq!(select_best(pool).unwrap().title, "b.cbz");
    }

    #[test]
    fn recency_breaks_grab_ties() {
        let mut older = release("old.cbz", Some(10));
        older.published = Some(datetime!(2024-01-01 0:00 UTC));
        let mut newer = release("new.cbz", Some(10));
        newer.published = Some(datetime!(2025-01-01 0:00 UTC));
        let pool = vec![older, newer];
        assert_eq!(select_best(pool).unwrap().title, "new.cbz");
    }

    #[test]
    fn cbr_still_selectable_when_no_cbz() {
        let pool = vec![release("only.cbr", Some(1))];
        assert_eq!(select_best(pool).unwrap().title, "only.cbr");
    }

    #[test]
    fn empty_pool_is_none() {
        assert!(select_best(vec![]).is_none());
    }
}
