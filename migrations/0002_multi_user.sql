-- Several people, several sending accounts.
--
-- Until now a single config.yaml held one person's profile and one mailbox,
-- and every row belonged to user 1. This moves the per-person settings into
-- the database so more than one person can use an instance, and splits the
-- sending credentials into their own table so one person can send through
-- several accounts.
--
-- The file is not abandoned: it is imported once on first run, and the
-- import is what a single-user install upgrades through.

-- Who someone is, as the brokers need to see them.
--
-- Separate from `users` because that table is about signing in and this one
-- is about identity: a broker matching a record needs the name and the
-- addresses, and none of that has anything to do with authentication.
CREATE TABLE user_profiles (
    user_id       INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    first_name    TEXT NOT NULL DEFAULT '',
    last_name     TEXT NOT NULL DEFAULT '',
    email         TEXT NOT NULL DEFAULT '',
    address       TEXT NOT NULL DEFAULT '',
    city          TEXT NOT NULL DEFAULT '',
    state         TEXT NOT NULL DEFAULT '',
    zip_code      TEXT NOT NULL DEFAULT '',
    country       TEXT NOT NULL DEFAULT '',
    phone         TEXT NOT NULL DEFAULT '',
    date_of_birth TEXT NOT NULL DEFAULT '',
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

-- How someone wants their requests sent and their replies read.
CREATE TABLE user_settings (
    user_id          INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    template         TEXT NOT NULL DEFAULT 'generic',
    rate_limit_ms    INTEGER NOT NULL DEFAULT 2000,
    -- JSON arrays. Stored as text because SQLite has no array type and these
    -- are only ever read whole.
    regions          TEXT NOT NULL DEFAULT '[]',
    excluded_brokers TEXT NOT NULL DEFAULT '[]',

    inbox_enabled    INTEGER NOT NULL DEFAULT 0,
    inbox_provider   TEXT NOT NULL DEFAULT '',
    inbox_server     TEXT NOT NULL DEFAULT '',
    inbox_port       INTEGER NOT NULL DEFAULT 0,
    inbox_email      TEXT NOT NULL DEFAULT '',
    inbox_password   TEXT NOT NULL DEFAULT '',
    inbox_folder     TEXT NOT NULL DEFAULT 'INBOX',

    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

-- An account requests can be sent from.
--
-- Several per person is the point: Gmail stops accepting around 500 messages
-- a day, so a 764-broker run either takes two days on one account or one day
-- across two. A run rolls over to the next account when one is spent.
CREATE TABLE sender_accounts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- A name the person recognises, e.g. "personal gmail".
    label         TEXT NOT NULL DEFAULT '',
    -- personal: only the owner sends through it.
    -- family:   anyone on this instance may send through it.
    --
    -- Worth being deliberate about. A shared account lets someone else put
    -- mail into the world over your address, and broker replies to it land
    -- in the owner's mailbox rather than the sender's.
    scope         TEXT NOT NULL DEFAULT 'personal',
    -- smtp, resend, or sendgrid.
    provider      TEXT NOT NULL,
    -- The address requests are sent from, and what brokers reply to.
    from_address  TEXT NOT NULL,

    smtp_host     TEXT NOT NULL DEFAULT '',
    smtp_port     INTEGER NOT NULL DEFAULT 0,
    smtp_username TEXT NOT NULL DEFAULT '',
    smtp_password TEXT NOT NULL DEFAULT '',
    smtp_use_tls  INTEGER NOT NULL DEFAULT 1,
    api_key       TEXT NOT NULL DEFAULT '',

    -- How many this account will send in a day before the run moves on.
    daily_limit   INTEGER NOT NULL DEFAULT 250,
    -- Set false to keep an account configured but out of the rotation.
    enabled       INTEGER NOT NULL DEFAULT 1,
    -- Lower goes first, so a free account can be spent before a paid one.
    priority      INTEGER NOT NULL DEFAULT 0,

    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_sa_user  ON sender_accounts(user_id, enabled, priority);
-- Shared accounts are looked up across owners, so they get their own index.
CREATE INDEX idx_sa_scope ON sender_accounts(scope, enabled, priority);

-- One person cannot add the same address twice; two people can each have
-- their own, which is why the user is part of the key.
CREATE UNIQUE INDEX idx_sa_user_address ON sender_accounts(user_id, from_address);

-- Which account a request went out through.
--
-- This is what makes the daily count per account rather than per person:
-- without it, three accounts would share one cap and the rollover would
-- never trigger.
ALTER TABLE removal_requests ADD COLUMN sender_account_id INTEGER;

CREATE INDEX idx_rr_sender_account ON removal_requests(sender_account_id, sent_at);

-- Give the existing user the rows the code now expects. Both are empty
-- until the config file is imported, which happens on first run.
INSERT INTO user_profiles (user_id) VALUES (1);
INSERT INTO user_settings (user_id) VALUES (1);
