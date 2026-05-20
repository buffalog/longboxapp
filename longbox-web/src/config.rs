use std::path::PathBuf;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub comicvine_api_key: String,
    pub library_root_path: String,
    pub database_url: String,
    pub bind_addr: String,
    pub log_level: String,
    pub match_threshold: f64,
    pub cors_permissive: bool,
    /// Phase B watch folder. When set + readable, the web layer
    /// starts the post-process watcher at boot. Unset = Phase B not
    /// enabled; unreadable = warn-and-skip.
    pub download_watch_path: Option<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("required environment variable not set: {0}")]
    MissingRequired(&'static str),

    #[error("env var {var} has invalid value {value:?}: {reason}")]
    InvalidValue {
        var: &'static str,
        value: String,
        reason: String,
    },

    #[error("library root path {path:?} does not exist or is not a readable directory")]
    LibraryRootNotReadable { path: PathBuf },
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let comicvine_api_key = required_non_empty("COMICVINE_API_KEY")?;
        let library_root_path = required_non_empty("LIBRARY_ROOT_PATH")?;

        // Validate library root exists and is readable.
        let path = PathBuf::from(&library_root_path);
        if !path.is_dir() {
            return Err(ConfigError::LibraryRootNotReadable { path });
        }
        // Cheap readable check: try to read the directory.
        std::fs::read_dir(&path)
            .map_err(|_| ConfigError::LibraryRootNotReadable { path: path.clone() })?;

        let database_url =
            optional("DATABASE_URL").unwrap_or_else(|| "sqlite:./longbox.db?mode=rwc".to_owned());

        let bind_addr = optional("BIND_ADDR").unwrap_or_else(|| "0.0.0.0:3000".to_owned());

        let log_level = optional("LOG_LEVEL").unwrap_or_else(|| "info".to_owned());

        let match_threshold = match optional("MATCH_THRESHOLD") {
            Some(raw) => parse_f64_in_range("MATCH_THRESHOLD", &raw, 0.0, 1.0)?,
            None => 0.85,
        };

        let cors_permissive = match optional("CORS_PERMISSIVE") {
            Some(raw) => parse_bool("CORS_PERMISSIVE", &raw)?,
            None => false,
        };

        let download_watch_path = optional("DOWNLOAD_WATCH_PATH");

        Ok(Self {
            comicvine_api_key,
            library_root_path: normalize_path(&library_root_path),
            database_url,
            bind_addr,
            log_level,
            match_threshold,
            cors_permissive,
            download_watch_path,
        })
    }
}

/// Normalize a path string per Step 5 bootstrap rules: strip trailing slashes
/// except for the root `/` itself. No symlink resolution, no canonicalization,
/// no component parsing. Same normalizer is applied to both the configured
/// value and the stored row before comparison.
pub fn normalize_path(raw: &str) -> String {
    if raw == "/" {
        return raw.to_owned();
    }
    raw.trim_end_matches('/').to_owned()
}

fn required_non_empty(var: &'static str) -> Result<String, ConfigError> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_owned()),
        _ => Err(ConfigError::MissingRequired(var)),
    }
}

fn optional(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_owned())
}

fn parse_f64_in_range(
    var: &'static str,
    raw: &str,
    min: f64,
    max: f64,
) -> Result<f64, ConfigError> {
    let v = f64::from_str(raw).map_err(|_| ConfigError::InvalidValue {
        var,
        value: raw.to_owned(),
        reason: "expected a floating-point number".into(),
    })?;
    if !(min..=max).contains(&v) {
        return Err(ConfigError::InvalidValue {
            var,
            value: raw.to_owned(),
            reason: format!("expected value in [{min}, {max}]"),
        });
    }
    Ok(v)
}

fn parse_bool(var: &'static str, raw: &str) -> Result<bool, ConfigError> {
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidValue {
            var,
            value: raw.to_owned(),
            reason: "expected boolean (true/false/1/0/yes/no/on/off)".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_strips_trailing_slash() {
        assert_eq!(normalize_path("/comics/"), "/comics");
        assert_eq!(normalize_path("/comics//"), "/comics");
        assert_eq!(normalize_path("/comics"), "/comics");
    }

    #[test]
    fn normalize_path_preserves_root() {
        assert_eq!(normalize_path("/"), "/");
    }

    #[test]
    fn parse_bool_accepts_common_forms() {
        assert!(parse_bool("X", "true").unwrap());
        assert!(parse_bool("X", "1").unwrap());
        assert!(parse_bool("X", "yes").unwrap());
        assert!(!parse_bool("X", "false").unwrap());
        assert!(!parse_bool("X", "0").unwrap());
        assert!(parse_bool("X", "banana").is_err());
    }

    #[test]
    fn parse_f64_in_range_rejects_out_of_bounds() {
        assert!(parse_f64_in_range("T", "0.5", 0.0, 1.0).is_ok());
        assert!(parse_f64_in_range("T", "1.5", 0.0, 1.0).is_err());
        assert!(parse_f64_in_range("T", "-0.1", 0.0, 1.0).is_err());
        assert!(parse_f64_in_range("T", "not a number", 0.0, 1.0).is_err());
    }
}
