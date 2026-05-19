# Phase B+ queue

Shaped follow-up items deferred out of Phase B. Each entry has a
source (why it's here), scope (what to build), acceptance (how we know
it's done), and explicit out-of-scope notes so the next implementer
doesn't drift.

For the unshaped backlog — items the Phase B brief deliberately
deferred without further design — see the "Out of scope (Phase B+)"
section of `longbox-phase-b-prompt.md`. Items move from there to this
queue once they've been shaped to the level below.

## B+.1 — PollWatcher fallback for virtiofs / network filesystem environments

**Source:** Phase B manual smoke finding #3 (documented in
`longbox-phase-b-known-limitations.md`).

**Scope:** add `notify::PollWatcher` as a runtime-configurable
alternative to `RecommendedWatcher` in `longbox-postprocess`. Config
knob `DOWNLOAD_WATCH_BACKEND=poll|recommended|auto` chooses backend;
`auto` detects filesystem characteristics or defaults to `recommended`
with documented escape hatch.

**Acceptance:** drop a file from macOS host into Colima-mounted watch
folder, watcher fires within poll interval (default 2–5 s), file
processes end-to-end without container restart.

**Out of scope for B+.1:** auto-detection of virtiofs/SMB/NFS mounts.
The escape hatch (env var) ships first; auto-detection is a follow-up
if needed.
