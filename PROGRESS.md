# Port progress

Running log of the Go → Rust port. Ordered newest first. Every entry
corresponds to a commit on `main`.

**Status: the port is complete, and the fork has started diverging.** Every
module in upstream eraser has a Rust counterpart. 924 tests passing.

Work since the port: several people on one instance, each with their own
sign-in, settings and mailboxes, several sending accounts per person so a run
is not capped by one provider's daily limit, and a reply pipeline whose
wording is picked by a schema-constrained model rather than written by one.

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

### `automation` — solvers by kind, with fallback

One sidecar was never going to be enough. A picture grid needs a vision model
and an invisible challenge needs a token service, and the person setting this
up may want one of each — or the same service twice with different settings.
`captcha_solver.solvers` is now a list, each entry naming the `kind` it is
asked about (an entry that names none is asked about everything), tried in the
order written.

The rule the module was built around does not bend because there are more
solvers: a solver is an optimisation, never a gate. The first entry that
reports a solve ends the attempt, and a failure does not — a solver that could
not clear the challenge, or that is not running at all, hands over to the next
one. What comes back when nobody manages it prefers a real failure to an
unreachable sidecar, because "the widget did not move" is worth reading and
"connection refused" is not. The page is re-read afterwards either way, so a
solver's claim is still never the evidence.

The old single `url`/`token` still works: it is the shorthand for one solver
asked about every kind. A config with both is a mistake worth saying out loud,
and the list wins — it is the one that can express more.

### `decision` and `reply` — the reply library, and a model that only chooses

The AI response pipeline's next instalment, and a change of mind about how it
should work. Generating a paragraph and then checking it for promises is a lot
of machinery to arrive at "please go ahead with the deletion". The wording
brokers need is short, repetitive, and known in advance, so it now ships
written: seven replies under `templates/replies/`, beside the request letters,
with the person's own details filled in.

What is left is *choosing*, and that is a `choice` question — which is what a
System One model answers. `decision` speaks Jev's own API
(`POST /v1/systemone`); the endpoint may be a host, a base URL, or the full
path, and the path is completed for you. Nothing in the module is on by
default.

This is the part worth dwelling on. The answer to a closed question is one of
the labels eruser wrote, so there is no prose to parse back into a category, no
category that was never on the list, and nothing a broker's email can talk the
model into: an instruction hidden in a quoted reply has nowhere to land. A
model that answers with a label nobody offered has produced an unusable answer,
which is treated as no answer at all.

Three things use it, each switched on separately and each falling back to what
the tool did before:

- **Routing** (`decider.route`) — which reply an email deserves. The rule table
  stays the default: an unfinished reply gets the missing-information reply.
  What the table cannot read is the other two, and deliberately so — the
  patterns cannot tell "click this link to confirm" from "reply to this email
  to confirm", and the first of those is `confirm`'s job, not a reply's. Those
  are exactly the readings a person supplies.
- **Filing** (`decider.classify`) — the replies the patterns could not place.
  Runs as part of `monitor`, after `monitor --reclassify`, and from the web's
  Reclassify button, which now reports what it filed as well as what it
  changed. It offers only
  the verdicts that need no URL to act on: `form_required` and
  `confirmation_required` come from the page rather than the email, so filing
  one without its URL would put a task on the list that cannot be carried out.
  A reply the model cannot place stays exactly where it was.
- **Wording** (`ai.wording`) — `canned`, the shipped reply that fits best, or
  `generated`, a model writing one as before. `canned` needs no model anywhere;
  `generated` keeps the validator it always had.

Every answer below `decider.min_confidence` is thrown away and the rule table
stands, so a model that is barely better than guessing never acts on anyone's
behalf. `eruser init` now asks about both halves and writes what was chosen,
offering Jev's endpoint while saying plainly that eruser installs no models and
ships none.

The state a model is shown is built by `decision::state_of`, quoted as data and
cut at 4000 characters with a marker — a broker's reply is a short answer on
top of a quoted thread.

The request shape is the API's own, checked field by field against the
published reference and pinned by a test that runs the real client against a
socket: the path, the bearer key, and a `choice` question whose `criteria` are
the labels it may answer with. Rate limits are the one thing the reference asks
for that is easy to forget, so `429` and `529` are retried twice with a growing
delay before the answer is given up on — a decision that arrives late is worth
more than one that does not, but not at the price of holding a scan open. Every
other status is reported once and the rules decide.

### `web` — the terminal interface

A full re-skin from the dark dashboard to a keyboard-driven terminal design:
IBM Plex Mono chrome, serif letters, one accent, a status bar, and numbered
tabs (1 mail · 2 run · 3 captchas · 4 letters · 5 sending; `w` starts a run).
Both fonts are vendored, so the page still loads with no network and reaches
no third party — the CSP and asset tests still pin that.

The mail view is the home page. It is not a mailbox but a view over what
eruser already knows: pending tasks land in `needs-you`, replies the
classifier could not place in `check`, and the run's history in `waiting`,
`removed`, and `no-record`. Reading an unsure reply shows its best guess and
confidence, and filing it (`y` / `w`) goes through the same classification
update a reclassify uses, so a human's ruling leaves the same marks a rule's
would.

The run wizard replaces the dashboard's send form: profile, recipients
(all / never written to / US only / not written to in a while), letter, and
a live progress pane that polls the active job. Two things it adds are real
rather than decorative — `auto` resolves the best-fit letter per broker
(GDPR where the broker is EU-based, CCPA for US, generic otherwise) through
one function shared by page and pipeline, and `stale_days` filters on when
the broker was last contacted, with never-contacted brokers always passing.

Letters are now editable from the web. The three shipped templates were
embedded and immutable; an edit stores an override per person in the
database (new migration) and wins until reverted, so a wording fix in a
release still reaches anyone who has not gone out of their way. Subjects are
flattened to one line on the way in — a subject that spans lines is header
injection. Previews render against a real broker from the database, and the
test button sends one to yourself through the ordinary sender.

The sending page gathers what was spread across accounts and settings: the
rotation with per-account usage, the pace, the read-only mailbox, and how
replies are sorted. The old accounts page still exists and links from it.
A stray dead link to `/monitor` was removed rather than made to work — the
monitor is a CLI command, not a page.

The old dashboard is gone; `/` serves the mail view and the dashboard route
redirects there for old bookmarks.

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
