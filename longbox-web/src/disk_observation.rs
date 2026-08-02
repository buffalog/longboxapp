//! Evidence about a file's contents, obtained by looking at the file.
//!
//! Lives here rather than inside a route module because **two** features now
//! make delete decisions — Library Tidy and Library Integrity — and a delete
//! decision must rest on a stat, never on a catalog column. Leaving this type
//! where only one of them could reach it is how the second one came to be
//! written without it.
//!
//! # `validate_digest` is FROZEN
//!
//! Do not change what it returns without a demonstrated failure that
//! requires it. Three revisions of the non-file case shipped in one
//! change set, and the middle one turned a refusal into a delete in
//! **two** features at once by returning the same tuple that means
//! "nothing is there".
//!
//! The general rule, which is the fourth time this shape has appeared
//! in this codebase: **a shared function's callers may read its
//! "nothing there" return as permission rather than as an error.**
//! Changing what a shared function returns in an edge case is a change
//! to every caller's control flow, and it cannot be assessed from the
//! function alone. Trace the callers, or do not make the change.
//!
//! Known and deliberately not fixed: every `metadata()` failure
//! collapses to "absent", so an unreadable file (`EACCES` on a parent,
//! `EIO` on a flaky mount) reads as missing rather than as unknown.
//! That is the **permissive** direction — the delete proceeds where it
//! should have refused — which is why it is worth revisiting later
//! rather than never. It stays recorded rather than patched because
//! every change to this function so far has cost more than it bought.
//!
//! It also carries **identity** — `(dev, ino)` — because "is this a copy?"
//! and "is this the same file?" are different questions and only the second
//! one can be answered from a path. `metadata()` follows symlinks, so an
//! alias of a file stats identically to it: same size, same mtime, same
//! validated digest. A guard counting "surviving copies" that does not
//! compare identity will count the file it is about to delete, reached by a
//! second name, as the copy that justifies deleting it.

use std::collections::HashMap;
use std::path::Path;

use crate::pathsafe::is_contained;

/// Stat a file and decide whether its stored digest can be trusted.
///
/// Returns `(exists, observed_size, trustworthy_digest, identity)`, where
/// identity is the resolved file's `(dev, ino)` — see the module docs for why
/// a content check needs it. The digest comes
/// back `Some` only when the stamp it was computed against still matches what
/// the file looks like now — same size, same mtime. A file modified since the
/// last analyze is therefore NOT reported as carrying its old digest, which is
/// what stops a stale digest making two different files look byte-identical.
pub(crate) async fn validate_digest(
    roots: &HashMap<i64, String>,
    library_root_id: i64,
    path_relative: &str,
    digest: Option<&str>,
    stamp_size: Option<i64>,
    stamp_mtime: Option<time::PrimitiveDateTime>,
) -> (bool, Option<i64>, Option<String>, Option<(u64, u64)>) {
    #[cfg(not(unix))]
    compile_error!(
        "DiskObservation identifies files by (dev, ino) to tell an alias from a copy; \
         a non-unix port needs an equivalent before any delete guard can be trusted"
    );
    use std::os::unix::fs::MetadataExt;
    if !is_contained(path_relative) {
        return (false, None, None, None);
    }
    let Some(root) = roots.get(&library_root_id) else {
        return (false, None, None, None);
    };
    let Ok(meta) = tokio::fs::metadata(Path::new(root).join(path_relative)).await else {
        return (false, None, None, None);
    };
    let observed_size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    // A non-file — a directory, a fifo, a link to one — is PRESENT but
    // carries no bytes we can reason about. It reports `exists = true`
    // with no digest and no identity, and the distinction is the whole
    // point.
    //
    // Returning the absent tuple here instead is a mistake that has
    // been made once already, and it made both callers WORSE than no
    // check at all: every guard downstream reads "not there" as a green
    // light. Integrity's `target_exists` short-circuits its own
    // freshness check, so a target that is a directory skipped straight
    // to the delete, removed the row and reverted the issue. Tidy drops
    // absent files before `classify_content`, so a non-file member took
    // the group under the two-file threshold and the entire
    // Identical/Distinct/Unknown verdict was skipped — a resolve that
    // had correctly refused began deleting.
    //
    // Present-but-unreadable is the honest answer and the conservative
    // one. No digest means it can never be the surviving copy that
    // justifies deleting a real file, and it can never be `Identical`
    // to anything; `classify_content` answers `Unknown` on a missing
    // digest and `Distinct` on a size mismatch, so both refuse. No
    // identity means it is never mistaken for an alias.
    if !meta.is_file() {
        return (true, Some(observed_size), None, None);
    }
    let ident = Some((meta.dev(), meta.ino()));
    let Some(digest) = digest.filter(|d| !d.is_empty()) else {
        return (true, Some(observed_size), None, ident);
    };
    let (Some(stamp_size), Some(stamp_mtime)) = (stamp_size, stamp_mtime) else {
        return (true, Some(observed_size), None, ident);
    };
    let Ok(modified) = meta.modified() else {
        return (true, Some(observed_size), None, ident);
    };
    let off = time::OffsetDateTime::from(modified).to_offset(time::UtcOffset::UTC);
    let observed_mtime = time::PrimitiveDateTime::new(off.date(), off.time());
    let fresh = stamp_size == observed_size && stamp_mtime == observed_mtime;
    (
        true,
        Some(observed_size),
        fresh.then(|| digest.to_owned()),
        ident,
    )
}

/// Evidence about a file's CONTENT, obtained by looking at the file.
///
/// A newtype with exactly one production constructor — [`DiskObservation::stat`],
/// which performs the stat itself — so a catalog value cannot be substituted
/// for a disk value here even by accident. That mistake has been made four
/// separate times on this codebase (digest freshness, post-process import
/// size, the detector's grouping key, and this classifier), always because
/// `files.size_bytes` is the nearest thing to hand: already loaded, no
/// syscall, usually correct. Discipline lost to that gradient every time, so
/// the value is made unavailable rather than discouraged.
///
/// Catalog `size_bytes` remains perfectly good for display, for sorting, and
/// for the corruption floor. It is only illegitimate as proof about bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskObservation {
    /// Size the file has right now. `None` when it is not on disk at all.
    size: Option<i64>,
    /// `(device, inode)` of the file the path resolved to. `None` when
    /// nothing was there. Two observations sharing this are two names
    /// for one file, not two copies of one content.
    identity: Option<(u64, u64)>,
    /// A digest validated against that same stat. `None` when there is no
    /// trustworthy one.
    digest: Option<String>,
}

impl DiskObservation {
    /// The only way production code obtains one: by stat-ing the file.
    pub(crate) async fn stat(
        roots: &HashMap<i64, String>,
        library_root_id: i64,
        path_relative: &str,
        digest: Option<&str>,
        stamp_size: Option<i64>,
        stamp_mtime: Option<time::PrimitiveDateTime>,
    ) -> (bool, Self) {
        let (exists, size, digest, identity) = validate_digest(
            roots,
            library_root_id,
            path_relative,
            digest,
            stamp_size,
            stamp_mtime,
        )
        .await;
        (
            exists,
            Self {
                size,
                digest,
                identity,
            },
        )
    }

    /// Size observed by the stat. Read-only on purpose — see the type
    /// doc: widening these fields to `pub(crate)` so a caller in
    /// another module could read them would make a struct literal
    /// legal crate-wide, which is exactly the substitution the newtype
    /// exists to prevent. That happened once, when this type was moved
    /// out of `routes/duplicate_files.rs`.
    pub(crate) fn size(&self) -> Option<i64> {
        self.size
    }

    /// The digest, present only when it was validated against the same
    /// stat that produced [`Self::size`].
    pub(crate) fn digest(&self) -> Option<&str> {
        self.digest.as_deref()
    }

    /// True when both observations resolved to the *same file* — one
    /// file reached by two names, not two copies of one content.
    ///
    /// The question every survival guard actually needs to ask before
    /// treating one file as evidence that another is redundant.
    /// Conservative: two observations that could not be identified
    /// (nothing on disk) are never called the same file.
    ///
    /// Note what this deliberately does NOT distinguish: a hardlinked
    /// pair also shares `(dev, ino)`, and unlinking one of those really
    /// does leave the other. Telling the two apart needs `st_nlink`,
    /// and both callers refuse rather than carry that distinction —
    /// see the delete guard in `routes::integrity` and the PR's
    /// limitations note. Refusing a hardlinked pair is a false refusal,
    /// not a false delete.
    pub(crate) fn is_same_file(&self, other: &Self) -> bool {
        match (self.identity, other.identity) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// Test-only constructor. Deliberately absent from production builds:
    /// the point of the newtype is that the only other way to get one is to
    /// look at a real file.
    #[cfg(test)]
    pub(crate) fn from_parts(size: Option<i64>, digest: Option<String>) -> Self {
        Self {
            size,
            digest,
            // No identity: a fabricated observation is never the same
            // file as anything, which keeps `is_same_file` conservative
            // in unit tests rather than silently equal.
            identity: None,
        }
    }
}
