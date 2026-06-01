-- Phase B virtiofs polling fallback.
--
-- The inotify-based watcher (notify::RecommendedWatcher → INotifyWatcher
-- on Linux) is blind to host-written files through Docker Desktop's
-- virtiofs mount: a CBZ landing in /watch from outside the container
-- never fires a Create event. The fix is to swap RecommendedWatcher for
-- notify::PollWatcher, which detects changes by walking the directory on
-- a fixed interval and diffing mtime+size.
--
-- This row is the poll interval, in whole seconds. 30s is the default
-- compromise: a downloaded comic shows up in LongBox within half a
-- minute; CPU cost is negligible on a watch folder that typically holds
-- single-digit files. Tune down for higher-throughput SABnzbd setups,
-- up for NAS-mounted folders where the walk itself is expensive.
INSERT INTO settings (key, value) VALUES
    ('phase_b_poll_interval_seconds', '30');
