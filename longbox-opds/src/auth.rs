//! OPDS access control: HTTP Basic credential parsing + bcrypt verification.
//!
//! Per-user accounts live in the `opds_users` table (see
//! `longbox_db::opds_users_repo`); the lookup and `last_seen` bookkeeping are
//! the web middleware's job. This module holds only the *pure* pieces — header
//! parsing and password hashing/verification — so they stay unit-testable
//! without a database. HTTP Basic is the only accepted scheme (the former
//! Bearer-token path was removed: per-user accounts make a shared token
//! redundant).

use std::sync::OnceLock;

use base64::Engine as _;

/// Realm advertised in the `WWW-Authenticate` header on 401 responses.
pub const REALM: &str = "LongBox";

/// Parse an `Authorization` header value into `(username, password)`. Returns
/// `None` for a missing/unknown scheme, malformed base64, or a non-UTF-8 /
/// colon-less Basic payload. Bearer (and everything else) yields `None`.
pub fn parse_basic(header: &str) -> Option<(String, String)> {
    let (scheme, rest) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(rest.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    // RFC 7617: split on the FIRST colon; passwords may contain colons.
    let (username, password) = text.split_once(':')?;
    Some((username.to_owned(), password.to_owned()))
}

/// Verify a plaintext password against a stored bcrypt hash. A malformed
/// stored hash is a non-match, never a panic.
pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

/// Burn a bcrypt verification against a throwaway hash, always returning
/// `false`. The middleware calls this on the user-not-found path so an
/// unknown username costs the same ~250ms as a wrong password — without it,
/// response timing would leak which usernames exist (an enumeration oracle).
/// The dummy hash is computed once and cached.
pub fn dummy_verify(password: &str) -> bool {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY.get_or_init(|| {
        hash_password("longbox-not-a-real-password").expect("hash dummy password")
    });
    let _ = bcrypt::verify(password, hash);
    false
}

/// Errors from OPDS credential setup (hashing). Wrapped so callers don't
/// depend on the `bcrypt` error type directly.
#[derive(Debug, thiserror::Error)]
pub enum OpdsAuthError {
    #[error("failed to hash OPDS password: {0}")]
    Hash(String),
}

/// bcrypt-hash a plaintext password at the default cost. Stored in the
/// `opds_users.password_hash` column.
pub fn hash_password(password: &str) -> Result<String, OpdsAuthError> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| OpdsAuthError::Hash(e.to_string()))
}

/// Generate a fresh API token: 32 random bytes, hex-encoded (64 chars).
///
/// DEPRECATED: the legacy single-credential admin surface still calls this;
/// it is removed when that surface is replaced by per-user account management.
pub fn generate_api_token() -> String {
    use rand::RngCore as _;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_header(user: &str, pass: &str) -> String {
        let raw = format!("{user}:{pass}");
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    #[test]
    fn parse_basic_extracts_credentials() {
        assert_eq!(
            parse_basic(&basic_header("reader", "hunter2")),
            Some(("reader".to_owned(), "hunter2".to_owned()))
        );
    }

    #[test]
    fn parse_basic_is_scheme_case_insensitive() {
        let raw = base64::engine::general_purpose::STANDARD.encode("reader:hunter2");
        assert_eq!(
            parse_basic(&format!("basic {raw}")),
            Some(("reader".to_owned(), "hunter2".to_owned()))
        );
    }

    #[test]
    fn parse_basic_rejects_bearer_and_other_schemes() {
        assert!(parse_basic("Bearer sometoken").is_none());
        assert!(parse_basic("Digest foo").is_none());
    }

    #[test]
    fn parse_basic_password_may_contain_colons() {
        assert_eq!(
            parse_basic(&basic_header("reader", "pa:ss:word")),
            Some(("reader".to_owned(), "pa:ss:word".to_owned()))
        );
    }

    #[test]
    fn parse_basic_rejects_malformed_payloads() {
        assert!(parse_basic("Basic !!!notbase64!!!").is_none());
        let no_colon = base64::engine::general_purpose::STANDARD.encode("nocolon");
        assert!(parse_basic(&format!("Basic {no_colon}")).is_none());
        assert!(parse_basic("Basic").is_none());
    }

    #[test]
    fn verify_password_roundtrips_with_hash() {
        let hash = hash_password("s3cret").unwrap();
        assert!(verify_password("s3cret", &hash));
        assert!(!verify_password("nope", &hash));
    }

    #[test]
    fn verify_password_on_garbage_hash_is_false_not_panic() {
        assert!(!verify_password("anything", "not-a-bcrypt-hash"));
    }

    #[test]
    fn dummy_verify_is_always_false() {
        assert!(!dummy_verify("whatever"));
    }
}
