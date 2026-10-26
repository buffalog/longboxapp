-- Per-issue Metron detail cache. The forward calendar writes each issue
-- here as it hydrates, so a load cut short (closed tab, shutdown) keeps
-- what it fetched and the next load resumes from there. The week-level
-- metron_calendar_cache stays the fast path for a complete week.
CREATE TABLE metron_issue_cache (
    metron_issue_id INTEGER PRIMARY KEY,
    payload_json    TEXT NOT NULL,
    cached_at       TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
