-- Pull-engine release-title exclusion keywords.
--
-- Comma-separated list of substrings the indexer pre-grab filter
-- silently drops from every result pool. Used to keep digital-only
-- formats out of the library — Marvel "Infinity Comic" (vertical-
-- scroll exclusive) and DC's "Infinite Comic" (DCU Infinite-exclusive)
-- titles surface as ordinary search hits because they're issue-numbered
-- inside their host volume, but they're not what the user is pulling for.
--
-- The match is a normalized substring check on the release title
-- (lowercased, dots → spaces) — same shape the parser normalizer uses
-- so "Infinity.Comic" and "INFINITY COMIC" both hit. Dropped releases
-- are silent (no diagnostic, no mismatch row) so they don't pollute
-- the parseable_count signal the filter uses to distinguish series-
-- mismatch from year-mismatch.
--
-- INSERT OR IGNORE: idempotent for hand-edited DBs that may have already
-- set this key out of band.
INSERT OR IGNORE INTO settings (key, value) VALUES
    ('pull_exclusion_keywords', 'Infinity Comic,Infinite Comic');
