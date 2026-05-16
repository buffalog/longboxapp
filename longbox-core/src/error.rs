use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid issue number: {0:?}")]
    InvalidIssueNumber(String),

    #[error("ComicInfo XML parse error: {0}")]
    ComicInfoParse(String),

    #[error("invalid regex pattern {name:?}: {source}")]
    InvalidPattern {
        name: String,
        #[source]
        source: regex::Error,
    },
}

pub type Result<T> = core::result::Result<T, CoreError>;
