-- When the Metron issue-linking resolver last fetched+matched this series'
-- Metron issues. NULL = not yet done. Only series with a metron_id are
-- candidates; this marks a linked series' issue-fetch as complete so it isn't
-- re-fetched (no-churn, mirrors metron_link_checked_at at the series level).
ALTER TABLE series ADD COLUMN metron_issues_linked_at TIMESTAMP;
