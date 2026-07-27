-- Content-identity columns for Library Integrity.
--
-- The dominant corruption in a real library is byte-identical files sitting on
-- SEPARATE issue records, one file each, at high match confidence. Library Tidy
-- is structurally blind to it: Tidy groups by `issue_id`, and these copies share
-- no issue. Only content identity finds them, so we need a durable digest.
--
-- BLAKE3, not MD5/SHA-2: this is dedup, not a security boundary, and BLAKE3 is
-- the fastest option that isn't collision-trivial. The column is NAMED for its
-- algorithm on purpose — a future swap adds `content_<algo>` alongside and
-- backfills, instead of silently reinterpreting rows under a generic
-- `content_hash` whose meaning changed at some migration nobody remembers.
--
-- All three columns are nullable and lazily populated. Hashing is confined to
-- size-collision candidates (two files of differing size cannot be identical),
-- so in a healthy library the overwhelming majority of rows stay NULL forever —
-- 80 of 7102 on the library this was built against.
--
-- `hashed_size_bytes` / `hashed_mtime` record the file version the digest was
-- computed against. A digest is trusted only when they still equal the row's
-- current `size_bytes` / `mtime`; anything else means the file changed under us
-- and the digest is stale. That check is self-validating — it cannot be
-- defeated by a writer that forgets to invalidate, which matters because a
-- stale digest here would present two DIFFERENT files as identical and offer
-- the user a delete button.
ALTER TABLE files ADD COLUMN content_blake3 TEXT;
ALTER TABLE files ADD COLUMN hashed_size_bytes INTEGER;
ALTER TABLE files ADD COLUMN hashed_mtime TIMESTAMP;

-- Partial index: the duplicate-detection query groups by digest, and only the
-- hashed minority is ever non-NULL. Indexing the NULLs would be almost the
-- entire table for no lookup benefit.
CREATE INDEX idx_files_content_blake3
    ON files(content_blake3)
    WHERE content_blake3 IS NOT NULL;
