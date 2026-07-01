-- When the Metron-linking resolver last checked this series against Metron
-- (matched or not). NULL = never checked. Distinguishes "no Metron match"
-- from "not yet attempted" so the resolver's work-list doesn't re-query
-- unmatched series forever.
ALTER TABLE series ADD COLUMN metron_link_checked_at TIMESTAMP;
