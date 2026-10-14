-- Split the ComicVine budget into an interactive and a background lane.
-- 100 + 80 = 180/h, the old single bucket. The background lane's bucket
-- replaces the enrichment worker's 30s request-interval gate, so that
-- setting goes.
INSERT OR IGNORE INTO settings (key, value) VALUES
  ('cv_interactive_rate_per_hour', '100'),
  ('cv_interactive_burst',         '10'),
  ('cv_background_rate_per_hour',  '80'),
  ('cv_background_yield_seconds',  '300');

DELETE FROM settings WHERE key = 'cv_enrichment_request_interval_seconds';
