//! Shared path-containment guard.
//!
//! The scanner writes root-relative paths, but a malformed or tampered
//! `path_relative` must never let `Path::join` escape the library root. Both
//! the OPDS download endpoint and the built-in reader validate every stored
//! relative path through [`is_contained`] before touching the filesystem, so
//! the check lives in exactly one place.

use std::path::{Component, Path};

/// True only when `path_relative` stays within the library root: a non-empty
/// relative path with no root/prefix component and no `..` escape. Only
/// `Normal` and `CurDir` components are allowed.
pub(crate) fn is_contained(path_relative: &str) -> bool {
    !path_relative.is_empty()
        && Path::new(path_relative)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_allows_plain_relative_paths() {
        assert!(is_contained("Batman/Batman 001.cbz"));
        assert!(is_contained("file.cbz"));
        assert!(is_contained("./a/b.cbz"));
    }

    #[test]
    fn containment_rejects_escapes() {
        assert!(!is_contained("")); // empty
        assert!(!is_contained("/etc/hosts")); // absolute
        assert!(!is_contained("../../../etc/hosts")); // parent traversal
        assert!(!is_contained("a/../../b.cbz")); // mid-path traversal
    }
}
