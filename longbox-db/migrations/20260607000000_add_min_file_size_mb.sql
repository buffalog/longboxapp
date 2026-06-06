-- Phase B size-floor: reject downloads below this many MB as
-- presumed partial/corrupt SAB deliveries. The load-bearing case
-- is an Absolute Batman issue (typically 50+ MB) arriving at 16 MB
-- with five rendered pages — the file is technically a valid CBR
-- (archive opens, ComicInfo parses) but the user definitely doesn't
-- want it in the library.
--
-- Default 35 catches the common failure modes while still admitting
-- legitimate small-format releases (ashcans, single-issue digital
-- comics with sparse art). Phase B reads this per-file via
-- `settings_repo::get_or_default`, so tuning via the Settings UI
-- takes effect on the next /watch/ arrival without a restart.

INSERT INTO settings (key, value) VALUES ('min_file_size_mb', '35');
