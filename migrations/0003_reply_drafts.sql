-- Drafts a model wrote, waiting for a person to send.
--
-- The replier is opt-in and works in drafts: when the monitor classifies a
-- broker reply that deserves an answer, a local model writes one, and the
-- draft lands here and on the task list. Nothing is sent by itself unless
-- the reply type is on the user's auto-send whitelist — and identity
-- requests are hardcoded unsendable in every tier.
--
-- A draft is keyed to the reply it answers, so re-running the monitor never
-- writes a second draft for an email already answered.

CREATE TABLE reply_drafts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL DEFAULT 1 REFERENCES users(id) ON DELETE CASCADE,

    broker_id    TEXT NOT NULL,
    broker_name  TEXT NOT NULL,

    -- Which kind of answer this is: missing_info, confirm, or
    -- id_verification. Drives both the prompt and what may auto-send.
    reply_type   TEXT NOT NULL,

    -- The broker reply this answers, by subject — the same key the
    -- broker_responses table deduplicates on.
    in_reply_to  TEXT NOT NULL,

    subject      TEXT NOT NULL,
    body         TEXT NOT NULL,

    -- Which model wrote it, so a person can judge the draft with that in
    -- mind. Empty for a template-only draft.
    model        TEXT NOT NULL DEFAULT '',

    -- draft:    waiting for a person, or for the auto-send whitelist.
    -- sent:     gone out through the normal send path.
    -- discarded: a person read it and did not want it.
    status       TEXT NOT NULL DEFAULT 'draft',

    -- The validation report: what the drafter was allowed to say and what
    -- it actually said. A person reviewing the draft sees this too.
    validation   TEXT NOT NULL DEFAULT '{}',

    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    sent_at      TEXT,

    UNIQUE(user_id, broker_id, in_reply_to, reply_type)
);

CREATE INDEX idx_rd_user_status ON reply_drafts(user_id, status);
CREATE INDEX idx_rd_user_broker ON reply_drafts(user_id, broker_id);
