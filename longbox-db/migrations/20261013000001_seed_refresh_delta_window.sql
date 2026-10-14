-- Catalog delta refresh window (days either side of today). Each
-- scheduled pull sweep upserts CV issues whose store_date falls inside
-- it. last_full_refresh_at is deliberately NOT seeded: no row means the
-- full refresh has never run, which starts one at boot.
INSERT OR IGNORE INTO settings (key, value) VALUES
  ('refresh_delta_days_back',  '14'),
  ('refresh_delta_days_ahead', '28');
