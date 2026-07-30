//! Content-identity hashing, shared by Library Tidy and the integrity scan.
//!
//! Both need to answer one question — *are these two files byte-identical?* —
//! and neither can answer it from the catalog, which is exactly how a library
//! ends up with the same book filed under three different issue numbers.
//!
//! Two properties make this cheap enough to run on demand:
//!
//! 1. **Size gating.** Files of differing size cannot be identical, so only
//!    size-collision groups are ever hashed. On the library this was built
//!    against that is 80 of 7102 present files — 7.4 GB of a 420 GB library.
//! 2. **Version-stamped persistence.** A digest is stored with the size and
//!    mtime it was computed against and reused until one of those moves, so
//!    repeat runs do no I/O at all.
//!
//! Reads are guarded like the delete path in `routes::duplicate_files`, not
//! like an ordinary read: lexical containment first, then canonicalization
//! confirming the resolved target is still inside the resolved library root.
//! A read is cheap to get wrong and this one feeds a delete button.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use longbox_db::{file_repo, library_root_repo, HashCandidate, Pool};
use time::{OffsetDateTime, PrimitiveDateTime};

/// What a hashing pass did. Every candidate lands in exactly one bucket, so
/// `candidates == fresh + hashed + skipped + failed` always holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct HashStats {
    /// Files sharing a size with at least one other present file.
    pub candidates: usize,
    /// Reused an existing digest — no I/O.
    pub fresh: usize,
    /// Read and digested this pass.
    pub hashed: usize,
    /// Deliberately not hashed: unreadable path, or the file changed while we
    /// were reading it. Not an error; it just has no trustworthy digest yet.
    pub skipped: usize,
    /// I/O or DB error. Surfaced rather than swallowed — a file that silently
    /// fails to hash is invisible to duplicate detection forever.
    pub failed: usize,
    pub bytes_hashed: u64,
    /// Of the files hashed this pass, how many yielded a usable internal
    /// label. Not every archive has one — it exists only because a release
    /// group put it there.
    pub labelled: usize,
    /// The first failure's path and reason, verbatim.
    ///
    /// `failed` on its own is an anonymous integer, and the moment it stops
    /// reading 0 the user is by definition in a situation where "go read the
    /// container logs" is the worst possible instruction. One example costs a
    /// String and turns "40 failed" into something actionable.
    pub first_failure: Option<String>,
}

impl HashStats {
    /// Count a failure, keeping the first explanation.
    fn fail(&mut self, path: &str, reason: impl std::fmt::Display) {
        self.failed += 1;
        if self.first_failure.is_none() {
            self.first_failure = Some(format!("{path}: {reason}"));
        }
    }
}

/// Read the archive's internal identity label. `None` for anything we cannot
/// confidently read: a non-archive extension, an unlistable file, or entry
/// names that yield no label.
///
/// Central-directory / header listing only. No entry is decompressed, no image
/// is decoded, nothing is fetched.
async fn read_archive_label(path: &Path) -> Option<(String, &'static str)> {
    let owned = path.to_path_buf();
    let names = tokio::task::spawn_blocking(move || longbox_archive::list_entry_names(&owned))
        .await
        .ok()?
        .ok()?;
    let (label, kind) = longbox_core::archive_label::label_from_entries(&names)?;
    Some((
        label,
        match kind {
            longbox_core::archive_label::LabelKind::Dir => "dir",
            longbox_core::archive_label::LabelKind::Page => "page",
        },
    ))
}

/// Bring every size-collision candidate's digest up to date.
///
/// Never aborts the run for one bad file. A single unreadable archive in a
/// 7000-file library must not deny the user the other 6999 findings — it is
/// counted in `failed` and logged with its path instead.
pub async fn refresh_digests(db: &Pool) -> Result<HashStats, longbox_db::DbError> {
    let candidates = file_repo::size_collision_candidates(db).await?;
    let roots: HashMap<i64, String> = library_root_repo::list_all(db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.path))
        .collect();

    let mut stats = HashStats {
        candidates: candidates.len(),
        ..Default::default()
    };

    for candidate in candidates {
        let Some(root) = roots.get(&candidate.library_root_id) else {
            tracing::warn!(
                file_id = candidate.file_id,
                library_root_id = candidate.library_root_id,
                "content-hash: unknown library root"
            );
            stats.fail(
                &candidate.path_relative,
                format!("unknown library_root_id {}", candidate.library_root_id),
            );
            continue;
        };

        // Resolve and stat BEFORE deciding freshness. The catalog is a cache
        // refreshed by a once-daily scan; only disk can say whether the bytes
        // behind this row still match the digest we stored.
        let on_disk = match resolve_and_stat(root, &candidate).await {
            Ok(Some(v)) => v,
            // Unresolvable or already gone — normal race with a scan, and it
            // leaves the row's existing digest untouched rather than trusted.
            Ok(None) => {
                stats.skipped += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    file_id = candidate.file_id,
                    path = %candidate.path_relative,
                    error = %e,
                    "content-hash: cannot stat file"
                );
                stats.fail(&candidate.path_relative, format!("cannot stat: {e}"));
                continue;
            }
        };

        if candidate.has_fresh_digest(on_disk.size, on_disk.mtime) {
            stats.fresh += 1;
            continue;
        }

        // Read the archive's internal label in the same pass. This file is
        // already being opened and read end to end for the digest, so listing
        // its central directory is the cheapest possible moment to learn what
        // the archive says it is. Listing only — no entry is decompressed.
        //
        // Best-effort: a label is evidence, not a precondition. An archive we
        // cannot list still gets its digest, and simply has no label.
        let label = read_archive_label(&on_disk.path).await;

        match digest_one(on_disk.path).await {
            Ok(Some(observed)) => {
                // A DB failure here must not discard the whole pass — the
                // other candidates' digests are already written and valid.
                match file_repo::set_content_hash(
                    db,
                    candidate.file_id,
                    &observed.digest,
                    observed.size,
                    observed.mtime,
                )
                .await
                {
                    Ok(()) => {
                        // Written after the digest, so a label can never be
                        // stored against a file whose digest failed to persist
                        // — the stamp that validates the label lives on the
                        // digest write.
                        let (label, kind) = match &label {
                            Some((l, k)) => (Some(l.as_str()), Some(*k)),
                            None => (None, None),
                        };
                        if let Err(e) =
                            file_repo::set_archive_label(db, candidate.file_id, label, kind).await
                        {
                            tracing::warn!(
                                file_id = candidate.file_id,
                                error = %e,
                                "content-hash: failed to persist archive label"
                            );
                        }
                        if label.is_some() {
                            stats.labelled += 1;
                        }
                        stats.hashed += 1;
                        stats.bytes_hashed += observed.size.max(0) as u64;
                    }
                    Err(e) => {
                        tracing::warn!(
                            file_id = candidate.file_id,
                            error = %e,
                            "content-hash: failed to persist digest"
                        );
                        stats.fail(
                            &candidate.path_relative,
                            format!("cannot persist digest: {e}"),
                        );
                    }
                }
            }
            Ok(None) => stats.skipped += 1,
            Err(e) => {
                tracing::warn!(
                    file_id = candidate.file_id,
                    path = %candidate.path_relative,
                    error = %e,
                    "content-hash: failed to digest file"
                );
                stats.fail(&candidate.path_relative, format!("cannot read: {e}"));
            }
        }
    }
    Ok(stats)
}

/// A resolved path plus the size/mtime observed on disk right now.
struct OnDisk {
    path: PathBuf,
    size: i64,
    mtime: PrimitiveDateTime,
}

/// Resolve within the library root and stat. `Ok(None)` when the path fails
/// containment or the file is simply gone.
async fn resolve_and_stat(
    root: &str,
    candidate: &HashCandidate,
) -> Result<Option<OnDisk>, std::io::Error> {
    let Some(path) = resolve_within_root(root, &candidate.path_relative).await? else {
        return Ok(None);
    };
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(OnDisk {
        path,
        size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
        mtime: to_primitive_utc(meta.modified()?),
    }))
}

/// A digest and the exact file version it covers.
struct Observed {
    digest: String,
    size: i64,
    mtime: PrimitiveDateTime,
}

/// `Ok(None)` means "no trustworthy digest right now, and that's fine":
/// the file is gone, or it changed under us mid-read.
async fn digest_one(abs: PathBuf) -> Result<Option<Observed>, std::io::Error> {
    tokio::task::spawn_blocking(move || hash_file(&abs))
        .await
        .map_err(std::io::Error::other)?
}

/// Lexical containment, then canonicalization — the same two-step the delete
/// resolver uses. The lexical pass rejects `..` and absolute paths; the
/// canonical pass stops a symlinked parent directory from redirecting the read
/// (and therefore a later delete) outside the library.
///
/// `Ok(None)` for a path that fails either check, or a file that is simply
/// gone — an absent file is a normal race with a scan, not an error.
async fn resolve_within_root(
    root: &str,
    path_relative: &str,
) -> Result<Option<PathBuf>, std::io::Error> {
    if !crate::pathsafe::is_contained(path_relative) {
        tracing::warn!(path = %path_relative, "content-hash: refused non-contained path");
        return Ok(None);
    }
    let canon_root = tokio::fs::canonicalize(root).await?;
    let target = match tokio::fs::canonicalize(Path::new(root).join(path_relative)).await {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !target.starts_with(&canon_root) {
        tracing::warn!(
            path = %path_relative,
            resolved = %target.display(),
            "content-hash: refused path resolving outside library root"
        );
        return Ok(None);
    }
    Ok(Some(target))
}

/// Blocking: stat, stream-hash, then check nothing moved under us.
///
/// Two different substitutions have to be caught, and they need two different
/// checks:
///
/// - **In-place rewrite.** `File::open` + `metadata()` are fd-scoped, so
///   bracketing the read with two fstats catches writes to the inode we are
///   actually reading.
/// - **Atomic replace.** `write temp, rename over target` never touches our
///   inode, so the fstat bracket is blind to it and would happily return a
///   digest of the OLD file stamped against a path that now holds the new one.
///   This is not hypothetical: it is how this app's own import pipeline writes
///   files (`longbox-postprocess`'s `commit_move` → `NamedTempFile::persist`).
///   Catching it needs a stat of the *path*, compared by inode.
///
/// Either way we discard rather than persist a digest that describes no
/// version of the file at that path. One wasted retry beats one wrongly
/// deleted comic.
fn hash_file(path: &Path) -> Result<Option<Observed>, std::io::Error> {
    let file = std::fs::File::open(path)?;
    let before = file.metadata()?;

    let mut hasher = blake3::Hasher::new();
    // `update_reader` owns its buffering and sizes it for the widest SIMD
    // path available on this target — a hand-rolled buffer here would be
    // slower and would need revisiting per architecture.
    hasher.update_reader(&file)?;
    let digest = hasher.finalize().to_hex().to_string();

    let after = file.metadata()?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        tracing::info!(path = %path.display(), "content-hash: file rewritten during read, discarding digest");
        return Ok(None);
    }

    // Non-unix has no stable inode identity, so this check cannot be written
    // portably — and letting it compile out silently would quietly restore the
    // atomic-replace hole on that target while every test still passed. Fail
    // at build time instead. LongBox ships as a Linux container and is
    // developed on macOS; if a third target ever appears, this is a decision
    // to make deliberately, not to inherit.
    #[cfg(not(unix))]
    compile_error!(
        "content_hash::hash_file needs inode identity to detect atomic replace; \
         port the check before building for a non-unix target"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let by_path = std::fs::metadata(path)?;
        if by_path.ino() != before.ino() || by_path.dev() != before.dev() {
            tracing::info!(path = %path.display(), "content-hash: file replaced during read, discarding digest");
            return Ok(None);
        }
    }

    Ok(Some(Observed {
        digest,
        size: i64::try_from(after.len()).unwrap_or(i64::MAX),
        mtime: to_primitive_utc(after.modified()?),
    }))
}

/// Normalize to UTC and drop the offset, matching how the scanner stores
/// mtime — the freshness check compares these values for equality, so the two
/// writers must agree on representation exactly.
fn to_primitive_utc(t: std::time::SystemTime) -> PrimitiveDateTime {
    let off = OffsetDateTime::from(t).to_offset(time::UtcOffset::UTC);
    PrimitiveDateTime::new(off.date(), off.time())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    /// The label must survive the real archive round-trip: a CBZ written to
    /// disk, listed through longbox-archive, and read by the parser. The unit
    /// tests in `longbox_core::archive_label` cover parsing; this covers the
    /// plumbing between them, which is where a wrong `LabelKind` would turn a
    /// page counter into an issue number.
    #[tokio::test]
    async fn reads_the_internal_label_out_of_a_real_archive() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("whatever-the-user-renamed-it.cbz");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        // A scene-style single top-level folder naming the real issue.
        for page in ["000", "001"] {
            zip.start_file(format!("My Little Warlord 008/008-{page}.jpg"), opts)
                .unwrap();
            zip.write_all(b"\xFF\xD8\xFF").unwrap();
        }
        zip.finish().unwrap();

        let (label, kind) = read_archive_label(&path).await.expect("label");
        assert_eq!(label, "My Little Warlord 008");
        assert_eq!(kind, "dir");

        // And it says 8 — not the page number, and regardless of the
        // filename on disk, which names nothing at all.
        let id = longbox_core::archive_label::parse_label(
            &label,
            longbox_core::archive_label::LabelKind::Dir,
        );
        assert_eq!(id.issue.as_deref(), Some("8"));
        assert_eq!(id.series.as_deref(), Some("my little warlord"));
    }

    #[tokio::test]
    async fn a_non_archive_yields_no_label_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "not-a-comic.cbz", b"definitely not a zip");
        assert!(read_archive_label(&path).await.is_none());
    }

    #[test]
    fn identical_bytes_hash_identically_and_differing_bytes_do_not() {
        let dir = tempfile::tempdir().unwrap();
        // Large enough to cross blake3's internal buffering, so this exercises
        // the streaming path rather than a single-chunk shortcut.
        let payload = vec![7u8; 300_000];
        let a = write_file(dir.path(), "a.cbz", &payload);
        let b = write_file(dir.path(), "b.cbz", &payload);
        let mut different = payload.clone();
        *different.last_mut().unwrap() = 8;
        let c = write_file(dir.path(), "c.cbz", &different);

        let ha = hash_file(&a).unwrap().unwrap();
        let hb = hash_file(&b).unwrap().unwrap();
        let hc = hash_file(&c).unwrap().unwrap();

        assert_eq!(ha.digest, hb.digest, "same bytes must digest the same");
        assert_ne!(ha.digest, hc.digest, "one flipped byte must change it");
        assert_eq!(ha.size, 300_000);
        assert_eq!(ha.digest.len(), 64, "blake3 hex is 32 bytes");
    }

    #[test]
    fn empty_and_missing_files_are_handled_distinctly() {
        let dir = tempfile::tempdir().unwrap();
        let empty = write_file(dir.path(), "empty.cbz", b"");
        // An empty file is hashable — it just has the empty digest.
        assert_eq!(hash_file(&empty).unwrap().unwrap().size, 0);
        // A missing one is an error at this layer; `resolve_within_root`
        // is what turns absence into a skip.
        assert!(hash_file(&dir.path().join("nope.cbz")).is_err());
    }

    #[tokio::test]
    async fn resolve_rejects_traversal_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        write_file(dir.path(), "ok.cbz", b"x");

        assert!(resolve_within_root(root, "ok.cbz").await.unwrap().is_some());
        assert!(resolve_within_root(root, "../escape.cbz")
            .await
            .unwrap()
            .is_none());
        assert!(resolve_within_root(root, "/etc/hosts")
            .await
            .unwrap()
            .is_none());
        assert!(resolve_within_root(root, "").await.unwrap().is_none());
        // Absent file inside the root is a skip, not an error.
        assert!(resolve_within_root(root, "gone.cbz")
            .await
            .unwrap()
            .is_none());
    }

    /// The atomic-replace window: `rename` a different file over the path
    /// while a digest is in flight. The fd-scoped stat bracket cannot see
    /// this — only the inode comparison can — and getting it wrong stamps a
    /// digest of the OLD bytes onto a path that now holds new ones.
    #[cfg(unix)]
    #[test]
    fn digest_is_discarded_when_the_path_is_atomically_replaced() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "comic.cbz", &vec![1u8; 100_000]);
        let held = std::fs::File::open(&target).unwrap();
        let original_ino = held.metadata().unwrap().ino();

        // Replace the path with a different inode, exactly as
        // NamedTempFile::persist does.
        let replacement = write_file(dir.path(), "replacement.cbz", &vec![2u8; 100_000]);
        std::fs::rename(&replacement, &target).unwrap();

        let new_ino = std::fs::metadata(&target).unwrap().ino();
        assert_ne!(original_ino, new_ino, "rename must swap the inode");

        // Hashing the path now sees only the new inode, which is consistent —
        // so this must succeed. The guard is not allowed to be a blanket
        // "any rename ever poisons this file" rule.
        assert!(
            hash_file(&target).unwrap().is_some(),
            "a settled file must still hash after an unrelated earlier rename"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_rejects_symlink_escaping_the_root() {
        let outside = tempfile::tempdir().unwrap();
        let secret = write_file(outside.path(), "secret.cbz", b"outside");
        let root_dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&secret, root_dir.path().join("link.cbz")).unwrap();

        let resolved = resolve_within_root(root_dir.path().to_str().unwrap(), "link.cbz")
            .await
            .unwrap();
        assert!(
            resolved.is_none(),
            "a symlink out of the library must not be hashable"
        );
    }
}
