//! Library Integrity — finding computation.
//!
//! Every finding is derived live from the catalog and the filesystem. There is
//! no findings table: a persisted finding is stale the moment a file moves,
//! and computing on read also makes the write surface trivially auditable —
//! this module holds no `INSERT`, `UPDATE` or `DELETE` at all.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

/// What a disk walk actually did.
///
/// Reported alongside the findings because class (b) reads 0/0 on a healthy
/// library, and its entire value is as a regression guard. A section showing
/// zero because the walk failed, or never ran, is the same lie as telling a
/// user to run an analysis that cannot reach their files. With this, zero is
/// a measurement.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalkProvenance {
    /// Absolute library root that was walked.
    pub root: String,
    /// Comic files seen on disk, by the scanner's own rules.
    pub files_seen: usize,
    /// Catalog rows compared against them (`is_present = 1`).
    pub rows_compared: usize,
    pub duration_ms: u64,
    /// Paths the walk could not read. Non-empty means the counts below are a
    /// floor, not a total — say so rather than reporting a clean zero.
    pub unreadable: Vec<String>,
}

/// Disk and catalog disagreeing about what exists.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Reconciliation {
    pub provenance: WalkProvenance,
    /// On disk, with no catalog row at all.
    pub orphans: Vec<String>,
    /// Catalog says `is_present = 1`, nothing on disk.
    pub ghosts: Vec<String>,
    /// Catalog says `is_present = 0`, but the file is right there. Not one of
    /// the two classes asked for, and reported separately rather than folded
    /// in: a scan fixes these on its own, so they are drift, not corruption.
    pub present_but_marked_absent: Vec<String>,
}

impl Reconciliation {
    /// True when the walk produced a usable comparison. A caller must not
    /// render "0 problems" when this is false.
    pub fn is_conclusive(&self) -> bool {
        self.provenance.unreadable.is_empty() && self.provenance.files_seen > 0
    }
}

/// Walk the library root and compare it against the catalog.
///
/// Uses `longbox_scanner::walk_library`, deliberately, rather than walking
/// here: reconciliation is only meaningful against the same definition of "a
/// comic file" that the scanner catalogues by. A second walker would drift and
/// every file it disagreed about would become a permanent false finding.
pub async fn reconcile(
    db: &longbox_db::Pool,
    library_root_id: i64,
    root: &str,
) -> Result<Reconciliation, longbox_db::DbError> {
    let started = std::time::Instant::now();

    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    let mut unreadable: Vec<String> = Vec::new();
    for entry in longbox_scanner::walk_library(std::path::Path::new(root)) {
        match entry {
            Ok(f) => {
                on_disk.insert(f.path_relative);
            }
            Err(e) => unreadable.push(e.to_string()),
        }
    }

    // Every catalog row for this root, with its presence flag — one query,
    // because both directions of the comparison need the same set.
    let rows: Vec<(String, bool)> =
        sqlx::query_as("SELECT path_relative, is_present FROM files WHERE library_root_id = ?")
            .bind(library_root_id)
            .fetch_all(db)
            .await?;

    let mut known: BTreeMap<&str, bool> = BTreeMap::new();
    for (path, present) in &rows {
        // A path can only appear once per root (UNIQUE constraint), so no
        // merge logic is needed here.
        known.insert(path.as_str(), *present);
    }
    let rows_compared = known.values().filter(|p| **p).count();

    let orphans: Vec<String> = on_disk
        .iter()
        .filter(|p| !known.contains_key(p.as_str()))
        .cloned()
        .collect();
    let ghosts: Vec<String> = known
        .iter()
        .filter(|(p, present)| **present && !on_disk.contains(**p))
        .map(|(p, _)| (*p).to_owned())
        .collect();
    let present_but_marked_absent: Vec<String> = known
        .iter()
        .filter(|(p, present)| !**present && on_disk.contains(**p))
        .map(|(p, _)| (*p).to_owned())
        .collect();

    Ok(Reconciliation {
        provenance: WalkProvenance {
            root: root.to_owned(),
            files_seen: on_disk.len(),
            rows_compared,
            duration_ms: started.elapsed().as_millis() as u64,
            unreadable,
        },
        orphans,
        ghosts,
        present_but_marked_absent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_findings_are_only_conclusive_when_the_walk_actually_walked() {
        let clean = Reconciliation {
            provenance: WalkProvenance {
                root: "/comics".into(),
                files_seen: 7115,
                rows_compared: 7115,
                duration_ms: 2000,
                unreadable: vec![],
            },
            orphans: vec![],
            ghosts: vec![],
            present_but_marked_absent: vec![],
        };
        assert!(clean.is_conclusive(), "a real walk finding nothing is news");

        // Walk saw nothing: 0/0 here means "did not look", not "all clear".
        let empty = Reconciliation {
            provenance: WalkProvenance {
                files_seen: 0,
                ..clean.provenance.clone()
            },
            ..clean.clone()
        };
        assert!(
            !empty.is_conclusive(),
            "a walk that saw no files cannot report a clean library"
        );

        // Partially readable: the counts are a floor, not a total.
        let partial = Reconciliation {
            provenance: WalkProvenance {
                unreadable: vec!["permission denied: /comics/x".into()],
                ..clean.provenance.clone()
            },
            ..clean
        };
        assert!(!partial.is_conclusive());
    }
}
