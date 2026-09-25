-- Letters a person has edited.
--
-- The three request letters ship embedded in the binary. Editing them used
-- to mean rebuilding; now a person can change the wording from the web
-- interface, and what they wrote is stored here.
--
-- Overrides, not copies: an absent row means "send the shipped letter", so a
-- wording fix in a release still reaches anyone who has not gone out of
-- their way to change it. Reverting is a delete.
--
-- Subjects are stored too, but a stored subject is still fixed text: the
-- subject a row carries is interpolated per broker the same way the shipped
-- ones are, and an injected newline in a subject line would be header
-- injection, so newlines are stripped before anything is stored or sent.

CREATE TABLE letter_overrides (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- generic, ccpa, or gdpr — the embedded template this replaces.
    name       TEXT NOT NULL,
    subject    TEXT NOT NULL,
    body       TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),

    PRIMARY KEY (user_id, name)
);
