use thiserror::Error;

/// Errors surfaced from the longbox-db crate.
///
/// Application code matches on these instead of inspecting raw SQLite error
/// codes. `UniqueViolation` carries a static field name (e.g. `"cv_id"`,
/// `"issues_series_id_number"`) so callers can produce typed HTTP responses
/// without parsing strings.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("row not found")]
    NotFound,

    #[error("unique constraint violated on field: {field}")]
    UniqueViolation { field: &'static str },

    #[error("migration failed: {0}")]
    MigrationFailed(String),

    #[error(transparent)]
    Other(sqlx::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::RowNotFound = &err {
            return DbError::NotFound;
        }
        if let sqlx::Error::Database(db_err) = &err {
            let code = db_err.code();
            // SQLite extended error codes: 2067 = SQLITE_CONSTRAINT_UNIQUE,
            // 1555 = SQLITE_CONSTRAINT_PRIMARYKEY. Both indicate a unique
            // constraint violation.
            if matches!(code.as_deref(), Some("2067" | "1555")) {
                if let Some(field) = parse_unique_violation(db_err.message()) {
                    return DbError::UniqueViolation { field };
                }
            }
        }
        DbError::Other(err)
    }
}

/// Map SQLite's `UNIQUE constraint failed: <table>.<col>[, <table>.<col>]`
/// message text to the canonical field name expected by Phase A's tests.
/// Composite-column constraints are flattened to `<table>_<col1>_<col2>`.
fn parse_unique_violation(msg: &str) -> Option<&'static str> {
    let after = msg.strip_prefix("UNIQUE constraint failed: ")?;
    Some(match after.trim() {
        "series.cv_id" => "cv_id",
        "series.metron_id" => "metron_id",
        "issues.cv_issue_id" => "cv_issue_id",
        "issues.metron_issue_id" => "metron_issue_id",
        "issues.series_id, issues.number" => "issues_series_id_number",
        "library_roots.path" => "path",
        "files.library_root_id, files.path_relative" => "files_library_root_id_path_relative",
        "publisher_filters.publisher_name" => "publisher_name",
        "indexer_configs.name" => "indexer_name",
        "webhook_configs.name" => "webhook_name",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_violations() {
        assert_eq!(
            parse_unique_violation("UNIQUE constraint failed: series.cv_id"),
            Some("cv_id")
        );
        assert_eq!(
            parse_unique_violation("UNIQUE constraint failed: library_roots.path"),
            Some("path")
        );
        assert_eq!(
            parse_unique_violation("UNIQUE constraint failed: issues.series_id, issues.number"),
            Some("issues_series_id_number")
        );
        assert_eq!(
            parse_unique_violation("UNIQUE constraint failed: indexer_configs.name"),
            Some("indexer_name")
        );
        assert_eq!(
            parse_unique_violation("UNIQUE constraint failed: webhook_configs.name"),
            Some("webhook_name")
        );
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_unique_violation("FOREIGN KEY constraint failed").is_none());
        assert!(parse_unique_violation("UNIQUE constraint failed: nonexistent.col").is_none());
    }
}
