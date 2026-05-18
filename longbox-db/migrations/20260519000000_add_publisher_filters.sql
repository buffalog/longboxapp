-- Task 4: international reprint filter.
--
-- CV returns one volume per publisher per language for popular IPs
-- (Batman, Spider-Man, etc.). The user wants the English original, not
-- the Panini/Planeta/Glénat/etc. reprints. Blocklist-by-publisher-name
-- with curated defaults is the right shape.
--
-- - publisher_name is case-insensitive via COLLATE NOCASE; UNIQUE
--   constraint inherits the collation, so "Panini Comics" and
--   "panini comics" can't both exist.
-- - mode='allow' is reserved for a future opt-in allowlist mode (not
--   exposed in UI today). The CHECK constraint accepts it at the DB
--   layer so we can change the policy without a migration.
-- - Names verified against CV's actual publisher field before seeding.

CREATE TABLE publisher_filters (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    publisher_name  TEXT NOT NULL COLLATE NOCASE UNIQUE,
    mode            TEXT NOT NULL DEFAULT 'block'
                    CHECK (mode IN ('block', 'allow')),
    created_at      TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO publisher_filters (publisher_name, mode) VALUES
    ('Panini Comics', 'block'),
    ('Panini France', 'block'),
    ('Panini Brasil', 'block'),
    ('Planeta DeAgostini', 'block'),
    ('Editorial Televisa', 'block'),
    ('ECC Ediciones', 'block'),
    ('Norma Editorial', 'block'),
    ('Éditions Glénat', 'block'),
    ('Arnoldo Mondadori Editore', 'block'),
    ('Salvat', 'block');
