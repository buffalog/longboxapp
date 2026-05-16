//! `Series` domain type. The constructor computes `sort_title` via
//! [`normalize_title`] so that both sides of a matcher comparison live in the
//! same normalized space.

use serde::{Deserialize, Serialize};

use crate::normalize::normalize_title;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    /// ComicVine volume ID. Nullable; unique when set.
    pub cv_id: Option<i64>,
    /// Metron series slug. Reserved for Phase B; never written in Phase A.
    pub metron_id: Option<String>,
    pub title: String,
    /// Normalized form of `title` used for similarity matching. Always
    /// recomputable from `title` via [`normalize_title`].
    pub sort_title: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

impl Series {
    /// Construct a series with `sort_title` computed from `title`.
    pub fn new(
        id: i64,
        cv_id: Option<i64>,
        title: impl Into<String>,
        start_year: Option<i32>,
    ) -> Self {
        let title = title.into();
        let sort_title = normalize_title(&title);
        Self {
            id,
            cv_id,
            metron_id: None,
            title,
            sort_title,
            start_year,
            publisher: None,
            description: None,
            cover_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_computes_sort_title() {
        let s = Series::new(1, Some(42), "The Walking Dead", Some(2003));
        assert_eq!(s.title, "The Walking Dead");
        assert_eq!(s.sort_title, "walking dead");
        assert_eq!(s.cv_id, Some(42));
        assert_eq!(s.start_year, Some(2003));
    }

    #[test]
    fn sort_title_equals_normalized_title() {
        let s = Series::new(1, None, "Spider-Man: Far From Home", None);
        assert_eq!(s.sort_title, normalize_title(&s.title));
    }

    #[test]
    fn metron_id_default_none() {
        let s = Series::new(1, None, "Saga", None);
        assert!(s.metron_id.is_none());
    }
}
