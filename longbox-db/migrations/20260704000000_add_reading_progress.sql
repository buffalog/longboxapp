-- Per-issue reader position for the built-in comic reader. One row per
-- issue that has been opened; absence means "never opened" and the reader
-- treats that as page 1. `last_page` stores the reader's primary position
-- (in spread mode, the left page of the current spread) so reopening lands
-- on the same place. ON DELETE CASCADE drops progress when an issue is
-- removed — a stale reading position has no meaning without its issue.
CREATE TABLE reading_progress (
    issue_id   INTEGER PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
    last_page  INTEGER NOT NULL DEFAULT 1,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
