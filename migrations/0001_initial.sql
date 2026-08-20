-- Initial schema.
--
-- Ported from the migrate() function in internal/history/history.go, with one
-- structural change: every row is owned by a user. Multi-user support is on
-- the roadmap, and retrofitting ownership onto a populated database is far
-- more painful than carrying an always-1 column until the feature lands.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL UNIQUE,
    -- NULL until authentication exists. A NULL hash can never match a
    -- password, so these rows are not loggable-in by accident.
    password_hash TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

-- The single implicit user of a local install.
INSERT INTO users (id, username, password_hash) VALUES (1, 'default', NULL);

CREATE TABLE removal_requests (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL DEFAULT 1 REFERENCES users(id) ON DELETE CASCADE,
    broker_id       TEXT NOT NULL,
    broker_name     TEXT NOT NULL,
    email           TEXT NOT NULL,
    template        TEXT NOT NULL,
    status          TEXT NOT NULL,
    message_id      TEXT NOT NULL DEFAULT '',
    error           TEXT NOT NULL DEFAULT '',
    sent_at         TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    pipeline_status TEXT NOT NULL DEFAULT 'email_sent'
);

CREATE INDEX idx_rr_user_broker     ON removal_requests(user_id, broker_id);
CREATE INDEX idx_rr_user_sent_at    ON removal_requests(user_id, sent_at);
CREATE INDEX idx_rr_user_status     ON removal_requests(user_id, status);
CREATE INDEX idx_rr_user_pipeline   ON removal_requests(user_id, pipeline_status);

-- Classified email replies from brokers.
CREATE TABLE broker_responses (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL DEFAULT 1 REFERENCES users(id) ON DELETE CASCADE,
    broker_id     TEXT NOT NULL,
    broker_name   TEXT NOT NULL,
    response_type TEXT NOT NULL,
    email_from    TEXT NOT NULL DEFAULT '',
    email_subject TEXT NOT NULL DEFAULT '',
    -- Retained so responses can be reclassified after a classifier change
    -- without re-fetching the mailbox.
    email_body    TEXT NOT NULL DEFAULT '',
    form_url      TEXT NOT NULL DEFAULT '',
    confirm_url   TEXT NOT NULL DEFAULT '',
    confidence    REAL NOT NULL DEFAULT 0,
    needs_review  INTEGER NOT NULL DEFAULT 0,
    received_at   TEXT,
    processed_at  TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_br_user_broker  ON broker_responses(user_id, broker_id);
CREATE INDEX idx_br_user_type    ON broker_responses(user_id, response_type);
CREATE INDEX idx_br_user_review  ON broker_responses(user_id, needs_review);

-- A reply is identified by (broker, subject); the monitor uses this to avoid
-- storing the same message twice across runs.
CREATE UNIQUE INDEX idx_br_user_broker_subject
    ON broker_responses(user_id, broker_id, email_subject);

-- Work that needs a human: CAPTCHAs, manual forms, review, confirmations.
CREATE TABLE pending_tasks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL DEFAULT 1 REFERENCES users(id) ON DELETE CASCADE,
    broker_id       TEXT NOT NULL,
    broker_name     TEXT NOT NULL,
    task_type       TEXT NOT NULL,
    form_url        TEXT NOT NULL DEFAULT '',
    screenshot_path TEXT NOT NULL DEFAULT '',
    -- JSON blob of the browser state the helper page needs to resume.
    browser_state   TEXT NOT NULL DEFAULT '',
    notes           TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    opened_at       TEXT,
    completed_at    TEXT
);

CREATE INDEX idx_pt_user_broker ON pending_tasks(user_id, broker_id);
CREATE INDEX idx_pt_user_type   ON pending_tasks(user_id, task_type);
CREATE INDEX idx_pt_user_status ON pending_tasks(user_id, status);
