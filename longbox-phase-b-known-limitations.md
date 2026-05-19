# Phase B known limitations

Limitations discovered during Phase B closeout manual smoke. Each entry
documents observed behavior, downstream impact, the resolution path,
and Phase B+ deferral status.

## L1 — Host → virtiofs → inotify event delivery is broken on Colima

### Behavior

When LongBox runs in a container under Colima (the macOS-on-aarch64
Docker host used in this project), bind-mounted host directories
surface inside the VM via `virtiofs`. Files written from the macOS
host into a bind-mounted watch folder are visible to the container
(via subsequent `read_dir` or `stat`), but **`notify::RecommendedWatcher`
does not fire events for those writes**. Empirically confirmed during
Phase B Step 8 smoke: five CBZ files dropped from the host into
`/Users/jeremy/longbox-phase-b-watch` (bind-mounted to `/watch` in the
container) produced zero `phase_b.detected` events; the same files
touched from inside the container fired events immediately.

The notify crate uses inotify on Linux via `RecommendedWatcher`.
inotify events originate inside the VM kernel and are only generated
for filesystem changes that the VM kernel sees first-hand. virtiofs
delivers file content from the host on demand (read-through) but does
not synthesize inotify events for host-side mutations, so events
genuinely don't exist for the VM kernel to deliver.

This is a virtiofs / Lima architectural property, not a notify bug or
a LongBox bug. Same behavior is reproducible with any inotify-based
tool inside Colima with host-bind-mounted watch directories.

### Impact

Production SAB-style workflow on this deployment shape:

- SAB writes a completed download to `$DOWNLOAD_WATCH_HOST` (macOS
  host path).
- The file is immediately visible inside the container's `/watch`.
- The notify watcher **does not fire**. The file sits indefinitely.
- The next container restart triggers `initial_sweep`, which walks
  the watch folder and enqueues the file. The pipeline then runs to
  completion normally.

In practice, files get processed only on container restart, not on
arrival. Phase B's per-file latency goes from "few seconds" (per the
brief) to "until the next restart" — minutes to days depending on
operator behavior.

Non-Colima deploys (Linux-native containers without virtiofs, bare
metal, plain Docker on Linux) are unaffected: native inotify works
end-to-end.

### Resolution path

`notify` 8.2 exposes `notify::PollWatcher` as an explicit alternative
to `RecommendedWatcher`. It scans the watch tree on a configurable
interval (default 30 s, tunable down to seconds) and synthesizes the
same `Event` shape from filesystem diffs. Trade-off: per-poll CPU and
inode-cache pressure proportional to watch-folder size, vs. real
event delivery on filesystems that don't support native events.

Sketch of the fix in `longbox-postprocess/src/lib.rs` (`spawn_watcher`):

```rust
// Detect virtiofs / non-event-supporting backends at startup and pick
// the right watcher type. Or just always use PollWatcher when an env
// var like LONGBOX_POLL_INTERVAL is set, falling back to recommended
// otherwise.
use notify::{Config, PollWatcher, RecommendedWatcher, Watcher};

let mut watcher: Box<dyn Watcher + Send> = match std::env::var("LONGBOX_POLL_INTERVAL_SECS") {
    Ok(s) if s.parse::<u64>().is_ok() => {
        let secs: u64 = s.parse().unwrap();
        Box::new(PollWatcher::new(callback, Config::default().with_poll_interval(Duration::from_secs(secs)))?)
    }
    _ => Box::new(notify::recommended_watcher(callback)?),
};
```

A 5–10 second poll interval keeps per-file latency in the same
ballpark as the brief's "few seconds" target and is cheap enough for
typical watch folders (single SAB completion subdir, never more than a
few hundred files in flight). Folders with thousands of pending files
should bump the interval higher.

### Status

Phase B+ deferral. Workaround (container restart triggers
initial-sweep) is sufficient for development verification, but real
production use on this deployment shape needs `PollWatcher` wired in
behind a config knob. The acceptance test continues to validate the
processing path; this limitation only affects the host → watcher
event-delivery edge.

Cross-references: `longbox-postprocess/src/lib.rs`
(`spawn_watcher`, `paths_from_event`, `eviction_paths_from_event` —
all unchanged by this fix; only the watcher backend swaps).
