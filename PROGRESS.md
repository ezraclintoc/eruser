# Port progress

Running log of the Go → Rust port. Ordered newest first. Every entry
corresponds to a commit on `main`.

**Status: the port is complete, and the fork has started diverging.** Every
module in upstream eraser has a Rust counterpart. 842 tests passing.

Work since the port: several people on one instance, each with their own
sign-in, settings and mailboxes, and several sending accounts per person so a
run is not capped by one provider's daily limit.

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
  `user_id` with a seeded `default` user. That seeded row is what an upgrade
  claims on first run, so an existing history stays with the person who owns
  it rather than being stranded beside a new account.
- **Settings are in the database, not a file.** Go read one `config.yaml` for
  the whole install; with several people, one profile and one mailbox for
  everyone is not usable. An existing file is imported once and the database
  is authoritative afterwards.
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

### `reply` — an auto-replier, in drafts

The roadmap's "AI response pipeline" as a first instalment: when the monitor
files a reply that deserves an answer, a drafting model the user runs — any
OpenAI-compatible endpoint, so Ollama, llama.cpp server, LM Studio, or vLLM —
writes one, and the draft lands on the task list. A person reads every word
and presses send; or, if the type is on the machine's `auto_send` whitelist,
it can go out by itself. Identity-verification requests are hardcoded
unsendable, checked at both the decision and the door.

The rules the model works under are enforced in code, not hoped for in a
prompt: every fact in a draft must come from the person's profile, the
broker's email is framed as data rather than instructions, and a validator
refuses any draft that promises documents, agrees to anything, invents an
email address, or runs long. It is re-validated at send time, so a draft
written under old rules cannot slip past new ones. A refused draft is no
draft, and the log says why.

A draft is keyed to the reply it answers, so re-running the monitor never
duplicates it, and it rides the task list as a `draft_reply` task — the same
queue a captcha uses. Off by default, like the solver; with the defaults,
nothing changes.

Also fixed here: pipeline settings (headless, timeout, solver, drafting) were
only ever read from the database's copy of the config, which does not carry
them — `fill` now reads that section from config.yaml, where it lives, and
the web server reads it the same way.

### `automation` — an optional captcha solver, tried before a person

Where the roadmap's "automatic CAPTCHA solving" lands, with the fork's rule
kept intact: a solver is an optimisation, never a gate. When a challenge
blocks a fill and a solver is configured, the solver gets a go — but its
claim of success is not evidence. The page is re-read, and only a challenge
that has genuinely gone lets the fill proceed. Every other path — solver
unreachable, solver reports failure, solver claims victory over a widget
still sitting on the page — lands in exactly the place the tool has always
landed: the page untouched, a screenshot taken, a captcha task on the task
list with a note saying what was attempted.

No solving models ship with eruser. The solver is a sidecar the user runs —
any service answering a three-field JSON contract — so the models and their
licences stay out of this repository, and a privacy tool does not ship
models nobody audited. Off by default; with the defaults nothing has
changed.

### `web` — "Send to Unsent", and send-all reads the page's filters

A second button starts a run against only the brokers never contacted,
whatever the page's status filter says. Fixing it turned up two old bugs in
the send-all path: the endpoint read its filters from the query string while
the page posted them in the body, so a run silently ignored every filter on
the page; and the JavaScript polled `data.job_id` while the API returns `id`,
so progress never appeared for a run started from the page. Both have tests.

### `cli` — `eruser cleanup-bounces`

Retire broker addresses that no longer accept mail. 764 community-maintained
entries means some are always dead, and every send to one wastes a slot
against the daily limit. Reads the mailbox for delivery failures, matches each
bounced address back to its broker, and takes them out of `brokers.yaml` with
`--remove` — a dry run until then. A bounce that names no address, or one
nobody here uses, is reported rather than silently ignored.

### `inbox` — spotting a delivery failure

`looks_like_a_bounce` judges from the envelope alone: sender and subject, not
the body. Deliberately not the classifier's bounce test — that one weighs the
body and counts `noreply@` as a hint, which is right when reading a broker's
reply and wrong here, because it would condemn nearly every broker autoreply
and this decides what gets deleted.

### `cli` — `eruser monitor --watch`

Read the mailbox on a timer instead of once. Upstream's Go held an IMAP IDLE
connection open and never reconnected, so its watch quietly stopped working
once the server dropped the idle connection after about half an hour. Each
pass here connects, reads, and disconnects, so a dropped connection, a laptop
waking from sleep, or a provider restart costs one cycle instead of ending the
watch. A failed pass is reported and retried; the interval defaults to five
minutes, since brokers answer over days and every check is a login the
provider counts.

### `web` — a page for the people on this instance

Lists everyone, adds someone, changes your own password. A household rather
than an organisation: anyone can add a person, the account that claimed the
instance can remove people, and there are no roles beyond that.

### settings live in the database, one set per person

`CurrentUser` carries them, read once per request by the middleware, so a
page and the API it calls cannot disagree about what is configured. An
existing `config.yaml` is imported the first time it is seen; after that
editing the database is not undone by a stale file.

### `web` — a page for the mailboxes requests are sent from

What each account has left today, and how much can be sent across all of
them. An account is personal unless it is marked as the family's, since
sharing one lets someone else send mail as its owner. The provider table
moved to `email::providers`, so the CLI, the wizard and the page agree on
which SMTP server an address implies rather than each hardcoding Gmail.

### `cli` — `eruser users` and `eruser accounts`

`users` exists mostly so a forgotten password is recoverable: there is no
reset by email, because the only mailbox eruser knows is the one it sends
from. `accounts` registers the mailboxes to send through. Secrets are
prompted for, never taken from a flag.

### `web` — the interface is behind a sign-in

`CurrentUser` is an extractor, so a handler that forgets to check who is
asking does not compile. An instance nobody has claimed goes to `/first-run`,
which takes over the seeded user row rather than creating a second account
beside it — an upgrade keeps its history.

### `send` — several sending accounts per run

A pool takes an account's allowance before each send and rolls over when one
is spent, so three mailboxes are three times the daily cap rather than one.

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
