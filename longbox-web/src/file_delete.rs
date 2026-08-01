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
//! The ghost is worse than untidy. A surviving row keeps its issue
//! `owned` (ownership is derived by `EXISTS` over present files), so
//! the issue never reverts to missing and the pull engine never
//! re-fetches it — which is precisely the payoff the delete exists to
//! deliver. The bytes-first order can therefore *silently defeat the
//! feature while appearing to succeed.*
//!
//! So: row first, and if the unlink then fails, say so loudly and
//! leave it. Reconciliation already detects that class and reports it
//! on a page the user has.
//!
//! This ordering was reversed in Library Tidy from PR #32 until this
//! change, and no test constrained it — the suite asserted only the
//! end state (row gone **and** file gone), which either order
//! satisfies. [`tests::row_is_gone_even_when_the_unlink_fails`] is the
//! first test that can tell them apart.

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
    let root = roots
        .get(&file.library_root_id)
        .ok_or_else(|| format!("unknown library_root_id {}", file.library_root_id))?;
    let abs = Path::new(root).join(&file.path_relative);

    // `is_contained` is purely lexical (rejects `..` and absolute
    // paths). Delete is higher-stakes than the read paths sharing that
    // guard, so also resolve symlinks and confirm the real target is
    // still inside the real root — a symlinked parent must not
    // redirect the delete outside the library.
    let canon_root = tokio::fs::canonicalize(root)
        .await
        .map_err(|e| format!("cannot resolve library root path: {e}"))?;
    let target = match tokio::fs::canonicalize(&abs).await {
        Ok(canon_target) => {
            if !canon_target.starts_with(&canon_root) {
                tracing::warn!(
                    file_id = file.id,
                    path = %file.path_relative,
                    resolved = %canon_target.display(),
                    "file_delete: refused path resolving outside library root"
                );
                return Err("resolved path escapes its library root".to_owned());
            }
            Some(canon_target)
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
