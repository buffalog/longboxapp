-- Phase A.8 Step 6: pull-attempt download tracking.
--
-- download_handle — the downloader's own job id (SABnzbd nzo_id /
-- NZBGet NZBID), captured at submission so the pull engine can poll the
-- download's status on later sweeps. NULL until a submit succeeds.
--
-- unknown_polls — consecutive status polls that returned
-- DownloadStatus::Unknown (the job is in neither the downloader's queue
-- nor its history). A flaky downloader can briefly drop a job from both,
-- so the engine transitions the attempt to 'failed' only after a
-- threshold of *consecutive* Unknowns, not on the first one.

ALTER TABLE pull_attempts ADD COLUMN download_handle TEXT;
ALTER TABLE pull_attempts ADD COLUMN unknown_polls INTEGER NOT NULL DEFAULT 0;
