-- Item A v2 piece 2: schema + settings rows for the Metron forward-calendar
-- integration. Purely additive; no existing rows or columns are altered or
-- migrated. The runtime feature stays inert until `metron_enabled` is flipped
-- to 'true' AND the deploy env carries valid METRON_API_USER /
-- METRON_API_PASSWORD; piece 3 wires the route handler that consumes both.
--
-- Migration count goes 18 -> 19. Bug 5's boot-integrity assertion catches
-- a missing-on-boot case here automatically (embedded migrations re-counted
-- from the embedded migrator on every recompile; applied count read live
-- from _sqlx_migrations).

-- 1. Cross-reference column reuses the EXISTING `series.metron_id`
--    column added in the 20260516040415 initial migration. That column
--    was reserved for exactly this purpose (TEXT UNIQUE — the UNIQUE
--    constraint provides the index queries need, including WHERE
--    metron_id IS NOT NULL lookups for piece 4's backfill pass).
--
--    No ALTER needed here. Piece 4's repo code stores Metron's numeric
--    series id as text (i64.to_string()) at the boundary; UNIQUE
--    semantics still hold because each Metron series resolves to at
--    most one LongBox series.

-- 2. Forward-calendar cache. Shape mirrors `cv_release_cache` but in its
--    own namespace because the payload shape is different (Metron-projected,
--    not CV-projected); conflating the two would force a wrapper enum on
--    every read. Per-week granularity matches the calendar's stepWeek
--    navigation unit.
CREATE TABLE metron_calendar_cache (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    date_from     TEXT NOT NULL,
    date_to       TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    cached_at     TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(date_from, date_to)
);

-- 3. Kill switch + tunables for the Metron path. Three rows (per
--    piece-2 kickoff resolution — the "fourth metron_* row" mentioned in
--    Q5 was a miscount; nothing functional needed a fourth).
--
--    - metron_enabled            'false'   — kill switch (default safe)
--    - metron_calendar_forward_weeks '4'   — depth of forward fetches
--    - metron_calendar_cache_ttl_hours '24' — cache freshness
INSERT INTO settings (key, value) VALUES
    ('metron_enabled', 'false'),
    ('metron_calendar_forward_weeks', '4'),
    ('metron_calendar_cache_ttl_hours', '24');

-- 4. GCD placeholder rows. No functional wiring — these reserve the
--    settings-row keys so a future GCD integration doesn't need a
--    schema migration, and they're discoverable in any future admin UI
--    that lists settings. The asymmetry with Metron's env-var pattern
--    is intentional: Metron is functional and credentialed; GCD is
--    reserved real estate.
INSERT INTO settings (key, value) VALUES
    ('gcd_api_user', ''),
    ('gcd_api_password', '');
