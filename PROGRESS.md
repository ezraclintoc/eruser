# Port progress

Running log of the Go → Rust port. Ordered newest first. Every entry
corresponds to a commit on `main`.

**Status:** Phase 1 (core) in progress — 6 of 8 modules ported, 140 tests
passing.

| Module | Upstream source | Go LOC | Rust tests | Status |
|---|---|---|---|---|
| `broker` | `internal/broker/broker.go` | 203 | 20 | ported |
| `config` | `internal/config/config.go` | 208 | 25 | ported |
| `template` | `internal/template/template.go` | 141 | 17 | ported |
| `email` | `internal/email/{sender,smtp}.go` | 169 | 16 | ported |
| `history` | `internal/history/history.go` | 901 | 43 | ported |
| `send` | extracted from `main.go` + `web/job.go` | — | 19 | ported |
| `cli` | `cmd/eraser/main.go` | 1577 | — | next |
| `web` | `internal/web/*` | 2942 | — | queued |
| `inbox` | `internal/inbox/*` | 1669 | — | phase 2 |
| `browser` | `internal/browser/*` | 1498 | — | phase 2 |

---

## Changelog

### `send` — the send pipeline

Extracted the per-broker render/send/record loop that existed twice in Go
(CLI and web UI) into one implementation with a progress callback.
Cancellation is awaited inside the rate-limit delay, so stopping a run is
immediate. Failures record a flattened error chain, so history says *which*
address was rejected.

### `history` — SQLite store

Ported to sqlx with migrations. Every table carries `user_id` from the first
migration with a seeded `default` user, so multi-user support later does not
mean rewriting populated databases. Broker replies are stored with an upsert
against a unique index rather than Go's select-then-insert, which could
duplicate a reply across two monitor runs.

### `email` — SMTP sending

lettre replaces the hand-rolled MIME envelope and TLS handshake. `Sender` is
an object-safe async trait, which is what makes the send pipeline testable
without a mail server. Every message now carries a real RFC 5322 Message-ID,
so a broker's reply can later be matched to the request it answers.

### `template` — request templates

Wording unchanged from upstream; syntax converted from Go `text/template` to
Jinja via minijinja. Undefined variables are now hard errors — Go rendered a
missing field as `<no value>` and sent the email anyway.

### `config` — user configuration

Same YAML schema, so an existing `~/.eraser/config.yaml` loads unchanged.
Insecure file permissions are an error rather than a printed warning, and the
file is created 0600 at open rather than chmod-ed afterwards.

### `broker` — broker database

764 entries carried over verbatim. Lookups return `Option<&Broker>` instead of
a nullable pointer into the backing slice; directory loads are sorted, so the
same directory always produces the same database.

### Scaffolding

Cargo project, README, MIT LICENSE, broker data, email templates.
