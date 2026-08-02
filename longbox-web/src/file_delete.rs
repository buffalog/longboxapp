//! The one way a file gets deleted from the library.
//!
//! Two features remove redundant copies — Library Tidy's keep-one
//! resolve, and Library Integrity's per-group delete — and they share
//! this operation rather than each implementing it. Two deletes
//! against `files` would be two chances to disagree about the guards,
//! the refusal conditions, or the order below, and disagreement
//! between two implementations of one rule is the defect shape that
//! produced most of this module's history (a second library walker, a
//! second parser, a second size source).
//!
//! # Order: the catalog row first, the bytes second
//!
//! Deleting a file is two writes to two systems with no transaction
//! spanning them, so one of them can succeed alone. That makes the
//! order a correctness decision, not a style one:
//!
//! | order | if the second write fails | result |
//! |---|---|---|
//! | **row, then bytes** | file remains on disk with no row | an **orphan** — the next scan sees an untracked file and re-adds it |
//! | bytes, then row | row remains, pointing at nothing | a **ghost** — unrecoverable |
//!
//! **The two callers reach the same order for different reasons, and
//! the reasons are not interchangeable.**
//!
//! *Library Integrity* — row-first because a surviving row keeps its
//! issue `owned` (ownership is derived by `EXISTS` over present
//! files), so the issue never reverts to missing and the pull engine
//! never re-fetches it. Bytes-first there can *silently defeat the
//! entire payoff while appearing to succeed*. That is a strong reason.
//!
//! *Library Tidy* — row-first because the alternative is worse, not
//! because row-first is good. Tidy gains nothing from a revert; what
//! it gains is that the failure is **visible**. Bytes-first leaves a
//! row claiming a file that is gone — silent until the next scan.
//! Row-first leaves bytes with no row, which the next scan re-adds,
//! recreating the very duplicate that was being removed. Annoying and
//! recoverable beats silent.
//!
//! Do not copy Integrity's justification onto Tidy: it does not hold
//! there, and someone reasoning from it will draw a wrong conclusion
//! about what a failed unlink costs. The previous version of this
//! comment argued the *opposite* order for Tidy on exactly that
//! ground, and was left dangling on an unrelated type when the
//! function moved.
//!
//! So: row first, and if the unlink then fails, say so loudly and
//! leave it. Reconciliation already detects that class and reports it
//! on a page the user has.
//!
//! This ordering was reversed in Library Tidy from PR #32 until this
//! change, and no test constrained it — the suite asserted only the
//! end state (row gone **and** file gone), which either order
//! satisfies.
//!
//! Both callers now pin it, and each pins its own route:
//! `row_is_gone_even_when_the_unlink_fails` (Tidy) and
//! `integrity_delete_removes_the_row_even_when_the_unlink_fails`
//! (Integrity). Two, not one, because for a period only Tidy's
//! existed — and reversing the order left every Integrity test
//! passing, so the caller with the *stronger* justification had no
//! test defending it. A doc that cites a constraint living in another
//! crate's test target is decorative: it cannot fail when the code it
//! claims to constrain changes.

use std::collections::HashMap;
use std::path::Path;

use longbox_db::{file_repo, FileRow, Pool};

use crate::pathsafe::is_contained;

/// What happened. `Ok` means the catalog row is gone — the caller can
/// rely on that. `unlink_error` being `Some` means the bytes survived
/// and the file is now an orphan awaiting the next scan.
#[derive(Debug, Default)]
pub struct Deleted {
    pub unlink_error: Option<String>,
}

/// Remove one file from the catalog and then from disk.
///
/// Every guard runs before either write. Returns `Err` only when
/// nothing was deleted; a failed unlink after a successful row removal
/// is `Ok` with `unlink_error` set, because the catalog is what the
/// rest of the system reads and it is already correct.
pub async fn delete_file(
    db: &Pool,
    roots: &HashMap<i64, String>,
    file: &FileRow,
) -> Result<Deleted, String> {
    // --- guards: all of them, before anything is written ---
    if !is_contained(&file.path_relative) {
        tracing::warn!(
            file_id = file.id,
            path = %file.path_relative,
            "file_delete: refused non-contained path"
        );
        return Err("stored path is not contained within its library root".to_owned());
    }
    // On the RAW STRING, before any `Path` conversion, because that is
    // the only place this information still exists.
    //
    // `is_contained` permits a `.` component — the read paths sharing it
    // legitimately see `./a/b.cbz`. For a delete that is not enough:
    // `"Batman/."` names a DIRECTORY, and unlinking it fails only AFTER
    // the catalog row is already gone, destroying a row for a path that
    // never named a file.
    //
    // No `std::path` API can catch that. `Path` parsing normalises `.`
    // away, so `Path::new("Batman/.")` and `Path::new("Batman")` are
    // indistinguishable to every accessor: `file_name()` returns
    // `Some("Batman")` for both, and `components().next_back()` returns
    // `Normal("Batman")` for both. A guard written against either one
    // compares equal and never fires — which is exactly what the first
    // version of this check did, and what the first proposed fix for it
    // would also have done. See the PR note: a normalised form of a
    // value is not the value.
    let last = file
        .path_relative
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("");
    if last == "." || last == ".." {
        tracing::warn!(
            file_id = file.id,
            path = %file.path_relative,
            "file_delete: refused path whose final component is not a file name"
        );
        return Err("stored path does not end in a file name".to_owned());
    }

    let root = roots
        .get(&file.library_root_id)
        .ok_or_else(|| format!("unknown library_root_id {}", file.library_root_id))?;
    let abs = Path::new(root).join(&file.path_relative);

    // `is_contained` is purely lexical (rejects `..` and absolute
    // paths). Delete is higher-stakes than the read paths sharing that
    // guard, so also resolve symlinks and confirm the real location is
    // still inside the real root — a symlinked parent directory must
    // not redirect the delete outside the library.
    //
    // Resolve the PARENT, not the file. Canonicalising the full path
    // follows a final-component symlink, and `remove_file` on the
    // result unlinks the link's TARGET while leaving the link — so
    // deleting one member of a duplicate group would destroy the other
    // one, the copy the user chose to keep, and leave its row behind
    // still marked owned. The scanner walks with `follow_links(true)`,
    // so a link and its target are both catalogued as ordinary rows
    // with identical bytes: exactly a duplicate group. Resolving only
    // the parent keeps the escape protection, which is what
    // canonicalisation was for, and drops the final-component
    // resolution, which was never the goal.
    let canon_root = tokio::fs::canonicalize(root)
        .await
        .map_err(|e| format!("cannot resolve library root path: {e}"))?;
    let (Some(parent), Some(name)) = (abs.parent(), abs.file_name()) else {
        return Err("stored path has no parent directory or file name".to_owned());
    };
    let target = match tokio::fs::canonicalize(parent).await {
        Ok(canon_parent) => {
            if !canon_parent.starts_with(&canon_root) {
                tracing::warn!(
                    file_id = file.id,
                    path = %file.path_relative,
                    resolved = %canon_parent.display(),
                    "file_delete: refused path resolving outside library root"
                );
                return Err("resolved path escapes its library root".to_owned());
            }
            Some(canon_parent.join(name))
        }
        // Already gone on disk. Not an error — the row still needs
        // removing, and that is the whole remaining job.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                file_id = file.id,
                path = %abs.display(),
                "file_delete: file already absent from disk"
            );
            None
        }
        Err(e) => return Err(format!("cannot resolve file path: {e}")),
    };

    // --- write 1: the catalog row. See the module docs for why first. ---
    match file_repo::delete(db, file.id).await {
        Ok(()) | Err(longbox_db::DbError::NotFound) => {}
        Err(e) => return Err(format!("failed to remove catalog row: {e}")),
    }

    // --- write 2: the bytes ---
    let mut out = Deleted::default();
    if let Some(target) = target {
        if let Err(e) = tokio::fs::remove_file(&target).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                // Loud, because the catalog is now correct and the disk
                // is not. Reconciliation will report this file as an
                // orphan until it is removed or re-adopted.
                tracing::error!(
                    file_id = file.id,
                    path = %target.display(),
                    error = %e,
                    "file_delete: catalog row removed but file could not be deleted — \
                     it is now an orphan and will be reported by reconciliation"
                );
                out.unlink_error = Some(format!(
                    "catalog row removed, but the file could not be deleted: {e}"
                ));
            }
        }
    }
    Ok(out)
}
