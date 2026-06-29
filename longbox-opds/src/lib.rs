//! OPDS 1.2 catalog support for LongBox.
//!
//! This crate holds the *pure* OPDS logic — access control today, and
//! Atom/OPDS feed construction in later commits — with no web framework
//! or database dependency, so every piece is unit-testable in isolation.
//! The Axum handlers and the auth middleware that wire this into the HTTP
//! surface live in `longbox-web`'s `routes::opds` module and call into
//! here.

pub mod auth;
pub mod feed;
pub mod opensearch;

pub use auth::{
    dummy_verify, generate_api_token, hash_password, parse_basic, verify_password, OpdsAuthError,
    REALM,
};
pub use feed::{
    archive_mime, Entry, Feed, FeedKind, Link, Page, PageMeta, ACQUISITION_TYPE, ITEMS_PER_PAGE,
    NAVIGATION_TYPE,
};
