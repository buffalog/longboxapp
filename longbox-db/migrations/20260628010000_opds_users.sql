-- Per-user OPDS accounts, replacing the single shared credential that lived
-- in the `settings` table (opds_username / opds_password_hash / opds_api_token).
-- The global `opds_enabled` toggle stays in `settings`.

CREATE TABLE opds_users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL COLLATE NOCASE UNIQUE,
    password_hash TEXT NOT NULL,  -- bcrypt
    enabled       INTEGER NOT NULL DEFAULT 1,  -- 0 = disabled, 1 = active
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT  -- updated on successful auth
);

-- Preserve existing access: if a username AND a password hash were both
-- configured under the old single-credential model, carry them into the new
-- table as the first enabled account. The seed migration stored empty strings
-- (not NULL) when unconfigured, so guard on non-empty TRIM.
INSERT INTO opds_users (username, password_hash, enabled)
SELECT u.value, h.value, 1
FROM settings u
JOIN settings h ON h.key = 'opds_password_hash'
WHERE u.key = 'opds_username'
  AND TRIM(u.value) <> ''
  AND TRIM(h.value) <> '';

-- Drop the obsolete single-credential + bearer-token rows. `opds_enabled` stays.
DELETE FROM settings
WHERE key IN ('opds_username', 'opds_password_hash', 'opds_api_token');
