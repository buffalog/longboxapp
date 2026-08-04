use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A file that Phase B couldn't process automatically and is waiting on
/// the user. Lives in an in-memory cache shared with `longbox-web` (the
/// shared `Arc<RwLock<Vec<…>>>` lands in Step 7, when the conflict /
/// failure surface is wired into the dashboard).
///
/// Not persisted to a table for v1 — Phase B's "pending" set is
/// rebuilt from filesystem state at every startup; durability beyond
/// the process lifetime is on the Phase B+ cleanup queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingIntervention {
    /// Where the file currently sits (under the watch folder).
    pub source_path: PathBuf,
    /// Where Phase B *would* have moved it absent the failure.
    pub target_path: PathBuf,
    pub reason: InterventionReason,
    pub size: i64,
    #[serde(with = "time::serde::iso8601")]
    pub last_attempt: OffsetDateTime,
}

/// Why a file is sitting in `_pending` rather than the library proper.
/// The string payloads on the non-Conflict variants carry the underlying
/// error message verbatim — useful for the dashboard's reason column
/// when the user is deciding what to do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum InterventionReason {
    /// Target library path already exists; Phase B refuses to overwrite.
    Conflict,
    /// The CBZ was matched but Phase B couldn't write the ComicInfo
    /// payload into it (zip-write failure, disk full, permission error).
    ///
    /// A LOCAL problem. Says nothing about the release, so it must not
    /// fail the pull attempt — see [`SourceArchiveUnreadable`].
    ///
    /// [`SourceArchiveUnreadable`]: InterventionReason::SourceArchiveUnreadable
    ComicInfoWriteFailed(String),
    /// The downloaded archive itself could not be read — CRC error,
    /// truncated RAR, corrupt zip central directory.
    ///
    /// A verdict about the RELEASE, and one only Phase B can reach:
    /// comic NZBs routinely ship without a par2 recovery set, so the
    /// downloader has no checksums, reports Completed in good faith,
    /// and the corruption is invisible until the archive is opened.
    /// Kept distinct from [`ComicInfoWriteFailed`] because the two
    /// demand opposite responses — fail the attempt and move on, versus
    /// leave it alone and retry once the disk is fixed.
    ///
    /// [`ComicInfoWriteFailed`]: InterventionReason::ComicInfoWriteFailed
    SourceArchiveUnreadable(String),
    /// File move from watch folder to final library path failed
    /// (cross-device error, target permissions, race condition).
    MoveFailed(String),
}

/// In-memory cache of files awaiting manual intervention. Shared with
/// `longbox-web` for the dashboard counter + list view at
/// `/files/pending-intervention`. The brief locks "no persistent table"
/// for v1; the cache is rebuilt on restart because `initial_sweep`
/// re-enqueues every watch-folder file, so real conflicts re-emit and
/// transient failures auto-heal if they've resolved.
///
/// Locking discipline: `std::sync::RwLock` (not the tokio flavor) so the
/// `notify` callback — which runs on its own sync thread — can call
/// `remove_by_source_path` without runtime-handle gymnastics. All
/// operations are short and never hold the lock across an `.await`.
#[derive(Debug, Default)]
pub struct PendingInterventionsCache {
    inner: Arc<RwLock<Vec<PendingIntervention>>>,
}

impl PendingInterventionsCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Insert or replace the entry for `item.source_path`. notify
    /// double-fires and user-retry-after-resolve both arrive as repeat
    /// events for the same path; "latest attempt wins" is the natural
    /// shape — the list view shows current state per file, not a history.
    pub fn push(&self, item: PendingIntervention) {
        let mut guard = self.inner.write().expect("cache lock poisoned");
        if let Some(slot) = guard
            .iter_mut()
            .find(|existing| existing.source_path == item.source_path)
        {
            *slot = item;
        } else {
            guard.push(item);
        }
    }

    /// Remove any entry with the given source_path. No-op if absent.
    /// Called when a file successfully imports (self-healing) and when
    /// the watcher sees a Remove/Rename-From event (user manually
    /// resolved the conflict by deleting or moving the source).
    pub fn remove_by_source_path(&self, source_path: &Path) {
        let mut guard = self.inner.write().expect("cache lock poisoned");
        guard.retain(|item| item.source_path != source_path);
    }

    /// Snapshot of the cache for read-only consumers (HTTP handlers,
    /// tests). Returns an owned Vec so the lock is released immediately.
    pub fn snapshot(&self) -> Vec<PendingIntervention> {
        self.inner.read().expect("cache lock poisoned").clone()
    }

    pub fn len(&self) -> usize {
        self.inner.read().expect("cache lock poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(source: &str, target: &str) -> PendingIntervention {
        PendingIntervention {
            source_path: PathBuf::from(source),
            target_path: PathBuf::from(target),
            reason: InterventionReason::Conflict,
            size: 123,
            last_attempt: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn push_appends_new_entries() {
        let cache = PendingInterventionsCache::new();
        cache.push(item("/a.cbz", "/lib/a.cbz"));
        cache.push(item("/b.cbz", "/lib/b.cbz"));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn push_replaces_existing_entry_by_source_path() {
        let cache = PendingInterventionsCache::new();
        let mut original = item("/a.cbz", "/lib/a.cbz");
        original.size = 1;
        cache.push(original);

        let mut replacement = item("/a.cbz", "/lib/a.cbz");
        replacement.size = 999;
        cache.push(replacement);

        let snapshot = cache.snapshot();
        assert_eq!(snapshot.len(), 1, "second push should replace, not append");
        assert_eq!(snapshot[0].size, 999);
    }

    #[test]
    fn remove_by_source_path_drops_matching_entry() {
        let cache = PendingInterventionsCache::new();
        cache.push(item("/a.cbz", "/lib/a.cbz"));
        cache.push(item("/b.cbz", "/lib/b.cbz"));
        cache.remove_by_source_path(Path::new("/a.cbz"));
        let snapshot = cache.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].source_path, PathBuf::from("/b.cbz"));
    }

    #[test]
    fn remove_by_source_path_is_noop_for_missing_entry() {
        let cache = PendingInterventionsCache::new();
        cache.push(item("/a.cbz", "/lib/a.cbz"));
        cache.remove_by_source_path(Path::new("/nonexistent.cbz"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn snapshot_is_independent_of_subsequent_writes() {
        let cache = PendingInterventionsCache::new();
        cache.push(item("/a.cbz", "/lib/a.cbz"));
        let snapshot = cache.snapshot();
        cache.push(item("/b.cbz", "/lib/b.cbz"));
        assert_eq!(snapshot.len(), 1, "snapshot is a value, not a live view");
    }
}
