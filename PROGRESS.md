# Port progress

Running log of the Go → Rust port. Ordered newest first. Every entry
corresponds to a commit on `main`.

**Status: the port is complete.** Every module in upstream eraser has a Rust
counterpart. 520 tests passing across 19,752 lines.

| Module | Upstream source | Go LOC | Rust tests |
|---|---|---|---|
| `broker` | `internal/broker/broker.go` | 203 | 21 |
| `config` | `internal/config/config.go` | 208 | 25 |
| `template` | `internal/template/template.go` | 141 | 17 |
| `email` | `internal/email/{sender,smtp}.go` | 169 | 16 |
| `history` | `internal/history/history.go` | 901 | 43 |
| `send` | extracted from `main.go` + `web/job.go` | — | 19 |
| `cli` | `cmd/eraser/main.go` | 1577 | 78 |
| `web` | `internal/web/*` + 21 templates | 2942 | 132 |
| `inbox` | `internal/inbox/*` | 1669 | 96 |
| `automation` | `internal/browser/*` | 1498 | 73 |

Commands: `init`, `send`, `list-brokers`, `status`, `add-broker`, `monitor`,
`confirm`, `fill`, `serve`.

---

## Bugs found in the original

Each of these has a test pinning it, so it cannot come back.

- **Classification was not deterministic.** Go picked the winning category
  while ranging over a map, whose iteration order is randomised, so the same
  broker reply could be filed differently from one run to the next — and
  which classification got stored depended on when the mailbox happened to be
  scanned.
- **A CSRF cookie wiped the setup wizard's session.** The middleware set its
  cookie with `insert`, which replaces every `Set-Cookie` header on the
  response. The wizard started a fresh session on every step and lost every
  answer.
- **Expiry pages counted as successful confirmations.** Success wording was
  checked before failure wording, and expiry pages routinely thank you
  somewhere on them, so "This link has expired. Thank you for your interest."
  was recorded as a confirmed removal.
- **A street address could overwrite an email address.** The form filler tried
  each field mapping against its own selector list independently, so a box
  named `email_address` matched both the email mapping and the address
  mapping.
- **Every `.gif` link was discarded as a tracking pixel.** `ends_with(".gif")
  || ends_with(".png") && contains("pixel")` — `&&` binds tighter, so the
  "pixel" test only ever applied to `.png`.
- **A copied-out binary silently sent nothing.** With no `data/` directory
  beside it, the broker database loaded zero entries and the run reported
  success.
- **A failed run looked like a successful one.** A job stopped by a bad
  password was marked `completed` with an error string attached.
- **"Already confirmed" was filed as a failure**, so re-running `confirm`
  turned finished work into reported errors.
- **Every reCAPTCHA was reported as v2.** v3 is invisible and usually passes
  on its own, so those pages were handed to a person for nothing.
- **The app password was echoed to the terminal** while being typed.
- **`browser_headless` could not be turned off.** The config key was
  documented, and `Load()` overwrote it with `true` on every load.

## Deliberate departures

- **Assets are served locally.** Upstream's web UI pulled Tailwind from
  cdn.tailwindcss.com, HTMX from unpkg, and two webfonts from Google on every
  page load — so a privacy tool announced itself to three third parties each
  time it was opened, and rendered unstyled with no network.
- **The schema is multi-user from the first migration.** Every table carries
  `user_id` with a seeded `default` user. Nothing exposes it yet; retrofitting
  ownership onto a populated database later is far worse.
- **Forms are not submitted by default.** `fill` types into the boxes, saves a
  full-page screenshot, and leaves the sending to a person. A form that
  submits the wrong thing cannot be un-submitted.
- **The IMAP connection is read-only.** The mailbox is opened with `EXAMINE`,
  so scanning does not mark anything as read, and upstream's archive-and-move
  is not ported. A matcher that misfires and moves real mail is worse than one
  that leaves it alone.
- **"We have no record of you" counts as a refusal.** One of the commonest
  ways a broker says it holds nothing, and it was going to the review queue.

---

## Changelog

### `automation` — form filling

chromiumoxide replaces chromedp. Deciding what goes in which box is now a
pure function over the fields a page declares, so the matching table has
tests for the first time; the browser only reads fields and types back what
it is given.

### `automation` — confirmation links and CAPTCHA detection

Following a confirmation link is an ordinary GET, so it does not use a
browser at all. The outcome is an enum rather than a bool: a bare 200 with
nothing on the page is "unclear", not a success.

### `inbox` — scanning wired into the CLI and the web UI

`eruser monitor`, and the three `/api/inbox` endpoints. Reply bodies are
stored so `--reclassify` can re-read them after the patterns change, without
going back to a mailbox that may since have been cleared.

### `inbox` — IMAP monitor

async-imap with rustls. Subjects are RFC 2047 decoded, so a German broker's
reply is no longer unreadable to the classifier. Broker matching indexes the
website domain as well as the contact address.

### `inbox` — reply parser and classifier

The pattern tables come over unchanged; upstream built them from real broker
mail. Their test cases came over verbatim too.

### `web` — the server, handlers, and setup wizard

axum and tower replace chi. Routes and markup unchanged.

### `web` — templates and assets

All 21 templates converted from Go `html/template` to Jinja with a
stack-based converter. Tailwind, HTMX, and the webfonts are served from the
machine running eruser.

### `web` — sessions, jobs, and security middleware

Sessions keep the SMTP password server-side during setup. CSRF tokens are
compared in constant time.

### `cli` — the command line

Each command its own module, with the output-producing parts as pure
functions so the wording is testable. The broker database is embedded in the
binary.

### `send` — the send pipeline

Extracted the per-broker loop that existed twice in Go. Cancellation is
awaited inside the rate-limit delay, so stopping a run is immediate.

### `history` — SQLite store

sqlx with migrations. Broker replies are stored with an upsert against a
unique index rather than Go's select-then-insert.

### `email` — SMTP sending

lettre replaces the hand-rolled MIME envelope and TLS handshake. Every
message carries a real RFC 5322 Message-ID, so a reply can be matched to the
request it answers.

### `template` — request templates

Wording unchanged. Undefined variables are hard errors — Go rendered a
missing field as `<no value>` and sent the email anyway.

### `config` — user configuration

Same YAML schema. Insecure file permissions are an error, and the file is
created 0600 at open rather than chmod-ed afterwards.

### `broker` — broker database

764 entries carried over verbatim.

### Scaffolding

Cargo project, README, MIT LICENSE, broker data, email templates.
