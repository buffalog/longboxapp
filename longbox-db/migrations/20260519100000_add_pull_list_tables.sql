-- Phase A.8 Step 3: pull list, indexer/downloader/webhook config, and
-- the CV release cache.
--
-- CHECK constraints are used on the new small, closed enum columns
-- (`downloader_config.kind`, `pull_attempts.status`) — matches the
-- `publisher_filters.mode` precedent. The unconstrained-TEXT pattern
-- (files.match_method/status) is reserved for genuinely open enums.
--
-- Note: the brief calls the downloader-config discriminator column
-- `type`; it is named `kind` here — `type` collides with the Rust
-- keyword in the sqlx-generated row struct.

-- Single-row downloader config. CHECK(id = 1) enforces exactly one
-- row at the DB layer; the repo upserts via INSERT OR REPLACE on id=1.
-- `secret` is dual-use: the SABnzbd apikey OR the NZBGet control
-- password. `username` is NZBGet-only (Basic auth) — NULL for SABnzbd.
CREATE TABLE downloader_config (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    kind        TEXT NOT NULL CHECK (kind IN ('sab', 'nzbget')),
    base_url    TEXT NOT NULL,
    username    TEXT,
    secret      TEXT NOT NULL,
    category    TEXT NOT NULL DEFAULT '',
    enabled     INTEGER NOT NULL DEFAULT 1,
    updated_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Newznab indexers — multi-row, queried in ascending `priority` order.
CREATE TABLE indexer_configs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT NOT NULL UNIQUE,
    base_url     TEXT NOT NULL,
    api_key      TEXT NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 1,
    priority     INTEGER NOT NULL DEFAULT 0,
    maxage_days  INTEGER NOT NULL DEFAULT 1500,
    created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Webhook notification targets. `event_mask` is an INTEGER bitset;
-- bit positions are defined in `webhook_config_repo` (EVENT_* consts).
CREATE TABLE webhook_configs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    url         TEXT NOT NULL,
    event_mask  INTEGER NOT NULL DEFAULT 0,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Series subscribed for auto-pull — one row per series.
-- `start_issue` is TEXT (issue numbers are TEXT: "Annual 1", "½");
-- NULL means "pull from the first solicited issue". `failure_count`
-- is zeroed by the repo on a successful pull.
CREATE TABLE pull_list (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id                INTEGER NOT NULL UNIQUE
                             REFERENCES series(id) ON DELETE CASCADE,
    added_at                 TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    start_issue              TEXT,
    paused                   INTEGER NOT NULL DEFAULT 0,
    last_pull_attempt_at     TIMESTAMP,
    last_successful_pull_at  TIMESTAMP,
    failure_count            INTEGER NOT NULL DEFAULT 0
);

-- Per-attempt audit + retry-exclusion log. series_id/issue_id both
-- CASCADE (an attempt is operationally meaningless once its catalog
-- rows are gone; the durable "what was auto-pulled" record lives on
-- files.match_method='pull_list'). indexer_id is SET NULL — an indexer
-- can be deconfigured without nuking attempt history.
CREATE TABLE pull_attempts (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id      INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    issue_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    attempted_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    indexer_id     INTEGER REFERENCES indexer_configs(id) ON DELETE SET NULL,
    release_id     TEXT,
    status         TEXT NOT NULL
                   CHECK (status IN ('pending', 'submitted', 'grabbed', 'failed', 'mismatched')),
    error_message  TEXT,
    retry_count    INTEGER NOT NULL DEFAULT 0
);
-- Phase B's processor looks up pending attempts by (series_id, issue_id)
-- to attribute a caught file to the pull engine.
CREATE INDEX idx_pull_attempts_series_issue ON pull_attempts(series_id, issue_id);

-- CV release-calendar cache, keyed by date range + publisher. The
-- payload is the CV projection serialized as JSON. TTL is a read-time
-- policy (release-calendar view: short; pull engine: daily) — the
-- repo exposes `cached_at` and a prune-stale helper; no `kind` split.
CREATE TABLE cv_release_cache (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    date_from     TEXT NOT NULL,
    date_to       TEXT NOT NULL,
    publisher     TEXT NOT NULL DEFAULT '',
    payload_json  TEXT NOT NULL,
    cached_at     TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(date_from, date_to, publisher)
);
