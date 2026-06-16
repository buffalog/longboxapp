//! OPDS access control: HTTP Basic + Bearer-token verification.
//!
//! Two mechanisms, both checked on every OPDS request:
//!
//! 1. **HTTP Basic** — username + bcrypt-hashed password stored in the
//!    `settings` table.
//! 2. **Bearer token** — a 32-byte hex token, accepted as an alternative
//!    to Basic on every endpoint.
//!
//! All verification here is pure (no DB, no HTTP): the web layer loads the
//! four `settings` rows into [`OpdsAccess`], reads the request's
//! `Authorization` header, and calls [`OpdsAccess::authorize`]. The
//! resulting [`AuthOutcome`] maps directly to a 503 / 200-passthrough /
//! 401 response.

use base64::Engine as _;
use subtle::ConstantTimeEq;

/// Realm advertised in the `WWW-Authenticate` header on 401 responses.
pub const REALM: &str = "LongBox";

/// The four OPDS `settings` rows, loaded by the web layer. Verification is
/// pure so it can be exercised without a database.
#[derive(Debug, Clone)]
pub struct OpdsAccess {
    /// `opds_enabled`. When false, every OPDS route returns 503.
    pub enabled: bool,
    /// `opds_username`. Empty/absent is treated the same as disabled
    /// (503): OPDS is "not configured" until a username exists.
    pub username: Option<String>,
    /// `opds_password_hash` (bcrypt).
    pub password_hash: Option<String>,
    /// `opds_api_token` (32-byte hex). Bearer alternative to Basic.
    pub api_token: Option<String>,
}

/// Result of authorizing a request, mapped to an HTTP status by the caller.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthOutcome {
    /// OPDS is disabled or has no username → `503 Service Unavailable`.
    Unconfigured,
    /// Credentials accepted → continue to the handler.
    Authorized,
    /// Missing or invalid credentials → `401` + `WWW-Authenticate: Basic`.
    Unauthorized,
}

impl OpdsAccess {
    /// OPDS is usable only when the toggle is on AND a non-empty username
    /// is set. Per spec, a disabled toggle and an unconfigured username
    /// both surface as 503.
    fn is_configured(&self) -> bool {
        self.enabled
            && self
                .username
                .as_deref()
                .is_some_and(|u| !u.trim().is_empty())
    }

    /// Authorize a request given the raw `Authorization` header value
    /// (`None` when the header is absent).
    pub fn authorize(&self, authorization: Option<&str>) -> AuthOutcome {
        if !self.is_configured() {
            return AuthOutcome::Unconfigured;
        }
        match authorization.and_then(parse_authorization) {
            Some(Credential::Bearer(token)) if self.verify_token(&token) => AuthOutcome::Authorized,
            Some(Credential::Basic { username, password })
                if self.verify_basic(&username, &password) =>
            {
                AuthOutcome::Authorized
            }
            _ => AuthOutcome::Unauthorized,
        }
    }

    /// Constant-time comparison of a presented bearer token against the
    /// stored one. A null/empty stored token never matches.
    fn verify_token(&self, presented: &str) -> bool {
        match self.api_token.as_deref() {
            Some(expected) if !expected.is_empty() => {
                presented.as_bytes().ct_eq(expected.as_bytes()).into()
            }
            _ => false,
        }
    }

    /// Verify a Basic username/password against the stored username and
    /// bcrypt hash. Both must be present; the username is compared in
    /// constant time (cheap, and avoids leaking which half failed), then
    /// the password is checked with bcrypt.
    fn verify_basic(&self, username: &str, password: &str) -> bool {
        let (Some(expected_user), Some(hash)) =
            (self.username.as_deref(), self.password_hash.as_deref())
        else {
            return false;
        };
        let user_ok: bool = username.as_bytes().ct_eq(expected_user.as_bytes()).into();
        // Run bcrypt unconditionally and combine with a bitwise `&` (NOT a
        // short-circuiting `&&`): a wrong username must cost the same ~250ms
        // as a wrong password, or the early-out leaks — via response timing —
        // whether the username was valid (an enumeration oracle that would
        // defeat the constant-time compare above). bcrypt::verify errs only on
        // a malformed stored hash; treat that as a non-match, never a panic.
        let password_ok = bcrypt::verify(password, hash).unwrap_or(false);
        user_ok & password_ok
    }
}

/// A parsed `Authorization` credential.
enum Credential {
    Basic { username: String, password: String },
    Bearer(String),
}

/// Parse an `Authorization` header value into a [`Credential`]. Returns
/// `None` for an unknown scheme, malformed base64, or non-UTF-8 / colon-less
/// Basic payloads.
fn parse_authorization(header: &str) -> Option<Credential> {
    let (scheme, rest) = header.split_once(' ')?;
    let rest = rest.trim();
    if scheme.eq_ignore_ascii_case("bearer") {
        if rest.is_empty() {
            return None;
        }
        Some(Credential::Bearer(rest.to_owned()))
    } else if scheme.eq_ignore_ascii_case("basic") {
        let decoded = base64::engine::general_purpose::STANDARD.decode(rest).ok()?;
        let text = String::from_utf8(decoded).ok()?;
        // RFC 7617: split on the FIRST colon; passwords may contain colons.
        let (username, password) = text.split_once(':')?;
        Some(Credential::Basic {
            username: username.to_owned(),
            password: password.to_owned(),
        })
    } else {
        None
    }
}

/// Errors from OPDS credential setup (hashing). Wrapped so callers don't
/// depend on the `bcrypt` error type directly.
#[derive(Debug, thiserror::Error)]
pub enum OpdsAuthError {
    #[error("failed to hash OPDS password: {0}")]
    Hash(String),
}

/// bcrypt-hash a plaintext password at the default cost. Stored in the
/// `opds_password_hash` settings row.
pub fn hash_password(password: &str) -> Result<String, OpdsAuthError> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| OpdsAuthError::Hash(e.to_string()))
}

/// Generate a fresh API token: 32 random bytes, hex-encoded (64 chars).
/// Stored in the `opds_api_token` settings row on first OPDS save and on
/// each "Regenerate".
pub fn generate_api_token() -> String {
    use rand::RngCore as _;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// bcrypt hash of the password "hunter2" at cost 12. Precomputed so
    /// the auth tests don't pay the hashing cost on every run.
    fn access_fixture() -> OpdsAccess {
        OpdsAccess {
            enabled: true,
            username: Some("reader".into()),
            password_hash: Some(hash_password("hunter2").unwrap()),
            api_token: Some("a".repeat(64)),
        }
    }

    fn basic_header(user: &str, pass: &str) -> String {
        let raw = format!("{user}:{pass}");
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    #[test]
    fn unconfigured_when_disabled() {
        let mut access = access_fixture();
        access.enabled = false;
        assert_eq!(
            access.authorize(Some(&basic_header("reader", "hunter2"))),
            AuthOutcome::Unconfigured
        );
    }

    #[test]
    fn unconfigured_when_username_empty_or_absent() {
        let mut access = access_fixture();
        access.username = Some("   ".into());
        assert_eq!(access.authorize(None), AuthOutcome::Unconfigured);
        access.username = None;
        assert_eq!(access.authorize(None), AuthOutcome::Unconfigured);
    }

    #[test]
    fn unauthorized_when_no_header() {
        assert_eq!(access_fixture().authorize(None), AuthOutcome::Unauthorized);
    }

    #[test]
    fn authorized_with_correct_basic() {
        assert_eq!(
            access_fixture().authorize(Some(&basic_header("reader", "hunter2"))),
            AuthOutcome::Authorized
        );
    }

    #[test]
    fn unauthorized_with_wrong_password() {
        assert_eq!(
            access_fixture().authorize(Some(&basic_header("reader", "wrong"))),
            AuthOutcome::Unauthorized
        );
    }

    #[test]
    fn unauthorized_with_wrong_username() {
        assert_eq!(
            access_fixture().authorize(Some(&basic_header("intruder", "hunter2"))),
            AuthOutcome::Unauthorized
        );
    }

    #[test]
    fn authorized_with_correct_bearer() {
        assert_eq!(
            access_fixture().authorize(Some(&format!("Bearer {}", "a".repeat(64)))),
            AuthOutcome::Authorized
        );
    }

    #[test]
    fn unauthorized_with_wrong_bearer() {
        assert_eq!(
            access_fixture().authorize(Some("Bearer deadbeef")),
            AuthOutcome::Unauthorized
        );
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert_eq!(
            access_fixture().authorize(Some(&format!("bearer {}", "a".repeat(64)))),
            AuthOutcome::Authorized
        );
    }

    #[test]
    fn password_may_contain_colons() {
        let mut access = access_fixture();
        access.password_hash = Some(hash_password("pa:ss:word").unwrap());
        assert_eq!(
            access.authorize(Some(&basic_header("reader", "pa:ss:word"))),
            AuthOutcome::Authorized
        );
    }

    #[test]
    fn empty_stored_token_never_matches() {
        let mut access = access_fixture();
        access.api_token = Some(String::new());
        assert_eq!(
            access.authorize(Some("Bearer ")),
            AuthOutcome::Unauthorized
        );
    }

    #[test]
    fn malformed_basic_payload_is_unauthorized() {
        let mut access = access_fixture();
        access.enabled = true;
        // Not valid base64.
        assert_eq!(
            access.authorize(Some("Basic !!!notbase64!!!")),
            AuthOutcome::Unauthorized
        );
        // Valid base64 but no colon.
        let no_colon = base64::engine::general_purpose::STANDARD.encode("nocolon");
        assert_eq!(
            access.authorize(Some(&format!("Basic {no_colon}"))),
            AuthOutcome::Unauthorized
        );
    }

    #[test]
    fn hash_password_roundtrips_with_verify() {
        let hash = hash_password("s3cret").unwrap();
        assert!(bcrypt::verify("s3cret", &hash).unwrap());
        assert!(!bcrypt::verify("nope", &hash).unwrap());
    }

    #[test]
    fn generated_token_is_64_hex_chars() {
        let token = generate_api_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        // Two successive tokens differ (randomness sanity check).
        assert_ne!(token, generate_api_token());
    }
}
