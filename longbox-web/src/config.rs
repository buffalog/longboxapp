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
    /// Metron API username (Basic Auth). Optional — the
    /// `metron_enabled` settings row is the actual gate. When
    /// `metron_enabled = true` AND this is None/empty, bootstrap
    /// logs warn and degrades to None on the MetronClient handle.
    /// Source: `METRON_API_USER` env var.
    pub metron_api_user: Option<String>,
    /// Metron API password. See [`Self::metron_api_user`]. Source:
    /// `METRON_API_PASSWORD` env var.
    pub metron_api_password: Option<String>,
    /// Wall-clock time the daily pull sweep fires, from
    /// `PULL_SCHEDULE_TIME` (`HH:MM`, default 05:00). Interpreted as
    /// UTC by the pull scheduler — see `longbox_pull::PullConfig`.
    pub pull_schedule_time: time::Time,
    /// Wall-clock time the daily library scan fires, from
    /// `SCAN_SCHEDULE_TIME` (`HH:MM`, default 03:00 — before the pull
    /// sweep, so the catalog is fresh). UTC, same rationale as above.
    pub scan_schedule_time: time::Time,
    /// Host-side mirror of the library bind-mount source path. Seed
    /// value for the `host_library_path` settings row; the UI's "Show
    /// in Finder" affordance reads the live row at request time and
    /// falls back to this when the row is empty. `None` when the env
    /// var is unset — operators on bare-metal deployments (no
    /// container) typically don't need it and the UI keeps a copy-only
    /// display. Source: `HOST_LIBRARY_PATH` env var.
    pub host_library_path: Option<String>,
    /// Public base URL the OPDS catalog advertises in feed links and the
    /// OpenSearch descriptor. When set, hrefs are absolute and reachable
    /// from the reader device (a LAN IP, Tailscale host, or domain — not a
    /// container-internal address); trailing slash stripped. Empty by
    /// default, in which case hrefs are emitted relative (path-only), which
    /// resolves correctly when the reader reaches LongBox at the same
    /// origin it fetched the feed from. Source: `OPDS_BASE_URL` env var.
    pub opds_base_url: String,
    /// Directory the OPDS cover proxy caches fetched cover art in, as
    /// `{issue_id}.jpg`. Lives inside the longbox-db Docker volume so it
    /// survives restarts without a new mount. Source: `OPDS_COVERS_DIR`
    /// env var; defaults to `/data/covers`.
    pub opds_covers_dir: PathBuf,
    /// When false (default), the OPDS cover proxy refuses to fetch
    /// `cover_url`s whose host is a loopback/private/link-local IP literal
    /// (an SSRF guard). Set true only when covers legitimately live on a
    /// private host reachable from the container. Source:
    /// `OPDS_ALLOW_PRIVATE_COVER_HOSTS` env var.
    pub opds_allow_private_cover_hosts: bool,
    /// TCP port for the dedicated OPDS-only listener, bound on `0.0.0.0`
    /// alongside the main app. It serves `/opds/v1/*` only (404 for anything
    /// else) so a reader can be pointed at a port with no admin surface.
    /// Source: `OPDS_PORT` env var; defaults to 8096.
    pub opds_port: u16,
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

        // Metron credentials. Both optional at this layer — the
        // `metron_enabled` settings row is the actual feature gate;
        // bootstrap collapses any (enabled, missing-creds) state to
        // a None MetronClient handle with a warn log, no hard fail.
        let metron_api_user = optional("METRON_API_USER");
        let metron_api_password = optional("METRON_API_PASSWORD");

        let pull_schedule_time = match optional("PULL_SCHEDULE_TIME") {
            Some(raw) => parse_schedule_time("PULL_SCHEDULE_TIME", &raw)?,
            None => time::macros::time!(05:00),
        };

        let scan_schedule_time = match optional("SCAN_SCHEDULE_TIME") {
            Some(raw) => parse_schedule_time("SCAN_SCHEDULE_TIME", &raw)?,
            None => time::macros::time!(03:00),
        };

        // Host-side library path. Stripped of trailing slashes
        // up-front so the consumers' substitution code doesn't have to
        // re-trim the value on every call.
        let host_library_path = optional("HOST_LIBRARY_PATH").map(|p| normalize_path(&p));

        // Public OPDS base URL. Normalized (trailing slash stripped) so
        // href construction can unconditionally append `/opds/v1/...`.
        // Empty when unset → relative hrefs.
        let opds_base_url = optional("OPDS_BASE_URL")
            .map(|u| normalize_path(&u))
            .unwrap_or_default();

        let opds_covers_dir = optional("OPDS_COVERS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/data/covers"));

        let opds_allow_private_cover_hosts = match optional("OPDS_ALLOW_PRIVATE_COVER_HOSTS") {
            Some(raw) => parse_bool("OPDS_ALLOW_PRIVATE_COVER_HOSTS", &raw)?,
            None => false,
        };

        let opds_port = match optional("OPDS_PORT") {
            Some(raw) => raw.parse::<u16>().map_err(|_| ConfigError::InvalidValue {
                var: "OPDS_PORT",
                value: raw,
                reason: "expected a TCP port number (1-65535)".into(),
            })?,
            None => 8096,
        };

        Ok(Self {
            comicvine_api_key,
            library_root_path: normalize_path(&library_root_path),
            database_url,
            bind_addr,
            log_level,
            match_threshold,
            cors_permissive,
            download_watch_path,
            metron_api_user,
            metron_api_password,
            pull_schedule_time,
            scan_schedule_time,
            host_library_path,
            opds_base_url,
            opds_covers_dir,
            opds_allow_private_cover_hosts,
            opds_port,
        })
    }
}

/// Parse a `HH:MM` 24-hour wall-clock string into a [`time::Time`].
/// The schedulers interpret the result as UTC. `var` names the source
/// env var for the error message.
fn parse_schedule_time(var: &'static str, raw: &str) -> Result<time::Time, ConfigError> {
    let invalid = |reason: &str| ConfigError::InvalidValue {
        var,
        value: raw.to_owned(),
        reason: reason.to_owned(),
    };
    let (h, m) = raw
        .split_once(':')
        .ok_or_else(|| invalid("expected HH:MM"))?;
    let hour: u8 = h
        .trim()
        .parse()
        .map_err(|_| invalid("hour is not a number"))?;
    let minute: u8 = m
        .trim()
        .parse()
        .map_err(|_| invalid("minute is not a number"))?;
    time::Time::from_hms(hour, minute, 0).map_err(|_| invalid("hour must be 0-23, minute 0-59"))
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

    #[test]
    fn parse_schedule_time_accepts_hh_mm_and_rejects_garbage() {
        let p = |raw| parse_schedule_time("SCHEDULE_TIME", raw);
        assert_eq!(p("05:00").unwrap(), time::macros::time!(05:00));
        assert_eq!(p("23:30").unwrap(), time::macros::time!(23:30));
        assert!(p("5am").is_err());
        assert!(p("24:00").is_err());
        assert!(p("05:99").is_err());
    }
}
