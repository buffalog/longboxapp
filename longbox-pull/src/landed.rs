//! What actually landed for a completed download.
//!
//! This is the missing half of the pull engine's feedback loop. The
//! success path has always been able to report itself: Phase B imports
//! a comic, matches it to an issue, and settles the attempt to
//! `grabbed` via `mark_grabbed_for_issue(series_id, issue_id)`. But
//! that handoff is keyed on a *matched issue*, so a download that
//! contained no comic at all has no identity to report with — nothing
//! matches, nothing settles, and the attempt sits `submitted` until
//! the downloader forgets the job and the poller calls it
//! "lost track of download". The download was never lost. It arrived
//! perfectly and contained a PDF.
//!
//! So the verdict here is derived from the bytes on disk rather than
//! from a catalog match, and it is deliberately biased toward saying
//! "I don't know".
//!
//! # The floor is the existing behaviour
//!
//! Abstaining is free here, and that is a structural property rather
//! than a matter of care.
//!
//! An [`Inconclusive`](LandedVerdict::Inconclusive) verdict leaves the
//! attempt in `submitted`, which is exactly where it would have been
//! if this module did not exist. `poll_in_flight` keeps working the
//! row, and if the downloader eventually drops the job it still
//! settles by the old route — `Unknown` × `UNKNOWN_POLL_LIMIT`, then
//! "lost track of download". So every abstention path degrades to
//! pre-existing behaviour. **This check can improve on the status quo
//! or leave it untouched; it cannot invent a way for an issue to
//! hang.**
//!
//! That is what makes the bias affordable. Abstaining costs one more
//! day of the old, dishonest message; a wrong
//! [`OnlyKnownNonPayload`](LandedVerdict::OnlyKnownNonPayload) writes a terminal
//! state that suppresses the release for that issue forever, silently
//! starving it in a way that looks exactly like the feature working.
//! The two errors are not symmetric, so the tie goes to abstention.
//!
//! [`classify`] also re-runs on every sweep, so a folder that was
//! unreadable or mid-write is re-judged the next day rather than
//! condemned once.
//!
//! # What the two folder checks prove, and what they do not
//!
//! Before [`classify`] will condemn a folder it runs two guards, and
//! **both prove a negative**:
//!
//! - the **completion window** proves *this is not a later job's
//!   folder*. A downloader names a job folder from a namespace it
//!   re-allocates once the previous folder is gone — SAB's `.1` suffix
//!   is a first-free-slot allocator, not an identity — so a recorded
//!   path outlives the directory it named, and a stale attempt can
//!   otherwise be judged against a stranger's download.
//! - the **removal check** proves *nothing has been taken out of any
//!   directory in this tree since the newest surviving file in it was
//!   written*. Not "since it was unpacked" — the baseline is the
//!   newest file still present, so a litter file written after the
//!   payload eats margin. [`REMOVAL_TOLERANCE_SECS`]'s Residual
//!   section states the same thing; they must not drift apart.
//!
//!   Phase B imports a comic by unlinking it from the watch folder,
//!   and only removes the folder itself when that leaves it empty — so
//!   a release carrying `.nfo`/`.sfv` litter survives import looking
//!   exactly like a release that never held a comic.
//!
//!   The comparison is **per directory** — each directory's mtime
//!   against the newest non-junk file directly inside it — and the
//!   wording is exact. Removing an entry bumps only its immediate
//!   parent, so a comic taken out of `job/disc1/` leaves `job`
//!   untouched; and a global `max(dirs)` vs `max(files)` test would
//!   let a file anywhere else in the tree mask `disc1`'s bump. Earlier
//!   revisions had each of those defects in turn. A directory with no
//!   non-junk file directly in it has no baseline at all, so it is
//!   unevaluable and the whole folder abstains — which also means a
//!   release nested entirely in subfolders is never condemned.
//!
//! **Neither proves positive ownership.** Nothing here establishes
//! that the directory *is* this attempt's output; the checks only rule
//! out the two ways it is known to be someone else's or to have been
//! altered. That distinction is the same shape as "structurally
//! incapable of modifying anything" versus "structurally incapable of
//! causing a modification", and it is load-bearing: a reader who takes
//! these for ownership proofs will widen them and reintroduce the
//! false condemnations they exist to prevent.
//!
//! Their tolerances point in **opposite** directions, which is the
//! easiest thing here to get wrong — see [`COMPLETION_WINDOW_SECS`]
//! and [`REMOVAL_TOLERANCE_SECS`], whose comments must be read
//! together.
//!
//! # Only positive evidence condemns
//!
//! The terminal verdict requires files to be present AND **every**
//! non-junk file to be positively recognised as holding no comic (see
//! [`RECOGNISED_NON_PAYLOAD`]). Anything unrecognised abstains.
//!
//! Note what that is *not*: it is not "no file Phase B would import".
//! That was an earlier design, and it condemned on failure to
//! enumerate — a folder of `.7z.001` split volumes was declared empty
//! of comics while the comic sat inside the split set. Enumerating
//! containers is unbounded; enumerating known-inert files fails toward
//! abstention. The direction is the design.
//!
//! **An empty directory is not evidence either.** Phase B moves an
//! imported comic out of the watch folder, so "no files here" is
//! equally consistent with "this release was a PDF" and with "the
//! comic already imported".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use longbox_postprocess::skip::{should_skip, SkipReason};

/// How deep a release folder may nest before the walk gives up.
/// Exceeding it is reported as unreadable (→ abstain) rather than
/// silently truncating the walk: a truncated walk could miss the one
/// comic in the tree and turn a good release into a permanent
/// suppression.
const MAX_DEPTH: usize = 8;

/// Slack around the downloader's reported completion time within which
/// a folder's mtime is accepted as plausibly this job's.
///
/// **Deliberately generous, and that is the safe direction here.**
/// Outside the window means abstain, and abstaining is free — every
/// abstention leaves the attempt exactly as it was. The case this
/// exists to catch is a folder belonging to a job from a different
/// sweep, where the gap is hours to days, so an hour of slack costs
/// nothing and absorbs clock skew, slow post-processing, and a
/// container clock that disagrees with the downloader's.
///
/// **Read this together with [`REMOVAL_TOLERANCE_SECS`], whose
/// tolerance points the other way.** Widening one to match the other
/// is the mistake this pair of comments exists to prevent.
const COMPLETION_WINDOW_SECS: i64 = 3600;

/// Slack allowed between a folder's mtime and the newest file inside
/// it before concluding something was removed from it.
///
/// **Deliberately tight, because here generosity is the harmful
/// direction.** Within tolerance means "untouched", which means
/// *condemn* — so a wide tolerance manufactures false condemnations,
/// the exact error the whole module is built to avoid. Unlike
/// [`COMPLETION_WINDOW_SECS`], erring wide here is not free.
///
/// # Why one second, and not two
///
/// Measured, not assumed. Reading a directory's mtime from inside the
/// container after removing an entry on the host returned the
/// **pre-removal** value in 6 of 6 primed-cache rounds — a consistent
/// 1s staleness from virtiofsd's attribute cache. An unprimed read
/// taken 2s later showed no skew, so this is caching, not clock drift.
///
/// A stale mtime is *older*, which shrinks `newest_dir - newest_file`
/// and pushes toward "untouched" — the condemn direction. So staleness
/// already spends 1s of margin before this constant spends any, and
/// the effective threshold here is **~2s**, not 1.
///
/// If someone later measures 0s staleness on a different mount, that
/// is not a reason to raise this to 2: it would restore the effective
/// ~2s on a mount that no longer needs the compensation, and every
/// second bought back is spent in the harmful direction.
///
/// # Known thin spot
///
/// The case against going lower still rests on thin evidence: two live
/// job folders whose directory mtime equalled their file mtime to the
/// second. That is n=2, on one machine, at one moment.
///
/// An earlier version of this comment claimed SAB's post-job deletions
/// bump this folder's mtime, and named them as the dominant unknown.
/// **That reasoning was about the wrong directory.** SAB downloads and
/// repairs in its *incomplete* folder and extracts into *complete*;
/// both deletions it performs — par2 cleanup at the verify/repair
/// stage (gated on `enable_par_cleanup`, default on) and the pp=3
/// removal of the source archives — operate on files that live in
/// `incomplete`. Neither touches the directory this code reads.
///
/// So the feared "every pp=3 job abstains as `ContentsRemoved`" is
/// probably unfounded, and the real question is narrower and still
/// open: **what else mutates the completed folder after extraction?**
/// SAB's `cleanup_list` and its Sorting feature are the obvious
/// candidates and neither has been checked.
///
/// If it turns out to land seconds later on some jobs, the directory
/// mtime is legitimately newer than the surviving files and those
/// folders abstain. That is a false abstention — free, and visible as
/// the feature doing *less* rather than doing harm. Being wrong in
/// that direction is the intended failure mode, so this is recorded as
/// a thin spot rather than presented as settled.
///
/// **Do not tune this constant before reading the live data.** The
/// `newer_by` value logged on every `ContentsRemoved` measures exactly
/// this gap: seconds means a real import, hours or days means an
/// unpacker restoring archived mtimes. One week of sweeps answers it.
///
/// # Residual
///
/// A removal within this tolerance of the newest *surviving* file is
/// not detected, and the folder is condemned. Note the baseline is the
/// newest remaining file, **not** the unpack time: a litter file
/// written N seconds after the payload eats N seconds of margin, and
/// Phase B's stability wait keys on the comic rather than the litter,
/// so nothing compensates for that.
const REMOVAL_TOLERANCE_SECS: i64 = 1;

/// What a completed download's output directory contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandedVerdict {
    /// At least one file Phase B would import. Nothing to do — the
    /// existing `grabbed` path will settle the attempt.
    HasComicFiles,
    /// Files are present and none of them is a comic archive Phase B
    /// would import. This is the only verdict that settles an attempt.
    OnlyKnownNonPayload,
    /// Not enough evidence to decide. Leaves the attempt untouched.
    Inconclusive(Inconclusive),
}

/// Why a directory could not be judged. Carried for logs — when an
/// issue keeps retrying, this says which ambiguity is responsible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inconclusive {
    /// Missing, or present but not a directory. The common cause is a
    /// user who cleared the watch folder by hand — those downloads
    /// stay outside this check's reach and still age out to
    /// "lost track of download".
    NotADirectory,
    /// A read failed, the tree nested past [`MAX_DEPTH`], or the walk
    /// hit a symlink and refused to judge around it. The last is worth
    /// naming: an operator seeing this reason will otherwise hunt for
    /// an I/O error on a folder that has none.
    Unreadable,
    /// At least one non-junk file is not on
    /// [`RECOGNISED_NON_PAYLOAD`], so we cannot say the folder holds no
    /// comic — it may be an unextracted container, a format we do not
    /// read, or something we have simply never seen. Abstains.
    UnrecognisedContent,
    /// The downloader reported no usable completion time, so the
    /// completion window cannot be evaluated at all. Condemning with
    /// that guard off is refused; see [`classify`].
    NoCompletionTime,
    /// The folder's mtime sits outside [`COMPLETION_WINDOW_SECS`] of
    /// the job's reported completion, so it is some other job's folder
    /// occupying a re-allocated name.
    NotThisJobsFolder,
    /// A directory in the tree is newer than the newest file directly
    /// inside it. That is what the code proves — **not** that
    /// something was definitely removed, which is only the most likely
    /// cause (Phase B taking the comic and leaving the release's
    /// litter).
    ///
    /// An unpacker that restores archived mtimes onto extracted files
    /// produces the same signature with nothing removed at all, and
    /// `newer_by` tells the two apart: seconds is a real import, hours
    /// or days is restored timestamps. It is logged for exactly that
    /// reason — the distribution is measurable from the first live
    /// sweeps rather than assumed.
    ContentsRemoved { newer_by: i64 },
    /// A directory in the tree holds no non-junk file directly, so its
    /// mtime has no baseline to be compared against. Removing a
    /// directory's only file produces this state, so it cannot be read
    /// as innocent. A release nested entirely in subfolders also lands
    /// here — abstaining, which is the safe direction.
    DirectoryNotEvaluable,
    /// No regular files at all. See the module note: absence is not
    /// evidence, because Phase B removes what it imports.
    Empty,
    /// Something is still being written (`.partial` and friends), so
    /// the contents aren't final yet.
    StillWriting,
}

/// Judge the directory a completed download landed in.
///
/// `dir` is resolved by the caller against its own watch root — this
/// function never sees the downloader's own path, which belongs to a
/// different filesystem namespace.
pub fn classify(dir: &Path, job_completed_at: Option<i64>) -> LandedVerdict {
    if !dir.is_dir() {
        return LandedVerdict::Inconclusive(Inconclusive::NotADirectory);
    }
    let Some(dir_mtime) = mtime_secs(dir) else {
        return LandedVerdict::Inconclusive(Inconclusive::Unreadable);
    };

    // Guard 1 — proves this is not a LATER job's folder. See the
    // module docs, "What the two folder checks prove".
    //
    // A missing timestamp ABSTAINS rather than skipping the guard.
    // Skipping it was a state in which the code proceeded to a
    // condemnation with a guard switched off, invisibly — the same
    // shape `RemoteStorage` and `DiskObservation` exist to make
    // unreachable. A downloader that reports no completion time now
    // makes this feature decline everything, visibly, instead of
    // condemning on weaker evidence than the design assumes.
    let Some(completed) = job_completed_at else {
        return LandedVerdict::Inconclusive(Inconclusive::NoCompletionTime);
    };
    if (dir_mtime - completed).abs() > COMPLETION_WINDOW_SECS {
        return LandedVerdict::Inconclusive(Inconclusive::NotThisJobsFolder);
    }

    let mut files = 0usize;
    let mut comics = 0usize;
    let mut still_writing = false;
    let mut unrecognised = false;
    // Guard 2 is evaluated **per directory**: each directory's mtime
    // against the newest non-junk file sitting DIRECTLY in it.
    //
    // A global `max(dirs)` vs `max(files)` comparison is weaker and is
    // not what the guard claims: a removal from `disc1` is masked by
    // any file anywhere else in the tree that is as new as `disc1`'s
    // bump. Keeping the comparison local to each directory removes
    // that cross-directory masking entirely.
    let mut dirs: Vec<(PathBuf, i64)> = vec![(dir.to_path_buf(), dir_mtime)];
    let mut newest_in_dir: HashMap<PathBuf, i64> = HashMap::new();

    let walked = walk(dir, 0, &mut |path, is_dir| {
        let m = mtime_secs(path);
        if is_dir {
            if let Some(m) = m {
                dirs.push((path.to_path_buf(), m));
            }
            return;
        }
        // OS junk never counts, for either purpose. It is not part of
        // what the downloader delivered — `.DS_Store` appears because
        // someone opened the folder in Finder — so it is neither
        // evidence that a payload arrived nor a valid input to the
        // newest-file baseline. Counting its mtime is what blinds
        // guard 2: a file written *after* the removal drags the
        // baseline up to meet the directory and the delta collapses.
        if is_os_junk(path) {
            return;
        }
        files += 1;
        if let (Some(m), Some(parent)) = (m, path.parent()) {
            newest_in_dir
                .entry(parent.to_path_buf())
                .and_modify(|n| *n = (*n).max(m))
                .or_insert(m);
        }
        match should_skip(path) {
            // `should_skip` returning None is production's own
            // definition of "Phase B would import this". Sharing the
            // predicate is the point: a second extension list here
            // would drift from the one that actually decides, and the
            // disagreement would surface as suppressed good releases.
            None => comics += 1,
            Some(SkipReason::InProgress) => still_writing = true,
            // Not a comic — but "not a comic" is not "no comic here".
            // Only a file we have positively recognised as holding no
            // payload may contribute to a condemnation.
            Some(_) => {
                if !is_recognised_non_payload(path) {
                    unrecognised = true;
                }
            }
        }
    });

    if walked.is_err() {
        return LandedVerdict::Inconclusive(Inconclusive::Unreadable);
    }
    if comics > 0 {
        return LandedVerdict::HasComicFiles;
    }
    if still_writing {
        return LandedVerdict::Inconclusive(Inconclusive::StillWriting);
    }
    if files == 0 {
        return LandedVerdict::Inconclusive(Inconclusive::Empty);
    }
    if unrecognised {
        return LandedVerdict::Inconclusive(Inconclusive::UnrecognisedContent);
    }

    // Guard 2 — proves nothing was taken out of any directory in this
    // tree since it was unpacked. See the module docs, "What the two
    // folder checks prove". A directory's mtime bumps when an entry is
    // removed, so a directory newer than the newest file directly
    // inside it has had something deleted from it — which is exactly
    // what Phase B does when it imports the comic and leaves the
    // release's litter.
    //
    // A directory with no non-junk file directly in it has no baseline
    // to compare its mtime against, so "was something removed here?"
    // cannot be answered — and emptying a directory of its files
    // produces precisely that state, so it must not read as innocent.
    // Deliberately NOT falling back to the subtree's newest file: that
    // is the cross-directory masking this comparison was rewritten to
    // eliminate.
    //
    // A demonstrated removal is reported ahead of a merely unevaluable
    // directory. Both abstain, so the verdict is the same either way,
    // but the reason is logged and "something was taken out of here, by
    // N seconds" is the one that carries information.
    let mut unevaluable = false;
    for (path, dir_mtime) in &dirs {
        let Some(newest) = newest_in_dir.get(path) else {
            unevaluable = true;
            continue;
        };
        let newer_by = dir_mtime - newest;
        if newer_by > REMOVAL_TOLERANCE_SECS {
            return LandedVerdict::Inconclusive(Inconclusive::ContentsRemoved { newer_by });
        }
    }
    if unevaluable {
        return LandedVerdict::Inconclusive(Inconclusive::DirectoryNotEvaluable);
    }

    LandedVerdict::OnlyKnownNonPayload
}

/// Artefacts the operating system and file browsers leave in a folder,
/// which the downloader did not deliver.
///
/// Deliberately a short, explicit list rather than "anything starting
/// with a dot": a scene release could ship a meaningful dotfile, and
/// silently discarding it would be the same over-reach this module
/// exists to avoid.
///
/// [`is_os_junk`] additionally matches the `._` AppleDouble prefix —
/// narrow, specific, and justified there. The refusal above is of
/// *blanket* dot-matching, not of prefix rules as such.
///
/// `@eaDir` is Synology's thumbnail *directory*; the walk skips
/// descending into it, via [`is_named_os_junk`] rather than
/// [`is_os_junk`].
const OS_JUNK: [&str; 5] = [
    ".DS_Store",
    "Thumbs.db",
    ".localized",
    "desktop.ini",
    "@eaDir",
];

/// Extensions we **positively recognise as not containing a comic**.
///
/// The name states the claim on purpose. This is not "things that
/// aren't comics" — that is every extension in existence, and treating
/// it that way is what the previous design did. This is the much
/// smaller set we have actually looked at and know holds no payload.
///
/// # Why the list may be incomplete, and why that is safe
///
/// [`classify`] condemns only when **every** non-junk file in the tree
/// is on this list. Anything unrecognised — a container we have never
/// seen, a format that does not exist yet, a bare filename with no
/// extension — makes the folder abstain.
///
/// So incompleteness costs an abstention, never a condemnation. That
/// is the whole point of the inversion: the previous design enumerated
/// *containers* and condemned anything it failed to enumerate, so a
/// missing entry silently suppressed a delivered comic. `.7z.001`
/// split volumes were one such miss; `.zNN`, `.tgz`, `.arj` and next
/// year's format were the others waiting.
///
/// # Maintenance instruction
///
/// **Grow this list from observed abstentions, not from imagination.**
/// It starts deliberately narrow. If the live sweeps show
/// `landed_abstained` dominated by `UnrecognisedContent` for some
/// extension, add it — that is a one-line change against real data.
/// Guessing wide buys nothing (an omission is already free) and risks
/// adding something that *can* hold a comic, which is the one way an
/// entry here becomes harmful.
const RECOGNISED_NON_PAYLOAD: [&str; 11] = [
    ".pdf", ".nfo", ".diz", ".sfv", ".txt", ".url", ".m3u", ".srr", ".epub", ".mobi", ".azw3",
];

/// True when the filename ends in an extension we have positively
/// recognised as holding no comic. Unrecognised → `false` → abstain.
fn is_recognised_non_payload(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            RECOGNISED_NON_PAYLOAD.iter().any(|e| lower.ends_with(e))
        })
}

/// Whole-name match against [`OS_JUNK`] only. Used for directories,
/// where skipping hides evidence.
fn is_named_os_junk(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| OS_JUNK.iter().any(|j| name.eq_ignore_ascii_case(j)))
}

fn is_os_junk(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| {
            // AppleDouble sidecars: macOS writes `._<original>` on
            // filesystems without native xattr support, *after* the
            // fact. Same mechanism as `.DS_Store` — a post-unpack
            // write that would otherwise become guard 2's baseline and
            // collapse the delta. Prefix rather than exact match,
            // since the tail is the wrapped file's name.
            name.starts_with("._") || OS_JUNK.iter().any(|j| name.eq_ignore_ascii_case(j))
        })
}

/// Filesystem mtime in Unix seconds. `None` on a read failure or a
/// pre-epoch timestamp.
///
/// Where that `None` lands differs by caller, and only the first is an
/// abstention outright: for the job root it returns `Unreadable`; for
/// a subdirectory the directory is dropped from the per-directory
/// checks; for a file it contributes no baseline. Both of the latter
/// are self-correcting in practice — a subdirectory whose `metadata`
/// fails also fails the `read_dir` immediately after (→ `Unreadable`),
/// and a file whose `metadata` fails was just unlinked, which bumped
/// its directory's mtime (→ `ContentsRemoved`). Stated precisely
/// because "routes to an abstention" was not true of all three.
fn mtime_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// Recursive walk reporting each directory and regular file in the
/// tree; the `bool` passed to `visit` is true for directories.
///
/// Directories named in [`OS_JUNK`] are neither reported nor descended
/// into. That is the one junk rule that leans toward condemnation —
/// anything hidden inside such a directory cannot count as a comic —
/// so it is a whole-name match against the explicit list only, never
/// the `._` prefix rule.
///
/// Symlinks are not followed — a link can point outside the watch root
/// — and encountering one aborts the walk with an error, which the
/// caller turns into an abstention. Skipping them silently would leave
/// the folder judged while blind to what the link references, and the
/// judgement on offer is a condemnation.
fn walk(dir: &Path, depth: usize, visit: &mut impl FnMut(&Path, bool)) -> std::io::Result<()> {
    if depth >= MAX_DEPTH {
        return Err(std::io::Error::other(format!(
            "release folder nests deeper than {MAX_DEPTH} levels"
        )));
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            // Explicit list only, NOT the `._` prefix rule: skipping a
            // directory hides whatever is inside it, and the verdict
            // that follows is a condemnation. The prefix rule exists
            // to stop junk polluting guard 2's baseline, which only
            // files can do, so it has no business pointing at the one
            // junk decision that leans toward condemning.
            if is_named_os_junk(&entry.path()) {
                continue;
            }
            visit(&entry.path(), true);
            walk(&entry.path(), depth + 1, visit)?;
        } else if file_type.is_file() {
            visit(&entry.path(), false);
        } else if file_type.is_symlink() {
            // Not followed — a link can point outside the watch root.
            // But skipping it silently would judge the folder while
            // blind to whatever it references, and the verdict that
            // follows is a condemnation. Unknown contents must abstain,
            // not condemn.
            return Err(std::io::Error::other(
                "release folder contains a symlink; contents cannot be judged",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests_support {
    //! Shared fixture for both test modules in this file.
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Wall-clock now, in Unix seconds. Fixtures that do not age their
    /// files sit at "now", so this is the completion time that puts
    /// them inside the window without pinning the test to a constant.
    pub fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs() as i64
    }

    /// Scratch directory under the OS temp dir, removed on drop.
    pub struct Scratch(PathBuf);

    impl Scratch {
        pub fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "longbox-landed-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).expect("create scratch");
            Self(base)
        }
        pub fn file(&self, rel: &str) -> &Self {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, b"x").expect("write file");
            self
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
        /// Force a path's mtime. Used to age fixtures so the folder
        /// guards can be exercised without sleeping through the
        /// tolerance.
        pub fn age(&self, rel: &str, unix_secs: i64) -> &Self {
            let p = if rel.is_empty() {
                self.0.clone()
            } else {
                self.0.join(rel)
            };
            let t = filetime::FileTime::from_unix_time(unix_secs, 0);
            filetime::set_file_mtime(&p, t).expect("set mtime");
            self
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{now_secs, Scratch};
    use super::*;
    use std::fs;

    #[test]
    fn a_pdf_release_is_the_terminal_verdict() {
        // The observed shape: one PDF plus the usual usenet litter.
        let s = Scratch::new("pdf");
        s.file("Amazing Spider-Man 006.pdf")
            .file("info.nfo")
            .file("release.diz");
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::OnlyKnownNonPayload
        );
    }

    #[test]
    fn a_comic_anywhere_in_the_tree_blocks_the_terminal_verdict() {
        // Nested, because SAB unpacks into subfolders. A walk that
        // only read the top level would suppress this good release.
        let s = Scratch::new("nested");
        s.file("readme.nfo").file("disc1/Saga 001.cbz");
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::HasComicFiles
        );
    }

    #[test]
    fn cbr_counts_as_a_comic() {
        // `.cbr` is a comic; bare `.rar` is not. Production's skip
        // filter draws that line and this must inherit it.
        let s = Scratch::new("cbr");
        s.file("Saga 001.cbr");
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::HasComicFiles
        );

        // But "not a comic" is not "no comic here". A bare `.rar` is
        // unrecognised, so it abstains — and note it abstains without
        // appearing on any list, which is what the inversion bought.
        let t = Scratch::new("rar");
        t.file("Saga 001.rar");
        assert_eq!(
            classify(t.path(), Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::UnrecognisedContent)
        );
    }

    #[test]
    fn an_empty_directory_abstains_rather_than_condemning() {
        // The dangerous false positive: Phase B removes what it
        // imports, so an empty folder is equally consistent with a
        // successful import.
        let s = Scratch::new("empty");
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::Empty)
        );
    }

    #[test]
    fn a_directory_of_empty_subdirectories_still_abstains() {
        let s = Scratch::new("emptydirs");
        fs::create_dir_all(s.path().join("a/b")).unwrap();
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::Empty)
        );
    }

    #[test]
    fn an_in_progress_file_abstains() {
        // Contents aren't final; condemning now could suppress a
        // release whose comic simply hasn't finished writing.
        let s = Scratch::new("partial");
        s.file("Saga 001.cbz.partial").file("info.nfo");
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::StillWriting)
        );
    }

    #[test]
    fn a_missing_directory_abstains() {
        // Jeremy's hand-cleared watch folder lands here.
        let s = Scratch::new("missing");
        let gone = s.path().join("never-existed");
        assert_eq!(
            classify(&gone, Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::NotADirectory)
        );
    }

    #[test]
    fn nesting_past_the_depth_cap_abstains_rather_than_truncating() {
        // A truncated walk could miss the only comic and permanently
        // suppress a good release, so depth exhaustion must abstain.
        let s = Scratch::new("deep");
        let deep = "a/".repeat(MAX_DEPTH + 2);
        s.file(&format!("{deep}Saga 001.cbz"));
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::Unreadable)
        );
    }

    #[test]
    fn os_junk_alone_is_not_evidence_of_a_delivered_payload() {
        // Previously asserted OnlyKnownNonPayload. That was defensible in
        // isolation and was exactly the silhouette of the false
        // positive: `.DS_Store` appears because someone opened the
        // folder in Finder, so a folder holding only OS junk is
        // equally consistent with "imported, then browsed".
        for junk in [".DS_Store", "Thumbs.db", "desktop.ini"] {
            let s = Scratch::new("junk");
            s.file(junk);
            assert_eq!(
                classify(s.path(), Some(now_secs())),
                LandedVerdict::Inconclusive(Inconclusive::Empty),
                "{junk} is not a payload file"
            );
        }
    }
}

#[cfg(test)]
mod guard_tests {
    //! The two folder guards, tested separately. Each fixture is built
    //! so that breaking one guard cannot be masked by the other.

    use super::tests_support::Scratch;
    use super::*;

    /// A fixed, plausible completion time. Fixtures are aged to it so
    /// the tests do not depend on wall-clock at run time.
    const T: i64 = 1_780_000_000;

    // -------- guard 1: the completion window --------

    #[test]
    fn a_folder_from_a_later_job_is_not_judged() {
        // The theft case. The recorded path is re-allocated to a
        // different job days later; its folder is junk, and without
        // this guard the stale attempt eats the verdict.
        let s = Scratch::new("laterjob");
        s.file("info.nfo");
        s.age("info.nfo", T + 200_000).age("", T + 200_000);

        assert_eq!(
            classify(s.path(), Some(T)),
            LandedVerdict::Inconclusive(Inconclusive::NotThisJobsFolder),
            "a folder created days after the job completed is someone else's"
        );
    }

    #[test]
    fn a_folder_inside_the_window_is_still_judged() {
        // Negative control for guard 1: within slack, the verdict must
        // still be reached, or the guard would suppress everything.
        let s = Scratch::new("inwindow");
        s.file("release.pdf").file("info.nfo");
        s.age("release.pdf", T).age("info.nfo", T).age("", T);

        assert_eq!(
            classify(s.path(), Some(T + 60)),
            LandedVerdict::OnlyKnownNonPayload
        );
    }

    #[test]
    fn without_a_completion_time_nothing_is_condemned() {
        // Previously this asserted the window check was SKIPPED and
        // the folder condemned anyway — a state where the code reached
        // a condemnation with a guard switched off, and nothing said
        // so. A downloader that reports no timestamp now makes the
        // feature decline everything, visibly.
        let s = Scratch::new("nots");
        s.file("release.pdf");
        s.age("release.pdf", T).age("", T);

        assert_eq!(
            classify(s.path(), None),
            LandedVerdict::Inconclusive(Inconclusive::NoCompletionTime)
        );
    }

    // -------- guard 2: the removal detector --------

    #[test]
    fn a_folder_something_was_removed_from_abstains() {
        // Phase B imported the comic and left the release's litter.
        // The directory mtime bumped on the unlink; the litter kept
        // its unpack time. Guard 1 passes (the folder IS this job's),
        // so only guard 2 can catch this.
        let s = Scratch::new("removed");
        s.file("info.nfo");
        s.age("info.nfo", T).age("", T + 600);

        assert!(
            matches!(
                classify(s.path(), Some(T + 600)),
                LandedVerdict::Inconclusive(Inconclusive::ContentsRemoved { .. })
            ),
            "a directory newer than everything in it has had something deleted"
        );
    }

    #[test]
    fn an_untouched_folder_is_condemned() {
        // Negative control for guard 2: a genuine PDF release, never
        // touched since unpack, must still be condemned — otherwise
        // the guard has disabled the feature.
        let s = Scratch::new("untouched");
        s.file("release.pdf").file("info.nfo");
        s.age("release.pdf", T).age("info.nfo", T).age("", T);

        assert_eq!(
            classify(s.path(), Some(T)),
            LandedVerdict::OnlyKnownNonPayload
        );
    }

    #[test]
    fn a_removal_from_a_subfolder_is_detected() {
        // The blindness a root-only comparison had. Removing an entry
        // bumps ONLY its immediate parent (measured on APFS), so the
        // job root's mtime is untouched and cannot see this.
        let s = Scratch::new("subremoval");
        s.file("disc1/info.nfo");
        s.age("disc1/info.nfo", T).age("disc1", T + 600).age("", T);

        assert!(
            matches!(
                classify(s.path(), Some(T)),
                LandedVerdict::Inconclusive(Inconclusive::ContentsRemoved { .. })
            ),
            "a bumped subdirectory is the only trace of a nested import"
        );
    }

    #[test]
    fn os_junk_written_after_a_removal_does_not_blind_the_guard() {
        // The general case, not just the junk-only one: Finder writes
        // .DS_Store after Phase B took the comic. If junk counted
        // toward the newest-file baseline it would rise to meet the
        // directory and the delta would collapse to zero.
        let s = Scratch::new("junkblind");
        s.file("info.nfo").file(".DS_Store");
        s.age("info.nfo", T)
            .age(".DS_Store", T + 600)
            .age("", T + 600);

        assert!(
            matches!(
                classify(s.path(), Some(T)),
                LandedVerdict::Inconclusive(Inconclusive::ContentsRemoved { .. })
            ),
            "OS junk must not be a valid newest-file baseline"
        );
    }

    #[test]
    fn a_removal_within_tolerance_is_not_detected() {
        // The named residual. Documented on REMOVAL_TOLERANCE_SECS
        // rather than hidden: a deletion within the tolerance of
        // unpack finishing reads as untouched.
        let s = Scratch::new("tolerance");
        s.file("info.nfo");
        s.age("info.nfo", T).age("", T + REMOVAL_TOLERANCE_SECS);

        assert_eq!(
            classify(s.path(), Some(T)),
            LandedVerdict::OnlyKnownNonPayload
        );
    }
}

#[cfg(test)]
mod per_directory_tests {
    //! The per-directory comparison, separately from the two guards it
    //! lives inside. Breaking it must fail these and an integration
    //! test, not only the guard tests.

    use super::tests_support::{now_secs, Scratch};
    use super::*;

    const T: i64 = 1_780_000_000;

    #[test]
    fn a_sibling_directory_cannot_mask_a_removal() {
        // The weakness a global max(dirs) vs max(files) comparison
        // had: disc2's untouched file is as new as disc1's removal
        // bump, so a global test sees delta 0 and condemns. Only a
        // per-directory comparison catches this.
        let s = Scratch::new("mask");
        s.file("disc1/info.nfo").file("disc2/extra.nfo");
        s.age("disc1/info.nfo", T)
            .age("disc1", T + 600)
            .age("disc2/extra.nfo", T + 600)
            .age("disc2", T + 600)
            .age("", T);

        assert!(
            matches!(
                classify(s.path(), Some(T)),
                LandedVerdict::Inconclusive(Inconclusive::ContentsRemoved { .. })
            ),
            "a file in a sibling directory must not vouch for disc1"
        );
    }

    #[test]
    fn a_directory_with_no_files_of_its_own_is_unevaluable() {
        // Emptying a directory of its files produces exactly this
        // state, so it must not read as innocent.
        let s = Scratch::new("noeval");
        s.file("disc1/info.nfo");
        s.age("disc1/info.nfo", T).age("disc1", T).age("", T);

        assert_eq!(
            classify(s.path(), Some(T)),
            LandedVerdict::Inconclusive(Inconclusive::DirectoryNotEvaluable),
            "the root holds no file of its own, so its mtime has no baseline"
        );
    }

    #[test]
    fn the_reported_gap_is_the_observed_one() {
        // The delta is logged so the real-world distribution can be
        // measured rather than assumed — it has to be the real number.
        let s = Scratch::new("delta");
        s.file("info.nfo");
        s.age("info.nfo", T).age("", T + 42);

        assert_eq!(
            classify(s.path(), Some(T)),
            LandedVerdict::Inconclusive(Inconclusive::ContentsRemoved { newer_by: 42 })
        );
    }

    #[test]
    fn an_apple_double_sidecar_does_not_become_the_baseline() {
        let s = Scratch::new("appledouble");
        s.file("info.nfo").file("._Saga 001.cbz");
        s.age("info.nfo", T)
            .age("._Saga 001.cbz", T + 600)
            .age("", T + 600);

        assert!(
            matches!(
                classify(s.path(), Some(T)),
                LandedVerdict::Inconclusive(Inconclusive::ContentsRemoved { .. })
            ),
            "._* is written after the fact, like .DS_Store"
        );
    }

    #[test]
    fn a_symlink_makes_the_folder_unjudgeable() {
        let s = Scratch::new("symlink");
        s.file("info.nfo");
        std::os::unix::fs::symlink("/tmp", s.path().join("disc1")).expect("symlink");

        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::Unreadable),
            "unknown contents must abstain, not be condemned blind"
        );
    }
}

#[cfg(test)]
mod recognition_tests {
    //! The inverted condition: condemn only when EVERY non-junk file
    //! is positively recognised as holding no comic.
    //!
    //! The previous design enumerated container extensions and
    //! condemned anything it failed to enumerate — so a missing entry
    //! suppressed a delivered comic. These tests pin the direction as
    //! much as the behaviour.

    use super::tests_support::{now_secs, Scratch};
    use super::*;

    #[test]
    fn a_genuine_pdf_release_is_condemned() {
        // The live case: one PDF plus scene litter, every file
        // recognised. This is the only shape that earns a terminal
        // verdict.
        let s = Scratch::new("pdfrel");
        s.file("Amazing Spider-Man 006.pdf")
            .file("info.nfo")
            .file("release.diz");
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::OnlyKnownNonPayload
        );
    }

    #[test]
    fn unextracted_containers_abstain_without_being_enumerated() {
        // None of these is on any list. They abstain because they are
        // UNRECOGNISED, which is the whole point — `.7z.001` was the
        // miss that killed the enumerate-containers design, and under
        // the inversion it needs no entry at all.
        for name in [
            "Saga 001.rar",
            "Saga 001.part01.rar",
            "Saga 001.r00",
            "Saga.7z.001",
            "Saga.z01",
            "Saga 001.tgz",
            "Saga 001.arj",
            "Saga 001.some-format-invented-next-year",
            "Saga 001",
        ] {
            let s = Scratch::new("unrec");
            s.file(name).file("info.nfo");
            assert_eq!(
                classify(s.path(), Some(now_secs())),
                LandedVerdict::Inconclusive(Inconclusive::UnrecognisedContent),
                "{name} must abstain without being enumerated"
            );
        }
    }

    #[test]
    fn an_unlisted_par2_costs_an_abstention_not_a_condemnation() {
        // `.par2` is deliberately NOT on the recognised list — it was
        // never observed in a live folder, and the maintenance rule is
        // to add from observed abstentions rather than imagination.
        //
        // So this abstains, and that is the designed behaviour rather
        // than an oversight: under the inversion an omission costs an
        // abstention, never a condemnation. Contrast the previous
        // design, where `.par2`'s presence or absence in the container
        // list flipped between inertness and condemning a delivered
        // comic — a question that simply does not arise now.
        let s = Scratch::new("par2rel");
        s.file("Amazing Spider-Man 006.pdf").file("Saga 001.par2");
        assert_eq!(
            classify(s.path(), Some(now_secs())),
            LandedVerdict::Inconclusive(Inconclusive::UnrecognisedContent)
        );
    }

    #[test]
    fn a_comic_still_wins_over_everything() {
        for name in ["Saga 001.cbz", "Saga 001.cbr"] {
            let s = Scratch::new("comicwins");
            s.file(name).file("info.nfo");
            assert_eq!(
                classify(s.path(), Some(now_secs())),
                LandedVerdict::HasComicFiles,
                "{name}"
            );
        }
    }

    #[test]
    fn ebook_formats_we_cannot_read_are_recognised_non_payload() {
        for name in ["Saga 001.epub", "Saga 001.mobi", "Saga 001.azw3"] {
            let s = Scratch::new("ebooks");
            s.file(name).file("info.nfo");
            assert_eq!(
                classify(s.path(), Some(now_secs())),
                LandedVerdict::OnlyKnownNonPayload,
                "{name}"
            );
        }
    }
}
