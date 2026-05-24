-- A.9 F6 hot-fix: split discovered_folders.dismissed_at into user vs
-- auto sources.
--
-- The original column conflated two semantically different states: a
-- user explicitly clicking Dismiss in /library/tidy ("I never want to
-- see this folder again") vs. the system auto-dismissing because the
-- folder transitioned out of untracked (post-add, post-convert, or
-- scanner F6 scan-end resolution). The upsert path skipped any
-- dismissed row, so once auto-dismissed a folder could never
-- resurface — even when its files later became unmatched again.
-- That trap stranded 26 folders / ~763 unmatched files in the live
-- catalog at the time of the hot-fix.
--
-- After this migration:
--   - `dismissed_at`      = user-permanent dismiss. Written only by
--                           `discovered_folders_repo::dismiss` (the
--                           /api/reconcile/dismiss route). Upsert
--                           preserves it; rows stay hidden until a
--                           manual un-dismiss path exists.
--   - `auto_dismissed_at` = state-derived dismiss. Written by
--                           `auto_dismiss` (post-add, post-convert)
--                           and `auto_dismiss_not_in` (scanner F6).
--                           Upsert clears it on re-detection; rows
--                           resurface when files re-qualify as
--                           untracked.
--
-- Backfill (Q2 policy b): every existing dismissed_at migrates to
-- auto_dismissed_at. The recent session is dominated by system-driven
-- dismisses (F6 + bulk-convert success post-action) chasing the dedup
-- and parser hot-fixes; treating those as user-permanent would leave
-- the 26 trapped folders stranded. If any past explicit user dismiss
-- gets demoted to auto here, the user re-dismisses in the UI on next
-- surfacing (one click) — small cost vs. the ~763-file resurfacing
-- win.
--
-- The upsert's WHERE dismissed_at IS NULL guard from the original
-- migration stays (the user-permanent semantics); the new ON CONFLICT
-- branch additionally clears auto_dismissed_at so re-detection
-- actually resurfaces auto-dismissed rows.

ALTER TABLE discovered_folders ADD COLUMN auto_dismissed_at TIMESTAMP;

UPDATE discovered_folders
   SET auto_dismissed_at = dismissed_at,
       dismissed_at = NULL
 WHERE dismissed_at IS NOT NULL;
