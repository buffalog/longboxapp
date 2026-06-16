//! OPDS 1.2 catalog support for LongBox.
//!
//! This crate holds the *pure* OPDS logic — access control today, and
//! Atom/OPDS feed construction in later commits — with no web framework
//! or database dependency, so every piece is unit-testable in isolation.
//! The Axum handlers and the auth middleware that wire this into the HTTP
//! surface live in `longbox-web`'s `routes::opds` module and call into
//! here.

pub mod auth;

pub use auth::{generate_api_token, hash_password, AuthOutcome, OpdsAccess, OpdsAuthError};
