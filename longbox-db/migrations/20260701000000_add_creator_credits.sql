-- Creator credits (role-attributed). `creators` dedupes a person across
-- sources via cv_person_id; `issue_credits` is the many-to-many person+role+
-- issue graph (one row per ATOMIC role — CV's comma-delimited role strings
-- are split at ingestion). `issues.credits_fetched` gates the background
-- credits resolver so each issue's per-issue CV detail is fetched once.
CREATE TABLE creators (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL,
    cv_person_id     INTEGER UNIQUE,
    metron_person_id INTEGER UNIQUE,
    created_at       TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_creators_name ON creators(name COLLATE NOCASE);

CREATE TABLE issue_credits (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    creator_id  INTEGER NOT NULL REFERENCES creators(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    UNIQUE(issue_id, creator_id, role)
);
CREATE INDEX idx_issue_credits_creator ON issue_credits(creator_id);
CREATE INDEX idx_issue_credits_issue   ON issue_credits(issue_id);

ALTER TABLE issues ADD COLUMN credits_fetched BOOLEAN NOT NULL DEFAULT 0;
