-- OPDS catalog access settings (longbox-opds).
--
-- Four key-value rows on the existing `settings` table. `settings.value`
-- is NOT NULL, so the "unset" state is the empty string rather than SQL
-- NULL; the OPDS auth layer treats an empty username/hash/token as absent.
--
-- opds_enabled starts 'false' so the catalog is inert until the operator
-- configures credentials in the Settings UI (which flips this to 'true'
-- and generates the API token).
INSERT OR IGNORE INTO settings (key, value) VALUES
    ('opds_enabled', 'false'),
    ('opds_username', ''),
    ('opds_password_hash', ''),
    ('opds_api_token', '');
