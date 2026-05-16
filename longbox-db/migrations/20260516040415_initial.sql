-- Phase A initial schema.
--
-- Match the brief's "Data model" section verbatim. Enum string values for
-- `files.match_method` and `files.status` are validated in application code
-- (see longbox-core::file::{MatchMethod, FileStatus}); the DB column type is
-- plain TEXT and no CHECK constraints are imposed.

CREATE TABLE library_roots (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT NOT NULL UNIQUE,
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE series (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    cv_id       INTEGER UNIQUE,
    metron_id   TEXT UNIQUE,
    title       TEXT NOT NULL,
    sort_title  TEXT NOT NULL,
    start_year  INTEGER,
    publisher   TEXT,
    description TEXT,
    cover_url   TEXT,
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_series_sort_title ON series(sort_title);

CREATE TABLE issues (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id       INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    cv_issue_id     INTEGER UNIQUE,
    metron_issue_id TEXT UNIQUE,
    number          TEXT NOT NULL,
    title           TEXT,
    cover_date      TEXT,
    summary         TEXT,
    cover_url       TEXT,
    created_at      TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at      TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(series_id, number)
);
CREATE INDEX idx_issues_series ON issues(series_id);

CREATE TABLE files (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id              INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    library_root_id       INTEGER NOT NULL REFERENCES library_roots(id),
    path_relative         TEXT NOT NULL,
    size_bytes            INTEGER NOT NULL,
    mtime                 TIMESTAMP NOT NULL,
    last_scanned_at       TIMESTAMP NOT NULL,
    match_method          TEXT NOT NULL,
    match_confidence      REAL NOT NULL,
    status                TEXT NOT NULL,
    cached_comicinfo_xml  TEXT,
    cached_at             TIMESTAMP,
    UNIQUE(library_root_id, path_relative)
);
CREATE INDEX idx_files_issue ON files(issue_id);
CREATE INDEX idx_files_status ON files(status);
CREATE INDEX idx_files_match_method ON files(match_method);

CREATE TABLE scan_runs (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    library_root_id     INTEGER NOT NULL REFERENCES library_roots(id),
    started_at          TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at         TIMESTAMP,
    files_seen          INTEGER NOT NULL DEFAULT 0,
    files_added         INTEGER NOT NULL DEFAULT 0,
    files_updated       INTEGER NOT NULL DEFAULT 0,
    files_matched       INTEGER NOT NULL DEFAULT 0,
    files_needs_review  INTEGER NOT NULL DEFAULT 0,
    files_unmatched     INTEGER NOT NULL DEFAULT 0,
    status              TEXT NOT NULL DEFAULT 'running',
    error_message       TEXT
);

CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE parsing_patterns (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    pattern     TEXT NOT NULL,
    priority    INTEGER NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Seed: default confidence threshold.
INSERT INTO settings (key, value) VALUES
  ('match_confidence_threshold', '0.85');

-- Seed: default parsing patterns. Priority order matters — lower numbers run
-- first. Pattern at priority 5 (volume-aware) runs first so it can claim
-- filenames containing volume markers before later patterns absorb them.
INSERT INTO parsing_patterns (name, pattern, priority, enabled) VALUES
  ('Series Vol N #M',
   '^(?P<series>.+?)\s+(?i:v|vol|volume)\s*(?P<volume>\d+)\s+#?(?P<number>\d+(?:\.\d+)?).*?\.(?i:cbz|cbr|cb7)$',
   5, 1),
  ('Series #NNN (YYYY)',
   '^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})\)(?:\s+-\s+(?P<title>.+))?\.(?i:cbz|cbr|cb7)$',
   10, 1),
  ('Series NNN (YYYY)',
   '^(?P<series>.+?)\s+(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})\)\.(?i:cbz|cbr|cb7)$',
   20, 1),
  ('Series_NNN or Series NNN',
   '^(?P<series>.+?)[_\s]+#?(?P<number>\d+(?:\.\d+)?)\.(?i:cbz|cbr|cb7)$',
   30, 1);
