-- CV volume `aliases` (newline-separated alternate titles, e.g. FBP's
-- original "Collider"). Stored verbatim as CV returns it; the pull-search
-- path splits on newline at read time. NULL = none known.
ALTER TABLE series ADD COLUMN aliases TEXT;
