-- schema.sql -- DDL for the github_pr box
--
-- Tables:
--   events          - raw webhook events (PR opened, push, comment, review, etc.)
--   review_comments - review comments posted by the box or fetched from GitHub
--   box_meta        - key/value metadata store (cursor, state, config)

CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    pr_id       INTEGER NOT NULL,
    event_type  TEXT    NOT NULL,
    actor       TEXT    NOT NULL DEFAULT '',
    payload     TEXT    NOT NULL DEFAULT '{}',
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

:do:

CREATE TABLE IF NOT EXISTS review_comments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    pr_id       INTEGER NOT NULL,
    path        TEXT    NOT NULL DEFAULT '',
    line        INTEGER NOT NULL DEFAULT 0,
    body        TEXT    NOT NULL DEFAULT '',
    author      TEXT    NOT NULL DEFAULT '',
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

:do:

CREATE TABLE IF NOT EXISTS box_meta (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL DEFAULT '',
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

:do:

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_events_pr_id ON events(pr_id);
CREATE INDEX IF NOT EXISTS idx_events_type  ON events(pr_id, event_type);
CREATE INDEX IF NOT EXISTS idx_comments_pr  ON review_comments(pr_id);
