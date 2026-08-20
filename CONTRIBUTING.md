# Contributing

The rewrite is in progress and the port was AI-written, so the most valuable
thing you can do right now is look at it critically.

## What helps most

**Broker database entries.** `data/brokers.yaml` is the heart of the project.
Adding a broker, correcting an address that bounces, or filling in an
`opt_out_url` is a real improvement and needs no Rust.

**Bugs in the port.** The Rust was translated from Go by an AI. It has tests
and it passes them, but tests only cover what someone thought to check. If
something behaves differently from the original, that's worth an issue.

**Template wording.** The GDPR, CCPA, and generic requests in
`templates/email/` are what brokers actually read. Better-phrased requests get
better compliance rates. These are plain text files — no recompile needed to
try a change.

**Documentation.** Corrections, clearer examples, anything that was confusing.

## Adding a broker

```yaml
- id: example-broker          # lowercase, hyphenated, unique
  name: Example Broker
  email: privacy@example.com  # required
  website: https://example.com
  opt_out_url: https://example.com/optout
  region: us                  # us, eu, or global
  category: people-search     # people-search, marketing, background-check
```

Or interactively:

```bash
cargo run -- --brokers data/brokers.yaml add-broker
```

A test validates the whole database on every push: unique ids, non-empty
names, and addresses that at least look like addresses.

## Code

```bash
cargo test                                        # everything
cargo clippy --all-targets -- -D warnings         # what CI runs
cargo fmt --all                                   # before committing
```

If you're working on the port itself, read [docs/PORTING.md](docs/PORTING.md)
first. It documents the conventions the existing modules were written against
and the Go patterns that need rethinking rather than translating.

Every module lands with tests in the same commit. Name tests after the
behaviour they pin, not the function they call.

## Commit messages

Conventional prefixes (`feat`, `fix`, `docs`, `chore`, `refactor`, `test`)
with the module in parentheses:

```
feat(broker): add 40 brokers from the state registry
fix(email): reject recipient addresses containing a semicolon
```

For port work, the body should say what changed relative to the Go original
and why. Those messages are the record of what this rewrite decided.

## Pull requests

Small and focused. One concern per PR. If it fixes something, a test that
would have failed before the fix is the most persuasive thing you can include.

## Security

Don't open a public issue for a vulnerability. This project handles home
addresses and email credentials. Use GitHub's private advisory reporting on
the Security tab.
